//! Cranelift gateway with guarded heap views and explicit runtime materialization boundaries.
mod fields;
mod locals;
mod readonly;
mod region;
mod virtual_iter;
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
    pub readonly_clif: Option<String>,
    pub mutation_clif: Option<String>,
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
    inline_fields: bool,
    entry_locals: Vec<(Register, usize)>,
    inline_scratch: usize,
    pub diagnostics: Option<NativeDiagnostics>,
    pub nest: Option<crate::region::nest::LoopNest>,
    pub(crate) readonly: Option<readonly::Kernel>,
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
    pub fn compile(
        lir: Rc<lir::Function>,
        feedback: bool,
        capture: bool,
        allow_nest: bool,
    ) -> Result<Self> {
        lir.verify()?;
        if std::mem::size_of::<usize>() != 8 || std::mem::size_of::<Slot>() != 16 {
            return Err(error("native backend requires 64-bit pointers"));
        }
        // Region code exposes enough SSA to optimize; Cranelift's default skips its optimization pipeline.
        let mut jit = JITBuilder::with_flags(&[("opt_level", "speed")], default_libcall_names())
            .map_err(error)?;
        let gateway = if lir.inline_scopes.is_empty() {
            crate::native_runtime::callback_no_inline as *const u8
        } else {
            callback as *const u8
        };
        jit.symbol("tis_callback", gateway);
        jit.symbol(
            "tis_inline_preflight",
            crate::native_runtime::inline_preflight as *const u8,
        );
        jit.symbol(
            "tis_guarded_helper",
            crate::native_runtime::guarded_helper_callback as *const u8,
        );
        jit.symbol(
            "tis_field",
            crate::native_runtime::field_callback as *const u8,
        );
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
            lir: lir.clone(),
            locations: vec![],
            readonly: None,
            root_states,
            bytes: 0,
            feedback,
            inline_fields: !feedback
                && cfg!(target_endian = "little")
                && lir.blocks.iter().any(|block| {
                    block.instructions.iter().any(|instruction| {
                        matches!(
                            instruction.operation,
                            Operation::Call {
                                helper: lir::Helper::GetAttr(ref attribute),
                                ..
                            } if attribute.inline_instances
                        )
                    })
                }),
            entry_locals: if feedback {
                Vec::new()
            } else {
                let loads = lir.invariant_local_loads();
                // The descriptor walk and guarded entry setup need multiple loads to amortize.
                // Single-load functions keep the compact helper path.
                if loads.len() >= 2 { loads } else { Vec::new() }
            },
            inline_scratch: if feedback {
                0
            } else {
                crate::body_inline::scratch_slots(&lir)
            },
            diagnostics: None,
            nest: (!feedback && allow_nest)
                .then(|| lir.nest.clone())
                .flatten(),
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
        let guarded_helper_id = module
            .declare_function("tis_guarded_helper", Linkage::Import, &callback_sig)
            .map_err(error)?;
        let mut inline_sig = module.make_signature();
        inline_sig.params = vec![
            AbiParam::new(ptr),
            AbiParam::new(ptr),
            AbiParam::new(types::I32),
            AbiParam::new(ptr),
        ];
        inline_sig.returns.push(AbiParam::new(types::I32));
        let inline_id = module
            .declare_function("tis_inline_preflight", Linkage::Import, &inline_sig)
            .map_err(error)?;
        let field_id = module
            .declare_function("tis_field", Linkage::Import, &callback_sig)
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
            let field_ref = module.declare_func_in_func(field_id, b.func);
            let guarded_helper_ref = module.declare_func_in_func(guarded_helper_id, b.func);
            let inline_ref = module.declare_func_in_func(inline_id, b.func);
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
            let private_vars = owner
                .nest
                .as_ref()
                .map(|n| {
                    n.private_iterators
                        .iter()
                        .map(|_| {
                            std::array::from_fn::<_, 4, _>(|_| {
                                let v = b.declare_var(types::I64);
                                let zero = b.ins().iconst(types::I64, 0);
                                b.def_var(v, zero);
                                v
                            })
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let mut region_active = region_active;
            if owner.nest.is_some() {
                let v = b.declare_var(types::I64);
                let zero = b.ins().iconst(types::I64, 0);
                b.def_var(v, zero);
                region_active.push(v);
            }
            let nest_vars = owner.nest.as_ref().map(|n| {
                virtual_iter::NestVars::new(&mut b, n, &owner.lir, *region_active.last().unwrap())
            });
            let outer_region = owner.nest.as_ref().map(|n| crate::region::LoopRegion {
                header: n.header,
                body: n.tail,
                iterator: n.iterator,
                entry: n.entry,
                imports: n.imports.clone(),
                schema: None,
            });
            let power_ref = module.declare_func_in_func(power_id, b.func);
            let blocks = (0..owner.lir.blocks.len())
                .map(|_| b.create_block())
                .collect::<Vec<_>>();
            let block_callbacks = block_callback_targets(&owner.lir, owner.nest.as_ref());
            let failed = b.create_block();
            for (index, &target) in blocks.iter().enumerate() {
                if owner.nest.as_ref().is_some_and(|n| {
                    n.blocks.contains(&crate::hir::BlockId(index)) && n.header.0 != index
                }) {
                    continue;
                }
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
                    if let Some(vars) = &nest_vars {
                        vars.publish($builder, registers, &owner.lir)?;
                    }
                    let state = &owner.lir.states[owner.lir.exits[exit.0].state.0];
                    scalars.materialize(
                        $builder,
                        registers,
                        state_values(state, owner.lir.inline_roots(owner.lir.exits[exit.0].state)),
                    )?;
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
                let in_nest = owner
                    .nest
                    .as_ref()
                    .is_some_and(|n| n.blocks.contains(&crate::hir::BlockId(block_index)));
                if owner.lir.block_origins[block_index].is_none() {
                    // Synthetic CFG edges carry registers, not physical profiling events.
                } else if in_nest {
                    let n = owner.nest.as_ref().unwrap();
                    if n.header.0 == block_index {
                        let active = b.use_var(*region_active.last().unwrap());
                        let prepare = b.create_block();
                        let ready = b.create_block();
                        b.ins().brif(active, ready, &[], prepare, &[]);
                        b.switch_to_block(prepare);
                        runtime_call!(&mut b, region_ref, host, registers, NEST_ENTER, 0, failed);
                        nest_vars
                            .as_ref()
                            .unwrap()
                            .initialize(&mut b, registers, &owner.lir)?;
                        for (recipe, vars) in n.private_iterators.iter().zip(&private_vars) {
                            for (field, var) in [0, 8].into_iter().zip(vars) {
                                let value =
                                    load(&mut b, registers, recipe.result, field, types::I64)?;
                                b.def_var(*var, value);
                            }
                        }
                        let one = b.ins().iconst(types::I64, 1);
                        b.def_var(*region_active.last().unwrap(), one);
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
                } else if let Some(index) = region_index {
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
                } else if !feedback && !block_callbacks.contains(&block_index) {
                    // Frozen profiles need callbacks only at loop headers and region exits.
                    // Reentrant calls may reopen observation, so check the invocation's current flag.
                    let collecting = b.ins().load(
                        types::I8,
                        MemFlagsData::new(),
                        host,
                        std::mem::offset_of!(crate::native_runtime::Context<'_>, collecting) as i32,
                    );
                    let observe = b.create_block();
                    let quiet = b.create_block();
                    let ready = b.create_block();
                    b.ins().brif(collecting, observe, &[], quiet, &[]);
                    b.switch_to_block(observe);
                    runtime_call!(
                        &mut b,
                        callback_ref,
                        host,
                        registers,
                        ENTER,
                        block_index as i32,
                        failed
                    );
                    b.ins().jump(ready, &[]);
                    b.switch_to_block(quiet);
                    let block = b.ins().iconst(types::I64, block_index as i64);
                    b.ins().store(
                        MemFlagsData::new(),
                        block,
                        host,
                        std::mem::offset_of!(crate::native_runtime::Context<'_>, previous) as i32,
                    );
                    let zero = b.ins().iconst(types::I8, 0);
                    b.ins().store(
                        MemFlagsData::new(),
                        zero,
                        host,
                        std::mem::offset_of!(crate::native_runtime::Context<'_>, skip_probe) as i32,
                    );
                    let count = b.ins().load(
                        types::I64,
                        MemFlagsData::new(),
                        host,
                        std::mem::offset_of!(crate::native_runtime::Context<'_>, inline_block_hits)
                            as i32,
                    );
                    let count = b.ins().iadd_imm_s(count, 1);
                    b.ins().store(
                        MemFlagsData::new(),
                        count,
                        host,
                        std::mem::offset_of!(crate::native_runtime::Context<'_>, inline_block_hits)
                            as i32,
                    );
                    b.ins().jump(ready, &[]);
                    b.switch_to_block(ready);
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
                let bootstrap_reads = if !feedback {
                    crate::leaf_inline::bootstrap_reads(&body.instructions)
                } else {
                    Default::default()
                };
                let mut unpack_reads = 0;
                for (instruction_index, instruction) in body.instructions.iter().enumerate() {
                    let location = i32::try_from(owner.locations.len()).map_err(error)?;
                    owner.locations.push((block_index, instruction_index));
                    if unpack_reads > 0 {
                        unpack_reads -= 1;
                        continue;
                    }
                    if bootstrap_reads.contains(&instruction_index) {
                        continue;
                    }
                    let destination = instruction.result;
                    if in_nest
                        && let Operation::Call {
                            helper: lir::Helper::Python(crate::hir::CoreOp::Iter),
                            ..
                        } = &instruction.operation
                    {
                        let n = owner.nest.as_ref().unwrap();
                        let i = n
                            .private_iterators
                            .iter()
                            .position(|p| Some(p.result) == destination)
                            .ok_or(Error::InvalidIr("private iterator definition"))?;
                        for (field, var) in [0, 8].into_iter().zip(private_vars[i]) {
                            let value = b.use_var(var);
                            store_word_cached(
                                &mut b,
                                &scalars,
                                registers,
                                destination.unwrap(),
                                field,
                                value,
                            )?;
                        }
                        let mut emitter = region::Emitter {
                            b: &mut b,
                            scalars: &scalars,
                            registers,
                            lir: &owner.lir,
                            region: outer_region.as_ref().unwrap(),
                            nest_vars: nest_vars.as_ref(),
                        };
                        emitter.reset_private(destination.unwrap())?;
                        continue;
                    }
                    if region_index.is_some() || in_nest {
                        let plan = region_index
                            .map(|i| &owner.lir.regions[i])
                            .unwrap_or_else(|| outer_region.as_ref().unwrap());
                        let mut emitter = region::Emitter {
                            b: &mut b,
                            scalars: &scalars,
                            registers,
                            lir: &owner.lir,
                            region: plan,
                            nest_vars: if in_nest { nest_vars.as_ref() } else { None },
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
                            let tag =
                                load_cached(&mut b, &scalars, registers, *view, 0, types::I64)?;
                            let len = b.ins().ushr_imm_s(tag, VIEW_LENGTH_SHIFT as i64);
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
                        Operation::Copy(value) => copy_slots(
                            &mut b,
                            &scalars,
                            registers,
                            &[(*value, destination.unwrap())],
                        )?,
                        Operation::Constant(value) => {
                            let value = value
                                .immediate()
                                .ok_or_else(|| error("allocating constant"))?;
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
                        Operation::BoolNot(value) => {
                            let value =
                                load_cached(&mut b, &scalars, registers, *value, 8, types::I64)?;
                            let inverted = b.ins().icmp_imm_s(IntCC::Equal, value, 0);
                            let bits = b.ins().uextend(types::I64, inverted);
                            store_scalar_cached(
                                &mut b,
                                &scalars,
                                registers,
                                destination.unwrap(),
                                RAW_BOOL,
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
                        Operation::NumericAdd {
                            left,
                            right,
                            guard,
                            overflow,
                        } => {
                            let left_tag =
                                load_cached(&mut b, &scalars, registers, *left, 0, types::I64)?;
                            let right_tag =
                                load_cached(&mut b, &scalars, registers, *right, 0, types::I64)?;
                            let left_kind = b.ins().iadd_imm_s(left_tag, -(INT as i64));
                            let right_kind = b.ins().iadd_imm_s(right_tag, -(INT as i64));
                            let left_number =
                                b.ins()
                                    .icmp_imm_s(IntCC::UnsignedLessThanOrEqual, left_kind, 1);
                            let right_number =
                                b.ins()
                                    .icmp_imm_s(IntCC::UnsignedLessThanOrEqual, right_kind, 1);
                            let valid = b.ins().band(left_number, right_number);
                            side_exit!(&mut b, valid, *guard);
                            let left_bits =
                                load_cached(&mut b, &scalars, registers, *left, 8, types::I64)?;
                            let right_bits =
                                load_cached(&mut b, &scalars, registers, *right, 8, types::I64)?;
                            let left_int = b.ins().icmp_imm_s(IntCC::Equal, left_tag, INT as i64);
                            let right_int = b.ins().icmp_imm_s(IntCC::Equal, right_tag, INT as i64);
                            let integers = b.ins().band(left_int, right_int);
                            let ints = b.create_block();
                            let floats = b.create_block();
                            let done = b.create_block();
                            b.ins().brif(integers, ints, &[], floats, &[]);
                            b.switch_to_block(ints);
                            let sum = b.ins().iadd(left_bits, right_bits);
                            let x = b.ins().bxor(left_bits, sum);
                            let y = b.ins().bxor(right_bits, sum);
                            let both = b.ins().band(x, y);
                            let safe = b.ins().icmp_imm_s(IntCC::SignedGreaterThanOrEqual, both, 0);
                            side_exit!(&mut b, safe, *overflow);
                            store_scalar_cached(
                                &mut b,
                                &scalars,
                                registers,
                                destination.unwrap(),
                                INT,
                                sum,
                            )?;
                            b.ins().jump(done, &[]);
                            b.switch_to_block(floats);
                            // Convert integers only according to their actual tag; Float payloads retain all bits.
                            let left_as_int = b.ins().fcvt_from_sint(types::F64, left_bits);
                            let right_as_int = b.ins().fcvt_from_sint(types::F64, right_bits);
                            let left_as_float =
                                b.ins().bitcast(types::F64, MemFlagsData::new(), left_bits);
                            let right_as_float =
                                b.ins().bitcast(types::F64, MemFlagsData::new(), right_bits);
                            let left = b.ins().select(left_int, left_as_int, left_as_float);
                            let right = b.ins().select(right_int, right_as_int, right_as_float);
                            let sum = b.ins().fadd(left, right);
                            store_scalar_cached(
                                &mut b,
                                &scalars,
                                registers,
                                destination.unwrap(),
                                FLOAT,
                                sum,
                            )?;
                            b.ins().jump(done, &[]);
                            b.switch_to_block(done);
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
                        Operation::NumberCompare {
                            value,
                            integer,
                            operator,
                            failure,
                        } => {
                            let tag =
                                load_cached(&mut b, &scalars, registers, *value, 0, types::I64)?;
                            let kind = b.ins().iadd_imm_s(tag, -(INT as i64));
                            let numeric =
                                b.ins().icmp_imm_s(IntCC::UnsignedLessThanOrEqual, kind, 1);
                            side_exit!(&mut b, numeric, *failure);
                            let bits =
                                load_cached(&mut b, &scalars, registers, *value, 8, types::I64)?;
                            let (int_cc, float_cc) = match operator.as_str() {
                                "==" => (IntCC::Equal, FloatCC::Equal),
                                "!=" => (IntCC::NotEqual, FloatCC::NotEqual),
                                "<" => (IntCC::SignedLessThan, FloatCC::LessThan),
                                "<=" => (IntCC::SignedLessThanOrEqual, FloatCC::LessThanOrEqual),
                                ">" => (IntCC::SignedGreaterThan, FloatCC::GreaterThan),
                                ">=" => {
                                    (IntCC::SignedGreaterThanOrEqual, FloatCC::GreaterThanOrEqual)
                                }
                                _ => return Err(error("number comparison")),
                            };
                            // Int payloads compare without conversion; only the exact literal becomes f64.
                            let literal = b.ins().iconst(types::I64, *integer);
                            let int_result = b.ins().icmp(int_cc, bits, literal);
                            let float = b.ins().bitcast(types::F64, MemFlagsData::new(), bits);
                            let literal = b.ins().f64const(*integer as f64);
                            let float_result = b.ins().fcmp(float_cc, float, literal);
                            let is_int = b.ins().icmp_imm_s(IntCC::Equal, tag, INT as i64);
                            let result = b.ins().select(is_int, int_result, float_result);
                            let result = b.ins().uextend(types::I64, result);
                            store_scalar_cached(
                                &mut b,
                                &scalars,
                                registers,
                                destination.unwrap(),
                                RAW_BOOL,
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
                        Operation::Call {
                            helper: lir::Helper::Python(crate::hir::CoreOp::LoadLocal(_)),
                            ..
                        } if !feedback
                            && owner
                                .entry_locals
                                .iter()
                                .any(|(r, _)| Some(*r) == destination) =>
                        {
                            let done = b.create_block();
                            locals::probe(
                                &mut b,
                                &scalars,
                                host,
                                registers,
                                destination.unwrap(),
                                Register(
                                    owner.lir.registers.len()
                                        + 4
                                        + owner
                                            .nest
                                            .as_ref()
                                            .map_or(0, |n| n.edges(&owner.lir).len()),
                                ),
                                done,
                            )?;
                            runtime_call!(
                                &mut b,
                                callback_ref,
                                host,
                                registers,
                                HELPER,
                                location,
                                failed
                            );
                            b.ins().jump(done, &[]);
                            b.switch_to_block(done);
                        }
                        Operation::Call {
                            helper: lir::Helper::GetAttr(attribute),
                            arguments,
                            ..
                        } if !feedback => {
                            let done = b.create_block();
                            if attribute.inline_instances {
                                fields::load(
                                    &mut b,
                                    &scalars,
                                    host,
                                    registers,
                                    arguments[0],
                                    destination.unwrap(),
                                    attribute,
                                    done,
                                )?;
                            }
                            // The miss path retains full observation, allocation and GC handling.
                            // A successful field probe needs only its receiver published, not a root map.
                            scalars.materialize(&mut b, registers, arguments.iter().copied())?;
                            let event = b.ins().iconst(types::I32, HELPER as i64);
                            let loc = b.ins().iconst(types::I32, location as i64);
                            let call = b.ins().call(field_ref, &[host, registers, event, loc]);
                            let status = b.inst_results(call)[0];
                            let hit = b.create_block();
                            let miss = b.create_block();
                            let success = b.ins().icmp_imm_s(IntCC::Equal, status, 0);
                            b.ins().brif(success, hit, &[], miss, &[]);
                            b.switch_to_block(hit);
                            scalars.reload_callback(
                                &mut b,
                                registers,
                                &owner.lir,
                                &owner.locations,
                                HELPER,
                                location,
                            )?;
                            b.ins().jump(done, &[]);
                            b.switch_to_block(miss);
                            let fallback = b.create_block();
                            let retry = b.ins().icmp_imm_s(IntCC::Equal, status, 1);
                            b.ins().brif(retry, fallback, &[], failed, &[]);
                            b.switch_to_block(fallback);
                            runtime_call!(
                                &mut b,
                                callback_ref,
                                host,
                                registers,
                                HELPER,
                                location,
                                failed
                            );
                            b.ins().jump(done, &[]);
                            b.switch_to_block(done);
                        }
                        Operation::Call {
                            helper: lir::Helper::InlineCall { body, .. },
                            ..
                        } if !feedback && !in_nest && region_index.is_none() => {
                            // All guards finish before the copied readonly body. Misses execute the
                            // original Call once with full caller roots; no nested frame is needed.
                            scalars.materialize(
                                &mut b,
                                registers,
                                instruction.operation.inputs(),
                            )?;
                            let scratch_index = owner.lir.registers.len()
                                + 4
                                + usize::from(!owner.entry_locals.is_empty())
                                + owner.nest.as_ref().map_or(0, |n| n.edges(&owner.lir).len());
                            let scratch =
                                b.ins().iadd_imm_s(registers, (scratch_index * 16) as i64);
                            let site = b.ins().iconst(types::I32, location as i64);
                            let call = b.ins().call(inline_ref, &[host, registers, site, scratch]);
                            let status = b.inst_results(call)[0];
                            let fast = b.create_block();
                            let slow = b.create_block();
                            let dispatch = b.create_block();
                            let done = b.create_block();
                            let error = b.ins().icmp_imm_s(IntCC::SignedLessThan, status, 0);
                            b.ins().brif(error, failed, &[], dispatch, &[]);
                            b.switch_to_block(dispatch);
                            let hit = b.ins().icmp_imm_s(IntCC::Equal, status, 0);
                            b.ins().brif(hit, fast, &[], slow, &[]);
                            b.switch_to_block(fast);
                            let mut values: Vec<ir::Value> = Vec::with_capacity(body.nodes.len());
                            for (index, node) in body.nodes.iter().enumerate() {
                                use crate::body_inline::Node;
                                let value = match node {
                                    Node::Argument(_) | Node::Field(..) => b.ins().load(
                                        types::F64,
                                        MemFlagsData::new(),
                                        scratch,
                                        (index * 16 + 8) as i32,
                                    ),
                                    Node::Constant(value) => b.ins().f64const(*value),
                                    Node::Add(a, c) => b.ins().fadd(values[*a], values[*c]),
                                    Node::Subtract(a, c) => b.ins().fsub(values[*a], values[*c]),
                                    Node::Multiply(a, c) => b.ins().fmul(values[*a], values[*c]),
                                    Node::Compare(operator, a, c) => {
                                        let cc = match operator.as_str() {
                                            "<" => FloatCC::LessThan,
                                            "<=" => FloatCC::LessThanOrEqual,
                                            ">" => FloatCC::GreaterThan,
                                            ">=" => FloatCC::GreaterThanOrEqual,
                                            "==" => FloatCC::Equal,
                                            _ => FloatCC::NotEqual,
                                        };
                                        b.ins().fcmp(cc, values[*a], values[*c])
                                    }
                                    Node::Not(a) => b.ins().icmp_imm_s(IntCC::Equal, values[*a], 0),
                                    Node::Select(condition, yes, no) => b.ins().select(
                                        values[*condition],
                                        values[*yes],
                                        values[*no],
                                    ),
                                    Node::Less(a, c) => {
                                        b.ins().fcmp(FloatCC::LessThan, values[*a], values[*c])
                                    }
                                };
                                values.push(value);
                            }
                            let (tag, bits) = if body.boolean {
                                (BOOL, b.ins().uextend(types::I64, values[body.result]))
                            } else {
                                (
                                    FLOAT,
                                    b.ins().bitcast(
                                        types::I64,
                                        MemFlagsData::new(),
                                        values[body.result],
                                    ),
                                )
                            };
                            store_scalar_cached(
                                &mut b,
                                &scalars,
                                registers,
                                destination.unwrap(),
                                tag,
                                bits,
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
                                failed
                            );
                            b.ins().jump(done, &[]);
                            b.switch_to_block(done);
                        }
                        Operation::Call {
                            helper: lir::Helper::Python(crate::hir::CoreOp::IsDone),
                            arguments,
                            ..
                        } if !feedback
                            && !in_nest
                            && region_index.is_none()
                            && arguments.len() == 1 =>
                        {
                            let tag = load_cached(
                                &mut b,
                                &scalars,
                                registers,
                                arguments[0],
                                0,
                                types::I64,
                            )?;
                            let kind = b.ins().band_imm_s(tag, KIND_MASK as i64);
                            let reference = b.ins().icmp_imm_s(IntCC::Equal, kind, REF as i64);
                            let fast = b.create_block();
                            let slow = b.create_block();
                            let done = b.create_block();
                            b.ins().brif(reference, slow, &[], fast, &[]);
                            b.switch_to_block(fast);
                            // Exhaustion is a heap sentinel; immediate values cannot represent it.
                            let no = b.ins().iconst(types::I64, 0);
                            store_scalar_cached(
                                &mut b,
                                &scalars,
                                registers,
                                destination.unwrap(),
                                BOOL,
                                no,
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
                                failed
                            );
                            b.ins().jump(done, &[]);
                            b.switch_to_block(done);
                        }
                        Operation::Call {
                            helper: lir::Helper::Python(crate::hir::CoreOp::Compare(operator)),
                            arguments,
                            ..
                        } if !feedback
                            && !in_nest
                            && region_index.is_none()
                            && arguments.len() == 2
                            && matches!(operator.as_str(), "==" | "!=") =>
                        {
                            let left = load_cached(
                                &mut b,
                                &scalars,
                                registers,
                                arguments[0],
                                0,
                                types::I64,
                            )?;
                            let right = load_cached(
                                &mut b,
                                &scalars,
                                registers,
                                arguments[1],
                                0,
                                types::I64,
                            )?;
                            let left_kind = b.ins().band_imm_s(left, KIND_MASK as i64);
                            let right_kind = b.ins().band_imm_s(right, KIND_MASK as i64);
                            let left_inline =
                                b.ins()
                                    .icmp_imm_s(IntCC::Equal, left_kind, INLINE_STRING as i64);
                            let right_inline =
                                b.ins()
                                    .icmp_imm_s(IntCC::Equal, right_kind, INLINE_STRING as i64);
                            let both = b.ins().band(left_inline, right_inline);
                            let fast = b.create_block();
                            let slow = b.create_block();
                            let done = b.create_block();
                            b.ins().brif(both, fast, &[], slow, &[]);
                            b.switch_to_block(fast);
                            // Both canonical words include length and every byte, including embedded NULs.
                            let left_bits = load_cached(
                                &mut b,
                                &scalars,
                                registers,
                                arguments[0],
                                8,
                                types::I64,
                            )?;
                            let right_bits = load_cached(
                                &mut b,
                                &scalars,
                                registers,
                                arguments[1],
                                8,
                                types::I64,
                            )?;
                            let tags_equal = b.ins().icmp(IntCC::Equal, left, right);
                            let bits_equal = b.ins().icmp(IntCC::Equal, left_bits, right_bits);
                            let equal = b.ins().band(tags_equal, bits_equal);
                            let value = if operator == "!=" {
                                b.ins().icmp_imm_s(IntCC::Equal, equal, 0)
                            } else {
                                equal
                            };
                            let value = b.ins().uextend(types::I64, value);
                            store_scalar_cached(
                                &mut b,
                                &scalars,
                                registers,
                                destination.unwrap(),
                                BOOL,
                                value,
                            )?;
                            b.ins().jump(done, &[]);
                            b.switch_to_block(slow);
                            // Retain the original helper and state for heap strings and every other type.
                            runtime_call!(
                                &mut b,
                                callback_ref,
                                host,
                                registers,
                                HELPER,
                                location,
                                failed
                            );
                            b.ins().jump(done, &[]);
                            b.switch_to_block(done);
                        }
                        Operation::Call { helper, .. }
                            if !feedback
                                && !in_nest
                                && region_index.is_none()
                                && (matches!(
                                    helper,
                                    lir::Helper::ScalarCall(_)
                                        | lir::Helper::Tuple
                                        | lir::Helper::LoadName(_)
                                        | lir::Helper::StoreName(_)
                                        | lir::Helper::StringNext
                                ) || matches!(helper, lir::Helper::Scoped(inner) if matches!(inner.as_ref(), lir::Helper::ScalarCall(_)))) =>
                        {
                            // Closed probes cannot collect or call Python; stores only replace an existing
                            // binding. Tuple allocation has a budget. Misses publish the original state.
                            let probe_event = match helper {
                                lir::Helper::Tuple => TUPLE_ALLOC,
                                lir::Helper::LoadName(_) => NAME_LOAD,
                                lir::Helper::StoreName(_) => NAME_STORE,
                                lir::Helper::StringNext => STRING_NEXT,
                                _ => SCALAR_CALL,
                            };
                            scalars.materialize(
                                &mut b,
                                registers,
                                instruction.operation.inputs(),
                            )?;
                            let event = b.ins().iconst(types::I32, probe_event as i64);
                            let site = b.ins().iconst(types::I32, location as i64);
                            let call = b
                                .ins()
                                .call(guarded_helper_ref, &[host, registers, event, site]);
                            let status = b.inst_results(call)[0];
                            let error = b.ins().icmp_imm_s(IntCC::SignedLessThan, status, 0);
                            let dispatch = b.create_block();
                            b.ins().brif(error, failed, &[], dispatch, &[]);
                            b.switch_to_block(dispatch);
                            let hit = b.ins().icmp_imm_s(IntCC::Equal, status, 0);
                            let fast = b.create_block();
                            let slow = b.create_block();
                            let done = b.create_block();
                            b.ins().brif(hit, fast, &[], slow, &[]);
                            b.switch_to_block(fast);
                            scalars.reload_callback(
                                &mut b,
                                registers,
                                &owner.lir,
                                &owner.locations,
                                probe_event,
                                location,
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
                                failed
                            );
                            b.ins().jump(done, &[]);
                            b.switch_to_block(done);
                        }
                        Operation::Call {
                            helper: lir::Helper::LeafGuard(_),
                            ..
                        } if !feedback
                            && crate::leaf_inline::bootstrap_end(
                                &body.instructions,
                                instruction_index,
                            )
                            .is_some() =>
                        {
                            runtime_call!(
                                &mut b,
                                callback_ref,
                                host,
                                registers,
                                LEAF_ENTER,
                                location,
                                failed,
                            )
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
                        && !in_nest
                        && region_index.is_none()
                        && let Operation::FloatListLoad { view, .. }
                        | Operation::FloatListStore { view, .. }
                        | Operation::FloatListUnpack { view, .. } = &instruction.operation
                    {
                        let tag = load_cached(&mut b, &scalars, registers, *view, 0, types::I64)?;
                        let sample = b.ins().band_imm_s(tag, VIEW_FEEDBACK as i64);
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
                        if in_nest
                            && owner
                                .nest
                                .as_ref()
                                .is_some_and(|n| !n.blocks.contains(&edge.block))
                        {
                            nest_vars
                                .as_ref()
                                .unwrap()
                                .publish(&mut b, registers, &owner.lir)?;
                        }
                        edge_jump(
                            &mut b,
                            &scalars,
                            registers,
                            &owner.lir,
                            edge,
                            &blocks,
                            (!feedback).then_some((
                                block_index,
                                region_active.as_slice(),
                                owner.nest.as_ref(),
                                nest_vars.as_ref(),
                            )),
                        )?;
                    }
                    Terminator::Branch { condition, yes, no } => {
                        let condition =
                            load_cached(&mut b, &scalars, registers, *condition, 8, types::I64)?;
                        let yes_block = b.create_block();
                        let no_block = b.create_block();
                        b.ins().brif(condition, yes_block, &[], no_block, &[]);
                        b.switch_to_block(yes_block);
                        if in_nest
                            && owner
                                .nest
                                .as_ref()
                                .is_some_and(|n| !n.blocks.contains(&yes.block))
                        {
                            nest_vars
                                .as_ref()
                                .unwrap()
                                .publish(&mut b, registers, &owner.lir)?;
                        }
                        edge_jump(
                            &mut b,
                            &scalars,
                            registers,
                            &owner.lir,
                            yes,
                            &blocks,
                            (!feedback).then_some((
                                block_index,
                                region_active.as_slice(),
                                owner.nest.as_ref(),
                                nest_vars.as_ref(),
                            )),
                        )?;
                        b.switch_to_block(no_block);
                        if in_nest
                            && owner
                                .nest
                                .as_ref()
                                .is_some_and(|n| !n.blocks.contains(&no.block))
                        {
                            nest_vars
                                .as_ref()
                                .unwrap()
                                .publish(&mut b, registers, &owner.lir)?;
                        }
                        edge_jump(
                            &mut b,
                            &scalars,
                            registers,
                            &owner.lir,
                            no,
                            &blocks,
                            (!feedback).then_some((
                                block_index,
                                region_active.as_slice(),
                                owner.nest.as_ref(),
                                nest_vars.as_ref(),
                            )),
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
                readonly_clif: None,
                mutation_clif: None,
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
        if self
            .nest
            .as_ref()
            .is_some_and(|n| n.blocks.contains(&crate::hir::BlockId(start)) && n.header.0 != start)
        {
            return Err(Error::InvalidIr(
                "native nest entry must pass through its header",
            ));
        }
        if !self.feedback && self.lir.regions.iter().any(|r| r.body.0 == start) {
            return Err(Error::InvalidIr(
                "native region entry must pass through its header",
            ));
        }
        if start == 0
            && !skip_probe
            && !vm.collecting(function)
            && let Some(kernel) = &self.readonly
            && let Some(outcome) = kernel.execute(vm, function)?
        {
            return Ok(outcome);
        }
        if self.inline_fields {
            vm.heap.enable_native_fields();
        }
        crate::native_runtime::execute(
            &self.lir,
            &self.locations,
            Some(&self.root_states),
            !self.feedback,
            &self.entry_locals,
            (!self.entry_locals.is_empty()).then(|| {
                self.lir.registers.len()
                    + 4
                    + self.nest.as_ref().map_or(0, |n| n.edges(&self.lir).len())
            }),
            self.nest.as_ref(),
            3 + usize::from(!self.entry_locals.is_empty())
                + self.nest.as_ref().map_or(0, |n| n.edges(&self.lir).len())
                + self.inline_scratch,
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
    i32::try_from(
        register
            .0
            .checked_mul(std::mem::size_of::<Slot>())
            .ok_or(Error::IntegerOverflow)?,
    )
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
        for field in [0, 8] {
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
type RegionEdge<'a> = (
    usize,
    &'a [Variable],
    Option<&'a crate::region::nest::LoopNest>,
    Option<&'a virtual_iter::NestVars>,
);

fn edge_jump(
    b: &mut FunctionBuilder<'_>,
    scalars: &ScalarVars,
    registers: ir::Value,
    lir: &lir::Function,
    edge: &lir::Edge,
    blocks: &[ir::Block],
    region_edge: Option<RegionEdge<'_>>,
) -> Result<()> {
    if let Some((from, active, nest, virtuals)) = region_edge {
        if let Some(n) = nest
            && n.blocks.contains(&crate::hir::BlockId(from))
        {
            for (i, (source, target)) in n.edges(lir).into_iter().enumerate() {
                if source.0 == from && target == edge.block {
                    let state = &virtuals.unwrap().iterators[i];
                    let count = b.use_var(state.count);
                    let next = b.ins().iadd_imm_s(count, 1);
                    b.def_var(state.count, next);
                }
            }
            if !n.blocks.contains(&edge.block) {
                let zero = b.ins().iconst(types::I64, 0);
                b.def_var(*active.last().unwrap(), zero);
            }
        } else {
            for (index, r) in lir.regions.iter().enumerate() {
                if from == r.body.0 && edge.block == r.header {
                    let meta = Register(lir.registers.len() + 2);
                    let count = load(b, registers, meta, 8, types::I64)?;
                    let next = b.ins().iadd_imm_s(count, 1);
                    b.ins()
                        .store(MemFlagsData::new(), next, registers, offset(meta, 8)?);
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
    Vec<Option<[Variable; 2]>>,
    std::collections::BTreeSet<usize>,
);
impl ScalarVars {
    fn new(b: &mut FunctionBuilder<'_>, lir: &lir::Function, defer: bool) -> Self {
        let recipes = lir.native_boxing_recipes();
        let copies = lir
            .blocks
            .iter()
            .flat_map(|b| &b.instructions)
            .filter_map(|i| {
                if let Operation::Copy(source) = i.operation {
                    Some((i.result.unwrap().0, source.0))
                } else {
                    None
                }
            })
            .collect::<std::collections::BTreeMap<_, _>>();
        let mut variables = std::collections::BTreeMap::new();
        let boxed = lir
            .registers
            .iter()
            .enumerate()
            .map(|(index, ty)| {
                if !defer
                    || lir.regions.is_empty()
                    || !matches!(ty, lir::Type::Boxed | lir::Type::FloatView)
                {
                    return None;
                }
                let mut source = index;
                while let Some(&next) = copies.get(&source) {
                    source = next;
                }
                Some(
                    *variables
                        .entry(source)
                        .or_insert_with(|| std::array::from_fn(|_| b.declare_var(types::I64))),
                )
            })
            .collect();
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
            boxed,
            copies.keys().copied().collect(),
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
                for (field, var) in [0, 8].into_iter().zip(vars) {
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
        if !matches!(
            event,
            HELPER
                | SEQUENCE
                | UNPACK_BATCH
                | FLOAT_VIEW
                | SCALAR_CALL
                | TUPLE_ALLOC
                | NAME_LOAD
                | NAME_STORE
                | LEAF_ENTER
        ) {
            return Ok(());
        }
        let (block, index) = locations[location as usize];
        let count = if event == LEAF_ENTER {
            crate::leaf_inline::bootstrap_end(&lir.blocks[block].instructions, index)
                .ok_or(Error::InvalidIr("native leaf bootstrap"))?
                - index
        } else if event == UNPACK_BATCH {
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
                for (field, var) in [0, 8].into_iter().zip(vars) {
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
            if self.3.contains(&index) {
                continue;
            }
            if let Some(vars) = vars {
                for (field, var) in [0, 8].into_iter().zip(vars) {
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
    for (field, word) in [(0, value.tag), (8, value.a)] {
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

fn state_values(state: &lir::FrameState, inline: &[Register]) -> Vec<Register> {
    state
        .values
        .iter()
        .map(|(_, register)| *register)
        .chain(state.local_values.iter().flatten().copied())
        .chain(inline.iter().copied())
        .collect()
}
fn callback_values(
    lir: &lir::Function,
    locations: &[(usize, usize)],
    event: i32,
    location: i32,
) -> Vec<Register> {
    if event == NEST_ENTER {
        return lir.nest.as_ref().unwrap().imports.clone();
    }
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
        let mut inputs = if lir.is_scoped_instruction(instruction) {
            vec![]
        } else {
            lir.observation_inputs[instruction.origin.0].clone()
        };
        inputs.extend(instruction.result);
        return inputs;
    }
    let mut values = instruction.operation.inputs();
    if !lir.is_scoped_instruction(instruction) {
        values.extend(&lir.observation_inputs[instruction.origin.0]);
    }
    if let Operation::Call { state, effects, .. } | Operation::Truth { state, effects, .. } =
        instruction.operation
        && effects.needs_safepoint()
    {
        let floats = lir.native_boxing_recipes();
        values.extend(lir.inline_gc_roots(state));
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
    if event == NEST_ENTER {
        let id = lir.exits[lir.nest.as_ref().unwrap().entry.0].state;
        return state_values(&lir.states[id.0], lir.inline_roots(id));
    }
    if event == REGION_ENTER {
        let id = lir.exits[lir.regions[location as usize].entry.0].state;
        return state_values(&lir.states[id.0], lir.inline_roots(id));
    }
    if event == ENTER || event == FINISH {
        return vec![];
    }
    let (block, index) = locations[location as usize];
    let instruction = &lir.blocks[block].instructions[index];
    let state = match instruction.operation {
        Operation::NumberCompare { failure, .. }
        | Operation::NumericAdd { guard: failure, .. }
        | Operation::FloatListView { failure, .. }
        | Operation::SequenceGet { failure, .. }
        | Operation::SequenceUnpack { failure, .. } => Some(lir.exits[failure.0].state),
        _ => None,
    };
    state.map_or_else(Vec::new, |state| {
        state_values(&lir.states[state.0], lir.inline_roots(state))
    })
}

/// OSR entries name every HIR backedge target, including forward-layout backedges.
/// A region exit must flush its deferred iterator/profile state before ordinary blocks run.
fn block_callback_targets(
    lir: &lir::Function,
    nest: Option<&crate::region::nest::LoopNest>,
) -> std::collections::BTreeSet<usize> {
    let mut targets = lir
        .osr_entries
        .iter()
        .map(|entry| entry.header.0)
        .collect::<std::collections::BTreeSet<_>>();
    for (index, block) in lir.blocks.iter().enumerate() {
        if !lir
            .regions
            .iter()
            .any(|r| r.header.0 == index || r.body.0 == index)
            && !nest.is_some_and(|n| n.blocks.contains(&crate::hir::BlockId(index)))
        {
            continue;
        }
        match &block.terminator {
            Terminator::Return(_) => {}
            Terminator::Jump(edge) => {
                targets.insert(edge.block.0);
            }
            Terminator::Branch { yes, no, .. } => {
                targets.insert(yes.block.0);
                targets.insert(no.block.0);
            }
        }
    }
    targets
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
        let native = NativeCode::compile(lir, false, false, false).unwrap();
        assert!(matches!(
            native.execute(&mut vm, &function, &code, 0, 0, body, true),
            Err(Error::InvalidIr(_))
        ));
        assert_eq!(vm.stack_len(), 0);
    }
}
