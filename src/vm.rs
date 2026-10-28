use crate::{
    Error, Heap, Object, Result, Value,
    bytecode::{Code, Opcode, read_operand},
    hir::{AddFeedback, AddStrategy, Constant, Function, ValueId},
    runtime,
};
use std::{cell::RefCell, rc::Rc};

#[derive(Debug)]
pub(crate) struct LocalFrame {
    values: Vec<Option<Value>>,
    code: Rc<Code>,
}

#[derive(Debug, Default)]
pub struct Vm {
    pub heap: Heap,
    pub(crate) stack: Vec<Option<Value>>,
    pub tiering: crate::Tiering,
    pub bytecode: crate::BytecodeMode,
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
    pub(crate) globals: Option<Value>,
    pub output: String,
    pub(crate) depth: usize,
    pub(crate) transient_roots: Vec<Value>,
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
    pub fn new() -> Self {
        Self::default()
    }

    /// Frame values occupy one stack window; both return and error release that window.
    pub fn run(&mut self, function: &mut Function, arguments: &[Value]) -> Result<Value> {
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
        if self.depth >= 256 {
            return Err(Error::Runtime(
                "RecursionError: maximum recursion depth exceeded".into(),
            ));
        }
        let code = function.borrow_mut().shared_bytecode()?;
        {
            let function = function.borrow();
            let mut profile = function.profile.borrow_mut();
            profile.layout(function.blocks.len(), function.value_count);
            profile.entries = profile.entries.saturating_add(1);
        }
        self.consider_compilation(&function, crate::profile::Hotspot::Function);
        let collecting = self.collecting(&function);
        if collecting {
            let values = self.bound_arguments(&function.borrow(), environment, arguments)?;
            let hir = function.borrow();
            let mut profile = hir.profile.borrow_mut();
            profile
                .arguments
                .resize_with(values.len(), Default::default);
            for (profile, value) in profile.arguments.iter_mut().zip(values) {
                profile.observe(crate::profile::ValueKind::of(&self.heap, value));
            }
        }
        let parameters = function
            .borrow()
            .parameters(function.borrow().entry())
            .to_vec();
        let mut slots = vec![None; code.slots];
        for (&parameter, &value) in parameters.iter().zip(arguments) {
            slots[parameter.0] = Some(value);
        }
        let locals = {
            let mut object = self.heap.get_mut(environment)?;
            let Object::Environment(env) = &mut *object else {
                return Err(Error::InvalidIr("frame environment"));
            };
            code.local_names
                .iter()
                .map(|name| env.values.remove(name))
                .collect()
        };
        self.execute_window(function, code, environment, slots, locals, 0, None)
    }

    #[allow(clippy::too_many_arguments)]
    fn execute_window(
        &mut self,
        function: Rc<RefCell<Function>>,
        code: Rc<Code>,
        environment: Value,
        slots: Vec<Option<Value>>,
        locals: Vec<Option<Value>>,
        ip: usize,
        block: Option<usize>,
    ) -> Result<Value> {
        if self.depth >= 256 {
            return Err(Error::Runtime(
                "RecursionError: maximum recursion depth exceeded".into(),
            ));
        }
        let base = self.stack.len();
        let frame_end = base + slots.len();
        self.stack.extend(slots);
        self.depth += 1;
        self.environments.push(environment);
        self.local_frames.push(LocalFrame {
            values: locals,
            code: code.clone(),
        });
        // Runtime calls currently recurse through Rust; grow before entering another frame
        // so the language recursion limit is reached before exhausting the native stack.
        let result = stacker::maybe_grow(128 * 1024, 2 * 1024 * 1024, || {
            if ip == 0
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
        self.depth -= 1;
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
            crate::tier::Outcome::Deopt(frame) => {
                let ip = code.resume_in(self.bytecode, frame.resume)?;
                self.stack[base..frame_end].copy_from_slice(&frame.slots);
                self.local_frames
                    .last_mut()
                    .ok_or(Error::InvalidIr("local frame"))?
                    .values = frame.locals;
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
            frame.slots,
            frame.locals,
            ip,
            Some(frame.resume.block.0),
        );
        *function = handle.borrow().clone();
        result
    }

    pub fn collect_garbage(&mut self, external_roots: &[Value]) -> Result<usize> {
        let roots = self
            .stack
            .iter()
            .filter_map(|value| *value)
            .chain(self.environments.iter().copied())
            .chain(
                self.local_frames
                    .iter()
                    .flat_map(|frame| frame.values.iter().filter_map(|v| *v)),
            )
            .chain(self.transient_roots.iter().copied())
            .chain(self.pinned.values().copied())
            .chain(self.globals)
            .chain(external_roots.iter().copied())
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

    fn consider_compilation(
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
            match function.lower_lir(crate::lir::LoweringMode::Profiled) {
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
                    let is_backedge = self.record_block(function, code, block, current_block)?;
                    collecting = self.collecting(function);
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
                        Constant::Unbound => Value::Unbound,
                        Constant::None => Value::None,
                        Constant::Bool(value) => Value::Bool(*value),
                        Constant::Int(value) => Value::Int(*value),
                        Constant::Float(value) => Value::Float(*value),
                        Constant::String(value) => {
                            self.heap.allocate(Object::String(value.clone()))
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
                        Opcode::List => self.heap.allocate(Object::List(values)),
                        Opcode::Tuple => self.heap.allocate(Object::Tuple(values)),
                        _ => {
                            let result = self.heap.allocate(Object::Dict(Vec::new()));
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
        Ok(&self
            .local_frames
            .last()
            .ok_or(Error::InvalidIr("local frame"))?
            .values)
    }

    pub(crate) fn load_local(&self, slot: usize) -> Result<Value> {
        let frame = self
            .local_frames
            .last()
            .ok_or(Error::InvalidIr("local frame"))?;
        frame
            .values
            .get(slot)
            .ok_or(Error::InvalidIr("local slot"))?
            .ok_or_else(|| {
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
            .last_mut()
            .ok_or(Error::InvalidIr("local frame"))?;
        *frame
            .values
            .get_mut(slot)
            .ok_or(Error::InvalidIr("local slot"))? = Some(value);
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
        let backedge = previous.is_some_and(|from| code.backedges.contains(&(from, block)));
        {
            let function = function.borrow();
            let mut profile = function.profile.borrow_mut();
            if self.profiling.collecting(profile.phase) {
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
        }
        Ok(backedge)
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
        let function = function.borrow();
        let mut profile = function.profile.borrow_mut();
        let site = &mut profile.sites[value.0];
        site.operands
            .resize_with(metadata.inputs.len(), Default::default);
        for (operand, &input) in site.operands.iter_mut().zip(&metadata.inputs) {
            operand.observe(crate::profile::ValueKind::of(&self.heap, read(input)?));
        }
        if let Some(target) = target {
            site.call(target);
        }
        Ok(true)
    }
    pub(crate) fn maybe_collect(&mut self) -> Result<()> {
        if self.heap.allocation_count() >= self.next_collection.max(4096) {
            self.collect_garbage(&[])?;
            self.next_collection = self.heap.allocation_count().saturating_mul(2).max(4096);
        }
        Ok(())
    }
}
