//! Guarded, non-reentrant execution of small straight-line HIR expressions.
use super::{ScalarBuiltin, compare, integer, number};
use crate::{
    Error, Object, Result, StringLiteral, Value, Vm,
    hir::{Constant, CoreOp, Function, Op, Terminator},
};
use std::{cell::RefCell, rc::Rc};

#[derive(Debug)]
pub(crate) struct Plan {
    operations: Vec<(usize, Step)>,
    result: usize,
    values: usize,
}
#[derive(Debug)]
enum Step {
    Literal(StringLiteral),
    Operation(Op),
}
impl Plan {
    fn compile(function: &Function) -> Option<Self> {
        if function.module
            || !function.parameter_names.is_empty()
            || !function.local_names.is_empty()
            || !function.global_names.is_empty()
            || !function.nonlocal_names.is_empty()
            || function.blocks.len() != 1
            || !function.blocks[0].parameters.is_empty()
            || function.value_count > 128
            || function.validated_backedges().is_err()
        {
            return None;
        }
        let Some(Terminator::Return(result)) = function.blocks[0].terminator else {
            return None;
        };
        let mut operations = vec![];
        for instruction in &function.blocks[0].instructions {
            let step = match &instruction.op {
                Op::Constant(Constant::String(text)) => Step::Literal(text.clone().into()),
                Op::Constant(Constant::Unbound) => return None,
                Op::Constant(_)
                | Op::Add(..)
                | Op::Subtract(..)
                | Op::Less(..)
                | Op::GetItem(..)
                | Op::Len(_) => Step::Operation(instruction.op.clone()),
                Op::Runtime(CoreOp::LoadName(_) | CoreOp::Compare(_) | CoreOp::Unary(_), _) => {
                    Step::Operation(instruction.op.clone())
                }
                Op::Runtime(CoreOp::Call(names), inputs)
                    if names.len() <= 4
                        && inputs.len() == names.len() + 1
                        && names.iter().all(Option::is_none) =>
                {
                    Step::Operation(instruction.op.clone())
                }
                _ => return None,
            };
            operations.push((instruction.result.0, step));
        }
        Some(Self {
            operations,
            result: result.0,
            values: function.value_count,
        })
    }
}

