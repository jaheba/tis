//! Fixed-size register instructions share constants, runtime operations and profile identities with stack code.
use super::Code;
use super::numeric::{Operator, Plan};
use crate::{
    Error, Result,
    hir::{CoreOp, Edge, Function, Op, ResumePoint, Terminator, ValueId},
};
use std::{cell::Cell, fmt::Write};

#[repr(u8)]
#[derive(Debug, Clone, Copy)]
pub(crate) enum Opcode {
    EnterBlock,
    Constant,
    Add,
    Subtract,
    Less,
    List,
    Tuple,
    Dict,
    GetItem,
    SetItem,
    Len,
    Runtime,
    CallPositional,
    PrepareMethod,
    Numeric,
    LoadLocal,
    LoadName,
    StoreName,
    StoreLocal,
    CheckLocal,
    Jump,
    Branch,
    Return,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub(crate) struct Instruction {
    pub op: Opcode,
    pub dst: u32,
    pub a: u32,
    pub b: u32,
    pub c: u32,
}
#[derive(Debug, Clone)]
pub(crate) struct Transfer {
    pub target: usize,
    pub pairs: Vec<(ValueId, ValueId)>,
}
#[derive(Debug, Clone, Default)]
pub(crate) struct RegisterCode {
    pub body: Vec<Instruction>,
    pub edges: Vec<Transfer>,
    pub names: Vec<Cell<Option<crate::bindings::NameSlot>>>,
    // Cells allow recursive frames to share quickening without retaining a mutable code borrow.
    pub numeric: Vec<Cell<Option<Plan>>>,
    quickened_window: Cell<Option<u32>>,
    pub max_arguments: usize,
    pub max_transfer: usize,
    offsets: Vec<Vec<usize>>,
    blocks: Vec<usize>,
}
fn operand(value: usize) -> Result<u32> {
    u32::try_from(value).map_err(|_| Error::InvalidIr("register operand overflow"))
}
impl RegisterCode {
    pub fn lower(function: &Function, code: &Code) -> Result<Self> {
        let mut output = Self::default();
        let mut uses = vec![0usize; code.sites.len()];
        for block in &function.blocks {
            for instruction in &block.instructions {
                for input in instruction.op.inputs() {
                    uses[input.0] += 1;
                }
            }
            match &block.terminator {
                Some(Terminator::Return(value)) => uses[value.0] += 1,
                Some(Terminator::Jump(edge)) => {
                    for input in &edge.arguments {
                        uses[input.0] += 1;
                    }
                }
                Some(Terminator::Branch { condition, yes, no }) => {
                    uses[condition.0] += 1;
                    for input in yes.arguments.iter().chain(&no.arguments) {
                        uses[input.0] += 1;
                    }
                }
                None => {}
            }
        }
        for (id, block) in function.blocks.iter().enumerate() {
            output.blocks.push(output.body.len());
            output.emit(Opcode::EnterBlock, 0, id, 0, 0)?;
            let mut offsets = vec![];
            // Only a single-use callee consumed in this block may carry an unboxed binding.
            // Escaping methods and values crossing block edges retain ordinary Python objects.
            let callees: std::collections::HashSet<_> = block
                .instructions
                .iter()
                .filter_map(|instruction| {
                    if let Op::Runtime(CoreOp::Call(names), inputs) = &instruction.op
                        && names.iter().all(Option::is_none)
                    {
                        inputs.first().copied()
                    } else {
                        None
                    }
                })
                .collect();
            for instruction in &block.instructions {
                offsets.push(output.body.len());
                let dst = instruction.result.0;
                let site = code.sites[dst]
                    .as_ref()
                    .ok_or(Error::InvalidIr("register site"))?;
                output.max_arguments = output.max_arguments.max(site.inputs.len());
                let (op, a, b, c) = match &instruction.op {
                    Op::Constant(_) => (
                        Opcode::Constant,
                        site.constant
                            .ok_or(Error::InvalidIr("constant pool mapping"))?,
                        0,
                        0,
                    ),
                    Op::Add(a, b) => (Opcode::Add, a.0, b.0, 0),
                    Op::Subtract(a, b) => (Opcode::Subtract, a.0, b.0, 0),
                    Op::Less(a, b) => (Opcode::Less, a.0, b.0, 0),
                    Op::List(_) => (Opcode::List, 0, 0, 0),
                    Op::Tuple(_) => (Opcode::Tuple, 0, 0, 0),
                    Op::Dict(_) => (Opcode::Dict, 0, 0, 0),
                    Op::GetItem(a, b) => (Opcode::GetItem, a.0, b.0, 0),
                    Op::SetItem { object, key, value } => {
                        (Opcode::SetItem, object.0, key.0, value.0)
                    }
                    Op::Len(a) => (Opcode::Len, a.0, 0, 0),
                    Op::Runtime(CoreOp::LoadName(_), _) | Op::Runtime(CoreOp::StoreName(_), _) => {
                        let cache = output.names.len();
                        output.names.push(Cell::new(None));
                        let (opcode, input) =
                            if let Op::Runtime(CoreOp::StoreName(_), inputs) = &instruction.op {
                                (Opcode::StoreName, inputs[0].0)
                            } else {
                                (Opcode::LoadName, 0)
                            };
                        (
                            opcode,
                            site.runtime.ok_or(Error::InvalidIr("name operation"))?,
                            input,
                            cache,
                        )
                    }
                    Op::Runtime(CoreOp::LoadLocal(slot), _) => (Opcode::LoadLocal, *slot, 0, 0),
                    Op::Runtime(CoreOp::StoreLocal(slot), inputs) => {
                        (Opcode::StoreLocal, *slot, inputs[0].0, 0)
                    }
                    Op::Runtime(CoreOp::CheckLocal(slot), inputs) => {
                        (Opcode::CheckLocal, *slot, inputs[0].0, 0)
                    }
                    Op::Runtime(CoreOp::GetAttr(_), inputs)
                        if uses[dst] == 1 && callees.contains(&instruction.result) =>
                    {
                        (
                            Opcode::PrepareMethod,
                            inputs[0].0,
                            site.runtime.ok_or(Error::InvalidIr("method lookup"))?,
                            0,
                        )
                    }
                    Op::Runtime(CoreOp::Call(names), _) if names.iter().all(Option::is_none) => {
                        (Opcode::CallPositional, 0, 0, 0)
                    }
                    Op::Runtime(_, inputs) if Operator::from_hir(&instruction.op).is_some() => {
                        (Opcode::Numeric, inputs[0].0, inputs[1].0, 0)
                    }
                    Op::Runtime(_, _) => (
                        Opcode::Runtime,
                        site.runtime
                            .ok_or(Error::InvalidIr("runtime pool mapping"))?,
                        0,
                        0,
                    ),
                };
                output.emit(op, dst, a, b, c)?;
            }
            offsets.push(output.body.len());
            output.offsets.push(offsets);
            match block
                .terminator
                .as_ref()
                .ok_or(Error::InvalidIr("register terminator"))?
            {
                Terminator::Return(value) => output.emit(Opcode::Return, 0, value.0, 0, 0)?,
                Terminator::Jump(edge) => {
                    let edge = output.edge(function, edge)?;
                    output.emit(Opcode::Jump, 0, edge, 0, 0)?;
                }
                Terminator::Branch { condition, yes, no } => {
                    let yes = output.edge(function, yes)?;
                    let no = output.edge(function, no)?;
                    output.emit(Opcode::Branch, 0, condition.0, yes, no)?;
                }
            }
        }
        for edge in &mut output.edges {
            edge.target = *output
                .blocks
                .get(edge.target)
                .ok_or(Error::InvalidIr("register edge target"))?;
        }
        Ok(output)
    }
    fn emit(&mut self, op: Opcode, dst: usize, a: usize, b: usize, c: usize) -> Result<()> {
        self.numeric.push(Cell::new(None));
        self.body.push(Instruction {
            op,
            dst: operand(dst)?,
            a: operand(a)?,
            b: operand(b)?,
            c: operand(c)?,
        });
        Ok(())
    }
    fn edge(&mut self, function: &Function, edge: &Edge) -> Result<usize> {
        let parameters = &function
            .blocks
            .get(edge.block.0)
            .ok_or(Error::InvalidIr("register block"))?
            .parameters;
        if parameters.len() != edge.arguments.len() {
            return Err(Error::InvalidIr("register edge arity"));
        }
        let id = self.edges.len();
        self.max_transfer = self.max_transfer.max(parameters.len());
        self.edges.push(Transfer {
            target: edge.block.0,
            pairs: parameters
                .iter()
                .copied()
                .zip(edge.arguments.iter().copied())
                .collect(),
        });
        Ok(id)
    }
    pub fn quicken(&self, function: &Function) -> u64 {
        let profile = function.profile.borrow();
        if self.quickened_window.get() == Some(profile.windows) {
            return 0;
        }
        self.quickened_window.set(Some(profile.windows));
        let mut installed = 0;
        for (instruction, slot) in self.body.iter().zip(&self.numeric) {
            let plan = function
                .instruction(ValueId(instruction.dst as usize))
                .and_then(|instruction| Operator::from_hir(&instruction.op))
                .and_then(|operator| {
                    profile
                        .sites
                        .get(instruction.dst as usize)
                        .and_then(|site| Plan::from_profile(operator, site))
                });
            if matches!(
                instruction.op,
                Opcode::Add | Opcode::Subtract | Opcode::Less | Opcode::Numeric
            ) {
                slot.set(plan);
                installed += u64::from(plan.is_some());
            }
        }
        installed
    }

