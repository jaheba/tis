//! Stack and register encodings lowered exclusively from HIR.
pub(crate) mod numeric;
pub(crate) mod register;
pub use numeric::NumericMode;

use std::fmt::Write;

use crate::{
    Error, Result,
    hir::{BlockId, Constant, Edge, Function, Op, Terminator, ValueId},
};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum BytecodeMode {
    Stack,
    #[default]
    Register,
}

pub(crate) type ReadyWindow = (u64, u32, crate::NumericMode);

#[derive(Debug, Clone)]
pub struct Code {
    pub(crate) register: register::RegisterCode,
    pub(crate) body: Vec<u8>,
    pub function: crate::profile::FunctionId,
    pub revision: u64,
    pub(crate) backedge_sources: Vec<Vec<usize>>,
    pub(crate) backedge_offsets: Vec<usize>,
    pub(crate) ready_windows: Vec<std::cell::Cell<Option<ReadyWindow>>>,
    pub(crate) sites: Vec<Option<Site>>,
    offsets: Vec<Vec<usize>>,
    block_offsets: Vec<usize>,
    pub(crate) constants: Vec<EncodedConstant>,
    pub(crate) literal_cache: Vec<std::cell::Cell<Option<crate::Value>>>,
    pub(crate) slots: usize,
    pub(crate) entry_parameters: Vec<ValueId>,
    pub(crate) local_names: Vec<String>,
    pub(crate) local_parameters: Vec<Option<usize>>,
    pub(crate) environment_required: bool,
    pub(crate) operations: Vec<(crate::hir::CoreOp, usize)>,
}

#[derive(Debug, Clone)]
pub(crate) struct Site {
    pub inputs: Vec<ValueId>,
    pub call: bool,
    pub constant: Option<usize>,
    pub runtime: Option<usize>,
}

#[repr(u8)]
#[derive(Debug, Clone, Copy)]
pub(crate) enum Opcode {
    Constant,
    Load,
    Store,
    Add,
    Subtract,
    Less,
    List,
    Tuple,
    Dict,
    GetItem,
    SetItem,
    Len,
    Jump,
    JumpIfFalse,
    Return,
    Runtime,
    EnterBlock,
    Observe,
    LoadLocal,
    StoreLocal,
    CheckLocal,
}

impl Opcode {
    pub(crate) fn decode(byte: u8) -> Result<Self> {
        Ok(match byte {
            0 => Self::Constant,
            1 => Self::Load,
            2 => Self::Store,
            3 => Self::Add,
            4 => Self::Subtract,
            5 => Self::Less,
            6 => Self::List,
            7 => Self::Tuple,
            8 => Self::Dict,
            9 => Self::GetItem,
            10 => Self::SetItem,
            11 => Self::Len,
            12 => Self::Jump,
            13 => Self::JumpIfFalse,
            14 => Self::Return,
            15 => Self::Runtime,
            16 => Self::EnterBlock,
            17 => Self::Observe,
            18 => Self::LoadLocal,
            19 => Self::StoreLocal,
            20 => Self::CheckLocal,
            _ => return Err(Error::InvalidIr("invalid opcode")),
        })
    }

    fn has_operand(self) -> bool {
        matches!(
            self,
            Self::EnterBlock
                | Self::LoadLocal
                | Self::StoreLocal
                | Self::CheckLocal
                | Self::Observe
                | Self::Runtime
                | Self::Constant
                | Self::Load
                | Self::Store
                | Self::Add
                | Self::List
                | Self::Tuple
                | Self::Dict
                | Self::Jump
                | Self::JumpIfFalse
        )
    }
}

impl Code {
    /// Merge invocation-owned quiet counts without changing detailed observation windows.
    pub(crate) fn flush_backedges(
        &self,
        profile: &mut crate::profile::FunctionProfile,
        pending: &mut [u64],
    ) {
        for (to, sources) in self.backedge_sources.iter().enumerate() {
            for (slot, &from) in sources.iter().enumerate() {
                let count = std::mem::take(&mut pending[self.backedge_offsets[to] + slot]);
                if count != 0 {
                    let edge = profile.backedges.entry((from, to)).or_default();
                    *edge = edge.saturating_add(count);
                }
            }
        }
    }

