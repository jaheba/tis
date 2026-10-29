//! Small JSON boundary: ordinary VM values, string keys, finite numbers and bounded depth.
use crate::{Dict, Error, Heap, Key, Object, Result, Value};
use serde_json::Value as Json;

fn invalid(message: impl std::fmt::Display) -> Error {
    Error::Runtime(format!("JSON error: {message}"))
}

pub(crate) fn loads(heap: &Heap, value: Value) -> Result<Value> {
    let Object::String(text) = heap.get(value)? else {
        return Err(Error::Type("json.loads requires a string"));
    };
    let parsed: Json = serde_json::from_str(text).map_err(invalid)?;
    decode(heap, parsed, 0)
}

fn decode(heap: &Heap, value: Json, depth: usize) -> Result<Value> {
    if depth > 64 {
        return Err(invalid("maximum depth exceeded"));
    }
    // Heap allocation does not collect. The enclosing VM helper publishes the completed
    // result before its next GC safepoint, so partially built containers stay alive.
    Ok(match value {
        Json::Null => Value::None,
        Json::Bool(value) => Value::Bool(value),
        Json::Number(value) => {
            let text = value.to_string();
            if !text.contains(['.', 'e', 'E']) {
                Value::Int(text.parse().map_err(|_| Error::IntegerOverflow)?)
            } else {
                let value: f64 = text.parse().map_err(invalid)?;
                if !value.is_finite() {
                    return Err(invalid("nonfinite number"));
                }
                Value::Float(value)
            }
        }
        Json::String(value) => heap.allocate(Object::String(value)),
        Json::Array(values) => {
            let values = values
                .into_iter()
                .map(|v| decode(heap, v, depth + 1))
                .collect::<Result<Vec<_>>>()?;
            heap.allocate(Object::List(values.into()))
        }
        Json::Object(values) => {
            let mut items = Dict::default();
            for (key, value) in values {
                items.insert(Key::String(key), decode(heap, value, depth + 1)?);
            }
            heap.allocate(Object::Dict(items))
        }
    })
}

pub(crate) fn dumps(heap: &Heap, value: Value) -> Result<Value> {
    let mut text = String::new();
    encode(heap, value, &mut text, &mut Vec::new())?;
    Ok(heap.allocate(Object::String(text)))
}

fn encode(heap: &Heap, value: Value, text: &mut String, active: &mut Vec<Value>) -> Result<()> {
    match value {
        Value::None => text.push_str("null"),
        Value::Bool(value) => text.push_str(if value { "true" } else { "false" }),
        Value::Int(value) => text.push_str(&value.to_string()),
        Value::Float(value) => {
            if !value.is_finite() {
                return Err(invalid("nonfinite number"));
            }
            text.push_str(&serde_json::to_string(&value).map_err(invalid)?);
        }
        Value::Ref(_) => {
            if active.len() >= 64 || active.contains(&value) {
                return Err(invalid("cycle or maximum depth exceeded"));
            }
            active.push(value);
            match heap.get(value)? {
                Object::String(value) => {
                    text.push_str(&serde_json::to_string(value).map_err(invalid)?)
                }
                Object::List(_) | Object::Tuple(_) => {
                    let values = heap.get(value)?.values().unwrap();
                    text.push('[');
                    for (index, &item) in values.iter().enumerate() {
                        if index != 0 {
                            text.push(',');
                        }
                        encode(heap, item, text, active)?;
                    }
                    text.push(']');
                }
                Object::Dict(values) => {
                    text.push('{');
                    for (index, (key, value)) in values.iter().enumerate() {
                        let Key::String(key) = key else {
                            return Err(Error::Type("json.dumps requires string keys"));
                        };
                        if index != 0 {
                            text.push(',');
                        }
                        text.push_str(&serde_json::to_string(key).map_err(invalid)?);
                        text.push(':');
                        encode(heap, *value, text, active)?;
                    }
                    text.push('}');
                }
                _ => return Err(Error::Type("unsupported JSON value")),
            }
            active.pop();
        }
        _ => return Err(Error::Type("unsupported JSON value")),
    }
    Ok(())
}
