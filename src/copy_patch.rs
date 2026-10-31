//! Copies build-time Rust machine-code stencils and patches read-only operand/continuation records.
//! Runtime linking performs no instruction selection or optimizing compilation.
mod patch;
#[cfg(any(all(target_arch = "aarch64", target_endian = "little"), test))]
mod tail;
use crate::{
    Error, Result, Vm,
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
    done_slow: Option<usize>,
    patch_owner: Option<usize>,
    cold: bool,
}
struct Plan {
    gateway: usize,
    nodes: Vec<Node>,
    blocks: Vec<usize>,
    locations: Vec<(usize, usize)>,
    scratch: usize,
    output: usize,
    entry_locals: Vec<(Register, usize)>,
    local_metadata: Option<usize>,
    // Boxes keep descriptor addresses stable while lowering appends more regions.
    #[allow(clippy::vec_box)]
    fills: Vec<Box<RangeFill>>,
    #[allow(clippy::vec_box)]
    reductions: Vec<Box<RangeReduce>>,
}
impl Plan {
    fn push(&mut self, stencil: &'static str, patch: Patch) -> usize {
        let id = self.nodes.len();
        self.nodes.push(Node {
            stencil,
            patch,
            next: Target::Next,
            alternate: Target::None,
            done_slow: None,
            patch_owner: None,
            cold: false,
        });
        id
    }
    fn split(&mut self, stencil: &'static str, patch: Patch, slow: &'static str) -> usize {
        let fast = self.push(stencil, patch);
        let cold = self.push(slow, patch);
        self.nodes[cold].patch_owner = Some(fast);
        self.nodes[cold].cold = true;
        self.nodes[fast].next = Target::Node(self.nodes.len());
        self.nodes[fast].alternate = Target::Node(cold);
        fast
    }
    fn callback(&mut self, event: i32, location: usize) -> Result<()> {
        self.push(
            "callback",
            Patch {
                callback: self.gateway,
                event,
                location: i32::try_from(location).map_err(|_| Error::IntegerOverflow)?,
                ..Patch::default()
            },
        );
        Ok(())
    }
    fn edge(&mut self, edge: &lir::Edge, lir: &lir::Function) -> Result<usize> {
        let start = self.nodes.len();
        let mut pending = lir.blocks[edge.block.0]
            .parameters
            .iter()
            .copied()
            .zip(edge.arguments.iter().copied())
            .filter(|(dst, src)| dst != src)
            .collect::<Vec<_>>();
        let temporary = Register(self.output + 1);
        while !pending.is_empty() {
            if let Some(index) = pending
                .iter()
                .position(|(dst, _)| !pending.iter().any(|(_, src)| src == dst))
            {
                let (dst, src) = pending.remove(index);
                self.push(
                    "copy",
                    Patch {
                        left: slot(src)?,
                        dst: slot(dst)?,
                        ..Patch::default()
                    },
                );
            } else {
                // Save one old destination to break a cycle. Acyclic transfers need
                // no scratch copies; duplicated sources still read the original value.
                let saved = pending[0].0;
                self.push(
                    "copy",
                    Patch {
                        left: slot(saved)?,
                        dst: slot(temporary)?,
                        ..Patch::default()
                    },
                );
                for (_, src) in &mut pending {
                    if *src == saved {
                        *src = temporary;
                    }
                }
            }
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
        let entry_locals = if !feedback {
            // Inlined parameter loads alias their argument registers; remaining LoadLocal
            // helpers refer to this physical frame, as in the shared native entry contract.
            let loads = lir.invariant_local_loads();
            if loads.len() >= 2 { loads } else { vec![] }
        } else {
            vec![]
        };
        // Put the readiness flag after the return slot and every parallel-edge temporary.
        let local_metadata =
            (!entry_locals.is_empty()).then_some(lir.registers.len() + 1 + scratch);
        let scratch = scratch + usize::from(local_metadata.is_some());
        let mut plan = Self {
            gateway: if lir.inline_scopes.is_empty() {
                native_runtime::callback_no_inline as *const u8 as usize
            } else {
                native_runtime::callback as *const u8 as usize
            },
            nodes: vec![],
            blocks: vec![],
            locations: vec![],
            scratch,
            output: lir.registers.len(),
            entry_locals,
            local_metadata,
            fills: vec![],
            reductions: vec![],
        };
        let quiet_edges = native_runtime::quiet_loop_edges(lir);
        let fills = if feedback {
            vec![]
        } else {
            crate::region::fill::find(lir)
        };
        let reductions = if feedback {
            vec![]
        } else {
            crate::region::reduce::find(lir)
        };
        let partial_origins = lir.block_origins.iter().any(Option::is_none);
        for (block, body) in lir.blocks.iter().enumerate() {
            plan.blocks.push(plan.nodes.len());
            // Inlined blocks without origins retain header callbacks, matching the
            // shared quiet-edge policy; those backedges cannot be deferred safely.
            let observing_header =
                partial_origins && lir.osr_entries.iter().any(|e| e.header.0 == block);
            if !feedback
                && !observing_header
                && let Some(origin) = lir.block_origins[block]
            {
                let collecting =
                    std::mem::offset_of!(crate::native_abi::NativeHeader, collecting) as u64;
                let previous =
                    std::mem::offset_of!(crate::native_abi::NativeHeader, previous) as u64;
                let skip = std::mem::offset_of!(crate::native_abi::NativeHeader, skip_probe) as u64;
                for (index, &(from, to)) in quiet_edges.iter().enumerate() {
                    if to != block {
                        continue;
                    }
                    plan.push(
                        "quiet_edge",
                        Patch {
                            a: collecting,
                            b: previous,
                            c: skip,
                            power: std::mem::offset_of!(
                                crate::native_abi::NativeHeader,
                                quiet_counts
                            ),
                            tag: lir.block_origins[from].unwrap().0 as u64,
                            status: index as u64,
                            ..Patch::default()
                        },
                    );
                }
                plan.split(
                    "quiet_enter",
                    Patch {
                        a: collecting,
                        b: previous,
                        c: skip,
                        power: std::mem::offset_of!(
                            crate::native_abi::NativeHeader,
                            inline_block_hits
                        ),
                        tag: origin.0 as u64,
                        callback: plan.gateway,
                        event: ENTER,
                        location: i32::try_from(block).map_err(|_| Error::IntegerOverflow)?,
                        ..Patch::default()
                    },
                    "callback",
                );
            } else {
                plan.callback(ENTER, block)?;
            }
            if let Some(fill) = fills.iter().find(|f| f.header.0 == block)
                && let Some(backedge) = quiet_edges
                    .iter()
                    .position(|&(from, to)| from == fill.body.0 && to == block)
            {
                let info = Box::new(RangeFill {
                    last: fill.last.map(slot).transpose()?.unwrap_or(usize::MAX),
                    constant: slot(fill.constant)?,
                    result: fill.result.map(slot).transpose()?.unwrap_or(usize::MAX),
                    backedge,
                    vm_offset: std::mem::offset_of!(native_runtime::Context<'_>, vm),
                    entries_offset: std::mem::offset_of!(Vm, tier_stats)
                        + std::mem::offset_of!(crate::tier::TierStats, native_region_entries),
                    iterations_offset: std::mem::offset_of!(Vm, tier_stats)
                        + std::mem::offset_of!(crate::tier::TierStats, native_region_iterations),
                    value: u8::from(fill.value),
                });
                let pointer = &*info as *const RangeFill as u64;
                plan.fills.push(info);
                plan.push(
                    "range_fill",
                    Patch {
                        left: slot(fill.iterator)?,
                        right: slot(fill.list)?,
                        a: pointer,
                        ..Patch::default()
                    },
                );
            }
            if let Some(reduction) = reductions.iter().find(|r| r.header.0 == block)
                && let Some(backedge) = quiet_edges
                    .iter()
                    .position(|&(from, to)| from == reduction.backedge.0 && to == block)
            {
                let mut outputs = [0; 4];
                let mut kinds = [0; 4];
                for (i, &(register, kind)) in reduction.outputs.iter().enumerate() {
                    outputs[i] = slot(register)?;
                    kinds[i] = kind;
                }
                let info = Box::new(RangeReduce {
                    outputs,
                    kinds,
                    len: reduction.outputs.len(),
                    backedge,
                    true_blocks: reduction.true_blocks as u64,
                    false_blocks: reduction.false_blocks as u64,
                    vm_offset: std::mem::offset_of!(native_runtime::Context<'_>, vm),
                    entries_offset: std::mem::offset_of!(Vm, tier_stats)
                        + std::mem::offset_of!(crate::tier::TierStats, native_region_entries),
                    iterations_offset: std::mem::offset_of!(Vm, tier_stats)
                        + std::mem::offset_of!(crate::tier::TierStats, native_region_iterations),
                });
                let pointer = &*info as *const RangeReduce as u64;
                plan.reductions.push(info);
                plan.push(
                    "range_reduce",
                    Patch {
                        left: slot(reduction.iterator)?,
                        right: slot(reduction.list)?,
                        a: pointer,
                        ..Patch::default()
                    },
                );
            }
            let bootstrap_reads = if !feedback {
                crate::leaf_inline::bootstrap_reads(&body.instructions)
            } else {
                Default::default()
            };
            let mut skip_instructions = 0;
            for (index, instruction) in body.instructions.iter().enumerate() {
                let location = plan.locations.len();
                plan.locations.push((block, index));
                if skip_instructions > 0 {
                    skip_instructions -= 1;
                    continue;
                }
                if bootstrap_reads.contains(&index) {
                    continue;
                }
                let dst = instruction.result.map(slot).transpose()?.unwrap_or(0);
                let mut p = Patch {
                    dst,
                    ..Patch::default()
                };
                let stencil = match &instruction.operation {
                    Operation::FloatListView { value, length, .. } => {
                        p.callback = native_runtime::float_view_callback as *const u8 as usize;
                        p.event = FLOAT_VIEW;
                        p.location = i32::try_from(location).map_err(|_| Error::IntegerOverflow)?;
                        if !feedback {
                            p.left = slot(*value)?;
                            p.a = length.map_or(0, |n| n as u64 + 1);
                            p.power = std::mem::offset_of!(
                                native_runtime::Context<'_>,
                                inline_float_views
                            );
                            "direct_float_view"
                        } else {
                            "callback"
                        }
                    }
                    Operation::FloatListUnpack { value, view } => {
                        p.left = slot(*value)?;
                        p.right = slot(*view)?;
                        p.callback = plan.gateway;
                        p.event = FINISH;
                        p.location = i32::try_from(location).map_err(|_| Error::IntegerOverflow)?;
                        p.tag = (!feedback) as u64;
                        "float_unpack"
                    }
                    Operation::FloatListLoad {
                        view,
                        index,
                        failure,
                    }
                    | Operation::FloatListStore {
                        view,
                        index,
                        failure,
                        ..
                    } => {
                        p.left = slot(*view)?;
                        p.a = *index as u64;
                        p.status = failure.0 as u64 + 1;
                        p.callback = plan.gateway;
                        p.event = FINISH;
                        p.location = i32::try_from(location).map_err(|_| Error::IntegerOverflow)?;
                        p.tag = (!feedback) as u64;
                        if let Operation::FloatListStore { value, .. } = &instruction.operation {
                            p.right = slot(*value)?;
                            "float_store"
                        } else {
                            "float_load"
                        }
                    }
                    Operation::SequenceUnpack {
                        value, kind, count, ..
                    } if !feedback => {
                        skip_instructions = *count;
                        p.callback = plan.gateway;
                        p.event = UNPACK_BATCH;
                        p.location = i32::try_from(location).map_err(|_| Error::IntegerOverflow)?;
                        if *count <= 3 {
                            p.left = slot(*value)?;
                            p.tag = *count as u64;
                            p.status = match kind {
                                crate::sequence::SequenceKind::List => 2,
                                crate::sequence::SequenceKind::Tuple => 3,
                            };
                            let mut outputs = [0; 3];
                            for (offset, dst) in outputs.iter_mut().take(*count).enumerate() {
                                let read = &body.instructions[index + offset + 1];
                                if !matches!(read.operation, Operation::SequenceGet { value: source, kind: k, index: lir::SequenceIndex::Constant(i), .. } if Some(source) == instruction.result && k == *kind && i == offset as i64)
                                {
                                    return Err(Error::InvalidIr("unpack stencil reads"));
                                }
                                *dst = slot(
                                    read.result
                                        .ok_or(Error::InvalidIr("unpack stencil destination"))?,
                                )?;
                            }
                            [p.a, p.b, p.c] = outputs.map(|n| n as u64);
                            p.power = std::mem::offset_of!(
                                native_runtime::Context<'_>,
                                inline_unpack_batches
                            );
                            "direct_unpack"
                        } else {
                            "callback"
                        }
                    }
                    Operation::SequenceUnpack { .. } | Operation::SequenceGet { .. } => {
                        p.callback = plan.gateway;
                        p.event = SEQUENCE;
                        p.location = i32::try_from(location).map_err(|_| Error::IntegerOverflow)?;
                        "callback"
                    }
                    Operation::Copy(value) => {
                        p.left = slot(*value)?;
                        "copy"
                    }
                    Operation::Constant(value) => {
                        let value = Slot::boxed(
                            value
                                .immediate()
                                .ok_or(Error::InvalidIr("allocating stencil constant"))?,
                        );
                        p.tag = value.tag;
                        p.a = value.a;
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
                    Operation::BoolNot(value) => {
                        p.left = slot(*value)?;
                        "bool_not"
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
                    Operation::Truth { value, .. } => {
                        p.left = slot(*value)?;
                        p.callback = plan.gateway;
                        p.event = HELPER;
                        p.location = i32::try_from(location).map_err(|_| Error::IntegerOverflow)?;
                        "truth"
                    }
                    Operation::Call {
                        helper: lir::Helper::Python(crate::hir::CoreOp::IsDone),
                        arguments,
                        ..
                    } if !feedback && arguments.len() == 1 => {
                        p.left = slot(arguments[0])?;
                        p.a = std::mem::offset_of!(crate::native_abi::NativeHeader, collecting)
                            as u64;
                        p.callback = plan.gateway;
                        p.event = HELPER;
                        p.location = i32::try_from(location).map_err(|_| Error::IntegerOverflow)?;
                        "is_done"
                    }
                    Operation::Call {
                        helper: lir::Helper::LeafGuard(_),
                        ..
                    } if !feedback
                        && crate::leaf_inline::bootstrap_end(&body.instructions, index)
                            .is_some() =>
                    {
                        p.callback = plan.gateway;
                        p.event = LEAF_ENTER;
                        p.location = i32::try_from(location).map_err(|_| Error::IntegerOverflow)?;
                        "callback"
                    }
                    Operation::Call {
                        helper: lir::Helper::GetAttr(attribute),
                        arguments,
                        ..
                    } if !feedback => {
                        p.callback = plan.gateway;
                        p.power = native_runtime::field_callback as *const u8 as usize;
                        p.event = HELPER;
                        p.location = i32::try_from(location).map_err(|_| Error::IntegerOverflow)?;
                        if attribute.inline_instances {
                            p.left = slot(arguments[0])?;
                            p.a = &attribute.cache as *const _ as usize as u64;
                            "direct_field"
                        } else {
                            "field_probe"
                        }
                    }
                    Operation::Call {
                        helper: lir::Helper::Python(crate::hir::CoreOp::Next),
                        arguments,
                        ..
                    } if !feedback && arguments.len() == 1 => {
                        p.left = slot(arguments[0])?;
                        p.callback = plan.gateway;
                        p.power = native_runtime::guarded_helper_callback as *const u8 as usize;
                        p.event = RANGE_NEXT;
                        p.location = i32::try_from(location).map_err(|_| Error::IntegerOverflow)?;
                        p.a = std::mem::offset_of!(native_runtime::Context<'_>, inline_next) as u64;
                        "direct_next"
                    }
                    Operation::Call {
                        helper, arguments, ..
                    } if !feedback
                        && matches!(helper, lir::Helper::GetItem | lir::Helper::SetItem) =>
                    {
                        let store = matches!(helper, lir::Helper::SetItem);
                        p.tag = u64::from(instruction.result.is_some());
                        if arguments.len() != if store { 3 } else { 2 } {
                            return Err(Error::InvalidIr("direct item operands"));
                        }
                        p.left = slot(arguments[0])?;
                        p.right = slot(arguments[1])?;
                        if store {
                            p.a = slot(arguments[2])? as u64;
                        }
                        p.callback = plan.gateway;
                        p.power = native_runtime::guarded_helper_callback as *const u8 as usize;
                        p.event = if store { LIST_SET } else { CLOSED_BUILTIN };
                        p.location = i32::try_from(location).map_err(|_| Error::IntegerOverflow)?;
                        p.b = (std::mem::offset_of!(crate::Vm, tier_stats)
                            + if store {
                                std::mem::offset_of!(crate::tier::TierStats, native_list_stores)
                            } else {
                                std::mem::offset_of!(crate::tier::TierStats, native_sequence_loads)
                            }) as u64;
                        p.c = std::mem::offset_of!(native_runtime::Context<'_>, vm) as u64;
                        if store {
                            "direct_setitem"
                        } else {
                            "direct_getitem"
                        }
                    }
                    Operation::Call {
                        helper: lir::Helper::Python(crate::hir::CoreOp::GetSlice),
                        arguments,
                        ..
                    } if !feedback && arguments.len() == 4 => {
                        p.left = slot(arguments[0])?;
                        p.right = slot(arguments[1])?;
                        p.a = slot(arguments[2])? as u64;
                        p.b = slot(arguments[3])? as u64;
                        p.c = std::mem::offset_of!(native_runtime::Context<'_>, vm) as u64;
                        p.status = (std::mem::offset_of!(crate::Vm, tier_stats)
                            + std::mem::offset_of!(crate::tier::TierStats, native_string_slices))
                            as u64;
                        p.callback = plan.gateway;
                        p.power = native_runtime::guarded_helper_callback as *const u8 as usize;
                        p.event = CLOSED_BUILTIN;
                        p.location = i32::try_from(location).map_err(|_| Error::IntegerOverflow)?;
                        "direct_string_slice"
                    }
                    Operation::Call {
                        helper: lir::Helper::Python(crate::hir::CoreOp::LoadLocal(_)),
                        ..
                    } if instruction.result.is_some_and(|result| {
                        plan.entry_locals.iter().any(|(load, _)| *load == result)
                    }) =>
                    {
                        p.left = slot(Register(plan.local_metadata.unwrap()))?;
                        p.callback = plan.gateway;
                        p.event = HELPER;
                        p.location = i32::try_from(location).map_err(|_| Error::IntegerOverflow)?;
                        "local_load"
                    }
                    Operation::Call {
                        helper: lir::Helper::ScalarCall(builtin),
                        arguments,
                        ..
                    } if !feedback
                        && arguments.len() == 2
                        && matches!(
                            builtin,
                            lir::ScalarBuiltin::Len | lir::ScalarBuiltin::Sqrt
                        ) =>
                    {
                        p.left = slot(arguments[1])?;
                        p.right = slot(arguments[0])?;
                        p.tag = if *builtin == lir::ScalarBuiltin::Len {
                            NATIVE_LEN
                        } else {
                            NATIVE_SQRT
                        };
                        p.a = (std::mem::offset_of!(crate::Vm, tier_stats)
                            + std::mem::offset_of!(
                                crate::tier::TierStats,
                                bytecode_positional_calls
                            )) as u64;
                        p.b = (std::mem::offset_of!(crate::Vm, tier_stats)
                            + std::mem::offset_of!(crate::tier::TierStats, bytecode_leaf_calls))
                            as u64;
                        p.c = std::mem::offset_of!(native_runtime::Context<'_>, vm) as u64;
                        p.callback = plan.gateway;
                        p.power = native_runtime::guarded_helper_callback as *const u8 as usize;
                        p.event = SCALAR_CALL;
                        p.location = i32::try_from(location).map_err(|_| Error::IntegerOverflow)?;
                        "direct_scalar"
                    }
                    Operation::Call { helper, .. }
                        if !feedback && closed_event(helper).is_some() =>
                    {
                        p.callback = plan.gateway;
                        p.power = native_runtime::guarded_helper_callback as *const u8 as usize;
                        p.event = closed_event(helper).unwrap();
                        p.location = i32::try_from(location).map_err(|_| Error::IntegerOverflow)?;
                        "guarded_callback"
                    }
                    Operation::NumberCompare { .. }
                    | Operation::NumericAdd { .. }
                    | Operation::Call { .. } => {
                        p.callback = plan.gateway;
                        p.event = HELPER;
                        p.location = i32::try_from(location).map_err(|_| Error::IntegerOverflow)?;
                        "callback"
                    }
                };
                let mut stencil = stencil;
                if !feedback
                    && stencil == "direct_next"
                    && !bootstrap_reads.contains(&(index + 1))
                    && !bootstrap_reads.contains(&(index + 2))
                    && let Some(done) = body.instructions.get(index + 1)
                    && let Some(truth) = body.instructions.get(index + 2)
                    && let (Some(value), Some(done_result), Some(condition)) =
                        (instruction.result, done.result, truth.result)
                    && let Operation::Call {
                        helper: lir::Helper::Python(crate::hir::CoreOp::IsDone),
                        arguments,
                        ..
                    } = &done.operation
                    && arguments.as_slice() == [value]
                    && let Operation::Truth { value: input, .. } = truth.operation
                    && input == done_result
                {
                    p.b = slot(done_result)? as u64;
                    p.c = slot(condition)? as u64;
                    p.status =
                        i32::try_from(location + 1).map_err(|_| Error::IntegerOverflow)? as u64;
                    skip_instructions = 2;
                    stencil = "next_test";
                }
                if !feedback
                    && !bootstrap_reads.contains(&(index + 1))
                    && let Some(next) = body.instructions.get(index + 1)
                    && let Some(raw) = next.result
                {
                    match (&instruction.operation, &next.operation) {
                        (Operation::GuardFloat { value, .. }, Operation::UnboxFloat(input))
                        | (Operation::GuardInt { value, .. }, Operation::UnboxInt(input))
                            if value == input =>
                        {
                            p.dst = slot(raw)?;
                            p.b = if p.tag == FLOAT { RAW_FLOAT } else { RAW_INT };
                            skip_instructions = 1;
                            stencil = "guard_unbox";
                        }
                        (Operation::FloatListLoad { .. }, Operation::UnboxFloat(input))
                            if instruction.result == Some(*input) =>
                        {
                            p.b = slot(raw)? as u64;
                            skip_instructions = 1;
                            stencil = "float_load_raw";
                        }
                        _ => {}
                    }
                }
                // Preserve both SSA outputs; only the adjacent retag dispatch disappears.
                let boxed_output = if !feedback && !bootstrap_reads.contains(&(index + 1)) {
                    body.instructions.get(index + 1).and_then(|boxed| {
                        let source = match (&boxed.operation, stencil) {
                            (Operation::BoxInt(value), "iadd") => Some(*value),
                            (
                                Operation::BoxFloat(value),
                                "fadd" | "fsub" | "fmul" | "fdiv" | "fpow",
                            ) => Some(*value),
                            _ => None,
                        };
                        (source.is_some() && source == instruction.result)
                            .then_some(boxed.result)
                            .flatten()
                    })
                } else {
                    None
                };
                let mut stencil = if let Some(boxed) = boxed_output {
                    p.a = slot(boxed)? as u64;
                    skip_instructions = 1;
                    match stencil {
                        "iadd" => "iadd_box",
                        "fadd" => "fadd_box",
                        "fsub" => "fsub_box",
                        "fmul" => "fmul_box",
                        "fdiv" => "fdiv_box",
                        "fpow" => "fpow_box",
                        _ => unreachable!(),
                    }
                } else {
                    stencil
                };
                if !feedback
                    && matches!(stencil, "fadd_box" | "fsub_box" | "fmul_box")
                    && !bootstrap_reads.contains(&(index + 2))
                    && let Some(second) = body.instructions.get(index + 2)
                    && let Operation::FloatBinary {
                        operator,
                        left,
                        right,
                        ..
                    } = &second.operation
                    && matches!(
                        operator,
                        FloatOperator::Add | FloatOperator::Subtract | FloatOperator::Multiply
                    )
                    && let Some(raw) = second.result
                {
                    // No guard, callback or failing arithmetic is crossed by this pair.
                    // Keep both raw/boxed images, and reuse the first value for dependent operands.
                    p.b = slot(raw)? as u64;
                    p.c = u64::MAX;
                    p.tag = slot(*left)? as u64;
                    p.status = slot(*right)? as u64;
                    p.event = i32::from(instruction.result == Some(*left))
                        | (i32::from(instruction.result == Some(*right)) << 1);
                    skip_instructions = 2;
                    if !bootstrap_reads.contains(&(index + 3))
                        && let Some(boxed) = body.instructions.get(index + 3)
                        && matches!(boxed.operation, Operation::BoxFloat(value) if value == raw)
                        && let Some(result) = boxed.result
                    {
                        p.c = slot(result)? as u64;
                        skip_instructions = 3;
                    }
                    stencil = match (stencil, operator) {
                        ("fadd_box", FloatOperator::Add) => "fpair_aa",
                        ("fadd_box", FloatOperator::Subtract) => "fpair_as",
                        ("fadd_box", FloatOperator::Multiply) => "fpair_am",
                        ("fsub_box", FloatOperator::Add) => "fpair_sa",
                        ("fsub_box", FloatOperator::Subtract) => "fpair_ss",
                        ("fsub_box", FloatOperator::Multiply) => "fpair_sm",
                        ("fmul_box", FloatOperator::Add) => "fpair_ma",
                        ("fmul_box", FloatOperator::Subtract) => "fpair_ms",
                        ("fmul_box", FloatOperator::Multiply) => "fpair_mm",
                        _ => unreachable!(),
                    };
                }
                if stencil == "next_test" {
                    let fast = plan.push(stencil, p);
                    let cold_next = plan.push("guarded_callback", p);
                    let cold_done = plan.split(
                        "is_done",
                        Patch {
                            dst: p.b as usize,
                            left: p.dst,
                            a: std::mem::offset_of!(NativeHeader, collecting) as u64,
                            callback: plan.gateway,
                            event: HELPER,
                            location: p.status as i32,
                            ..Patch::default()
                        },
                        "callback",
                    );
                    // IsDone returns an exact Bool, so this cold Truth never needs its own fallback.
                    plan.push(
                        "truth",
                        Patch {
                            left: p.b as usize,
                            dst: p.c as usize,
                            callback: plan.gateway,
                            event: HELPER,
                            location: i32::try_from(location + 2)
                                .map_err(|_| Error::IntegerOverflow)?,
                            ..Patch::default()
                        },
                    );
                    let join = plan.nodes.len();
                    for node in &mut plan.nodes[cold_next..join] {
                        node.cold = true;
                    }
                    plan.nodes[fast].next = Target::Node(join);
                    plan.nodes[fast].alternate = Target::Node(cold_next);
                    plan.nodes[fast].done_slow = Some(cold_done);
                } else if matches!(
                    stencil,
                    "local_load"
                        | "direct_field"
                        | "direct_float_view"
                        | "direct_unpack"
                        | "direct_getitem"
                        | "direct_setitem"
                        | "direct_next"
                        | "direct_string_slice"
                        | "direct_scalar"
                        | "is_done"
                ) {
                    let slow = match stencil {
                        "direct_field" => "field_probe",
                        "direct_getitem"
                        | "direct_setitem"
                        | "direct_next"
                        | "direct_string_slice"
                        | "direct_scalar" => "guarded_callback",
                        _ => "callback",
                    };
                    plan.split(stencil, p, slow);
                } else {
                    plan.push(stencil, p);
                }
                // Scalar boxed results contain no heap references and remain available in native registers.
                if feedback
                    && instruction
                        .result
                        .is_some_and(|register| lir.registers[register.0] == lir::Type::Boxed)
                    && !matches!(
                        instruction.operation,
                        Operation::NumericAdd { .. }
                            | Operation::Call { .. }
                            | Operation::SequenceUnpack { .. }
                            | Operation::SequenceGet { .. }
                    )
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
    entry_locals: Vec<(Register, usize)>,
    local_metadata: Option<usize>,
    quiet_edges: Vec<(usize, usize)>,
    pub code_bytes: usize,
    pub data_bytes: usize,
    pub stencils: usize,
    pub relocations: usize,
    pub feedback: bool,
    inline_fields: bool,
    #[allow(clippy::vec_box)]
    _fills: Vec<Box<RangeFill>>,
    #[allow(clippy::vec_box)]
    _reductions: Vec<Box<RangeReduce>>,
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
        let mut offsets = vec![0; plan.nodes.len()];
        let mut size = 0;
        let order = plan
            .nodes
            .iter()
            .enumerate()
            .filter(|(_, n)| !n.cold)
            .chain(plan.nodes.iter().enumerate().filter(|(_, n)| n.cold));
        for (index, node) in order {
            size = align(size, if cfg!(target_arch = "aarch64") { 4 } else { 16 })?;
            offsets[index] = size;
            size = size
                .checked_add(template(node.stencil)?.len())
                .ok_or(Error::IntegerOverflow)?;
        }
        let code_bytes = size;
        let patches = align(size, std::mem::align_of::<Patch>())?;
        let mut patch_ids = Vec::with_capacity(plan.nodes.len());
        let mut unique_patches = 0usize;
        for (index, node) in plan.nodes.iter().enumerate() {
            if let Some(parent) = node.patch_owner {
                if parent >= index {
                    return Err(Error::InvalidIr("cold patch owner"));
                }
                patch_ids.push(patch_ids[parent]);
            } else {
                patch_ids.push(unique_patches);
                unique_patches += 1;
            }
        }
        let data_bytes = unique_patches
            .checked_mul(std::mem::size_of::<Patch>())
            .ok_or(Error::IntegerOverflow)?;
        let length = patches
            .checked_add(data_bytes)
            .ok_or(Error::IntegerOverflow)?;
        let mut memory = MmapMut::map_anon(length).map_err(error)?;
        let base = memory.as_ptr() as usize;
        let patch_offset = |index: usize| patches + patch_ids[index] * std::mem::size_of::<Patch>();
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
        for fill in &plan.fills {
            for offset in [fill.last, fill.constant, fill.result]
                .into_iter()
                .filter(|&o| o != usize::MAX)
            {
                if !offset.is_multiple_of(std::mem::size_of::<Slot>())
                    || offset
                        .checked_add(std::mem::size_of::<Slot>())
                        .is_none_or(|end| end > frame_bytes)
                {
                    return Err(Error::InvalidIr("range fill output"));
                }
            }
        }
        for reduction in &plan.reductions {
            for &offset in &reduction.outputs[..reduction.len] {
                if !offset.is_multiple_of(std::mem::size_of::<Slot>())
                    || offset
                        .checked_add(std::mem::size_of::<Slot>())
                        .is_none_or(|end| end > frame_bytes)
                {
                    return Err(Error::InvalidIr("range reduction output"));
                }
            }
        }
        let mut relocations = 0;
        for (index, node) in plan.nodes.iter().enumerate() {
            let bytes = template(node.stencil)?;
            memory[offsets[index]..offsets[index] + bytes.len()].copy_from_slice(bytes);
            #[cfg(all(target_arch = "aarch64", target_endian = "little"))]
            if node.patch_owner.is_none()
                && let Some(next) = target(node.next, index)?
                && tail::link(
                    node.stencil,
                    &mut memory[offsets[index]..offsets[index] + bytes.len()],
                    offsets[index],
                    patch_offset(index),
                    offsets[next],
                    patch_offset(next),
                )
            {
                relocations += 2;
            }
            // An alias has code but no independently written data record.
            if node.patch_owner.is_some() {
                continue;
            }
            let mut patch = node.patch;
            let extra = if node.stencil == "direct_unpack" {
                patch.tag as usize
            } else if matches!(node.stencil, "next_test" | "direct_string_slice") {
                2
            } else if node.stencil == "direct_setitem"
                || node.stencil.ends_with("_box")
                || node.stencil == "float_load_raw"
            {
                1
            } else {
                0
            };
            let checked_offsets = [patch.dst, patch.left, patch.right].into_iter().chain(
                if node.stencil == "next_test" {
                    [patch.b as usize, patch.c as usize, 0]
                } else if node.stencil == "float_load_raw" {
                    [patch.b as usize, 0, 0]
                } else {
                    [patch.a as usize, patch.b as usize, patch.c as usize]
                }
                .into_iter()
                .take(extra),
            );
            let pair_offsets = if node.stencil.starts_with("fpair_") {
                [patch.a, patch.b, patch.c, patch.tag, patch.status]
            } else {
                [u64::MAX; 5]
            };
            for offset in checked_offsets.chain(
                pair_offsets
                    .into_iter()
                    .filter(|&o| o != u64::MAX)
                    .map(|o| o as usize),
            ) {
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
            // next_test repurposes callback/power as an IsDone continuation pair, never a C probe.
            if let Some(done) = node.done_slow {
                patch.callback = base + offsets[done];
                patch.power = base + patch_offset(done);
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
        let quiet_edges = if feedback {
            vec![]
        } else {
            native_runtime::quiet_loop_edges(&lir)
        };
        let inline_fields = !feedback
            && plan.nodes.iter().any(|node| {
                matches!(
                    node.stencil,
                    "direct_field"
                        | "direct_float_view"
                        | "direct_unpack"
                        | "direct_next"
                        | "direct_string_slice"
                        | "direct_scalar"
                        | "next_test"
                        | "range_fill"
                        | "range_reduce"
                        | "is_done"
                        | "direct_getitem"
                        | "direct_setitem"
                )
            });
        Ok(Self {
            memory,
            lir,
            locations: plan.locations,
            blocks,
            scratch: plan.scratch,
            entry_locals: plan.entry_locals,
            local_metadata: plan.local_metadata,
            quiet_edges,
            code_bytes,
            data_bytes,
            stencils: plan.nodes.len(),
            relocations,
            feedback,
            inline_fields,
            _fills: plan.fills,
            _reductions: plan.reductions,
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
        if self.inline_fields {
            vm.heap.enable_native_fields();
        }
        native_runtime::execute(
            &self.lir,
            &self.locations,
            &self.quiet_edges,
            None,
            // Slots remain canonical between stencils. Collecting helpers publish
            // their verified maps; quiet block entries need no stack mirror.
            !self.feedback,
            &self.entry_locals,
            self.local_metadata,
            None,
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

fn closed_event(helper: &lir::Helper) -> Option<i32> {
    use crate::hir::CoreOp;
    Some(match helper {
        lir::Helper::ScalarCall(_) => SCALAR_CALL,
        lir::Helper::Tuple => TUPLE_ALLOC,
        lir::Helper::LoadName(_) => NAME_LOAD,
        lir::Helper::StoreName(_) => NAME_STORE,
        lir::Helper::StringNext => STRING_NEXT,
        lir::Helper::SetItem => LIST_SET,
        lir::Helper::GetItem => CLOSED_BUILTIN,
        lir::Helper::Python(CoreOp::Next) => RANGE_NEXT,
        lir::Helper::Python(CoreOp::GetSlice | CoreOp::InplaceItem(_)) => CLOSED_BUILTIN,
        lir::Helper::Python(CoreOp::Compare(op)) if op == "in" || op == "not in" => CLOSED_BUILTIN,
        lir::Helper::Scoped(inner) if matches!(inner.as_ref(), lir::Helper::ScalarCall(_)) => {
            SCALAR_CALL
        }
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Tiering,
        hir::{Constant, CoreOp, Op},
    };
    #[test]
    fn stencil_library_contains_offline_code() {
        assert_eq!(generated::STENCILS.len(), 60);
        assert!(
            generated::STENCILS
                .iter()
                .all(|(_, bytes)| !bytes.is_empty())
        );
    }
    #[test]
    fn fusion_keeps_raw_and_boxed_results_and_avoids_observation_mode() {
        let mut f = crate::hir::Function::new(2);
        let entry = f.entry();
        let args = f.parameters(entry).to_vec();
        let sum = f.append(entry, crate::hir::Op::Add(args[0], args[1]));
        let twice = f.append(entry, crate::hir::Op::Add(sum, sum));
        f.terminate(entry, crate::hir::Terminator::Return(twice));
        let mut vm = crate::Vm::new();
        vm.profiling = crate::profile::ProfileConfig::eager();
        for _ in 0..40 {
            assert_eq!(
                vm.run(
                    &mut f,
                    &[crate::Value::Float(1.5), crate::Value::Float(2.0)]
                )
                .unwrap(),
                crate::Value::Float(7.0)
            );
        }
        let lir = f.lower_lir(lir::LoweringMode::Profiled).unwrap();
        let quiet = Plan::lower(&lir, false).unwrap();
        assert!(
            quiet.nodes.iter().any(|node| node.stencil == "fpair_aa"),
            "{}",
            lir.dump()
        );
        let pair = quiet
            .nodes
            .iter()
            .find(|node| node.stencil == "fpair_aa")
            .unwrap();
        assert_eq!(pair.patch.event, 3);
        assert_eq!(pair.patch.a as usize, slot(Register(sum.0)).unwrap());
        assert_eq!(pair.patch.c as usize, slot(Register(twice.0)).unwrap());
        assert!(quiet.nodes.iter().any(|node| node.stencil == "guard_unbox"));
        let observing = Plan::lower(&lir, true).unwrap();
        assert!(
            !observing
                .nodes
                .iter()
                .any(|node| node.stencil.ends_with("_box") || node.stencil.starts_with("fpair_"))
        );
        vm.profiling = crate::profile::ProfileConfig::default();
        vm.tiering = crate::Tiering::CopyPatch;
        vm.compile_lir(&f).unwrap();
        assert_eq!(
            vm.run(
                &mut f,
                &[crate::Value::Float(1.5), crate::Value::Float(2.0)]
            )
            .unwrap(),
            crate::Value::Float(7.0)
        );
    }
    #[test]
    fn float_pair_family_covers_dependencies_and_excludes_failing_operators() {
        let mut source = String::new();
        let mut names = vec![];
        for (a, first) in [('a', '+'), ('s', '-'), ('m', '*')] {
            for (b, second) in [('a', '+'), ('s', '-'), ('m', '*')] {
                let name = format!("pair_{a}{b}");
                source.push_str(&format!(
                    "def {name}(x):\n    a=x{first}x\n    return a{second}x\n"
                ));
                names.push((name, format!("fpair_{a}{b}")));
            }
        }
        source.push_str("def update(a,x):\n    a[0]+=x*x\n    return a[0]\ndef divide(x):\n    a=x/x\n    return a+x\ndef power(x):\n    a=x*x\n    return a**x\nfor repeat in range(40):\n    update([1.0],2.0)\n    divide(2.0)\n    power(2.0)\n");
        for (name, _) in &names {
            source.push_str(&format!("    {name}(1.25)\n"));
        }
        let mut vm = crate::Vm::new();
        vm.profiling = crate::profile::ProfileConfig::eager();
        vm.run_program(&mut crate::compile(&source).unwrap())
            .unwrap();
        let lower = |name: &str| {
            vm.function_hir(vm.get_global(name).unwrap())
                .unwrap()
                .lower_lir(lir::LoweringMode::Profiled)
                .unwrap()
        };
        for (name, expected) in names {
            let lir = lower(&name);
            let quiet = Plan::lower(&lir, false).unwrap();
            assert!(
                quiet.nodes.iter().any(|n| n.stencil == expected),
                "{name}: {}",
                lir.dump()
            );
            assert!(
                !Plan::lower(&lir, true)
                    .unwrap()
                    .nodes
                    .iter()
                    .any(|n| n.stencil.starts_with("fpair_"))
            );
        }
        assert!(
            Plan::lower(&lower("update"), false)
                .unwrap()
                .nodes
                .iter()
                .any(|n| n.stencil == "fpair_ma" && n.patch.c == u64::MAX)
        );
        for name in ["divide", "power"] {
            assert!(
                !Plan::lower(&lower(name), false)
                    .unwrap()
                    .nodes
                    .iter()
                    .any(|n| n.stencil.starts_with("fpair_"))
            );
        }
    }

    #[test]
    fn load_unbox_fusion_preserves_observation_and_storage_guard_exits() {
        let mut f = crate::hir::Function::new(1);
        let entry = f.entry();
        let input = f.parameters(entry)[0];
        let zero = f.append(
            entry,
            crate::hir::Op::Constant(crate::hir::Constant::Int(0)),
        );
        let value = f.append(entry, crate::hir::Op::GetItem(input, zero));
        let sum = f.append(entry, crate::hir::Op::Add(value, value));
        f.terminate(entry, crate::hir::Terminator::Return(sum));
        let mut vm = crate::Vm::new();
        vm.profiling = crate::profile::ProfileConfig::eager();
        let values = vm
            .heap
            .allocate(crate::Object::List(vec![crate::Value::Float(3.0)].into()));
        for _ in 0..40 {
            assert_eq!(vm.run(&mut f, &[values]).unwrap(), crate::Value::Float(6.0));
        }
        let lir = f.lower_lir(lir::LoweringMode::Profiled).unwrap();
        let quiet = Plan::lower(&lir, false).unwrap();
        assert!(
            quiet
                .nodes
                .iter()
                .any(|node| node.stencil == "float_load_raw")
        );
        assert!(
            !Plan::lower(&lir, true)
                .unwrap()
                .nodes
                .iter()
                .any(|node| node.stencil == "float_load_raw")
        );
        vm.profiling = crate::profile::ProfileConfig::default();
        vm.tiering = crate::Tiering::CopyPatch;
        vm.compile_lir(&f).unwrap();

        assert_eq!(vm.run(&mut f, &[values]).unwrap(), crate::Value::Float(6.0));
        assert!(
            vm.tier_stats.copy_patch_data_bytes
                < vm.tier_stats.copy_patch_stencils * std::mem::size_of::<Patch>() as u64
        );
        let ints = vm
            .heap
            .allocate(crate::Object::List(vec![crate::Value::Int(4)].into()));
        assert_eq!(vm.run(&mut f, &[ints]).unwrap(), crate::Value::Int(8));
        assert!(vm.tier_stats.deoptimizations > 0);
    }
    #[test]
    fn iterator_test_fusion_keeps_exhaustion_callbacks_and_error_effects() {
        let mut program = crate::compile(
            r#"
def f(xs):
    out=[]
    for x in xs:
        out.append(x)
    return out
xs=[1,2,3]
for repeat in range(40):
    assert f(xs)==[1,2,3]
class Source:
    def __init__(self):
        self.i=0
    def __iter__(self):
        return self
    def __next__(self):
        self.i+=1
        if self.i>2:
            raise StopIteration
        return self.i
s=Source()
calls=[0]
class Broken:
    def __iter__(self):
        return self
    def __next__(self):
        calls[0]+=1
        raise ValueError('bad')
b=Broken()
"#,
        )
        .unwrap();
        let mut vm = crate::Vm::new();
        vm.profiling = crate::profile::ProfileConfig::eager();
        vm.run_program(&mut program).unwrap();
        let value = vm.get_global("f").unwrap();
        let function = match vm.heap.get(value).unwrap() {
            crate::Object::Function(c) => c.function.clone(),
            _ => panic!(),
        };
        let lir = function
            .borrow()
            .lower_lir(lir::LoweringMode::Profiled)
            .unwrap();
        assert!(
            Plan::lower(&lir, false)
                .unwrap()
                .nodes
                .iter()
                .any(|node| node.stencil == "next_test")
        );
        assert!(
            !Plan::lower(&lir, true)
                .unwrap()
                .nodes
                .iter()
                .any(|node| node.stencil == "next_test")
        );
        vm.profiling = crate::profile::ProfileConfig::default();
        vm.tiering = crate::Tiering::CopyPatch;
        vm.compile_lir(&function.borrow()).unwrap();
        let result = vm.call(value, &[vm.get_global("s").unwrap()]).unwrap();
        assert_eq!(
            vm.heap.get_item(result, crate::Value::Int(1)).unwrap(),
            crate::Value::Int(2)
        );
        assert!(vm.call(value, &[vm.get_global("b").unwrap()]).is_err());
        let mut check = crate::compile("assert s.i==3\nassert calls[0]==1\n").unwrap();
        vm.run_program(&mut check).unwrap();
        assert_eq!(vm.stack_len(), 0);
    }
    #[test]
    fn ascii_slice_avoids_callbacks_and_refreshes_mutated_string_rows() {
        let mut f = crate::hir::Function::new(4);
        let entry = f.entry();
        let args = f.parameters(entry).to_vec();
        let result = f.append(
            entry,
            crate::hir::Op::Runtime(crate::hir::CoreOp::GetSlice, args),
        );
        f.terminate(entry, crate::hir::Terminator::Return(result));
        let mut vm = crate::Vm::new();
        vm.profiling = crate::profile::ProfileConfig::eager();
        let source = vm
            .heap
            .allocate(crate::Object::String("abcdefghijklmnopqrstuvwxyz".into()));
        let args = [
            source,
            crate::Value::Int(1),
            crate::Value::Int(4),
            crate::Value::None,
        ];
        for _ in 0..40 {
            vm.run(&mut f, &args).unwrap();
        }
        let lir = f.lower_lir(lir::LoweringMode::Profiled).unwrap();
        assert!(
            Plan::lower(&lir, false)
                .unwrap()
                .nodes
                .iter()
                .any(|n| n.stencil == "direct_string_slice")
        );
        assert!(
            !Plan::lower(&lir, true)
                .unwrap()
                .nodes
                .iter()
                .any(|n| n.stencil == "direct_string_slice")
        );
        vm.profiling = crate::profile::ProfileConfig::default();
        vm.tiering = crate::Tiering::CopyPatch;
        vm.compile_lir(&f).unwrap();
        for _ in 0..8 {
            vm.run(&mut f, &args).unwrap();
        }
        let calls = vm.tier_stats.native_runtime_calls;
        let slices = vm.tier_stats.native_string_slices;
        for _ in 0..100 {
            let result = vm.run(&mut f, &args).unwrap();
            assert_eq!(vm.heap.as_str(&result), Some("bcd"));
        }
        assert_eq!(vm.tier_stats.native_runtime_calls, calls);
        assert_eq!(vm.tier_stats.native_string_slices - slices, 100);
        vm.collect_garbage(&[source]).unwrap();
        *vm.heap.get_mut(source).unwrap() = crate::Object::String("éabcdefghijklmnop".into());
        let result = vm.run(&mut f, &args).unwrap();
        assert_eq!(vm.heap.as_str(&result), Some("abc"));
        *vm.heap.get_mut(source).unwrap() = crate::Object::String("0123456789abcdef".into());
        let calls = vm.tier_stats.native_runtime_calls;
        let result = vm.run(&mut f, &args).unwrap();
        assert_eq!(vm.heap.as_str(&result), Some("123"));
        assert_eq!(vm.tier_stats.native_runtime_calls, calls);
        vm.profiling = crate::profile::ProfileConfig::eager();
        let probes = vm.tier_stats.native_feedback_probes;
        vm.run(&mut f, &args).unwrap();
        assert!(vm.tier_stats.native_feedback_probes > probes);
        assert_eq!(vm.stack_len(), 0);
    }

    #[test]
    fn cached_unbound_local_raises_at_its_original_location() {
        let mut f = Function::new(0);
        f.local_names = vec!["missing".into(), "later".into()];
        f.local_slots = f.local_names.clone();
        let block = f.entry();
        let print = f.append(block, Op::Runtime(CoreOp::LoadName("print".into()), vec![]));
        let message = f.append(block, Op::Constant(Constant::String("before".into())));
        f.append(
            block,
            Op::Runtime(CoreOp::Call(vec![None]), vec![print, message]),
        );
        let value = f.append(block, Op::Runtime(CoreOp::LoadLocal(0), vec![]));
        f.append(block, Op::Runtime(CoreOp::LoadLocal(1), vec![]));
        f.terminate(block, crate::hir::Terminator::Return(value));
        let mut vm = Vm::new();
        vm.tiering = Tiering::CopyPatch;
        vm.thresholds.function_entries = 1;
        vm.profiling.window_ticks = 2;
        for _ in 0..20 {
            let error = vm.run(&mut f, &[]).unwrap_err();
            assert!(
                error.to_string().contains("UnboundLocalError: missing"),
                "{error}"
            );
        }
        assert!(vm.tier_stats.copy_patch_entries > 0);
        assert!(
            vm.copy_artifacts
                .values()
                .any(|code| code.entry_locals.len() >= 2)
        );
        assert_eq!(vm.output, "before\n".repeat(20));
        assert_eq!(vm.stack_len(), 0);
    }

    #[test]
    fn cached_parameter_loads_refresh_between_calls_without_callbacks() {
        let mut program = crate::compile(
            "def choose(a,b):\n    if b:\n        return a\n    return 0\nfor i in range(40):\n    assert choose(i,True)==i\n    assert choose(i,False)==0\n",
        )
        .unwrap();
        let mut vm = Vm::new();
        vm.tiering = Tiering::CopyPatch;
        vm.thresholds.function_entries = 2;
        vm.profiling.window_ticks = 4;
        vm.run_program(&mut program).unwrap();
        let choose = vm.get_global("choose").unwrap();
        for _ in 0..8 {
            vm.call(choose, &[crate::Value::Int(1), crate::Value::Bool(true)])
                .unwrap();
        }
        let before = vm.tier_stats.native_runtime_calls;
        for value in 0..100 {
            assert_eq!(
                vm.call(
                    choose,
                    &[crate::Value::Int(value), crate::Value::Bool(true)]
                )
                .unwrap(),
                crate::Value::Int(value)
            );
        }
        assert_eq!(vm.tier_stats.native_runtime_calls, before);
        assert!(
            vm.copy_artifacts
                .values()
                .any(|code| !code.entry_locals.is_empty())
        );
        assert_eq!(vm.stack_len(), 0);
    }

    #[test]
    fn cached_locals_remain_rooted_across_collecting_helpers() {
        let mut program = crate::compile(
            "def keep(a,n):\n    junk=[[i] for i in range(n)]\n    return a[0]\nfor i in range(40):\n    assert keep([42],2)==42\n",
        ).unwrap();
        let mut vm = Vm::new();
        vm.tiering = Tiering::CopyPatch;
        vm.thresholds.function_entries = 2;
        vm.profiling.window_ticks = 4;
        vm.run_program(&mut program).unwrap();
        let keep = vm.get_global("keep").unwrap();
        let hir = vm.function_hir(keep).unwrap();
        let lir = hir.lower_lir(lir::LoweringMode::Profiled).unwrap();
        let plan = Plan::lower(&lir, false).unwrap();
        assert!(plan.entry_locals.len() >= 2);
        assert!(plan.local_metadata.unwrap() > plan.output);
        assert!(plan.nodes.iter().any(|node| node.stencil == "local_load"));
        assert!(Plan::lower(&lir, true).unwrap().entry_locals.is_empty());
        vm.compile_lir(&hir).unwrap();
        let value = vm
            .heap
            .allocate(crate::Object::List(vec![crate::Value::Int(42)].into()));
        let epoch = vm.heap.collection_epoch();
        assert_eq!(
            vm.call(keep, &[value, crate::Value::Int(10000)]).unwrap(),
            crate::Value::Int(42)
        );
        assert!(vm.heap.collection_epoch() > epoch);
        assert!(vm.heap.get(value).is_ok());
        assert!(
            vm.copy_artifacts
                .values()
                .any(|code| !code.entry_locals.is_empty())
        );
        vm.profiling = crate::profile::ProfileConfig::eager();
        let probes = vm.tier_stats.native_feedback_probes;
        assert_eq!(
            vm.call(keep, &[value, crate::Value::Int(10)]).unwrap(),
            crate::Value::Int(42)
        );
        assert!(vm.tier_stats.native_feedback_probes > probes);
        assert_eq!(vm.stack_len(), 0);
    }

    #[test]
    fn cached_parent_locals_feed_inlined_parameter_aliases() {
        let mut program = crate::compile(
            "class Box:\n    def __init__(self):\n        self.x=0\n    def add(self,x):\n        self.x=self.x+x\n        return self.x\ndef caller(b,x):\n    return b.add(x)\nb=Box()\nfor i in range(500):\n    caller(b,1)\n",
        ).unwrap();
        let mut vm = Vm::new();
        vm.tiering = Tiering::CopyPatch;
        vm.thresholds.function_entries = 16;
        vm.thresholds.loop_backedges = 32;
        vm.run_program(&mut program).unwrap();
        let caller = vm.get_global("caller").unwrap();
        let b = vm.get_global("b").unwrap();
        let source = vm.function_hir(caller).unwrap();
        let lir = source.lower_lir(lir::LoweringMode::Profiled).unwrap();
        assert!(!lir.inline_scopes.is_empty());
        let plan = Plan::lower(&lir, false).unwrap();
        assert!(plan.entry_locals.len() >= 2);
        assert!(plan.nodes.iter().any(|node| node.stencil == "local_load"));
        vm.compile_lir(&source).unwrap();
        let inlines = vm.tier_stats.leaf_body_inlines;
        assert_eq!(
            vm.call(caller, &[b, crate::Value::Int(2)]).unwrap(),
            crate::Value::Int(502)
        );
        assert!(vm.tier_stats.leaf_body_inlines > inlines);
        vm.collect_garbage(&[caller, b]).unwrap();
        assert_eq!(
            vm.call(caller, &[b, crate::Value::Int(3)]).unwrap(),
            crate::Value::Int(505)
        );
        assert_eq!(vm.stack_len(), 0);
    }

    #[test]
    fn direct_scalar_calls_guard_identity_and_keep_call_accounting() {
        use crate::{Object, Value};
        for sqrt in [false, true] {
            let mut vm = Vm::new();
            vm.run_program(&mut crate::compile("from math import sqrt\nlength=len\n").unwrap())
                .unwrap();
            let callable = vm.get_global(if sqrt { "sqrt" } else { "length" }).unwrap();
            let list = vm.heap.allocate(Object::List(
                vec![Value::Float(1.0), Value::Float(2.0), Value::Float(3.0)].into(),
            ));
            let input = if sqrt { Value::Float(4.0) } else { list };
            let mut f = Function::new(2);
            let entry = f.entry();
            let args = f.parameters(entry).to_vec();
            let result = f.append(entry, Op::Runtime(CoreOp::Call(vec![None]), args));
            f.terminate(entry, crate::hir::Terminator::Return(result));
            vm.profiling = crate::profile::ProfileConfig::eager();
            for _ in 0..40 {
                vm.run(&mut f, &[callable, input]).unwrap();
            }
            let lir = f.lower_lir(lir::LoweringMode::Profiled).unwrap();
            assert!(
                Plan::lower(&lir, false)
                    .unwrap()
                    .nodes
                    .iter()
                    .any(|n| n.stencil == "direct_scalar")
            );
            assert!(
                !Plan::lower(&lir, true)
                    .unwrap()
                    .nodes
                    .iter()
                    .any(|n| n.stencil == "direct_scalar")
            );
            vm.profiling = crate::profile::ProfileConfig::default();
            vm.tiering = Tiering::CopyPatch;
            vm.compile_lir(&f).unwrap();
            for _ in 0..8 {
                vm.run(&mut f, &[callable, input]).unwrap();
            }
            let calls = vm.tier_stats.native_runtime_calls;
            let positional = vm.tier_stats.bytecode_positional_calls;
            let leaves = vm.tier_stats.bytecode_leaf_calls;
            let cases = if sqrt {
                vec![
                    (Value::Int(9), Value::Float(3.0)),
                    (Value::Bool(true), Value::Float(1.0)),
                    (Value::Float(0.25), Value::Float(0.5)),
                ]
            } else {
                vec![
                    (list, Value::Int(3)),
                    (vm.heap.string("é🙂"), Value::Int(2)),
                    (vm.heap.string(""), Value::Int(0)),
                    (
                        vm.heap
                            .allocate(Object::String("abcdefghijklmnopqrstuvwxyz".into())),
                        Value::Int(26),
                    ),
                ]
            };
            for index in 0..100 {
                let (arg, expected) = cases[index % cases.len()];
                assert_eq!(vm.run(&mut f, &[callable, arg]).unwrap(), expected);
            }
            assert_eq!(vm.tier_stats.native_runtime_calls, calls);
            assert_eq!(vm.tier_stats.bytecode_positional_calls - positional, 100);
            assert_eq!(vm.tier_stats.bytecode_leaf_calls - leaves, 100);
            vm.collect_garbage(&[callable, list]).unwrap();
            if sqrt {
                let zero = vm.run(&mut f, &[callable, Value::Float(-0.0)]).unwrap();
                assert!(matches!(zero,Value::Float(v) if v.to_bits()==(-0.0f64).to_bits()));
                assert!(
                    matches!(vm.run(&mut f,&[callable,Value::Float(f64::NAN)]).unwrap(),Value::Float(v) if v.is_nan())
                );
                assert_eq!(
                    vm.run(&mut f, &[callable, Value::Float(f64::INFINITY)])
                        .unwrap(),
                    Value::Float(f64::INFINITY)
                );
                assert!(
                    vm.run(&mut f, &[callable, Value::Float(-1.0)])
                        .unwrap_err()
                        .to_string()
                        .contains("math domain")
                );
                *vm.heap.get_mut(callable).unwrap() = Object::Native("len".into());
                assert_eq!(vm.run(&mut f, &[callable, list]).unwrap(), Value::Int(3));
            } else {
                let text = vm
                    .heap
                    .allocate(Object::String("é🙂abcdefghijklmnop".into()));
                let before = vm.tier_stats.native_runtime_calls;
                assert_eq!(vm.run(&mut f, &[callable, text]).unwrap(), Value::Int(18));
                assert!(vm.tier_stats.native_runtime_calls > before);
            }
            assert_eq!(vm.stack_len(), 0);
        }
    }

    #[test]
    fn fresh_native_parameters_survive_collection_before_return() {
        use crate::{Object, Value};
        for tier in [Tiering::Native, Tiering::CopyPatch] {
            let mut f = Function::new(2);
            let entry = f.entry();
            let args = f.parameters(entry).to_vec();
            let zero = f.append(entry, Op::Constant(Constant::Int(0)));
            let one = f.append(entry, Op::Constant(Constant::Int(1)));
            let header = f.block(1);
            let body = f.block(0);
            let exit = f.block(0);
            let counter = f.parameters(header)[0];
            f.terminate(
                entry,
                crate::hir::Terminator::Jump(crate::hir::Edge {
                    block: header,
                    arguments: vec![zero],
                }),
            );
            let more = f.append(header, Op::Less(counter, args[1]));
            f.terminate(
                header,
                crate::hir::Terminator::Branch {
                    condition: more,
                    yes: crate::hir::Edge {
                        block: body,
                        arguments: vec![],
                    },
                    no: crate::hir::Edge {
                        block: exit,
                        arguments: vec![],
                    },
                },
            );
            f.append(body, Op::List(vec![counter]));
            let next = f.append(body, Op::Add(counter, one));
            f.terminate(
                body,
                crate::hir::Terminator::Jump(crate::hir::Edge {
                    block: header,
                    arguments: vec![next],
                }),
            );
            f.terminate(exit, crate::hir::Terminator::Return(args[0]));
            let mut vm = Vm::new();
            vm.profiling = crate::profile::ProfileConfig::eager();
            let kept = vm.heap.allocate(Object::List(vec![Value::Int(42)].into()));
            for _ in 0..40 {
                assert_eq!(vm.run(&mut f, &[kept, Value::Int(2)]).unwrap(), kept);
            }
            vm.profiling = crate::profile::ProfileConfig::default();
            vm.tiering = tier;
            vm.compile_lir(&f).unwrap();
            let epoch = vm.heap.collection_epoch();
            assert_eq!(vm.run(&mut f, &[kept, Value::Int(10000)]).unwrap(), kept);
            assert!(vm.heap.collection_epoch() > epoch);
            assert_eq!(
                vm.heap.get_item(kept, Value::Int(0)).unwrap(),
                Value::Int(42)
            );
            assert_eq!(vm.stack_len(), 0);
        }
    }

    #[test]
    fn templates_cannot_resolve_unknown_names() {
        assert!(template("invalid").is_err());
    }
}
