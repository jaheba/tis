//! Register bytecode uses the same HIR slots, operations, feedback and tier handoff as stack bytecode.
use crate::{
    Error, Object, Result, Value, Vm,
    bytecode::{Code, register::Opcode},
    hir::{CoreOp, Function, ValueId},
};
use std::{cell::RefCell, rc::Rc};

#[derive(Debug, Default)]
pub(crate) struct RegisterScratch {
    arguments: Vec<Value>,
    transfers: Vec<Value>,
    methods: Vec<(ValueId, Value, &'static str)>,
    backedges: Vec<u64>,
}

impl Vm {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn execute_register(
        &mut self,
        function: &Rc<RefCell<Function>>,
        code: &Code,
        base: usize,
        frame_end: usize,
        ip: usize,
        previous: Option<usize>,
    ) -> Result<Value> {
        let mut scratch = if let Some(scratch) = self.register_scratch.pop() {
            self.tier_stats.register_scratch_reuses += 1;
            scratch
        } else {
            self.tier_stats.register_scratch_creations += 1;
            RegisterScratch::default()
        };
        scratch
            .backedges
            .resize(*code.backedge_offsets.last().unwrap(), 0);
        let result = self.execute_register_inner(
            function,
            code,
            base,
            frame_end,
            ip,
            previous,
            &mut scratch,
        );
        if !scratch.backedges.is_empty() {
            code.flush_backedges(
                &mut function.borrow().profile.borrow_mut(),
                &mut scratch.backedges,
            );
        }
        scratch.backedges.clear();
        scratch.arguments.clear();
        scratch.transfers.clear();
        scratch.methods.clear();
        self.register_scratch.push(scratch);
        result
    }

