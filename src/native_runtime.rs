//! Shared native entry, helper, GC and deoptimization protocol.
use crate::{
    Error, ObjectId, Result, Value, Vm,
    bytecode::Code,
    hir::Function,
    lir::{self, Operation},
    native_abi::*,
    tier::Outcome,
};
use std::{cell::RefCell, ffi::c_void, rc::Rc};
fn error(message: impl std::fmt::Display) -> Error {
    Error::Runtime(format!("native execution: {message}"))
}
impl Slot {
    pub(crate) fn boxed(value: Value) -> Self {
        match value {
            Value::Unbound => Self { tag: UNBOUND, a: 0 },
            Value::None => Self { tag: NONE, a: 0 },
            Value::Bool(v) => Self {
                tag: BOOL,
                a: v as u64,
            },
            Value::Int(v) => Self {
                tag: INT,
                a: v as u64,
            },
            Value::Float(v) => Self {
                tag: FLOAT,
                a: v.to_bits(),
            },
            Value::InlineString(text) => {
                let (high, a) = text.words();
                Self {
                    tag: INLINE_STRING | ((high as u64) << 32),
                    a,
                }
            }
            Value::Ref(id) => Self {
                tag: REF | ((id.heap as u64) << 32),
                a: id.index as u64 | ((id.generation as u64) << 32),
            },
        }
    }
    pub(crate) fn value(self) -> Result<Value> {
        Ok(match self.tag & KIND_MASK {
            UNBOUND => Value::Unbound,
            NONE => Value::None,
            BOOL => Value::Bool(self.a != 0),
            INT => Value::Int(self.a as i64),
            FLOAT => Value::Float(f64::from_bits(self.a)),
            INLINE_STRING => Value::InlineString(
                crate::InlineString::from_words((self.tag >> 32) as u32, self.a)
                    .ok_or(Error::InvalidIr("invalid inline UTF-8"))?,
            ),
            REF => Value::Ref(ObjectId {
                heap: (self.tag >> 32) as u32,
                index: self.a as u32,
                generation: (self.a >> 32) as u32,
            }),
            _ => return Err(Error::InvalidIr("native register is not boxed")),
        })
    }
}
pub(crate) type Entry = unsafe extern "C" fn(*mut c_void, *mut Slot, u32) -> u64;

