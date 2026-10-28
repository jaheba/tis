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
            Value::Unbound => Self {
                tag: UNBOUND,
                a: 0,
                b: 0,
                c: 0,
            },
            Value::None => Self {
                tag: NONE,
                a: 0,
                b: 0,
                c: 0,
            },
            Value::Bool(v) => Self {
                tag: BOOL,
                a: v as u64,
                b: 0,
                c: 0,
            },
            Value::Int(v) => Self {
                tag: INT,
                a: v as u64,
                b: 0,
                c: 0,
            },
            Value::Float(v) => Self {
                tag: FLOAT,
                a: v.to_bits(),
                b: 0,
                c: 0,
            },
            Value::Ref(id) => Self {
                tag: REF,
                a: id.heap as u64,
                b: id.index as u64,
                c: id.generation as u64,
            },
        }
    }
    pub(crate) fn value(self) -> Result<Value> {
        Ok(match self.tag {
            UNBOUND => Value::Unbound,
            NONE => Value::None,
            BOOL => Value::Bool(self.a != 0),
            INT => Value::Int(self.a as i64),
            FLOAT => Value::Float(f64::from_bits(self.a)),
            REF => Value::Ref(ObjectId {
                heap: self.a as usize,
                index: self.b as usize,
                generation: self.c as usize,
            }),
            _ => return Err(Error::InvalidIr("native register is not boxed")),
        })
    }
}
pub(crate) type Entry = unsafe extern "C" fn(*mut c_void, *mut Slot, u32) -> u64;

