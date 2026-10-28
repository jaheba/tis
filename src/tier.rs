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
}
#[derive(Debug, Clone, Default)]
pub struct TierStats {
    pub lowered: u64,
    pub compilation_errors: u64,
    pub lir_entries: u64,
    pub native_entries: u64,
    pub native_compilations: u64,
    pub native_code_bytes: u64,
    pub native_osr_entries: u64,
    pub osr_entries: u64,
    pub deoptimizations: u64,
    pub retired: u64,
}
#[derive(Debug, Clone, Copy)]
pub(crate) enum Datum {
    Boxed(Value),
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
            || lir.hir_values.iter().enumerate().any(|(i, r)| r.0 != i)
        {
            return Err(Error::InvalidIr("stale LIR artifact"));
        }
        lir.verify()?;
        for block in &lir.blocks {
            if block
                .parameters
                .iter()
                .any(|register| register.0 >= function.value_count)
            {
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

        self.native_artifacts.remove(&(lir.source, lir.revision));
        self.retired_versions.remove(&(lir.source, lir.revision));
        self.artifacts
            .insert((lir.source, lir.revision), Rc::new(lir));
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
        let mut registers = vec![None; lir.registers.len()];
        for (index, value) in self.stack[base..frame_end].iter().enumerate() {
            registers[index] = value.map(Datum::Boxed);
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
                let result = match &instruction.operation {
                    Operation::Constant(constant) => Some(Datum::Boxed(match constant {
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
                    Operation::FloatCompare {
                        operator,
                        left,
                        right,
                    } => {
                        let (left, right) = (
                            read(&registers, *left)?.float()?,
                            read(&registers, *right)?.float()?,
                        );
                        Some(Datum::Bool(match operator.as_str() {
                            "==" => left == right,
                            "!=" => left != right,
                            "<" => left < right,
                            "<=" => left <= right,
                            ">" => left > right,
                            ">=" => left >= right,
                            _ => return Err(Error::InvalidIr("float comparison")),
                        }))
                    }
                    Operation::BoxFloat(value) => Some(Datum::Boxed(Value::Float(
                        read(&registers, *value)?.float()?,
                    ))),
                    Operation::BoxBool(value) => Some(Datum::Boxed(Value::Bool(
                        read(&registers, *value)?.boolean()?,
                    ))),
                    Operation::UnboxInt(value) => {
                        let Datum::Boxed(Value::Int(value)) = read(&registers, *value)? else {
                            return Err(Error::InvalidIr("unguarded integer unbox"));
                        };
                        Some(Datum::Int(value))
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
                        ..
                    } => {
                        self.lir_roots(lir, *state, roots, &registers, base, &mut rooted)?;
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
                        ..
                    } => {
                        self.lir_roots(lir, *state, roots, &registers, base, &mut rooted)?;
                        let args = arguments
                            .iter()
                            .map(|&r| read(&registers, r)?.boxed())
                            .collect::<Result<Vec<_>>>()?;
                        self.observe_site(function, code, base, instruction.origin)?;
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
                            self.observe_site(function, code, base, instruction.origin)?;
                        }
                        if self.stack[base + destination.0].is_none() {
                            rooted.push(destination.0);
                        }
                        self.stack[base + destination.0] = Some(value);
                        let function = function.borrow();
                        let mut profile = function.profile.borrow_mut();
                        let site = &mut profile.sites[instruction.origin.0];
                        site.result
                            .observe(crate::profile::ValueKind::of(&self.heap, value));
                        if matches!(
                            instruction.operation,
                            Operation::BoxInt(_) | Operation::BoxFloat(_) | Operation::BoxBool(_)
                        ) {
                            site.fast_paths = site.fast_paths.saturating_add(1);
                        }
                    }
                }
                self.maybe_collect()?;
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
                if self.stack[base + parameter.0].is_none() {
                    rooted.push(parameter.0);
                }
                self.stack[base + parameter.0] = Some(value.boxed()?);
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
        for index in rooted.drain(..) {
            self.stack[base + index] = None;
        }
        for (value, register) in &state.values {
            if !roots.contains(register) {
                return Err(Error::InvalidIr("missing LIR root"));
            }
            rooted.push(value.0);
            self.stack[base + value.0] = Some(read(*register)?);
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
        let frame = lir.reconstruct(exit.state, environment, |r| {
            registers[r.0]
                .ok_or(Error::InvalidIr("missing deoptimization value"))?
                .boxed()
        })?;
        self.mark_deoptimization(function, &frame);
        Ok(Outcome::Deopt(frame))
    }
    pub(crate) fn mark_deoptimization(
        &mut self,
        function: &Rc<RefCell<Function>>,
        frame: &lir::DeoptFrame,
    ) {
        let hir = function.borrow();
        if self.artifacts.remove(&(hir.id(), hir.revision())).is_some() {
            self.tier_stats.retired = self.tier_stats.retired.saturating_add(1);
        }
        self.native_artifacts.remove(&(hir.id(), hir.revision()));
        self.retired_versions.insert((hir.id(), hir.revision()));
        self.tier_stats.deoptimizations = self.tier_stats.deoptimizations.saturating_add(1);
        let mut profile = hir.profile.borrow_mut();
        profile.deoptimizations = profile.deoptimizations.saturating_add(1);
        if let Some(instruction) = hir.blocks[frame.resume.block.0]
            .instructions
            .get(frame.resume.instruction)
        {
            let site = &mut profile.sites[instruction.result.0];
            site.deoptimizations = site.deoptimizations.saturating_add(1);
            site.guard_failures = site.guard_failures.saturating_add(1);
        }
    }
    pub(crate) fn lir_helper(&mut self, helper: &Helper, args: &[Value]) -> Result<Value> {
        Ok(match helper {
            Helper::StringConstant(text) => self.heap.allocate(Object::String(text.clone())),
            Helper::Add => runtime::add(&mut self.heap, args[0], args[1])?,
            Helper::Subtract => runtime::subtract(args[0], args[1])?,
            Helper::Less => runtime::less(args[0], args[1])?,
            Helper::List => self.heap.allocate(Object::List(args.to_vec())),
            Helper::Tuple => self.heap.allocate(Object::Tuple(args.to_vec())),
            Helper::Dict => {
                let dict = self.heap.allocate(Object::Dict(vec![]));
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
            Helper::Len => Value::Int(
                i64::try_from(self.heap.len(args[0])?).map_err(|_| Error::IntegerOverflow)?,
            ),
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
        if self.tiering == Tiering::Native {
            let key = (lir.source, lir.revision);
            let native = if let Some(native) = self.native_artifacts.get(&key) {
                native.clone()
            } else {
                let native = match crate::native::NativeCode::compile(lir.clone()) {
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
