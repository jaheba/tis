use crate::{
    Error, Heap, Object, Result, Value,
    bytecode::{Code, Opcode, read_operand},
    hir::{AddFeedback, AddStrategy, Function, ValueId},
    runtime,
};
use std::{cell::RefCell, rc::Rc};

// Offsets stay valid when nested calls grow the shared local-value buffer.
#[derive(Debug)]
pub(crate) struct LocalFrame {
    start: usize,
    len: usize,
    code: Rc<Code>,
}

/// Trailing defaults for an arity-checked positional call.
#[derive(Clone, Copy)]
pub(crate) struct PositionalBindings<'a> {
    pub defaults: &'a [Value],
    pub count: usize,
}
impl PositionalBindings<'_> {
    fn value(self, arguments: &[Value], index: usize) -> Value {
        arguments
            .get(index)
            .copied()
            .unwrap_or_else(|| self.defaults[index + self.defaults.len() - self.count])
    }
}

enum FrameInput<'a> {
    Fresh {
        arguments: &'a [Value],
        bindings: Option<PositionalBindings<'a>>,
    },
    Resume {
        slots: Vec<Option<Value>>,
        locals: Vec<Option<Value>>,
    },
}

#[derive(Debug, Default)]
pub struct Vm {
    pub heap: Heap,
    pub(crate) stack: Vec<Option<Value>>,
    pub tiering: crate::Tiering,
    pub bytecode: crate::BytecodeMode,
    pub numeric: crate::NumericMode,
    pub capture_native_code: bool,
    pub disable_native_nests: bool,
    /// Experimental multi-block incorporation; set before collecting feedback or compiling.
    pub cfg_inlining: bool,
    /// Experimental closed native recursion; set before compilation.
    pub readonly_regions: bool,
    /// Experimental ordered-write regions; set before compilation.
    pub mutation_regions: bool,
    pub(crate) copy_artifacts: std::collections::BTreeMap<
        (crate::profile::FunctionId, u64),
        Rc<crate::copy_patch::CopyCode>,
    >,
    pub(crate) native_artifacts: std::collections::BTreeMap<
        (crate::profile::FunctionId, u64),
        Rc<crate::native::NativeCode>,
    >,
    pub tier_stats: crate::TierStats,
    pub(crate) retired_versions: std::collections::BTreeSet<(crate::profile::FunctionId, u64)>,
    pub(crate) artifacts:
        std::collections::BTreeMap<(crate::profile::FunctionId, u64), Rc<crate::lir::Function>>,
    pub(crate) environments: Vec<Value>,
    pub(crate) local_frames: Vec<LocalFrame>,
    pub(crate) local_slots: Vec<Option<Value>>,
    pub(crate) register_scratch: Vec<crate::register_vm::RegisterScratch>,
    pub(crate) globals: Option<Value>,
    // Inactive buffers contain no roots. Each active native frame owns a distinct allocation.
    pub(crate) native_register_pool: Vec<crate::native_runtime::Scratch>,
    pub(crate) modules: std::collections::BTreeMap<String, Value>,
    pub(crate) literal_pool: std::collections::HashMap<String, Value>,
    pub output: String,
    pub(crate) depth: usize,
    pub(crate) transient_roots: Vec<Value>,
    pub(crate) try_bodies:
        std::collections::BTreeMap<(crate::profile::FunctionId, u64), Rc<RefCell<Function>>>,
    definition_codes: std::collections::BTreeMap<(crate::profile::FunctionId, u64), Rc<Code>>,
    pub(crate) expression_plans: std::collections::BTreeMap<
        (crate::profile::FunctionId, u64),
        Option<Rc<crate::execution::expression::Plan>>,
    >,
    pub(crate) expression_scratch: Vec<Value>,
    pub(crate) pending_exception: Option<Value>,
    pub(crate) active_exceptions: Vec<Value>,
    pinned: std::collections::BTreeMap<usize, Value>,
    next_root: usize,
    next_collection: usize,
    pub thresholds: crate::profile::HotThresholds,
    pub profiling: crate::profile::ProfileConfig,
    compilation_requests: Vec<crate::profile::CompilationRequest>,
    pub(crate) requested:
        std::collections::BTreeSet<(crate::profile::FunctionId, u64, crate::profile::Hotspot)>,
}

impl Vm {
    pub(crate) fn intern_literal(
        &mut self,
        text: &str,
        cache: &std::cell::Cell<Option<Value>>,
    ) -> Value {
        if let Some(text) = crate::InlineString::new(text) {
            return Value::InlineString(text);
        }
        if let Some(value) = cache.get()
            && self.heap.get(value).is_ok()
        {
            self.tier_stats.literal_hits += 1;
            return value;
        }
        let value = match self.literal_pool.get(text).copied() {
            Some(value) if self.heap.get(value).is_ok() => value,
            _ => {
                let value = self.heap.allocate(Object::String(text.into()));
                self.literal_pool.insert(text.into(), value);
                self.tier_stats.interned_literals += 1;
                value
            }
        };
        cache.set(Some(value));
        value
    }

    pub fn new() -> Self {
        Self::default()
    }

    /// Frame values occupy one stack window; both return and error release that window.
    pub fn run(&mut self, function: &mut Function, arguments: &[Value]) -> Result<Value> {
        if self.depth == 0 {
            self.pending_exception = None;
        }
        let handle = Rc::new(RefCell::new(function.clone()));
        let parent = self.ensure_globals();
        let result = self.invoke_function(handle.clone(), parent, &[], arguments, &[]);
        *function = handle.borrow().clone();
        result
    }

    pub fn run_program(&mut self, program: &mut crate::Program) -> Result<Value> {
        let module = program
            .function_mut("<module>")
            .ok_or(Error::InvalidIr("missing module"))?;
        self.run(module, &[])
    }

    pub(crate) fn execute_frame(
        &mut self,
        function: Rc<RefCell<Function>>,
        environment: Value,
        arguments: &[Value],
    ) -> Result<Value> {
        self.execute_bound_frame(function, environment, arguments, None)
    }

