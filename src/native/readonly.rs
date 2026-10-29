//! Closed regions execute generic LIR without callbacks or heap mutation.
//! Read-only recursion replays on failure. Acyclic mutation buffers ordered writes,
//! commits its completed prefix, and resumes the VM at the failed instruction.
use super::*;
use crate::{
    heap::{NativeFieldDirectory, NativeFields},
    hir::{self, CoreOp},
    lir::Helper,
};
use std::{
    collections::BTreeMap,
    mem::{offset_of, size_of},
};

const LEN: u64 = 20;
const SQRT: u64 = 21;
const MATH: u64 = 22;
mod emit;
mod mutation;
use emit::Emitter;

type KernelEntry = unsafe extern "C" fn(*mut Host, *const Slot, *mut Slot, u64) -> u64;

#[repr(C)]
struct Host {
    directory: NativeFieldDirectory,
    globals: *const Slot,
    class: Slot,
    shape: u64,
    miss_field: u64,
    receiver: Slot,
    registers: *mut Slot,
    writes: *mut mutation::Write,
    write_count: u64,
    state: u64,
    exit_reason: u64,
}

pub(crate) struct Kernel {
    lir: Rc<lir::Function>,
    source: hir::Function,
    globals: Vec<(Register, crate::Name)>,
    fields: Vec<(Register, crate::Attribute)>,
    entry: KernelEntry,
    arity: usize,
    mutation: bool,
    write_capacity: usize,
    scratch: RefCell<mutation::Scratch>,
}
impl fmt::Debug for Kernel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(if self.mutation {
            "MutationKernel"
        } else {
            "ReadonlyKernel"
        })
        .field("source", &self.lir.source)
        .finish()
    }
}

impl NativeCode {
    pub(crate) fn compile_regions(
        &mut self,
        source: &hir::Function,
        readonly: bool,
        mutation: bool,
    ) -> Result<()> {
        let plan = if mutation {
            Plan::build_mutation(source)?
        } else {
            None
        };
        let plan = match plan {
            Some(plan) => Some(plan),
            None if readonly => Plan::build(source)?,
            None => None,
        };
        let Some(plan) = plan else {
            return Ok(());
        };
        let module = self.module.as_mut().unwrap();
        let mut signature = module.make_signature();
        signature
            .params
            .extend((0..4).map(|_| AbiParam::new(types::I64)));
        signature.returns.push(AbiParam::new(types::I64));
        let id = module
            .declare_function("readonly_region", Linkage::Local, &signature)
            .map_err(error)?;
        let mut context = module.make_context();
        context.func.signature = signature;
        let self_ref = module.declare_func_in_func(id, &mut context.func);
        let mut frontend = FunctionBuilderContext::new();
        {
            let mut builder = FunctionBuilder::new(&mut context.func, &mut frontend);
            Emitter::emit(&mut builder, self_ref, &plan)?;
            builder.finalize(module.target_config());
        }
        if let Some(diagnostics) = &mut self.diagnostics {
            if plan.mutation {
                diagnostics.mutation_clif = Some(context.func.display().to_string());
            } else {
                diagnostics.readonly_clif = Some(context.func.display().to_string());
            }
        }
        module.define_function(id, &mut context).map_err(error)?;
        self.bytes += context
            .compiled_code()
            .map_or(0, |code| code.code_buffer().len());
        module.finalize_definitions().map_err(error)?;
        let pointer = module.get_finalized_function(id);
        // SAFETY: this module owns the function with exactly the four-word signature above.
        // NativeCode keeps the executable allocation alive throughout every invocation.
        let entry = unsafe { std::mem::transmute::<*const u8, KernelEntry>(pointer) };
        self.readonly = Some(Kernel {
            lir: plan.lir,
            source: source.clone(),
            globals: plan.globals,
            fields: plan.fields,
            entry,
            arity: plan.arity,
            mutation: plan.mutation,
            write_capacity: plan.write_capacity,
            scratch: RefCell::new(mutation::Scratch::default()),
        });
        Ok(())
    }
}

