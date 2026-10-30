//! Operations on values and heap objects, independent of interpreter frames.

use crate::{Error, Heap, Object, Result, Value};

pub(crate) fn add(heap: &mut Heap, lhs: Value, rhs: Value) -> Result<Value> {
    if let (Some(lhs), Some(rhs)) = (integer(lhs), integer(rhs)) {
        return lhs
            .checked_add(rhs)
            .map(Value::Int)
            .ok_or(Error::IntegerOverflow);
    }
    if let (Some(lhs), Some(rhs)) = (number(lhs), number(rhs)) {
        return Ok(Value::Float(lhs + rhs));
    }
    if let (Some(lhs), Some(rhs)) = (heap.as_str(&lhs), heap.as_str(&rhs)) {
        return Ok(heap.string_owned(format!("{lhs}{rhs}")));
    }
    if let (Value::Ref(_), Value::Ref(_)) = (lhs, rhs) {
        let object = match (heap.get(lhs)?, heap.get(rhs)?) {
            (Object::String(lhs), Object::String(rhs)) => Object::String(format!("{lhs}{rhs}")),
            (Object::List(lhs), Object::List(rhs)) => {
                Object::List(lhs.iter().chain(rhs).copied().collect::<Vec<_>>().into())
            }
            (Object::Tuple(lhs), Object::Tuple(rhs)) => {
                Object::Tuple(lhs.iter().chain(rhs).copied().collect())
            }
            _ => return Err(Error::Type("unsupported addition operands")),
        };
        return Ok(heap.allocate(object));
    }
    Err(Error::Type("unsupported addition operands"))
}

pub(crate) fn subtract(lhs: Value, rhs: Value) -> Result<Value> {
    if let (Some(lhs), Some(rhs)) = (integer(lhs), integer(rhs)) {
        lhs.checked_sub(rhs)
            .map(Value::Int)
            .ok_or(Error::IntegerOverflow)
    } else if let (Some(lhs), Some(rhs)) = (number(lhs), number(rhs)) {
        Ok(Value::Float(lhs - rhs))
    } else {
        Err(Error::Type("subtraction requires numbers"))
    }
}

pub(crate) fn less(lhs: Value, rhs: Value) -> Result<Value> {
    if let (Some(lhs), Some(rhs)) = (integer(lhs), integer(rhs)) {
        Ok(Value::Bool(lhs < rhs))
    } else if let (Some(lhs), Some(rhs)) = (number(lhs), number(rhs)) {
        Ok(Value::Bool(lhs < rhs))
    } else {
        Err(Error::Type("comparison requires numbers"))
    }
}

pub(crate) fn truth(heap: &Heap, value: Value) -> Result<bool> {
    Ok(match value {
        Value::Unbound => return Err(Error::InvalidIr("unbound value escaped a local read")),
        Value::InlineString(text) => !text.as_str().is_empty(),
        Value::None => false,
        Value::Bool(value) => value,
        Value::Int(value) => value != 0,
        Value::Float(value) => value != 0.0,
        Value::Ref(_) => match heap.get(value)? {
            Object::String(_)
            | Object::List(_)
            | Object::Tuple(_)
            | Object::Dict(_)
            | Object::Set(_)
            | Object::Range { .. }
            | Object::View { .. } => heap.len(value)? != 0,
            _ => true,
        },
    })
}

fn integer(value: Value) -> Option<i64> {
    match value {
        Value::Int(value) => Some(value),
        Value::Bool(value) => Some(i64::from(value)),
        _ => None,
    }
}

fn number(value: Value) -> Option<f64> {
    match value {
        Value::Float(value) => Some(value),
        other => integer(other).map(|value| value as f64),
    }
}

/// Only exact Int/Float operands are supported. None requests the original Python operation;
/// integer overflow is distinct so speculative callers preserve its side-exit reason.
pub(crate) fn numeric_add(lhs: Value, rhs: Value) -> Option<Result<Value>> {
    Some(match (lhs, rhs) {
        (Value::Int(a), Value::Int(b)) => a
            .checked_add(b)
            .map(Value::Int)
            .ok_or(Error::IntegerOverflow),
        (Value::Int(a), Value::Float(b)) => Ok(Value::Float(a as f64 + b)),
        (Value::Float(a), Value::Int(b)) => Ok(Value::Float(a + b as f64)),
        (Value::Float(a), Value::Float(b)) => Ok(Value::Float(a + b)),
        _ => return None,
    })
}

/// Integer comparisons stay exact; callers verify the literal's f64 representation before use.
/// Bool and heap operands require the original Python comparison.
pub(crate) fn number_compare(value: Value, integer: i64, operator: &str) -> Option<bool> {
    match value {
        Value::Int(value) => Some(match operator {
            "==" => value == integer,
            "!=" => value != integer,
            "<" => value < integer,
            "<=" => value <= integer,
            ">" => value > integer,
            ">=" => value >= integer,
            _ => return None,
        }),
        Value::Float(value) => Some(
            crate::scalar::FloatCondition::from_symbol(operator)?.evaluate(value, integer as f64),
        ),
        _ => None,
    }
}