    pub fn lower(function: &Function) -> Result<Self> {
        // Bytecode roots its register window; liveness snapshots are needed only by LIR.
        let mut backedge_sources = vec![Vec::new(); function.blocks.len()];
        for (from, to) in function.validated_backedges()? {
            backedge_sources[to].push(from);
        }
        let mut backedge_offsets = vec![0];
        for sources in &backedge_sources {
            backedge_offsets.push(backedge_offsets.last().unwrap() + sources.len());
        }
        let mut sites = vec![None; function.value_count];
        for block in &function.blocks {
            for instruction in &block.instructions {
                sites[instruction.result.0] = Some(Site {
                    inputs: instruction.op.inputs(),
                    call: matches!(&instruction.op, Op::Runtime(crate::hir::CoreOp::Call(_), _)),
                    constant: None,
                    runtime: None,
                });
            }
        }
        let mut builder = Builder {
            code: Self {
                register: register::RegisterCode::default(),
                body: Vec::new(),
                function: function.id(),
                revision: function.revision(),
                backedge_sources,
                backedge_offsets,
                ready_windows: vec![std::cell::Cell::new(None); function.blocks.len()],
                sites,
                offsets: Vec::new(),
                block_offsets: Vec::new(),
                constants: Vec::new(),
                literal_cache: Vec::new(),
                slots: function.value_count,
                entry_parameters: function.parameters(function.entry()).to_vec(),
                local_names: function.local_slots.clone(),
                local_parameters: function
                    .local_slots
                    .iter()
                    .map(|name| {
                        function
                            .parameter_names
                            .iter()
                            .position(|parameter| parameter == name)
                    })
                    .collect(),
                environment_required: function.environment_required(),
                operations: Vec::new(),
            },
            patches: Vec::new(),
        };
        let mut offsets = Vec::new();
        for (block_id, block) in function.blocks.iter().enumerate() {
            offsets.push(builder.code.body.len());
            builder.code.block_offsets.push(builder.code.body.len());
            builder.operand(Opcode::EnterBlock, block_id)?;
            let mut instruction_offsets = Vec::new();
            for instruction in &block.instructions {
                instruction_offsets.push(builder.code.body.len());
                builder.operand(Opcode::Observe, instruction.result.0)?;
                builder.operation(&instruction.op, instruction.result)?;
                builder.slot(Opcode::Store, instruction.result)?;
            }
            instruction_offsets.push(builder.code.body.len());
            builder.code.offsets.push(instruction_offsets);
            match block
                .terminator
                .as_ref()
                .ok_or(Error::InvalidIr("missing terminator"))?
            {
                Terminator::Return(value) => {
                    builder.slot(Opcode::Load, *value)?;
                    builder.emit(Opcode::Return);
                }
                Terminator::Jump(edge) => builder.edge(function, edge)?,
                Terminator::Branch { condition, yes, no } => {
                    builder.slot(Opcode::Load, *condition)?;
                    let otherwise = builder.operand(Opcode::JumpIfFalse, 0)?;
                    builder.edge(function, yes)?;
                    builder.patch(otherwise, builder.code.body.len())?;
                    builder.edge(function, no)?;
                }
            }
        }
        for (position, block) in builder.patches.clone() {
            builder.patch(
                position,
                *offsets
                    .get(block.0)
                    .ok_or(Error::InvalidIr("invalid block"))?,
            )?;
        }
        builder.code.literal_cache = (0..builder.code.constants.len())
            .map(|_| std::cell::Cell::new(None))
            .collect();
        builder.code.register = register::RegisterCode::lower(function, &builder.code)?;
        Ok(builder.code)
    }

