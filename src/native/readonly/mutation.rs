//! Ordered primitive field updates stay private until native code returns.
use super::*;

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub(super) struct Write {
    pub receiver: Slot,
    pub slot: u64,
    pub value: Slot,
}

#[derive(Default)]
pub(super) struct Scratch {
    pub registers: Vec<Slot>,
    pub writes: Vec<Write>,
}

impl Plan {
    pub(super) fn build_mutation(source: &hir::Function) -> Result<Option<Self>> {
        if source.parameter_names.is_empty()
            || source.parameter_names.len() > 4
            || source.environment_required()
            || (source.local_states.is_empty() && !source.local_slots.is_empty())
            || source.blocks.len() > 48
            || source.value_count > 512
            || !source.keyword_only.is_empty()
            || source.vararg.is_some()
            || source.kwarg.is_some()
        {
            return Ok(None);
        }
        let lir = Rc::new(lir::Function::lower_with_inline(
            source,
            lir::LoweringMode::Generic,
            false,
            false,
        )?);
        fn visit(lir: &lir::Function, block: usize, colors: &mut [u8]) -> bool {
            if colors[block] == 1 {
                return false;
            }
            if colors[block] == 2 {
                return true;
            }
            colors[block] = 1;
            let edges = match &lir.blocks[block].terminator {
                Terminator::Return(_) => vec![],
                Terminator::Jump(edge) => vec![edge],
                Terminator::Branch { yes, no, .. } => vec![yes, no],
            };
            if !edges
                .into_iter()
                .all(|edge| visit(lir, edge.block.0, colors))
            {
                return false;
            }
            colors[block] = 2;
            true
        }
        if !visit(&lir, 0, &mut vec![0; lir.blocks.len()]) {
            return Ok(None);
        }
        for body in &lir.blocks {
            if let Terminator::Return(returned) = body.terminator
                && !lir.blocks.iter().flat_map(|b| &b.instructions).any(|i| {
                    i.result == Some(returned)
                        && matches!(i.operation, Operation::Constant(hir::Constant::None))
                })
            {
                return Ok(None);
            }
        }
        let mut fields = vec![];
        let mut write_capacity = 0;
        let mut reads = 0;
        for instruction in lir.blocks.iter().flat_map(|body| &body.instructions) {
            let Some(result) = instruction.result else {
                return Ok(None);
            };
            match &instruction.operation {
                Operation::Constant(
                    hir::Constant::Unbound
                    | hir::Constant::None
                    | hir::Constant::Int(_)
                    | hir::Constant::Float(_),
                ) => {}
                Operation::Truth { .. }
                | Operation::BoolNot(_)
                | Operation::BoxBool(_)
                | Operation::NumberCompare { .. } => {}
                Operation::Call { helper, .. } => match helper {
                    Helper::GetAttr(attribute) | Helper::SetAttr(attribute) => {
                        fields.push((result, attribute.clone()));
                        if matches!(helper, Helper::SetAttr(_)) {
                            // Write-only initialization has no existing field layout to reuse.
                            if write_capacity == 0 && reads == 0 {
                                return Ok(None);
                            }
                            write_capacity += 1;
                        } else {
                            reads += 1;
                        }
                    }
                    Helper::Add | Helper::Subtract | Helper::Less => {}
                    Helper::Python(
                        CoreOp::LoadLocal(_) | CoreOp::CheckLocal(_) | CoreOp::StoreLocal(_),
                    ) => {}
                    Helper::Python(CoreOp::Binary(op) | CoreOp::Inplace(op))
                        if ["+", "-", "*", "/"].contains(&op.as_str()) => {}
                    Helper::Python(CoreOp::Compare(op))
                        if ["==", "!=", "<", ">", "<=", ">=", "is", "is not"]
                            .contains(&op.as_str()) => {}
                    _ => return Ok(None),
                },
                _ => return Ok(None),
            }
        }
        if write_capacity == 0 {
            return Ok(None);
        }
        let parameters = source
            .parameter_names
            .iter()
            .map(|name| source.local_slots.iter().position(|n| n == name))
            .collect::<Option<Vec<_>>>();
        let Some(parameters) = parameters else {
            return Ok(None);
        };
        Ok(Some(Self {
            lir,
            globals: vec![],
            fields,
            tuples: BTreeMap::new(),
            recursive: BTreeMap::new(),
            methods: BTreeMap::new(),
            parameters,
            arity: 0,
            mutation: true,
            write_capacity,
        }))
    }
}

