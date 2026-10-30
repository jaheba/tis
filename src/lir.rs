//! Typed, register-based lowering for a future native backend.
//! All speculative exits precede Python-visible effects and resume an unchanged HIR operation.

mod prune;
mod remap;
mod unpack;

use crate::{
    Error, Result,
    hir::{self, BlockId, Constant, CoreOp, ResumePoint, ValueId},
    profile::{FunctionId, ValueKind},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Write,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoweringMode {
    Generic,
    Profiled,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Register(pub usize);
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Type {
    Boxed,
    FloatView,
    I64,
    F64,
    Bool,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct StateId(pub usize);
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExitId(pub usize);

// Keep the GC value map and environment mode adjacent in the hot metadata header.
#[repr(C)]
#[derive(Debug, Clone)]
pub struct FrameState {
    pub values: Vec<(ValueId, Register)>,
    /// The current lexical environment is a separate implicit boxed input and GC root.
    pub environment_root: FrameEnvironment,
    pub resume: ResumePoint,
    /// Mutable frame locals stay rooted independently of temporary register liveness.
    pub local_count: usize,
    pub local_values: Vec<Option<Register>>,
}
/// The VM always roots the physical environment; copied bodies add logical frame roots.
/// The non-null Rc lets this replace the old boolean plus padding without growing FrameState.
#[derive(Debug, Clone)]
pub enum FrameEnvironment {
    Current,
    Inline(std::rc::Rc<InlineState>),
}
/// Copied safepoints keep their additional metadata outside ordinary frame states.
#[derive(Debug, Clone)]
pub struct InlineState {
    pub snapshot: std::rc::Rc<crate::inline_frames::Snapshot>,
    pub roots: Vec<Register>,
    pub gc_roots: Vec<Register>,
}
impl InlineState {
    pub fn new(
        snapshot: std::rc::Rc<crate::inline_frames::Snapshot>,
        registers: &[Type],
    ) -> Result<Self> {
        let roots = snapshot.roots(registers)?;
        Ok(Self {
            snapshot,
            gc_roots: roots.clone(),
            roots,
        })
    }
}
impl FrameState {
    pub fn inline(&self) -> Option<&InlineState> {
        match &self.environment_root {
            FrameEnvironment::Current => None,
            FrameEnvironment::Inline(state) => Some(state),
        }
    }
    pub fn inline_roots(&self) -> &[Register] {
        self.inline().map_or(&[], |state| state.roots.as_slice())
    }
    pub fn inline_gc_roots(&self) -> &[Register] {
        self.inline().map_or(&[], |state| state.gc_roots.as_slice())
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ExitReason {
    TypeGuard,
    IntegerOverflow,
    FloatArithmetic,
    SequenceGuard,
}
#[derive(Debug, Clone)]
pub struct SideExit {
    /// Logical caller-to-callee frames; root publication uses the enclosing physical state.
    pub inline: Option<std::rc::Rc<crate::inline_frames::Snapshot>>,
    pub state: StateId,
    pub reason: ExitReason,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Effects {
    pub may_raise: bool,
    /// May allocate Python heap objects; Rust scratch allocations cannot trigger tracing GC.
    pub may_allocate: bool,
    pub may_call: bool,
    pub may_write: bool,
}
impl Effects {
    pub const READ_ONLY: Self = Self {
        may_raise: true,
        may_allocate: false,
        may_call: false,
        may_write: false,
    };

    pub fn needs_safepoint(self) -> bool {
        self.may_allocate || self.may_call
    }

    fn covers(self, required: Self) -> bool {
        (!required.may_raise || self.may_raise)
            && (!required.may_allocate || self.may_allocate)
            && (!required.may_call || self.may_call)
            && (!required.may_write || self.may_write)
    }
    const CONSERVATIVE: Self = Self {
        may_raise: true,
        may_allocate: true,
        may_call: true,
        may_write: true,
    };
}

/// Helpers consume and return boxed values; environment and root maps are explicit call metadata.
#[derive(Debug, Clone)]
pub enum Helper {
    Scoped(Box<Helper>),
    LeafGuard(std::rc::Rc<crate::leaf_inline::Plan>),
    LeafReceiver,
    LeafEnvironment,
    CurrentEnvironment,
    LeafReturn(std::rc::Rc<crate::leaf_inline::Plan>),
    /// Return accounting for a tuple whose components flow directly to the caller.
    TupleReturn(std::rc::Rc<crate::leaf_inline::Plan>),
    InlineCall {
        body: std::rc::Rc<crate::body_inline::Plan>,
        names: Vec<Option<String>>,
    },
    StringConstant(crate::StringLiteral),
    GetAttr(crate::Attribute),
    SetAttr(crate::Attribute),
    LoadName(crate::Name),
    StoreName(crate::Name),
    Add,
    Subtract,
    Less,
    List,
    Tuple,
    Dict,
    GetItem,
    SetItem,
    Len,
    /// A guarded no-GC scalar probe with general positional-call fallback and full Call effects.
    ScalarCall(ScalarBuiltin),
    /// Exact string iterator probe; exhaustion and other receivers use the original Next helper.
    StringNext,
    Python(CoreOp),
}
pub use crate::execution::ScalarBuiltin;
pub use crate::scalar::FloatOperator;
pub use crate::sequence::SequenceKind;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SequenceIndex {
    Constant(i64),
    Value(Register),
}
#[derive(Debug, Clone)]
pub enum Operation {
    FloatListView {
        value: Register,
        length: Option<usize>,
        failure: ExitId,
    },
    /// The view has already checked exact unpack length; preserve the boxed source for frame snapshots.
    FloatListUnpack {
        value: Register,
        view: Register,
    },
    FloatListLoad {
        view: Register,
        index: i64,
        failure: ExitId,
    },
    FloatListStore {
        view: Register,
        index: i64,
        value: Register,
        failure: ExitId,
    },
    SequenceUnpack {
        value: Register,
        kind: SequenceKind,
        count: usize,
        failure: ExitId,
    },
    SequenceGet {
        value: Register,
        kind: SequenceKind,
        index: SequenceIndex,
        failure: ExitId,
    },
    Constant(Constant),
    /// Preserve a HIR result identity while reusing an already checked boxed value.
    Copy(Register),
    Truth {
        value: Register,
        state: StateId,
        roots: Vec<Register>,
        effects: Effects,
    },
    GuardInt {
        value: Register,
        exit: ExitId,
    },
    GuardFloat {
        value: Register,
        exit: ExitId,
    },
    UnboxFloat(Register),
    IntToFloat(Register),
    FloatBinary {
        operator: FloatOperator,
        left: Register,
        right: Register,
        failure: ExitId,
    },
    /// Compare exact Int/Float values with an integer literal representable exactly in f64.
    NumberCompare {
        value: Register,
        integer: i64,
        operator: String,
        failure: ExitId,
    },
    FloatCompare {
        operator: String,
        left: Register,
        right: Register,
    },
    BoxFloat(Register),
    BoxBool(Register),
    BoolNot(Register),
    UnboxInt(Register),
    /// Exact Int/Float union, returning Int only for Int+Int. No allocation or Python calls.
    NumericAdd {
        left: Register,
        right: Register,
        guard: ExitId,
        overflow: ExitId,
    },
    CheckedAdd {
        left: Register,
        right: Register,
        overflow: ExitId,
    },
    BoxInt(Register),
    Call {
        helper: Helper,
        arguments: Vec<Register>,
        state: StateId,
        roots: Vec<Register>,
        effects: Effects,
    },
}
#[derive(Debug, Clone)]
pub struct Instruction {
    pub result: Option<Register>,
    pub operation: Operation,
    pub origin: ValueId,
    pub point: ResumePoint,
}
#[derive(Debug, Clone)]
pub struct Edge {
    pub block: BlockId,
    pub arguments: Vec<Register>,
}
#[derive(Debug, Clone)]
pub enum Terminator {
    Return(Register),
    Jump(Edge),
    Branch {
        condition: Register,
        yes: Edge,
        no: Edge,
    },
}
#[derive(Debug, Clone)]
pub struct Block {
    pub parameters: Vec<Register>,
    pub instructions: Vec<Instruction>,
    pub terminator: Terminator,
}
#[derive(Debug, Clone)]
pub struct OsrEntry {
    pub header: BlockId,
    pub state: StateId,
}

#[derive(Debug, Clone)]
pub struct Function {
    pub inline_origins: BTreeMap<Register, (crate::inline_frames::ScopeId, ValueId)>,
    pub inline_scopes: Vec<crate::leaf_inline::Scope>,
    pub scoped_results: BTreeSet<Register>,
    pub shadow_slots: usize,
    pub source: FunctionId,
    pub revision: u64,
    pub registers: Vec<Type>,
    pub blocks: Vec<Block>,
    /// Synthetic inline blocks do not create physical HIR block observations.
    pub block_origins: Vec<Option<BlockId>>,
    pub states: Vec<FrameState>,
    pub exits: Vec<SideExit>,
    pub osr_entries: Vec<OsrEntry>,
    /// Boxed registers correspond directly to interpreter frame slots.
    pub hir_values: Vec<Register>,
    /// Original boxed HIR operands needed when an active artifact resumes observation.
    pub observation_inputs: Vec<Vec<Register>>,
    pub local_count: usize,
    /// Primitive loop parameters and entry-invariant scalars retain boxed sources for OSR reconstruction.
    pub scalar_sources: BTreeMap<Register, Register>,
    pub scalar_entries: BTreeMap<BlockId, ExitId>,
    pub regions: Vec<crate::region::LoopRegion>,
    pub nest: Option<crate::region::nest::LoopNest>,
}

impl Function {
    /// Copied caches are initialized by their own guards, not by a physical caller OSR entry.
    pub(crate) fn scalar_imports(&self) -> impl Iterator<Item = (&Register, &Register)> {
        self.scalar_sources.iter().filter(|(raw, _)| {
            !self
                .inline_scopes
                .iter()
                .any(|scope| scope.registers.contains(&raw.0))
        })
    }

    pub(crate) fn is_scoped_instruction(&self, instruction: &Instruction) -> bool {
        instruction
            .result
            .is_some_and(|result| self.scoped_results.contains(&result))
            || match &instruction.operation {
                Operation::NumberCompare { failure, .. } => self.exits[failure.0].inline.is_some(),
                Operation::Truth { state, .. } => self.states[state.0].inline().is_some(),
                _ => false,
            }
    }

    pub fn inline_roots(&self, id: StateId) -> &[Register] {
        self.states.get(id.0).map_or(&[], FrameState::inline_roots)
    }
    pub fn inline_gc_roots(&self, id: StateId) -> &[Register] {
        self.states
            .get(id.0)
            .map_or(&[], FrameState::inline_gc_roots)
    }
    // These producers always return immediate values. Feedback alone cannot prove that a
    // boxed register is safe to omit, and native float recipes may not yet have a boxed slot.
    fn primitive_registers(&self) -> BTreeSet<Register> {
        self.blocks
            .iter()
            .flat_map(|block| &block.instructions)
            .filter_map(|instruction| {
                let primitive = matches!(
                    instruction.operation,
                    Operation::BoxFloat(_)
                        | Operation::BoxInt(_)
                        | Operation::BoxBool(_)
                        | Operation::NumericAdd { .. }
                        | Operation::Constant(
                            Constant::None
                                | Constant::Unbound
                                | Constant::Bool(_)
                                | Constant::Int(_)
                                | Constant::Float(_)
                        )
                );
                primitive.then_some(instruction.result).flatten()
            })
            .collect()
    }
    fn refresh_inline_gc_roots(&mut self) {
        let primitives = self.primitive_registers();
        for state in &mut self.states {
            let FrameEnvironment::Inline(state) = &mut state.environment_root else {
                continue;
            };
            let state = std::rc::Rc::make_mut(state);
            state.gc_roots = state
                .roots
                .iter()
                .copied()
                .filter(|register| !primitives.contains(register))
                .collect();
        }
    }
    pub fn lower(hir: &hir::Function, mode: LoweringMode) -> Result<Self> {
        Self::lower_with_inline(hir, mode, true, false)
    }

    /// Experimental bounded multi-block incorporation using the same verifier and backends.
    pub fn lower_with_cfg(hir: &hir::Function, mode: LoweringMode) -> Result<Self> {
        Self::lower_with_inline(hir, mode, true, true)
    }

    pub(crate) fn lower_with_inline(
        hir: &hir::Function,
        mode: LoweringMode,
        allow_inline: bool,
        allow_cfg: bool,
    ) -> Result<Self> {
        let analysis = hir.analyze()?;
        let profile = hir.profile();
        let mut output = Self {
            inline_origins: BTreeMap::new(),
            inline_scopes: vec![],
            scoped_results: BTreeSet::new(),
            shadow_slots: 0,
            source: hir.id(),
            revision: hir.revision(),
            registers: vec![Type::Boxed; hir.value_count],
            blocks: vec![],
            block_origins: (0..hir.blocks.len()).map(|i| Some(BlockId(i))).collect(),
            states: vec![],
            exits: vec![],
            osr_entries: vec![],
            hir_values: (0..hir.value_count).map(Register).collect(),
            observation_inputs: vec![vec![]; hir.value_count],
            local_count: hir.local_slots.len(),
            scalar_sources: BTreeMap::new(),
            scalar_entries: BTreeMap::new(),
            regions: vec![],
            nest: None,
        };
        let integer_constants = hir
            .blocks
            .iter()
            .flat_map(|b| &b.instructions)
            .filter_map(|i| {
                if let hir::Op::Constant(Constant::Int(value)) = i.op {
                    Some((i.result, value))
                } else {
                    None
                }
            })
            .collect::<BTreeMap<_, _>>();
        let mut float_values = BTreeSet::new();
        let mut float_params = BTreeSet::new();
        if mode == LoweringMode::Profiled {
            for (block_id, body) in hir.blocks.iter().enumerate() {
                if block_id != 0 {
                    float_params.extend(body.parameters.iter().copied());
                }
                for instruction in &body.instructions {
                    if matches!(instruction.op, hir::Op::Constant(Constant::Float(_)))
                        || profile.site(instruction.result).is_some_and(|s| {
                            s.executions > 0 && s.result.monomorphic() == Some(ValueKind::Float)
                        })
                    {
                        float_values.insert(instruction.result);
                    }
                }
            }
            loop {
                let before = float_params.clone();
                for body in &hir.blocks {
                    let edges: Vec<_> = match body.terminator.as_ref().unwrap() {
                        hir::Terminator::Jump(e) => vec![e],
                        hir::Terminator::Branch { yes, no, .. } => vec![yes, no],
                        _ => vec![],
                    };
                    for edge in edges {
                        for (&source, &target) in edge
                            .arguments
                            .iter()
                            .zip(&hir.blocks[edge.block.0].parameters)
                        {
                            if !float_values.contains(&source) && !float_params.contains(&source) {
                                float_params.remove(&target);
                            }
                        }
                    }
                }
                if before == float_params {
                    break;
                }
            }
        }
        let mut typed_params = BTreeMap::new();
        for value in &float_params {
            let scalar = output.register(Type::F64);
            typed_params.insert(*value, scalar);
            output.scalar_sources.insert(scalar, Register(value.0));
        }
        let mut globals = BTreeMap::new();
        for (block_id, body) in hir.blocks.iter().enumerate() {
            let mut instructions = vec![];
            let mut sequences = BTreeMap::<hir::ValueId, SequenceKind>::new();
            let mut views = BTreeMap::<hir::ValueId, Register>::new();
            let mut float_unpacks = BTreeMap::<hir::ValueId, Register>::new();
            // Scalar SSA values survive helper calls; their boxed shadows still support frame reconstruction.
            let mut scalars = globals.clone();
            scalars.extend(
                typed_params
                    .iter()
                    .map(|(v, r)| (*v, (ValueKind::Float, *r))),
            );
            for (index, instruction) in body.instructions.iter().enumerate() {
                let origin = instruction.result;
                output.observation_inputs[origin.0] = instruction
                    .op
                    .inputs()
                    .into_iter()
                    .map(|v| Register(v.0))
                    .collect();
                let result = Register(origin.0);
                let state = output.state(
                    hir,
                    ResumePoint {
                        block: BlockId(block_id),
                        instruction: index,
                    },
                    &analysis.live_before[block_id][index],
                );
                let emit = |instructions: &mut Vec<Instruction>, result, operation| {
                    instructions.push(Instruction {
                        result,
                        operation,
                        origin,
                        point: ResumePoint {
                            block: BlockId(block_id),
                            instruction: index,
                        },
                    })
                };
                if let hir::Op::Runtime(CoreOp::Unary(op), inputs) = &instruction.op
                    && op == "-"
                    && inputs.len() == 1
                    && let Some(value) = hir
                        .blocks
                        .iter()
                        .flat_map(|b| &b.instructions)
                        .find(|i| i.result == inputs[0])
                    && let hir::Op::Constant(Constant::Float(value)) = value.op
                {
                    emit(
                        &mut instructions,
                        Some(result),
                        Operation::Constant(Constant::Float(-value)),
                    );
                    continue;
                }
                if let hir::Op::Runtime(CoreOp::Unary(op), inputs) = &instruction.op
                    && op == "not"
                {
                    // Truth retains the original error point and heap fallback; inversion is typed and pure.
                    views.clear();
                    let truth = output.register(Type::Bool);
                    emit(
                        &mut instructions,
                        Some(truth),
                        Operation::Truth {
                            value: Register(inputs[0].0),
                            state,
                            roots: output.states[state.0]
                                .values
                                .iter()
                                .map(|(_, register)| *register)
                                .collect(),
                            effects: Effects::CONSERVATIVE,
                        },
                    );
                    let inverted = output.register(Type::Bool);
                    emit(&mut instructions, Some(inverted), Operation::BoolNot(truth));
                    emit(
                        &mut instructions,
                        Some(result),
                        Operation::BoxBool(inverted),
                    );
                    continue;
                }
                if let hir::Op::Runtime(CoreOp::Unpack(count), inputs) = &instruction.op
                    && mode == LoweringMode::Profiled
                    && *count > 0
                    && unpack_bundle(hir, block_id, index, *count)
                    && profile.site(origin).is_some_and(|site| {
                        site.executions >= hir::AddFeedback::WARMUP as u64
                            && site.operands.first().and_then(|p| p.monomorphic())
                                == Some(ValueKind::List)
                    })
                    && body.instructions[index + 1..index + 1 + count]
                        .iter()
                        .all(|item| {
                            profile.site(item.result).is_some_and(|site| {
                                site.executions >= hir::AddFeedback::WARMUP as u64
                                    && site.result.monomorphic() == Some(ValueKind::Float)
                            })
                        })
                {
                    let view = output.register(Type::FloatView);
                    let failure = output.exit(state, ExitReason::SequenceGuard);
                    emit(
                        &mut instructions,
                        Some(view),
                        Operation::FloatListView {
                            value: Register(inputs[0].0),
                            length: Some(*count),
                            failure,
                        },
                    );
                    emit(
                        &mut instructions,
                        Some(result),
                        Operation::FloatListUnpack {
                            value: Register(inputs[0].0),
                            view,
                        },
                    );
                    views.clear();
                    views.insert(inputs[0], view);
                    views.insert(origin, view);
                    float_unpacks.insert(origin, view);
                    continue;
                }
                if let hir::Op::Runtime(CoreOp::UnpackItem(index), inputs) = &instruction.op
                    && let Some(&view) = float_unpacks.get(&inputs[0])
                    && views.get(&inputs[0]) == Some(&view)
                {
                    let failure = output.exit(state, ExitReason::SequenceGuard);
                    emit(
                        &mut instructions,
                        Some(result),
                        Operation::FloatListLoad {
                            view,
                            index: i64::try_from(*index)
                                .map_err(|_| Error::InvalidIr("unpack index"))?,
                            failure,
                        },
                    );
                    let scalar = output.register(Type::F64);
                    emit(
                        &mut instructions,
                        Some(scalar),
                        Operation::UnboxFloat(result),
                    );
                    scalars.insert(origin, (ValueKind::Float, scalar));
                    continue;
                }
                let typed = match &instruction.op {
                    hir::Op::GetItem(source, key) => Some((*source, *key, None)),
                    hir::Op::SetItem { object, key, value } => Some((*object, *key, Some(*value))),
                    _ => None,
                }
                .and_then(|(source, key, value)| {
                    let site = profile.site(origin)?;
                    if mode != LoweringMode::Profiled
                        || site.executions < hir::AddFeedback::WARMUP as u64
                        || site.operands.first()?.monomorphic() != Some(ValueKind::List)
                    {
                        return None;
                    }
                    let hir::Op::Constant(Constant::Int(index)) = hir.instruction(key)?.op else {
                        return None;
                    };
                    let float = if value.is_some() {
                        site.operands.get(2)?.monomorphic()
                    } else {
                        site.result.monomorphic()
                    };
                    (float == Some(ValueKind::Float)).then_some((source, index, value))
                });
                if let Some((source, index, value)) = typed {
                    let failure = output.exit(state, ExitReason::SequenceGuard);
                    let view = if let Some(view) = views.get(&source) {
                        *view
                    } else {
                        views.clear();
                        let view = output.register(Type::FloatView);
                        emit(
                            &mut instructions,
                            Some(view),
                            Operation::FloatListView {
                                value: Register(source.0),
                                length: None,
                                failure,
                            },
                        );
                        views.insert(source, view);
                        view
                    };
                    if let Some(value) = value {
                        let scalar = if let Some(&(ValueKind::Float, reg)) = scalars.get(&value) {
                            reg
                        } else {
                            let guard = output.exit(state, ExitReason::TypeGuard);
                            emit(
                                &mut instructions,
                                None,
                                Operation::GuardFloat {
                                    value: Register(value.0),
                                    exit: guard,
                                },
                            );
                            let scalar = output.register(Type::F64);
                            emit(
                                &mut instructions,
                                Some(scalar),
                                Operation::UnboxFloat(Register(value.0)),
                            );
                            scalars.insert(value, (ValueKind::Float, scalar));
                            scalar
                        };
                        emit(
                            &mut instructions,
                            Some(result),
                            Operation::FloatListStore {
                                view,
                                index,
                                value: scalar,
                                failure,
                            },
                        );
                    } else {
                        emit(
                            &mut instructions,
                            Some(result),
                            Operation::FloatListLoad {
                                view,
                                index,
                                failure,
                            },
                        );

                        let scalar = output.register(Type::F64);
                        emit(
                            &mut instructions,
                            Some(scalar),
                            Operation::UnboxFloat(result),
                        );
                        scalars.insert(origin, (ValueKind::Float, scalar));
                    }
                    continue;
                }
                let integer_add = mode == LoweringMode::Profiled
                    && matches!(instruction.op, hir::Op::Add(..))
                    && profile.site(origin).is_some_and(|site| {
                        site.executions >= hir::AddFeedback::WARMUP as u64
                            && site.operands.len() == 2
                            && site
                                .operands
                                .iter()
                                .all(|operand| operand.monomorphic() == Some(ValueKind::Int))
                    });
                let float_signature = profile
                    .site(origin)
                    .filter(|site| {
                        mode == LoweringMode::Profiled
                            && site.executions >= hir::AddFeedback::WARMUP as u64
                            && site.operands.len() == 2
                    })
                    .and_then(|site| {
                        let left = site.operands[0].monomorphic()?;
                        let right = site.operands[1].monomorphic()?;
                        if [left, right]
                            .iter()
                            .all(|kind| matches!(kind, ValueKind::Int | ValueKind::Float))
                            && [left, right].contains(&ValueKind::Float)
                        {
                            Some([left, right])
                        } else {
                            None
                        }
                    });
                let float_operator = match &instruction.op {
                    hir::Op::Add(..) => Some(FloatOperator::Add),
                    hir::Op::Subtract(..) => Some(FloatOperator::Subtract),
                    hir::Op::Runtime(CoreOp::Binary(op) | CoreOp::Inplace(op), _) => {
                        match op.as_str() {
                            "+" => Some(FloatOperator::Add),
                            "-" => Some(FloatOperator::Subtract),
                            "*" => Some(FloatOperator::Multiply),
                            "/" => Some(FloatOperator::Divide),
                            "**" => Some(FloatOperator::Power),
                            _ => None,
                        }
                    }
                    _ => None,
                };
                let float_compare = match &instruction.op {
                    hir::Op::Runtime(CoreOp::Compare(op), _)
                        if ["==", "!=", "<", "<=", ">", ">="].contains(&op.as_str()) =>
                    {
                        Some(op.clone())
                    }
                    _ => None,
                };
                let number_compare = float_compare.as_ref().and_then(|operator| {
                    if mode != LoweringMode::Profiled {
                        return None;
                    }
                    let inputs = instruction.op.inputs();
                    let site = profile.site(origin)?;
                    if site.executions < hir::AddFeedback::WARMUP as u64 || site.operands.len() != 2
                    {
                        return None;
                    }
                    let (value_index, integer, operator) =
                        if let Some(&integer) = integer_constants.get(&inputs[1]) {
                            (0, integer, operator.clone())
                        } else if let Some(&integer) = integer_constants.get(&inputs[0]) {
                            let reversed = match operator.as_str() {
                                "<" => ">",
                                "<=" => ">=",
                                ">" => "<",
                                ">=" => "<=",
                                other => other,
                            };
                            (1, integer, reversed.to_owned())
                        } else {
                            return None;
                        };
                    let operand = &site.operands[value_index];
                    if !exact_float_integer(integer)
                        || operand.observations() == 0
                        || operand.observations()
                            != operand
                                .count(ValueKind::Int)
                                .saturating_add(operand.count(ValueKind::Float))
                    {
                        return None;
                    }
                    Some((Register(inputs[value_index].0), integer, operator))
                });
                let sequence_kind = |input: hir::ValueId| {
                    if let Some(kind) = sequences.get(&input) {
                        return Some(*kind);
                    }
                    profile
                        .site(origin)
                        .filter(|site| {
                            mode == LoweringMode::Profiled
                                && site.executions >= hir::AddFeedback::WARMUP as u64
                        })
                        .and_then(|site| site.operands.first()?.monomorphic())
                        .and_then(|kind| match kind {
                            ValueKind::List => Some(SequenceKind::List),
                            ValueKind::Tuple => Some(SequenceKind::Tuple),
                            _ => None,
                        })
                };
                let specialized = match &instruction.op {
                    hir::Op::Runtime(CoreOp::Unpack(count), inputs)
                        if unpack_bundle(hir, block_id, index, *count) =>
                    {
                        sequence_kind(inputs[0]).map(|kind| Operation::SequenceUnpack {
                            value: Register(inputs[0].0),
                            kind,
                            count: *count,
                            failure: output.exit(state, ExitReason::SequenceGuard),
                        })
                    }
                    hir::Op::Runtime(CoreOp::UnpackItem(index), inputs) => sequence_kind(inputs[0])
                        .map(|kind| Operation::SequenceGet {
                            value: Register(inputs[0].0),
                            kind,
                            index: SequenceIndex::Constant(*index as i64),
                            failure: output.exit(state, ExitReason::SequenceGuard),
                        }),
                    hir::Op::GetItem(value, index) => {
                        sequence_kind(*value).map(|kind| Operation::SequenceGet {
                            value: Register(value.0),
                            kind,
                            index: SequenceIndex::Value(Register(index.0)),
                            failure: output.exit(state, ExitReason::SequenceGuard),
                        })
                    }
                    _ => None,
                };
                if let Some(operation) = specialized {
                    views.clear();
                    if let Operation::SequenceUnpack { kind, .. } = operation {
                        sequences.insert(origin, kind);
                    }
                    emit(&mut instructions, Some(result), operation);
                } else if let Some((value, integer, operator)) = number_compare {
                    let raw = output.register(Type::Bool);
                    emit(
                        &mut instructions,
                        Some(raw),
                        Operation::NumberCompare {
                            value,
                            integer,
                            operator,
                            failure: output.exit(state, ExitReason::TypeGuard),
                        },
                    );
                    emit(&mut instructions, Some(result), Operation::BoxBool(raw));
                } else if let Some(signature) = float_signature
                    && (float_operator.is_some()
                        || (float_compare.is_some()
                            && signature == [ValueKind::Float, ValueKind::Float]))
                {
                    let guard = output.exit(state, ExitReason::TypeGuard);
                    let inputs = instruction.op.inputs();
                    let mut args = vec![];
                    for (input, kind) in inputs.into_iter().zip(signature) {
                        if let Some(&(known, scalar)) = scalars.get(&input)
                            && known == kind
                        {
                            if kind == ValueKind::Float {
                                args.push(scalar);
                            } else {
                                let reg = output.register(Type::F64);
                                emit(&mut instructions, Some(reg), Operation::IntToFloat(scalar));
                                args.push(reg);
                            }
                            continue;
                        }
                        let source = input;
                        let input = Register(input.0);
                        if kind == ValueKind::Float {
                            emit(
                                &mut instructions,
                                None,
                                Operation::GuardFloat {
                                    value: input,
                                    exit: guard,
                                },
                            );
                            let reg = output.register(Type::F64);
                            emit(&mut instructions, Some(reg), Operation::UnboxFloat(input));
                            scalars.insert(source, (ValueKind::Float, reg));
                            args.push(reg);
                        } else {
                            emit(
                                &mut instructions,
                                None,
                                Operation::GuardInt {
                                    value: input,
                                    exit: guard,
                                },
                            );
                            let int = output.register(Type::I64);
                            emit(&mut instructions, Some(int), Operation::UnboxInt(input));
                            scalars.insert(source, (ValueKind::Int, int));
                            let reg = output.register(Type::F64);
                            emit(&mut instructions, Some(reg), Operation::IntToFloat(int));
                            args.push(reg);
                        }
                    }
                    if let Some(operator) = float_operator {
                        let failure = output.exit(state, ExitReason::FloatArithmetic);
                        let reg = output.register(Type::F64);
                        emit(
                            &mut instructions,
                            Some(reg),
                            Operation::FloatBinary {
                                operator,
                                left: args[0],
                                right: args[1],
                                failure,
                            },
                        );
                        emit(&mut instructions, Some(result), Operation::BoxFloat(reg));
                        scalars.insert(origin, (ValueKind::Float, reg));
                    } else {
                        let reg = output.register(Type::Bool);
                        emit(
                            &mut instructions,
                            Some(reg),
                            Operation::FloatCompare {
                                operator: float_compare.unwrap(),
                                left: args[0],
                                right: args[1],
                            },
                        );
                        emit(&mut instructions, Some(result), Operation::BoxBool(reg));
                    }
                } else if integer_add {
                    let hir::Op::Add(left, right) = instruction.op else {
                        unreachable!()
                    };
                    let guard = output.exit(state, ExitReason::TypeGuard);
                    let overflow = output.exit(state, ExitReason::IntegerOverflow);
                    let mut args = vec![];
                    for input in [left, right] {
                        if let Some(&(ValueKind::Int, scalar)) = scalars.get(&input) {
                            args.push(scalar);
                            continue;
                        }
                        emit(
                            &mut instructions,
                            None,
                            Operation::GuardInt {
                                value: Register(input.0),
                                exit: guard,
                            },
                        );
                        let scalar = output.register(Type::I64);
                        emit(
                            &mut instructions,
                            Some(scalar),
                            Operation::UnboxInt(Register(input.0)),
                        );
                        scalars.insert(input, (ValueKind::Int, scalar));
                        args.push(scalar);
                    }
                    let sum = output.register(Type::I64);
                    emit(
                        &mut instructions,
                        Some(sum),
                        Operation::CheckedAdd {
                            left: args[0],
                            right: args[1],
                            overflow,
                        },
                    );
                    emit(&mut instructions, Some(result), Operation::BoxInt(sum));
                    scalars.insert(origin, (ValueKind::Int, sum));
                } else if float_operator == Some(FloatOperator::Add)
                    && mode == LoweringMode::Profiled
                    && profile.site(origin).is_some_and(|site| {
                        site.executions >= hir::AddFeedback::WARMUP as u64
                            && site.operands.len() == 2
                            && site.operands.iter().all(|operand| {
                                operand.observations() > 0
                                    && operand.observations()
                                        == operand
                                            .count(ValueKind::Int)
                                            .saturating_add(operand.count(ValueKind::Float))
                            })
                    })
                {
                    let inputs = instruction.op.inputs();
                    emit(
                        &mut instructions,
                        Some(result),
                        Operation::NumericAdd {
                            left: Register(inputs[0].0),
                            right: Register(inputs[1].0),
                            guard: output.exit(state, ExitReason::TypeGuard),
                            overflow: output.exit(state, ExitReason::IntegerOverflow),
                        },
                    );
                } else if let hir::Op::Constant(Constant::String(text)) = &instruction.op
                    && text.len() > crate::InlineString::CAPACITY
                {
                    views.clear();
                    let roots = output.states[state.0]
                        .values
                        .iter()
                        .map(|(_, r)| *r)
                        .collect();
                    emit(
                        &mut instructions,
                        Some(result),
                        Operation::Call {
                            helper: Helper::StringConstant(text.clone().into()),
                            arguments: vec![],
                            state,
                            roots,
                            effects: Helper::StringConstant(text.clone().into()).effects(),
                        },
                    );
                } else if let hir::Op::Constant(constant) = &instruction.op {
                    emit(
                        &mut instructions,
                        Some(result),
                        Operation::Constant(constant.clone()),
                    );
                } else {
                    views.clear();
                    let helper = match &instruction.op {
                        hir::Op::Add(..) => Helper::Add,
                        hir::Op::Subtract(..) => Helper::Subtract,
                        hir::Op::Less(..) => Helper::Less,
                        hir::Op::List(_) => Helper::List,
                        hir::Op::Tuple(_) => Helper::Tuple,
                        hir::Op::Dict(_) => Helper::Dict,
                        hir::Op::GetItem(..) => Helper::GetItem,
                        hir::Op::SetItem { .. } => Helper::SetItem,
                        hir::Op::Len(_) => Helper::Len,
                        hir::Op::Runtime(CoreOp::LoadName(name), _) => {
                            Helper::LoadName(name.clone().into())
                        }
                        hir::Op::Runtime(CoreOp::StoreName(name), _) => {
                            Helper::StoreName(name.clone().into())
                        }
                        hir::Op::Runtime(CoreOp::GetAttr(name), _) => {
                            let mut attribute: crate::Attribute = name.clone().into();
                            // Builtin method sites need no machine-code instance probe or heap directory.
                            // A later receiver change remains valid through the general helper fallback.
                            attribute.inline_instances = mode == LoweringMode::Profiled
                                && profile.site(origin).is_some_and(|site| {
                                    site.operands.first().is_some_and(|operand| {
                                        operand.count(ValueKind::Instance) > 0
                                    })
                                });
                            Helper::GetAttr(attribute)
                        }
                        hir::Op::Runtime(CoreOp::SetAttr(name), _) => {
                            Helper::SetAttr(name.clone().into())
                        }
                        hir::Op::Runtime(CoreOp::Call(names), _)
                            if mode == LoweringMode::Profiled
                                && names.iter().all(Option::is_none)
                                && profile.site(origin).is_some_and(|site| {
                                    site.executions >= 8
                                        && site.call_targets.len() == 1
                                        && site.other_targets == 0
                                        && site.inline_body.as_ref().is_some_and(|body| {
                                            body.source != hir.id()
                                                && body.parameters
                                                    == names.len() + usize::from(body.bound)
                                        })
                                }) =>
                        {
                            Helper::InlineCall {
                                body: profile.site(origin).unwrap().inline_body.clone().unwrap(),
                                names: names.clone(),
                            }
                        }
                        hir::Op::Runtime(CoreOp::Call(names), _)
                            if mode == LoweringMode::Profiled
                                && !names.is_empty()
                                && names.iter().all(Option::is_none) =>
                        {
                            let leaf = profile
                                .site(origin)
                                .filter(|site| {
                                    site.executions >= hir::AddFeedback::WARMUP as u64
                                        && site.other_targets == 0
                                        && site.call_targets.len() == 1
                                        && site
                                            .operands
                                            .first()
                                            .and_then(|operand| operand.monomorphic())
                                            == Some(ValueKind::Native)
                                })
                                .and_then(|site| {
                                    if let crate::profile::CallTarget::Native(name) =
                                        site.call_targets.keys().next()?
                                    {
                                        ScalarBuiltin::from_name(name)
                                    } else {
                                        None
                                    }
                                });
                            leaf.filter(|builtin| builtin.accepts(names.len()))
                                .map(Helper::ScalarCall)
                                .unwrap_or_else(|| Helper::Python(CoreOp::Call(names.clone())))
                        }
                        hir::Op::Runtime(CoreOp::Next, inputs)
                            if mode == LoweringMode::Profiled
                                && inputs.len() == 1
                                && hir.instruction(inputs[0]).is_some_and(|iterator| {
                                    matches!(iterator.op, hir::Op::Runtime(CoreOp::Iter, _))
                                        && (profile
                                            .site(inputs[0])
                                            .and_then(|site| site.operands.first())
                                            .and_then(|operand| operand.monomorphic())
                                            == Some(ValueKind::String)
                                            || profile
                                                .site(origin)
                                                .and_then(|site| site.result.monomorphic())
                                                == Some(ValueKind::String))
                                }) =>
                        {
                            Helper::StringNext
                        }
                        hir::Op::Runtime(op, _) => Helper::Python(op.clone()),
                        hir::Op::Constant(_) => unreachable!(),
                    };
                    let arguments = instruction
                        .op
                        .inputs()
                        .into_iter()
                        .map(|id| Register(id.0))
                        .collect();
                    let effects = helper.effects();
                    let roots = if effects.needs_safepoint() {
                        output.states[state.0]
                            .values
                            .iter()
                            .map(|(_, register)| *register)
                            .collect()
                    } else {
                        Vec::new()
                    };
                    emit(
                        &mut instructions,
                        Some(result),
                        Operation::Call {
                            helper,
                            arguments,
                            state,
                            roots,
                            effects,
                        },
                    );
                }
                if block_id == 0
                    && float_values.contains(&origin)
                    && !scalars.contains_key(&origin)
                    && matches!(
                        instruction.op,
                        hir::Op::Constant(_) | hir::Op::Runtime(CoreOp::LoadLocal(_), _)
                    )
                {
                    let live = if index + 1 < body.instructions.len() {
                        &analysis.live_before[block_id][index + 1]
                    } else {
                        &analysis.live_terminator[block_id]
                    };
                    if live.contains(&origin) {
                        let point = ResumePoint {
                            block: BlockId(block_id),
                            instruction: index + 1,
                        };
                        let state = output.state(hir, point, live);
                        let guard = output.exit(state, ExitReason::TypeGuard);
                        let scalar = output.register(Type::F64);
                        instructions.push(Instruction {
                            result: None,
                            operation: Operation::GuardFloat {
                                value: result,
                                exit: guard,
                            },
                            origin,
                            point,
                        });
                        instructions.push(Instruction {
                            result: Some(scalar),
                            operation: Operation::UnboxFloat(result),
                            origin,
                            point,
                        });
                        scalars.insert(origin, (ValueKind::Float, scalar));
                        globals.insert(origin, (ValueKind::Float, scalar));
                        output.scalar_sources.insert(scalar, result);
                    }
                }
                if matches!(
                    instruction.op,
                    hir::Op::Runtime(CoreOp::Unpack(_) | CoreOp::UnpackStar(..), _)
                ) {
                    sequences.entry(origin).or_insert(SequenceKind::Tuple);
                }
            }
            let point = ResumePoint {
                block: BlockId(block_id),
                instruction: body.instructions.len(),
            };
            let mut edge_scalars = BTreeMap::new();
            let outgoing: Vec<_> = match body.terminator.as_ref().unwrap() {
                hir::Terminator::Jump(e) => vec![e],
                hir::Terminator::Branch { yes, no, .. } => vec![yes, no],
                _ => vec![],
            };
            for edge in outgoing {
                for (&source, target) in edge
                    .arguments
                    .iter()
                    .zip(&hir.blocks[edge.block.0].parameters)
                {
                    if !typed_params.contains_key(target) {
                        continue;
                    }
                    let scalar = if let Some(&(ValueKind::Float, r)) = scalars.get(&source) {
                        r
                    } else {
                        let state = output.state(hir, point, &analysis.live_terminator[block_id]);
                        let failure = output.exit(state, ExitReason::TypeGuard);
                        let scalar = output.register(Type::F64);
                        instructions.push(Instruction {
                            result: None,
                            operation: Operation::GuardFloat {
                                value: Register(source.0),
                                exit: failure,
                            },
                            origin: source,
                            point,
                        });
                        instructions.push(Instruction {
                            result: Some(scalar),
                            operation: Operation::UnboxFloat(Register(source.0)),
                            origin: source,
                            point,
                        });
                        scalars.insert(source, (ValueKind::Float, scalar));
                        scalar
                    };
                    edge_scalars.insert(source, scalar);
                }
            }
            let edge = |edge: &hir::Edge| {
                let mut arguments: Vec<_> =
                    edge.arguments.iter().map(|id| Register(id.0)).collect();
                arguments.extend(
                    edge.arguments
                        .iter()
                        .zip(&hir.blocks[edge.block.0].parameters)
                        .filter(|(_, target)| typed_params.contains_key(target))
                        .map(|(source, _)| edge_scalars[source]),
                );
                Edge {
                    block: edge.block,
                    arguments,
                }
            };
            let terminator = match body.terminator.as_ref().unwrap() {
                hir::Terminator::Return(id) => Terminator::Return(Register(id.0)),
                hir::Terminator::Jump(e) => Terminator::Jump(edge(e)),
                hir::Terminator::Branch { condition, yes, no } => {
                    let point = ResumePoint {
                        block: BlockId(block_id),
                        instruction: body.instructions.len(),
                    };
                    let state = output.state(hir, point, &analysis.live_terminator[block_id]);
                    let roots = output.states[state.0]
                        .values
                        .iter()
                        .map(|(_, register)| *register)
                        .collect();
                    let truth = output.register(Type::Bool);
                    instructions.push(Instruction {
                        result: Some(truth),
                        origin: *condition,
                        point,
                        operation: Operation::Truth {
                            value: Register(condition.0),
                            state,
                            roots,
                            effects: Effects::CONSERVATIVE,
                        },
                    });
                    Terminator::Branch {
                        condition: truth,
                        yes: edge(yes),
                        no: edge(no),
                    }
                }
            };
            output.blocks.push(Block {
                parameters: body
                    .parameters
                    .iter()
                    .map(|id| Register(id.0))
                    .chain(
                        body.parameters
                            .iter()
                            .filter_map(|id| typed_params.get(id).copied()),
                    )
                    .collect(),
                instructions,
                terminator,
            });
        }
        for block in 0..hir.blocks.len() {
            let live = &analysis.live_in[block];
            if output
                .scalar_sources
                .values()
                .any(|r| live.contains(&ValueId(r.0)))
            {
                let state = output.state(
                    hir,
                    ResumePoint {
                        block: BlockId(block),
                        instruction: 0,
                    },
                    live,
                );
                let exit = output.exit(state, ExitReason::TypeGuard);
                output.scalar_entries.insert(BlockId(block), exit);
            }
        }
        let headers = analysis
            .backedges
            .iter()
            .map(|(_, header)| *header)
            .collect::<BTreeSet<_>>();
        for header in headers {
            let state = output.state(
                hir,
                ResumePoint {
                    block: BlockId(header),
                    instruction: 0,
                },
                &analysis.live_in[header],
            );
            output.osr_entries.push(OsrEntry {
                header: BlockId(header),
                state,
            });
        }
        if mode == LoweringMode::Profiled && allow_inline {
            crate::leaf_inline::merge(hir, &mut output, allow_cfg)?;
        }
        if mode == LoweringMode::Profiled && allow_inline {
            output.reuse_unpack_bundles();
        }
        if mode == LoweringMode::Profiled && output.blocks.len() == hir.blocks.len() {
            for (header, body, iterator, state) in crate::region::LoopRegion::candidates(&output) {
                let entry = output.exit(state, ExitReason::TypeGuard);
                let imports = crate::region::LoopRegion::imports(&output, header, body);
                let schema = crate::region::schema::Schema::build(&output, header, body);
                output.regions.push(crate::region::LoopRegion {
                    schema,
                    imports,
                    header,
                    body,
                    iterator,
                    entry,
                });
            }
        }
        if let Some((mut nest, state)) = (output.blocks.len() == hir.blocks.len())
            .then(|| crate::region::nest::LoopNest::discover(&output))
            .flatten()
        {
            nest.entry = output.exit(state, ExitReason::TypeGuard);
            output.nest = Some(nest);
        }
        output.refresh_inline_gc_roots();
        output.verify()?;
        Ok(output)
    }
    fn register(&mut self, kind: Type) -> Register {
        let id = Register(self.registers.len());
        self.registers.push(kind);
        id
    }
    fn state(&mut self, hir: &hir::Function, resume: ResumePoint, live: &[ValueId]) -> StateId {
        let id = StateId(self.states.len());
        self.states.push(FrameState {
            resume,
            values: live
                .iter()
                .map(|value| (*value, Register(value.0)))
                .collect(),
            environment_root: FrameEnvironment::Current,
            local_count: self.local_count,
            local_values: if hir.local_state(resume).is_empty() {
                vec![None; self.local_count]
            } else {
                hir.local_state(resume)
                    .iter()
                    .map(|v| v.map(|v| Register(v.0)))
                    .collect()
            },
        });
        id
    }
    fn exit(&mut self, state: StateId, reason: ExitReason) -> ExitId {
        let id = ExitId(self.exits.len());
        self.exits.push(SideExit {
            inline: None,
            state,
            reason,
        });
        id
    }

    /// Uncaptured frame slots without explicit writes are invariant during an invocation.
    /// Callees can mutate captured environments or referenced objects, but cannot change these slots.
    pub(crate) fn invariant_local_loads(&self) -> Vec<(Register, usize)> {
        // A block-zero OSR entry is not necessarily a fresh invocation.
        if self.osr_entries.iter().any(|entry| entry.header.0 == 0) {
            return Vec::new();
        }
        let written = self
            .blocks
            .iter()
            .flat_map(|block| &block.instructions)
            .filter_map(|instruction| match &instruction.operation {
                Operation::Call {
                    helper: Helper::Python(CoreOp::StoreLocal(slot)),
                    ..
                } => Some(*slot),
                _ => None,
            })
            .collect::<BTreeSet<_>>();
        self.blocks
            .iter()
            .flat_map(|block| &block.instructions)
            .filter_map(|instruction| match &instruction.operation {
                Operation::Call {
                    helper: Helper::Python(CoreOp::LoadLocal(slot)),
                    ..
                } if !written.contains(slot) => Some((instruction.result?, *slot)),
                _ => None,
            })
            .collect()
    }

    pub fn verify(&self) -> Result<()> {
        let fail = || Error::InvalidIr("invalid LIR");
        let physical_blocks = self
            .states
            .iter()
            .find_map(|state| {
                state
                    .inline()
                    .and_then(|state| state.snapshot.frames.first())
                    .map(|frame| frame.source.blocks.len())
            })
            .unwrap_or(self.blocks.len());
        if self.block_origins.len() != self.blocks.len()
            || self
                .block_origins
                .iter()
                .enumerate()
                .any(|(index, origin)| {
                    *origin != (index < physical_blocks).then_some(BlockId(index))
                })
        {
            return Err(Error::InvalidIr("inline block origin map"));
        }
        let kind = |register: Register, expected: Type| -> Result<()> {
            if self.registers.get(register.0) != Some(&expected) {
                Err(fail())
            } else {
                Ok(())
            }
        };
        let state = |id: StateId| -> Result<&FrameState> { self.states.get(id.0).ok_or_else(fail) };
        let exit = |id: ExitId| -> Result<&SideExit> {
            let exit = self.exits.get(id.0).ok_or_else(fail)?;
            state(exit.state)?;
            Ok(exit)
        };
        for side_exit in &self.exits {
            let physical = state(side_exit.state)?;
            if let Some(snapshot) = &side_exit.inline {
                snapshot.verify(&self.registers)?;
                let outer = &snapshot.frames[0].source;
                if outer.id() != self.source || outer.revision() != self.revision {
                    return Err(Error::InvalidIr("inline exit source"));
                }
                // Every logical value needs an ordinary HIR slot or an explicitly mapped
                // shadow register; unrelated callee ValueIds cannot alias caller slots.
                if snapshot.roots(&self.registers)?.iter().any(|register| {
                    !physical.values.iter().any(|(_, root)| root == register)
                        && !self.inline_roots(side_exit.state).contains(register)
                }) {
                    return Err(Error::InvalidIr("inline exit lacks physical root mapping"));
                }
            }
        }
        for (raw, source) in &self.scalar_sources {
            kind(*raw, Type::F64)?;
            kind(*source, Type::Boxed)?;
            if self.hir_values.get(source.0) != Some(source)
                && !self.scoped_results.contains(source)
            {
                return Err(fail());
            }
            let parameter = self.blocks.iter().any(|b| b.parameters.contains(raw));
            if !parameter
                && !self.blocks.iter().flat_map(|b| &b.instructions).any(|i| {
                    i.result == Some(*raw)
                        && matches!(i.operation,Operation::UnboxFloat(value) if value==*source)
                })
            {
                return Err(fail());
            }
        }
        for entry in &self.osr_entries {
            let state = state(entry.state)?;
            if self
                .scalar_imports()
                .any(|(_, source)| state.values.iter().any(|(_, r)| r == source))
                && !self.scalar_entries.contains_key(&entry.header)
            {
                return Err(Error::InvalidIr("missing scalar OSR entry guard"));
            }
        }
        for (block, id) in &self.scalar_entries {
            let entry = exit(*id)?;
            if block.0 >= self.blocks.len()
                || entry.reason != ExitReason::TypeGuard
                || state(entry.state)?.resume
                    != (ResumePoint {
                        block: *block,
                        instruction: 0,
                    })
            {
                return Err(fail());
            }
        }
        if self.observation_inputs.len() != self.hir_values.len() {
            return Err(fail());
        }
        for inputs in &self.observation_inputs {
            for &input in inputs {
                kind(input, Type::Boxed)?;
            }
        }
        let candidates = crate::region::LoopRegion::candidates(self);
        let mut covered = BTreeSet::new();
        for region in &self.regions {
            let (_, _, _, entry_state) = candidates
                .iter()
                .find(|(h, b, i, _)| {
                    *h == region.header && *b == region.body && *i == region.iterator
                })
                .ok_or_else(fail)?;
            let expected_state = state(*entry_state)?;
            if region.schema
                != crate::region::schema::Schema::build(self, region.header, region.body)
                || region.imports
                    != crate::region::LoopRegion::imports(self, region.header, region.body)
                || region
                    .imports
                    .iter()
                    .any(|r| !expected_state.values.iter().any(|(_, v)| v == r))
                || exit(region.entry)?.state != *entry_state
                || exit(region.entry)?.reason != ExitReason::TypeGuard
                || !covered.insert(region.header)
                || !covered.insert(region.body)
            {
                return Err(fail());
            }
        }
        if let Some(nest) = &self.nest {
            let (mut expected, snapshot) =
                crate::region::nest::LoopNest::discover(self).ok_or_else(fail)?;
            expected.entry = nest.entry;
            if *nest != expected
                || exit(nest.entry)?.state != snapshot
                || exit(nest.entry)?.reason != ExitReason::TypeGuard
                || nest
                    .imports
                    .iter()
                    .any(|r| !self.states[snapshot.0].values.iter().any(|(_, v)| v == r))
            {
                return Err(fail());
            }
        }
        let mut definitions = BTreeSet::new();
        for body in &self.blocks {
            for &parameter in &body.parameters {
                if self.registers.get(parameter.0) == Some(&Type::F64) {
                    let source = self.scalar_sources.get(&parameter).ok_or_else(fail)?;
                    if !body.parameters.contains(source) {
                        return Err(fail());
                    }
                } else {
                    kind(parameter, Type::Boxed)?;
                }
                if !definitions.insert(parameter) {
                    return Err(fail());
                }
            }
            let mut guarded = BTreeMap::new();
            let mut unboxed_sources = self.scalar_sources.clone();
            for instruction in &body.instructions {
                if instruction
                    .result
                    .is_some_and(|result| self.scoped_results.contains(&result))
                    && !self.inline_scopes.iter().any(|scope| {
                        scope.callsite == instruction.point
                            && scope.destination == instruction.origin
                    })
                {
                    return Err(Error::InvalidIr("inline instruction source scope"));
                }
                let expected = match &instruction.operation {
                    Operation::FloatListView { value, failure, .. } => {
                        kind(*value, Type::Boxed)?;
                        let snapshot = state(exit(*failure)?.state)?;
                        if exit(*failure)?.reason != ExitReason::SequenceGuard
                            || snapshot.resume != instruction.point
                            || (!snapshot.values.iter().any(|(_, r)| r == value)
                                && !snapshot.inline_roots().contains(value))
                        {
                            return Err(fail());
                        }
                        Some(Type::FloatView)
                    }
                    Operation::FloatListUnpack { value, view } => {
                        kind(*value, Type::Boxed)?;
                        kind(*view, Type::FloatView)?;
                        let producer = body
                            .instructions
                            .iter()
                            .find(|i| i.result == Some(*view))
                            .ok_or_else(fail)?;
                        if !matches!(producer.operation, Operation::FloatListView { value: source, length: Some(_), .. } if source == *value)
                        {
                            return Err(fail());
                        }
                        Some(Type::Boxed)
                    }
                    Operation::FloatListLoad { view, failure, .. }
                    | Operation::FloatListStore { view, failure, .. } => {
                        kind(*view, Type::FloatView)?;
                        if let Operation::FloatListStore { value, .. } = &instruction.operation {
                            kind(*value, Type::F64)?;
                        }
                        if exit(*failure)?.reason != ExitReason::SequenceGuard
                            || state(exit(*failure)?.state)?.resume != instruction.point
                        {
                            return Err(fail());
                        }
                        if matches!(instruction.operation, Operation::FloatListLoad { .. }) {
                            guarded.insert(instruction.result.ok_or_else(fail)?, Type::F64);
                        }
                        Some(Type::Boxed)
                    }
                    Operation::SequenceUnpack { failure, .. }
                    | Operation::SequenceGet { failure, .. } => {
                        let snapshot = state(exit(*failure)?.state)?;
                        if exit(*failure)?.reason != ExitReason::SequenceGuard
                            || snapshot.resume != instruction.point
                        {
                            return Err(fail());
                        }
                        for input in instruction.operation.inputs() {
                            kind(input, Type::Boxed)?;
                            if !snapshot.values.iter().any(|(_, r)| *r == input)
                                && !snapshot.inline_roots().contains(&input)
                            {
                                return Err(fail());
                            }
                        }
                        Some(Type::Boxed)
                    }
                    Operation::Copy(value) => {
                        kind(*value, Type::Boxed)?;
                        Some(Type::Boxed)
                    }
                    Operation::Constant(value) if value.immediate().is_none() => return Err(fail()),
                    Operation::Constant(_) => Some(Type::Boxed),
                    Operation::Truth {
                        value,
                        state: id,
                        roots,
                        effects,
                    } => {
                        kind(*value, Type::Boxed)?;
                        let snapshot = state(*id)?;
                        if !snapshot
                            .values
                            .iter()
                            .any(|(_, register)| register == value)
                            && !snapshot.inline_roots().contains(value)
                            || snapshot.resume != instruction.point
                            || !effects.covers(Effects::CONSERVATIVE)
                            || roots.iter().copied().collect::<BTreeSet<_>>()
                                != if effects.needs_safepoint() {
                                    snapshot
                                        .values
                                        .iter()
                                        .map(|(_, r)| *r)
                                        .chain(snapshot.inline_roots().iter().copied())
                                        .collect()
                                } else {
                                    BTreeSet::new()
                                }
                        {
                            return Err(fail());
                        }
                        for &root in roots {
                            kind(root, Type::Boxed)?;
                        }
                        guarded.clear();
                        Some(Type::Bool)
                    }
                    Operation::GuardInt { value, exit: id }
                    | Operation::GuardFloat { value, exit: id } => {
                        kind(*value, Type::Boxed)?;
                        if exit(*id)?.reason != ExitReason::TypeGuard {
                            return Err(fail());
                        }
                        if state(exit(*id)?.state)?.resume != instruction.point {
                            return Err(fail());
                        }
                        if !state(exit(*id)?.state)?
                            .values
                            .iter()
                            .any(|(_, register)| register == value)
                            && !self.inline_roots(exit(*id)?.state).contains(value)
                            && !self.scoped_results.contains(value)
                        {
                            return Err(fail());
                        }
                        guarded.insert(
                            *value,
                            if matches!(instruction.operation, Operation::GuardInt { .. }) {
                                Type::I64
                            } else {
                                Type::F64
                            },
                        );
                        None
                    }
                    Operation::UnboxInt(value) => {
                        kind(*value, Type::Boxed)?;
                        if guarded.get(value) != Some(&Type::I64) {
                            return Err(fail());
                        }
                        unboxed_sources.insert(instruction.result.ok_or_else(fail)?, *value);
                        Some(Type::I64)
                    }
                    Operation::UnboxFloat(value) => {
                        kind(*value, Type::Boxed)?;
                        if guarded.get(value) != Some(&Type::F64) {
                            return Err(fail());
                        }
                        unboxed_sources.insert(instruction.result.ok_or_else(fail)?, *value);
                        Some(Type::F64)
                    }
                    Operation::IntToFloat(value) => {
                        kind(*value, Type::I64)?;
                        let source = *unboxed_sources.get(value).ok_or_else(fail)?;
                        unboxed_sources.insert(instruction.result.ok_or_else(fail)?, source);
                        Some(Type::F64)
                    }
                    Operation::FloatBinary {
                        left,
                        right,
                        failure,
                        ..
                    } => {
                        kind(*left, Type::F64)?;
                        kind(*right, Type::F64)?;
                        let snapshot = state(exit(*failure)?.state)?;
                        if exit(*failure)?.reason != ExitReason::FloatArithmetic
                            || snapshot.resume != instruction.point
                        {
                            return Err(fail());
                        }
                        for input in [left, right] {
                            let source = unboxed_sources.get(input).ok_or_else(fail)?;
                            if !snapshot
                                .values
                                .iter()
                                .any(|(_, register)| register == source)
                                && !self.inline_roots(exit(*failure)?.state).contains(source)
                            {
                                return Err(fail());
                            }
                        }
                        Some(Type::F64)
                    }
                    Operation::NumberCompare {
                        value,
                        integer,
                        operator,
                        failure,
                    } => {
                        kind(*value, Type::Boxed)?;
                        let edge = exit(*failure)?;
                        let snapshot = state(edge.state)?;
                        if !exact_float_integer(*integer)
                            || !["==", "!=", "<", "<=", ">", ">="].contains(&operator.as_str())
                            || edge.reason != ExitReason::TypeGuard
                            || snapshot.resume != instruction.point
                            || (!snapshot.values.iter().any(|(_, r)| r == value)
                                && !snapshot.inline_roots().contains(value))
                        {
                            return Err(fail());
                        }
                        Some(Type::Bool)
                    }
                    Operation::FloatCompare {
                        operator,
                        left,
                        right,
                    } => {
                        kind(*left, Type::F64)?;
                        kind(*right, Type::F64)?;
                        if !["==", "!=", "<", "<=", ">", ">="].contains(&operator.as_str()) {
                            return Err(fail());
                        }
                        Some(Type::Bool)
                    }
                    Operation::BoxFloat(value) => {
                        kind(*value, Type::F64)?;
                        unboxed_sources.insert(*value, instruction.result.ok_or_else(fail)?);
                        Some(Type::Boxed)
                    }
                    Operation::BoolNot(value) => {
                        kind(*value, Type::Bool)?;
                        Some(Type::Bool)
                    }
                    Operation::BoxBool(value) => {
                        kind(*value, Type::Bool)?;
                        Some(Type::Boxed)
                    }
                    Operation::NumericAdd {
                        left,
                        right,
                        guard,
                        overflow,
                    } => {
                        for input in [left, right] {
                            kind(*input, Type::Boxed)?;
                        }
                        for (id, reason) in [
                            (*guard, ExitReason::TypeGuard),
                            (*overflow, ExitReason::IntegerOverflow),
                        ] {
                            let edge = exit(id)?;
                            let snapshot = state(edge.state)?;
                            if edge.reason != reason
                                || snapshot.resume != instruction.point
                                || [left, right].iter().any(|input| {
                                    !snapshot.values.iter().any(|(_, r)| r == *input)
                                        && !self.inline_roots(edge.state).contains(input)
                                })
                            {
                                return Err(fail());
                            }
                        }
                        if exit(*guard)?.state != exit(*overflow)?.state {
                            return Err(fail());
                        }
                        Some(Type::Boxed)
                    }
                    Operation::CheckedAdd {
                        left,
                        right,
                        overflow,
                    } => {
                        kind(*left, Type::I64)?;
                        kind(*right, Type::I64)?;
                        let snapshot = state(exit(*overflow)?.state)?;
                        for input in [left, right] {
                            let boxed = unboxed_sources.get(input).ok_or_else(fail)?;
                            if !snapshot
                                .values
                                .iter()
                                .any(|(_, register)| register == boxed)
                                && !snapshot.inline_roots().contains(boxed)
                            {
                                return Err(fail());
                            }
                        }
                        if exit(*overflow)?.reason != ExitReason::IntegerOverflow {
                            return Err(fail());
                        }
                        if state(exit(*overflow)?.state)?.resume != instruction.point {
                            return Err(fail());
                        }
                        Some(Type::I64)
                    }
                    Operation::BoxInt(value) => {
                        kind(*value, Type::I64)?;
                        unboxed_sources.insert(*value, instruction.result.ok_or_else(fail)?);
                        Some(Type::Boxed)
                    }
                    Operation::Call {
                        helper,
                        arguments,
                        state: id,
                        roots,
                        effects,
                        ..
                    } => {
                        if let Helper::TupleReturn(plan) = helper {
                            let frame = state(*id)?
                                .inline()
                                .and_then(|state| state.snapshot.frames.last())
                                .ok_or(Error::InvalidIr("tuple return without inline frame"))?;
                            let body = &frame.source.blocks[frame.resume.block.0];
                            let tuple = body
                                .instructions
                                .get(frame.resume.instruction)
                                .ok_or_else(fail)?;
                            let hir::Op::Tuple(fields) = &tuple.op else {
                                return Err(fail());
                            };
                            if frame.source.id() != plan.source.id()
                                || frame.source.revision() != plan.source.revision()
                                || frame.resume.instruction + 1 != body.instructions.len()
                                || !matches!(body.terminator, Some(hir::Terminator::Return(value)) if value == tuple.result)
                                || !(1..=4).contains(&fields.len())
                                || frame
                                    .values
                                    .iter()
                                    .find(|(value, _)| *value == fields[0])
                                    .map(|(_, r)| *r)
                                    != arguments.first().copied()
                            {
                                return Err(Error::InvalidIr("invalid scalarized tuple return"));
                            }
                        }
                        if let Helper::Scoped(inner) = helper
                            && (!matches!(
                                inner.as_ref(),
                                Helper::LoadName(_)
                                    | Helper::ScalarCall(_)
                                    | Helper::Python(CoreOp::Call(_))
                            ) || state(*id)?
                                .inline()
                                .and_then(|state| state.snapshot.frames.last())
                                .map(|frame| frame.environment)
                                != arguments.first().copied())
                        {
                            return Err(Error::InvalidIr("invalid scoped runtime context"));
                        }
                        if !helper.accepts(arguments.len())
                            || matches!(helper, Helper::Python(CoreOp::LoadLocal(slot) | CoreOp::StoreLocal(slot) | CoreOp::CheckLocal(slot)) if *slot >= self.local_count)
                        {
                            return Err(fail());
                        }
                        for &arg in arguments {
                            kind(arg, Type::Boxed)?;
                        }
                        if let Some((scope, _)) = instruction
                            .result
                            .and_then(|result| self.inline_origins.get(&result))
                            && !state(*id)?.inline().is_some_and(|state| {
                                state
                                    .snapshot
                                    .frames
                                    .last()
                                    .is_some_and(|frame| frame.scope == *scope)
                            })
                        {
                            return Err(Error::InvalidIr("missing inline helper state"));
                        }
                        let snapshot = state(*id)?;
                        if snapshot.resume != instruction.point {
                            return Err(fail());
                        }
                        let required = snapshot
                            .values
                            .iter()
                            .map(|(_, r)| *r)
                            .chain(self.inline_roots(*id).iter().copied())
                            .collect::<BTreeSet<_>>();
                        if arguments
                            .iter()
                            .any(|argument| !required.contains(argument))
                            || roots.iter().copied().collect::<BTreeSet<_>>()
                                != if effects.needs_safepoint() {
                                    required
                                } else {
                                    BTreeSet::new()
                                }
                            || !effects.covers(helper.effects())
                        {
                            return Err(fail());
                        }
                        for &root in roots {
                            kind(root, Type::Boxed)?;
                        }
                        guarded.clear();
                        Some(Type::Boxed)
                    }
                };
                match (instruction.result, expected) {
                    (Some(register), Some(expected)) => {
                        kind(register, expected)?;
                        if !definitions.insert(register) {
                            return Err(fail());
                        }
                    }
                    (None, None) => {}
                    _ => return Err(fail()),
                }
            }
            let check_edge = |edge: &Edge| -> Result<()> {
                let target = self.blocks.get(edge.block.0).ok_or_else(fail)?;
                if edge.arguments.len() != target.parameters.len() {
                    return Err(fail());
                }
                for (&arg, param) in edge.arguments.iter().zip(&target.parameters) {
                    kind(arg, *self.registers.get(param.0).ok_or_else(fail)?)?;
                    if self.registers[param.0] == Type::F64 {
                        let source = self.scalar_sources.get(param).ok_or_else(fail)?;
                        let index = target
                            .parameters
                            .iter()
                            .position(|r| r == source)
                            .ok_or_else(fail)?;
                        if unboxed_sources.get(&arg) != Some(&edge.arguments[index]) {
                            return Err(fail());
                        }
                    }
                }
                Ok(())
            };
            match &body.terminator {
                Terminator::Return(value) => kind(*value, Type::Boxed)?,
                Terminator::Jump(edge) => check_edge(edge)?,
                Terminator::Branch { condition, yes, no } => {
                    kind(*condition, Type::Bool)?;
                    check_edge(yes)?;
                    check_edge(no)?;
                }
            }
        }
        if self.shadow_slots > self.registers.len()
            || (self.shadow_slots != 0 && self.shadow_slots < self.hir_values.len())
        {
            return Err(Error::InvalidIr("inline shadow register layout"));
        }
        for &register in &self.scoped_results {
            kind(register, Type::Boxed)?;
            if self.shadow_slots == 0 || register.0 >= self.shadow_slots {
                return Err(Error::InvalidIr("inline boxed result storage"));
            }
            if !self.inline_scopes.iter().any(|scope| {
                scope.registers.contains(&register.0) || scope.destination.0 == register.0
            }) {
                return Err(Error::InvalidIr("unowned inline register"));
            }
        }
        let mut scope_ids = BTreeSet::new();
        for scope in &self.inline_scopes {
            scope.source.analyze()?;
            if scope.id.0 == 0
                || !scope_ids.insert(scope.id)
                || scope.registers.start < self.hir_values.len()
                || scope.registers.end > self.shadow_slots
                || scope.registers.start >= scope.registers.end
                || scope.destination.0 >= self.hir_values.len()
            {
                return Err(Error::InvalidIr("invalid inline scope layout"));
            }
        }
        for (register, (scope, value)) in &self.inline_origins {
            if !self.scoped_results.contains(register)
                || !self
                    .inline_scopes
                    .iter()
                    .any(|source| source.id == *scope && value.0 < source.source.value_count)
            {
                return Err(Error::InvalidIr("invalid scoped HIR origin"));
            }
        }
        let primitives = self.primitive_registers();
        for (index, frame) in self.states.iter().enumerate() {
            let id = StateId(index);
            let Some(state) = frame.inline() else {
                continue;
            };
            let inline = &state.snapshot;
            inline.verify(&self.registers)?;
            let outer = &inline.frames[0].source;
            let inner = &inline.frames[inline.frames.len() - 1];
            if outer.id() != self.source
                || outer.revision() != self.revision
                || !self.inline_scopes.iter().any(|scope| {
                    scope.id == inner.scope
                        && scope.source.id() == inner.source.id()
                        && scope.source.revision() == inner.source.revision()
                })
            {
                return Err(Error::InvalidIr("unknown inline frame scope"));
            }
            if self.inline_roots(id) != inline.roots(&self.registers)? {
                return Err(Error::InvalidIr("inline root recipe mismatch"));
            }
            let expected_gc: Vec<_> = state
                .roots
                .iter()
                .copied()
                .filter(|register| !primitives.contains(register))
                .collect();
            if state.gc_roots != expected_gc {
                return Err(Error::InvalidIr("inline GC roots"));
            }
            for register in self.inline_roots(id) {
                kind(*register, Type::Boxed)?;
                if register.0 >= self.shadow_slots {
                    return Err(Error::InvalidIr("inline shadow root bounds"));
                }
            }
        }
        for snapshot in &self.states {
            if snapshot.resume.block.0 >= self.blocks.len()
                || snapshot.local_count != self.local_count
                || snapshot.local_values.len() != self.local_count
            {
                return Err(fail());
            }
            let mut slots = BTreeSet::new();
            for register in snapshot.local_values.iter().flatten() {
                kind(*register, Type::Boxed)?;
                if !snapshot.values.iter().any(|(_, r)| r == register) {
                    return Err(fail());
                }
            }
            for (value, register) in &snapshot.values {
                kind(*register, Type::Boxed)?;
                if self.hir_values.get(value.0) != Some(register) || !slots.insert(*value) {
                    return Err(fail());
                }
            }
        }
        // Raw float views are block-local and end before any callback that could access generic list storage.
        for body in &self.blocks {
            let mut active = None;
            for instruction in &body.instructions {
                match &instruction.operation {
                    Operation::FloatListView { .. } => active = instruction.result,
                    Operation::FloatListLoad { view, .. }
                    | Operation::FloatListStore { view, .. }
                    | Operation::FloatListUnpack { view, .. } => {
                        if active != Some(*view) {
                            return Err(fail());
                        }
                    }
                    Operation::Call { .. }
                    | Operation::SequenceGet { .. }
                    | Operation::SequenceUnpack { .. }
                    | Operation::Truth { .. } => active = None,
                    _ => {}
                }
            }
        }
        // A mutable unpack view is legal only while its eager reads cannot run Python or mutate the source.
        for body in &self.blocks {
            for (index, instruction) in body.instructions.iter().enumerate() {
                if let Operation::SequenceUnpack { kind, count, .. } = instruction.operation {
                    let result = instruction.result.ok_or_else(fail)?;
                    let start = index + 1;
                    let end = start.checked_add(count).ok_or_else(fail)?;
                    let items = body.instructions.get(start..end).ok_or_else(fail)?;
                    if !items.iter().enumerate().all(|(index,item)|matches!(item.operation,
                        Operation::SequenceGet {value,kind:k,index:SequenceIndex::Constant(i),..} if value==result && k==kind && i128::from(i)==index as i128)) {
                        return Err(fail());
                    }
                    let mut uses = 0;
                    for block in &self.blocks {
                        uses += block
                            .instructions
                            .iter()
                            .flat_map(|i| i.operation.inputs())
                            .filter(|r| *r == result)
                            .count();
                        match &block.terminator {
                            Terminator::Return(r) => uses += usize::from(*r == result),
                            Terminator::Jump(edge) => {
                                uses += edge.arguments.iter().filter(|r| **r == result).count()
                            }
                            Terminator::Branch { condition, yes, no } => {
                                uses += usize::from(*condition == result)
                                    + yes
                                        .arguments
                                        .iter()
                                        .chain(&no.arguments)
                                        .filter(|r| **r == result)
                                        .count()
                            }
                        }
                    }
                    if uses != count
                        || self
                            .states
                            .iter()
                            .any(|s| s.local_values.contains(&Some(result)))
                    {
                        return Err(fail());
                    }
                }
            }
        }
        for entry in &self.osr_entries {
            if state(entry.state)?.resume.block != entry.header {
                return Err(fail());
            }
        }
        self.verify_dominance()?;
        Ok(())
    }

    pub fn dump(&self) -> String {
        let mut text = format!("lir f{} revision {}\n", self.source.0, self.revision);
        for (index, block) in self.blocks.iter().enumerate() {
            writeln!(text, "b{index}({:?}):", block.parameters).unwrap();
            for instruction in &block.instructions {
                write!(text, "  ").unwrap();
                if let Some(result) = instruction.result {
                    write!(text, "r{}:{:?} = ", result.0, self.registers[result.0]).unwrap();
                }
                match &instruction.operation {
                    Operation::Call {
                        helper,
                        arguments,
                        state,
                        ..
                    } => {
                        let helper = match helper {
                            Helper::Python(CoreOp::Function(_)) => "MakeFunction".into(),
                            Helper::Python(CoreOp::Class(name, _)) => format!("MakeClass({name})"),
                            other => format!("{other:?}"),
                        };
                        writeln!(
                            text,
                            "call {helper} {arguments:?} safepoint=s{} ; %{}",
                            state.0, instruction.origin.0
                        )
                        .unwrap();
                    }
                    other => writeln!(text, "{other:?} ; %{}", instruction.origin.0).unwrap(),
                }
            }
            writeln!(text, "  {:?}", block.terminator).unwrap();
        }
        for region in &self.regions {
            writeln!(
                text,
                "region b{} + b{} iterator={:?} entry=exit{} imports={:?} hoisted={}",
                region.header.0,
                region.body.0,
                region.iterator,
                region.entry.0,
                region.imports,
                region.schema.as_ref().map_or(0, |s| s.constraints.len())
            )
            .unwrap();
        }
        if let Some(n) = &self.nest {
            writeln!(
                text,
                "nest b{} blocks={:?} private={:?} entry=exit{}",
                n.header.0, n.blocks, n.private_iterators, n.entry.0
            )
            .unwrap();
        }
        for (boxed, raw) in self.boxing_recipes() {
            writeln!(text, "materialize {boxed:?} = Float({raw:?})").unwrap();
        }
        for (register, (scope, value)) in &self.inline_origins {
            writeln!(
                text,
                "origin r{} = scope{}:%{}",
                register.0, scope.0, value.0
            )
            .unwrap();
        }
        for (index, origin) in self
            .block_origins
            .iter()
            .enumerate()
            .filter(|(_, origin)| origin.is_none())
        {
            writeln!(&mut text, "synthetic block b{index} origin={origin:?}").unwrap();
        }
        for scope in &self.inline_scopes {
            writeln!(
                text,
                "inline scope{} {} {:?}@{} call=b{}:{} result=%{} registers={:?}",
                scope.id.0,
                scope.source.name,
                scope.source.id(),
                scope.source.revision(),
                scope.callsite.block.0,
                scope.callsite.instruction,
                scope.destination.0,
                scope.registers
            )
            .unwrap();
        }
        for (id, frame) in self.states.iter().enumerate() {
            let Some(state) = frame.inline() else {
                continue;
            };
            writeln!(
                text,
                "s{} inline roots={:?} gc={:?}",
                id, state.roots, state.gc_roots
            )
            .unwrap();
        }
        for (id, exit) in self.exits.iter().enumerate() {
            writeln!(text, "exit{id}: {:?} -> s{}", exit.reason, exit.state.0).unwrap();
            if let Some(snapshot) = &exit.inline {
                for frame in &snapshot.frames {
                    writeln!(text, "  scope{} parent={:?} source={:?}@{} resume=b{}:{} env={:?} slots={:?} locals={:?} pending={:?}",
                        frame.scope.0, frame.parent, frame.source.id(), frame.source.revision(),
                        frame.resume.block.0, frame.resume.instruction, frame.environment,
                        frame.values, frame.locals, frame.pending).unwrap();
                }
            }
        }
        for (id, state) in self.states.iter().enumerate() {
            writeln!(
                text,
                "s{id}: resume b{}:{} env=current locals={} bindings={:?} slots={:?}",
                state.resume.block.0,
                state.resume.instruction,
                state.local_count,
                state.local_values,
                state.values
            )
            .unwrap();
        }
        for entry in &self.osr_entries {
            writeln!(text, "osr b{} -> s{}", entry.header.0, entry.state.0).unwrap();
        }
        text
    }
}

/// Values reconstructed from native root locations before re-entering the bytecode interpreter.
#[derive(Debug, Clone)]
pub struct DeoptFrame {
    pub function: FunctionId,
    pub revision: u64,
    pub resume: ResumePoint,
    pub environment: crate::Value,
    pub slots: Vec<Option<crate::Value>>,
    pub locals: Vec<Option<crate::Value>>,
}
impl Function {
    pub fn reconstruct(
        &self,
        state: StateId,
        environment: crate::Value,
        locals: &[Option<crate::Value>],
        mut read: impl FnMut(Register) -> Result<crate::Value>,
    ) -> Result<DeoptFrame> {
        let snapshot = self
            .states
            .get(state.0)
            .ok_or(Error::InvalidIr("invalid frame state"))?;
        let mut slots = vec![None; self.hir_values.len()];
        if locals.len() != snapshot.local_count {
            return Err(Error::InvalidIr("deoptimization local layout"));
        }
        for (value, register) in &snapshot.values {
            *slots
                .get_mut(value.0)
                .ok_or(Error::InvalidIr("invalid frame slot"))? = Some(read(*register)?);
        }
        let mut locals = locals.to_vec();
        for (slot, register) in snapshot.local_values.iter().enumerate() {
            if let Some(register) = register {
                locals[slot] = match read(*register)? {
                    crate::Value::Unbound => None,
                    value => Some(value),
                };
            }
        }
        Ok(DeoptFrame {
            function: self.source,
            revision: self.revision,
            resume: snapshot.resume,
            environment,
            slots,
            locals,
        })
    }
    pub fn resume_offset(&self, state: StateId, code: &crate::bytecode::Code) -> Result<usize> {
        if code.function != self.source || code.revision != self.revision {
            return Err(Error::InvalidIr("stale compiled code"));
        }
        code.resume_offset(
            self.states
                .get(state.0)
                .ok_or(Error::InvalidIr("invalid frame state"))?
                .resume,
        )
    }
}

impl Operation {
    pub(crate) fn inputs(&self) -> Vec<Register> {
        match self {
            Self::FloatListView { value, .. } => vec![*value],
            Self::FloatListUnpack { value, view } => vec![*value, *view],
            Self::FloatListLoad { view, .. } => vec![*view],
            Self::FloatListStore { view, value, .. } => vec![*view, *value],
            Self::SequenceUnpack { value, .. } => vec![*value],
            Self::SequenceGet { value, index, .. } => match index {
                SequenceIndex::Constant(_) => vec![*value],
                SequenceIndex::Value(index) => vec![*value, *index],
            },
            Self::Constant(_) => vec![],
            Self::Copy(value)
            | Self::GuardInt { value, .. }
            | Self::GuardFloat { value, .. }
            | Self::UnboxFloat(value)
            | Self::IntToFloat(value)
            | Self::BoxFloat(value)
            | Self::BoxBool(value)
            | Self::BoolNot(value)
            | Self::UnboxInt(value)
            | Self::BoxInt(value)
            | Self::NumberCompare { value, .. }
            | Self::Truth { value, .. } => vec![*value],
            Self::NumericAdd { left, right, .. }
            | Self::CheckedAdd { left, right, .. }
            | Self::FloatBinary { left, right, .. }
            | Self::FloatCompare { left, right, .. } => vec![*left, *right],
            Self::Call { arguments, .. } => arguments.clone(),
        }
    }
}
impl Function {
    fn verify_dominance(&self) -> Result<()> {
        let fail = || Error::InvalidIr("LIR definition does not dominate use");
        if self.blocks.is_empty() {
            return Err(fail());
        }
        let edges = |term: &Terminator| match term {
            Terminator::Return(_) => vec![],
            Terminator::Jump(edge) => vec![edge.block.0],
            Terminator::Branch { yes, no, .. } => vec![yes.block.0, no.block.0],
        };
        let mut predecessors = vec![vec![]; self.blocks.len()];
        let mut definitions = BTreeMap::new();
        for (id, block) in self.blocks.iter().enumerate() {
            for &register in &block.parameters {
                definitions.insert(register, (id, 0));
            }
            for (index, ins) in block.instructions.iter().enumerate() {
                if let Some(result) = ins.result {
                    definitions.insert(result, (id, index + 1));
                }
            }
            for to in edges(&block.terminator) {
                predecessors.get_mut(to).ok_or_else(fail)?.push(id);
            }
        }
        let mut reachable = BTreeSet::new();
        let mut pending = vec![0];
        while let Some(id) = pending.pop() {
            if reachable.insert(id) {
                pending.extend(edges(&self.blocks[id].terminator));
            }
        }
        let mut dominators = vec![reachable.clone(); self.blocks.len()];
        dominators[0] = BTreeSet::from([0]);
        loop {
            let mut changed = false;
            for &id in reachable.iter().filter(|&&id| id != 0) {
                let mut next = reachable.clone();
                for &pred in &predecessors[id] {
                    if reachable.contains(&pred) {
                        next = next.intersection(&dominators[pred]).copied().collect();
                    }
                }
                next.insert(id);
                if next != dominators[id] {
                    dominators[id] = next;
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
        let check = |register: Register, block: usize, at: usize| -> Result<()> {
            let &(defined, position) = definitions.get(&register).ok_or_else(fail)?;
            if reachable.contains(&block)
                && (!dominators[block].contains(&defined) || (defined == block && position >= at))
            {
                return Err(fail());
            }
            Ok(())
        };
        for (id, block) in self.blocks.iter().enumerate() {
            for (index, ins) in block.instructions.iter().enumerate() {
                for register in ins.operation.inputs() {
                    check(register, id, index + 1)?;
                }
                if ins
                    .result
                    .is_some_and(|r| self.registers[r.0] == Type::Boxed)
                    && !self.is_scoped_instruction(ins)
                {
                    for &input in self.observation_inputs.get(ins.origin.0).ok_or_else(fail)? {
                        check(input, id, index + 1)?;
                    }
                }
                let snapshot = match &ins.operation {
                    Operation::NumberCompare { failure: exit, .. }
                    | Operation::NumericAdd { guard: exit, .. }
                    | Operation::GuardInt { exit, .. }
                    | Operation::GuardFloat { exit, .. }
                    | Operation::FloatBinary { failure: exit, .. }
                    | Operation::CheckedAdd { overflow: exit, .. } => {
                        Some(self.exits[exit.0].state)
                    }
                    Operation::FloatListView { failure, .. }
                    | Operation::FloatListLoad { failure, .. }
                    | Operation::FloatListStore { failure, .. }
                    | Operation::SequenceGet { failure, .. }
                    | Operation::SequenceUnpack { failure, .. } => {
                        Some(self.exits[failure.0].state)
                    }
                    Operation::Call { state, .. } | Operation::Truth { state, .. } => Some(*state),
                    _ => None,
                };
                if let Some(state) = snapshot {
                    for (_, register) in &self.states[state.0].values {
                        check(*register, id, index + 1)?;
                    }
                    for register in self.inline_roots(state) {
                        check(*register, id, index + 1)?;
                    }
                }
            }
            let inputs = match &block.terminator {
                Terminator::Return(value) => vec![*value],
                Terminator::Jump(edge) => edge.arguments.clone(),
                Terminator::Branch { condition, yes, no } => std::iter::once(*condition)
                    .chain(yes.arguments.iter().copied())
                    .chain(no.arguments.iter().copied())
                    .collect(),
            };
            for register in inputs {
                check(register, id, block.instructions.len() + 1)?;
            }
        }
        for entry in &self.osr_entries {
            for (_, register) in &self.states[entry.state.0].values {
                check(*register, entry.header.0, 1)?;
            }
        }
        Ok(())
    }
}

impl Helper {
    /// Adding a reentrant Python protocol here must also update its allocation/call effects.
    pub fn effects(&self) -> Effects {
        let readonly = Effects::READ_ONLY;
        let allocating = Effects {
            may_allocate: true,
            ..readonly
        };
        let writing = Effects {
            may_write: true,
            ..readonly
        };
        match self {
            Self::Scoped(helper) => helper.effects(),
            Self::LeafGuard(_)
            | Self::LeafReceiver
            | Self::LeafEnvironment
            | Self::CurrentEnvironment
            | Self::LeafReturn(_)
            | Self::TupleReturn(_)
            | Self::LoadName(_) => readonly,
            Self::StoreName(_) => writing,
            Self::StringConstant(_) | Self::List | Self::Tuple => allocating,
            Self::Subtract
            | Self::Less
            | Self::Len
            | Self::SetItem
            | Self::SetAttr(_)
            | Self::GetAttr(_)
            | Self::ScalarCall(_)
            | Self::StringNext
            | Self::InlineCall { .. }
            | Self::Add
            | Self::Dict
            | Self::GetItem => Effects::CONSERVATIVE,
            Self::Python(op) => match op {
                CoreOp::LoadName(_)
                | CoreOp::LoadLocal(_)
                | CoreOp::CheckLocal(_)
                | CoreOp::IsDone => readonly,
                CoreOp::StoreLocal(_)
                | CoreOp::StoreName(_)
                | CoreOp::DeleteName(_)
                | CoreOp::ListAppend => writing,
                CoreOp::Missing | CoreOp::Slice | CoreOp::UnpackItem(_) => allocating,
                _ => Effects::CONSERVATIVE,
            },
        }
    }
    fn accepts(&self, count: usize) -> bool {
        match self {
            Self::Scoped(helper) => {
                count > 0
                    && !matches!(helper.as_ref(), Self::Scoped(_))
                    && helper.accepts(count - 1)
            }
            Self::LeafGuard(plan) => {
                count == plan.source.parameter_names.len() + 1 - usize::from(plan.bound)
            }
            Self::LeafReceiver
            | Self::LeafEnvironment
            | Self::LeafReturn(_)
            | Self::TupleReturn(_) => count == 1,
            Self::CurrentEnvironment => count == 0,
            Self::InlineCall { names, .. } => count == names.len() + 1,
            Self::StringConstant(_) | Self::LoadName(_) => count == 0,
            Self::StoreName(_) | Self::GetAttr(_) => count == 1,
            Self::SetAttr(_) => count == 2,
            Self::ScalarCall(builtin) => count > 0 && builtin.accepts(count - 1),
            Self::Add | Self::Subtract | Self::Less | Self::GetItem => count == 2,
            Self::SetItem => count == 3,
            Self::Len | Self::StringNext => count == 1,
            Self::List | Self::Tuple => true,
            Self::Dict => count.is_multiple_of(2),
            Self::Python(op) => match op {
                CoreOp::Missing
                | CoreOp::Import(_)
                | CoreOp::LoadName(_)
                | CoreOp::LoadLocal(_)
                | CoreOp::Namespace => count == 0,
                CoreOp::StoreName(_)
                | CoreOp::StoreLocal(_)
                | CoreOp::CheckLocal(_)
                | CoreOp::Unary(_)
                | CoreOp::GetAttr(_)
                | CoreOp::Iter
                | CoreOp::Next
                | CoreOp::IsDone
                | CoreOp::Unpack(_)
                | CoreOp::UnpackStar(..)
                | CoreOp::UnpackItem(_)
                | CoreOp::Format(..)
                | CoreOp::Assert => count == 1,
                CoreOp::TryCall(_) => count == 0,
                CoreOp::EnterExcept => count == 1,
                CoreOp::ExceptionMatch | CoreOp::ListAppend => count == 2,
                CoreOp::ExitExcept | CoreOp::DeleteName(_) | CoreOp::DeleteBinding(_) => count == 0,
                CoreOp::DeleteAttr(_) | CoreOp::ZeroSuper(_) => count == 1,
                CoreOp::DeleteItem => count == 2,
                CoreOp::Generator(filters, _) => count == filters + 3,
                CoreOp::Raise => count <= 2,
                CoreOp::PropagateException | CoreOp::With(_) => count == 1,
                CoreOp::Binary(_)
                | CoreOp::Inplace(_)
                | CoreOp::Compare(_)
                | CoreOp::SetAttr(_) => count == 2,
                CoreOp::Slice => count == 3,
                CoreOp::GetSlice => count == 4,
                CoreOp::Call(names) => count == names.len() + 1,
                CoreOp::Function(_)
                | CoreOp::Class(..)
                | CoreOp::Set
                | CoreOp::Join
                | CoreOp::BytesLiteral(_)
                | CoreOp::IntegerLiteral(_) => true,
            },
        }
    }
}

pub(crate) fn unpack_bundle(hir: &hir::Function, block: usize, index: usize, count: usize) -> bool {
    let body = &hir.blocks[block];
    let result = body.instructions[index].result;
    let Some(end) = (index + 1).checked_add(count) else {
        return false;
    };
    let Some(items) = body.instructions.get(index + 1..end) else {
        return false;
    };
    if !items.iter().enumerate().all(|(index, item)| {
        matches!(&item.op, hir::Op::Runtime(CoreOp::UnpackItem(i), inputs)
            if *i == index && inputs == &[result])
    }) {
        return false;
    }

    temporary_uses(hir, result) == Some(count)
}

/// A compiler temporary cannot expose tuple identity through a named local or closure.
pub(crate) fn temporary_uses(hir: &hir::Function, result: ValueId) -> Option<usize> {
    let uses = hir
        .blocks
        .iter()
        .map(|body| {
            let mut uses = body
                .instructions
                .iter()
                .flat_map(|i| i.op.inputs())
                .filter(|v| *v == result)
                .count();
            match body.terminator.as_ref().unwrap() {
                hir::Terminator::Return(v) => uses += usize::from(*v == result),
                hir::Terminator::Jump(edge) => {
                    uses += edge.arguments.iter().filter(|v| **v == result).count()
                }
                hir::Terminator::Branch { condition, yes, no } => {
                    uses += usize::from(*condition == result)
                        + yes
                            .arguments
                            .iter()
                            .chain(&no.arguments)
                            .filter(|v| **v == result)
                            .count();
                }
            }
            uses
        })
        .sum::<usize>();
    (!hir
        .local_states
        .iter()
        .flatten()
        .flatten()
        .any(|v| *v == Some(result)))
    .then_some(uses)
}

impl Function {
    /// Boxed float values can be reconstructed from these primitive producers at native boundaries.
    pub fn boxing_recipes(&self) -> BTreeMap<Register, Register> {
        self.blocks
            .iter()
            .flat_map(|block| &block.instructions)
            .filter_map(|instruction| {
                if let Operation::BoxFloat(source) = instruction.operation {
                    Some((instruction.result?, source))
                } else {
                    None
                }
            })
            .collect()
    }
}

impl Function {
    pub(crate) fn native_boxing_recipes(&self) -> BTreeMap<Register, Register> {
        let mut recipes = self.boxing_recipes();
        // OSR imports retain runtime tags until guarded; only locally produced floats are deferred.
        for entry in &self.osr_entries {
            let state = &self.states[entry.state.0];
            for register in state
                .values
                .iter()
                .map(|(_, r)| r)
                .chain(state.local_values.iter().flatten())
            {
                recipes.remove(register);
            }
        }
        recipes
    }
}

/// The wider roundtrip avoids i64 saturation falsely accepting i64::MAX after f64 rounding.
fn exact_float_integer(value: i64) -> bool {
    value as f64 as i128 == value as i128
}