    #[allow(clippy::too_many_arguments)]
    fn execute_register_inner(
        &mut self,
        function: &Rc<RefCell<Function>>,
        code: &Code,
        base: usize,
        frame_end: usize,
        mut ip: usize,
        mut previous: Option<usize>,
        scratch: &mut RegisterScratch,
    ) -> Result<Value> {
        let register = &code.register;
        let (mut collecting, mut quiet_window) = self.bytecode_phase(function);
        let RegisterScratch {
            arguments,
            transfers,
            methods,
            backedges,
        } = scratch;
        arguments.reserve(register.max_arguments.min(8));
        transfers.reserve(register.max_transfer);
        // Frame setup may allocate. Thereafter only allocating/reentrant instructions and
        // deferred method materialization need collection polls; pure dispatch cannot add pressure.
        self.maybe_collect()?;
        // Prepared receivers remain rooted in their caller's register window.
        loop {
            let instruction = *register
                .body
                .get(ip)
                .ok_or(Error::InvalidIr("register instruction pointer"))?;
            let instruction_index = ip;
            ip += 1;
            let a = instruction.a as usize;
            let b = instruction.b as usize;
            let c = instruction.c as usize;
            let op = instruction.op;
            match op {
                Opcode::EnterBlock => {
                    if !methods.is_empty() {
                        self.materialize_methods(base, methods)?;
                        self.maybe_collect()?;
                    }
                    if let Some(window) = quiet_window {
                        let sources = &code.backedge_sources[a];
                        let slot = previous
                            .and_then(|from| sources.iter().position(|source| *source == from));
                        // Pure blocks cannot reopen feedback. New hot headers retain the original
                        // admission path; completed headers accumulate exact frame-local counts.
                        if slot.is_none() || code.ready_windows[a].get() == Some(window) {
                            if let Some(slot) = slot {
                                let count = &mut backedges[code.backedge_offsets[a] + slot];
                                *count = count.saturating_add(1);
                            }
                            previous = Some(a);
                            continue;
                        }
                    }
                    let (backedge, phase) = self.record_block_and_phase_batched(
                        function,
                        code,
                        a,
                        previous,
                        Some(backedges),
                    )?;
                    collecting = phase;
                    quiet_window = self.bytecode_phase(function).1;
                    previous = Some(a);
                    if backedge
                        && let Some(lir) = self.artifact(code.function, code.revision)
                        && lir.osr_entries.iter().any(|entry| entry.header.0 == a)
                    {
                        self.tier_stats.osr_entries = self.tier_stats.osr_entries.saturating_add(1);
                        let outcome = self.execute_tier(
                            function,
                            code,
                            lir,
                            base,
                            frame_end,
                            crate::hir::BlockId(a),
                            true,
                        )?;
                        return self.finish_lir(function, code, base, frame_end, outcome);
                    }
                    continue;
                }
                Opcode::Jump | Opcode::Branch => {
                    let edge = if matches!(op, Opcode::Jump) {
                        a
                    } else {
                        let value = self.read(base, ValueId(a))?;
                        let truth = self.py_truth(value)?;
                        if matches!(value, Value::Ref(_)) {
                            self.maybe_collect()?;
                            (collecting, quiet_window) = self.bytecode_phase(function);
                        }
                        if truth { b } else { c }
                    };
                    let edge = register
                        .edges
                        .get(edge)
                        .ok_or(Error::InvalidIr("register edge"))?;
                    // Read every source before writing destinations, including swapped loop parameters.
                    transfers.clear();
                    for (_, source) in &edge.pairs {
                        transfers.push(self.read(base, *source)?);
                    }
                    for ((destination, _), value) in edge.pairs.iter().zip(transfers.iter()) {
                        self.write(base, *destination, *value)?;
                    }
                    ip = edge.target;
                    continue;
                }
                Opcode::Return => return self.read(base, ValueId(a)),
                _ => {}
            }
            if !collecting && let Some(bundle) = &register.unpack_bundles[instruction_index] {
                let source = self.read(base, bundle.source)?;
                let items = match self.heap.get(source) {
                    Ok(Object::Tuple(items)) if items.len() == bundle.items.len() => {
                        transfers.clear();
                        transfers.extend_from_slice(items);
                        true
                    }
                    Ok(Object::List(items)) if items.len() == bundle.items.len() => {
                        transfers.clear();
                        transfers.extend((0..items.len()).map(|i| items.value(i).unwrap()));
                        true
                    }
                    _ => false,
                };
                if items {
                    // The token is consumed only by these adjacent reads. No callback, collection
                    // or side exit can observe a list before its complete snapshot has been copied.
                    self.write(base, bundle.token, source)?;
                    for (&destination, &value) in bundle.items.iter().zip(transfers.iter()) {
                        self.write(base, destination, value)?;
                    }
                    ip += bundle.items.len();
                    continue;
                }
            }
            if !collecting && let Some(branch) = register.iterator_branches[instruction_index] {
                // Keep both HIR results and the original instruction offsets. Active feedback and
                // reconstruction after Next still use the unfused body; no iterator call is repeated.
                let iterator = self.read(base, ValueId(a))?;
                let (value, entered_runtime) = if let Some(value) = self.try_range_next(iterator)? {
                    (value, false)
                } else if let Some(value) = self.try_string_next(iterator)? {
                    (value, false)
                } else {
                    (self.next(iterator)?, true)
                };
                self.write(base, ValueId(instruction.dst as usize), value)?;
                if entered_runtime {
                    self.maybe_collect()?;
                    (collecting, quiet_window) = self.bytecode_phase(function);
                    if collecting {
                        continue;
                    }
                }
                self.tier_stats.bytecode_fused_iterations += 1;
                let done = self.is_done(value);
                self.write(base, branch.done, Value::Bool(done))?;
                let edge = &register.edges[if done { branch.yes } else { branch.no }];
                transfers.clear();
                for (_, source) in &edge.pairs {
                    transfers.push(self.read(base, *source)?);
                }
                for ((destination, _), value) in edge.pairs.iter().zip(transfers.iter()) {
                    self.write(base, *destination, *value)?;
                }
                ip = edge.target;
                continue;
            }
            let mut checkpoint = !matches!(
                op,
                Opcode::IsDone
                    | Opcode::LoadLocal
                    | Opcode::StoreLocal
                    | Opcode::CheckLocal
                    | Opcode::LoadName
                    | Opcode::StoreName
            );
            if collecting && !methods.is_empty() {
                self.materialize_methods(base, methods)?;
            }
            let destination = ValueId(instruction.dst as usize);
            let mut observed =
                collecting && self.observe_site(function, code, base, destination)?;
            let specialized = if self.numeric == crate::NumericMode::Adaptive {
                if let Some(plan) = register.numeric[instruction_index].get() {
                    let (left, right) = if matches!(op, Opcode::InplaceItem) {
                        let inputs = &code.sites[destination.0]
                            .as_ref()
                            .ok_or(Error::InvalidIr("item numeric inputs"))?
                            .inputs;
                        (self.read(base, inputs[2])?, self.read(base, inputs[3])?)
                    } else {
                        (self.read(base, ValueId(a))?, self.read(base, ValueId(b))?)
                    };
                    match plan.evaluate(left, right) {
                        crate::bytecode::numeric::Outcome::Value(value) => {
                            self.tier_stats.numeric_fast_paths =
                                self.tier_stats.numeric_fast_paths.saturating_add(1);
                            Some(value)
                        }
                        crate::bytecode::numeric::Outcome::TypeMiss => {
                            register.numeric[instruction_index].set(None);
                            self.tier_stats.numeric_type_misses =
                                self.tier_stats.numeric_type_misses.saturating_add(1);
                            self.numeric_miss(function, destination, left, right, observed);
                            (collecting, quiet_window) = self.bytecode_phase(function);
                            observed = true;
                            None
                        }
                        crate::bytecode::numeric::Outcome::ArithmeticMiss => {
                            self.tier_stats.numeric_arithmetic_fallbacks = self
                                .tier_stats
                                .numeric_arithmetic_fallbacks
                                .saturating_add(1);
                            None
                        }
                    }
                } else {
                    None
                }
            } else {
                None
            };
            let result = if let Some(value) = specialized {
                if matches!(op, Opcode::InplaceItem) {
                    let object = self.read(base, ValueId(a))?;
                    let key = self.read(base, ValueId(b))?;
                    if crate::sequence::set(&mut self.heap, object, key, value)? {
                        checkpoint = false;
                    } else {
                        self.py_set_item(object, key, value)?;
                    }
                    Value::None
                } else {
                    checkpoint = false;
                    value
                }
            } else {
                match op {
                    Opcode::Constant => match code
                        .constants
                        .get(a)
                        .ok_or(Error::InvalidIr("register constant"))?
                    {
                        crate::bytecode::EncodedConstant::Immediate(value) => {
                            checkpoint = false;
                            *value
                        }
                        crate::bytecode::EncodedConstant::String(value) => {
                            self.intern_literal(value, &code.literal_cache[a])
                        }
                    },
                    Opcode::Add => {
                        let lhs = self.read(base, ValueId(a))?;
                        let rhs = self.read(base, ValueId(b))?;
                        let (result, force) = self.adaptive_add(function, destination, lhs, rhs)?;
                        if force {
                            (collecting, quiet_window) = self.bytecode_phase(function);
                            observed = true;
                        }
                        result
                    }
                    Opcode::Subtract => self.py_binary(
                        "-",
                        self.read(base, ValueId(a))?,
                        self.read(base, ValueId(b))?,
                    )?,
                    Opcode::Less => self.py_compare(
                        "<",
                        self.read(base, ValueId(a))?,
                        self.read(base, ValueId(b))?,
                    )?,
                    Opcode::StringEqual => {
                        let left = self.read(base, ValueId(a))?;
                        let right = self.read(base, ValueId(b))?;
                        if let (Value::InlineString(left), Value::InlineString(right)) =
                            (left, right)
                        {
                            Value::Bool((left == right) != (c != 0))
                        } else {
                            self.py_compare(if c == 0 { "==" } else { "!=" }, left, right)?
                        }
                    }
                    Opcode::GetItem => {
                        let object = self.read(base, ValueId(a))?;
                        let key = if c == 0 {
                            self.read(base, ValueId(b))?
                        } else {
                            Value::Int(b as i64)
                        };
                        if let Some(value) = crate::sequence::get_builtin(&self.heap, object, key)?
                        {
                            checkpoint = false;
                            value
                        } else {
                            if c == 0 {
                                self.py_get_item(object, key)?
                            } else {
                                self.operation(&code.operations[c - 1].0, &[object])?
                            }
                        }
                    }
                    Opcode::SetItem => {
                        let (object, key, value) = (
                            self.read(base, ValueId(a))?,
                            self.read(base, ValueId(b))?,
                            self.read(base, ValueId(c))?,
                        );
                        if crate::sequence::set(&mut self.heap, object, key, value)? {
                            checkpoint = false;
                        } else {
                            self.py_set_item(object, key, value)?;
                        }
                        Value::None
                    }
                    Opcode::Len => Value::Int(self.py_len(self.read(base, ValueId(a))?)?),
                    Opcode::LoadName => {
                        let (crate::hir::CoreOp::LoadName(name), _) = &code.operations[a] else {
                            return Err(Error::InvalidIr("name load"));
                        };
                        self.load_name_slot(name, &register.names[c])?
                    }
                    Opcode::StoreName => {
                        let (crate::hir::CoreOp::StoreName(name), _) = &code.operations[a] else {
                            return Err(Error::InvalidIr("name store"));
                        };
                        self.store_name_slot(
                            name,
                            self.read(base, ValueId(b))?,
                            &register.names[c],
                        )?;
                        Value::None
                    }
                    Opcode::Iter => self.iter(self.read(base, ValueId(a))?)?,
                    Opcode::Next => {
                        let iterator = self.read(base, ValueId(a))?;
                        if let Some(value) = self.try_range_next(iterator)? {
                            checkpoint = false;
                            value
                        } else if let Some(value) = self.try_string_next(iterator)? {
                            checkpoint = false;
                            value
                        } else {
                            self.next(iterator)?
                        }
                    }
                    Opcode::IsDone => Value::Bool(self.is_done(self.read(base, ValueId(a))?)),
                    Opcode::LoadLocal => self.load_local(a)?,
                    Opcode::StoreLocal => {
                        self.store_local(a, self.read(base, ValueId(b))?)?;
                        Value::None
                    }
                    Opcode::CheckLocal => {
                        let value = self.read(base, ValueId(b))?;
                        self.check_local(a, value)?;
                        value
                    }
                    Opcode::GetAttr => {
                        let object = self.read(base, ValueId(b))?;
                        let field = &register.fields[a];
                        if let Some(value) =
                            self.try_plain_attribute(object, &field.text, Some(field))
                        {
                            checkpoint = false;
                            value
                        } else {
                            self.get_attribute(object, field)?
                        }
                    }
                    Opcode::SetAttr => {
                        let object = self.read(base, ValueId(b))?;
                        let field = &register.fields[a];
                        // Exception attributes may normalize an iterable through Python callbacks.
                        if !self.attribute_protocols
                            && matches!(self.heap.get(object), Ok(Object::Instance { .. }))
                            && !matches!(
                                field.text.as_str(),
                                "args"
                                    | "__cause__"
                                    | "__context__"
                                    | "__suppress_context__"
                                    | "__traceback__"
                                    | "__class__"
                            )
                        {
                            checkpoint = false;
                            self.raw_set_attribute(object, field, self.read(base, ValueId(c))?)?
                        } else {
                            self.set_attribute(object, field, self.read(base, ValueId(c))?)?
                        }
                    }
                    Opcode::PrepareMethod => {
                        let receiver = self.read(base, ValueId(a))?;
                        let (operation, _) = &code.operations[b];
                        let crate::hir::CoreOp::GetAttr(name) = operation else {
                            return Err(Error::InvalidIr("method lookup"));
                        };
                        if !collecting
                            && self.tiering == crate::Tiering::Bytecode
                            && let Some(name) = self.builtin_method(receiver, name)?
                        {
                            methods.push((destination, receiver, name));
                            receiver
                        } else {
                            self.get_attribute(receiver, &register.fields[c])?
                        }
                    }
                    Opcode::CallPositional => {
                        let inputs = &code.sites[destination.0]
                            .as_ref()
                            .ok_or(Error::InvalidIr("call inputs"))?
                            .inputs;
                        let (&callee, inputs) = inputs
                            .split_first()
                            .ok_or(Error::InvalidIr("call target"))?;
                        let callable = self.read(base, callee)?;
                        arguments.clear();
                        for &input in inputs {
                            arguments.push(self.read(base, input)?);
                        }
                        if let Some(index) = methods.iter().position(|(slot, _, _)| *slot == callee)
                        {
                            let (_, receiver, name) = methods.swap_remove(index);
                            self.call_prepared_method(receiver, name, arguments)?
                        } else {
                            self.call_from_registers(callable, arguments)?
                        }
                    }
                    Opcode::InplaceItem => {
                        let site = code
                            .sites
                            .get(destination.0)
                            .and_then(Option::as_ref)
                            .ok_or(Error::InvalidIr("item update inputs"))?;
                        let old = self.read(base, site.inputs[2])?;
                        let rhs = self.read(base, site.inputs[3])?;
                        let (CoreOp::InplaceItem(operator), _) = code
                            .operations
                            .get(c)
                            .ok_or(Error::InvalidIr("item update runtime pool"))?
                        else {
                            return Err(Error::InvalidIr("item update operation"));
                        };
                        let object = self.read(base, ValueId(a))?;
                        let key = self.read(base, ValueId(b))?;
                        if self.inplace_item(operator, object, key, old, rhs)? {
                            checkpoint = false;
                        }
                        Value::None
                    }
                    Opcode::Runtime
                    | Opcode::Numeric
                    | Opcode::List
                    | Opcode::Tuple
                    | Opcode::Dict => {
                        let site = code
                            .sites
                            .get(destination.0)
                            .and_then(Option::as_ref)
                            .ok_or(Error::InvalidIr("register inputs"))?;
                        arguments.clear();
                        for input in &site.inputs {
                            arguments.push(self.read(base, *input)?);
                        }
                        match op {
                            Opcode::List => {
                                self.heap.allocate(Object::List(arguments.clone().into()))
                            }
                            Opcode::Tuple => self.heap.allocate(Object::Tuple(arguments.clone())),
                            Opcode::Dict => self.build_dict(arguments)?,
                            _ => {
                                let (operation, count) = code
                                    .operations
                                    .get(if matches!(op, Opcode::Numeric) {
                                        site.runtime
                                            .ok_or(Error::InvalidIr("numeric runtime pool"))?
                                    } else {
                                        a
                                    })
                                    .ok_or(Error::InvalidIr("register runtime operation"))?;
                                if *count != arguments.len() {
                                    return Err(Error::InvalidIr("register runtime arity"));
                                }
                                if matches!(operation, CoreOp::Yield) {
                                    let value = arguments[0];
                                    self.materialize_methods(base, methods)?;
                                    self.yield_point = Some(crate::generator::YieldPoint {
                                        depth: self.depth,
                                        ip,
                                        previous,
                                        destination: Some(destination),
                                    });
                                    return Ok(value);
                                }
                                // Inputs remain in this register window across nested execution.
                                // Unlike stack bytecode, operand collection does not remove their roots.
                                self.operation(operation, arguments)?
                            }
                        }
                    }
                    Opcode::EnterBlock | Opcode::Jump | Opcode::Branch | Opcode::Return => {
                        unreachable!()
                    }
                }
            };
            self.write(base, destination, result)?;
            if observed {
                function.borrow().profile.borrow_mut().sites[destination.0]
                    .result
                    .observe(crate::profile::ValueKind::of(&self.heap, result));
            }
            if checkpoint {
                self.maybe_collect()?;
                (collecting, quiet_window) = self.bytecode_phase(function);
            }
        }
    }
}

impl Vm {
    fn materialize_methods(
        &mut self,
        base: usize,
        methods: &mut Vec<(ValueId, Value, &'static str)>,
    ) -> Result<()> {
        for (slot, receiver, name) in methods.drain(..) {
            let method = self.heap.allocate(Object::Bound(receiver, name.into()));
            self.write(base, slot, method)?;
        }
        Ok(())
    }

    fn numeric_miss(
        &mut self,
        function: &Rc<RefCell<Function>>,
        destination: ValueId,
        left: Value,
        right: Value,
        observed: bool,
    ) {
        let function = function.borrow();
        let mut profile = function.profile.borrow_mut();
        profile.start_window(self.profiling);
        let site = &mut profile.sites[destination.0];
        site.guard_failures = site.guard_failures.saturating_add(1);
        // A miss must record unexpected kinds even when the previous window exhausted its sample budget.
        if !observed {
            site.executions = site.executions.saturating_add(1);
            let offset = usize::from(matches!(
                function.instruction(destination).map(|i| &i.op),
                Some(crate::hir::Op::Runtime(CoreOp::InplaceItem(_), _))
            )) * 2;
            site.operands.resize_with(offset + 2, Default::default);
            site.operands[offset].observe(crate::profile::ValueKind::of(&self.heap, left));
            site.operands[offset + 1].observe(crate::profile::ValueKind::of(&self.heap, right));
        }
        self.requested
            .retain(|(id, revision, _)| *id != function.id() || *revision != function.revision());
    }
}
