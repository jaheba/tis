//! Executable LIR reference backend. It shares helper calls and frame roots with bytecode.
use crate::{
    Error, Object, Result, Value, Vm,
    bytecode::Code,
    hir::{self, BlockId, Function},
    lir::{self, Helper, Operation, Register, Terminator},
    profile::FunctionId,
    runtime,
};
use std::{cell::RefCell, rc::Rc};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Tiering {
    #[default]
    Bytecode,
    Lir,
    Native,
    CopyPatch,
}
#[derive(Debug, Clone, Default)]
pub struct TierStats {
    pub numeric_specializations: u64,
    pub numeric_fast_paths: u64,
    pub numeric_type_misses: u64,
    pub numeric_arithmetic_fallbacks: u64,
    pub lowered: u64,
    pub copy_patch_entries: u64,
    pub copy_patch_osr_entries: u64,
    pub copy_patch_compilations: u64,
    pub copy_patch_code_bytes: u64,
    pub copy_patch_data_bytes: u64,
    pub copy_patch_stencils: u64,
    pub copy_patch_relocations: u64,
    pub native_compile_ns: u64,
    pub copy_patch_compile_ns: u64,
    pub compilation_errors: u64,
    pub lir_entries: u64,
    pub native_entries: u64,
    pub native_compilations: u64,
    pub native_code_bytes: u64,
    pub native_osr_entries: u64,
    pub osr_entries: u64,
    pub deoptimizations: u64,
    pub retired: u64,
    pub native_feedback_probes: u64,
    pub native_root_syncs: u64,
    pub native_unpack_batches: u64,
    pub native_truth_fallbacks: u64,
    pub native_float_views: u64,
    pub native_float_view_failures: u64,
    pub native_region_entries: u64,
    pub native_region_iterations: u64,
    pub native_region_failures: u64,
    pub native_runtime_calls: u64,
    pub native_field_hits: u64,
    pub native_inline_field_hits: u64,
    pub native_inline_blocks: u64,
    pub native_body_inlines: u64,
    pub bytecode_positional_calls: u64,
    pub bytecode_leaf_calls: u64,
    pub bytecode_method_calls: u64,
    pub literal_hits: u64,
    pub interned_literals: u64,
    pub name_cache_hits: u64,
    pub shape_read_hits: u64,
    pub shape_write_hits: u64,
    pub positional_frames: u64,
    pub register_scratch_creations: u64,
    pub register_scratch_reuses: u64,
}
#[derive(Debug, Clone, Copy)]
pub(crate) enum Datum {
    Boxed(Value),
    FloatView(Value),
    Int(i64),
    Float(f64),
    Bool(bool),
}
impl Datum {
    fn boxed(self) -> Result<Value> {
        match self {
            Self::Boxed(value) => Ok(value),
            _ => Err(Error::InvalidIr("expected boxed LIR register")),
        }
    }
    fn int(self) -> Result<i64> {
        match self {
            Self::Int(value) => Ok(value),
            _ => Err(Error::InvalidIr("expected integer LIR register")),
        }
    }
    fn float(self) -> Result<f64> {
        match self {
            Self::Float(value) => Ok(value),
            _ => Err(Error::InvalidIr("expected float LIR register")),
        }
    }
    fn boolean(self) -> Result<bool> {
        match self {
            Self::Bool(value) => Ok(value),
            _ => Err(Error::InvalidIr("expected boolean LIR register")),
        }
    }
}
#[derive(Debug)]
pub(crate) enum Outcome {
    Return(Value),
    Deopt(lir::DeoptFrame),
}