    pub(crate) fn execute_bound_frame(
        &mut self,
        function: Rc<RefCell<Function>>,
        environment: Value,
        arguments: &[Value],
        bindings: Option<PositionalBindings<'_>>,
    ) -> Result<Value> {
        if self.depth >= 256 {
            return Err(Error::Runtime(
                "RecursionError: maximum recursion depth exceeded".into(),
            ));
        }
        if bindings.is_some() {
            self.tier_stats.positional_frames += 1;
        }
        let code = {
            let mut function = function.borrow_mut();
            if let Some(code) = &function.code {
                code.clone()
            } else {
                let key = (function.id(), function.revision());
                if let Some(code) = self.definition_codes.get(&key) {
                    function.code = Some(code.clone());
                    code.clone()
                } else {
                    let start = std::time::Instant::now();
                    let result = function.shared_bytecode();
                    self.tier_stats.bytecode_compile_ns += start.elapsed().as_nanos() as u64;
                    let code = result?;
                    self.tier_stats.bytecode_compilations += 1;
                    // Buffers contain no owned heap handles; their weak caches validate heap/environment identity.
                    self.definition_codes.insert(key, code.clone());
                    code
                }
            }
        };
        {
            let function = function.borrow();
            let mut profile = function.profile.borrow_mut();
            profile.layout(function.blocks.len(), function.value_count);
            profile.entries = profile.entries.saturating_add(1);
        }
        self.consider_compilation(&function, crate::profile::Hotspot::Function);
        let collecting = self.collecting(&function);
        if collecting {
            let values = if let Some(bindings) = bindings {
                (0..bindings.count)
                    .map(|index| bindings.value(arguments, index))
                    .collect()
            } else {
                self.bound_arguments(&function.borrow(), environment, arguments)?
            };
            let hir = function.borrow();
            let mut profile = hir.profile.borrow_mut();
            profile
                .arguments
                .resize_with(values.len(), Default::default);
            for (profile, value) in profile.arguments.iter_mut().zip(values) {
                profile.observe(crate::profile::ValueKind::of(&self.heap, value));
            }
        }
        self.execute_window(
            function,
            code,
            environment,
            FrameInput::Fresh {
                arguments,
                bindings,
            },
            0,
            None,
            None,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn execute_window(
        &mut self,
        function: Rc<RefCell<Function>>,
        code: Rc<Code>,
        environment: Value,
        input: FrameInput<'_>,
        ip: usize,
        block: Option<usize>,
        pending: Option<crate::inline_frames::PendingResume>,
    ) -> Result<Value> {
        if self.depth >= 256 {
            return Err(Error::Runtime(
                "RecursionError: maximum recursion depth exceeded".into(),
            ));
        }
        let fresh = matches!(input, FrameInput::Fresh { .. });
        let base = self.stack.len();
        let local_start = self.local_slots.len();
        let (register_len, local_len) = match &input {
            FrameInput::Fresh { .. } => (code.slots, code.local_names.len()),
            FrameInput::Resume { slots, locals } => (slots.len(), locals.len()),
        };
        let frame_end = base + register_len;
        let initialized = (|| -> Result<()> {
            match input {
                FrameInput::Fresh {
                    arguments,
                    bindings,
                } => {
                    self.stack.resize(frame_end, None);
                    self.local_slots.resize(local_start + local_len, None);
                    for (&parameter, &value) in code.entry_parameters.iter().zip(arguments) {
                        self.stack[base + parameter.0] = Some(value);
                    }
                    if let Some(bindings) = bindings {
                        for (slot, parameter) in code.local_parameters.iter().enumerate() {
                            self.local_slots[local_start + slot] =
                                parameter.map(|index| bindings.value(arguments, index));
                        }
                    } else {
                        let mut object = self.heap.get_mut(environment)?;
                        let Object::Environment(env) = &mut *object else {
                            return Err(Error::InvalidIr("frame environment"));
                        };
                        for (slot, name) in code.local_names.iter().enumerate() {
                            self.local_slots[local_start + slot] = env.values.remove(name);
                        }
                    }
                }
                FrameInput::Resume { slots, locals } => {
                    self.stack.extend(slots);
                    self.local_slots.extend(locals);
                }
            }
            Ok(())
        })();
        if let Err(error) = initialized {
            self.local_slots.truncate(local_start);
            self.stack.truncate(base);
            return Err(error);
        }
        if self.depth == 0 {
            self.pending_exception = None;
        }
        let exception_depth = self.active_exceptions.len();
        self.depth += 1;
        self.environments.push(environment);
        self.local_frames.push(LocalFrame {
            start: local_start,
            len: local_len,
            code: code.clone(),
        });
        // Runtime calls currently recurse through Rust; grow before entering another frame
        // so the language recursion limit is reached before exhausting the native stack.
        let result = stacker::maybe_grow(128 * 1024, 2 * 1024 * 1024, || {
            if let Some(pending) = pending {
                let value = self.resume_inline_frames(pending.frames)?;
                self.stack[base + pending.continuation.destination.0] = Some(value);
                for slot in pending.continuation.local_destinations {
                    self.local_slots[local_start + slot] = Some(value);
                }
            }
            if fresh
                && ip == 0
                && let Some(lir) = self.artifact(code.function, code.revision)
            {
                let outcome = self.execute_tier(
                    &function,
                    &code,
                    lir,
                    base,
                    frame_end,
                    crate::hir::BlockId(0),
                    false,
                )?;
                self.finish_lir(&function, &code, base, frame_end, outcome)
            } else {
                self.execute(&function, &code, base, frame_end, ip, block)
            }
        });
        {
            let function = function.borrow();
            let mut profile = function.profile.borrow_mut();
            match &result {
                Ok(value) => {
                    profile.returns = profile.returns.saturating_add(1);
                    if self.profiling.collecting(profile.phase) {
                        profile
                            .return_types
                            .observe(crate::profile::ValueKind::of(&self.heap, *value));
                    }
                }
                Err(_) => profile.errors = profile.errors.saturating_add(1),
            }
            // Completed calls let recursive bodies contribute results before compilation freezes the window.
            profile.advance(self.profiling);
        }
        self.environments.pop();
        self.local_frames.pop();
        self.local_slots.truncate(local_start);
        self.depth -= 1;
        self.active_exceptions.truncate(exception_depth);
        self.stack.truncate(base);
        result
    }

    pub(crate) fn finish_lir(
        &mut self,
        function: &Rc<RefCell<Function>>,
        code: &Code,
        base: usize,
        frame_end: usize,
        outcome: crate::tier::Outcome,
    ) -> Result<Value> {
        match outcome {
            crate::tier::Outcome::Return(value) => Ok(value),
            crate::tier::Outcome::InlineDeopt(mut stack) => {
                stack.validate(&self.heap)?;
                let outer = &stack.frames[0].state;
                if outer.function != code.function
                    || outer.revision != code.revision
                    || outer.slots.len() != frame_end - base
                {
                    return Err(Error::InvalidIr("inline exit physical caller"));
                }
                let roots = self.transient_roots.len();
                self.transient_roots.extend(stack.roots());
                let result = (|| {
                    let outer = stack.frames.remove(0);
                    self.stack[base..frame_end].copy_from_slice(&outer.state.slots);
                    *self
                        .environments
                        .last_mut()
                        .ok_or(Error::InvalidIr("inline exit environment"))? =
                        outer.state.environment;
                    let locals = self
                        .local_frames
                        .last()
                        .ok_or(Error::InvalidIr("local frame"))?;
                    let start = locals.start;
                    if locals.len != outer.state.locals.len() {
                        return Err(Error::InvalidIr("inline exit physical locals"));
                    }
                    self.local_slots[start..start + locals.len]
                        .copy_from_slice(&outer.state.locals);
                    if let Some(pending) = outer.pending {
                        let value = self.resume_inline_frames(stack.frames)?;
                        self.stack[base + pending.destination.0] = Some(value);
                        for slot in pending.local_destinations {
                            self.local_slots[start + slot] = Some(value);
                        }
                    }
                    let ip = code.resume_in(self.bytecode, outer.state.resume)?;
                    self.execute(
                        function,
                        code,
                        base,
                        frame_end,
                        ip,
                        Some(outer.state.resume.block.0),
                    )
                })();
                self.transient_roots.truncate(roots);
                result
            }
            crate::tier::Outcome::Deopt(frame) => {
                let ip = code.resume_in(self.bytecode, frame.resume)?;
                self.stack[base..frame_end].copy_from_slice(&frame.slots);
                let locals = self
                    .local_frames
                    .last()
                    .ok_or(Error::InvalidIr("local frame"))?;
                if frame.locals.len() != locals.len {
                    return Err(Error::InvalidIr("deoptimization locals"));
                }
                self.local_slots[locals.start..locals.start + locals.len]
                    .copy_from_slice(&frame.locals);
                self.execute(
                    function,
                    code,
                    base,
                    frame_end,
                    ip,
                    Some(frame.resume.block.0),
                )
            }
        }
    }

    /// Continue at a pre-effect HIR boundary after a speculative LIR side exit.
    pub fn resume_hir(
        &mut self,
        function: &mut Function,
        frame: crate::lir::DeoptFrame,
    ) -> Result<Value> {
        if frame.function != function.id()
            || frame.revision != function.revision()
            || frame.slots.len() != function.value_count
            || frame.locals.len() != function.local_slots.len()
        {
            return Err(Error::InvalidIr("stale deoptimization frame"));
        }
        if !matches!(self.heap.get(frame.environment)?, Object::Environment(_)) {
            return Err(Error::InvalidIr("deoptimization environment"));
        }
        for value in frame.slots.iter().chain(&frame.locals).flatten() {
            if matches!(value, Value::Ref(_)) {
                self.heap.get(*value)?;
            }
        }
        let code = function.shared_bytecode()?;
        let ip = code.resume_in(self.bytecode, frame.resume)?;
        let handle = Rc::new(RefCell::new(function.clone()));
        {
            let function = handle.borrow();
            let mut profile = function.profile.borrow_mut();
            profile.layout(function.blocks.len(), function.value_count);
            profile.deoptimizations = profile.deoptimizations.saturating_add(1);
            if let Some(instruction) = function
                .blocks
                .get(frame.resume.block.0)
                .and_then(|block| block.instructions.get(frame.resume.instruction))
            {
                let site = &mut profile.sites[instruction.result.0];
                site.deoptimizations = site.deoptimizations.saturating_add(1);
            }
        }
        let result = self.execute_window(
            handle.clone(),
            code,
            frame.environment,
            FrameInput::Resume {
                slots: frame.slots,
                locals: frame.locals,
            },
            ip,
            Some(frame.resume.block.0),
            None,
        );
        *function = handle.borrow().clone();
        result
    }

    /// Resume the innermost logical frame, then install each result in its suspended caller.
    /// All snapshots are rooted before any resumed frame can collect.
    pub fn resume_inline_stack(
        &mut self,
        stack: crate::inline_frames::DeoptStack,
    ) -> Result<Value> {
        stack.validate(&self.heap)?;
        let roots = self.transient_roots.len();
        self.transient_roots.extend(stack.roots());
        let result = self.resume_inline_frames(stack.frames);
        self.transient_roots.truncate(roots);
        result
    }

    fn resume_inline_frames(
        &mut self,
        mut frames: Vec<crate::inline_frames::ResumeFrame>,
    ) -> Result<Value> {
        if frames.is_empty() {
            return Err(Error::InvalidIr("empty inline resume"));
        }
        let root = frames.remove(0);
        let mut source = (*root.source).clone();
        let code = source.shared_bytecode()?;
        let ip = code.resume_in(self.bytecode, root.state.resume)?;
        let pending = match root.pending {
            Some(continuation) => Some(crate::inline_frames::PendingResume {
                continuation,
                frames,
            }),
            None if frames.is_empty() => None,
            None => return Err(Error::InvalidIr("missing inline continuation")),
        };
        self.execute_window(
            Rc::new(RefCell::new(source)),
            code,
            root.state.environment,
            FrameInput::Resume {
                slots: root.state.slots,
                locals: root.state.locals,
            },
            ip,
            Some(root.state.resume.block.0),
            pending,
        )
    }

    pub fn collect_garbage(&mut self, external_roots: &[Value]) -> Result<usize> {
        let roots = self
            .stack
            .iter()
            .filter_map(|value| *value)
            .chain(self.environments.iter().copied())
            .chain(self.local_slots.iter().filter_map(|value| *value))
            .chain(self.transient_roots.iter().copied())
            .chain(self.pending_exception)
            .chain(self.active_exceptions.iter().copied())
            .chain(self.pinned.values().copied())
            .chain(self.modules.values().copied())
            .chain(self.literal_pool.values().copied())
            .chain(self.globals)
            .chain(external_roots.iter().copied())
            .filter(|value| matches!(value, Value::Ref(_)))
            .collect::<Vec<_>>();
        self.heap.collect(&roots)
    }

    /// Pin a host-held handle across automatic collections until `unpin` is called.
    pub fn pin(&mut self, value: Value) -> Result<usize> {
        if matches!(value, Value::Ref(_)) {
            self.heap.get(value)?;
        }
        let root = self.next_root;
        self.next_root = root.checked_add(1).ok_or(Error::IntegerOverflow)?;
        self.pinned.insert(root, value);
        Ok(root)
    }

    pub fn unpin(&mut self, root: usize) -> Option<Value> {
        self.pinned.remove(&root)
    }

    pub fn take_compilation_requests(&mut self) -> Vec<crate::profile::CompilationRequest> {
        std::mem::take(&mut self.compilation_requests)
    }

    pub(crate) fn consider_compilation(
        &mut self,
        function: &Rc<RefCell<Function>>,
        hotspot: crate::profile::Hotspot,
    ) {
        let function = function.borrow();
        let compiled_profile =
            function.profile.borrow().phase == crate::profile::ProfilePhase::Compiled;
        if self.profiling.mode == crate::profile::ProfileMode::Staged
            && compiled_profile
            && (self.tiering == crate::Tiering::Bytecode || self.has_lir(&function))
        {
            return;
        }
        let count = match hotspot {
            crate::profile::Hotspot::Function => function.profile.borrow().entries,
            crate::profile::Hotspot::Loop { header } => function
                .profile
                .borrow()
                .loop_count(crate::hir::BlockId(header)),
        };
        let threshold = match hotspot {
            crate::profile::Hotspot::Function => self.thresholds.function_entries,
            crate::profile::Hotspot::Loop { .. } => self.thresholds.loop_backedges,
        };
        let ready = compiled_profile
            || function.profile.borrow_mut().probe(
                threshold != 0 && count >= threshold,
                matches!(hotspot, crate::profile::Hotspot::Loop { .. }),
                self.profiling,
            );
        if !ready || threshold == 0 || count < threshold {
            return;
        }
        if self.numeric == crate::NumericMode::Adaptive
            && self.profiling.mode == crate::profile::ProfileMode::Staged
            && let Some(code) = &function.code
        {
            self.tier_stats.numeric_specializations = self
                .tier_stats
                .numeric_specializations
                .saturating_add(code.register.quicken(&function));
        }
        if self.profiling.mode == crate::profile::ProfileMode::Staged {
            self.retired_versions
                .remove(&(function.id(), function.revision()));
        }
        let notify = self
            .requested
            .insert((function.id(), function.revision(), hotspot));
        if self.tiering != crate::Tiering::Bytecode
            && !self.has_lir(&function)
            && !self
                .retired_versions
                .contains(&(function.id(), function.revision()))
        {
            match crate::lir::Function::lower_with_inline(
                &function,
                crate::lir::LoweringMode::Profiled,
                self.profiling.mode == crate::profile::ProfileMode::Staged,
                self.cfg_inlining,
            ) {
                Ok(lir) => {
                    self.artifacts
                        .insert((function.id(), function.revision()), Rc::new(lir));
                    self.tier_stats.lowered = self.tier_stats.lowered.saturating_add(1);
                    function.profile.borrow_mut().phase = crate::profile::ProfilePhase::Compiled;
                }
                Err(_) => {
                    function.profile.borrow_mut().phase = crate::profile::ProfilePhase::Retired;
                    self.retired_versions
                        .insert((function.id(), function.revision()));
                    self.tier_stats.compilation_errors =
                        self.tier_stats.compilation_errors.saturating_add(1);
                }
            }
        }
        if !notify {
            return;
        }
        self.compilation_requests
            .push(crate::profile::CompilationRequest {
                function: function.id(),
                revision: function.revision(),
                hotspot,
                count,
                hir: function.snapshot(),
            });
    }

    pub(crate) fn collecting(&self, function: &Rc<RefCell<Function>>) -> bool {
        self.profiling
            .collecting(function.borrow().profile.borrow().phase)
    }

    pub fn stack_len(&self) -> usize {
        self.stack.len()
    }

    #[allow(clippy::too_many_arguments)]
    fn execute(
        &mut self,
        function: &Rc<RefCell<Function>>,
        code: &Code,
        base: usize,
        frame_end: usize,
        ip: usize,
        block: Option<usize>,
    ) -> Result<Value> {
        match self.bytecode {
            crate::BytecodeMode::Stack => {
                self.execute_stack(function, code, base, frame_end, ip, block)
            }
            crate::BytecodeMode::Register => {
                self.execute_register(function, code, base, frame_end, ip, block)
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn execute_stack(
        &mut self,
        function: &Rc<RefCell<Function>>,
        code: &Code,
        base: usize,
        frame_end: usize,
        mut ip: usize,
        mut current_block: Option<usize>,
    ) -> Result<Value> {
        let mut collecting = self.collecting(function);
        let mut observed = false;
        loop {
            let opcode = Opcode::decode(
                *code
                    .body
                    .get(ip)
                    .ok_or(Error::InvalidIr("invalid instruction pointer"))?,
            )?;
            ip += 1;
            match opcode {
                Opcode::EnterBlock => {
                    let block = read_operand(&code.body, &mut ip)?;
                    let (is_backedge, phase) =
                        self.record_block_and_phase(function, code, block, current_block)?;
                    collecting = phase;
                    current_block = Some(block);
                    if is_backedge
                        && let Some(lir) = self.artifact(code.function, code.revision)
                        && lir.osr_entries.iter().any(|entry| entry.header.0 == block)
                    {
                        self.tier_stats.osr_entries = self.tier_stats.osr_entries.saturating_add(1);
                        let outcome = self.execute_tier(
                            function,
                            code,
                            lir,
                            base,
                            frame_end,
                            crate::hir::BlockId(block),
                            true,
                        )?;
                        return self.finish_lir(function, code, base, frame_end, outcome);
                    }
                }
                Opcode::Observe => {
                    let site = read_operand(&code.body, &mut ip)?;
                    observed =
                        collecting && self.observe_site(function, code, base, ValueId(site))?;
                }
                Opcode::Constant => {
                    let index = read_operand(&code.body, &mut ip)?;
                    let value = match code
                        .constants
                        .get(index)
                        .ok_or(Error::InvalidIr("invalid constant"))?
                    {
                        crate::bytecode::EncodedConstant::Immediate(value) => *value,
                        crate::bytecode::EncodedConstant::String(value) => {
                            self.intern_literal(value, &code.literal_cache[index])
                        }
                    };
                    self.push(value);
                }
                Opcode::Load => {
                    let slot = ValueId(read_operand(&code.body, &mut ip)?);
                    let value = self.read(base, slot)?;
                    self.push(value);
                }
                Opcode::Store => {
                    let slot = ValueId(read_operand(&code.body, &mut ip)?);
                    let value = self.pop(frame_end)?;
                    self.write(base, slot, value)?;
                    if observed {
                        let function = function.borrow();
                        if function.instruction(slot).is_some() {
                            function.profile.borrow_mut().sites[slot.0]
                                .result
                                .observe(crate::profile::ValueKind::of(&self.heap, value));
                        }
                    }
                }
                Opcode::Add => {
                    let site = ValueId(read_operand(&code.body, &mut ip)?);
                    let rhs = self.pop(frame_end)?;
                    let lhs = self.pop(frame_end)?;
                    let (value, force_observed) = self.adaptive_add(function, site, lhs, rhs)?;
                    if force_observed {
                        collecting = self.collecting(function);
                        observed = true;
                    }
                    self.push(value);
                }
                Opcode::Subtract | Opcode::Less => {
                    let rhs = self.pop(frame_end)?;
                    let lhs = self.pop(frame_end)?;
                    let value = if matches!(opcode, Opcode::Subtract) {
                        runtime::subtract(lhs, rhs)?
                    } else {
                        runtime::less(lhs, rhs)?
                    };
                    self.push(value);
                }
                Opcode::List | Opcode::Tuple | Opcode::Dict => {
                    let count = read_operand(&code.body, &mut ip)?;
                    let count = if matches!(opcode, Opcode::Dict) {
                        count
                            .checked_mul(2)
                            .ok_or(Error::InvalidIr("collection operand overflow"))?
                    } else {
                        count
                    };
                    if self.stack.len().saturating_sub(frame_end) < count {
                        return Err(Error::InvalidIr("operand stack underflow"));
                    }
                    let values = self
                        .stack
                        .split_off(self.stack.len() - count)
                        .into_iter()
                        .map(|value| value.ok_or(Error::InvalidIr("empty operand")))
                        .collect::<Result<Vec<_>>>()?;
                    let result = match opcode {
                        Opcode::List => self.heap.allocate(Object::List(values.into())),
                        Opcode::Tuple => self.heap.allocate(Object::Tuple(values)),
                        _ => {
                            let result = self.heap.allocate(Object::Dict(Default::default()));
                            for pair in values.as_chunks::<2>().0 {
                                self.heap.set_item(result, pair[0], pair[1])?;
                            }
                            result
                        }
                    };
                    self.push(result);
                }
                Opcode::GetItem => {
                    let key = self.pop(frame_end)?;
                    let object = self.pop(frame_end)?;
                    let value = self.heap.get_item(object, key)?;
                    self.push(value);
                }
                Opcode::SetItem => {
                    let value = self.pop(frame_end)?;
                    let key = self.pop(frame_end)?;
                    let object = self.pop(frame_end)?;
                    self.heap.set_item(object, key, value)?;
                    self.push(Value::None);
                }
                Opcode::Len => {
                    let object = self.pop(frame_end)?;
                    let len = self.heap.len(object)?;
                    self.push(Value::Int(
                        i64::try_from(len).map_err(|_| Error::IntegerOverflow)?,
                    ));
                }
                Opcode::Jump => ip = read_operand(&code.body, &mut ip)?,
                Opcode::JumpIfFalse => {
                    let target = read_operand(&code.body, &mut ip)?;
                    let value = self.pop(frame_end)?;
                    if !runtime::truth(&self.heap, value)? {
                        ip = target;
                    }
                }
                Opcode::Runtime => {
                    let id = read_operand(&code.body, &mut ip)?;
                    let (operation, count) = code
                        .operations
                        .get(id)
                        .ok_or(Error::InvalidIr("runtime operation"))?;
                    if self.stack.len().saturating_sub(frame_end) < *count {
                        return Err(Error::InvalidIr("operand stack underflow"));
                    }
                    let values = self
                        .stack
                        .split_off(self.stack.len() - count)
                        .into_iter()
                        .map(|value| value.ok_or(Error::InvalidIr("empty operand")))
                        .collect::<Result<Vec<_>>>()?;
                    let roots = self.transient_roots.len();
                    self.transient_roots.extend_from_slice(&values);
                    let result = self.operation(operation, &values);
                    self.transient_roots.truncate(roots);
                    let result = result?;
                    self.push(result);
                }
                Opcode::Return => return self.pop(frame_end),
                Opcode::LoadLocal => {
                    let slot = read_operand(&code.body, &mut ip)?;
                    self.push(self.load_local(slot)?);
                }
                Opcode::StoreLocal => {
                    let slot = read_operand(&code.body, &mut ip)?;
                    let value = self.pop(frame_end)?;
                    self.store_local(slot, value)?;
                    self.push(Value::None);
                }
                Opcode::CheckLocal => {
                    let slot = read_operand(&code.body, &mut ip)?;
                    let value = self.pop(frame_end)?;
                    self.check_local(slot, value)?;
                    self.push(value);
                }
            }
            // Collect only between instructions, when all live temporaries are rooted.
            self.maybe_collect()?;
        }
    }

    pub(crate) fn local_values(&self) -> Result<&[Option<Value>]> {
        let frame = self
            .local_frames
            .last()
            .ok_or(Error::InvalidIr("local frame"))?;
        Ok(&self.local_slots[frame.start..frame.start + frame.len])
    }

    pub(crate) fn load_local(&self, slot: usize) -> Result<Value> {
        let frame = self
            .local_frames
            .last()
            .ok_or(Error::InvalidIr("local frame"))?;
        if slot >= frame.len {
            return Err(Error::InvalidIr("local slot"));
        }
        self.local_slots[frame.start + slot].ok_or_else(|| {
            Error::Runtime(format!(
                "UnboundLocalError: {}",
                frame.code.local_names[slot]
            ))
        })
    }

    pub(crate) fn check_local(&self, slot: usize, value: Value) -> Result<()> {
        if value == Value::Unbound {
            let frame = self
                .local_frames
                .last()
                .ok_or(Error::InvalidIr("local frame"))?;
            let name = frame
                .code
                .local_names
                .get(slot)
                .ok_or(Error::InvalidIr("local slot"))?;
            return Err(Error::Runtime(format!("UnboundLocalError: {name}")));
        }
        Ok(())
    }

    pub(crate) fn store_local(&mut self, slot: usize, value: Value) -> Result<()> {
        let frame = self
            .local_frames
            .last()
            .ok_or(Error::InvalidIr("local frame"))?;
        if slot >= frame.len {
            return Err(Error::InvalidIr("local slot"));
        }
        self.local_slots[frame.start + slot] = Some(value);
        Ok(())
    }

    fn push(&mut self, value: Value) {
        self.stack.push(Some(value));
    }

    fn pop(&mut self, frame_end: usize) -> Result<Value> {
        if self.stack.len() <= frame_end {
            return Err(Error::InvalidIr("operand stack underflow"));
        }
        self.stack
            .pop()
            .flatten()
            .ok_or(Error::InvalidIr("empty operand"))
    }

    #[inline]
    pub(crate) fn adaptive_add(
        &mut self,
        function: &Rc<RefCell<Function>>,
        site: ValueId,
        lhs: Value,
        rhs: Value,
    ) -> Result<(Value, bool)> {
        let mut force_observed = false;
        let mut hir = function.borrow_mut();
        let feedback = hir
            .feedback_mut(site)
            .ok_or(Error::InvalidIr("addition has no feedback site"))?;
        let before = (feedback.fast_paths, feedback.guard_failures);
        let result = self.add(lhs, rhs, feedback);
        let delta = (
            feedback.fast_paths.saturating_sub(before.0),
            feedback.guard_failures.saturating_sub(before.1),
        );
        let mut profile = hir.profile.borrow_mut();
        if delta.1 > 0 && self.profiling.mode == crate::profile::ProfileMode::Staged {
            profile.start_window(self.profiling);
            force_observed = true;
            let failed = &mut profile.sites[site.0];
            failed.executions = failed.executions.saturating_add(1);
            failed.operands.resize_with(2, Default::default);
            failed.operands[0].observe(crate::profile::ValueKind::of(&self.heap, lhs));
            failed.operands[1].observe(crate::profile::ValueKind::of(&self.heap, rhs));
            self.requested
                .retain(|(id, revision, _)| *id != hir.id() || *revision != hir.revision());
        }
        let site = &mut profile.sites[site.0];
        site.fast_paths = site.fast_paths.saturating_add(delta.0);
        site.guard_failures = site.guard_failures.saturating_add(delta.1);
        result.map(|value| (value, force_observed))
    }

    fn add(&mut self, lhs: Value, rhs: Value, feedback: &mut AddFeedback) -> Result<Value> {
        feedback.executions = feedback.executions.saturating_add(1);
        if feedback.strategy == AddStrategy::Integer {
            if let (Value::Int(lhs), Value::Int(rhs)) = (lhs, rhs)
                && let Some(value) = lhs.checked_add(rhs)
            {
                feedback.fast_paths = feedback.fast_paths.saturating_add(1);
                return Ok(Value::Int(value));
            }
            feedback.guard_failures = feedback.guard_failures.saturating_add(1);
            feedback.strategy = AddStrategy::Generic;
            feedback.consecutive_integers = 0;
        }
        let result = runtime::add(&mut self.heap, lhs, rhs);
        if matches!(
            (lhs, rhs, &result),
            (Value::Int(_), Value::Int(_), Ok(Value::Int(_)))
        ) {
            feedback.consecutive_integers += 1;
            if feedback.consecutive_integers == AddFeedback::WARMUP {
                feedback.strategy = AddStrategy::Integer;
            }
        } else {
            feedback.consecutive_integers = 0;
        }
        result
    }

    pub(crate) fn read(&self, base: usize, value: ValueId) -> Result<Value> {
        self.stack
            .get(base + value.0)
            .ok_or(Error::InvalidIr("invalid value"))?
            .ok_or(Error::Uninitialized(value.0))
    }

    pub(crate) fn write(&mut self, base: usize, value: ValueId, result: Value) -> Result<()> {
        *self
            .stack
            .get_mut(base + value.0)
            .ok_or(Error::InvalidIr("invalid value"))? = Some(result);
        Ok(())
    }
}

impl Vm {
    pub(crate) fn record_block(
        &mut self,
        function: &Rc<RefCell<Function>>,
        code: &Code,
        block: usize,
        previous: Option<usize>,
    ) -> Result<bool> {
        self.record_block_and_phase(function, code, block, previous)
            .map(|(backedge, _)| backedge)
    }

    // Read the live phase at every block, including after recursive calls; only backedges can
    // change it here. Returning it avoids the interpreter borrowing the same profile twice.
    pub(crate) fn record_block_and_phase(
        &mut self,
        function: &Rc<RefCell<Function>>,
        code: &Code,
        block: usize,
        previous: Option<usize>,
    ) -> Result<(bool, bool)> {
        let backedge = previous.is_some_and(|from| {
            code.backedge_sources
                .get(block)
                .is_some_and(|sources| sources.contains(&from))
        });
        let mut collecting;
        {
            let function = function.borrow();
            let mut profile = function.profile.borrow_mut();
            collecting = self.profiling.collecting(profile.phase);
            if collecting {
                let count = profile
                    .blocks
                    .get_mut(block)
                    .ok_or(Error::InvalidIr("profile block"))?;
                *count = count.saturating_add(1);
                if let Some(previous) = previous {
                    let count = profile.edges.entry((previous, block)).or_default();
                    *count = count.saturating_add(1);
                }
            }
            if let Some(previous) = previous
                && backedge
            {
                let count = profile.backedges.entry((previous, block)).or_default();
                *count = count.saturating_add(1);
            }
        }
        if backedge {
            self.consider_compilation(function, crate::profile::Hotspot::Loop { header: block });
            collecting = self.collecting(function);
        }
        Ok((backedge, collecting))
    }
    pub(crate) fn observe_site(
        &self,
        function: &Rc<RefCell<Function>>,
        code: &Code,
        base: usize,
        value: ValueId,
    ) -> Result<bool> {
        self.observe_site_with(function, code, value, |value| self.read(base, value))
    }

    pub(crate) fn observe_site_with(
        &self,
        function: &Rc<RefCell<Function>>,
        code: &Code,
        value: ValueId,
        mut read: impl FnMut(ValueId) -> Result<Value>,
    ) -> Result<bool> {
        if !self.collecting(function) {
            return Ok(false);
        }
        {
            let function = function.borrow();
            let mut profile = function.profile.borrow_mut();
            let window = profile.windows;
            if !profile.sites[value.0].sample(window, self.profiling) {
                return Ok(false);
            }
        }
        let metadata = code
            .sites
            .get(value.0)
            .and_then(Option::as_ref)
            .ok_or(Error::InvalidIr("profile site"))?;
        let target = if metadata.call {
            metadata
                .inputs
                .first()
                .map(|&input| {
                    read(input).map(|value| crate::profile::CallTarget::of(&self.heap, value))
                })
                .transpose()?
        } else {
            None
        };
        // Recursive targets share this profile. Capture their semantic body before borrowing
        // the caller's feedback mutably, or the plan's profile snapshot cannot be read.
        let leaf_body =
            if target.is_some() && self.profiling.mode == crate::profile::ProfileMode::Staged {
                crate::leaf_inline::Plan::observe(
                    &self.heap,
                    read(metadata.inputs[0])?,
                    self.cfg_inlining,
                )
            } else {
                None
            };
        let function = function.borrow();
        let mut profile = function.profile.borrow_mut();
        let site = &mut profile.sites[value.0];
        site.operands
            .resize_with(metadata.inputs.len(), Default::default);
        for (operand, &input) in site.operands.iter_mut().zip(&metadata.inputs) {
            operand.observe(crate::profile::ValueKind::of(&self.heap, read(input)?));
        }
        if let Some(target) = target {
            if self.profiling.mode == crate::profile::ProfileMode::Staged {
                site.leaf_body = leaf_body;
            }
            if site.inline_body.is_none() {
                site.inline_body =
                    crate::body_inline::Plan::observe(&self.heap, read(metadata.inputs[0])?);
            }
            site.call(target);
        }
        Ok(true)
    }
    /// A budget is useful only for an audited operation with this exact allocation bound and
    /// no internal GC/reentrancy. It does not make arbitrary allocating helpers safe to elide.
    #[inline]
    pub(crate) fn allocation_fits_without_collection(&self, objects: usize) -> bool {
        self.heap
            .allocation_count()
            .checked_add(objects)
            .is_some_and(|count| count < self.next_collection.max(4096))
    }
    pub(crate) fn maybe_collect(&mut self) -> Result<()> {
        if !self.allocation_fits_without_collection(0) {
            self.collect_garbage(&[])?;
            self.next_collection = self.heap.allocation_count().saturating_mul(2).max(4096);
        }
        Ok(())
    }
}

#[cfg(test)]
mod frame_storage_tests {
    use super::*;
    #[test]
    fn local_stack_ranges_are_released_on_success_and_failure() {
        let mut vm = Vm::new();
        vm.run_program(
            &mut crate::compile("def f(x=42):\n    return x\ndef fail(x):\n    return 1//0\n")
                .unwrap(),
        )
        .unwrap();
        let f = vm.get_global("f").unwrap();
        let fail = vm.get_global("fail").unwrap();
        for _ in 0..10 {
            assert_eq!(vm.call(f, &[]).unwrap(), Value::Int(42));
            assert!(vm.call(fail, &[Value::Int(1)]).is_err());
            assert!(vm.local_slots.is_empty());
            assert!(vm.local_frames.is_empty());
            assert!(vm.stack.is_empty());
            assert!(vm.environments.is_empty());
        }
        assert!(vm.tier_stats.register_scratch_reuses > 0);
    }
}

#[cfg(test)]
mod tuple_budget_tests {
    use super::*;

    #[test]
    fn tuple_budget_preserves_hidden_roots_at_the_collection_boundary() {
        for due in [false, true] {
            let mut vm = Vm::new();
            vm.tiering = crate::Tiering::Native;
            vm.thresholds.function_entries = 1;
            vm.thresholds.loop_backedges = 4;
            vm.next_collection = usize::MAX;
            vm.run_program(&mut crate::compile("class Box:\n    def __init__(self,x):\n        self.value=x\nholder=Box([1])\ndef f(holder):\n    keep=holder.value\n    holder.value=None\n    temporary=(7,)\n    return keep,temporary\nfor i in range(300):\n    holder.value=[i]\n    f(holder)\n").unwrap()).unwrap();
            let f = vm.get_global("f").unwrap();
            // Keep must live only in SSA/native state, not an independently traced local slot.
            let hir = vm.function_hir(f).unwrap();
            assert!(!hir.blocks.iter().flat_map(|b| &b.instructions).any(|i| {
                matches!(
                    i.op,
                    crate::hir::Op::Runtime(crate::hir::CoreOp::StoreLocal(_), _)
                )
            }));
            let holder = vm.get_global("holder").unwrap();
            let attribute: crate::Attribute = "value".to_owned().into();
            let child = vm.heap.allocate(Object::List(vec![Value::Int(1)].into()));
            vm.set_attribute(holder, &attribute, child).unwrap();
            let count = vm.heap.allocation_count();
            vm.call(f, &[holder]).unwrap();
            // Entry environments also allocate. Measure the closed call's exact total, then
            // place pressure at its FIRST tuple, whose contents do not include the live keep value.
            let delta = vm.heap.allocation_count() - count;
            assert!(delta >= 2);
            for _ in 0..6000 {
                vm.heap.allocate(Object::Tuple(vec![]));
            }
            let keep = vm.heap.allocate(Object::List(vec![Value::Int(456)].into()));
            vm.set_attribute(holder, &attribute, keep).unwrap();
            let count = vm.heap.allocation_count();
            vm.next_collection = if due {
                count + delta - 1
            } else {
                count + delta + 1
            };
            let epoch = vm.heap.collection_epoch();
            let roots = vm.tier_stats.native_root_syncs;
            let fields = vm.tier_stats.native_inline_field_hits;
            let result = vm.call(f, &[holder]).unwrap();
            assert!(vm.tier_stats.native_inline_field_hits > fields);
            assert_eq!(vm.heap.get_item(result, Value::Int(0)).unwrap(), keep);
            assert_eq!(
                vm.heap.get_item(keep, Value::Int(0)).unwrap(),
                Value::Int(456)
            );
            let temporary = vm.heap.get_item(result, Value::Int(1)).unwrap();
            assert_eq!(
                vm.heap.get_item(temporary, Value::Int(0)).unwrap(),
                Value::Int(7)
            );
            if due {
                assert!(vm.heap.collection_epoch() > epoch);
                assert_eq!(vm.tier_stats.native_root_syncs - roots, 1);
                assert!(vm.heap.allocation_count() < count);
            } else {
                assert_eq!(vm.heap.collection_epoch(), epoch);
                assert_eq!(vm.tier_stats.native_root_syncs, roots);
                assert_eq!(vm.heap.allocation_count(), count + delta);
            }
            vm.collect_garbage(&[result]).unwrap();
            assert!(vm.heap.get(keep).is_ok());
            vm.collect_garbage(&[]).unwrap();
            assert!(vm.heap.get(keep).is_err());
            assert_eq!(vm.stack_len(), 0);
        }
    }

    #[test]
    fn allocation_budget_overflow_uses_the_slow_path() {
        let mut vm = Vm::new();
        vm.heap.allocate(Object::Tuple(vec![]));
        vm.next_collection = usize::MAX;
        assert!(!vm.allocation_fits_without_collection(usize::MAX));
    }
}

#[cfg(test)]
mod leaf_inline_tests {
    use super::*;

    #[test]
    fn scalarized_tuple_components_survive_collection_after_return() {
        for tier in [
            crate::Tiering::Lir,
            crate::Tiering::Native,
            crate::Tiering::CopyPatch,
        ] {
            let mut vm = Vm::new();
            vm.tiering = tier;
            vm.cfg_inlining = true;
            vm.thresholds.function_entries = 16;
            vm.next_collection = usize::MAX;
            vm.run_program(&mut crate::compile("class Box:\n    def __init__(self):\n        self.a=None\n        self.b=None\ndef pair(holder):\n    first=holder.a\n    second=holder.b\n    holder.a=None\n    holder.b=None\n    return first,second\ndef caller(holder):\n    a,b=pair(holder)\n    garbage=[0]\n    return a,b,garbage\nholder=Box()\nfor i in range(500):\n    holder.a=[i]\n    holder.b=[i]\n    caller(holder)\n").unwrap()).unwrap();
            let caller = vm.get_global("caller").unwrap();
            let holder = vm.get_global("holder").unwrap();
            // Force collection at the caller allocation after the eliminated tuple.
            for _ in 0..5000 {
                vm.heap.allocate(Object::Tuple(vec![]));
            }
            let a = vm.heap.allocate(Object::List(vec![Value::Int(123)].into()));
            let b = vm.heap.allocate(Object::List(vec![Value::Int(456)].into()));
            vm.set_attribute(holder, &"a".to_owned().into(), a).unwrap();
            vm.set_attribute(holder, &"b".to_owned().into(), b).unwrap();
            vm.next_collection = vm.heap.allocation_count() + 1;
            let epoch = vm.heap.collection_epoch();
            let hits = vm.tier_stats.virtual_tuple_returns;
            let result = vm.call(caller, &[holder]).unwrap();
            assert_eq!(vm.tier_stats.virtual_tuple_returns, hits + 1);
            assert!(vm.heap.collection_epoch() > epoch);
            assert_eq!(vm.heap.get_item(result, Value::Int(0)).unwrap(), a);
            assert_eq!(vm.heap.get_item(result, Value::Int(1)).unwrap(), b);
            assert_eq!(vm.heap.get_item(a, Value::Int(0)).unwrap(), Value::Int(123));
            assert_eq!(vm.heap.get_item(b, Value::Int(0)).unwrap(), Value::Int(456));
            assert!(
                vm.stack.is_empty()
                    && vm.local_frames.is_empty()
                    && vm.local_slots.is_empty()
                    && vm.transient_roots.is_empty()
            );
            vm.collect_garbage(&[]).unwrap();
            assert!(vm.heap.get(a).is_err() && vm.heap.get(b).is_err());
        }
    }

    #[test]
    fn copied_leaves_preserve_the_language_recursion_limit() {
        for tier in [
            crate::Tiering::Lir,
            crate::Tiering::Native,
            crate::Tiering::CopyPatch,
        ] {
            let mut vm = Vm::new();
            vm.tiering = tier;
            vm.thresholds.function_entries = 16;
            vm.run_program(&mut crate::compile("class Box:\n    def __init__(self):\n        self.x=0\n    def leaf(self):\n        self.x+=1\ndef caller(b):\n    return b.leaf()\nb=Box()\nfor i in range(500):\n    caller(b)\n").unwrap()).unwrap();
            let caller = vm.get_global("caller").unwrap();
            let b = vm.get_global("b").unwrap();
            assert!(vm.tier_stats.leaf_body_inlines > 0);
            vm.depth = 255;
            assert!(vm.call(caller, &[b]).is_err());
            assert_eq!(vm.depth, 255);
            vm.depth = 0;
            vm.run_program(&mut crate::compile("assert b.x==500\n").unwrap())
                .unwrap();
            assert!(vm.stack.is_empty());
            assert!(vm.local_frames.is_empty());
            assert!(vm.local_slots.is_empty());
            assert!(vm.transient_roots.is_empty());
        }
    }

    #[test]
    fn copied_allocations_root_a_value_after_its_last_owner_is_cleared() {
        for tier in [
            crate::Tiering::Lir,
            crate::Tiering::Native,
            crate::Tiering::CopyPatch,
        ] {
            let mut vm = Vm::new();
            vm.tiering = tier;
            vm.thresholds.function_entries = 16;
            vm.next_collection = usize::MAX;
            vm.run_program(&mut crate::compile("class Box:\n    def __init__(self):\n        self.child=None\ndef leaf(b,x,y):\n    keep=b.child\n    number=x*y\n    b.child=None\n    garbage=[0]\n    return keep,number,garbage\ndef caller(b):\n    return leaf(b,2.0,3.0)\nb=Box()\nfor i in range(500):\n    b.child=[i]\n    caller(b)\n").unwrap()).unwrap();
            let caller = vm.get_global("caller").unwrap();
            let b = vm.get_global("b").unwrap();
            // Cross the collection floor, then collect at the first copied list allocation.
            for _ in 0..5000 {
                vm.heap.allocate(Object::Tuple(vec![]));
            }
            let child = vm.heap.allocate(Object::List(vec![Value::Int(123)].into()));
            vm.set_attribute(b, &"child".to_owned().into(), child)
                .unwrap();
            vm.next_collection = vm.heap.allocation_count() + 1;
            let epoch = vm.heap.collection_epoch();
            let hits = vm.tier_stats.leaf_body_inlines;
            let result = vm.call(caller, &[b]).unwrap();
            assert!(vm.tier_stats.leaf_body_inlines > hits);
            assert!(vm.heap.collection_epoch() > epoch);
            assert_eq!(vm.heap.get_item(result, Value::Int(0)).unwrap(), child);
            assert_eq!(
                vm.heap.get_item(child, Value::Int(0)).unwrap(),
                Value::Int(123)
            );
            assert_eq!(
                vm.heap.get_item(result, Value::Int(1)).unwrap(),
                Value::Float(6.0)
            );
            assert!(vm.stack.is_empty());
            assert!(vm.local_frames.is_empty());
            assert!(vm.local_slots.is_empty());
            assert!(vm.transient_roots.is_empty());
            vm.collect_garbage(&[]).unwrap();
            assert!(vm.heap.get(child).is_err());
        }
    }
    #[test]
    fn copied_float_phis_and_detached_references_survive_a_callee_collection() {
        for tier in [
            crate::Tiering::Lir,
            crate::Tiering::Native,
            crate::Tiering::CopyPatch,
        ] {
            let mut vm = Vm::new();
            vm.tiering = tier;
            vm.cfg_inlining = true;
            vm.thresholds.function_entries = 16;
            vm.next_collection = usize::MAX;
            vm.run_program(&mut crate::compile("class Box:\n    def __init__(self):\n        self.child=None\ndef leaf(b,flag,x):\n    keep=b.child\n    if flag:\n        v=x*2.0\n    else:\n        v=x/2.0\n    b.child=None\n    temporary=[v]\n    return keep,v,temporary\ndef caller(b,flag):\n    return leaf(b,flag,4.0)\nb=Box()\nfor i in range(500):\n    b.child=[i]\n    caller(b,i%2==0)\n").unwrap()).unwrap();
            let caller = vm.get_global("caller").unwrap();
            let b = vm.get_global("b").unwrap();
            for flag in [false, true] {
                for _ in 0..5000 {
                    vm.heap.allocate(Object::Tuple(vec![]));
                }
                let child = vm.heap.allocate(Object::List(vec![Value::Int(123)].into()));
                vm.set_attribute(b, &"child".to_owned().into(), child)
                    .unwrap();
                vm.next_collection = vm.heap.allocation_count() + 1;
                let epoch = vm.heap.collection_epoch();
                let hits = vm.tier_stats.leaf_body_inlines;
                let result = vm.call(caller, &[b, Value::Bool(flag)]).unwrap();
                assert!(vm.tier_stats.leaf_body_inlines > hits);
                assert!(vm.heap.collection_epoch() > epoch, "{tier:?}");
                assert_eq!(vm.heap.get_item(result, Value::Int(0)).unwrap(), child);
                assert_eq!(
                    vm.heap.get_item(child, Value::Int(0)).unwrap(),
                    Value::Int(123)
                );
                let expected = Value::Float(if flag { 8.0 } else { 2.0 });
                assert_eq!(vm.heap.get_item(result, Value::Int(1)).unwrap(), expected);
                let temporary = vm.heap.get_item(result, Value::Int(2)).unwrap();
                assert_eq!(
                    vm.heap.get_item(temporary, Value::Int(0)).unwrap(),
                    expected
                );
                assert!(vm.stack.is_empty());
                assert!(vm.local_frames.is_empty());
                assert!(vm.local_slots.is_empty());
                assert!(vm.transient_roots.is_empty());
                vm.collect_garbage(&[]).unwrap();
                assert!(vm.heap.get(child).is_err());
            }
        }
    }
}

#[cfg(test)]
mod mutation_region_tests {
    use super::*;

    #[test]
    fn a_detached_receiver_survives_collection_after_a_committed_prefix() {
        for bytecode in [crate::BytecodeMode::Stack, crate::BytecodeMode::Register] {
            let mut vm = Vm::new();
            vm.tiering = crate::Tiering::Native;
            vm.mutation_regions = true;
            vm.bytecode = bytecode;
            vm.thresholds.function_entries = 16;
            vm.next_collection = usize::MAX;
            vm.run_program(&mut crate::compile("class Inner:\n    def __init__(self):\n        self.append=1.0\nclass Box:\n    def __init__(self):\n        self.child=Inner()\n        self.result=0.0\n        self.count=0.0\n    def step(self):\n        saved=self.child\n        self.count+=1.0\n        self.child=0\n        self.result=saved.append\nbox=Box()\nbound=box.step\nfor i in range(500):\n    box.child=Inner()\n    bound()\n").unwrap()).unwrap();
            assert!(vm.tier_stats.mutation_entries > 0);
            let boxed = vm.get_global("box").unwrap();
            let bound = vm.get_global("bound").unwrap();
            for _ in 0..5000 {
                vm.heap.allocate(Object::Tuple(vec![]));
            }
            let child = vm.heap.allocate(Object::List(vec![Value::Int(123)].into()));
            vm.set_attribute(boxed, &"child".to_owned().into(), child)
                .unwrap();
            // Entry stays below the threshold. The failing list-method read allocates a bound
            // method and collects after the native prefix detached the only heap owner.
            vm.next_collection = vm.heap.allocation_count() + 1;
            let exits = vm.tier_stats.mutation_guard_exits;
            let epoch = vm.heap.collection_epoch();
            vm.call(bound, &[]).unwrap();
            assert_eq!(vm.tier_stats.mutation_guard_exits, exits + 1);
            assert!(vm.heap.collection_epoch() > epoch);
            vm.run_program(
                &mut crate::compile(
                    "assert box.count==501.0\nassert box.child==0\nbox.result(9)\n",
                )
                .unwrap(),
            )
            .unwrap();
            assert_eq!(vm.heap.len(child).unwrap(), 2);
            assert!(
                vm.stack.is_empty()
                    && vm.local_frames.is_empty()
                    && vm.local_slots.is_empty()
                    && vm.transient_roots.is_empty()
            );
            vm.run_program(&mut crate::compile("box.result=0\n").unwrap())
                .unwrap();
            vm.collect_garbage(&[]).unwrap();
            assert!(vm.heap.get(child).is_err());
        }
    }
}
