//! Compiled patterns own their engine; matches retain their original subject.
use super::*;
use std::rc::Rc;
type Captures = Vec<Option<(usize, usize)>>;

#[derive(Debug, Clone)]
pub enum Engine {
    Text(fancy_regex::Regex),
    Bytes(::regex::bytes::Regex),
}
#[derive(Debug, Clone)]
pub struct Pattern {
    pub engine: Rc<Engine>,
}
#[derive(Debug, Clone)]
pub struct Match {
    pub subject: Value,
    pub spans: Vec<Option<(usize, usize)>>,
}
impl Vm {
    fn regex_pattern(&self, value: Value, flags: i64) -> Result<Pattern> {
        if let Ok(Object::Library(payload)) = self.heap.get(value)
            && let Payload::Regex(pattern) = &**payload
        {
            if flags != 0 {
                return Err(invalid("cannot process flags with a compiled pattern"));
            }
            return Ok(pattern.clone());
        }
        if flags & !(2 | 8 | 16 | 64 | 256) != 0 {
            return Err(Error::Unsupported("regular expression flags"));
        }
        let engine = if let Some(text) = self.heap.as_str(&value) {
            let mut builder = fancy_regex::RegexBuilder::new(text);
            builder
                .case_insensitive(flags & 2 != 0)
                .multi_line(flags & 8 != 0)
                .dot_matches_new_line(flags & 16 != 0)
                .ignore_whitespace(flags & 64 != 0);
            if flags & 256 != 0 {
                return Err(Error::Unsupported("ASCII text regex flag"));
            }
            Engine::Text(
                builder
                    .build()
                    .map_err(|e| Error::Runtime(format!("re.error: {e}")))?,
            )
        } else {
            let Object::Bytes(bytes) = self.heap.get(value)? else {
                return Err(Error::Type(
                    "first argument must be string or compiled pattern",
                ));
            };
            let mut pattern = String::new();
            for &byte in bytes {
                if byte.is_ascii() {
                    pattern.push(char::from(byte));
                } else {
                    use std::fmt::Write;
                    write!(pattern, "\\x{byte:02x}").unwrap();
                }
            }
            let mut builder = ::regex::bytes::RegexBuilder::new(&pattern);
            builder
                .unicode(false)
                .case_insensitive(flags & 2 != 0)
                .multi_line(flags & 8 != 0)
                .dot_matches_new_line(flags & 16 != 0)
                .ignore_whitespace(flags & 64 != 0);
            Engine::Bytes(
                builder
                    .build()
                    .map_err(|e| Error::Runtime(format!("re.error: {e}")))?,
            )
        };
        Ok(Pattern {
            engine: Rc::new(engine),
        })
    }
    fn regex_captures(
        &self,
        pattern: &Pattern,
        subject: Value,
        pos: usize,
    ) -> Result<Option<Captures>> {
        match &*pattern.engine {
            Engine::Text(regex) => {
                let text = self.heap.as_str(&subject).ok_or(Error::Type(
                    "cannot use a string pattern on a bytes-like object",
                ))?;
                Ok(regex
                    .captures_from_pos(text, pos)
                    .map_err(|e| Error::Runtime(format!("re.error: {e}")))?
                    .map(|caps| {
                        (0..caps.len())
                            .map(|i| caps.get(i).map(|m| (m.start(), m.end())))
                            .collect()
                    }))
            }
            Engine::Bytes(regex) => {
                let object = self.heap.get(subject)?;
                let (Object::Bytes(bytes) | Object::ByteArray(bytes)) = object else {
                    return Err(Error::Type(
                        "cannot use a bytes pattern on a string-like object",
                    ));
                };
                Ok(regex.captures_at(bytes, pos).map(|caps| {
                    (0..caps.len())
                        .map(|i| caps.get(i).map(|m| (m.start(), m.end())))
                        .collect()
                }))
            }
        }
    }
    fn regex_group(
        &self,
        subject: Value,
        span: Option<(usize, usize)>,
        unmatched_empty: bool,
    ) -> Result<Value> {
        if let Some(text) = self.heap.as_str(&subject) {
            return Ok(match span {
                Some((a, b)) => self.heap.string(&text[a..b]),
                None if unmatched_empty => self.heap.string(""),
                None => Value::None,
            });
        }
        let object = self.heap.get(subject)?;
        let (Object::Bytes(bytes) | Object::ByteArray(bytes)) = object else {
            return Err(Error::Type("regex subject"));
        };
        Ok(match span {
            Some((a, b)) => self.heap.allocate(Object::Bytes(bytes[a..b].to_vec())),
            None if unmatched_empty => self.heap.allocate(Object::Bytes(vec![])),
            None => Value::None,
        })
    }
    pub(crate) fn library_regex(
        &mut self,
        receiver: Option<Value>,
        name: &str,
        args: &[Value],
        keywords: &[(String, Value)],
    ) -> Result<Value> {
        if !keywords.is_empty() {
            return Err(Error::Unsupported("regex keyword arguments"));
        }
        if let Some(receiver) = receiver {
            let Object::Library(payload) = self.heap.get(receiver)? else {
                return Err(Error::Type("regex object"));
            };
            if let Payload::Match(m) = &**payload {
                let m = m.clone();
                let indices = if args.is_empty() {
                    vec![Value::Int(0)]
                } else {
                    args.to_vec()
                };
                let mut values = Vec::new();
                for index in indices {
                    let i = usize::try_from(crate::execution::integer(index)?)
                        .map_err(|_| Error::Runtime("IndexError: no such group".into()))?;
                    let span = *m
                        .spans
                        .get(i)
                        .ok_or_else(|| Error::Runtime("IndexError: no such group".into()))?;
                    values.push(self.regex_group(m.subject, span, false)?);
                }
                return if values.len() == 1 {
                    Ok(values[0])
                } else {
                    Ok(self.heap.allocate(Object::Tuple(values)))
                };
            }
        }
        let (pattern, args) = if let Some(receiver) = receiver {
            (self.regex_pattern(receiver, 0)?, args)
        } else {
            arity(args, 1, 5)?;
            let flags_index = match name {
                "sub" => 4,
                "compile" => 1,
                _ => 2,
            };
            let flags = args
                .get(flags_index)
                .map(|v| crate::execution::integer(*v))
                .transpose()?
                .unwrap_or(0);
            (self.regex_pattern(args[0], flags)?, &args[1..])
        };
        if name == "compile" {
            return Ok(self
                .heap
                .allocate(Object::Library(Box::new(Payload::Regex(pattern)))));
        }
        if name == "sub" {
            return self.regex_sub(&pattern, args);
        }
        arity(args, 1, 1)?;
        let subject = args[0];
        let captures = self.regex_captures(&pattern, subject, 0)?;
        if name == "search" || name == "match" || name == "fullmatch" {
            let Some(spans) = captures else {
                return Ok(Value::None);
            };
            let (a, b) = spans[0].unwrap();
            let len = if let Some(s) = self.heap.as_str(&subject) {
                s.len()
            } else {
                self.heap.len(subject)?
            };
            if (name != "search" && a != 0) || (name == "fullmatch" && b != len) {
                return Ok(Value::None);
            }
            return Ok(self
                .heap
                .allocate(Object::Library(Box::new(Payload::Match(Match {
                    subject,
                    spans,
                })))));
        }
        if name != "findall" {
            return Err(Error::Unsupported("regex operation"));
        }
        let mut result = Vec::new();
        let mut pos = 0;
        while let Some(spans) = self.regex_captures(&pattern, subject, pos)? {
            let (a, b) = spans[0].unwrap();
            let value = if spans.len() <= 2 {
                self.regex_group(subject, spans[usize::from(spans.len() == 2)], true)?
            } else {
                self.heap.allocate(Object::Tuple(
                    spans[1..]
                        .iter()
                        .map(|s| self.regex_group(subject, *s, true))
                        .collect::<Result<_>>()?,
                ))
            };
            result.push(value);
            if a == b {
                return Err(Error::Unsupported("empty regex findall matches"));
            }
            pos = b;
        }
        Ok(self.heap.allocate(Object::List(result.into())))
    }
    fn regex_sub(&mut self, pattern: &Pattern, args: &[Value]) -> Result<Value> {
        arity(args, 2, 3)?;
        let replacement = args[0];
        let subject = args[1];
        let count = args
            .get(2)
            .map(|v| crate::execution::integer(*v))
            .transpose()?
            .unwrap_or(0);
        let text = self.heap.as_str(&subject).map(str::to_owned);
        let source = if let Some(s) = &text {
            s.as_bytes().to_vec()
        } else {
            let object = self.heap.get(subject)?;
            let (Object::Bytes(bytes) | Object::ByteArray(bytes)) = object else {
                return Err(Error::Type("regex subject"));
            };
            bytes.clone()
        };
        let replacement = if text.is_some() {
            self.heap
                .as_str(&replacement)
                .ok_or(Error::Type("replacement must be str"))?
                .as_bytes()
                .to_vec()
        } else {
            let Object::Bytes(bytes) = self.heap.get(replacement)? else {
                return Err(Error::Type("replacement must be bytes"));
            };
            bytes.clone()
        };
        // Backreference replacement parsing is explicit until the full template parser exists.
        if replacement.contains(&b'\\') {
            return Err(Error::Unsupported("regex replacement escapes"));
        }
        let mut output = Vec::new();
        let mut pos = 0;
        let mut n = 0;
        while count >= 0 && (count == 0 || n < count) {
            let Some(spans) = self.regex_captures(pattern, subject, pos)? else {
                break;
            };
            let (a, b) = spans[0].unwrap();
            if a == b {
                return Err(Error::Unsupported("empty regex substitution"));
            }
            output.extend_from_slice(&source[pos..a]);
            output.extend_from_slice(&replacement);
            pos = b;
            n += 1;
        }
        output.extend_from_slice(&source[pos..]);
        if text.is_some() {
            Ok(self.heap.string_owned(
                String::from_utf8(output).map_err(|_| Error::Type("regex UTF-8 result"))?,
            ))
        } else {
            Ok(self.heap.allocate(Object::Bytes(output)))
        }
    }
}
