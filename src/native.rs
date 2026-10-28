//! Cranelift gateway with guarded heap views and explicit runtime materialization boundaries.
mod region;
use crate::{
    Error, Result, Value, Vm,
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
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext, Variable};
use cranelift_jit::{JITBuilder, JITModule};
use cranelift_module::{Linkage, Module, default_libcall_names};
use std::{cell::RefCell, fmt, rc::Rc};

use crate::native_abi::*;
use crate::native_runtime::{Entry, callback, power};
#[derive(Debug, Clone)]
pub struct NativeDiagnostics {
    pub clif: String,
    pub disassembly: String,
    pub machine_code: Vec<u8>,
}
pub(crate) struct NativeCode {
    module: Option<JITModule>,
    entry: Option<Entry>,
    pub lir: Rc<lir::Function>,
    locations: Vec<(usize, usize)>,
    root_states: Vec<lir::FrameState>,
    pub bytes: usize,
    pub feedback: bool,
    pub diagnostics: Option<NativeDiagnostics>,
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
    pub fn compile(lir: Rc<lir::Function>, feedback: bool, capture: bool) -> Result<Self> {
        lir.verify()?;
        if std::mem::size_of::<usize>() != 8 || std::mem::size_of::<Slot>() != 32 {
            return Err(error("native backend requires 64-bit pointers"));
        }
        // Region code exposes enough SSA to optimize; Cranelift's default skips its optimization pipeline.
        let mut jit = JITBuilder::with_flags(&[("opt_level", "speed")], default_libcall_names())
            .map_err(error)?;
        jit.symbol("tis_callback", callback as *const u8);
        jit.symbol(
            "tis_region",
            crate::native_runtime::region_callback as *const u8,
        );
        jit.symbol(
            "tis_float_view",
            crate::native_runtime::float_view_callback as *const u8,
        );
        jit.symbol("tis_power", power as *const u8);
        let recipes = lir.native_boxing_recipes();
        let root_states = lir
            .states
            .iter()
            .map(|state| {
                let mut roots = state.clone();
                roots
                    .values
                    .retain(|(_, register)| !recipes.contains_key(register));
                roots
            })
            .collect();
        let mut owner = Self {
            module: Some(JITModule::new(jit)),
            entry: None,
            lir,
            locations: vec![],
            root_states,
            bytes: 0,
            feedback,
            diagnostics: None,
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
        let view_id = module
            .declare_function("tis_float_view", Linkage::Import, &callback_sig)
            .map_err(error)?;
        let region_id = module
            .declare_function("tis_region", Linkage::Import, &callback_sig)
            .map_err(error)?;
        let mut power_sig = module.make_signature();
        power_sig.params = vec![AbiParam::new(types::F64), AbiParam::new(types::F64)];
        power_sig.returns.push(AbiParam::new(types::F64));
        let power_id = module
            .declare_function("tis_power", Linkage::Import, &power_sig)
            .map_err(error)?;
        let mut context = module.make_context();
        context.set_disasm(capture);
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
            let scalars = ScalarVars::new(&mut b, &owner.lir, !feedback);
            let entry = b.create_block();
            b.switch_to_block(entry);
            b.append_block_params_for_function_params(entry);
            let params = b.block_params(entry).to_vec();
            let (host, registers, start) = (params[0], params[1], params[2]);
            scalars.initialize(&mut b, registers, &owner.lir)?;
            let callback_ref = module.declare_func_in_func(callback_id, b.func);
            let view_ref = module.declare_func_in_func(view_id, b.func);
            let region_ref = module.declare_func_in_func(region_id, b.func);
            let region_active = owner
                .lir
                .regions
                .iter()
                .map(|_| {
                    let v = b.declare_var(types::I64);
                    let zero = b.ins().iconst(types::I64, 0);
                    b.def_var(v, zero);
                    v
                })
                .collect::<Vec<_>>();
            let power_ref = module.declare_func_in_func(power_id, b.func);
            let blocks = (0..owner.lir.blocks.len())
                .map(|_| b.create_block())
                .collect::<Vec<_>>();
            let failed = b.create_block();
            for (index, &target) in blocks.iter().enumerate() {
                // A region body requires its prepared header. Even the private entry dispatcher must not
                // admit an unprepared jump past guards whose checks have been hoisted out of the loop.
                if !feedback && owner.lir.regions.iter().any(|r| r.body.0 == index) {
                    continue;
                }
                let next = b.create_block();
                let matches = b.ins().icmp_imm_s(IntCC::Equal, start, index as i64);
                b.ins().brif(matches, target, &[], next, &[]);
                b.switch_to_block(next);
            }
            b.ins().jump(failed, &[]);
            macro_rules! runtime_call {
                ($builder:expr, $function:expr, $host:expr, $registers:expr, $event:expr, $location:expr, $failed:expr $(,)?) => {{
                    let needed = if $event == ENTER && scalars.2.iter().any(Option::is_some) {
                        vec![]
                    } else {
                        callback_values(&owner.lir, &owner.locations, $event, $location)
                    };
                    scalars.materialize($builder, $registers, needed)?;
                    let event = $builder.ins().iconst(types::I32, $event as i64);
                    let location = $builder.ins().iconst(types::I32, $location as i64);
                    let call = $builder
                        .ins()
                        .call($function, &[$host, $registers, event, location]);
                    let status = $builder.inst_results(call)[0];
                    let failed = $builder.create_block();
                    let next = $builder.create_block();
                    $builder.ins().brif(status, failed, &[], next, &[]);
                    $builder.switch_to_block(failed);
                    let needed =
                        callback_failure_values(&owner.lir, &owner.locations, $event, $location);
                    scalars.materialize($builder, $registers, needed)?;
                    $builder.ins().jump($failed, &[]);
                    $builder.switch_to_block(next);
                    scalars.reload_callback(
                        $builder,
                        $registers,
                        &owner.lir,
                        &owner.locations,
                        $event,
                        $location,
                    )?;
                }};
            }
            macro_rules! side_exit {
                ($builder:expr, $passed:expr, $exit:expr) => {{
                    let exit = $exit;
                    let success = $builder.create_block();
                    let failed = $builder.create_block();
                    $builder.ins().brif($passed, success, &[], failed, &[]);
                    $builder.switch_to_block(failed);
                    let state = &owner.lir.states[owner.lir.exits[exit.0].state.0];
                    scalars.materialize($builder, registers, state_values(state))?;
                    let status = $builder.ins().iconst(types::I64, exit.0 as i64 + 1);
                    $builder.ins().return_(&[status]);
                    $builder.switch_to_block(success);
                }};
            }
            for (block_index, body) in owner.lir.blocks.iter().enumerate() {
                b.switch_to_block(blocks[block_index]);
                let region_index = (!feedback)
                    .then(|| {
                        owner
                            .lir
                            .regions
                            .iter()
                            .position(|r| r.header.0 == block_index || r.body.0 == block_index)
                    })
                    .flatten();
                if let Some(index) = region_index {
                    let r = &owner.lir.regions[index];
                    if r.header.0 == block_index {
                        let active = b.use_var(region_active[index]);
                        let prepare = b.create_block();
                        let ready = b.create_block();
                        b.ins().brif(active, ready, &[], prepare, &[]);
                        b.switch_to_block(prepare);
                        runtime_call!(
                            &mut b,
                            region_ref,
                            host,
                            registers,
                            REGION_ENTER,
                            index as i32,
                            failed
                        );
                        let one = b.ins().iconst(types::I64, 1);
                        b.def_var(region_active[index], one);
                        b.ins().jump(ready, &[]);
                        b.switch_to_block(ready);
                    }
                    let block = b.ins().iconst(types::I64, block_index as i64);
                    b.ins().store(
                        MemFlagsData::new(),
                        block,
                        registers,
                        offset(Register(owner.lir.registers.len() + 1), 0)?,
                    );
                } else {
                    runtime_call!(
                        &mut b,
                        callback_ref,
                        host,
                        registers,
                        ENTER,
                        block_index as i32,
                        failed
                    );
                }
                let mut unpack_reads = 0;
                for (instruction_index, instruction) in body.instructions.iter().enumerate() {
                    let location = i32::try_from(owner.locations.len()).map_err(error)?;
                    owner.locations.push((block_index, instruction_index));
                    if unpack_reads > 0 {
                        unpack_reads -= 1;
                        continue;
                    }
                    let destination = instruction.result;
                    if let Some(index) = region_index {
                        let mut emitter = region::Emitter {
                            b: &mut b,
                            scalars: &scalars,
                            registers,
                            lir: &owner.lir,
                            region: &owner.lir.regions[index],
                        };
                        if emitter.emit(&instruction.operation, destination)? {
                            continue;
                        }
                    }
                    match &instruction.operation {
                        Operation::FloatListView { .. } => runtime_call!(
                            &mut b, view_ref, host, registers, FLOAT_VIEW, location, failed,
                        ),
                        Operation::FloatListUnpack { value, .. } => {
                            copy_slots(
                                &mut b,
                                &scalars,
                                registers,
                                &[(*value, destination.unwrap())],
                            )?;
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
                            let pointer =
                                load_cached(&mut b, &scalars, registers, *view, 8, types::I64)?;
                            let len =
                                load_cached(&mut b, &scalars, registers, *view, 16, types::I64)?;
                            let index = if *index < 0 {
                                b.ins().iadd_imm_s(len, *index)
                            } else {
                                b.ins().iconst(types::I64, *index)
                            };
                            let valid = b.ins().icmp(IntCC::UnsignedLessThan, index, len);
                            if region_index.is_none_or(|i| owner.lir.regions[i].schema.is_none()) {
                                side_exit!(&mut b, valid, *failure);
                            }
                            let offset = b.ins().ishl_imm_u(index, 3);
                            let pointer = b.ins().iadd(pointer, offset);
                            if let Operation::FloatListStore { value, .. } = &instruction.operation
                            {
                                let value = load_cached(
                                    &mut b,
                                    &scalars,
                                    registers,
                                    *value,
                                    8,
                                    types::F64,
                                )?;
                                b.ins().store(MemFlagsData::new(), value, pointer, 0);
                                store_constant_cached(
                                    &mut b,
                                    &scalars,
                                    registers,
                                    destination.unwrap(),
                                    Slot::boxed(Value::None),
                                )?;
                            } else {
                                let value =
                                    b.ins().load(types::F64, MemFlagsData::new(), pointer, 0);
                                let bits = b.ins().bitcast(types::I64, MemFlagsData::new(), value);
                                store_scalar_cached(
                                    &mut b,
                                    &scalars,
                                    registers,
                                    destination.unwrap(),
                                    FLOAT,
                                    bits,
                                )?;
                            }
                        }
                        Operation::SequenceUnpack { count, .. } if !feedback => {
                            unpack_reads = *count;
                            runtime_call!(
                                &mut b,
                                callback_ref,
                                host,
                                registers,
                                UNPACK_BATCH,
                                location,
                                failed,
                            );
                        }
                        Operation::SequenceUnpack { .. } | Operation::SequenceGet { .. } => {
                            runtime_call!(
                                &mut b,
                                callback_ref,
                                host,
                                registers,
                                SEQUENCE,
                                location,
                                failed,
                            )
                        }
                        Operation::Constant(value) => {
                            let value = match value {
                                crate::hir::Constant::Unbound => Value::Unbound,
                                crate::hir::Constant::None => Value::None,
                                crate::hir::Constant::Bool(v) => Value::Bool(*v),
                                crate::hir::Constant::Int(v) => Value::Int(*v),
                                crate::hir::Constant::Float(v) => Value::Float(*v),
                                _ => return Err(error("allocating constant")),
                            };
                            store_constant_cached(
                                &mut b,
                                &scalars,
                                registers,
                                destination.unwrap(),
                                Slot::boxed(value),
                            )?;
                        }
                        Operation::GuardInt { value, exit }
                        | Operation::GuardFloat { value, exit } => {
                            let tag =
                                load_cached(&mut b, &scalars, registers, *value, 0, types::I64)?;
                            let expected =
                                if matches!(instruction.operation, Operation::GuardInt { .. }) {
                                    INT
                                } else {
                                    FLOAT
                                };
                            let passed = b.ins().icmp_imm_s(IntCC::Equal, tag, expected as i64);
                            side_exit!(&mut b, passed, *exit);
                        }
                        Operation::UnboxInt(value)
                        | Operation::UnboxFloat(value)
                        | Operation::BoxInt(value)
                        | Operation::BoxFloat(value)
                        | Operation::BoxBool(value) => {
                            let bits =
                                load_cached(&mut b, &scalars, registers, *value, 8, types::I64)?;
                            let tag = match instruction.operation {
                                Operation::UnboxInt(_) => RAW_INT,
                                Operation::UnboxFloat(_) => RAW_FLOAT,
                                Operation::BoxInt(_) => INT,
                                Operation::BoxFloat(_) => FLOAT,
                                _ => BOOL,
                            };
                            store_scalar_cached(
                                &mut b,
                                &scalars,
                                registers,
                                destination.unwrap(),
                                tag,
                                bits,
                            )?;
                        }
                        Operation::IntToFloat(value) => {
                            let value =
                                load_cached(&mut b, &scalars, registers, *value, 8, types::I64)?;
                            let converted = b.ins().fcvt_from_sint(types::F64, value);
                            store_float_cached(
                                &mut b,
                                &scalars,
                                registers,
                                destination.unwrap(),
                                converted,
                            )?;
                        }
                        Operation::CheckedAdd {
                            left,
                            right,
                            overflow,
                        } => {
                            let left =
                                load_cached(&mut b, &scalars, registers, *left, 8, types::I64)?;
                            let right =
                                load_cached(&mut b, &scalars, registers, *right, 8, types::I64)?;
                            let sum = b.ins().iadd(left, right);
                            let x = b.ins().bxor(left, sum);
                            let y = b.ins().bxor(right, sum);
                            let both = b.ins().band(x, y);
                            let safe = b.ins().icmp_imm_s(IntCC::SignedGreaterThanOrEqual, both, 0);
                            side_exit!(&mut b, safe, *overflow);
                            store_scalar_cached(
                                &mut b,
                                &scalars,
                                registers,
                                destination.unwrap(),
                                RAW_INT,
                                sum,
                            )?;
                        }
                        Operation::FloatBinary {
                            operator,
                            left,
                            right,
                            failure,
                        } => {
                            let left =
                                load_cached(&mut b, &scalars, registers, *left, 8, types::F64)?;
                            let right =
                                load_cached(&mut b, &scalars, registers, *right, 8, types::F64)?;
                            let result = match operator {
                                FloatOperator::Add => b.ins().fadd(left, right),
                                FloatOperator::Subtract => b.ins().fsub(left, right),
                                FloatOperator::Multiply => b.ins().fmul(left, right),
                                FloatOperator::Divide => {
                                    let zero = b.ins().f64const(0.0);
                                    let safe = b.ins().fcmp(FloatCC::NotEqual, right, zero);
                                    side_exit!(&mut b, safe, *failure);
                                    b.ins().fdiv(left, right)
                                }
                                FloatOperator::Power => {
                                    let zero = b.ins().f64const(0.0);
                                    let is_zero = b.ins().fcmp(FloatCC::Equal, left, zero);
                                    let negative = b.ins().fcmp(FloatCC::LessThan, right, zero);
                                    let invalid = b.ins().band(is_zero, negative);
                                    let safe = b.ins().icmp_imm_s(IntCC::Equal, invalid, 0);
                                    side_exit!(&mut b, safe, *failure);
                                    let call = b.ins().call(power_ref, &[left, right]);
                                    let result = b.inst_results(call)[0];
                                    let ordered = b.ins().fcmp(FloatCC::Ordered, result, result);
                                    side_exit!(&mut b, ordered, *failure);
                                    result
                                }
                            };
                            store_float_cached(
                                &mut b,
                                &scalars,
                                registers,
                                destination.unwrap(),
                                result,
                            )?;
                        }
                        Operation::FloatCompare {
                            operator,
                            left,
                            right,
                        } => {
                            let left =
                                load_cached(&mut b, &scalars, registers, *left, 8, types::F64)?;
                            let right =
                                load_cached(&mut b, &scalars, registers, *right, 8, types::F64)?;
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
                            store_scalar_cached(
                                &mut b,
                                &scalars,
                                registers,
                                destination.unwrap(),
                                RAW_BOOL,
                                bits,
                            )?;
                        }
                        Operation::Truth { value, .. } => {
                            let tag =
                                load_cached(&mut b, &scalars, registers, *value, 0, types::I64)?;
                            let scalar = b.ins().icmp_imm_s(
                                IntCC::UnsignedLessThanOrEqual,
                                tag,
                                FLOAT as i64,
                            );
                            let fast = b.create_block();
                            let slow = b.create_block();
                            let done = b.create_block();
                            b.ins().brif(scalar, fast, &[], slow, &[]);
                            b.switch_to_block(fast);
                            let bits =
                                load_cached(&mut b, &scalars, registers, *value, 8, types::I64)?;
                            let float = b.ins().icmp_imm_s(IntCC::Equal, tag, FLOAT as i64);
                            // Ignore the sign bit so both floating-point zeros are false, while NaNs stay true.
                            let magnitude = b.ins().ishl_imm_u(bits, 1);
                            let payload = b.ins().select(float, magnitude, bits);
                            let nonzero = b.ins().icmp_imm_s(IntCC::NotEqual, payload, 0);
                            let not_none = b.ins().icmp_imm_s(IntCC::NotEqual, tag, NONE as i64);
                            let truth = b.ins().band(nonzero, not_none);
                            let truth = b.ins().uextend(types::I64, truth);
                            store_scalar_cached(
                                &mut b,
                                &scalars,
                                registers,
                                destination.unwrap(),
                                RAW_BOOL,
                                truth,
                            )?;
                            b.ins().jump(done, &[]);
                            b.switch_to_block(slow);
                            runtime_call!(
                                &mut b,
                                callback_ref,
                                host,
                                registers,
                                HELPER,
                                location,
                                failed,
                            );
                            let value =
                                load(&mut b, registers, destination.unwrap(), 8, types::I64)?;
                            store_scalar_cached(
                                &mut b,
                                &scalars,
                                registers,
                                destination.unwrap(),
                                RAW_BOOL,
                                value,
                            )?;
                            b.ins().jump(done, &[]);
                            b.switch_to_block(done);
                        }
                        Operation::Call { .. } => runtime_call!(
                            &mut b,
                            callback_ref,
                            host,
                            registers,
                            HELPER,
                            location,
                            failed,
                        ),
                    }
                    if !feedback
                        && region_index.is_none()
                        && let Operation::FloatListLoad { view, .. }
                        | Operation::FloatListStore { view, .. }
                        | Operation::FloatListUnpack { view, .. } = &instruction.operation
                    {
                        let sample =
                            load_cached(&mut b, &scalars, registers, *view, 24, types::I64)?;
                        let record = b.create_block();
                        let done = b.create_block();
                        b.ins().brif(sample, record, &[], done, &[]);
                        b.switch_to_block(record);
                        runtime_call!(
                            &mut b,
                            callback_ref,
                            host,
                            registers,
                            FINISH,
                            location,
                            failed,
                        );
                        b.ins().jump(done, &[]);
                        b.switch_to_block(done);
                    }
                    // Arithmetic/constant results are primitive; eager diagnostics retain their finish callbacks.
                    if feedback
                        && destination.is_some_and(|register| {
                            owner.lir.registers[register.0] == lir::Type::Boxed
                        })
                        && !matches!(
                            instruction.operation,
                            Operation::Call { .. }
                                | Operation::SequenceUnpack { .. }
                                | Operation::SequenceGet { .. }
                        )
                    {
                        runtime_call!(
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
                            &scalars,
                            registers,
                            &[(*value, Register(owner.lir.registers.len()))],
                        )?;
                        let status = b.ins().iconst(types::I64, 0);
                        b.ins().return_(&[status]);
                    }
                    Terminator::Jump(edge) => {
                        edge_jump(
                            &mut b,
                            &scalars,
                            registers,
                            &owner.lir,
                            edge,
                            &blocks,
                            (!feedback).then_some((block_index, region_active.as_slice())),
                        )?;
                    }
                    Terminator::Branch { condition, yes, no } => {
                        let condition =
                            load_cached(&mut b, &scalars, registers, *condition, 8, types::I64)?;
                        let yes_block = b.create_block();
                        let no_block = b.create_block();
                        b.ins().brif(condition, yes_block, &[], no_block, &[]);
                        b.switch_to_block(yes_block);
                        edge_jump(
                            &mut b,
                            &scalars,
                            registers,
                            &owner.lir,
                            yes,
                            &blocks,
                            (!feedback).then_some((block_index, region_active.as_slice())),
                        )?;
                        b.switch_to_block(no_block);
                        edge_jump(
                            &mut b,
                            &scalars,
                            registers,
                            &owner.lir,
                            no,
                            &blocks,
                            (!feedback).then_some((block_index, region_active.as_slice())),
                        )?;
                    }
                }
            }
            b.switch_to_block(failed);
            let status = b.ins().iconst(types::I64, -1);
            b.ins().return_(&[status]);
            b.seal_all_blocks();
            b.finalize(module.target_config());
        }
        let clif = capture.then(|| context.func.display().to_string());
        module.define_function(id, &mut context).map_err(error)?;
        owner.bytes = context
            .compiled_code()
            .map_or(0, |code| code.code_buffer().len());
        module.finalize_definitions().map_err(error)?;
        let pointer = module.get_finalized_function(id);
        // SAFETY: the finalized function has exactly Entry's target C ABI and remains executable
        // until this NativeCode's final Rc is dropped. No pointer is exposed to callers.
        if let Some(clif) = clif {
            // SAFETY: the finalized module owns this complete function image until owner is dropped.
            let machine_code = unsafe { std::slice::from_raw_parts(pointer, owner.bytes) }.to_vec();
            owner.diagnostics = Some(NativeDiagnostics {
                clif,
                disassembly: context
                    .compiled_code()
                    .and_then(|c| c.vcode.clone())
                    .unwrap_or_default(),
                machine_code,
            });
        }
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
        if !self.feedback && self.lir.regions.iter().any(|r| r.body.0 == start) {
            return Err(Error::InvalidIr(
                "native region entry must pass through its header",
            ));
        }
        crate::native_runtime::execute(
            &self.lir,
            &self.locations,
            Some(&self.root_states),
            !self.feedback && !self.lir.regions.is_empty(),
            2,
            |context, registers, start| {
                // SAFETY: the shared bridge keeps its Context and register allocation alive for this call.
                unsafe { self.entry.unwrap()(context, registers, start) }
            },
            vm,
            function,
            code,
            base,
            frame_end,
            start,
            skip_probe,
        )
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
    scalars: &ScalarVars,
    registers: ir::Value,
    pairs: &[(Register, Register)],
) -> Result<()> {
    let mut copies = vec![];
    for &(from, to) in pairs {
        for field in [0, 8, 16, 24] {
            copies.push((
                to,
                field,
                load_cached(b, scalars, registers, from, field, types::I64)?,
            ));
        }
    }
    for (to, field, value) in copies {
        store_word_cached(b, scalars, registers, to, field, value)?;
    }
    Ok(())
}
fn edge_jump(
    b: &mut FunctionBuilder<'_>,
    scalars: &ScalarVars,
    registers: ir::Value,
    lir: &lir::Function,
    edge: &lir::Edge,
    blocks: &[ir::Block],
    region_edge: Option<(usize, &[Variable])>,
) -> Result<()> {
    if let Some((from, active)) = region_edge {
        for (index, r) in lir.regions.iter().enumerate() {
            if from == r.body.0 && edge.block == r.header {
                let meta = Register(lir.registers.len() + 1);
                let count = load(b, registers, meta, 24, types::I64)?;
                let next = b.ins().iadd_imm_s(count, 1);
                b.ins()
                    .store(MemFlagsData::new(), next, registers, offset(meta, 24)?);
            }
            if (from == r.header.0 || from == r.body.0)
                && edge.block != r.header
                && edge.block != r.body
            {
                let zero = b.ins().iconst(types::I64, 0);
                b.def_var(active[index], zero);
            }
        }
    }

    let pairs = edge
        .arguments
        .iter()
        .copied()
        .zip(lir.blocks[edge.block.0].parameters.iter().copied())
        .collect::<Vec<_>>();
    let mut values = vec![];
    let mut boxed = vec![];
    for (from, to) in pairs {
        if lir.registers[to.0] == lir::Type::F64 {
            values.push((to, load_cached(b, scalars, registers, from, 8, types::F64)?));
        } else {
            boxed.push((from, to));
        }
    }
    // Parallel SSA assignments preserve old values for swaps without publishing a slot image.
    copy_slots(b, scalars, registers, &boxed)?;
    for (to, value) in values {
        b.def_var(scalars.0[to.0].unwrap().0, value);
    }
    b.ins().jump(blocks[edge.block.0], &[]);
    Ok(())
}
// Tagged SSA preserves all Python variants, including unbound locals, without an eager slot image.
// Floats with an explicit unboxing recipe keep their smaller primitive representation.
struct ScalarVars(
    Vec<Option<(Variable, ir::Type)>>,
    Vec<Option<Variable>>,
    Vec<Option<[Variable; 4]>>,
);
impl ScalarVars {
    fn new(b: &mut FunctionBuilder<'_>, lir: &lir::Function, defer: bool) -> Self {
        let recipes = lir.native_boxing_recipes();
        Self(
            lir.registers
                .iter()
                .map(|ty| match ty {
                    lir::Type::F64 => Some((b.declare_var(types::F64), types::F64)),
                    lir::Type::I64 | lir::Type::Bool => {
                        Some((b.declare_var(types::I64), types::I64))
                    }
                    _ => None,
                })
                .collect(),
            (0..lir.registers.len())
                .map(|index| {
                    (defer && recipes.contains_key(&Register(index)))
                        .then(|| b.declare_var(types::F64))
                })
                .collect(),
            lir.registers
                .iter()
                .map(|ty| {
                    (defer && !lir.regions.is_empty() && *ty == lir::Type::Boxed)
                        .then(|| std::array::from_fn(|_| b.declare_var(types::I64)))
                })
                .collect(),
        )
    }
    fn materialize(
        &self,
        b: &mut FunctionBuilder<'_>,
        registers: ir::Value,
        values: impl IntoIterator<Item = Register>,
    ) -> Result<()> {
        for register in values
            .into_iter()
            .collect::<std::collections::BTreeSet<_>>()
        {
            if let Some(var) = self.1[register.0] {
                let value = b.use_var(var);
                let bits = b.ins().bitcast(types::I64, MemFlagsData::new(), value);
                store_scalar(b, registers, register, FLOAT, bits)?;
            } else if let Some(vars) = self.2[register.0] {
                for (field, var) in [0, 8, 16, 24].into_iter().zip(vars) {
                    let value = b.use_var(var);
                    b.ins().store(
                        MemFlagsData::new(),
                        value,
                        registers,
                        offset(register, field)?,
                    );
                }
            }
        }
        Ok(())
    }
    fn reload_callback(
        &self,
        b: &mut FunctionBuilder<'_>,
        registers: ir::Value,
        lir: &lir::Function,
        locations: &[(usize, usize)],
        event: i32,
        location: i32,
    ) -> Result<()> {
        if !matches!(event, HELPER | SEQUENCE | UNPACK_BATCH) {
            return Ok(());
        }
        let (block, index) = locations[location as usize];
        let count = if event == UNPACK_BATCH {
            match lir.blocks[block].instructions[index].operation {
                Operation::SequenceUnpack { count, .. } => count,
                _ => 0,
            }
        } else {
            0
        };
        for instruction in &lir.blocks[block].instructions[index..=index + count] {
            if let Some(r) = instruction.result
                && let Some(vars) = self.2[r.0]
            {
                for (field, var) in [0, 8, 16, 24].into_iter().zip(vars) {
                    let value = load(b, registers, r, field, types::I64)?;
                    b.def_var(var, value);
                }
            }
        }
        Ok(())
    }
    fn initialize(
        &self,
        b: &mut FunctionBuilder<'_>,
        registers: ir::Value,
        lir: &lir::Function,
    ) -> Result<()> {
        // OSR can bypass producers, so seed live primitive values and boxed recipes from the entry image.
        for (index, entry) in self.0.iter().enumerate() {
            let Some(&(var, ty)) = entry.as_ref() else {
                continue;
            };
            let zero = if lir.scalar_sources.contains_key(&Register(index)) {
                load(b, registers, Register(index), 8, types::F64)?
            } else if ty == types::F64 {
                b.ins().f64const(0.0)
            } else {
                b.ins().iconst(ty, 0)
            };
            b.def_var(var, zero);
        }
        for (index, vars) in self.2.iter().enumerate() {
            if let Some(vars) = vars {
                for (field, var) in [0, 8, 16, 24].into_iter().zip(vars) {
                    let value = load(b, registers, Register(index), field, types::I64)?;
                    b.def_var(*var, value);
                }
            }
        }
        for (index, var) in self.1.iter().enumerate() {
            if let Some(var) = var {
                let value = load(b, registers, Register(index), 8, types::F64)?;
                b.def_var(*var, value);
            }
        }
        Ok(())
    }
}
fn load_cached(
    b: &mut FunctionBuilder<'_>,
    scalars: &ScalarVars,
    registers: ir::Value,
    register: Register,
    field: i32,
    ty: ir::Type,
) -> Result<ir::Value> {
    if let Some(var) = scalars.1.get(register.0).copied().flatten() {
        if field == 0 {
            return Ok(b.ins().iconst(types::I64, FLOAT as i64));
        }
        if field == 8 {
            let value = b.use_var(var);
            return Ok(if ty == types::F64 {
                value
            } else {
                b.ins().bitcast(ty, MemFlagsData::new(), value)
            });
        }
    }
    if scalars.1.get(register.0).copied().flatten().is_some() && matches!(field, 16 | 24) {
        return Ok(b.ins().iconst(ty, 0));
    }
    if let Some(vars) = scalars.2.get(register.0).copied().flatten() {
        let value = b.use_var(vars[field as usize / 8]);
        return Ok(if ty == types::I64 {
            value
        } else {
            b.ins().bitcast(ty, MemFlagsData::new(), value)
        });
    }
    if field == 8
        && let Some((var, source)) = scalars.0.get(register.0).copied().flatten()
    {
        let value = b.use_var(var);
        return Ok(if source == ty {
            value
        } else {
            b.ins().bitcast(ty, MemFlagsData::new(), value)
        });
    }
    load(b, registers, register, field, ty)
}
fn store_word_cached(
    b: &mut FunctionBuilder<'_>,
    scalars: &ScalarVars,
    registers: ir::Value,
    r: Register,
    field: i32,
    value: ir::Value,
) -> Result<()> {
    if let Some(vars) = scalars.2.get(r.0).copied().flatten() {
        b.def_var(vars[field as usize / 8], value);
    } else {
        b.ins()
            .store(MemFlagsData::new(), value, registers, offset(r, field)?);
    }
    Ok(())
}
fn store_constant_cached(
    b: &mut FunctionBuilder<'_>,
    scalars: &ScalarVars,
    registers: ir::Value,
    r: Register,
    value: Slot,
) -> Result<()> {
    for (field, word) in [(0, value.tag), (8, value.a), (16, value.b), (24, value.c)] {
        let word = b.ins().iconst(types::I64, word as i64);
        store_word_cached(b, scalars, registers, r, field, word)?;
    }
    Ok(())
}
fn store_scalar_cached(
    b: &mut FunctionBuilder<'_>,
    scalars: &ScalarVars,
    registers: ir::Value,
    register: Register,
    tag: u64,
    value: ir::Value,
) -> Result<()> {
    if tag == FLOAT
        && let Some(var) = scalars.1[register.0]
    {
        let value = b.ins().bitcast(types::F64, MemFlagsData::new(), value);
        b.def_var(var, value);
        return Ok(());
    }
    if let Some((var, ty)) = scalars.0.get(register.0).copied().flatten() {
        let value = if ty == types::F64 {
            b.ins().bitcast(ty, MemFlagsData::new(), value)
        } else {
            value
        };
        b.def_var(var, value);
        return Ok(());
    }
    if scalars.2.get(register.0).copied().flatten().is_some() {
        let tag = b.ins().iconst(types::I64, tag as i64);
        store_word_cached(b, scalars, registers, register, 0, tag)?;
        store_word_cached(b, scalars, registers, register, 8, value)?;
        let zero = b.ins().iconst(types::I64, 0);
        store_word_cached(b, scalars, registers, register, 16, zero)?;
        store_word_cached(b, scalars, registers, register, 24, zero)?;
        return Ok(());
    }
    store_scalar(b, registers, register, tag, value)
}
fn store_float_cached(
    b: &mut FunctionBuilder<'_>,
    scalars: &ScalarVars,
    registers: ir::Value,
    register: Register,
    value: ir::Value,
) -> Result<()> {
    if let Some((var, ty)) = scalars.0.get(register.0).copied().flatten() {
        debug_assert_eq!(ty, types::F64);
        b.def_var(var, value);
        return Ok(());
    }
    store_float(b, registers, register, value)
}

fn state_values(state: &lir::FrameState) -> Vec<Register> {
    state
        .values
        .iter()
        .map(|(_, register)| *register)
        .chain(state.local_values.iter().flatten().copied())
        .collect()
}
fn callback_values(
    lir: &lir::Function,
    locations: &[(usize, usize)],
    event: i32,
    location: i32,
) -> Vec<Register> {
    if event == REGION_ENTER {
        return lir.regions[location as usize].imports.clone();
    }
    if event == ENTER {
        return lir.blocks[location as usize].parameters.clone();
    }
    let (block, index) = locations[location as usize];
    let instruction = &lir.blocks[block].instructions[index];
    if event == FINISH {
        // Reopened windows inspect only this HIR site's operands and result. Keeping unrelated
        // recipes live here would force spills even on the branch that skips observation.
        let mut inputs = lir.observation_inputs[instruction.origin.0].clone();
        inputs.extend(instruction.result);
        return inputs;
    }
    let mut values = instruction.operation.inputs();
    values.extend(&lir.observation_inputs[instruction.origin.0]);
    if let Operation::Call { state, effects, .. } | Operation::Truth { state, effects, .. } =
        instruction.operation
        && effects.needs_safepoint()
    {
        let floats = lir.native_boxing_recipes();
        values.extend(
            lir.states[state.0]
                .values
                .iter()
                .map(|(_, r)| *r)
                .filter(|r| !floats.contains_key(r)),
        );
    }
    values
}

fn callback_failure_values(
    lir: &lir::Function,
    locations: &[(usize, usize)],
    event: i32,
    location: i32,
) -> Vec<Register> {
    if event == REGION_ENTER {
        return state_values(
            &lir.states[lir.exits[lir.regions[location as usize].entry.0].state.0],
        );
    }
    if event == ENTER || event == FINISH {
        return vec![];
    }
    let (block, index) = locations[location as usize];
    let instruction = &lir.blocks[block].instructions[index];
    let state = match instruction.operation {
        Operation::FloatListView { failure, .. }
        | Operation::SequenceGet { failure, .. }
        | Operation::SequenceUnpack { failure, .. } => Some(lir.exits[failure.0].state),
        _ => None,
    };
    state.map_or_else(Vec::new, |state| state_values(&lir.states[state.0]))
}

#[cfg(test)]
mod region_entry_tests {
    use super::*;
    #[test]
    fn an_unprepared_region_body_is_rejected_before_native_execution() {
        let mut program = crate::compile(
            "def f(rows):\n    for xs in rows:\n        a,b=xs\n        xs[0]=a+1.0\n",
        )
        .unwrap();
        let mut vm = Vm::new();
        vm.tiering = crate::Tiering::Native;
        vm.thresholds.function_entries = 1;
        vm.thresholds.loop_backedges = 4;
        vm.profiling.window_ticks = 16;
        vm.run_program(&mut program).unwrap();
        let f = vm.get_global("f").unwrap();
        let xs = vm.heap.allocate(crate::Object::List(
            vec![Value::Float(1.0), Value::Float(2.0)].into(),
        ));
        let rows = vm.heap.allocate(crate::Object::List(vec![xs; 10].into()));
        for _ in 0..24 {
            vm.call(f, &[rows]).unwrap();
        }
        let hir = vm.function_hir(f).unwrap();
        let lir = Rc::new(hir.lower_lir(lir::LoweringMode::Profiled).unwrap());
        let body = lir.regions.first().unwrap().body.0;
        let code = hir.code.clone().unwrap();
        let function = Rc::new(RefCell::new(hir));
        let native = NativeCode::compile(lir, false, false).unwrap();
        assert!(matches!(
            native.execute(&mut vm, &function, &code, 0, 0, body, true),
            Err(Error::InvalidIr(_))
        ));
        assert_eq!(vm.stack_len(), 0);
    }
}