    pub fn resume_offset(&self, point: ResumePoint) -> Result<usize> {
        self.offsets
            .get(point.block.0)
            .and_then(|block| block.get(point.instruction))
            .copied()
            .ok_or(Error::InvalidIr("register resume point"))
    }
    pub fn disassemble(&self, code: &Code) -> Result<String> {
        let mut text = String::new();
        for (pc, i) in self.body.iter().enumerate() {
            writeln!(
                text,
                "{pc:04} {:?} r{} {} {} {}",
                i.op, i.dst, i.a, i.b, i.c
            )
            .unwrap();
        }
        for (id, edge) in self.body.iter().enumerate() {
            if matches!(
                edge.op,
                Opcode::Runtime
                    | Opcode::Numeric
                    | Opcode::CallPositional
                    | Opcode::PrepareMethod
                    | Opcode::LoadName
                    | Opcode::StoreName
            ) {
                let (operation, _) = code
                    .operations
                    .get(
                        if matches!(edge.op, Opcode::Numeric | Opcode::CallPositional) {
                            code.sites[edge.dst as usize]
                                .as_ref()
                                .and_then(|site| site.runtime)
                                .ok_or(Error::InvalidIr("runtime operation"))?
                        } else if matches!(edge.op, Opcode::PrepareMethod) {
                            edge.b as usize
                        } else {
                            edge.a as usize
                        },
                    )
                    .ok_or(Error::InvalidIr("register operation"))?;
                writeln!(
                    text,
                    "runtime @{id}: {}",
                    Op::Runtime(operation.clone(), vec![])
                )
                .unwrap();
            }
        }
        for (id, plan) in self.numeric.iter().enumerate() {
            if let Some(plan) = plan.get() {
                writeln!(text, "specialized @{id}: {plan:?}").unwrap();
            }
        }
        for (id, edge) in self.edges.iter().enumerate() {
            writeln!(text, "edge{id}: @{} {:?}", edge.target, edge.pairs).unwrap();
        }
        Ok(text)
    }
}
