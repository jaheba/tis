//! Small JSON boundary: ordinary VM values, string keys, finite numbers and bounded depth.
use crate::{Dict, Error, Heap, Key, Object, Result, Value};
use jiter::{Jiter, Peek};
use std::fmt::Write;

fn invalid(message: impl std::fmt::Display) -> Error {
    Error::Runtime(format!("JSON error: {message}"))
}

pub(crate) fn loads(heap: &Heap, value: Value) -> Result<Value> {
    let Some(text) = heap.as_str(&value) else {
        return Err(Error::Type("json.loads requires a string"));
    };
    let mut parser = Jiter::new(text.as_bytes());
    let peek = parser.peek().map_err(invalid)?;
    let result = decode(heap, &mut parser, peek, 0)?;
    parser.finish().map_err(invalid)?;
    Ok(result)
}

fn decode(heap: &Heap, parser: &mut Jiter<'_>, peek: Peek, depth: usize) -> Result<Value> {
    if depth > 64 {
        return Err(invalid("maximum depth exceeded"));
    }
    // Heap allocation cannot collect; the completed result is published before the next safepoint.
    // Consume borrowed parser strings immediately because its scratch buffer is reused.
    Ok(match peek {
        Peek::Null => {
            parser.known_null().map_err(invalid)?;
            Value::None
        }
        Peek::True | Peek::False => Value::Bool(parser.known_bool(peek).map_err(invalid)?),
        Peek::String => heap.string(parser.known_str().map_err(invalid)?),
        Peek::Array => {
            let mut values = Vec::new();
            if let Some(mut next) = parser.known_array().map_err(invalid)? {
                loop {
                    values.push(decode(heap, parser, next, depth + 1)?);
                    let Some(peek) = parser.array_step().map_err(invalid)? else {
                        break;
                    };
                    next = peek;
                }
            }
            heap.allocate(Object::List(values.into()))
        }
        Peek::Object => {
            let mut items = Dict::default();
            if let Some(key) = parser.known_object().map_err(invalid)? {
                let mut key: compact_str::CompactString = key.into();
                loop {
                    let next = parser.peek().map_err(invalid)?;
                    let value = decode(heap, parser, next, depth + 1)?;
                    items.insert(Key::String(key), value);
                    let Some(next) = parser.next_key().map_err(invalid)? else {
                        break;
                    };
                    key = next.into();
                }
            }
            heap.allocate(Object::Dict(items))
        }
        peek if peek.is_num() => {
            let bytes = parser.known_number_bytes(peek).map_err(invalid)?;
            let token = std::str::from_utf8(bytes).map_err(invalid)?;
            if !bytes.iter().any(|b| matches!(b, b'.' | b'e' | b'E')) {
                match token.parse::<i64>() {
                    Ok(n) => Value::Int(n),
                    Err(_) => crate::integer::value(heap, token.parse().map_err(invalid)?),
                }
            } else {
                let value: f64 = token.parse().map_err(invalid)?;
                if !value.is_finite() {
                    return Err(invalid("nonfinite number"));
                }
                Value::Float(value)
            }
        }
        _ => return Err(invalid("expected a JSON value")),
    })
}

#[derive(Clone, Copy)]
pub(crate) struct DumpOptions {
    pub ensure_ascii: bool,
    pub sort_keys: bool,
}
impl Default for DumpOptions {
    fn default() -> Self {
        Self {
            ensure_ascii: true,
            sort_keys: false,
        }
    }
}

pub(crate) fn dumps(heap: &Heap, value: Value, options: DumpOptions) -> Result<Value> {
    let mut text = String::new();
    encode(heap, value, &mut text, &mut Vec::new(), options)?;
    Ok(heap.string_owned(text))
}

fn quoted(value: &str, text: &mut String, ascii: bool) {
    text.push('"');
    let mut start = 0;
    for (index, c) in value.char_indices() {
        if c == '"' || c == '\\' || c < '\u{20}' || (ascii && !c.is_ascii()) {
            text.push_str(&value[start..index]);
            match c {
                '"' => text.push_str("\\\""),
                '\\' => text.push_str("\\\\"),
                '\n' => text.push_str("\\n"),
                '\r' => text.push_str("\\r"),
                '\t' => text.push_str("\\t"),
                '\u{8}' => text.push_str("\\b"),
                '\u{c}' => text.push_str("\\f"),
                _ => {
                    for unit in c.encode_utf16(&mut [0; 2]) {
                        write!(text, "\\u{unit:04x}").unwrap();
                    }
                }
            }
            start = index + c.len_utf8();
        }
    }
    text.push_str(&value[start..]);
    text.push('"');
}

fn encode(
    heap: &Heap,
    value: Value,
    text: &mut String,
    active: &mut Vec<Value>,
    options: DumpOptions,
) -> Result<()> {
    match value {
        Value::InlineString(value) => quoted(value.as_str(), text, options.ensure_ascii),
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
                Object::String(value) => quoted(value, text, options.ensure_ascii),
                Object::BigInt(value) => write!(text, "{value}").unwrap(),
                Object::List(_) | Object::Tuple(_) => {
                    let values = heap.get(value)?.values().unwrap();
                    text.push('[');
                    for (index, &item) in values.iter().enumerate() {
                        if index != 0 {
                            text.push_str(", ");
                        }
                        encode(heap, item, text, active, options)?;
                    }
                    text.push(']');
                }
                Object::Dict(values) => {
                    text.push('{');
                    if options.sort_keys {
                        let mut entries = values.iter().collect::<Vec<_>>();
                        if entries
                            .iter()
                            .any(|(key, _)| !matches!(key, Key::String(_)))
                        {
                            return Err(Error::Type("json.dumps requires string keys"));
                        }
                        entries.sort_unstable_by(|(a, _), (b, _)| {
                            let (Key::String(a), Key::String(b)) = (a, b) else {
                                unreachable!()
                            };
                            a.cmp(b)
                        });
                        encode_entries(heap, entries.into_iter(), text, active, options)?;
                    } else {
                        encode_entries(heap, values.iter(), text, active, options)?;
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

fn encode_entries<'a>(
    heap: &Heap,
    entries: impl Iterator<Item = &'a (Key, Value)>,
    text: &mut String,
    active: &mut Vec<Value>,
    options: DumpOptions,
) -> Result<()> {
    for (index, (key, value)) in entries.enumerate() {
        let Key::String(key) = key else {
            return Err(Error::Type("json.dumps requires string keys"));
        };
        if index != 0 {
            text.push_str(", ");
        }
        quoted(key, text, options.ensure_ascii);
        text.push_str(": ");
        encode(heap, *value, text, active, options)?;
    }
    Ok(())
}
