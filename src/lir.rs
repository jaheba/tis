//! Typed, register-based lowering for a future native backend.
//! All speculative exits precede Python-visible effects and resume an unchanged HIR operation.

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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StateId(pub usize);
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExitId(pub usize);

#[derive(Debug, Clone)]
pub struct FrameState {
    pub resume: ResumePoint,
    pub values: Vec<(ValueId, Register)>,
    /// The current lexical environment is a separate implicit boxed input and GC root.
    pub environment_root: bool,
    /// Mutable frame locals stay rooted independently of temporary register liveness.
    pub local_count: usize,
    pub local_values: Vec<Option<Register>>,
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
    StringConstant(String),
    Add,
    Subtract,
    Less,
    List,
    Tuple,
    Dict,
    GetItem,
    SetItem,
    Len,
    Python(CoreOp),
}
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
        failure: ExitId,
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
    FloatCompare {
        operator: String,
        left: Register,
        right: Register,
    },
    BoxFloat(Register),
    BoxBool(Register),
    UnboxInt(Register),
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
    pub source: FunctionId,
    pub revision: u64,
    pub registers: Vec<Type>,
    pub blocks: Vec<Block>,
    pub states: Vec<FrameState>,
    pub exits: Vec<SideExit>,
    pub osr_entries: Vec<OsrEntry>,
    /// Boxed registers correspond directly to interpreter frame slots.
    pub hir_values: Vec<Register>,
    pub local_count: usize,
    /// Primitive loop parameters and entry-invariant scalars retain boxed sources for OSR reconstruction.
    pub scalar_sources: BTreeMap<Register, Register>,
    pub scalar_entries: BTreeMap<BlockId, ExitId>,
}

