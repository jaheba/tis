//! Copies build-time Rust machine-code stencils and patches read-only operand/continuation records.
//! Runtime linking performs no instruction selection or optimizing compilation.
mod patch;
use crate::{
    Error, Result, Value, Vm,
    bytecode::Code,
    hir::Function,
    lir::{self, FloatOperator, Operation, Register, Terminator},
    native_abi::*,
    native_runtime,
    tier::Outcome,
};
use memmap2::{Mmap, MmapMut};
use patch::Patch;
use std::{cell::RefCell, ffi::c_void, fmt, rc::Rc};
mod generated {
    include!(concat!(env!("OUT_DIR"), "/stencils.rs"));
}
type Entry = unsafe extern "C" fn(*mut c_void, *mut Slot, *const Patch) -> u64;
#[derive(Clone, Copy)]
enum Target {
    Next,
    Node(usize),
    Block(usize),
    None,
}
struct Node {
    stencil: &'static str,
    patch: Patch,
    next: Target,
    alternate: Target,
}
struct Plan {
    nodes: Vec<Node>,
    blocks: Vec<usize>,
    locations: Vec<(usize, usize)>,
    scratch: usize,
    output: usize,
}
impl Plan {
    fn push(&mut self, stencil: &'static str, patch: Patch) -> usize {
        let id = self.nodes.len();
        self.nodes.push(Node {
            stencil,
            patch,
            next: Target::Next,
            alternate: Target::None,
        });
        id
    }
    fn callback(&mut self, event: i32, location: usize) -> Result<()> {
        self.push(
            "callback",
            Patch {
                callback: native_runtime::callback as *const u8 as usize,
                event,
                location: i32::try_from(location).map_err(|_| Error::IntegerOverflow)?,
                ..Patch::default()
            },
        );
        Ok(())
    }
    fn edge(&mut self, edge: &lir::Edge, lir: &lir::Function) -> Result<usize> {
        let start = self.nodes.len();
        // Snapshot all arguments before overwriting any destination block parameter.
        for (index, &source) in edge.arguments.iter().enumerate() {
            self.push(
                "copy",
                Patch {
                    left: slot(source)?,
                    dst: slot(Register(self.output + 1 + index))?,
                    ..Patch::default()
                },
            );
        }
        for (index, &destination) in lir.blocks[edge.block.0].parameters.iter().enumerate() {
            self.push(
                "copy",
                Patch {
                    left: slot(Register(self.output + 1 + index))?,
                    dst: slot(destination)?,
                    ..Patch::default()
                },
            );
        }
        let jump = self.push("jump", Patch::default());
        self.nodes[jump].next = Target::Block(edge.block.0);
        Ok(start)
    }
    fn lower(lir: &lir::Function, feedback: bool) -> Result<Self> {
        lir.verify()?;
        let scratch = lir
            .blocks
            .iter()
            .flat_map(|block| match &block.terminator {
                Terminator::Jump(e) => vec![e.arguments.len()],
                Terminator::Branch { yes, no, .. } => vec![yes.arguments.len(), no.arguments.len()],
                _ => vec![],
            })
            .max()
            .unwrap_or(0);
        let mut plan = Self {
            nodes: vec![],
            blocks: vec![],
            locations: vec![],
            scratch,
            output: lir.registers.len(),
        };
        for (block, body) in lir.blocks.iter().enumerate() {
            plan.blocks.push(plan.nodes.len());
            plan.callback(ENTER, block)?;
            for (index, instruction) in body.instructions.iter().enumerate() {
                let location = plan.locations.len();
                plan.locations.push((block, index));
                let dst = instruction.result.map(slot).transpose()?.unwrap_or(0);
                let mut p = Patch {
                    dst,
                    ..Patch::default()
                };
                let stencil = match &instruction.operation {
                    Operation::Constant(value) => {
                        let value = Slot::boxed(match value {
                            crate::hir::Constant::Unbound => Value::Unbound,
                            crate::hir::Constant::None => Value::None,
                            crate::hir::Constant::Bool(v) => Value::Bool(*v),
                            crate::hir::Constant::Int(v) => Value::Int(*v),
                            crate::hir::Constant::Float(v) => Value::Float(*v),
                            _ => return Err(Error::InvalidIr("allocating stencil constant")),
                        });
                        p.tag = value.tag;
                        p.a = value.a;
                        p.b = value.b;
                        p.c = value.c;
                        "constant"
                    }
                    Operation::GuardInt { value, exit } | Operation::GuardFloat { value, exit } => {
                        p.left = slot(*value)?;
                        p.tag = if matches!(instruction.operation, Operation::GuardInt { .. }) {
                            INT
                        } else {
                            FLOAT
                        };
                        p.status = exit.0 as u64 + 1;
                        "guard"
                    }
                    Operation::UnboxInt(value)
                    | Operation::UnboxFloat(value)
                    | Operation::BoxInt(value)
                    | Operation::BoxFloat(value)
                    | Operation::BoxBool(value) => {
                        p.left = slot(*value)?;
                        p.tag = match instruction.operation {
                            Operation::UnboxInt(_) => RAW_INT,
                            Operation::UnboxFloat(_) => RAW_FLOAT,
                            Operation::BoxInt(_) => INT,
                            Operation::BoxFloat(_) => FLOAT,
                            _ => BOOL,
                        };
                        "retag"
                    }
                    Operation::IntToFloat(value) => {
                        p.left = slot(*value)?;
                        "to_float"
                    }
                    Operation::CheckedAdd {
                        left,
                        right,
                        overflow,
                    } => {
                        p.left = slot(*left)?;
                        p.right = slot(*right)?;
                        p.status = overflow.0 as u64 + 1;
                        "iadd"
                    }
                    Operation::FloatBinary {
                        operator,
                        left,
                        right,
                        failure,
                    } => {
                        p.left = slot(*left)?;
                        p.right = slot(*right)?;
                        p.status = failure.0 as u64 + 1;
                        p.power = native_runtime::power as *const u8 as usize;
                        match operator {
                            FloatOperator::Add => "fadd",
                            FloatOperator::Subtract => "fsub",
                            FloatOperator::Multiply => "fmul",
                            FloatOperator::Divide => "fdiv",
                            FloatOperator::Power => "fpow",
                        }
                    }
                    Operation::FloatCompare {
                        operator,
                        left,
                        right,
                    } => {
                        p.left = slot(*left)?;
                        p.right = slot(*right)?;
                        match operator.as_str() {
                            "==" => "feq",
                            "!=" => "fne",
                            "<" => "flt",
                            "<=" => "fle",
                            ">" => "fgt",
                            ">=" => "fge",
                            _ => return Err(Error::InvalidIr("stencil comparison")),
                        }
                    }
                    Operation::Call { .. } | Operation::Truth { .. } => {
                        p.callback = native_runtime::callback as *const u8 as usize;
                        p.event = HELPER;
                        p.location = i32::try_from(location).map_err(|_| Error::IntegerOverflow)?;
                        "callback"
                    }
                };
                plan.push(stencil, p);
                // Pure boxed results contain no heap references and remain available in native registers.
                if feedback
                    && instruction
                        .result
                        .is_some_and(|register| lir.registers[register.0] == lir::Type::Boxed)
                    && !matches!(instruction.operation, Operation::Call { .. })
                {
                    plan.callback(FINISH, location)?;
                }
            }
            match &body.terminator {
                Terminator::Return(value) => {
                    let id = plan.push(
                        "return",
                        Patch {
                            left: slot(*value)?,
                            dst: slot(Register(plan.output))?,
                            ..Patch::default()
                        },
                    );
                    plan.nodes[id].next = Target::None;
                }
                Terminator::Jump(edge) => {
                    plan.edge(edge, lir)?;
                }
                Terminator::Branch { condition, yes, no } => {
                    let branch = plan.push(
                        "branch",
                        Patch {
                            left: slot(*condition)?,
                            ..Patch::default()
                        },
                    );
                    let yes = plan.edge(yes, lir)?;
                    let no = plan.edge(no, lir)?;
                    plan.nodes[branch].next = Target::Node(yes);
                    plan.nodes[branch].alternate = Target::Node(no);
                }
            }
        }
        Ok(plan)
    }
}
fn slot(register: Register) -> Result<usize> {
    register
        .0
        .checked_mul(std::mem::size_of::<Slot>())
        .ok_or(Error::IntegerOverflow)
}
fn error(message: impl fmt::Display) -> Error {
    Error::Runtime(format!("copy-and-patch: {message}"))
}
fn align(value: usize, alignment: usize) -> Result<usize> {
    value
        .checked_add(alignment - 1)
        .map(|value| value & !(alignment - 1))
        .ok_or(Error::IntegerOverflow)
}
fn template(name: &str) -> Result<&'static [u8]> {
    generated::STENCILS
        .iter()
        .find(|(candidate, _)| *candidate == name)
        .map(|(_, code)| *code)
        .ok_or_else(|| error(format!("no {name} stencil for this target")))
}

