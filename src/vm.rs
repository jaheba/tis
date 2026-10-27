use crate::{
    Error, Heap, Object, Result, Value,
    bytecode::{Code, Opcode, read_operand},
    ir::{AddFeedback, AddStrategy, Constant, Function, ValueId},
    runtime,
};
use std::{cell::RefCell, rc::Rc};

#[derive(Debug, Default)]
pub struct Vm {
    pub heap: Heap,
    stack: Vec<Option<Value>>,
    pub(crate) environments: Vec<Value>,
    pub(crate) globals: Option<Value>,
    pub output: String,
    pub(crate) depth: usize,
    pub(crate) transient_roots: Vec<Value>,
    pinned: std::collections::BTreeMap<usize, Value>,
    next_root: usize,
    next_collection: usize,
}

impl Vm {
    pub fn new() -> Self {
        Self::default()
    }

    /// Frame values occupy one stack window; both return and error release that window.
    pub fn run(&mut self, function: &mut Function, arguments: &[Value]) -> Result<Value> {
        let handle = Rc::new(RefCell::new(function.clone()));
        let parent = self.ensure_globals();
        let result = self.invoke_function(handle.clone(), parent, &[], arguments, &[]);
        *function = handle.borrow().clone();
        result
    }

    pub fn run_program(&mut self, program: &mut crate::Program) -> Result<Value> {
        let module = program
            .function_mut("<module>")
            .ok_or(Error::InvalidIr("missing module"))?;
        self.run(module, &[])
    }

    pub(crate) fn execute_frame(
        &mut self,
        function: Rc<RefCell<Function>>,
        environment: Value,
        arguments: &[Value],
    ) -> Result<Value> {
        if self.depth >= 256 {
            return Err(Error::Runtime(
                "RecursionError: maximum recursion depth exceeded".into(),
            ));
        }
        let code = function.borrow_mut().bytecode()?.clone();
        let parameters = function
            .borrow()
            .parameters(function.borrow().entry())
            .to_vec();
        let base = self.stack.len();
        let frame_end = base + code.slots;
        self.stack.resize(frame_end, None);
        for (&parameter, &value) in parameters.iter().zip(arguments) {
            self.stack[base + parameter.0] = Some(value);
        }
        self.depth += 1;
        self.environments.push(environment);
        // Runtime calls currently recurse through Rust; grow before entering another frame
        // so the language recursion limit is reached before exhausting the native stack.
        let result = stacker::maybe_grow(128 * 1024, 2 * 1024 * 1024, || {
            self.execute(&function, &code, base, frame_end)
        });
        self.environments.pop();
        self.depth -= 1;
        self.stack.truncate(base);
        result
    }

    pub fn collect_garbage(&mut self, external_roots: &[Value]) -> Result<usize> {
        let roots = self
            .stack
            .iter()
            .filter_map(|value| *value)
            .chain(self.environments.iter().copied())
            .chain(self.transient_roots.iter().copied())
            .chain(self.pinned.values().copied())
            .chain(self.globals)
            .chain(external_roots.iter().copied())
            .collect::<Vec<_>>();
        self.heap.collect(&roots)
    }

    /// Pin a host-held handle across automatic collections until `unpin` is called.
    pub fn pin(&mut self, value: Value) -> Result<usize> {
        if matches!(value, Value::Ref(_)) {
            self.heap.get(value)?;
        }
        let root = self.next_root;
        self.next_root = root.checked_add(1).ok_or(Error::IntegerOverflow)?;
        self.pinned.insert(root, value);
        Ok(root)
    }

    pub fn unpin(&mut self, root: usize) -> Option<Value> {
        self.pinned.remove(&root)
    }

    pub fn stack_len(&self) -> usize {
        self.stack.len()
    }

