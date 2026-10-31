//! Operations on values and heap objects, independent of interpreter frames.

use crate::{Error, Heap, Object, Result, Value};

pub(crate) fn add(heap: &mut Heap, lhs: Value, rhs: Value) -> Result<Value> {
    if let (Some(lhs), Some(rhs)) = (integer(lhs), integer(rhs)) {
        return match lhs.checked_add(rhs) {
            Some(n) => Ok(Value::Int(n)),
            None => crate::integer::binary(heap, "+", Value::Int(lhs), Value::Int(rhs)),
        };
    }
    if crate::integer::is_big(heap, lhs) || crate::integer::is_big(heap, rhs) {
        return crate::integer::binary(heap, "+", lhs, rhs);
    }
    if let (Some(lhs), Some(rhs)) = (number(lhs), number(rhs)) {
        return Ok(Value::Float(lhs + rhs));
    }
    if let (Some(lhs), Some(rhs)) = (heap.as_str(&lhs), heap.as_str(&rhs)) {
        return Ok(heap.string_owned(format!("{lhs}{rhs}")));
    }
    if let (Value::Ref(_), Value::Ref(_)) = (lhs, rhs) {
        let object = match (heap.get(lhs)?, heap.get(rhs)?) {
            (
                left @ (Object::Bytes(_) | Object::ByteArray(_)),
                right @ (Object::Bytes(_) | Object::ByteArray(_)),
            ) => {
                let (Object::Bytes(lhs) | Object::ByteArray(lhs)) = left else {
                    unreachable!()
                };
                let (Object::Bytes(rhs) | Object::ByteArray(rhs)) = right else {
                    unreachable!()
                };
                let data = [lhs.as_slice(), rhs.as_slice()].concat();
                if matches!(left, Object::ByteArray(_)) {
                    Object::ByteArray(data)
                } else {
                    Object::Bytes(data)
                }
            }
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

pub(crate) fn subtract(heap: &Heap, lhs: Value, rhs: Value) -> Result<Value> {
    if let (Some(a), Some(b)) = (integer(lhs), integer(rhs)) {
        return match a.checked_sub(b) {
            Some(n) => Ok(Value::Int(n)),
            None => crate::integer::binary(heap, "-", lhs, rhs),
        };
    }
    if crate::integer::is_big(heap, lhs) || crate::integer::is_big(heap, rhs) {
        return crate::integer::binary(heap, "-", lhs, rhs);
    }
    if let (Some(lhs), Some(rhs)) = (number(lhs), number(rhs)) {
        Ok(Value::Float(lhs - rhs))
    } else {
        Err(Error::Type("subtraction requires numbers"))
    }
}

pub(crate) fn less(heap: &Heap, lhs: Value, rhs: Value) -> Result<Value> {
    if let Some(order) = crate::integer::order(heap, lhs, rhs) {
        return Ok(Value::Bool(order == Some(std::cmp::Ordering::Less)));
    }
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
            Object::Library(payload)
                if matches!(&**payload, crate::library::Payload::Decimal(_)) =>
            {
                let crate::library::Payload::Decimal(d) = &**payload else {
                    unreachable!()
                };
                d.coefficient != num_bigint::BigInt::from(0)
            }
            Object::BigInt(n) => n != &num_bigint::BigInt::from(0),
            Object::String(_)
            | Object::Bytes(_)
            | Object::ByteArray(_)
            | Object::List(_)
            | Object::Tuple(_)
            | Object::Dict(_)
            | Object::Set(_)
            | Object::FrozenSet(_)
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
