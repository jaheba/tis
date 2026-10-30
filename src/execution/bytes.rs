//! Immutable byte sequences; no execution-tier or frame dependencies in storage and conversion.
use super::*;

// Python includes vertical tab in ASCII whitespace; Rust’s ASCII predicate does not.
fn whitespace(byte: &u8) -> bool {
    matches!(*byte, b' ' | 9..=13)
}

pub(super) fn repr(data: &[u8]) -> String {
    let quote = if data.contains(&b'\'') && !data.contains(&b'"') {
        '"'
    } else {
        '\''
    };
    let mut out = format!("b{quote}");
    for &byte in data {
        match byte {
            b'\\' => out.push_str("\\\\"),
            b'\n' => out.push_str("\\n"),
            b'\r' => out.push_str("\\r"),
            b'\t' => out.push_str("\\t"),
            32..=126 => {
                if char::from(byte) == quote {
                    out.push('\\');
                }
                out.push(char::from(byte));
            }
            _ => {
                use std::fmt::Write;
                write!(out, "\\x{byte:02x}").unwrap();
            }
        }
    }
    out.push(quote);
    out
}

pub(super) fn method_name(name: &str) -> Option<&'static str> {
    Some(match name {
        "decode" => "decode",
        "hex" => "hex",
        "fromhex" => "fromhex",
        "join" => "join",
        "lower" => "lower",
        "upper" => "upper",
        "strip" => "strip",
        "lstrip" => "lstrip",
        "rstrip" => "rstrip",
        "startswith" => "startswith",
        "endswith" => "endswith",
        "find" => "find",
        "rfind" => "rfind",
        "index" => "index",
        "rindex" => "rindex",
        "count" => "count",
        "replace" => "replace",
        "split" => "split",
        "rsplit" => "rsplit",
        "isdigit" => "isdigit",
        "isalpha" => "isalpha",
        "isalnum" => "isalnum",
        "isspace" => "isspace",
        _ => return None,
    })
}