struct Plan {
    lir: Rc<lir::Function>,
    globals: Vec<(Register, crate::Name)>,
    fields: Vec<(Register, crate::Attribute)>,
    tuples: BTreeMap<Register, Vec<Register>>,
    recursive: BTreeMap<Register, Register>,
    methods: BTreeMap<Register, Register>,
    parameters: Vec<usize>,
    arity: usize,
    mutation: bool,
    write_capacity: usize,
}
impl Plan {
    fn build(source: &hir::Function) -> Result<Option<Self>> {
        if source.parameter_names.is_empty()
            || source.parameter_names.len() > 4
            || !source.keyword_only.is_empty()
            || source.vararg.is_some()
            || source.kwarg.is_some()
            || source.environment_required()
            || source.blocks.len() > 48
            || source.value_count > 512
        {
            return Ok(None);
        }
        let lir = Rc::new(lir::Function::lower_with_inline(
            source,
            lir::LoweringMode::Generic,
            false,
            false,
        )?);
        // Native recursion is bounded separately. CFG backedges are not admitted into this region.
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
                Terminator::Jump(e) => vec![e],
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
        let mut plan = Self {
            lir: lir.clone(),
            globals: vec![],
            fields: vec![],
            tuples: BTreeMap::new(),
            recursive: BTreeMap::new(),
            methods: BTreeMap::new(),
            parameters: vec![],
            arity: 0,
            mutation: false,
            write_capacity: 0,
        };
        for parameter in &source.parameter_names {
            let Some(slot) = source.local_slots.iter().position(|name| name == parameter) else {
                return Ok(None);
            };
            plan.parameters.push(slot);
        }
        // First identify direct tuple returns and recursive method tokens independently of block order.
        for block in &lir.blocks {
            for instruction in &block.instructions {
                let Some(result) = instruction.result else {
                    return Ok(None);
                };
                if let Operation::Call {
                    helper, arguments, ..
                } = &instruction.operation
                {
                    match helper {
                        Helper::Tuple => {
                            plan.tuples.insert(result, arguments.clone());
                        }
                        Helper::GetAttr(attribute) if attribute.text == source.name => {
                            if lir::temporary_uses(source, hir::ValueId(result.0)) != Some(1) {
                                return Ok(None);
                            }
                            plan.methods.insert(result, arguments[0]);
                        }
                        _ => {}
                    }
                }
            }
            if let Terminator::Return(value) = block.terminator {
                let Some(fields) = plan.tuples.get(&value) else {
                    return Ok(None);
                };
                if plan.arity != 0 && plan.arity != fields.len()
                    || fields.is_empty()
                    || fields.len() > 4
                {
                    return Ok(None);
                }
                plan.arity = fields.len();
            }
        }
        for block in &lir.blocks {
            for instruction in &block.instructions {
                let result = instruction.result.unwrap();
                match &instruction.operation {
                    Operation::Constant(hir::Constant::String(_)) => return Ok(None),
                    Operation::BoolNot(_)
                    | Operation::BoxBool(_)
                    | Operation::NumberCompare { .. }
                    | Operation::Constant(_)
                    | Operation::Truth { .. }
                    | Operation::SequenceUnpack { .. }
                    | Operation::SequenceGet {
                        index: lir::SequenceIndex::Constant(_),
                        ..
                    } => {}
                    Operation::Call {
                        helper, arguments, ..
                    } => match helper {
                        Helper::GetAttr(attribute) => {
                            if !plan.methods.contains_key(&result) {
                                plan.fields.push((result, attribute.clone()));
                            }
                        }
                        Helper::LoadName(name) => plan.globals.push((result, name.clone())),
                        Helper::Add
                        | Helper::Subtract
                        | Helper::Less
                        | Helper::GetItem
                        | Helper::Len => {}
                        Helper::Tuple => {
                            if lir::temporary_uses(source, hir::ValueId(result.0)) != Some(1) {
                                return Ok(None);
                            }
                        }
                        Helper::Python(
                            CoreOp::LoadLocal(_) | CoreOp::StoreLocal(_) | CoreOp::CheckLocal(_),
                        ) => {}
                        Helper::Python(CoreOp::Unary(op)) if op == "not" => {}
                        Helper::Python(CoreOp::Binary(op) | CoreOp::Inplace(op))
                            if ["+", "-", "*", "/"].contains(&op.as_str()) => {}
                        Helper::Python(CoreOp::Compare(op))
                            if ["==", "!=", "<", ">", "<=", ">=", "is", "is not"]
                                .contains(&op.as_str()) => {}
                        Helper::Python(CoreOp::Call(names))
                            if names.iter().all(Option::is_none) =>
                        {
                            if let Some(receiver) = plan.methods.get(&arguments[0]) {
                                if arguments.len() != source.parameter_names.len() {
                                    return Ok(None);
                                }
                                plan.recursive.insert(result, *receiver);
                            } else {
                                let Some(producer) = lir
                                    .blocks
                                    .iter()
                                    .flat_map(|b| &b.instructions)
                                    .find(|i| i.result == Some(arguments[0]))
                                else {
                                    return Ok(None);
                                };
                                if arguments.len() != 2
                                    || !matches!(&producer.operation,Operation::Call {helper:Helper::LoadName(name),..} if name.text=="len")
                                        && !matches!(&producer.operation,Operation::Call {helper:Helper::GetAttr(attribute),..} if attribute.text=="sqrt")
                                {
                                    return Ok(None);
                                }
                            }
                        }
                        Helper::Python(CoreOp::Unpack(_) | CoreOp::UnpackItem(_)) => {}
                        _ => return Ok(None),
                    },
                    _ => return Ok(None),
                }
            }
        }
        // Calls and their unpack tokens are virtual tuples; they never escape into locals or phis.
        let mut next = lir.registers.len();
        for &result in plan.recursive.keys() {
            if lir::temporary_uses(source, hir::ValueId(result.0)) != Some(1) {
                return Ok(None);
            }
            plan.tuples.insert(
                result,
                (0..plan.arity)
                    .map(|_| {
                        let r = Register(next);
                        next += 1;
                        r
                    })
                    .collect(),
            );
        }
        for block in &lir.blocks {
            for instruction in &block.instructions {
                let unpack = match &instruction.operation {
                    Operation::Call {
                        helper: Helper::Python(CoreOp::Unpack(count)),
                        arguments,
                        ..
                    } => Some((arguments[0], *count)),
                    Operation::SequenceUnpack { value, count, .. } => Some((*value, *count)),
                    _ => None,
                };
                if let Some((value, count)) = unpack {
                    let Some(fields) = plan.tuples.get(&value).cloned() else {
                        return Ok(None);
                    };
                    if fields.len() != count
                        || lir::temporary_uses(source, hir::ValueId(instruction.result.unwrap().0))
                            != Some(count)
                    {
                        return Ok(None);
                    }
                    plan.tuples.insert(instruction.result.unwrap(), fields);
                }
            }
        }
        for instruction in lir.blocks.iter().flat_map(|b| &b.instructions) {
            let item = match &instruction.operation {
                Operation::SequenceGet {
                    value,
                    index: lir::SequenceIndex::Constant(index),
                    ..
                } => Some((*value, *index)),
                Operation::Call {
                    helper: Helper::Python(CoreOp::UnpackItem(index)),
                    arguments,
                    ..
                } => Some((arguments[0], *index as i64)),
                _ => None,
            };
            if let Some((value, index)) = item
                && (index < 0
                    || plan
                        .tuples
                        .get(&value)
                        .is_none_or(|fields| index as usize >= fields.len()))
            {
                return Ok(None);
            }
        }
        if plan.recursive.is_empty() {
            return Ok(None);
        }
        Ok(Some(plan))
    }
}