/// Each active native invocation owns its buffers. Only empty buffers enter the VM pool,
/// so inactive capacity never retains Python handles or participates in GC tracing.
#[derive(Debug, Default)]
pub(crate) struct Scratch {
    registers: Vec<Slot>,
    rooted: Vec<usize>,
    helper_arguments: Vec<Value>,
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn execute(
    lir: &lir::Function,
    locations: &[(usize, usize)],
    root_states: Option<&[lir::FrameState]>,
    lazy_slots: bool,
    entry_locals: &[(lir::Register, usize)],
    local_metadata: Option<usize>,
    nest: Option<&crate::region::nest::LoopNest>,
    extra_slots: usize,
    invoke: impl FnOnce(*mut c_void, *mut Slot, u32) -> u64,
    vm: &mut Vm,
    function: &Rc<RefCell<Function>>,
    code: &Code,
    base: usize,
    frame_end: usize,
    start: usize,
    skip_probe: bool,
) -> Result<Outcome> {
    // Call setup can allocate environments; the initial VM window is already fully rooted.
    vm.maybe_collect()?;
    let size = lir
        .registers
        .len()
        .checked_add(1 + extra_slots)
        .ok_or(Error::IntegerOverflow)?;
    let mut scratch = vm.native_register_pool.pop().unwrap_or_default();
    let mut registers = std::mem::take(&mut scratch.registers);
    registers.resize(size, Slot::default());
    let result = (|| {
        let mut rooted = std::mem::take(&mut scratch.rooted);
        // Copy the initial image and record its existing shadow roots in the same frame scan.
        for (index, value) in vm.stack[base..frame_end].iter().enumerate() {
            if let Some(value) = value {
                registers[index] = Slot::boxed(*value);
                rooted.push(index);
            }
        }
        let locals_ready = start == 0 && !skip_probe && !entry_locals.is_empty();
        if let Some(index) = local_metadata {
            // This native-only flag lies after all HIR/region slots and contains no references.
            // Keeping it in the image preserves the common Context and VM statistics layouts.
            registers
                .get_mut(index)
                .ok_or(Error::InvalidIr("native local metadata"))?
                .tag = u64::from(locals_ready);
        }
        if locals_ready {
            // These slots have no writes in this LIR and are inaccessible to reentrant callees.
            // Copy Values, never pointers into the movable local buffer. The original frame range
            // remains a GC root. Missing locals stay guarded and raise only at the original load.
            let locals = vm.local_values()?;
            for &(result, slot) in entry_locals {
                registers[result.0] = Slot::boxed(
                    locals
                        .get(slot)
                        .copied()
                        .flatten()
                        .unwrap_or(Value::Unbound),
                );
            }
        }
        let mut entry_failure = None;
        if let Some(exit) = lir.scalar_entries.get(&crate::hir::BlockId(start)) {
            let state = &lir.states[lir.exits[exit.0].state.0];
            for (raw, source) in lir.scalar_imports() {
                if !state.values.iter().any(|(_, r)| r == source) {
                    continue;
                }
                if let Value::Float(value) = registers[source.0].value()? {
                    registers[raw.0] = Slot {
                        tag: RAW_FLOAT,
                        a: value.to_bits(),
                    };
                } else {
                    entry_failure = Some(*exit);
                    break;
                }
            }
        }
        let mut context = Context {
            vm: vm as *mut Vm,
            inline_field_hits: 0,
            inline_block_hits: 0,
            function,
            code,
            lir,
            locations,
            root_states,
            lazy_slots,
            nest,
            base,
            rooted,
            previous: usize::MAX,
            skip_probe,
            collecting: vm.collecting(function),
            error: None,
            registers: registers.as_mut_ptr(),
            helper_arguments: std::mem::take(&mut scratch.helper_arguments),
            side_exit: entry_failure,
            views: crate::region::HeapViews::default(),
            active_region: None,
            region_done: None,
            schema_key: vec![],
        };
        let outcome = (|| {
            // SAFETY: Context and the fixed-size slot allocation remain alive and unmoved for this
            // synchronous invocation. Generated memory offsets are bounds checked during emission.
            let status = if context.side_exit.is_some() {
                u64::MAX
            } else {
                invoke(
                    (&mut context as *mut Context<'_>).cast(),
                    registers.as_mut_ptr(),
                    u32::try_from(start).map_err(error)?,
                )
            };
            vm.tier_stats.native_inline_field_hits += context.inline_field_hits;
            vm.tier_stats.native_inline_blocks += context.inline_block_hits;
            vm.tier_stats.native_field_hits += context.inline_field_hits;
            vm.tier_stats.shape_read_hits += context.inline_field_hits;
            context.flush_region()?;
            if let Some(error) = context.error.take() {
                return Err(error);
            }
            if status == 0 && context.side_exit.is_none() {
                return Ok(Outcome::Return(registers[lir.registers.len()].value()?));
            }
            let exit = lir
                .exits
                .get(match context.side_exit {
                    Some(exit) => exit.0,
                    None => usize::try_from(status.wrapping_sub(1)).map_err(error)?,
                })
                .ok_or(Error::InvalidIr("native exit status"))?;
            if let Some(snapshot) = &exit.inline {
                let stack = snapshot.reconstruct(&lir.registers, |r| registers[r.0].value())?;
                vm.mark_inline_deoptimization(function, &stack, exit.reason);
                return Ok(Outcome::InlineDeopt(stack));
            }
            let environment = *vm
                .environments
                .last()
                .ok_or(Error::InvalidIr("native environment"))?;
            let frame = lir.reconstruct(exit.state, environment, vm.local_values()?, |r| {
                registers[r.0].value()
            })?;
            vm.mark_deoptimization(function, &frame, exit.reason);
            Ok(Outcome::Deopt(frame))
        })();
        scratch.rooted = context.rooted;
        scratch.helper_arguments = context.helper_arguments;
        outcome
    })();
    // Reuse only after reconstruction and every callback have finished. Clearing also prevents
    // stale references from being read when a later function uses fewer registers.
    registers.clear();
    scratch.registers = registers;
    scratch.rooted.clear();
    scratch.helper_arguments.clear();
    vm.native_register_pool.push(scratch);
    result
}
pub(crate) extern "C" fn power(left: f64, right: f64) -> f64 {
    left.powf(right)
}

#[repr(C)]
pub(crate) struct Context<'a> {
    pub(crate) vm: *mut Vm,
    pub(crate) inline_field_hits: u64,
    pub(crate) inline_block_hits: u64,
    function: &'a Rc<RefCell<Function>>,
    code: &'a Code,
    lir: &'a lir::Function,
    locations: &'a [(usize, usize)],
    // Deferred scalar recipes cannot contain references; GC reads only the remaining tagged values.
    root_states: Option<&'a [lir::FrameState]>,
    lazy_slots: bool,
    nest: Option<&'a crate::region::nest::LoopNest>,
    base: usize,
    rooted: Vec<usize>,
    // A word-sized sentinel lets native quiet blocks update bookkeeping without a Rust callback.
    pub(crate) previous: usize,
    pub(crate) skip_probe: bool,
    pub(crate) collecting: bool,
    error: Option<Error>,
    registers: *mut Slot,
    // Each native invocation owns its scratch buffer, including during recursive helper calls.
    helper_arguments: Vec<Value>,
    side_exit: Option<lir::ExitId>,
    views: crate::region::HeapViews,
    active_region: Option<usize>,
    region_done: Option<(usize, Value)>,
    schema_key: Vec<u64>,
}
/// Probe only existing instance fields. No allocation, Python calls or collection may occur here.
pub(crate) extern "C" fn field_callback(
    raw: *mut c_void,
    registers: *mut Slot,
    _event: i32,
    location: i32,
) -> i32 {
    // SAFETY: the suspended native entry owns this Context, VM and complete register image.
    let context = unsafe { &mut *raw.cast::<Context<'_>>() };
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<bool> {
        let vm = unsafe { &mut *context.vm };
        vm.tier_stats.native_runtime_calls += 1;
        if registers != context.registers {
            return Err(Error::InvalidIr("field register image"));
        }
        if context.collecting {
            return Ok(false);
        }
        let &(block, index) = context
            .locations
            .get(location as usize)
            .ok_or(Error::InvalidIr("field location"))?;
        let instruction = &context.lir.blocks[block].instructions[index];
        let Operation::Call {
            helper: lir::Helper::GetAttr(attribute),
            arguments,
            ..
        } = &instruction.operation
        else {
            return Err(Error::InvalidIr("field operation"));
        };
        // Verification proves the operand and destination lie inside this invocation's image.
        let receiver = unsafe { (*registers.add(arguments[0].0)).value()? };
        let value = match vm.heap.get(receiver) {
            Ok(crate::Object::Instance { attributes, .. }) => {
                let (value, hit) = attributes.read(attribute);
                if hit {
                    vm.tier_stats.shape_read_hits += 1;
                }
                value
            }
            _ => None,
        };
        if let Some(value) = value {
            unsafe {
                *registers.add(
                    instruction
                        .result
                        .ok_or(Error::InvalidIr("field destination"))?
                        .0,
                ) = Slot::boxed(value);
            }
            vm.tier_stats.native_field_hits += 1;
            Ok(true)
        } else {
            Ok(false)
        }
    }));
    match result {
        Ok(Ok(true)) => 0,
        Ok(Ok(false)) => 1,
        Ok(Err(error)) => {
            context.error = Some(error);
            2
        }
        Err(_) => {
            context.error = Some(Error::Runtime("panic in native field probe".into()));
            2
        }
    }
}