pub(crate) struct CopyCode {
    memory: Mmap,
    lir: Rc<lir::Function>,
    locations: Vec<(usize, usize)>,
    blocks: Vec<(usize, usize)>,
    scratch: usize,
    pub code_bytes: usize,
    pub data_bytes: usize,
    pub stencils: usize,
    pub relocations: usize,
    pub feedback: bool,
}
impl fmt::Debug for CopyCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CopyCode")
            .field("source", &self.lir.source)
            .field("stencils", &self.stencils)
            .field("bytes", &self.memory.len())
            .finish()
    }
}
impl CopyCode {
    pub fn compile(lir: Rc<lir::Function>, feedback: bool) -> Result<Self> {
        if generated::STENCILS.is_empty() {
            return Err(error("stencils are available only for AArch64 and x86-64"));
        }
        let plan = Plan::lower(&lir, feedback)?;
        let mut offsets = vec![];
        let mut size = 0;
        for node in &plan.nodes {
            size = align(size, 16)?;
            offsets.push(size);
            size = size
                .checked_add(template(node.stencil)?.len())
                .ok_or(Error::IntegerOverflow)?;
        }
        let code_bytes = size;
        let patches = align(size, std::mem::align_of::<Patch>())?;
        let data_bytes = plan
            .nodes
            .len()
            .checked_mul(std::mem::size_of::<Patch>())
            .ok_or(Error::IntegerOverflow)?;
        let length = patches
            .checked_add(data_bytes)
            .ok_or(Error::IntegerOverflow)?;
        let mut memory = MmapMut::map_anon(length).map_err(error)?;
        let base = memory.as_ptr() as usize;
        let patch_offset = |index: usize| patches + index * std::mem::size_of::<Patch>();
        let target = |target: Target, current: usize| -> Result<Option<usize>> {
            let index = match target {
                Target::Next => current.checked_add(1).ok_or(Error::IntegerOverflow)?,
                Target::Node(index) => index,
                Target::Block(block) => *plan
                    .blocks
                    .get(block)
                    .ok_or(Error::InvalidIr("stencil block"))?,
                Target::None => return Ok(None),
            };
            if index >= plan.nodes.len() {
                return Err(Error::InvalidIr("missing stencil continuation"));
            }
            Ok(Some(index))
        };
        let frame_bytes = slot(Register(
            lir.registers
                .len()
                .checked_add(1 + plan.scratch)
                .ok_or(Error::IntegerOverflow)?,
        ))?;
        let mut relocations = 0;
        for (index, node) in plan.nodes.iter().enumerate() {
            let bytes = template(node.stencil)?;
            memory[offsets[index]..offsets[index] + bytes.len()].copy_from_slice(bytes);
            let mut patch = node.patch;
            for offset in [patch.dst, patch.left, patch.right] {
                if !offset.is_multiple_of(std::mem::size_of::<Slot>())
                    || offset
                        .checked_add(std::mem::size_of::<Slot>())
                        .is_none_or(|end| end > frame_bytes)
                {
                    return Err(Error::InvalidIr("stencil register offset"));
                }
            }
            if let Some(next) = target(node.next, index)? {
                patch.next = base + offsets[next];
                patch.next_patch = base + patch_offset(next);
                relocations += 2;
            }
            if let Some(alternate) = target(node.alternate, index)? {
                patch.alternate = base + offsets[alternate];
                patch.alternate_patch = base + patch_offset(alternate);
                relocations += 2;
            }
            // SAFETY: the anonymous writable image contains aligned space for every Patch. No
            // entry is callable until all bytes and continuation pointers have been initialized.
            unsafe {
                memory
                    .as_mut_ptr()
                    .add(patch_offset(index))
                    .cast::<Patch>()
                    .write(patch);
            }
        }
        let memory = memory.make_exec().map_err(error)?;
        if memory.as_ptr() as usize != base {
            return Err(error("executable mapping moved"));
        }
        // SAFETY: this immutable, live image contains the copied code and all patched records.
        // Flush before publishing an entry pointer, including cross-core instruction pipelines.
        unsafe {
            wasmtime_internal_jit_icache_coherence::clear_cache(
                memory.as_ptr().cast(),
                memory.len(),
            )
            .map_err(error)?;
        }
        wasmtime_internal_jit_icache_coherence::pipeline_flush_mt().map_err(error)?;
        let blocks = plan
            .blocks
            .iter()
            .map(|&node| (offsets[node], patch_offset(node)))
            .collect();
        Ok(Self {
            memory,
            lir,
            locations: plan.locations,
            blocks,
            scratch: plan.scratch,
            code_bytes,
            data_bytes,
            stencils: plan.nodes.len(),
            relocations,
            feedback,
        })
    }
    #[allow(clippy::too_many_arguments)]
    pub fn execute(
        &self,
        vm: &mut Vm,
        function: &Rc<RefCell<Function>>,
        code: &Code,
        base: usize,
        frame_end: usize,
        start: usize,
        osr: bool,
    ) -> Result<Outcome> {
        let &(entry, patch) = self
            .blocks
            .get(start)
            .ok_or(Error::InvalidIr("copy-and-patch entry"))?;
        native_runtime::execute(
            &self.lir,
            &self.locations,
            self.scratch,
            |context, registers, _| {
                // SAFETY: these bytes came from a verified same-target C-ABI stencil. This borrowed
                // Rc-owned image and its patch records outlive the synchronous, tail-threaded call.
                unsafe {
                    let entry: Entry = std::mem::transmute(self.memory.as_ptr().add(entry));
                    entry(context, registers, self.memory.as_ptr().add(patch).cast())
                }
            },
            vm,
            function,
            code,
            base,
            frame_end,
            start,
            osr,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stencil_library_contains_offline_code() {
        assert_eq!(generated::STENCILS.len(), 21);
        assert!(
            generated::STENCILS
                .iter()
                .all(|(_, bytes)| !bytes.is_empty())
        );
    }
    #[test]
    fn templates_cannot_resolve_unknown_names() {
        assert!(template("invalid").is_err());
    }
}