impl Vm {
    pub(crate) fn generator_expression(&mut self, callable: Value) -> Result<Value> {
        let Object::Function(closure) = self.heap.get(callable)? else {
            return self.call(callable, &[]);
        };
        let (function, environment) = (closure.function.clone(), closure.environment);
        if self.collecting(&function) {
            return self.call(callable, &[]);
        }
        let key = {
            let body = function.borrow();
            (body.id(), body.revision())
        };
        let plan = self
            .expression_plans
            .entry(key)
            .or_insert_with(|| Plan::compile(&function.borrow()).map(Rc::new))
            .clone();
        let Some(plan) = plan else {
            return self.call(callable, &[]);
        };
        // Each accepted operation allocates at most one VM object. The kernel itself never
        // collects or enters Python; pressure or an unknown call returns to ordinary frames.
        if !self.allocation_fits_without_collection(plan.operations.len()) {
            return self.call(callable, &[]);
        }
        let mut values = std::mem::take(&mut self.expression_scratch);
        values.resize(plan.values, Value::None);
        let result = (|| -> Result<Option<Value>> {
            for (destination, step) in &plan.operations {
                if let Step::Operation(Op::GetItem(_, key)) = step
                    && let Ok(Object::Slice { start, stop, step }) = self.heap.get(values[key.0])
                    && [*start, *stop, *step]
                        .iter()
                        .any(|v| self.instance_class(*v).is_some())
                {
                    return Ok(None);
                }

                if let Step::Operation(op)=step && op.inputs().iter().any(|id|matches!(self.heap.get(values[id.0]),Ok(Object::Dict(items)|Object::Set(items)) if items.has_protocol_keys())) {return Ok(None);}

                if let Step::Operation(op) = step {
                    if op
                        .inputs()
                        .iter()
                        .any(|id| self.instance_class(values[id.0]).is_some())
                    {
                        return Ok(None);
                    }
                    if matches!(op, Op::Runtime(CoreOp::Compare(_), _))
                        && op.inputs().iter().any(|id| {
                            matches!(
                                self.heap.get(values[id.0]),
                                Ok(Object::List(_)
                                    | Object::Tuple(_)
                                    | Object::Dict(_)
                                    | Object::Set(_))
                            )
                        })
                    {
                        return Ok(None);
                    }
                }

                values[*destination] = match step {
                    Step::Literal(literal) => self.intern_literal(&literal.text, &literal.cache),
                    Step::Operation(op) => match op {
                        Op::Constant(value) => value
                            .immediate()
                            .ok_or(Error::InvalidIr("expression constant"))?,
                        Op::Add(a, b) => {
                            crate::runtime::add(&mut self.heap, values[a.0], values[b.0])?
                        }
                        Op::Subtract(a, b) => crate::runtime::subtract(values[a.0], values[b.0])?,
                        Op::Less(a, b) => crate::runtime::less(values[a.0], values[b.0])?,
                        Op::GetItem(a, b) => self.heap.get_item(values[a.0], values[b.0])?,
                        Op::Len(a) => Value::Int(
                            i64::try_from(self.heap.len(values[a.0])?)
                                .map_err(|_| Error::IntegerOverflow)?,
                        ),
                        Op::Runtime(CoreOp::LoadName(name), _) => self.lookup(environment, name)?,
                        Op::Runtime(CoreOp::Compare(op), args) => Value::Bool(compare(
                            &self.heap,
                            op,
                            values[args[0].0],
                            values[args[1].0],
                        )?),
                        Op::Runtime(CoreOp::Unary(op), args) => {
                            let value = values[args[0].0];
                            match op.as_str() {
                                "not" => Value::Bool(!crate::runtime::truth(&self.heap, value)?),
                                "+" => {
                                    number(value)?;
                                    value
                                }
                                "-" => match value {
                                    Value::Int(n) => {
                                        Value::Int(n.checked_neg().ok_or(Error::IntegerOverflow)?)
                                    }
                                    value => Value::Float(-number(value)?),
                                },
                                "~" => Value::Int(!integer(value)?),
                                _ => return Ok(None),
                            }
                        }
                        Op::Runtime(CoreOp::Call(_), inputs) => {
                            let callee = values[inputs[0].0];
                            let builtin = match self.heap.get(callee) {
                                Ok(Object::Native(name)) => ScalarBuiltin::from_name(name),
                                _ => None,
                            };
                            let Some(builtin) =
                                builtin.filter(|builtin| builtin.accepts(inputs.len() - 1))
                            else {
                                return Ok(None);
                            };
                            let mut arguments = [Value::None; 4];
                            for (destination, source) in arguments.iter_mut().zip(&inputs[1..]) {
                                *destination = values[source.0];
                            }
                            let Some(value) = self.try_scalar_call(
                                builtin,
                                callee,
                                &arguments[..inputs.len() - 1],
                            ) else {
                                return Ok(None);
                            };
                            value?
                        }
                        _ => return Err(Error::InvalidIr("expression plan operation")),
                    },
                };
            }
            Ok(Some(values[plan.result]))
        })();
        values.clear();
        self.expression_scratch = values;
        if matches!(result, Ok(None)) {
            return self.call(callable, &[]);
        }
        self.tier_stats.generator_expression_kernels += 1;
        Self::record_expression(&function, &result);
        let roots = self.transient_roots.len();
        if let Ok(Some(value)) = &result {
            self.transient_roots.push(*value);
        }
        self.consider_compilation(&function, crate::profile::Hotspot::Function);
        self.transient_roots.truncate(roots);
        result?.ok_or(Error::InvalidIr("expression result"))
    }

    fn record_expression(function: &Rc<RefCell<Function>>, result: &Result<Option<Value>>) {
        let function = function.borrow();
        let mut profile = function.profile.borrow_mut();
        profile.layout(function.blocks.len(), function.value_count);
        profile.entries = profile.entries.saturating_add(1);
        if result.is_ok() {
            profile.returns = profile.returns.saturating_add(1);
        } else {
            profile.errors = profile.errors.saturating_add(1);
        }
    }
}