pub(crate) extern "C" fn callback_no_inline(
    raw: *mut c_void,
    registers: *mut Slot,
    event: i32,
    location: i32,
) -> i32 {
    callback_impl::<false>(raw, registers, event, location)
}
pub(crate) extern "C" fn callback(
    raw: *mut c_void,
    registers: *mut Slot,
    event: i32,
    location: i32,
) -> i32 {
    callback_impl::<true>(raw, registers, event, location)
}
#[inline(always)]
fn callback_impl<const SCOPED: bool>(
    raw: *mut c_void,
    registers: *mut Slot,
    event: i32,
    location: i32,
) -> i32 {
    // SAFETY: only generated Entry calls this private callback, using its original Context
    // pointer. Calls are synchronous; nested JIT frames allocate distinct contexts and slots.
    let context = unsafe { &mut *raw.cast::<Context<'_>>() };
    // SAFETY: each synchronous gateway holds the suspended invocation's exclusive VM borrow.
    unsafe {
        (&mut *context.vm).tier_stats.native_runtime_calls += 1;
    }

    // SAFETY: the VM's exclusive borrow is suspended for Entry; callbacks temporarily reborrow it.
    let vm = unsafe { &mut *context.vm };
    if event == UNPACK_BATCH {
        vm.tier_stats.native_unpack_batches += 1;
    }
    let checkpoint = (
        vm.stack.len(),
        vm.environments.len(),
        vm.transient_roots.len(),
        vm.depth,
        vm.local_frames.len(),
        vm.local_slots.len(),
    );
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        context.flush_region()?;
        context.step::<SCOPED>(registers, event, location)
    }));
    match result {
        Ok(Ok(())) => i32::from(context.side_exit.is_some()),
        Ok(Err(error)) => {
            context.error = Some(error);
            1
        }
        Err(_) => {
            // No Rust panic may unwind across a generated frame. Discard any abandoned nested frames.
            // SAFETY: generated code is still suspended in this callback, so the VM remains exclusively borrowed.
            let vm = unsafe { &mut *context.vm };
            vm.stack.truncate(checkpoint.0);
            vm.environments.truncate(checkpoint.1);
            vm.local_frames.truncate(checkpoint.4);
            vm.local_slots.truncate(checkpoint.5);
            vm.transient_roots.truncate(checkpoint.2);
            vm.depth = checkpoint.3;
            context.error = Some(Error::Runtime("panic in native runtime helper".into()));
            1
        }
    }
}
/// Validate identity/revision, binding and primitive operands only. The body executes in the
/// caller's generated code, never here. No VM allocation/GC/Python/frame mutation is permitted.
pub(crate) extern "C" fn inline_preflight(
    raw: *mut c_void,
    registers: *mut Slot,
    location: i32,
    output: *mut Slot,
) -> i32 {
    // SAFETY: the synchronous generated caller owns Context and its fixed image/scratch allocation.
    let context = unsafe { &mut *raw.cast::<Context<'_>>() };
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<bool> {
        context.flush_region()?;
        // SAFETY: this is the only temporary reborrow of the suspended caller's exclusive VM.
        let vm = unsafe { &mut *context.vm };
        vm.tier_stats.native_runtime_calls += 1;
        // Preserve the skipped callee-entry recursion/collection checkpoints. Pressure must use
        // the fully rooted original call before any speculative field access or body execution.
        if context.collecting || vm.depth >= 256 || !vm.allocation_fits_without_collection(0) {
            return Ok(false);
        }
        if registers != context.registers {
            return Err(Error::InvalidIr("inline image"));
        }
        let &(block, index) = context
            .locations
            .get(location as usize)
            .ok_or(Error::InvalidIr("inline location"))?;
        let Operation::Call {
            helper: lir::Helper::InlineCall { body, .. },
            arguments,
            ..
        } = &context.lir.blocks[block].instructions[index].operation
        else {
            return Err(Error::InvalidIr("inline body"));
        };
        // SAFETY: native installation reserves the maximum plan length after all region/local
        // metadata. Neither image nor scratch moves; no raw pointer survives this callback.
        let slots =
            unsafe { std::slice::from_raw_parts(registers, context.lir.registers.len() + 1) };
        let output = unsafe { std::slice::from_raw_parts_mut(output, body.nodes.len()) };
        let arguments = arguments
            .iter()
            .map(|r| slots[r.0].value())
            .collect::<Result<Vec<_>>>()?;
        let hit = body.prepare(&vm.heap, &arguments, output).is_some();
        if hit {
            vm.tier_stats.native_body_inlines += 1;
        }
        Ok(hit)
    }));
    match result {
        Ok(Ok(hit)) => i32::from(!hit),
        Ok(Err(error)) => {
            context.error = Some(error);
            -1
        }
        Err(_) => {
            context.error = Some(Error::Runtime("panic in inline preflight".into()));
            -1
        }
    }
}