    /// Resume before operand loads, with an empty operand stack and restored HIR slots.
    pub fn resume_offset(&self, point: crate::hir::ResumePoint) -> Result<usize> {
        self.offsets
            .get(point.block.0)
            .and_then(|offsets| offsets.get(point.instruction))
            .copied()
            .ok_or(Error::InvalidIr("invalid resume point"))
    }
    pub fn block_offset(&self, block: BlockId) -> Result<usize> {
        self.block_offsets
            .get(block.0)
            .copied()
            .ok_or(Error::InvalidIr("invalid block"))
    }

    pub fn bytes(&self) -> &[u8] {
        &self.body
    }

    pub fn disassemble_register(&self) -> Result<String> {
        self.register.disassemble(self)
    }

    pub fn register_instruction_count(&self) -> usize {
        self.register.body.len()
    }

    pub fn stack_instruction_count(&self) -> Result<usize> {
        let mut pc = 0;
        let mut count = 0;
        while pc < self.body.len() {
            let op = Opcode::decode(self.body[pc])?;
            pc += 1;
            count += 1;
            if op.has_operand() {
                read_operand(&self.body, &mut pc)?;
            }
        }
        Ok(count)
    }

    pub fn register_body_size(&self) -> usize {
        self.register.body.len() * std::mem::size_of::<register::Instruction>()
    }

    pub(crate) fn resume_in(
        &self,
        mode: BytecodeMode,
        point: crate::hir::ResumePoint,
    ) -> Result<usize> {
        match mode {
            BytecodeMode::Stack => self.resume_offset(point),
            BytecodeMode::Register => self.register.resume_offset(point),
        }
    }

    pub fn disassemble(&self) -> Result<String> {
        let mut output = String::new();
        let mut ip = 0;
        while ip < self.body.len() {
            let start = ip;
            let opcode = Opcode::decode(self.body[ip])?;
            ip += 1;
            write!(output, "{start:04} {opcode:?}").unwrap();
            if opcode.has_operand() {
                let operand = read_operand(&self.body, &mut ip)?;
                write!(output, " {operand}").unwrap();
                if matches!(
                    opcode,
                    Opcode::LoadLocal | Opcode::StoreLocal | Opcode::CheckLocal
                ) {
                    let name = self
                        .local_names
                        .get(operand)
                        .ok_or(Error::InvalidIr("local slot"))?;
                    write!(output, " ({name})").unwrap();
                }
                if matches!(opcode, Opcode::Runtime) {
                    let (operation, _) = self
                        .operations
                        .get(operand)
                        .ok_or(Error::InvalidIr("runtime operation"))?;
                    write!(output, " ({})", Op::Runtime(operation.clone(), vec![])).unwrap();
                }
                if matches!(opcode, Opcode::Constant) {
                    write!(
                        output,
                        " ({:?})",
                        self.constants
                            .get(operand)
                            .ok_or(Error::InvalidIr("invalid constant"))?
                    )
                    .unwrap();
                }
            }
            output.push('\n');
        }
        Ok(output)
    }
}

// Immediates contain no heap handles, so compiled code can share them across VMs.
#[derive(Debug, Clone)]
pub(crate) enum EncodedConstant {
    Immediate(crate::Value),
    String(String),
}
impl EncodedConstant {
    fn new(value: &Constant) -> Self {
        if let Some(value) = value.immediate() {
            Self::Immediate(value)
        } else {
            let Constant::String(text) = value else {
                unreachable!()
            };
            Self::String(text.clone())
        }
    }
}

struct Builder {
    code: Code,
    patches: Vec<(usize, BlockId)>,
}

