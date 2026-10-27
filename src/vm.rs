use crate::{
    Error, Heap, Object, Result, Value,
    bytecode::{Code, Opcode, read_operand},
    ir::{AddFeedback, AddStrategy, Constant, Function, ValueId},
    runtime,
};

#[derive(Debug, Default)]
pub struct Vm {
    pub heap: Heap,
    stack: Vec<Option<Value>>,
}

impl Vm {
    pub fn new() -> Self {
        Self::default()
    }

    /// Frame values occupy one stack window; both return and error release that window.
    pub fn run(&mut self, function: &mut Function, arguments: &[Value]) -> Result<Value> {
        let expected = function.parameters(function.entry()).len();
        if expected != arguments.len() {
            return Err(Error::Arguments {
                expected,
                actual: arguments.len(),
            });
        }
        for &value in arguments {
            if matches!(value, Value::Ref(_)) {
                self.heap.get(value)?;
            }
        }
        let code = match function.code.take() {
            Some(code) => code,
            None => Code::lower(function)?,
        };
        let base = self.stack.len();
        let frame_end = base + code.slots;
        self.stack.resize(frame_end, None);
        for (&parameter, &value) in function.parameters(function.entry()).iter().zip(arguments) {
            self.stack[base + parameter.0] = Some(value);
        }
        let result = self.execute(function, &code, base, frame_end);
        self.stack.truncate(base);
        function.code = Some(code);
        result
    }

    pub fn collect_garbage(&mut self, external_roots: &[Value]) -> Result<usize> {
        let roots = self
            .stack
            .iter()
            .filter_map(|value| *value)
            .chain(external_roots.iter().copied())
            .collect::<Vec<_>>();
        self.heap.collect(&roots)
    }

    pub fn stack_len(&self) -> usize {
        self.stack.len()
    }

    fn execute(
        &mut self,
        function: &mut Function,
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
                Opcode::Return => return self.pop(frame_end),
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