/// Closed scalar/name evaluation and budgeted tuple allocation cannot collect or alter VM frames.
/// Status is 0=hit, 1=miss, -1=error. A miss occurs before allocation/effects and uses the original
/// fully rooted helper exactly once. Heap::allocate must remain non-collecting for this contract.
pub(crate) extern "C" fn guarded_helper_callback(
    raw: *mut c_void,
    registers: *mut Slot,
    event: i32,
    location: i32,
) -> i32 {
    // SAFETY: the synchronous native caller owns this Context and fixed register image throughout.
    let context = unsafe { &mut *raw.cast::<Context<'_>>() };
    // Count this C boundary honestly even though it bypasses the general dispatcher.
    unsafe {
        (&mut *context.vm).tier_stats.native_runtime_calls += 1;
    }
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<bool> {
        if !matches!(
            event,
            SCALAR_CALL | TUPLE_ALLOC | NAME_LOAD | NAME_STORE | STRING_NEXT
        ) || registers != context.registers
        {
            return Err(Error::InvalidIr("guarded helper gateway"));
        }
        if context.collecting {
            return Ok(false);
        }
        context.flush_region()?;
        let &(block, index) = context
            .locations
            .get(location as usize)
            .ok_or(Error::InvalidIr("guarded helper location"))?;
        let instruction = &context.lir.blocks[block].instructions[index];
        let Operation::Call {
            helper, arguments, ..
        } = &instruction.operation
        else {
            return Err(Error::InvalidIr("guarded helper instruction"));
        };
        // SAFETY: installation verifies offsets; the synchronous caller owns this fixed image.
        let slots =
            unsafe { std::slice::from_raw_parts_mut(registers, context.lir.registers.len() + 1) };
        // SAFETY: only this gateway temporarily reborrows the suspended caller's exclusive VM.
        let vm = unsafe { &mut *context.vm };
        let (probe_helper, probe_arguments) = match helper {
            lir::Helper::Scoped(inner) if matches!(inner.as_ref(), lir::Helper::ScalarCall(_)) => {
                (inner.as_ref(), &arguments[1..])
            }
            _ => (helper, arguments.as_slice()),
        };
        let value = match (event, probe_helper) {
            (SCALAR_CALL, lir::Helper::ScalarCall(builtin)) => {
                let (callee, arguments) = probe_arguments
                    .split_first()
                    .ok_or(Error::InvalidIr("scalar call operands"))?;
                if arguments.len() > 4 {
                    return Err(Error::InvalidIr("scalar call arity"));
                }
                let mut values = [Value::None; 4];
                for (dst, register) in values.iter_mut().zip(arguments) {
                    *dst = slots[register.0].value()?;
                }
                let Some(value) = vm.try_scalar_call(
                    *builtin,
                    slots[callee.0].value()?,
                    &values[..arguments.len()],
                ) else {
                    return Ok(false);
                };
                let value = value?;
                debug_assert!(matches!(value, Value::Int(_) | Value::Float(_)));
                value
            }
            (STRING_NEXT, lir::Helper::StringNext) => {
                let [iterator] = arguments.as_slice() else {
                    return Err(Error::InvalidIr("string next operands"));
                };
                let Some(value) = vm.try_string_next(slots[iterator.0].value()?)? else {
                    return Ok(false);
                };
                value
            }
            (NAME_LOAD, lir::Helper::LoadName(name)) => {
                // The authoritative cache/scope lookup only reads environments and updates metadata.
                // It cannot allocate VM objects, collect, or enter Python; Ref results stay in the image.
                vm.load_name_slot(&name.text, &name.cache)?
            }
            (NAME_STORE, lir::Helper::StoreName(name)) => {
                let [argument] = arguments.as_slice() else {
                    return Err(Error::InvalidIr("name store operands"));
                };
                if !vm.try_store_name_slot(slots[argument.0].value()?, &name.cache)? {
                    return Ok(false);
                }
                Value::None
            }
            (TUPLE_ALLOC, lir::Helper::Tuple) => {
                // Tuple literals allocate exactly one arena entry. Neither operand decoding nor
                // Heap::allocate collects/calls Python; reject pressure BEFORE creating the tuple.
                if !vm.allocation_fits_without_collection(1) {
                    return Ok(false);
                }
                let values = arguments
                    .iter()
                    .map(|r| slots[r.0].value())
                    .collect::<Result<Vec<_>>>()?;
                vm.heap.allocate(crate::Object::Tuple(values))
            }
            _ => return Err(Error::InvalidIr("guarded helper contract")),
        };
        slots[instruction
            .result
            .ok_or(Error::InvalidIr("guarded helper result"))?
            .0] = Slot::boxed(value);
        Ok(true)
    }));
    match result {
        Ok(Ok(hit)) => i32::from(!hit),
        Ok(Err(error)) => {
            if let Some(&(block, index)) = context.locations.get(location as usize)
                && let Operation::Call { state, .. } =
                    context.lir.blocks[block].instructions[index].operation
            {
                // SAFETY: generated code is suspended and the failed closed probe released its VM borrow.
                unsafe {
                    (&*context.vm).record_inline_error(context.lir, state);
                }
            }
            context.error = Some(error);
            -1
        }
        Err(_) => {
            context.error = Some(Error::Runtime("panic in native guarded helper".into()));
            -1
        }
    }
}

