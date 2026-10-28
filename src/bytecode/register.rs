//! Fixed-size register instructions share constants, runtime operations and profile identities with stack code.
use super::Code;
use crate::{
    Error, Result,
    hir::{CoreOp, Edge, Function, Op, ResumePoint, Terminator, ValueId},
};
use std::fmt::Write;

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
    LoadLocal,
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
        for (id, block) in function.blocks.iter().enumerate() {
            output.blocks.push(output.body.len());
            output.emit(Opcode::EnterBlock, 0, id, 0, 0)?;
            let mut offsets = vec![];
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
                    Op::Runtime(CoreOp::LoadLocal(slot), _) => (Opcode::LoadLocal, *slot, 0, 0),
                    Op::Runtime(CoreOp::StoreLocal(slot), inputs) => {
                        (Opcode::StoreLocal, *slot, inputs[0].0, 0)
                    }
                    Op::Runtime(CoreOp::CheckLocal(slot), inputs) => {
                        (Opcode::CheckLocal, *slot, inputs[0].0, 0)
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
            if matches!(edge.op, Opcode::Runtime) {
                let (operation, _) = code
                    .operations
                    .get(edge.a as usize)
                    .ok_or(Error::InvalidIr("register operation"))?;
                writeln!(
                    text,
                    "runtime @{id}: {}",
                    Op::Runtime(operation.clone(), vec![])
                )
                .unwrap();
            }
        }
        for (id, edge) in self.edges.iter().enumerate() {
            writeln!(text, "edge{id}: @{} {:?}", edge.target, edge.pairs).unwrap();
        }
        Ok(text)
    }
}