#[allow(clippy::too_many_arguments)]
pub(crate) fn execute(
    lir: &lir::Function,
    locations: &[(usize, usize)],
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
    let mut registers = vec![
        Slot::default();
        lir.registers
            .len()
            .checked_add(1 + extra_slots)
            .ok_or(Error::IntegerOverflow)?
    ];
    for (index, value) in vm.stack[base..frame_end].iter().enumerate() {
        if let Some(value) = value {
            registers[index] = Slot::boxed(*value);
        }
    }
    let rooted = vm.stack[base..frame_end]
        .iter()
        .enumerate()
        .filter_map(|(index, value)| value.map(|_| index))
        .collect();
    let mut context = Context {
        vm: vm as *mut Vm,
        function,
        code,
        lir,
        locations,
        base,
        rooted,
        previous: None,
        skip_probe,
        error: None,
        registers: registers.as_mut_ptr(),
        helper_arguments: Vec::new(),
    };
    // SAFETY: Context and the fixed-size slot allocation remain alive and unmoved for this
    // synchronous invocation. Generated memory offsets are bounds checked during emission.
    let status = {
        invoke(
            (&mut context as *mut Context<'_>).cast(),
            registers.as_mut_ptr(),
            u32::try_from(start).map_err(error)?,
        )
    };
    if let Some(error) = context.error {
        return Err(error);
    }
    if status == 0 {
        return Ok(Outcome::Return(registers[lir.registers.len()].value()?));
    }
    let exit = lir
        .exits
        .get(usize::try_from(status.wrapping_sub(1)).map_err(error)?)
        .ok_or(Error::InvalidIr("native exit status"))?;
    let environment = *vm
        .environments
        .last()
        .ok_or(Error::InvalidIr("native environment"))?;
    let frame = lir.reconstruct(exit.state, environment, vm.local_values()?, |r| {
        registers[r.0].value()
    })?;
    vm.mark_deoptimization(function, &frame, exit.reason);
    Ok(Outcome::Deopt(frame))
}
pub(crate) extern "C" fn power(left: f64, right: f64) -> f64 {
    left.powf(right)
}

struct Context<'a> {
    vm: *mut Vm,
    function: &'a Rc<RefCell<Function>>,
    code: &'a Code,
    lir: &'a lir::Function,
    locations: &'a [(usize, usize)],
    base: usize,
    rooted: Vec<usize>,
    previous: Option<usize>,
    skip_probe: bool,
    error: Option<Error>,
    registers: *mut Slot,
    // Each native invocation owns its scratch buffer, including during recursive helper calls.
    helper_arguments: Vec<Value>,
}
pub(crate) extern "C" fn callback(
    raw: *mut c_void,
    registers: *mut Slot,
    event: i32,
    location: i32,
) -> i32 {
    // SAFETY: only generated Entry calls this private callback, using its original Context
    // pointer. Calls are synchronous; nested JIT frames allocate distinct contexts and slots.
    let context = unsafe { &mut *raw.cast::<Context<'_>>() };
    // SAFETY: the VM's exclusive borrow is suspended for Entry; callbacks temporarily reborrow it.
    let vm = unsafe { &mut *context.vm };
    let checkpoint = (
        vm.stack.len(),
        vm.environments.len(),
        vm.transient_roots.len(),
        vm.depth,
    );
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        context.step(registers, event, location)
    }));
    match result {
        Ok(Ok(())) => 0,
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
            vm.local_frames.truncate(checkpoint.1);
            vm.transient_roots.truncate(checkpoint.2);
            vm.depth = checkpoint.3;
            context.error = Some(Error::Runtime("panic in native runtime helper".into()));
            1
        }
    }
}
impl Context<'_> {
    fn step(&mut self, registers: *mut Slot, event: i32, location: i32) -> Result<()> {
        if registers != self.registers {
            return Err(Error::InvalidIr("native register buffer changed"));
        }
        // SAFETY: this is the fixed-size buffer passed to Entry. The callback has sole access while
        // generated code is suspended; helpers cannot resize or retain this buffer.
        let registers =
            unsafe { std::slice::from_raw_parts_mut(registers, self.lir.registers.len() + 1) };
        // SAFETY: callbacks reborrow the exclusively owned VM only until returning to Entry.
        let vm = unsafe { &mut *self.vm };
        if event == ENTER {
            for parameter in &self.lir.blocks[location as usize].parameters {
                let value = registers[parameter.0].value()?;
                if vm.stack[self.base + parameter.0].is_none() {
                    self.rooted.push(parameter.0);
                }
                vm.stack[self.base + parameter.0] = Some(value);
            }
            if self.skip_probe {
                self.skip_probe = false;
            } else {
                vm.record_block(self.function, self.code, location as usize, self.previous)?;
            }
            self.previous = Some(location as usize);
            return vm.maybe_collect();
        }
        let &(block, index) = self
            .locations
            .get(location as usize)
            .ok_or(Error::InvalidIr("native callback site"))?;
        let instruction = &self.lir.blocks[block].instructions[index];
        let mut observed = false;
        if event == HELPER {
            let (state, roots) = match &instruction.operation {
                Operation::Call { state, roots, .. } | Operation::Truth { state, roots, .. } => {
                    (*state, roots)
                }
                _ => return Err(Error::InvalidIr("native helper instruction")),
            };
            vm.sync_lir_roots(
                &self.lir.states[state.0],
                roots,
                self.base,
                &mut self.rooted,
                |r| registers[r.0].value(),
            )?;
            let result = match &instruction.operation {
                Operation::Call {
                    helper, arguments, ..
                } => {
                    self.helper_arguments.clear();
                    for r in arguments {
                        self.helper_arguments.push(registers[r.0].value()?);
                    }
                    observed =
                        vm.observe_site(self.function, self.code, self.base, instruction.origin)?;
                    let roots = vm.transient_roots.len();
                    vm.transient_roots.extend_from_slice(&self.helper_arguments);
                    let result = vm.lir_helper(helper, &self.helper_arguments);
                    vm.transient_roots.truncate(roots);
                    Slot::boxed(result?)
                }
                Operation::Truth { value, .. } => Slot {
                    tag: RAW_BOOL,
                    a: crate::runtime::truth(&vm.heap, registers[value.0].value()?)? as u64,
                    b: 0,
                    c: 0,
                },
                _ => unreachable!(),
            };
            registers[instruction.result.unwrap().0] = result;
        } else if event != FINISH {
            return Err(Error::InvalidIr("native callback event"));
        }
        if let Some(result) = instruction.result
            && self.lir.registers[result.0] == lir::Type::Boxed
        {
            if event == FINISH {
                observed =
                    vm.observe_site(self.function, self.code, self.base, instruction.origin)?;
            }
            let value = registers[result.0].value()?;
            if vm.stack[self.base + result.0].is_none() {
                self.rooted.push(result.0);
            }
            vm.stack[self.base + result.0] = Some(value);
            let function = self.function.borrow();
            let mut profile = function.profile.borrow_mut();
            let site = &mut profile.sites[instruction.origin.0];
            if observed {
                site.result
                    .observe(crate::profile::ValueKind::of(&vm.heap, value));
            }
            if matches!(
                instruction.operation,
                Operation::BoxInt(_) | Operation::BoxFloat(_) | Operation::BoxBool(_)
            ) {
                site.fast_paths = site.fast_paths.saturating_add(1);
            }
        }
        vm.maybe_collect()
    }
}