    fn execute(
        &mut self,
        function: &Rc<RefCell<Function>>,
        code: &Code,
        base: usize,
        frame_end: usize,
    ) -> Result<Value> {
        let mut ip = 0;
        loop {
            let opcode = Opcode::decode(
                *code
                    .body
                    .get(ip)
                    .ok_or(Error::InvalidIr("invalid instruction pointer"))?,
            )?;
            ip += 1;
            match opcode {
                Opcode::Constant => {
                    let index = read_operand(&code.body, &mut ip)?;
                    let value = match code
                        .constants
                        .get(index)
                        .ok_or(Error::InvalidIr("invalid constant"))?
                    {
                        Constant::None => Value::None,
                        Constant::Bool(value) => Value::Bool(*value),
                        Constant::Int(value) => Value::Int(*value),
                        Constant::Float(value) => Value::Float(*value),
                        Constant::String(value) => {
                            self.heap.allocate(Object::String(value.clone()))
                        }
                    };
                    self.push(value);
                }
                Opcode::Load => {
                    let slot = ValueId(read_operand(&code.body, &mut ip)?);
                    let value = self.read(base, slot)?;
                    self.push(value);
                }
                Opcode::Store => {
                    let slot = ValueId(read_operand(&code.body, &mut ip)?);
                    let value = self.pop(frame_end)?;
                    self.write(base, slot, value)?;
                }
                Opcode::Add => {
                    let site = ValueId(read_operand(&code.body, &mut ip)?);
                    let rhs = self.pop(frame_end)?;
                    let lhs = self.pop(frame_end)?;
                    let mut function = function.borrow_mut();
                    let feedback = function
                        .feedback_mut(site)
                        .ok_or(Error::InvalidIr("addition has no feedback site"))?;
                    let value = self.add(lhs, rhs, feedback)?;
                    self.push(value);
                }
                Opcode::Subtract | Opcode::Less => {
                    let rhs = self.pop(frame_end)?;
                    let lhs = self.pop(frame_end)?;
                    let value = if matches!(opcode, Opcode::Subtract) {
                        runtime::subtract(lhs, rhs)?
                    } else {
                        runtime::less(lhs, rhs)?
                    };
                    self.push(value);
                }
                Opcode::List | Opcode::Tuple | Opcode::Dict => {
                    let count = read_operand(&code.body, &mut ip)?;
                    let count = if matches!(opcode, Opcode::Dict) {
                        count
                            .checked_mul(2)
                            .ok_or(Error::InvalidIr("collection operand overflow"))?
                    } else {
                        count
                    };
                    if self.stack.len().saturating_sub(frame_end) < count {
                        return Err(Error::InvalidIr("operand stack underflow"));
                    }
                    let values = self
                        .stack
                        .split_off(self.stack.len() - count)
                        .into_iter()
                        .map(|value| value.ok_or(Error::InvalidIr("empty operand")))
                        .collect::<Result<Vec<_>>>()?;
                    let result = match opcode {
                        Opcode::List => self.heap.allocate(Object::List(values)),
                        Opcode::Tuple => self.heap.allocate(Object::Tuple(values)),
                        _ => {
                            let result = self.heap.allocate(Object::Dict(Vec::new()));
                            for pair in values.as_chunks::<2>().0 {
                                self.heap.set_item(result, pair[0], pair[1])?;
                            }
                            result
                        }
                    };
                    self.push(result);
                }
                Opcode::GetItem => {
                    let key = self.pop(frame_end)?;
                    let object = self.pop(frame_end)?;
                    let value = self.heap.get_item(object, key)?;
                    self.push(value);
                }
                Opcode::SetItem => {
                    let value = self.pop(frame_end)?;
                    let key = self.pop(frame_end)?;
                    let object = self.pop(frame_end)?;
                    self.heap.set_item(object, key, value)?;
                    self.push(Value::None);
                }
                Opcode::Len => {
                    let object = self.pop(frame_end)?;
                    let len = self.heap.len(object)?;
                    self.push(Value::Int(
                        i64::try_from(len).map_err(|_| Error::IntegerOverflow)?,
                    ));
                }
                Opcode::Jump => ip = read_operand(&code.body, &mut ip)?,
                Opcode::JumpIfFalse => {
                    let target = read_operand(&code.body, &mut ip)?;
                    let value = self.pop(frame_end)?;
                    if !runtime::truth(&self.heap, value)? {
                        ip = target;
                    }
                }
                Opcode::Runtime => {
                    let id = read_operand(&code.body, &mut ip)?;
                    let (operation, count) = code
                        .operations
                        .get(id)
                        .ok_or(Error::InvalidIr("runtime operation"))?;
                    if self.stack.len().saturating_sub(frame_end) < *count {
                        return Err(Error::InvalidIr("operand stack underflow"));
                    }
                    let values = self
                        .stack
                        .split_off(self.stack.len() - count)
                        .into_iter()
                        .map(|value| value.ok_or(Error::InvalidIr("empty operand")))
                        .collect::<Result<Vec<_>>>()?;
                    let roots = self.transient_roots.len();
                    self.transient_roots.extend_from_slice(&values);
                    let result = self.operation(operation, &values);
                    self.transient_roots.truncate(roots);
                    let result = result?;
                    self.push(result);
                }
                Opcode::Return => return self.pop(frame_end),
            }
            // Collect only between instructions, when all live temporaries are rooted.
            if self.heap.allocation_count() >= self.next_collection.max(4096) {
                self.collect_garbage(&[])?;
                self.next_collection = self.heap.allocation_count().saturating_mul(2).max(4096);
            }
        }
    }

    fn push(&mut self, value: Value) {
        self.stack.push(Some(value));
    }

    fn pop(&mut self, frame_end: usize) -> Result<Value> {
        if self.stack.len() <= frame_end {
            return Err(Error::InvalidIr("operand stack underflow"));
        }
        self.stack
            .pop()
            .flatten()
            .ok_or(Error::InvalidIr("empty operand"))
    }

    fn add(&mut self, lhs: Value, rhs: Value, feedback: &mut AddFeedback) -> Result<Value> {
        feedback.executions = feedback.executions.saturating_add(1);
        if feedback.strategy == AddStrategy::Integer {
            if let (Value::Int(lhs), Value::Int(rhs)) = (lhs, rhs)
                && let Some(value) = lhs.checked_add(rhs)
            {
                feedback.fast_paths = feedback.fast_paths.saturating_add(1);
                return Ok(Value::Int(value));
            }
            feedback.guard_failures = feedback.guard_failures.saturating_add(1);
            feedback.strategy = AddStrategy::Generic;
            feedback.consecutive_integers = 0;
        }
        let result = runtime::add(&mut self.heap, lhs, rhs);
        if matches!(
            (lhs, rhs, &result),
            (Value::Int(_), Value::Int(_), Ok(Value::Int(_)))
        ) {
            feedback.consecutive_integers += 1;
            if feedback.consecutive_integers == AddFeedback::WARMUP {
                feedback.strategy = AddStrategy::Integer;
            }
        } else {
            feedback.consecutive_integers = 0;
        }
        result
    }

    fn read(&self, base: usize, value: ValueId) -> Result<Value> {
        self.stack
            .get(base + value.0)
            .ok_or(Error::InvalidIr("invalid value"))?
            .ok_or(Error::Uninitialized(value.0))
    }

    fn write(&mut self, base: usize, value: ValueId, result: Value) -> Result<()> {
        *self
            .stack
            .get_mut(base + value.0)
            .ok_or(Error::InvalidIr("invalid value"))? = Some(result);
        Ok(())
    }
}
