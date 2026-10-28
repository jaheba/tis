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
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitReason {
    TypeGuard,
    IntegerOverflow,
    FloatArithmetic,
}
#[derive(Debug, Clone)]
pub struct SideExit {
    pub state: StateId,
    pub reason: ExitReason,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Effects {
    pub may_raise: bool,
    pub may_allocate: bool,
    pub may_call: bool,
    pub may_write: bool,
}
impl Effects {
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
#[derive(Debug, Clone)]
pub enum Operation {
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
        };
        for (block_id, body) in hir.blocks.iter().enumerate() {
            let mut instructions = vec![];
            for (index, instruction) in body.instructions.iter().enumerate() {
                let origin = instruction.result;
                let result = Register(origin.0);
                let state = output.state(
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
                if let Some(signature) = float_signature
                    && (float_operator.is_some()
                        || (float_compare.is_some()
                            && signature == [ValueKind::Float, ValueKind::Float]))
                {
                    let guard = output.exit(state, ExitReason::TypeGuard);
                    let inputs = instruction.op.inputs();
                    let mut args = vec![];
                    for (input, kind) in inputs.into_iter().zip(signature) {
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
                    emit(
                        &mut instructions,
                        None,
                        Operation::GuardInt {
                            value: Register(left.0),
                            exit: guard,
                        },
                    );
                    emit(
                        &mut instructions,
                        None,
                        Operation::GuardInt {
                            value: Register(right.0),
                            exit: guard,
                        },
                    );
                    let a = output.register(Type::I64);
                    let b = output.register(Type::I64);
                    let sum = output.register(Type::I64);
                    emit(
                        &mut instructions,
                        Some(a),
                        Operation::UnboxInt(Register(left.0)),
                    );
                    emit(
                        &mut instructions,
                        Some(b),
                        Operation::UnboxInt(Register(right.0)),
                    );
                    emit(
                        &mut instructions,
                        Some(sum),
                        Operation::CheckedAdd {
                            left: a,
                            right: b,
                            overflow,
                        },
                    );
                    emit(&mut instructions, Some(result), Operation::BoxInt(sum));
                } else if let hir::Op::Constant(Constant::String(text)) = &instruction.op {
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
                            effects: Effects::CONSERVATIVE,
                        },
                    );
                } else if let hir::Op::Constant(constant) = &instruction.op {
                    emit(
                        &mut instructions,
                        Some(result),
                        Operation::Constant(constant.clone()),
                    );
                } else {
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
                    let roots = output.states[state.0]
                        .values
                        .iter()
                        .map(|(_, register)| *register)
                        .collect();
                    emit(
                        &mut instructions,
                        Some(result),
                        Operation::Call {
                            helper,
                            arguments,
                            state,
                            roots,
                            effects: Effects::CONSERVATIVE,
                        },
                    );
                }
            }
            let edge = |edge: &hir::Edge| Edge {
                block: edge.block,
                arguments: edge.arguments.iter().map(|id| Register(id.0)).collect(),
            };
            let terminator = match body.terminator.as_ref().unwrap() {
                hir::Terminator::Return(id) => Terminator::Return(Register(id.0)),
                hir::Terminator::Jump(e) => Terminator::Jump(edge(e)),
                hir::Terminator::Branch { condition, yes, no } => {
                    let point = ResumePoint {
                        block: BlockId(block_id),
                        instruction: body.instructions.len(),
                    };
                    let state = output.state(point, &analysis.live_terminator[block_id]);
                    let roots = output.states[state.0]
                        .values
                        .iter()
                        .map(|(_, r)| *r)
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
                parameters: body.parameters.iter().map(|id| Register(id.0)).collect(),
                instructions,
                terminator,
            });
        }
        let headers = analysis
            .backedges
            .iter()
            .map(|(_, header)| *header)
            .collect::<BTreeSet<_>>();
        for header in headers {
            let state = output.state(
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
    fn state(&mut self, resume: ResumePoint, live: &[ValueId]) -> StateId {
        let id = StateId(self.states.len());
        self.states.push(FrameState {
            resume,
            values: live
                .iter()
                .map(|value| (*value, Register(value.0)))
                .collect(),
            environment_root: true,
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
        let mut definitions = BTreeSet::new();
        for body in &self.blocks {
            for &parameter in &body.parameters {
                kind(parameter, Type::Boxed)?;
                if !definitions.insert(parameter) {
                    return Err(fail());
                }
            }
            let mut guarded = BTreeMap::new();
            let mut unboxed_sources = BTreeMap::new();
            for instruction in &body.instructions {
                let expected = match &instruction.operation {
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
                            || !effects.may_raise
                            || roots.iter().copied().collect::<BTreeSet<_>>()
                                != snapshot.values.iter().map(|(_, r)| *r).collect()
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
                        if !helper.accepts(arguments.len()) {
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
                            || roots.iter().copied().collect::<BTreeSet<_>>() != required
                            || !effects.may_raise
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
                for &arg in &edge.arguments {
                    kind(arg, Type::Boxed)?;
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
            if snapshot.resume.block.0 >= self.blocks.len() || !snapshot.environment_root {
                return Err(fail());
            }
            let mut slots = BTreeSet::new();
            for (value, register) in &snapshot.values {
                kind(*register, Type::Boxed)?;
                if self.hir_values.get(value.0) != Some(register) || !slots.insert(*value) {
                    return Err(fail());
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
        for (id, exit) in self.exits.iter().enumerate() {
            writeln!(text, "exit{id}: {:?} -> s{}", exit.reason, exit.state.0).unwrap();
        }
        for (id, state) in self.states.iter().enumerate() {
            writeln!(
                text,
                "s{id}: resume b{}:{} env=current slots={:?}",
                state.resume.block.0, state.resume.instruction, state.values
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
}
impl Function {
    pub fn reconstruct(
        &self,
        state: StateId,
        environment: crate::Value,
        mut read: impl FnMut(Register) -> Result<crate::Value>,
    ) -> Result<DeoptFrame> {
        let snapshot = self
            .states
            .get(state.0)
            .ok_or(Error::InvalidIr("invalid frame state"))?;
        let mut slots = vec![None; self.hir_values.len()];
        for (value, register) in &snapshot.values {
            *slots
                .get_mut(value.0)
                .ok_or(Error::InvalidIr("invalid frame slot"))? = Some(read(*register)?);
        }
        Ok(DeoptFrame {
            function: self.source,
            revision: self.revision,
            resume: snapshot.resume,
            environment,
            slots,
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
    fn inputs(&self) -> Vec<Register> {
        match self {
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
    fn accepts(&self, count: usize) -> bool {
        match self {
            Self::StringConstant(_) => count == 0,
            Self::Add | Self::Subtract | Self::Less | Self::GetItem => count == 2,
            Self::SetItem => count == 3,
            Self::Len => count == 1,
            Self::List | Self::Tuple => true,
            Self::Dict => count.is_multiple_of(2),
            Self::Python(op) => match op {
                CoreOp::Missing | CoreOp::LoadName(_) | CoreOp::Namespace => count == 0,
                CoreOp::StoreName(_)
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