impl Function {
    pub fn lower(hir: &hir::Function, mode: LoweringMode) -> Result<Self> {
        let analysis = hir.analyze()?;
        let profile = hir.profile();
        let mut output = Self {
            source: hir.id(),
            revision: hir.revision(),
            registers: vec![Type::Boxed; hir.value_count],
            blocks: vec![],
            states: vec![],
            exits: vec![],
            osr_entries: vec![],
            hir_values: (0..hir.value_count).map(Register).collect(),
            local_count: hir.local_slots.len(),
            scalar_sources: BTreeMap::new(),
            scalar_entries: BTreeMap::new(),
        };
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
            // Scalar SSA values survive helper calls; their boxed shadows still support frame reconstruction.
            let mut scalars = globals.clone();
            scalars.extend(
                typed_params
                    .iter()
                    .map(|(v, r)| (*v, (ValueKind::Float, *r))),
            );
            for (index, instruction) in body.instructions.iter().enumerate() {
                let origin = instruction.result;
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
                } else if let hir::Op::Constant(Constant::String(text)) = &instruction.op {
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
                            helper: Helper::StringConstant(text.clone()),
                            arguments: vec![],
                            state,
                            roots,
                            effects: Helper::StringConstant(text.clone()).effects(),
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
                    let roots = Vec::new();
                    let truth = output.register(Type::Bool);
                    instructions.push(Instruction {
                        result: Some(truth),
                        origin: *condition,
                        point,
                        operation: Operation::Truth {
                            value: Register(condition.0),
                            state,
                            roots,
                            effects: Effects::READ_ONLY,
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
            environment_root: true,
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
        self.exits.push(SideExit { state, reason });
        id
    }

    pub fn verify(&self) -> Result<()> {
        let fail = || Error::InvalidIr("invalid LIR");
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
        for (raw, source) in &self.scalar_sources {
            kind(*raw, Type::F64)?;
            kind(*source, Type::Boxed)?;
            if self.hir_values.get(source.0) != Some(source) {
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
                .scalar_sources
                .values()
                .any(|source| state.values.iter().any(|(_, r)| r == source))
                && !self.scalar_entries.contains_key(&entry.header)
            {
                return Err(fail());
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
                let expected = match &instruction.operation {
                    Operation::FloatListView { value, failure } => {
                        kind(*value, Type::Boxed)?;
                        let snapshot = state(exit(*failure)?.state)?;
                        if exit(*failure)?.reason != ExitReason::SequenceGuard
                            || snapshot.resume != instruction.point
                            || !snapshot.values.iter().any(|(_, r)| r == value)
                        {
                            return Err(fail());
                        }
                        Some(Type::FloatView)
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
                            if !snapshot.values.iter().any(|(_, r)| *r == input) {
                                return Err(fail());
                            }
                        }
                        Some(Type::Boxed)
                    }
                    Operation::Constant(Constant::String(_)) => return Err(fail()),
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
                            || snapshot.resume != instruction.point
                            || !effects.covers(Effects::READ_ONLY)
                            || roots.iter().copied().collect::<BTreeSet<_>>()
                                != if effects.needs_safepoint() {
                                    snapshot.values.iter().map(|(_, r)| *r).collect()
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
                            {
                                return Err(fail());
                            }
                        }
                        Some(Type::F64)
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
                    Operation::BoxBool(value) => {
                        kind(*value, Type::Bool)?;
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
                        if !helper.accepts(arguments.len())
                            || matches!(helper, Helper::Python(CoreOp::LoadLocal(slot) | CoreOp::StoreLocal(slot) | CoreOp::CheckLocal(slot)) if *slot >= self.local_count)
                        {
                            return Err(fail());
                        }
                        for &arg in arguments {
                            kind(arg, Type::Boxed)?;
                        }
                        let snapshot = state(*id)?;
                        if snapshot.resume != instruction.point {
                            return Err(fail());
                        }
                        let required = snapshot
                            .values
                            .iter()
                            .map(|(_, r)| *r)
                            .collect::<BTreeSet<_>>();
                        if arguments
                            .iter()
                            .any(|argument| !required.contains(argument))
                            || !snapshot.environment_root
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
        for snapshot in &self.states {
            if snapshot.resume.block.0 >= self.blocks.len()
                || !snapshot.environment_root
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
                    | Operation::FloatListStore { view, .. } => {
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
        for (boxed, raw) in self.boxing_recipes() {
            writeln!(text, "materialize {boxed:?} = Float({raw:?})").unwrap();
        }
        for (id, exit) in self.exits.iter().enumerate() {
            writeln!(text, "exit{id}: {:?} -> s{}", exit.reason, exit.state.0).unwrap();
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
            Self::FloatListLoad { view, .. } => vec![*view],
            Self::FloatListStore { view, value, .. } => vec![*view, *value],
            Self::SequenceUnpack { value, .. } => vec![*value],
            Self::SequenceGet { value, index, .. } => match index {
                SequenceIndex::Constant(_) => vec![*value],
                SequenceIndex::Value(index) => vec![*value, *index],
            },
            Self::Constant(_) => vec![],
            Self::GuardInt { value, .. }
            | Self::GuardFloat { value, .. }
            | Self::UnboxFloat(value)
            | Self::IntToFloat(value)
            | Self::BoxFloat(value)
            | Self::BoxBool(value)
            | Self::UnboxInt(value)
            | Self::BoxInt(value)
            | Self::Truth { value, .. } => vec![*value],
            Self::CheckedAdd { left, right, .. }
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
                let snapshot = match &ins.operation {
                    Operation::GuardInt { exit, .. }
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
            Self::Subtract | Self::Less | Self::Len => readonly,
            Self::SetItem => writing,
            Self::StringConstant(_)
            | Self::Add
            | Self::List
            | Self::Tuple
            | Self::Dict
            | Self::GetItem => allocating,
            Self::Python(op) => match op {
                CoreOp::LoadName(_)
                | CoreOp::LoadLocal(_)
                | CoreOp::CheckLocal(_)
                | CoreOp::IsDone
                | CoreOp::Compare(_)
                | CoreOp::Unary(_) => readonly,
                CoreOp::StoreLocal(_) | CoreOp::StoreName(_) => writing,
                CoreOp::Binary(op)
                    if matches!(op.as_str(), "/" | "//" | "%" | "**" | "&" | "|" | "^") =>
                {
                    readonly
                }
                CoreOp::Inplace(op)
                    if matches!(op.as_str(), "-" | "/" | "//" | "%" | "**" | "&" | "|" | "^") =>
                {
                    readonly
                }
                CoreOp::Binary(op) if op == "*" => allocating,
                CoreOp::Missing | CoreOp::Slice | CoreOp::UnpackItem(_) => allocating,
                // Iteration, calls and attribute/format operations retain conservative effects.
                _ => Effects::CONSERVATIVE,
            },
        }
    }
    fn accepts(&self, count: usize) -> bool {
        match self {
            Self::StringConstant(_) => count == 0,
            Self::Add | Self::Subtract | Self::Less | Self::GetItem => count == 2,
            Self::SetItem => count == 3,
            Self::Len => count == 1,
            Self::List | Self::Tuple => true,
            Self::Dict => count.is_multiple_of(2),
            Self::Python(op) => match op {
                CoreOp::Missing
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
                CoreOp::Binary(_)
                | CoreOp::Inplace(_)
                | CoreOp::Compare(_)
                | CoreOp::SetAttr(_) => count == 2,
                CoreOp::Slice => count == 3,
                CoreOp::Call(names) => count == names.len() + 1,
                CoreOp::Function(_) | CoreOp::Class(..) | CoreOp::Set | CoreOp::Join => true,
            },
        }
    }
}

fn unpack_bundle(hir: &hir::Function, block: usize, index: usize, count: usize) -> bool {
    let body = &hir.blocks[block];
    let result = body.instructions[index].result;
    let Some(end) = (index + 1).checked_add(count) else {
        return false;
    };
    if body.instructions.get(index+1..end).is_none_or(|items|
        !items.iter().enumerate().all(|(index,item)|matches!(&item.op,hir::Op::Runtime(CoreOp::UnpackItem(i),inputs) if *i==index && inputs==&[result]))) {return false;}
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
    uses == count
        && !hir
            .local_states
            .iter()
            .flatten()
            .flatten()
            .any(|v| *v == Some(result))
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