impl Vm {
    /// Explicit compilation also permits recompilation after an artifact was retired by a guard failure.
    pub fn compile_lir(&mut self, function: &Function) -> Result<()> {
        let lir = function.lower_lir(lir::LoweringMode::Profiled)?;
        self.install_lir(function, lir)
    }
    pub fn install_lir(&mut self, function: &Function, lir: lir::Function) -> Result<()> {
        if lir.source != function.id()
            || lir.revision != function.revision()
            || lir.hir_values.len() != function.value_count
            || lir.local_count != function.local_slots.len()
            || lir.hir_values.iter().enumerate().any(|(i, r)| r.0 != i)
        {
            return Err(Error::InvalidIr("stale LIR artifact"));
        }
        lir.verify()?;
        for block in &lir.blocks {
            if block.parameters.iter().any(|register| {
                lir.registers[register.0] == lir::Type::Boxed && register.0 >= function.value_count
            }) {
                return Err(Error::InvalidIr("invalid boxed LIR parameter"));
            }
            for instruction in &block.instructions {
                if let Some(result) = instruction.result
                    && lir.registers[result.0] == lir::Type::Boxed
                    && (result.0 >= function.value_count || result.0 != instruction.origin.0)
                {
                    return Err(Error::InvalidIr("invalid boxed LIR result"));
                }
            }
        }

        self.copy_artifacts.remove(&(lir.source, lir.revision));
        self.native_artifacts.remove(&(lir.source, lir.revision));
        self.retired_versions.remove(&(lir.source, lir.revision));
        self.artifacts
            .insert((lir.source, lir.revision), Rc::new(lir));
        function.profile.borrow_mut().phase = crate::profile::ProfilePhase::Compiled;
        self.tier_stats.lowered = self.tier_stats.lowered.saturating_add(1);
        Ok(())
    }
    pub fn has_lir(&self, function: &Function) -> bool {
        self.artifacts
            .contains_key(&(function.id(), function.revision()))
    }
    pub(crate) fn artifact(&self, id: FunctionId, revision: u64) -> Option<Rc<lir::Function>> {
        if self.tiering != Tiering::Bytecode {
            self.artifacts.get(&(id, revision)).cloned()
        } else {
            None
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn execute_lir(
        &mut self,
        function: &Rc<RefCell<Function>>,
        code: &Code,
        lir: &lir::Function,
        base: usize,
        frame_end: usize,
        start: BlockId,
        mut previous: Option<usize>,
        skip_first_probe: bool,
    ) -> Result<Outcome> {
        // Retain a checkpoint for call-setup allocations even when the body cannot allocate.
        self.maybe_collect()?;
        let mut registers = vec![None; lir.registers.len()];
        for (index, value) in self.stack[base..frame_end].iter().enumerate() {
            registers[index] = value.map(Datum::Boxed);
        }
        if let Some(exit) = lir.scalar_entries.get(&start) {
            let state = &lir.states[lir.exits[exit.0].state.0];
            for (raw, source) in &lir.scalar_sources {
                if !state.values.iter().any(|(_, r)| r == source) {
                    continue;
                }
                let Some(Datum::Boxed(Value::Float(value))) = registers[source.0] else {
                    return self.lir_exit(function, lir, *exit, &registers);
                };
                registers[raw.0] = Some(Datum::Float(value));
            }
        }
        let read = |registers: &[Option<Datum>], r: Register| -> Result<Datum> {
            registers
                .get(r.0)
                .copied()
                .flatten()
                .ok_or(Error::InvalidIr("uninitialized LIR register"))
        };
        let mut rooted = self.stack[base..frame_end]
            .iter()
            .enumerate()
            .filter_map(|(index, value)| value.map(|_| index))
            .collect::<Vec<_>>();
        let mut block = start;
        let mut first = true;
        loop {
            if !first || !skip_first_probe {
                self.record_block(function, code, block.0, previous)?;
            }
            first = false;
            previous = Some(block.0);
            let body = lir
                .blocks
                .get(block.0)
                .ok_or(Error::InvalidIr("invalid LIR block"))?;
            for instruction in &body.instructions {
                let mut observed = false;
                let mut checkpoint = false;
                let result = match &instruction.operation {
                    Operation::FloatListView {
                        value,
                        length,
                        failure,
                    } => {
                        let source = read(&registers, *value)?.boxed()?;
                        let valid = if matches!(source, Value::Ref(_)) {
                            match &mut *self.heap.get_mut(source)? {
                                Object::List(v) => {
                                    length.is_none_or(|len| len == v.len())
                                        && v.float_view().is_some()
                                }
                                _ => false,
                            }
                        } else {
                            false
                        };
                        if !valid {
                            return self.lir_exit(function, lir, *failure, &registers);
                        }
                        Some(Datum::FloatView(source))
                    }
                    Operation::FloatListUnpack { value, .. } => Some(read(&registers, *value)?),
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
                        let Datum::FloatView(source) = read(&registers, *view)? else {
                            return Err(Error::InvalidIr("float view"));
                        };
                        let Object::List(list) = self.heap.get(source)? else {
                            return Err(Error::InvalidIr("float list"));
                        };
                        let index = if *index < 0 {
                            list.len() as i128 + i128::from(*index)
                        } else {
                            i128::from(*index)
                        };
                        let index = usize::try_from(index).ok().filter(|i| *i < list.len());
                        let Some(index) = index else {
                            return self.lir_exit(function, lir, *failure, &registers);
                        };
                        if let Operation::FloatListStore { value, .. } = &instruction.operation {
                            let value = read(&registers, *value)?.float()?;
                            self.heap.set_item(
                                source,
                                Value::Int(index as i64),
                                Value::Float(value),
                            )?;
                            Some(Datum::Boxed(Value::None))
                        } else {
                            Some(Datum::Boxed(list.value(index).ok_or(Error::Index)?))
                        }
                    }
                    Operation::SequenceUnpack {
                        value,
                        kind,
                        count,
                        failure,
                    } => {
                        let Some(value) = crate::sequence::unpack(
                            &self.heap,
                            read(&registers, *value)?.boxed()?,
                            *kind,
                            *count,
                        )?
                        else {
                            return self.lir_exit(function, lir, *failure, &registers);
                        };
                        Some(Datum::Boxed(value))
                    }
                    Operation::SequenceGet {
                        value,
                        kind,
                        index,
                        failure,
                    } => {
                        let index = match index {
                            lir::SequenceIndex::Constant(i) => Value::Int(*i),
                            lir::SequenceIndex::Value(r) => read(&registers, *r)?.boxed()?,
                        };
                        let Some(value) = crate::sequence::get(
                            &self.heap,
                            read(&registers, *value)?.boxed()?,
                            *kind,
                            index,
                        )?
                        else {
                            return self.lir_exit(function, lir, *failure, &registers);
                        };
                        Some(Datum::Boxed(value))
                    }
                    Operation::Constant(constant) => Some(Datum::Boxed(match constant {
                        hir::Constant::Unbound => Value::Unbound,
                        hir::Constant::None => Value::None,
                        hir::Constant::Bool(v) => Value::Bool(*v),
                        hir::Constant::Int(v) => Value::Int(*v),
                        hir::Constant::Float(v) => Value::Float(*v),
                        hir::Constant::String(_) => {
                            return Err(Error::InvalidIr(
                                "allocating LIR constant without safepoint",
                            ));
                        }
                    })),
                    Operation::GuardInt { value, exit } => {
                        if !matches!(read(&registers, *value)?, Datum::Boxed(Value::Int(_))) {
                            return self.lir_exit(function, lir, *exit, &registers);
                        }
                        None
                    }
                    Operation::GuardFloat { value, exit } => {
                        if !matches!(read(&registers, *value)?, Datum::Boxed(Value::Float(_))) {
                            return self.lir_exit(function, lir, *exit, &registers);
                        }
                        None
                    }
                    Operation::UnboxFloat(value) => {
                        let Datum::Boxed(Value::Float(value)) = read(&registers, *value)? else {
                            return Err(Error::InvalidIr("unguarded float unbox"));
                        };
                        Some(Datum::Float(value))
                    }
                    Operation::IntToFloat(value) => {
                        Some(Datum::Float(read(&registers, *value)?.int()? as f64))
                    }
                    Operation::FloatBinary {
                        operator,
                        left,
                        right,
                        failure,
                    } => {
                        let Some(result) = operator.evaluate(
                            read(&registers, *left)?.float()?,
                            read(&registers, *right)?.float()?,
                        ) else {
                            return self.lir_exit(function, lir, *failure, &registers);
                        };
                        Some(Datum::Float(result))
                    }
                    Operation::NumberCompare {
                        value,
                        integer,
                        operator,
                        failure,
                    } => {
                        let Some(value) = runtime::number_compare(
                            read(&registers, *value)?.boxed()?,
                            *integer,
                            operator,
                        ) else {
                            return self.lir_exit(function, lir, *failure, &registers);
                        };
                        Some(Datum::Bool(value))
                    }
                    Operation::FloatCompare {
                        operator,
                        left,
                        right,
                    } => {
                        let (left, right) = (
                            read(&registers, *left)?.float()?,
                            read(&registers, *right)?.float()?,
                        );
                        Some(Datum::Bool(
                            crate::scalar::FloatCondition::from_symbol(operator)
                                .ok_or(Error::InvalidIr("float comparison"))?
                                .evaluate(left, right),
                        ))
                    }
                    Operation::BoxFloat(value) => Some(Datum::Boxed(Value::Float(
                        read(&registers, *value)?.float()?,
                    ))),
                    Operation::BoolNot(value) => {
                        Some(Datum::Bool(!read(&registers, *value)?.boolean()?))
                    }
                    Operation::BoxBool(value) => Some(Datum::Boxed(Value::Bool(
                        read(&registers, *value)?.boolean()?,
                    ))),
                    Operation::UnboxInt(value) => {
                        let Datum::Boxed(Value::Int(value)) = read(&registers, *value)? else {
                            return Err(Error::InvalidIr("unguarded integer unbox"));
                        };
                        Some(Datum::Int(value))
                    }
                    Operation::NumericAdd {
                        left,
                        right,
                        guard,
                        overflow,
                    } => {
                        match runtime::numeric_add(
                            read(&registers, *left)?.boxed()?,
                            read(&registers, *right)?.boxed()?,
                        ) {
                            Some(Ok(value)) => Some(Datum::Boxed(value)),
                            miss => {
                                return self.lir_exit(
                                    function,
                                    lir,
                                    if miss.is_none() { *guard } else { *overflow },
                                    &registers,
                                );
                            }
                        }
                    }
                    Operation::CheckedAdd {
                        left,
                        right,
                        overflow,
                    } => {
                        let Some(value) = read(&registers, *left)?
                            .int()?
                            .checked_add(read(&registers, *right)?.int()?)
                        else {
                            return self.lir_exit(function, lir, *overflow, &registers);
                        };
                        Some(Datum::Int(value))
                    }
                    Operation::BoxInt(value) => {
                        Some(Datum::Boxed(Value::Int(read(&registers, *value)?.int()?)))
                    }
                    Operation::Truth {
                        value,
                        state,
                        roots,
                        effects,
                    } => {
                        checkpoint = effects.needs_safepoint();
                        if checkpoint {
                            self.lir_roots(lir, *state, roots, &registers, base, &mut rooted)?;
                        }
                        Some(Datum::Bool(runtime::truth(
                            &self.heap,
                            read(&registers, *value)?.boxed()?,
                        )?))
                    }
                    Operation::Call {
                        helper,
                        arguments,
                        state,
                        roots,
                        effects,
                    } => {
                        checkpoint = effects.needs_safepoint();
                        if checkpoint {
                            self.lir_roots(lir, *state, roots, &registers, base, &mut rooted)?;
                        }
                        let args = arguments
                            .iter()
                            .map(|&r| read(&registers, r)?.boxed())
                            .collect::<Result<Vec<_>>>()?;
                        observed =
                            self.observe_site_with(function, code, instruction.origin, |v| {
                                read(&registers, Register(v.0))?.boxed()
                            })?;
                        // Boxed operands remain rooted throughout nested calls; helpers return before the next collection.
                        let roots = self.transient_roots.len();
                        self.transient_roots.extend_from_slice(&args);
                        let result = self.lir_helper(helper, &args);
                        self.transient_roots.truncate(roots);
                        Some(Datum::Boxed(result?))
                    }
                };
                if let Some(result) = result {
                    let destination = instruction
                        .result
                        .ok_or(Error::InvalidIr("missing LIR result"))?;
                    *registers
                        .get_mut(destination.0)
                        .ok_or(Error::InvalidIr("invalid LIR register"))? = Some(result);
                    if let Datum::Boxed(value) = result {
                        if !matches!(instruction.operation, Operation::Call { .. }) {
                            observed =
                                self.observe_site_with(function, code, instruction.origin, |v| {
                                    read(&registers, Register(v.0))?.boxed()
                                })?;
                        }
                        if self.stack[base + destination.0].is_none() {
                            rooted.push(destination.0);
                        }
                        self.stack[base + destination.0] = Some(value);
                        let function = function.borrow();
                        let mut profile = function.profile.borrow_mut();
                        let site = &mut profile.sites[instruction.origin.0];
                        if observed {
                            site.result
                                .observe(crate::profile::ValueKind::of(&self.heap, value));
                        }
                        if matches!(
                            instruction.operation,
                            Operation::BoxInt(_) | Operation::BoxFloat(_) | Operation::BoxBool(_)
                        ) {
                            site.fast_paths = site.fast_paths.saturating_add(1);
                        }
                    }
                }
                if checkpoint {
                    self.maybe_collect()?;
                }
            }
            let edge = match &body.terminator {
                Terminator::Return(value) => {
                    return Ok(Outcome::Return(read(&registers, *value)?.boxed()?));
                }
                Terminator::Jump(edge) => edge,
                Terminator::Branch { condition, yes, no } => {
                    if read(&registers, *condition)?.boolean()? {
                        yes
                    } else {
                        no
                    }
                }
            };
            let args = edge
                .arguments
                .iter()
                .map(|&r| read(&registers, r))
                .collect::<Result<Vec<_>>>()?;
            let parameters = &lir.blocks[edge.block.0].parameters;
            for (&parameter, value) in parameters.iter().zip(args) {
                registers[parameter.0] = Some(value);
                if lir.registers[parameter.0] == lir::Type::Boxed {
                    if self.stack[base + parameter.0].is_none() {
                        rooted.push(parameter.0);
                    }
                    self.stack[base + parameter.0] = Some(value.boxed()?);
                }
            }
            block = edge.block;
        }
    }

    fn lir_roots(
        &mut self,
        lir: &lir::Function,
        state: lir::StateId,
        roots: &[Register],
        registers: &[Option<Datum>],
        base: usize,
        rooted: &mut Vec<usize>,
    ) -> Result<()> {
        let state = lir
            .states
            .get(state.0)
            .ok_or(Error::InvalidIr("invalid LIR safepoint"))?;
        self.sync_lir_roots(state, roots, base, rooted, |register| {
            registers[register.0]
                .ok_or(Error::InvalidIr("missing safepoint value"))?
                .boxed()
        })
    }
    pub(crate) fn sync_lir_roots(
        &mut self,
        state: &lir::FrameState,
        roots: &[Register],
        base: usize,
        rooted: &mut Vec<usize>,
        mut read: impl FnMut(Register) -> Result<Value>,
    ) -> Result<()> {
        // Installation verifies the immutable root map; do not repeat a quadratic membership scan at each helper.
        debug_assert!(
            state
                .values
                .iter()
                .all(|(_, register)| roots.contains(register))
        );
        for index in rooted.drain(..) {
            self.stack[base + index] = None;
        }
        for (value, register) in &state.values {
            let root = read(*register)?;
            // Scalars stay in the register image for reconstruction; only heap references need GC publication.
            if matches!(root, Value::Ref(_)) {
                rooted.push(value.0);
                self.stack[base + value.0] = Some(root);
            }
        }
        Ok(())
    }
    fn lir_exit(
        &mut self,
        function: &Rc<RefCell<Function>>,
        lir: &lir::Function,
        exit: lir::ExitId,
        registers: &[Option<Datum>],
    ) -> Result<Outcome> {
        let exit = &lir.exits[exit.0];
        let environment = *self
            .environments
            .last()
            .ok_or(Error::InvalidIr("missing LIR environment"))?;
        let frame = lir.reconstruct(exit.state, environment, self.local_values()?, |r| {
            registers[r.0]
                .ok_or(Error::InvalidIr("missing deoptimization value"))?
                .boxed()
        })?;
        self.mark_deoptimization(function, &frame, exit.reason);
        Ok(Outcome::Deopt(frame))
    }
    pub(crate) fn mark_deoptimization(
        &mut self,
        function: &Rc<RefCell<Function>>,
        frame: &lir::DeoptFrame,
        reason: lir::ExitReason,
    ) {
        let hir = function.borrow();
        if self.artifacts.remove(&(hir.id(), hir.revision())).is_some() {
            self.tier_stats.retired = self.tier_stats.retired.saturating_add(1);
        }
        self.copy_artifacts.remove(&(hir.id(), hir.revision()));
        self.native_artifacts.remove(&(hir.id(), hir.revision()));
        self.retired_versions.insert((hir.id(), hir.revision()));
        self.tier_stats.deoptimizations = self.tier_stats.deoptimizations.saturating_add(1);
        let mut profile = hir.profile.borrow_mut();
        profile.deoptimizations = profile.deoptimizations.saturating_add(1);
        if self.profiling.mode == crate::profile::ProfileMode::Staged {
            profile.start_window(self.profiling);
            self.requested
                .retain(|(id, revision, _)| *id != hir.id() || *revision != hir.revision());
        } else {
            profile.phase = crate::profile::ProfilePhase::Retired;
        }
        if let Some(instruction) = hir.blocks[frame.resume.block.0]
            .instructions
            .get(frame.resume.instruction)
        {
            let site = &mut profile.sites[instruction.result.0];
            site.deoptimizations = site.deoptimizations.saturating_add(1);
            site.guard_failures = site.guard_failures.saturating_add(1);
            let count = site.exit_reasons.entry(reason).or_default();
            *count = count.saturating_add(1);
            // Guard misses bypass sampling so new types are retained even after a window closes.
            site.executions = site.executions.saturating_add(1);
            let inputs = instruction.op.inputs();
            site.operands.resize_with(inputs.len(), Default::default);
            for (operand, input) in site.operands.iter_mut().zip(inputs) {
                if let Some(Some(value)) = frame.slots.get(input.0) {
                    operand.observe(crate::profile::ValueKind::of(&self.heap, *value));
                }
            }
        }
    }
    pub(crate) fn lir_helper(&mut self, helper: &Helper, args: &[Value]) -> Result<Value> {
        Ok(match helper {
            Helper::InlineCall { .. } => self.call_from_registers(args[0], &args[1..])?,
            Helper::GetAttr(attribute) => self.get_attribute(args[0], attribute)?,
            Helper::SetAttr(attribute) => self.set_attribute(args[0], attribute, args[1])?,
            Helper::LoadName(name) => self.load_name_slot(&name.text, &name.cache)?,
            Helper::StoreName(name) => {
                self.store_name_slot(&name.text, args[0], &name.cache)?;
                Value::None
            }
            Helper::StringConstant(literal) => self.intern_literal(&literal.text, &literal.cache),
            Helper::Add => runtime::add(&mut self.heap, args[0], args[1])?,
            Helper::Subtract => runtime::subtract(args[0], args[1])?,
            Helper::Less => runtime::less(args[0], args[1])?,
            Helper::List => self.heap.allocate(Object::List(args.to_vec().into())),
            Helper::Tuple => self.heap.allocate(Object::Tuple(args.to_vec())),
            Helper::Dict => {
                let dict = self.heap.allocate(Object::Dict(Default::default()));
                for pair in args.as_chunks::<2>().0 {
                    self.heap.set_item(dict, pair[0], pair[1])?;
                }
                dict
            }
            Helper::GetItem => self.heap.get_item(args[0], args[1])?,
            Helper::SetItem => {
                self.heap.set_item(args[0], args[1], args[2])?;
                Value::None
            }
            Helper::ScalarCall(_) => self.call_from_registers(args[0], &args[1..])?,
            Helper::Len => Value::Int(
                i64::try_from(self.heap.len(args[0])?).map_err(|_| Error::IntegerOverflow)?,
            ),
            Helper::Python(crate::hir::CoreOp::Call(names))
                if names.iter().all(Option::is_none) =>
            {
                // LIR and native gateways root every operand throughout reentrant calls.
                // Plain positional calls can share the register path; keywords/starred calls
                // still require the general binder and its evaluation/duplicate checks.
                self.call_from_registers(args[0], &args[1..])?
            }
            Helper::Python(operation) => self.operation(operation, args)?,
        })
    }
}

impl Vm {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn execute_tier(
        &mut self,
        function: &Rc<RefCell<Function>>,
        code: &Code,
        lir: Rc<lir::Function>,
        base: usize,
        frame_end: usize,
        start: BlockId,
        osr: bool,
    ) -> Result<Outcome> {
        if self.tiering == Tiering::CopyPatch {
            let key = (lir.source, lir.revision);
            let feedback = self.profiling.mode == crate::profile::ProfileMode::Eager;
            let compiled = if let Some(compiled) = self.copy_artifacts.get(&key)
                && (!feedback || compiled.feedback)
            {
                compiled.clone()
            } else {
                let start = std::time::Instant::now();
                let result = crate::copy_patch::CopyCode::compile(lir.clone(), feedback);
                self.tier_stats.copy_patch_compile_ns = self
                    .tier_stats
                    .copy_patch_compile_ns
                    .saturating_add(start.elapsed().as_nanos().min(u64::MAX as u128) as u64);
                let compiled = match result {
                    Ok(compiled) => Rc::new(compiled),
                    Err(error) => {
                        self.tier_stats.compilation_errors =
                            self.tier_stats.compilation_errors.saturating_add(1);
                        return Err(error);
                    }
                };
                self.tier_stats.copy_patch_compilations =
                    self.tier_stats.copy_patch_compilations.saturating_add(1);
                self.tier_stats.copy_patch_code_bytes = self
                    .tier_stats
                    .copy_patch_code_bytes
                    .saturating_add(compiled.code_bytes as u64);
                self.tier_stats.copy_patch_data_bytes = self
                    .tier_stats
                    .copy_patch_data_bytes
                    .saturating_add(compiled.data_bytes as u64);
                self.tier_stats.copy_patch_stencils = self
                    .tier_stats
                    .copy_patch_stencils
                    .saturating_add(compiled.stencils as u64);
                self.tier_stats.copy_patch_relocations = self
                    .tier_stats
                    .copy_patch_relocations
                    .saturating_add(compiled.relocations as u64);
                self.copy_artifacts.insert(key, compiled.clone());
                compiled
            };
            if osr {
                self.tier_stats.copy_patch_osr_entries =
                    self.tier_stats.copy_patch_osr_entries.saturating_add(1);
            } else {
                self.tier_stats.copy_patch_entries =
                    self.tier_stats.copy_patch_entries.saturating_add(1);
            }
            compiled.execute(self, function, code, base, frame_end, start.0, osr)
        } else if self.tiering == Tiering::Native {
            let key = (lir.source, lir.revision);
            let feedback = self.profiling.mode == crate::profile::ProfileMode::Eager;
            let want_nest = !self.disable_native_nests
                && !feedback
                && lir
                    .nest
                    .as_ref()
                    .is_some_and(|n| !n.blocks.contains(&start) || n.header == start);
            let native = if let Some(native) = self.native_artifacts.get(&key)
                && (!feedback || native.feedback)
                && native.nest.is_some() == want_nest
            {
                native.clone()
            } else {
                let start = std::time::Instant::now();
                let result = crate::native::NativeCode::compile(
                    lir.clone(),
                    feedback,
                    self.capture_native_code,
                    want_nest,
                );
                self.tier_stats.native_compile_ns = self
                    .tier_stats
                    .native_compile_ns
                    .saturating_add(start.elapsed().as_nanos().min(u64::MAX as u128) as u64);
                let native = match result {
                    Ok(native) => Rc::new(native),
                    Err(error) => {
                        self.tier_stats.compilation_errors =
                            self.tier_stats.compilation_errors.saturating_add(1);
                        return Err(error);
                    }
                };
                self.tier_stats.native_compilations =
                    self.tier_stats.native_compilations.saturating_add(1);
                self.tier_stats.native_code_bytes = self
                    .tier_stats
                    .native_code_bytes
                    .saturating_add(native.bytes as u64);
                self.native_artifacts.insert(key, native.clone());
                native
            };
            if osr {
                self.tier_stats.native_osr_entries =
                    self.tier_stats.native_osr_entries.saturating_add(1);
            } else {
                self.tier_stats.native_entries = self.tier_stats.native_entries.saturating_add(1);
            }
            native.execute(self, function, code, base, frame_end, start.0, osr)
        } else {
            if !osr {
                self.tier_stats.lir_entries = self.tier_stats.lir_entries.saturating_add(1);
            }
            self.execute_lir(function, code, &lir, base, frame_end, start, None, osr)
        }
    }
}

impl Vm {
    pub fn native_diagnostics(&self, callable: Value) -> Result<&crate::NativeDiagnostics> {
        let f = self.function_hir(callable)?;
        self.native_artifacts
            .get(&(f.id(), f.revision()))
            .and_then(|code| code.diagnostics.as_ref())
            .ok_or(Error::InvalidIr("native diagnostics were not captured"))
    }
}
