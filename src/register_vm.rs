//! Register bytecode uses the same HIR slots, operations, feedback and tier handoff as stack bytecode.
use crate::{
    Error, Object, Result, Value, Vm,
    bytecode::{Code, register::Opcode},
    hir::{Function, ValueId},
    runtime,
};
use std::{cell::RefCell, rc::Rc};

impl Vm {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn execute_register(
        &mut self,
        function: &Rc<RefCell<Function>>,
        code: &Code,
        base: usize,
        frame_end: usize,
        mut ip: usize,
        mut previous: Option<usize>,
    ) -> Result<Value> {
        let register = &code.register;
        let mut collecting = self.collecting(function);
        let mut arguments = Vec::with_capacity(register.max_arguments.min(8));
        let mut transfers = Vec::with_capacity(register.max_transfer);
        loop {
            let instruction = *register
                .body
                .get(ip)
                .ok_or(Error::InvalidIr("register instruction pointer"))?;
            ip += 1;
            let a = instruction.a as usize;
            let b = instruction.b as usize;
            let c = instruction.c as usize;
            let op = instruction.op;
            match op {
                Opcode::EnterBlock => {
                    let backedge = self.record_block(function, code, a, previous)?;
                    collecting = self.collecting(function);
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
                    self.maybe_collect()?;
                    continue;
                }
                Opcode::Jump | Opcode::Branch => {
                    let edge = if matches!(op, Opcode::Jump) {
                        a
                    } else if runtime::truth(&self.heap, self.read(base, ValueId(a))?)? {
                        b
                    } else {
                        c
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
                    for ((destination, _), value) in edge.pairs.iter().zip(&transfers) {
                        self.write(base, *destination, *value)?;
                    }
                    ip = edge.target;
                    continue;
                }
                Opcode::Return => return self.read(base, ValueId(a)),
                _ => {}
            }
            let destination = ValueId(instruction.dst as usize);
            let mut observed =
                collecting && self.observe_site(function, code, base, destination)?;
            let result = match op {
                Opcode::Constant => match code
                    .constants
                    .get(a)
                    .ok_or(Error::InvalidIr("register constant"))?
                {
                    crate::hir::Constant::Unbound => Value::Unbound,
                    crate::hir::Constant::None => Value::None,
                    crate::hir::Constant::Bool(value) => Value::Bool(*value),
                    crate::hir::Constant::Int(value) => Value::Int(*value),
                    crate::hir::Constant::Float(value) => Value::Float(*value),
                    crate::hir::Constant::String(value) => {
                        self.heap.allocate(Object::String(value.clone()))
                    }
                },
                Opcode::Add => {
                    let lhs = self.read(base, ValueId(a))?;
                    let rhs = self.read(base, ValueId(b))?;
                    let (result, force) = self.adaptive_add(function, destination, lhs, rhs)?;
                    if force {
                        collecting = self.collecting(function);
                        observed = true;
                    }
                    result
                }
                Opcode::Subtract => {
                    runtime::subtract(self.read(base, ValueId(a))?, self.read(base, ValueId(b))?)?
                }
                Opcode::Less => {
                    runtime::less(self.read(base, ValueId(a))?, self.read(base, ValueId(b))?)?
                }
                Opcode::GetItem => self
                    .heap
                    .get_item(self.read(base, ValueId(a))?, self.read(base, ValueId(b))?)?,
                Opcode::SetItem => {
                    self.heap.set_item(
                        self.read(base, ValueId(a))?,
                        self.read(base, ValueId(b))?,
                        self.read(base, ValueId(c))?,
                    )?;
                    Value::None
                }
                Opcode::Len => Value::Int(
                    i64::try_from(self.heap.len(self.read(base, ValueId(a))?)?)
                        .map_err(|_| Error::IntegerOverflow)?,
                ),
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
                Opcode::Runtime | Opcode::List | Opcode::Tuple | Opcode::Dict => {
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
                        Opcode::List => self.heap.allocate(Object::List(arguments.clone())),
                        Opcode::Tuple => self.heap.allocate(Object::Tuple(arguments.clone())),
                        Opcode::Dict => {
                            let dict = self.heap.allocate(Object::Dict(vec![]));
                            for pair in arguments.as_chunks::<2>().0 {
                                self.heap.set_item(dict, pair[0], pair[1])?;
                            }
                            dict
                        }
                        _ => {
                            let (operation, count) = code
                                .operations
                                .get(a)
                                .ok_or(Error::InvalidIr("register runtime operation"))?;
                            if *count != arguments.len() {
                                return Err(Error::InvalidIr("register runtime arity"));
                            }
                            let roots = self.transient_roots.len();
                            self.transient_roots.extend_from_slice(&arguments);
                            let result = self.operation(operation, &arguments);
                            self.transient_roots.truncate(roots);
                            result?
                        }
                    }
                }
                Opcode::EnterBlock | Opcode::Jump | Opcode::Branch | Opcode::Return => {
                    unreachable!()
                }
            };
            self.write(base, destination, result)?;
            if observed {
                function.borrow().profile.borrow_mut().sites[destination.0]
                    .result
                    .observe(crate::profile::ValueKind::of(&self.heap, result));
            }
            self.maybe_collect()?;
        }
    }
}
