//! Cranelift gateway. Generated code only accesses fixed-size register slots; heap access stays in Rust helpers.
use crate::{
    Error, ObjectId, Result, Value, Vm,
    bytecode::Code,
    hir::Function,
    lir::{self, FloatOperator, Operation, Register, Terminator},
    tier::Outcome,
};
use cranelift_codegen::ir::{
    self, AbiParam, InstBuilder, MemFlagsData,
    condcodes::{FloatCC, IntCC},
    types,
};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
use cranelift_jit::{JITBuilder, JITModule};
use cranelift_module::{Linkage, Module, default_libcall_names};
use std::{cell::RefCell, ffi::c_void, fmt, rc::Rc};

const NONE: u64 = 0;
const BOOL: u64 = 1;
const INT: u64 = 2;
const FLOAT: u64 = 3;
const REF: u64 = 4;
const RAW_INT: u64 = 5;
const RAW_FLOAT: u64 = 6;
const RAW_BOOL: u64 = 7;
const EMPTY: u64 = 8;
const ENTER: i32 = 1;
const HELPER: i32 = 2;
const FINISH: i32 = 3;
#[repr(C)]
#[derive(Clone, Copy)]
struct Slot {
    tag: u64,
    a: u64,
    b: u64,
    c: u64,
}
impl Default for Slot {
    fn default() -> Self {
        Self {
            tag: EMPTY,
            a: 0,
            b: 0,
            c: 0,
        }
    }
}
impl Slot {
    fn boxed(value: Value) -> Self {
        match value {
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
    fn value(self) -> Result<Value> {
        Ok(match self.tag {
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
type Entry = unsafe extern "C" fn(*mut c_void, *mut Slot, u32) -> u64;
pub(crate) struct NativeCode {
    module: Option<JITModule>,
    entry: Option<Entry>,
    pub lir: Rc<lir::Function>,
    locations: Vec<(usize, usize)>,
    pub bytes: usize,
}
impl fmt::Debug for NativeCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NativeCode")
            .field("source", &self.lir.source)
            .field("bytes", &self.bytes)
            .finish()
    }
}
impl Drop for NativeCode {
    fn drop(&mut self) {
        if let Some(module) = self.module.take() {
            // SAFETY: every invocation borrows an Rc-owned NativeCode. The final owner cannot drop
            // until generated code has returned; neither entry pointers nor module references escape.
            unsafe {
                module.free_memory();
            }
        }
    }
}
fn error(message: impl fmt::Display) -> Error {
    Error::Runtime(format!("native compilation: {message}"))
}

impl NativeCode {
    pub fn compile(lir: Rc<lir::Function>) -> Result<Self> {
        lir.verify()?;
        if std::mem::size_of::<usize>() != 8 || std::mem::size_of::<Slot>() != 32 {
            return Err(error("native backend requires 64-bit pointers"));
        }
        let mut jit = JITBuilder::new(default_libcall_names()).map_err(error)?;
        jit.symbol("tis_callback", callback as *const u8);
        jit.symbol("tis_power", power as *const u8);
        let mut owner = Self {
            module: Some(JITModule::new(jit)),
            entry: None,
            lir,
            locations: vec![],
            bytes: 0,
        };
        let module = owner.module.as_mut().unwrap();
        let ptr = module.target_config().pointer_type();
        let mut callback_sig = module.make_signature();
        callback_sig.params = vec![
            AbiParam::new(ptr),
            AbiParam::new(ptr),
            AbiParam::new(types::I32),
            AbiParam::new(types::I32),
        ];
        callback_sig.returns.push(AbiParam::new(types::I32));
        let callback_id = module
            .declare_function("tis_callback", Linkage::Import, &callback_sig)
            .map_err(error)?;
        let mut power_sig = module.make_signature();
        power_sig.params = vec![AbiParam::new(types::F64), AbiParam::new(types::F64)];
        power_sig.returns.push(AbiParam::new(types::F64));
        let power_id = module
            .declare_function("tis_power", Linkage::Import, &power_sig)
            .map_err(error)?;
        let mut context = module.make_context();
        context.func.signature.params = vec![
            AbiParam::new(ptr),
            AbiParam::new(ptr),
            AbiParam::new(types::I32),
        ];
        context
            .func
            .signature
            .returns
            .push(AbiParam::new(types::I64));
        let name = format!("tis_{}_{}", owner.lir.source.0, owner.lir.revision);
        let id = module
            .declare_function(&name, Linkage::Local, &context.func.signature)
            .map_err(error)?;
        let mut builder_context = FunctionBuilderContext::new();
        {
            let mut b = FunctionBuilder::new(&mut context.func, &mut builder_context);
            let entry = b.create_block();
            b.switch_to_block(entry);
            b.append_block_params_for_function_params(entry);
            let params = b.block_params(entry).to_vec();
            let (host, registers, start) = (params[0], params[1], params[2]);
            let callback_ref = module.declare_func_in_func(callback_id, b.func);
            let power_ref = module.declare_func_in_func(power_id, b.func);
            let blocks = (0..owner.lir.blocks.len())
                .map(|_| b.create_block())
                .collect::<Vec<_>>();
            let failed = b.create_block();
            for (index, &target) in blocks.iter().enumerate() {
                let next = b.create_block();
                let matches = b.ins().icmp_imm_s(IntCC::Equal, start, index as i64);
                b.ins().brif(matches, target, &[], next, &[]);
                b.switch_to_block(next);
            }
            b.ins().jump(failed, &[]);
            for (block_index, body) in owner.lir.blocks.iter().enumerate() {
                b.switch_to_block(blocks[block_index]);
                call(
                    &mut b,
                    callback_ref,
                    host,
                    registers,
                    ENTER,
                    block_index as i32,
                    failed,
                );
                for (instruction_index, instruction) in body.instructions.iter().enumerate() {
                    let location = i32::try_from(owner.locations.len()).map_err(error)?;
                    owner.locations.push((block_index, instruction_index));
                    let destination = instruction.result;
                    match &instruction.operation {
                        Operation::Constant(value) => {
                            let value = match value {
                                crate::hir::Constant::None => Value::None,
                                crate::hir::Constant::Bool(v) => Value::Bool(*v),
                                crate::hir::Constant::Int(v) => Value::Int(*v),
                                crate::hir::Constant::Float(v) => Value::Float(*v),
                                _ => return Err(error("allocating constant")),
                            };
                            store_constant(
                                &mut b,
                                registers,
                                destination.unwrap(),
                                Slot::boxed(value),
                            )?;
                        }
                        Operation::GuardInt { value, exit }
                        | Operation::GuardFloat { value, exit } => {
                            let tag = load(&mut b, registers, *value, 0, types::I64)?;
                            let expected =
                                if matches!(instruction.operation, Operation::GuardInt { .. }) {
                                    INT
                                } else {
                                    FLOAT
                                };
                            let passed = b.ins().icmp_imm_s(IntCC::Equal, tag, expected as i64);
                            guard(&mut b, passed, *exit);
                        }
                        Operation::UnboxInt(value)
                        | Operation::UnboxFloat(value)
                        | Operation::BoxInt(value)
                        | Operation::BoxFloat(value)
                        | Operation::BoxBool(value) => {
                            let bits = load(&mut b, registers, *value, 8, types::I64)?;
                            let tag = match instruction.operation {
                                Operation::UnboxInt(_) => RAW_INT,
                                Operation::UnboxFloat(_) => RAW_FLOAT,
                                Operation::BoxInt(_) => INT,
                                Operation::BoxFloat(_) => FLOAT,
                                _ => BOOL,
                            };
                            store_scalar(&mut b, registers, destination.unwrap(), tag, bits)?;
                        }
                        Operation::IntToFloat(value) => {
                            let value = load(&mut b, registers, *value, 8, types::I64)?;
                            let converted = b.ins().fcvt_from_sint(types::F64, value);
                            store_float(&mut b, registers, destination.unwrap(), converted)?;
                        }
                        Operation::CheckedAdd {
                            left,
                            right,
                            overflow,
                        } => {
                            let left = load(&mut b, registers, *left, 8, types::I64)?;
                            let right = load(&mut b, registers, *right, 8, types::I64)?;
                            let sum = b.ins().iadd(left, right);
                            let x = b.ins().bxor(left, sum);
                            let y = b.ins().bxor(right, sum);
                            let both = b.ins().band(x, y);
                            let safe = b.ins().icmp_imm_s(IntCC::SignedGreaterThanOrEqual, both, 0);
                            guard(&mut b, safe, *overflow);
                            store_scalar(&mut b, registers, destination.unwrap(), RAW_INT, sum)?;
                        }
                        Operation::FloatBinary {
                            operator,
                            left,
                            right,
                            failure,
                        } => {
                            let left = load(&mut b, registers, *left, 8, types::F64)?;
                            let right = load(&mut b, registers, *right, 8, types::F64)?;
                            let result = match operator {
                                FloatOperator::Add => b.ins().fadd(left, right),
                                FloatOperator::Subtract => b.ins().fsub(left, right),
                                FloatOperator::Multiply => b.ins().fmul(left, right),
                                FloatOperator::Divide => {
                                    let zero = b.ins().f64const(0.0);
                                    let safe = b.ins().fcmp(FloatCC::NotEqual, right, zero);
                                    guard(&mut b, safe, *failure);
                                    b.ins().fdiv(left, right)
                                }
                                FloatOperator::Power => {
                                    let zero = b.ins().f64const(0.0);
                                    let is_zero = b.ins().fcmp(FloatCC::Equal, left, zero);
                                    let negative = b.ins().fcmp(FloatCC::LessThan, right, zero);
                                    let invalid = b.ins().band(is_zero, negative);
                                    let safe = b.ins().icmp_imm_s(IntCC::Equal, invalid, 0);
                                    guard(&mut b, safe, *failure);
                                    let call = b.ins().call(power_ref, &[left, right]);
                                    let result = b.inst_results(call)[0];
                                    let ordered = b.ins().fcmp(FloatCC::Ordered, result, result);
                                    guard(&mut b, ordered, *failure);
                                    result
                                }
                            };
                            store_float(&mut b, registers, destination.unwrap(), result)?;
                        }
                        Operation::FloatCompare {
                            operator,
                            left,
                            right,
                        } => {
                            let left = load(&mut b, registers, *left, 8, types::F64)?;
                            let right = load(&mut b, registers, *right, 8, types::F64)?;
                            let cc = match operator.as_str() {
                                "==" => FloatCC::Equal,
                                "!=" => FloatCC::NotEqual,
                                "<" => FloatCC::LessThan,
                                "<=" => FloatCC::LessThanOrEqual,
                                ">" => FloatCC::GreaterThan,
                                ">=" => FloatCC::GreaterThanOrEqual,
                                _ => return Err(error("float comparison")),
                            };
                            let compared = b.ins().fcmp(cc, left, right);
                            let bits = b.ins().uextend(types::I64, compared);
                            store_scalar(&mut b, registers, destination.unwrap(), RAW_BOOL, bits)?;
                        }
                        Operation::Truth { .. } | Operation::Call { .. } => call(
                            &mut b,
                            callback_ref,
                            host,
                            registers,
                            HELPER,
                            location,
                            failed,
                        ),
                    }
                    if destination
                        .is_some_and(|register| owner.lir.registers[register.0] == lir::Type::Boxed)
                        && !matches!(instruction.operation, Operation::Call { .. })
                    {
                        call(
                            &mut b,
                            callback_ref,
                            host,
                            registers,
                            FINISH,
                            location,
                            failed,
                        );
                    }
                }
                match &body.terminator {
                    Terminator::Return(value) => {
                        copy_slots(
                            &mut b,
                            registers,
                            &[(*value, Register(owner.lir.registers.len()))],
                        )?;
                        let status = b.ins().iconst(types::I64, 0);
                        b.ins().return_(&[status]);
                    }
                    Terminator::Jump(edge) => {
                        edge_jump(&mut b, registers, &owner.lir, edge, &blocks)?;
                    }
                    Terminator::Branch { condition, yes, no } => {
                        let condition = load(&mut b, registers, *condition, 8, types::I64)?;
                        let yes_block = b.create_block();
                        let no_block = b.create_block();
                        b.ins().brif(condition, yes_block, &[], no_block, &[]);
                        b.switch_to_block(yes_block);
                        edge_jump(&mut b, registers, &owner.lir, yes, &blocks)?;
                        b.switch_to_block(no_block);
                        edge_jump(&mut b, registers, &owner.lir, no, &blocks)?;
                    }
                }
            }
            b.switch_to_block(failed);
            let status = b.ins().iconst(types::I64, -1);
            b.ins().return_(&[status]);
            b.seal_all_blocks();
            b.finalize(module.target_config());
        }
        module.define_function(id, &mut context).map_err(error)?;
        owner.bytes = context
            .compiled_code()
            .map_or(0, |code| code.code_buffer().len());
        module.finalize_definitions().map_err(error)?;
        let pointer = module.get_finalized_function(id);
        // SAFETY: the finalized function has exactly Entry's target C ABI and remains executable
        // until this NativeCode's final Rc is dropped. No pointer is exposed to callers.
        owner.entry = Some(unsafe { std::mem::transmute::<*const u8, Entry>(pointer) });
        Ok(owner)
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
        skip_probe: bool,
    ) -> Result<Outcome> {
        let mut registers = vec![Slot::default(); self.lir.registers.len() + 1];
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
            program: self,
            base,
            rooted,
            previous: None,
            skip_probe,
            error: None,
            registers: registers.as_mut_ptr(),
        };
        // SAFETY: Context and the fixed-size slot allocation remain alive and unmoved for this
        // synchronous invocation. Generated memory offsets are bounds checked during emission.
        let status = unsafe {
            self.entry.unwrap()(
                (&mut context as *mut Context<'_>).cast(),
                registers.as_mut_ptr(),
                u32::try_from(start).map_err(error)?,
            )
        };
        if let Some(error) = context.error {
            return Err(error);
        }
        if status == 0 {
            return Ok(Outcome::Return(
                registers[self.lir.registers.len()].value()?,
            ));
        }
        let exit = self
            .lir
            .exits
            .get(usize::try_from(status.wrapping_sub(1)).map_err(error)?)
            .ok_or(Error::InvalidIr("native exit status"))?;
        let environment = *vm
            .environments
            .last()
            .ok_or(Error::InvalidIr("native environment"))?;
        let frame = self
            .lir
            .reconstruct(exit.state, environment, |r| registers[r.0].value())?;
        vm.mark_deoptimization(function, &frame);
        Ok(Outcome::Deopt(frame))
    }
}

fn offset(register: Register, field: i32) -> Result<i32> {
    i32::try_from(register.0.checked_mul(32).ok_or(Error::IntegerOverflow)?)
        .ok()
        .and_then(|v| v.checked_add(field))
        .ok_or(Error::IntegerOverflow)
}
fn load(
    b: &mut FunctionBuilder<'_>,
    registers: ir::Value,
    register: Register,
    field: i32,
    kind: ir::Type,
) -> Result<ir::Value> {
    Ok(b.ins().load(
        kind,
        MemFlagsData::new(),
        registers,
        offset(register, field)?,
    ))
}
fn store_constant(
    b: &mut FunctionBuilder<'_>,
    registers: ir::Value,
    register: Register,
    slot: Slot,
) -> Result<()> {
    for (field, value) in [(0, slot.tag), (8, slot.a), (16, slot.b), (24, slot.c)] {
        let value = b.ins().iconst(types::I64, value as i64);
        b.ins().store(
            MemFlagsData::new(),
            value,
            registers,
            offset(register, field)?,
        );
    }
    Ok(())
}
fn store_scalar(
    b: &mut FunctionBuilder<'_>,
    registers: ir::Value,
    register: Register,
    tag: u64,
    value: ir::Value,
) -> Result<()> {
    let tag = b.ins().iconst(types::I64, tag as i64);
    b.ins()
        .store(MemFlagsData::new(), tag, registers, offset(register, 0)?);
    b.ins()
        .store(MemFlagsData::new(), value, registers, offset(register, 8)?);
    Ok(())
}
fn store_float(
    b: &mut FunctionBuilder<'_>,
    registers: ir::Value,
    register: Register,
    value: ir::Value,
) -> Result<()> {
    let tag = b.ins().iconst(types::I64, RAW_FLOAT as i64);
    b.ins()
        .store(MemFlagsData::new(), tag, registers, offset(register, 0)?);
    b.ins()
        .store(MemFlagsData::new(), value, registers, offset(register, 8)?);
    Ok(())
}
fn copy_slots(
    b: &mut FunctionBuilder<'_>,
    registers: ir::Value,
    pairs: &[(Register, Register)],
) -> Result<()> {
    let mut copies = vec![];
    for &(from, to) in pairs {
        for field in [0, 8, 16, 24] {
            copies.push((to, field, load(b, registers, from, field, types::I64)?));
        }
    }
    for (to, field, value) in copies {
        b.ins()
            .store(MemFlagsData::new(), value, registers, offset(to, field)?);
    }
    Ok(())
}
fn edge_jump(
    b: &mut FunctionBuilder<'_>,
    registers: ir::Value,
    lir: &lir::Function,
    edge: &lir::Edge,
    blocks: &[ir::Block],
) -> Result<()> {
    let pairs = edge
        .arguments
        .iter()
        .copied()
        .zip(lir.blocks[edge.block.0].parameters.iter().copied())
        .collect::<Vec<_>>();
    copy_slots(b, registers, &pairs)?;
    b.ins().jump(blocks[edge.block.0], &[]);
    Ok(())
}
fn guard(b: &mut FunctionBuilder<'_>, passed: ir::Value, exit: lir::ExitId) {
    let success = b.create_block();
    let failed = b.create_block();
    b.ins().brif(passed, success, &[], failed, &[]);
    b.switch_to_block(failed);
    let status = b.ins().iconst(types::I64, exit.0 as i64 + 1);
    b.ins().return_(&[status]);
    b.switch_to_block(success);
}
fn call(
    b: &mut FunctionBuilder<'_>,
    function: ir::FuncRef,
    host: ir::Value,
    registers: ir::Value,
    event: i32,
    location: i32,
    failed: ir::Block,
) {
    let event = b.ins().iconst(types::I32, event as i64);
    let location = b.ins().iconst(types::I32, location as i64);
    let call = b.ins().call(function, &[host, registers, event, location]);
    let status = b.inst_results(call)[0];
    let next = b.create_block();
    b.ins().brif(status, failed, &[], next, &[]);
    b.switch_to_block(next);
}
extern "C" fn power(left: f64, right: f64) -> f64 {
    left.powf(right)
}

struct Context<'a> {
    vm: *mut Vm,
    function: &'a Rc<RefCell<Function>>,
    code: &'a Code,
    program: &'a NativeCode,
    base: usize,
    rooted: Vec<usize>,
    previous: Option<usize>,
    skip_probe: bool,
    error: Option<Error>,
    registers: *mut Slot,
}
extern "C" fn callback(raw: *mut c_void, registers: *mut Slot, event: i32, location: i32) -> i32 {
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
        let registers = unsafe {
            std::slice::from_raw_parts_mut(registers, self.program.lir.registers.len() + 1)
        };
        // SAFETY: callbacks reborrow the exclusively owned VM only until returning to Entry.
        let vm = unsafe { &mut *self.vm };
        if event == ENTER {
            for parameter in &self.program.lir.blocks[location as usize].parameters {
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
            .program
            .locations
            .get(location as usize)
            .ok_or(Error::InvalidIr("native callback site"))?;
        let instruction = &self.program.lir.blocks[block].instructions[index];
        if event == HELPER {
            let (state, roots) = match &instruction.operation {
                Operation::Call { state, roots, .. } | Operation::Truth { state, roots, .. } => {
                    (*state, roots)
                }
                _ => return Err(Error::InvalidIr("native helper instruction")),
            };
            vm.sync_lir_roots(
                &self.program.lir.states[state.0],
                roots,
                self.base,
                &mut self.rooted,
                |r| registers[r.0].value(),
            )?;
            let result = match &instruction.operation {
                Operation::Call {
                    helper, arguments, ..
                } => {
                    let values = arguments
                        .iter()
                        .map(|r| registers[r.0].value())
                        .collect::<Result<Vec<_>>>()?;
                    vm.observe_site(self.function, self.code, self.base, instruction.origin)?;
                    let roots = vm.transient_roots.len();
                    vm.transient_roots.extend_from_slice(&values);
                    let result = vm.lir_helper(helper, &values);
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
            && self.program.lir.registers[result.0] == lir::Type::Boxed
        {
            if event == FINISH {
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
            site.result
                .observe(crate::profile::ValueKind::of(&vm.heap, value));
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