impl Vm {
    pub(super) fn intern_bytes(&mut self, data: &[u8]) -> Value {
        if let Some(&value) = self.bytes_pool.get(data) {
            return value;
        }
        let value = self.heap.allocate(Object::Bytes(data.to_vec()));
        self.bytes_pool.insert(data.to_vec(), value);
        value
    }
    fn bytes_value(&mut self, data: Vec<u8>) -> Value {
        if data.is_empty() {
            self.intern_bytes(&[])
        } else {
            self.heap.allocate(Object::Bytes(data))
        }
    }
    fn byte_data(&self, value: Value) -> Result<&[u8]> {
        match self.heap.get(value) {
            Ok(Object::Bytes(v)) => Ok(v),
            _ => Err(error(format!(
                "TypeError: a bytes-like object is required, not '{}'",
                self.value_type_name(value).unwrap_or("object")
            ))),
        }
    }
    pub(super) fn bytes_keywords(
        &mut self,
        args: &[Value],
        keywords: &[(String, Value)],
        constructor: bool,
    ) -> Result<Vec<Value>> {
        let names = if constructor {
            &["source", "encoding", "errors"][..]
        } else {
            &["encoding", "errors"][..]
        };
        arity(args, 0, names.len())?;
        let mut values = vec![None; names.len()];
        for (slot, &value) in values.iter_mut().zip(args) {
            *slot = Some(value);
        }
        for (name, value) in keywords {
            let Some(index) = names.iter().position(|n| n == name) else {
                return Err(error(format!(
                    "TypeError: unexpected keyword argument '{name}'"
                )));
            };
            if values[index].replace(*value).is_some() {
                return Err(error(format!(
                    "TypeError: multiple values for argument '{name}'"
                )));
            }
        }
        let count = values
            .iter()
            .rposition(Option::is_some)
            .map_or(0, |i| i + 1);
        let offset = usize::from(constructor);
        let mut bound = Vec::with_capacity(count);
        for (i, value) in values.into_iter().take(count).enumerate() {
            bound.push(value.unwrap_or_else(|| {
                if constructor && i == 0 {
                    Value::Int(0)
                } else {
                    self.heap
                        .string(if i == offset { "utf-8" } else { "strict" })
                }
            }));
        }
        Ok(bound)
    }
    pub(super) fn construct_bytes(&mut self, args: &[Value]) -> Result<Value> {
        arity(args, 0, 3)?;
        if args.is_empty() {
            return Ok(self.intern_bytes(&[]));
        }
        let source = args[0];
        if args.len() > 1 {
            let text = self
                .heap
                .as_str(&source)
                .ok_or(Error::Type("encoding without a string argument"))?
                .to_owned();
            return self.encode_bytes(&text, &args[1..]);
        }
        if matches!(self.heap.get(source), Ok(Object::Bytes(_))) {
            return Ok(source);
        }
        if self.heap.as_str(&source).is_some() {
            return Err(Error::Type("string argument without an encoding"));
        }
        if let Some(value) = self.call_special(source, "__bytes__", &[])? {
            if matches!(self.heap.get(value), Ok(Object::Bytes(_))) {
                return Ok(value);
            }
            return Err(Error::Type("__bytes__ returned non-bytes"));
        }
        if crate::integer::is_integer(&self.heap, source)
            || self.special(source, "__index__")?.is_some()
        {
            let count = self.py_index(source)?;
            if count < 0 {
                return Err(error("ValueError: negative count"));
            }
            let count = usize::try_from(count).map_err(|_| Error::IntegerOverflow)?;
            return Ok(self.bytes_value(vec![0; count]));
        }
        let values = self.collect_iter(source)?;
        self.protocol_roots(&values, |vm| {
            let mut data = Vec::with_capacity(values.len());
            for &value in &values {
                data.push(
                    u8::try_from(vm.py_index(value)?)
                        .map_err(|_| error("ValueError: bytes must be in range(0, 256)"))?,
                );
            }
            Ok(vm.bytes_value(data))
        })
    }
    fn encoding_args(&self, args: &[Value]) -> Result<(String, String)> {
        arity(args, 0, 2)?;
        let name = args
            .first()
            .map(|v| {
                self.heap
                    .as_str(v)
                    .ok_or(Error::Type("encoding must be str"))
            })
            .transpose()?
            .unwrap_or("utf-8");
        let errors = args
            .get(1)
            .map(|v| self.heap.as_str(v).ok_or(Error::Type("errors must be str")))
            .transpose()?
            .unwrap_or("strict");
        let canonical = name.to_ascii_lowercase().replace([' ', '-'], "_");
        let canonical = match canonical.as_str() {
            "utf8" | "utf_8" | "u8" => "utf-8",
            "ascii" | "us_ascii" | "646" => "ascii",
            _ => return Err(error(format!("LookupError: unknown encoding: {name}"))),
        };
        Ok((canonical.into(), errors.into()))
    }
    pub(super) fn encode_bytes(&mut self, text: &str, args: &[Value]) -> Result<Value> {
        let (encoding, errors) = self.encoding_args(args)?;
        if encoding == "utf-8" {
            return Ok(self.bytes_value(text.as_bytes().to_vec()));
        }
        let mut data = Vec::with_capacity(text.len());
        for (index, ch) in text.chars().enumerate() {
            if ch.is_ascii() {
                data.push(ch as u8);
            } else {
                match errors.as_str() {
                    "ignore" => {}
                    "replace" => data.push(b'?'),
                    "strict" => {
                        return Err(error(format!(
                            "UnicodeEncodeError: 'ascii' codec can't encode character '{}' in position {index}: ordinal not in range(128)",
                            ch.escape_unicode()
                        )));
                    }
                    _ => {
                        return Err(error(format!(
                            "LookupError: unknown error handler name '{errors}'"
                        )));
                    }
                }
            }
        }
        Ok(self.bytes_value(data))
    }
    fn decode_bytes(&mut self, data: &[u8], args: &[Value]) -> Result<Value> {
        let (encoding, errors) = self.encoding_args(args)?;
        let mut text = String::new();
        let mut position = 0;
        while position < data.len() {
            let (valid, invalid, reason) = if encoding == "ascii" {
                let valid = data[position..]
                    .iter()
                    .position(|v| !v.is_ascii())
                    .unwrap_or(data.len() - position);
                (valid, 1, "ordinal not in range(128)")
            } else {
                match std::str::from_utf8(&data[position..]) {
                    Ok(rest) => {
                        text.push_str(rest);
                        break;
                    }
                    Err(err) => {
                        let byte = data[position + err.valid_up_to()];
                        let reason = if err.error_len().is_none() {
                            "unexpected end of data"
                        } else if !(0xc2..=0xf4).contains(&byte) {
                            "invalid start byte"
                        } else {
                            "invalid continuation byte"
                        };
                        (
                            err.valid_up_to(),
                            err.error_len()
                                .unwrap_or(data.len() - position - err.valid_up_to()),
                            reason,
                        )
                    }
                }
            };
            text.push_str(
                std::str::from_utf8(&data[position..position + valid]).expect("validated prefix"),
            );
            position += valid;
            if position == data.len() {
                break;
            }
            match errors.as_str() {
                "ignore" => {}
                "replace" => text.push('\u{fffd}'),
                "strict" => {
                    return Err(error(format!(
                        "UnicodeDecodeError: '{encoding}' codec can't decode byte 0x{:02x} in position {position}: {reason}",
                        data[position]
                    )));
                }
                _ => {
                    return Err(error(format!(
                        "LookupError: unknown error handler name '{errors}'"
                    )));
                }
            }
            position += invalid;
        }
        Ok(self.heap.string_owned(text))
    }
    pub(super) fn bytes_fromhex(&mut self, args: &[Value]) -> Result<Value> {
        arity(args, 1, 1)?;
        let text = self
            .heap
            .as_str(&args[0])
            .ok_or(Error::Type("fromhex() argument must be str"))?;
        let raw = text.as_bytes();
        let mut data = vec![];
        let mut index = 0;
        while index < raw.len() {
            if whitespace(&raw[index]) {
                index += 1;
                continue;
            }
            let hi = char::from(raw[index]).to_digit(16);
            let lo = raw.get(index + 1).and_then(|v| char::from(*v).to_digit(16));
            match (hi, lo) {
                (Some(hi), Some(lo)) => data.push((hi * 16 + lo) as u8),
                _ => {
                    return Err(error(format!(
                        "ValueError: non-hexadecimal number found in fromhex() arg at position {}",
                        if hi.is_none() { index } else { index + 1 }
                    )));
                }
            }
            index += 2;
        }
        Ok(self.bytes_value(data))
    }
    pub(super) fn bytes_method(
        &mut self,
        receiver: Value,
        name: &str,
        args: &[Value],
    ) -> Result<Value> {
        let data = self.byte_data(receiver)?.to_vec();
        match name {
            "decode" => self.decode_bytes(&data, args),
            "fromhex" => self.bytes_fromhex(args),
            "hex" => {
                arity(args, 0, 0)?;
                use std::fmt::Write;
                let mut text = String::with_capacity(data.len() * 2);
                for b in data {
                    write!(text, "{b:02x}").unwrap();
                }
                Ok(self.heap.string_owned(text))
            }
            "lower" | "upper" => {
                arity(args, 0, 0)?;
                Ok(self.bytes_value(if name == "lower" {
                    data.to_ascii_lowercase()
                } else {
                    data.to_ascii_uppercase()
                }))
            }
            "isdigit" | "isalpha" | "isalnum" | "isspace" => {
                arity(args, 0, 0)?;
                Ok(Value::Bool(
                    !data.is_empty()
                        && data.iter().all(|b| match name {
                            "isdigit" => b.is_ascii_digit(),
                            "isalpha" => b.is_ascii_alphabetic(),
                            "isalnum" => b.is_ascii_alphanumeric(),
                            _ => whitespace(b),
                        }),
                ))
            }
            "strip" | "lstrip" | "rstrip" => {
                arity(args, 0, 1)?;
                let trim = match args.first() {
                    Some(value) if *value != Value::None => Some(self.byte_data(*value)?),
                    _ => None,
                };
                let is_trim =
                    |v: &u8| trim.map_or_else(|| whitespace(v), |chars| chars.contains(v));
                let start = if name == "rstrip" {
                    0
                } else {
                    data.iter().position(|b| !is_trim(b)).unwrap_or(data.len())
                };
                let end = if name == "lstrip" {
                    data.len()
                } else {
                    data.iter()
                        .rposition(|b| !is_trim(b))
                        .map_or(start, |i| i + 1)
                        .max(start)
                };
                Ok(self.bytes_value(data[start..end].to_vec()))
            }
            "join" => {
                arity(args, 1, 1)?;
                let values = self.collect_iter(args[0])?;
                let mut result = vec![];
                for (i, value) in values.into_iter().enumerate() {
                    if i > 0 {
                        result.extend_from_slice(&data);
                    }
                    result.extend_from_slice(self.byte_data(value)?);
                }
                Ok(self.bytes_value(result))
            }
            "startswith" | "endswith" | "find" | "rfind" | "index" | "rindex" | "count" => {
                arity(args, 1, 3)?;
                let len = data.len() as i64;
                let raw_start = if args.len() > 1 {
                    self.py_index(args[1])?
                } else {
                    0
                };
                let raw_end = if args.len() > 2 && args[2] != Value::None {
                    self.py_index(args[2])?
                } else {
                    len
                };
                let normalize = |n: i64| if n < 0 { (len + n).max(0) } else { n.min(len) } as usize;
                let start = normalize(raw_start);
                let end = normalize(raw_end);
                let valid = raw_start <= len && start <= end;
                let part = if valid { &data[start..end] } else { &[] };
                if matches!(name, "startswith" | "endswith") {
                    let choices = match self.heap.get(args[0]) {
                        Ok(Object::Tuple(v)) => v.clone(),
                        _ => vec![args[0]],
                    };
                    for value in choices {
                        let needle = self.byte_data(value)?;
                        if valid
                            && if name == "startswith" {
                                part.starts_with(needle)
                            } else {
                                part.ends_with(needle)
                            }
                        {
                            return Ok(Value::Bool(true));
                        }
                    }
                    return Ok(Value::Bool(false));
                }
                let needle = self.byte_data(args[0])?;
                let result = if !valid {
                    -1
                } else if needle.is_empty() {
                    if name == "count" {
                        return Ok(Value::Int((part.len() + 1) as i64));
                    }
                    if name.starts_with('r') {
                        end as i64
                    } else {
                        start as i64
                    }
                } else if name == "count" {
                    let mut count = 0;
                    let mut next = 0;
                    for (at, bytes) in part.windows(needle.len()).enumerate() {
                        if at >= next && bytes == needle {
                            count += 1;
                            next = at + needle.len();
                        }
                    }
                    return Ok(Value::Int(count));
                } else {
                    let found = if name.starts_with('r') {
                        part.windows(needle.len()).rposition(|v| v == needle)
                    } else {
                        part.windows(needle.len()).position(|v| v == needle)
                    };
                    found.map_or(-1, |i| (start + i) as i64)
                };
                if name == "count" {
                    return Ok(Value::Int(0));
                }
                if result < 0 && matches!(name, "index" | "rindex") {
                    return Err(error("ValueError: subsection not found"));
                }
                Ok(Value::Int(result))
            }
            "replace" => {
                arity(args, 2, 3)?;
                let old = self.byte_data(args[0])?.to_vec();
                let new = self.byte_data(args[1])?.to_vec();
                let limit = if args.len() > 2 {
                    self.py_index(args[2])?
                } else {
                    -1
                };
                let mut result = vec![];
                let mut position: usize = 0;
                let mut count = 0;
                loop {
                    let matches = (limit < 0 || count < limit)
                        && (old.is_empty()
                            || data.get(position..position.saturating_add(old.len()))
                                == Some(old.as_slice()));
                    if matches {
                        result.extend_from_slice(&new);
                        count += 1;
                        position += old.len();
                    }
                    if position >= data.len() {
                        break;
                    }
                    if !matches || old.is_empty() {
                        result.push(data[position]);
                        position += 1;
                    }
                }
                Ok(self.bytes_value(result))
            }
            "split" | "rsplit" => {
                arity(args, 0, 2)?;
                let limit = if args.len() > 1 {
                    self.py_index(args[1])?
                } else {
                    -1
                };
                let separator = match args.first() {
                    Some(v) if *v != Value::None => Some(self.byte_data(*v)?.to_vec()),
                    _ => None,
                };
                let mut chunks = vec![];
                if let Some(separator) = separator {
                    if separator.is_empty() {
                        return Err(error("ValueError: empty separator"));
                    }
                    let mut remainder = data.as_slice();
                    while limit < 0 || chunks.len() < limit as usize {
                        let at = if name == "rsplit" {
                            remainder
                                .windows(separator.len())
                                .rposition(|v| v == separator)
                        } else {
                            remainder
                                .windows(separator.len())
                                .position(|v| v == separator)
                        };
                        let Some(at) = at else {
                            break;
                        };
                        if name == "rsplit" {
                            chunks.push(remainder[at + separator.len()..].to_vec());
                            remainder = &remainder[..at];
                        } else {
                            chunks.push(remainder[..at].to_vec());
                            remainder = &remainder[at + separator.len()..];
                        }
                    }
                    chunks.push(remainder.to_vec());
                    if name == "rsplit" {
                        chunks.reverse();
                    }
                } else {
                    let mut remainder = data.as_slice();
                    let mut count = 0;
                    loop {
                        if name == "rsplit" {
                            while remainder.last().is_some_and(whitespace) {
                                remainder = &remainder[..remainder.len() - 1];
                            }
                        } else {
                            while remainder.first().is_some_and(whitespace) {
                                remainder = &remainder[1..];
                            }
                        }
                        if remainder.is_empty() {
                            break;
                        }
                        if limit >= 0 && count >= limit {
                            chunks.push(remainder.to_vec());
                            break;
                        }
                        let at = if name == "rsplit" {
                            remainder.iter().rposition(whitespace)
                        } else {
                            remainder.iter().position(whitespace)
                        };
                        match at {
                            Some(i) => {
                                if name == "rsplit" {
                                    chunks.push(remainder[i + 1..].to_vec());
                                    remainder = &remainder[..i];
                                } else {
                                    chunks.push(remainder[..i].to_vec());
                                    remainder = &remainder[i + 1..];
                                }
                                count += 1;
                            }
                            None => {
                                chunks.push(remainder.to_vec());
                                break;
                            }
                        }
                    }
                    if name == "rsplit" {
                        chunks.reverse();
                    }
                }
                let values = chunks
                    .into_iter()
                    .map(|v| self.bytes_value(v))
                    .collect::<Vec<_>>();
                Ok(self.heap.allocate(Object::List(values.into())))
            }
            _ => Err(Error::Unsupported("bytes method")),
        }
    }
}