impl Kernel {
    pub(crate) fn execute(
        &self,
        vm: &mut Vm,
        function: &Rc<RefCell<hir::Function>>,
    ) -> Result<Option<Outcome>> {
        vm.maybe_collect()?;
        let locals = vm.local_values()?;
        let arguments = self
            .source
            .parameter_names
            .iter()
            .map(|name| {
                locals[self
                    .source
                    .local_slots
                    .iter()
                    .position(|n| n == name)
                    .unwrap()]
                .map(Slot::boxed)
            })
            .collect::<Option<Vec<_>>>();
        let Some(arguments) = arguments else {
            return Ok(None);
        };
        let receiver = arguments[0].value()?;
        let Ok(crate::Object::Instance { class, attributes }) = vm.heap.get(receiver) else {
            return Ok(None);
        };
        let class = *class;
        if attributes.get(&self.source.name).is_some() {
            return Ok(None);
        }
        let shape = attributes.native_fields().0;
        let Ok(Some(method)) = vm.class_attribute(class, &self.source.name) else {
            return Ok(None);
        };
        let Ok(crate::Object::Function(closure)) = vm.heap.get(method) else {
            return Ok(None);
        };
        let callee = closure.function.borrow();
        if callee.id() != self.source.id()
            || callee.revision() != self.source.revision()
            || !closure.defaults.is_empty()
            || vm.environments.last() != Some(&closure.environment)
        {
            return Ok(None);
        }
        drop(callee);
        let mut globals = vec![Slot::default(); self.globals.len()];
        for (index, (_, name)) in self.globals.iter().enumerate() {
            let Ok(value) = vm.load_name_slot(&name.text, &name.cache) else {
                return Ok(None);
            };
            globals[index] = match vm.heap.get(value) {
                Ok(crate::Object::Native(name)) if name == "len" => Slot { tag: LEN, a: 0 },
                Ok(crate::Object::Native(name)) if name == "math" => Slot { tag: MATH, a: 0 },
                _ => Slot::boxed(value),
            };
        }
        vm.heap.enable_native_fields();
        // Regions never reenter Rust. Scratch retains weak bits, not GC roots. Exit spills refresh
        // every reconstructed value; write_count publishes only entries written by this invocation.
        let mut scratch = self.scratch.borrow_mut();
        if self.mutation {
            scratch
                .registers
                .resize(self.lir.registers.len(), Slot { tag: UNBOUND, a: 0 });
            scratch
                .writes
                .resize(self.write_capacity, mutation::Write::default());
        }
        let mut host = Host {
            directory: vm.heap.native_fields.get(),
            globals: globals.as_ptr(),
            class: Slot::boxed(class),
            shape,
            miss_field: u64::MAX,
            receiver: Slot::default(),
            registers: scratch.registers.as_mut_ptr(),
            writes: scratch.writes.as_mut_ptr(),
            write_count: 0,
            state: 0,
            exit_reason: lir::ExitReason::TypeGuard as u64,
        };
        let mut output = [Slot::default(); 4];
        // SAFETY: the verified region reads bounds/generation/shape-guarded heap views only.
        // It cannot call Rust/Python, mutate the heap, allocate or collect. All input owners remain
        // rooted by the suspended VM frame; stack buffers and executable code outlive this call.
        let status = unsafe {
            (self.entry)(
                &mut host,
                arguments.as_ptr(),
                output.as_mut_ptr(),
                (256 - vm.depth.min(256)).min(32) as u64,
            )
        };
        if self.mutation {
            let result = self.finish_mutation(
                vm,
                function,
                &host,
                &scratch.registers,
                &scratch.writes,
                status,
            );
            return result;
        }
        if status != 0 {
            vm.tier_stats.readonly_misses += 1;
            if let Some((_, attribute)) = self.fields.get(host.miss_field as usize)
                && let Ok(receiver) = host.receiver.value()
                && let Ok(crate::Object::Instance { attributes, .. }) = vm.heap.get(receiver)
            {
                attributes.read(attribute);
            }
            return Ok(None);
        }
        vm.tier_stats.readonly_entries += 1;
        let values = output[..self.arity]
            .iter()
            .map(|v| v.value())
            .collect::<Result<Vec<_>>>()?;
        Ok(Some(Outcome::Return(
            vm.heap.allocate(crate::Object::Tuple(values)),
        )))
    }
}