impl Kernel {
    pub(super) fn finish_mutation(
        &self,
        vm: &mut Vm,
        function: &Rc<RefCell<hir::Function>>,
        host: &Host,
        registers: &[Slot],
        writes: &[Write],
        status: u64,
    ) -> Result<Option<Outcome>> {
        // Cache learning changes no Python state. A cold guard before any write may replay entry.
        if let Some((_, attribute)) = self.fields.get(host.miss_field as usize)
            && let Ok(receiver) = host.receiver.value()
            && let Ok(crate::Object::Instance { attributes, .. }) = vm.heap.get(receiver)
        {
            attributes.read(attribute);
        }
        let count = host.write_count as usize;
        if count > writes.len() {
            return Err(Error::InvalidIr("native write count"));
        }
        if status != 0 && count == 0 {
            vm.tier_stats.mutation_cold_misses += 1;
            return Ok(None);
        }
        // No Python, collection, structural mutation or failure can intervene in this validated prefix.
        // Ordinary mutable heap guards refresh the weak directory after every committed object.
        let writes = &writes[..count];
        for (index, write) in writes.iter().enumerate() {
            let same = |other: &Write| {
                other.receiver.tag == write.receiver.tag && other.receiver.a == write.receiver.a
            };
            if writes[..index].iter().any(same) {
                continue;
            }
            let receiver = write.receiver.value()?;
            let mut object = vm.heap.get_mut(receiver)?;
            let crate::Object::Instance { attributes, .. } = &mut *object else {
                return Err(Error::InvalidIr("native write receiver"));
            };
            // No observer can see commits to independent objects; retain order within each object.
            for write in writes[index..].iter().filter(|other| same(other)) {
                attributes.write_existing(write.slot as usize, write.value.value()?)?;
            }
        }
        vm.tier_stats.mutation_writes += count as u64;
        if status == 0 {
            vm.tier_stats.mutation_entries += 1;
            return Ok(Some(Outcome::Return(Value::None)));
        }
        let state = lir::StateId(host.state as usize);
        let environment = *vm
            .environments
            .last()
            .ok_or(Error::InvalidIr("native write environment"))?;
        let frame = self
            .lir
            .reconstruct(state, environment, vm.local_values()?, |r| {
                registers[r.0].value()
            })?;
        // These exits need not retire the generic region: weak caches can learn new shapes, and
        // numeric tag guards already accept both int and float on subsequent invocations.
        vm.tier_stats.mutation_guard_exits += 1;
        vm.tier_stats.deoptimizations += 1;
        let reason = [
            lir::ExitReason::TypeGuard,
            lir::ExitReason::IntegerOverflow,
            lir::ExitReason::FloatArithmetic,
            lir::ExitReason::SequenceGuard,
        ]
        .get(host.exit_reason as usize)
        .copied()
        .ok_or(Error::InvalidIr("native mutation exit reason"))?;
        let hir = function.borrow();
        let mut profile = hir.profile.borrow_mut();
        profile.deoptimizations += 1;
        if let Some(instruction) = hir.blocks[frame.resume.block.0]
            .instructions
            .get(frame.resume.instruction)
        {
            let site = &mut profile.sites[instruction.result.0];
            site.deoptimizations += 1;
            site.guard_failures += 1;
            site.executions += 1;
            let inputs = instruction.op.inputs();
            site.operands.resize_with(inputs.len(), Default::default);
            for (operand, input) in site.operands.iter_mut().zip(inputs) {
                if let Some(Some(value)) = frame.slots.get(input.0) {
                    operand.observe(crate::profile::ValueKind::of(&vm.heap, *value));
                }
            }
            *site.exit_reasons.entry(reason).or_default() += 1;
        }
        Ok(Some(Outcome::Deopt(frame)))
    }
}