impl Builder {
    fn operation(&mut self, op: &Op, result: ValueId) -> Result<()> {
        if let Op::Constant(value) = op {
            let id = self.code.constants.len();
            self.code.constants.push(EncodedConstant::new(value));
            self.code.sites[result.0].as_mut().unwrap().constant = Some(id);
            self.operand(Opcode::Constant, id)?;
            return Ok(());
        }
        for input in op.inputs() {
            self.slot(Opcode::Load, input)?;
        }
        match op {
            Op::Add(..) => {
                self.operand(Opcode::Add, result.0)?;
            }
            Op::Subtract(..) => self.emit(Opcode::Subtract),
            Op::Less(..) => self.emit(Opcode::Less),
            Op::List(values) => {
                self.operand(Opcode::List, values.len())?;
            }
            Op::Tuple(values) => {
                self.operand(Opcode::Tuple, values.len())?;
            }
            Op::Dict(values) => {
                self.operand(Opcode::Dict, values.len())?;
            }
            Op::GetItem(..) => self.emit(Opcode::GetItem),
            Op::SetItem { .. } => self.emit(Opcode::SetItem),
            Op::Len(..) => self.emit(Opcode::Len),
            Op::Runtime(crate::hir::CoreOp::LoadLocal(slot), _) => {
                self.operand(Opcode::LoadLocal, *slot)?;
            }
            Op::Runtime(crate::hir::CoreOp::StoreLocal(slot), _) => {
                self.operand(Opcode::StoreLocal, *slot)?;
            }
            Op::Runtime(crate::hir::CoreOp::CheckLocal(slot), _) => {
                self.operand(Opcode::CheckLocal, *slot)?;
            }
            Op::Runtime(operation, values) => {
                let id = self.code.operations.len();
                self.code.operations.push((operation.clone(), values.len()));
                self.code.sites[result.0].as_mut().unwrap().runtime = Some(id);
                self.operand(Opcode::Runtime, id)?;
            }
            Op::Constant(_) => unreachable!(),
        }
        Ok(())
    }

    fn edge(&mut self, function: &Function, edge: &Edge) -> Result<()> {
        let block = function
            .blocks
            .get(edge.block.0)
            .ok_or(Error::InvalidIr("invalid block"))?;
        if edge.arguments.len() != block.parameters.len() {
            return Err(Error::InvalidIr("block argument count"));
        }
        // Load all arguments before overwriting any parameter, including swapped backedges.
        for &value in &edge.arguments {
            self.slot(Opcode::Load, value)?;
        }
        for &parameter in block.parameters.iter().rev() {
            self.slot(Opcode::Store, parameter)?;
        }
        let position = self.operand(Opcode::Jump, 0)?;
        self.patches.push((position, edge.block));
        Ok(())
    }

    fn slot(&mut self, opcode: Opcode, value: ValueId) -> Result<()> {
        if value.0 >= self.code.slots {
            return Err(Error::InvalidIr("invalid value"));
        }
        self.operand(opcode, value.0)?;
        Ok(())
    }

    fn emit(&mut self, opcode: Opcode) {
        self.code.body.push(opcode as u8);
    }

    fn operand(&mut self, opcode: Opcode, value: usize) -> Result<usize> {
        self.emit(opcode);
        let position = self.code.body.len();
        self.code.body.extend_from_slice(
            &u32::try_from(value)
                .map_err(|_| Error::InvalidIr("bytecode operand overflow"))?
                .to_le_bytes(),
        );
        Ok(position)
    }

    fn patch(&mut self, position: usize, target: usize) -> Result<()> {
        self.code.body[position..position + 4].copy_from_slice(
            &u32::try_from(target)
                .map_err(|_| Error::InvalidIr("bytecode offset overflow"))?
                .to_le_bytes(),
        );
        Ok(())
    }
}

pub(crate) fn read_operand(body: &[u8], ip: &mut usize) -> Result<usize> {
    let bytes = body
        .get(
            *ip..ip
                .checked_add(4)
                .ok_or(Error::InvalidIr("instruction pointer overflow"))?,
        )
        .ok_or(Error::InvalidIr("truncated operand"))?;
    *ip += 4;
    Ok(u32::from_le_bytes(bytes.try_into().unwrap()) as usize)
}