/// Float views cannot execute Python, collect, or change VM frames, so panic containment needs no frame checkpoint.
pub(crate) extern "C" fn float_view_callback(
    raw: *mut c_void,
    registers: *mut Slot,
    event: i32,
    location: i32,
) -> i32 {
    // SAFETY: generated callers pass their invocation's live Context; callbacks cannot retain it.
    let context = unsafe { &mut *raw.cast::<Context<'_>>() };
    // SAFETY: each synchronous gateway holds the suspended invocation's exclusive VM borrow.
    unsafe {
        (&mut *context.vm).tier_stats.native_runtime_calls += 1;
    }

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if event != FLOAT_VIEW {
            return Err(Error::InvalidIr("float view event"));
        }
        context.flush_region()?;
        // Promotion can change a cached descriptor's storage kind as well as its data pointer.
        if !context.views.descriptors.is_empty() {
            context.views.clear();
        }
        context.float_view(registers, location)
    }));
    match result {
        Ok(Ok(())) => i32::from(context.side_exit.is_some()),
        Ok(Err(error)) => {
            context.error = Some(error);
            1
        }
        Err(_) => {
            context.error = Some(Error::Runtime("panic in native float view".into()));
            1
        }
    }
}
/// Preparation can allocate owned view buffers but cannot execute Python or change VM frames.
pub(crate) extern "C" fn region_callback(
    raw: *mut c_void,
    registers: *mut Slot,
    event: i32,
    location: i32,
) -> i32 {
    // SAFETY: the private entry passes its live invocation context and exclusively owned register image.
    let context = unsafe { &mut *raw.cast::<Context<'_>>() };
    // SAFETY: each synchronous gateway holds the suspended invocation's exclusive VM borrow.
    unsafe {
        (&mut *context.vm).tier_stats.native_runtime_calls += 1;
    }

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        context.flush_region()?;
        if !matches!(event, REGION_ENTER | NEST_ENTER) || registers != context.registers {
            return Err(Error::InvalidIr("region entry ABI"));
        }
        if event == NEST_ENTER {
            context.prepare_nest()
        } else {
            context.prepare_region(location as usize)
        }
    }));
    match result {
        Ok(Ok(())) => i32::from(context.side_exit.is_some()),
        Ok(Err(error)) => {
            context.error = Some(error);
            1
        }
        Err(_) => {
            context.error = Some(Error::Runtime("panic preparing native region".into()));
            1
        }
    }
}
impl Context<'_> {
    fn flush_region(&mut self) -> Result<()> {
        let Some(index) = self.active_region.take() else {
            return Ok(());
        };
        // SAFETY: an active region or nest always has a private metadata slot in this invocation.
        let metadata = unsafe { *self.registers.add(self.lir.registers.len() + 1) };
        // SAFETY: the second metadata slot carries the descriptor length and loop count.
        let counts = unsafe { *self.registers.add(self.lir.registers.len() + 2) };
        // SAFETY: generated code is suspended or returned; only this callback reborrows the VM.
        let vm = unsafe { &mut *self.vm };
        let function = self.function.borrow();
        let mut profile = function.profile.borrow_mut();
        if index == usize::MAX {
            let nest = self.nest.ok_or(Error::InvalidIr("active nest metadata"))?;
            for (i, (from, to)) in nest.edges(self.lir).into_iter().enumerate() {
                // SAFETY: nest compilation reserves one initialized counter slot for every verified edge.
                let count = unsafe { (*self.registers.add(self.lir.registers.len() + 4 + i)).a };
                vm.tier_stats.native_region_iterations =
                    vm.tier_stats.native_region_iterations.saturating_add(count);
                let edge = profile.backedges.entry((from.0, to.0)).or_default();
                *edge = edge.saturating_add(count);
            }
        } else {
            let region = &self.lir.regions[index];
            vm.tier_stats.native_region_iterations = vm
                .tier_stats
                .native_region_iterations
                .saturating_add(counts.a);
            let count = profile
                .backedges
                .entry((region.body.0, region.header.0))
                .or_default();
            *count = count.saturating_add(counts.a);
        }
        self.previous = metadata.tag as usize;
        Ok(())
    }
    fn prepare_nest(&mut self) -> Result<()> {
        let nest = self.nest.ok_or(Error::InvalidIr("missing native nest"))?;
        // SAFETY: the native invocation exclusively owns both the VM and this complete register/counter image.
        let vm = unsafe { &mut *self.vm };
        let slots = unsafe {
            std::slice::from_raw_parts_mut(
                self.registers,
                self.lir.registers.len() + 4 + nest.edges(self.lir).len(),
            )
        };
        if vm.collecting(self.function) {
            self.side_exit = Some(nest.entry);
            return Ok(());
        }
        let mut roots = nest
            .imports
            .iter()
            .map(|r| slots[r.0].value())
            .collect::<Result<Vec<_>>>()?;
        let done = vm.heap.allocate(crate::Object::Done);
        roots.push(done);
        for recipe in &nest.private_iterators {
            let source = slots[recipe.source.0].value()?;
            if !matches!(
                vm.heap.get(source),
                Ok(crate::Object::List(_) | crate::Object::Tuple(_) | crate::Object::Range { .. })
            ) {
                vm.tier_stats.native_region_failures += 1;
                self.side_exit = Some(nest.entry);
                return Ok(());
            }
            let size = vm.heap.len(source)?;
            let iterator = vm.heap.allocate(crate::Object::Iterator {
                source,
                index: 0,
                size,
                exhausted: false,
            });
            slots[recipe.result.0] = Slot::boxed(iterator);
            roots.push(iterator);
        }
        let iterator = slots[nest.iterator.0].value()?;
        self.views.clear();
        if !self
            .views
            .prepare(&mut vm.heap, roots.into_iter().map(Ok), iterator)?
        {
            vm.tier_stats.native_region_failures += 1;
            self.side_exit = Some(nest.entry);
            return Ok(());
        }
        for &child in &nest.children {
            let region = &self.lir.regions[child];
            let iterator = slots[region.iterator.0].value()?;
            if let Some(schema) = &region.schema {
                let imports = region
                    .imports
                    .iter()
                    .map(|r| Ok((*r, slots[r.0].value()?)))
                    .collect::<Result<std::collections::BTreeMap<_, _>>>()?;
                if !schema.validate(&vm.heap, iterator, &imports)? {
                    vm.tier_stats.native_region_failures += 1;
                    self.side_exit = Some(nest.entry);
                    return Ok(());
                }
            }
        }
        // Auxiliary region state uses two slots: header/pointer, then length/iteration count.
        slots[self.lir.registers.len() + 1] = Slot {
            tag: nest.header.0 as u64,
            a: self.views.descriptors.as_ptr() as u64,
        };
        slots[self.lir.registers.len() + 2] = Slot {
            tag: self.views.descriptors.len() as u64,
            a: 0,
        };
        slots[self.lir.registers.len() + 3] = Slot::boxed(done);
        for i in 0..nest.edges(self.lir).len() {
            slots[self.lir.registers.len() + 4 + i] = Slot::default();
        }
        self.active_region = Some(usize::MAX);
        vm.tier_stats.native_region_entries += 1;
        Ok(())
    }
    fn prepare_region(&mut self, index: usize) -> Result<()> {
        let region = self
            .lir
            .regions
            .get(index)
            .ok_or(Error::InvalidIr("region index"))?;
        // SAFETY: preparation is a synchronous callback into the exclusively borrowed VM.
        let vm = unsafe { &mut *self.vm };
        // SAFETY: the native region backend allocated three auxiliary slots beyond its ordinary return slot.
        let slots =
            unsafe { std::slice::from_raw_parts_mut(self.registers, self.lir.registers.len() + 4) };
        let epoch = vm.heap.collection_epoch();
        let done = match self.region_done {
            Some((saved, value)) if saved == epoch => value,
            _ => {
                let value = vm.heap.allocate(crate::Object::Done);
                self.region_done = Some((epoch, value));
                value
            }
        };
        let iterator = slots[region.iterator.0].value()?;
        let roots = region
            .imports
            .iter()
            .map(|r| slots[r.0].value())
            .chain(std::iter::once(Ok(done)));
        if vm.collecting(self.function) || !self.views.prepare(&mut vm.heap, roots, iterator)? {
            vm.tier_stats.native_region_failures += 1;
            self.side_exit = Some(region.entry);
            return Ok(());
        }
        if let Some(schema) = &region.schema {
            let crate::Object::Iterator { source, .. } = vm.heap.get(iterator)? else {
                return Err(Error::InvalidIr("prepared iterator"));
            };
            let source = Slot::boxed(*source);
            self.schema_key.clear();
            self.schema_key.extend([index as u64, source.tag, source.a]);
            for register in &region.imports {
                if *register == region.iterator {
                    continue;
                }
                let s = slots[register.0];
                self.schema_key.extend([register.0 as u64, s.tag]);
                if s.tag & KIND_MASK == REF {
                    self.schema_key.extend([s.a]);
                }
            }
            if !self.views.schemas.contains(self.schema_key.as_slice()) {
                let imported = region
                    .imports
                    .iter()
                    .map(|r| Ok((*r, slots[r.0].value()?)))
                    .collect::<Result<std::collections::BTreeMap<_, _>>>()?;
                if !schema.validate(&vm.heap, iterator, &imported)? {
                    vm.tier_stats.native_region_failures += 1;
                    self.side_exit = Some(region.entry);
                    return Ok(());
                }
                self.views.schemas.insert(self.schema_key.clone());
            }
        }
        // Auxiliary region state uses two slots: header/pointer, then length/iteration count.
        slots[self.lir.registers.len() + 1] = Slot {
            tag: region.header.0 as u64,
            a: self.views.descriptors.as_ptr() as u64,
        };
        slots[self.lir.registers.len() + 2] = Slot {
            tag: self.views.descriptors.len() as u64,
            a: 0,
        };
        slots[self.lir.registers.len() + 3] = Slot::boxed(done);
        self.active_region = Some(index);
        vm.tier_stats.native_region_entries += 1;
        Ok(())
    }

    fn float_view(&mut self, registers: *mut Slot, location: i32) -> Result<()> {
        if registers != self.registers {
            return Err(Error::InvalidIr("native register buffer changed"));
        }
        let &(block, index) = self
            .locations
            .get(location as usize)
            .ok_or(Error::InvalidIr("float view location"))?;
        let instruction = &self.lir.blocks[block].instructions[index];
        // SAFETY: the private native entry owns this fixed-size image for the synchronous callback.
        let registers =
            unsafe { std::slice::from_raw_parts_mut(registers, self.lir.registers.len() + 1) };
        // SAFETY: generated code is suspended and the callback temporarily reborrows its exclusive VM.
        let vm = unsafe { &mut *self.vm };
        vm.tier_stats.native_float_views += 1;
        let Operation::FloatListView {
            value,
            length,
            failure,
        } = instruction.operation
        else {
            return Err(Error::InvalidIr("float view callback"));
        };
        let source = registers[value.0].value()?;
        let view = if matches!(source, Value::Ref(_)) {
            let mut object = vm.heap.get_mut(source)?;
            if let crate::Object::List(list) = &mut *object
                && length.is_none_or(|len| len == list.len())
            {
                list.float_view()
                    .map(|values| (values.as_mut_ptr() as usize, values.len()))
            } else {
                None
            }
        } else {
            None
        };
        let Some((pointer, length)) = view else {
            vm.tier_stats.native_float_view_failures += 1;
            self.side_exit = Some(failure);
            return Ok(());
        };
        let dst = instruction
            .result
            .ok_or(Error::InvalidIr("float view result"))?;
        // The verifier forbids view access after callbacks or across block edges. The owner stays live
        // in the register image; no collection or resizing operation occurs while generated code uses it.
        registers[dst.0] = Slot {
            tag: FLOAT_VIEW_TAG
                | ((length as u64) << VIEW_LENGTH_SHIFT)
                | if self.collecting { VIEW_FEEDBACK } else { 0 },
            a: pointer as u64,
        };
        Ok(())
    }

    fn step<const SCOPED: bool>(
        &mut self,
        registers: *mut Slot,
        event: i32,
        location: i32,
    ) -> Result<()> {
        if registers != self.registers {
            return Err(Error::InvalidIr("native register buffer changed"));
        }
        if event == LEAF_ENTER {
            let &(block, index) = self
                .locations
                .get(location as usize)
                .ok_or(Error::InvalidIr("native leaf location"))?;
            let body = &self.lir.blocks[block].instructions;
            let end = crate::leaf_inline::bootstrap_end(body, index)
                .ok_or(Error::InvalidIr("native leaf bootstrap"))?;
            let guard = body[index]
                .result
                .ok_or(Error::InvalidIr("native leaf guard"))?;
            // Quiet setup cannot allocate, collect, or invoke Python. It shares the ordinary
            // helper semantics but publishes all results in one suspended native invocation.
            if !self.collecting {
                // SAFETY: Entry owns this fixed image and exclusively suspends its VM borrow.
                let slots = unsafe {
                    std::slice::from_raw_parts_mut(registers, self.lir.registers.len() + 1)
                };
                let vm = unsafe { &mut *self.vm };
                vm.tier_stats.native_leaf_entry_batches += 1;
                if !self.views.descriptors.is_empty() {
                    self.views.floats_dirty = true;
                }
                for offset in std::iter::once(0).chain(2..=end - index) {
                    let instruction = &body[index + offset];
                    let Operation::Call {
                        helper, arguments, ..
                    } = &instruction.operation
                    else {
                        return Err(Error::InvalidIr("native leaf setup"));
                    };
                    self.helper_arguments.clear();
                    for argument in arguments {
                        self.helper_arguments.push(slots[argument.0].value()?);
                    }
                    let value = vm.lir_helper(helper, &self.helper_arguments)?;
                    let result = instruction
                        .result
                        .ok_or(Error::InvalidIr("native leaf setup result"))?;
                    slots[result.0] = Slot::boxed(value);
                    if vm.stack[self.base + result.0].is_none() {
                        self.rooted.push(result.0);
                    }
                    vm.stack[self.base + result.0] = Some(value);
                    if offset == 0 && !matches!(value, Value::Int(_)) {
                        break;
                    }
                }
                return Ok(());
            }
            self.step::<SCOPED>(registers, HELPER, location)?;
            // SAFETY: Entry supplied this fixed register image and the completed helper has
            // released its borrow. Read only the verified guard destination before continuing.
            let passed = unsafe { (*registers.add(guard.0)).value()? };
            if matches!(passed, Value::Int(_)) {
                for offset in 2..=end - index {
                    let next = location
                        .checked_add(i32::try_from(offset).map_err(error)?)
                        .ok_or(Error::IntegerOverflow)?;
                    self.step::<SCOPED>(registers, HELPER, next)?;
                }
            }
            return Ok(());
        }
        if event == SEQUENCE || event == UNPACK_BATCH {
            self.views.floats_dirty = true;
        }
        if event == UNPACK_BATCH && self.collecting {
            // A recursive call may reopen feedback while an artifact with fused reads is active.
            let &(block, index) = self
                .locations
                .get(location as usize)
                .ok_or(Error::InvalidIr("native unpack location"))?;
            let Operation::SequenceUnpack { count, .. } =
                self.lir.blocks[block].instructions[index].operation
            else {
                return Err(Error::InvalidIr("native unpack batch"));
            };
            for offset in 0..=count {
                let location = location
                    .checked_add(i32::try_from(offset).map_err(error)?)
                    .ok_or(Error::IntegerOverflow)?;
                self.step::<SCOPED>(registers, SEQUENCE, location)?;
                if self.side_exit.is_some() {
                    break;
                }
            }
            return Ok(());
        }
        // SAFETY: this is the fixed-size buffer passed to Entry. The callback has sole access while
        // generated code is suspended; helpers cannot resize or retain this buffer.
        let registers =
            unsafe { std::slice::from_raw_parts_mut(registers, self.lir.registers.len() + 1) };
        // SAFETY: callbacks reborrow the exclusively owned VM only until returning to Entry.
        let vm = unsafe { &mut *self.vm };
        if event == ENTER {
            // No collection runs in block bookkeeping; lazy artifacts publish roots at actual safepoints.
            if !self.lazy_slots {
                for parameter in &self.lir.blocks[location as usize].parameters {
                    if self.lir.registers[parameter.0] != lir::Type::Boxed {
                        continue;
                    }
                    let value = registers[parameter.0].value()?;
                    if vm.stack[self.base + parameter.0].is_none() {
                        self.rooted.push(parameter.0);
                    }
                    vm.stack[self.base + parameter.0] = Some(value);
                }
            }
            if let Some(origin) = self.lir.block_origins[location as usize] {
                if self.skip_probe {
                    self.skip_probe = false;
                } else {
                    vm.record_block(
                        self.function,
                        self.code,
                        origin.0,
                        (self.previous != usize::MAX).then_some(self.previous),
                    )?;
                }
                self.collecting = vm.collecting(self.function);
                self.previous = origin.0;
            }
            return Ok(());
        }
        let &(block, index) = self
            .locations
            .get(location as usize)
            .ok_or(Error::InvalidIr("native callback site"))?;
        let instruction = &self.lir.blocks[block].instructions[index];
        let scoped = || SCOPED && self.lir.is_scoped_instruction(instruction);
        if event == HELPER
            && let Operation::NumberCompare {
                value,
                integer,
                ref operator,
                failure,
            } = instruction.operation
        {
            let Some(value) =
                crate::runtime::number_compare(registers[value.0].value()?, integer, operator)
            else {
                self.side_exit = Some(failure);
                return Ok(());
            };
            registers[instruction
                .result
                .ok_or(Error::InvalidIr("number comparison result"))?
                .0] = Slot {
                tag: RAW_BOOL,
                a: u64::from(value),
            };
            return Ok(());
        }
        if event == HELPER
            && let Operation::NumericAdd {
                left,
                right,
                guard,
                overflow,
            } = instruction.operation
        {
            let observed = self.collecting && !scoped() && {
                vm.tier_stats.native_feedback_probes += 1;
                vm.observe_site_with(self.function, self.code, instruction.origin, |r| {
                    registers[r.0].value()
                })?
            };
            let value = match crate::runtime::numeric_add(
                registers[left.0].value()?,
                registers[right.0].value()?,
            ) {
                Some(Ok(value)) => value,
                miss => {
                    self.side_exit = Some(if miss.is_none() { guard } else { overflow });
                    return Ok(());
                }
            };
            registers[instruction
                .result
                .ok_or(Error::InvalidIr("numeric add result"))?
                .0] = Slot::boxed(value);
            if observed {
                self.function.borrow().profile.borrow_mut().sites[instruction.origin.0]
                    .result
                    .observe(crate::profile::ValueKind::of(&vm.heap, value));
            }
            return Ok(());
        }
        if event == UNPACK_BATCH {
            let Operation::SequenceUnpack {
                value,
                kind,
                count,
                failure,
            } = instruction.operation
            else {
                return Err(Error::InvalidIr("native unpack batch"));
            };
            let source = registers[value.0].value()?;
            let Some(items) = crate::sequence::items(&vm.heap, source, kind)?
                .filter(|items| items.len() == count)
            else {
                self.side_exit = Some(failure);
                return Ok(());
            };
            // Verification proves contiguous element reads with distinct destinations and no intervening effects.
            // Nothing in this batch can collect; later safepoints publish the complete live register image.
            let result = instruction
                .result
                .ok_or(Error::InvalidIr("native unpack result"))?;
            registers[result.0] = Slot::boxed(source);
            for offset in 0..items.len() {
                let value = items.get(offset).unwrap();
                let element = &self.lir.blocks[block].instructions[index + offset + 1];
                let result = element
                    .result
                    .ok_or(Error::InvalidIr("native unpack element"))?;
                registers[result.0] = Slot::boxed(value);
            }
            return Ok(());
        }
        if event == SEQUENCE {
            // These reads cannot call Python, mutate the heap or collect. Never retain payload borrows.
            let (result, failure) = match &instruction.operation {
                Operation::SequenceUnpack {
                    value,
                    kind,
                    count,
                    failure,
                } => (
                    crate::sequence::unpack(&vm.heap, registers[value.0].value()?, *kind, *count)?,
                    *failure,
                ),
                Operation::SequenceGet {
                    value,
                    kind,
                    index,
                    failure,
                } => {
                    let index = match index {
                        lir::SequenceIndex::Constant(i) => Value::Int(*i),
                        lir::SequenceIndex::Value(r) => registers[r.0].value()?,
                    };
                    (
                        crate::sequence::get(&vm.heap, registers[value.0].value()?, *kind, index)?,
                        *failure,
                    )
                }
                _ => return Err(Error::InvalidIr("native sequence instruction")),
            };
            let Some(value) = result else {
                self.side_exit = Some(failure);
                return Ok(());
            };
            let observed = self.collecting && !scoped() && {
                vm.tier_stats.native_feedback_probes += 1;
                vm.observe_site_with(self.function, self.code, instruction.origin, |v| {
                    registers[v.0].value()
                })?
            };
            let result = instruction
                .result
                .ok_or(Error::InvalidIr("sequence result"))?;
            registers[result.0] = Slot::boxed(value);
            if matches!(value, Value::Ref(_)) {
                if vm.stack[self.base + result.0].is_none() {
                    self.rooted.push(result.0);
                }
                vm.stack[self.base + result.0] = Some(value);
            }
            if observed {
                self.function.borrow().profile.borrow_mut().sites[instruction.origin.0]
                    .result
                    .observe(crate::profile::ValueKind::of(&vm.heap, value));
            }
            return Ok(());
        }
        let mut observed = false;
        let mut checkpoint = false;
        if event == HELPER {
            let (state, roots, effects) = match &instruction.operation {
                Operation::Call {
                    state,
                    roots,
                    effects,
                    ..
                }
                | Operation::Truth {
                    state,
                    roots,
                    effects,
                    ..
                } => (*state, roots, *effects),
                _ => return Err(Error::InvalidIr("native helper instruction")),
            };
            // Builtin iteration changes only iterator state; the invariant iterator is renewed at entry.
            // Other helpers invalidate owned views or may materialize float-list snapshots.
            if !self.views.descriptors.is_empty()
                && !matches!(
                    instruction.operation,
                    Operation::Call {
                        helper: lir::Helper::Python(
                            crate::hir::CoreOp::Iter
                                | crate::hir::CoreOp::Next
                                | crate::hir::CoreOp::IsDone
                        ),
                        ..
                    }
                )
            {
                if effects.may_write || effects.needs_safepoint() {
                    self.views.clear();
                } else {
                    self.views.floats_dirty = true;
                }
            }
            checkpoint = effects.needs_safepoint();
            if checkpoint {
                vm.tier_stats.native_root_syncs += 1;
                vm.sync_lir_roots::<SCOPED>(
                    &self.root_states.unwrap_or(&self.lir.states)[state.0],
                    roots,
                    self.base,
                    &mut self.rooted,
                    |r| registers[r.0].value(),
                )?;
            }
            let result = match &instruction.operation {
                Operation::Call {
                    helper, arguments, ..
                } => {
                    self.helper_arguments.clear();
                    for r in arguments {
                        self.helper_arguments.push(registers[r.0].value()?);
                    }
                    observed = self.collecting && !scoped() && {
                        vm.tier_stats.native_feedback_probes += 1;
                        vm.observe_site_with(self.function, self.code, instruction.origin, |v| {
                            registers[v.0].value()
                        })?
                    };
                    let roots = vm.transient_roots.len();
                    if effects.may_call {
                        vm.transient_roots.extend_from_slice(&self.helper_arguments);
                    }
                    let result = vm.lir_helper(helper, &self.helper_arguments);
                    if SCOPED && result.is_err() {
                        vm.record_inline_error(self.lir, state);
                    }
                    vm.transient_roots.truncate(roots);
                    // A recursive call can reopen this function's observation window.
                    if effects.may_call {
                        self.collecting = vm.collecting(self.function);
                    }
                    Slot::boxed(result?)
                }
                Operation::Truth { value, .. } => {
                    vm.tier_stats.native_truth_fallbacks += 1;
                    Slot {
                        tag: RAW_BOOL,
                        a: crate::runtime::truth(&vm.heap, registers[value.0].value()?)? as u64,
                    }
                }
                _ => unreachable!(),
            };
            registers[instruction.result.unwrap().0] = result;
        } else if event != FINISH {
            return Err(Error::InvalidIr("native callback event"));
        }
        if let Some(result) = instruction.result
            && self.lir.registers[result.0] == lir::Type::Boxed
        {
            if event == FINISH && self.collecting && !scoped() {
                vm.tier_stats.native_feedback_probes += 1;
                observed =
                    vm.observe_site_with(self.function, self.code, instruction.origin, |v| {
                        registers[v.0].value()
                    })?;
            }
            let value = registers[result.0].value()?;
            if vm.stack[self.base + result.0].is_none() {
                self.rooted.push(result.0);
            }
            vm.stack[self.base + result.0] = Some(value);
            let scalar = matches!(
                instruction.operation,
                Operation::BoxInt(_) | Operation::BoxFloat(_) | Operation::BoxBool(_)
            );
            if (observed || scalar) && !scoped() {
                let function = self.function.borrow();
                let mut profile = function.profile.borrow_mut();
                let site = &mut profile.sites[instruction.origin.0];
                if observed {
                    site.result
                        .observe(crate::profile::ValueKind::of(&vm.heap, value));
                }
                if scalar {
                    site.fast_paths = site.fast_paths.saturating_add(1);
                }
            }
        }
        // Only allocating/calling helpers are GC checkpoints; every such checkpoint has fresh roots.
        if checkpoint {
            vm.maybe_collect()
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
mod compact_value_tests {
    use super::*;

    #[test]
    fn pooled_scratch_is_empty_after_nested_calls_and_helper_errors() {
        let mut vm = Vm::new();
        vm.tiering = crate::Tiering::Native;
        vm.thresholds.function_entries = 1;
        vm.thresholds.loop_backedges = 4;
        vm.run_program(&mut crate::compile(
            "def recurse(n):\n    if n:\n        return recurse(n-1)\n    return len([1,2])\ndef repeat(n):\n    return 'abc'*n\nfor i in range(300):\n    assert recurse(4)==2\n    repeat(1)\n",
        ).unwrap()).unwrap();
        assert!(vm.native_register_pool.len() >= 2);
        assert!(
            vm.native_register_pool
                .iter()
                .any(|s| s.helper_arguments.capacity() > 0)
        );
        let repeat = vm.get_global("repeat").unwrap();
        assert!(vm.call(repeat, &[Value::Int(i64::MAX)]).is_err());
        assert!(vm.native_register_pool.iter().all(|s| {
            s.registers.is_empty() && s.rooted.is_empty() && s.helper_arguments.is_empty()
        }));
        vm.collect_garbage(&[]).unwrap();
        let value = vm.call(repeat, &[Value::Int(2)]).unwrap();
        assert_eq!(vm.heap.as_str(&value).unwrap(), "abcabc");
        assert_eq!(vm.stack_len(), 0);
    }

    #[test]
    fn boxed_slots_preserve_all_value_bits() {
        assert_eq!(std::mem::size_of::<Slot>(), 16);
        for value in [
            Value::Unbound,
            Value::None,
            Value::Bool(false),
            Value::Bool(true),
            Value::Int(i64::MIN),
            Value::Int(i64::MAX),
            Value::InlineString(crate::InlineString::new("").unwrap()),
            Value::InlineString(crate::InlineString::new("é😀abcde").unwrap()),
            Value::Ref(ObjectId {
                heap: u32::MAX,
                index: u32::MAX,
                generation: u32::MAX,
            }),
        ] {
            assert_eq!(Slot::boxed(value).value().unwrap(), value);
        }
        assert!(
            Slot {
                tag: INLINE_STRING,
                a: u64::MAX
            }
            .value()
            .is_err()
        );
        assert!(
            Slot {
                tag: INLINE_STRING | (0xff << 32),
                a: 1 << 56
            }
            .value()
            .is_err()
        );
        for bits in [
            0,
            1u64 << 63,
            f64::INFINITY.to_bits(),
            f64::NEG_INFINITY.to_bits(),
            0x7ff8_0000_0000_1234,
            0xfff8_0000_0000_4321,
        ] {
            let Value::Float(result) = Slot::boxed(Value::Float(f64::from_bits(bits)))
                .value()
                .unwrap()
            else {
                panic!("float changed kind");
            };
            assert_eq!(result.to_bits(), bits);
        }
    }
}
