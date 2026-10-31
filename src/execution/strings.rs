//! Borrowed string operations and shared character/byte bounds.
use super::*;

impl Vm {
    pub(super) fn string_method(
        &mut self,
        receiver: Value,
        name: &str,
        args: &[Value],
    ) -> Result<Value> {
        if matches!(
            name,
            "find" | "rfind" | "index" | "rindex" | "count" | "startswith" | "endswith"
        ) {
            arity(args, 1, 3)?;
            let affix = matches!(name, "startswith" | "endswith");
            if !affix && self.heap.as_str(&args[0]).is_none() {
                return Err(Error::Type("substring must be a string"));
            }
            let bounds = if args.len() == 1 {
                None
            } else {
                let mut roots = [Value::None; 4];
                roots[0] = receiver;
                roots[1..=args.len()].copy_from_slice(args);
                Some(self.protocol_roots(&roots[..=args.len()], |vm| {
                    let mut bounds = [None; 2];
                    for (slot, &value) in bounds.iter_mut().zip(&args[1..]) {
                        if value != Value::None {
                            *slot = Some(vm.clamped_index(value)?);
                        }
                    }
                    Ok(bounds)
                })?)
            };
            return search(&self.heap, receiver, name, args[0], bounds);
        }
        let collected = if name == "join"
            && args.len() == 1
            && !matches!(
                self.heap.get(args[0]),
                Ok(Object::List(_) | Object::Tuple(_))
            ) {
            Some(self.protocol_roots(&[receiver, args[0]], |vm| vm.collect_iter(args[0]))?)
        } else {
            None
        };
        let text = self
            .heap
            .as_str(&receiver)
            .ok_or(Error::Type("expected string"))?;
        Self::string_builtin(&self.heap, text, name, args, collected.as_deref())
    }
    fn string_builtin(
        heap: &Heap,
        text: &str,
        name: &str,
        args: &[Value],
        collected: Option<&[Value]>,
    ) -> Result<Value> {
        let result = match name {
            "join" => {
                arity(args, 1, 1)?;
                match heap.get(args[0]) {
                    Ok(Object::List(items)) => {
                        if let Some(values) = items.native_values() {
                            return join_values(heap, text, values);
                        }
                        if items.is_empty() {
                            return Ok(heap.string(""));
                        }
                        return Err(Error::Type("join requires strings"));
                    }
                    Ok(Object::Tuple(values)) => return join_values(heap, text, values),
                    _ => {}
                }
                let values = collected.ok_or(Error::Type("join requires iterable"))?;
                join_values(heap, text, values)?
            }
            "split" => {
                arity(args, 0, 2)?;
                let limit = args
                    .get(1)
                    .map(|&value| integer(value))
                    .transpose()?
                    .unwrap_or(-1);
                let parts = if args.is_empty() || args[0] == Value::None {
                    let mut rest = text.trim_start();
                    let mut parts = vec![];
                    while !rest.is_empty() {
                        if limit >= 0 && parts.len() == limit as usize {
                            parts.push(rest.to_owned());
                            break;
                        }
                        let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
                        parts.push(rest[..end].to_owned());
                        rest = rest[end..].trim_start();
                    }
                    parts
                } else {
                    let Some(sep) = heap.as_str(&(args[0])) else {
                        return Err(Error::Type("separator must be string"));
                    };
                    if sep.is_empty() {
                        return Err(error("ValueError: empty separator"));
                    }
                    if limit < 0 {
                        text.split(sep).map(str::to_owned).collect()
                    } else {
                        text.splitn(limit as usize + 1, sep)
                            .map(str::to_owned)
                            .collect()
                    }
                };
                heap.allocate(Object::List(
                    parts
                        .into_iter()
                        .map(|s| text_heap(heap, s))
                        .collect::<Vec<_>>()
                        .into(),
                ))
            }
            "splitlines" => {
                arity(args, 0, 1)?;
                let keep = args
                    .first()
                    .map(|&value| runtime::truth(heap, value))
                    .transpose()?
                    .unwrap_or(false);
                let mut parts = vec![];
                let mut start = 0;
                let mut chars = text.char_indices().peekable();
                // str::lines omits Python's additional Unicode line boundaries.
                while let Some((index, ch)) = chars.next() {
                    if matches!(
                        ch,
                        '\n' | '\r' | '\u{000b}' | '\u{000c}' | '\u{001c}'
                            ..='\u{001e}' | '\u{0085}' | '\u{2028}' | '\u{2029}'
                    ) {
                        let mut end = index + ch.len_utf8();
                        if ch == '\r' && chars.peek().is_some_and(|&(_, next)| next == '\n') {
                            end = chars.next().unwrap().0 + 1;
                        }
                        parts.push(text_heap(
                            heap,
                            text[start..if keep { end } else { index }].into(),
                        ));
                        start = end;
                    }
                }
                if start < text.len() {
                    parts.push(text_heap(heap, text[start..].into()));
                }
                heap.allocate(Object::List(parts.into()))
            }
            "rjust" => {
                arity(args, 1, 2)?;
                let width = integer(args[0])?;
                let fill = if let Some(&value) = args.get(1) {
                    let Some(fill) = heap.as_str(&(value)) else {
                        return Err(Error::Type("fill character must be a string"));
                    };
                    if fill.chars().count() != 1 {
                        return Err(Error::Type("fill character must be exactly one character"));
                    }
                    fill.chars().next().unwrap()
                } else {
                    ' '
                };
                let padding = width.saturating_sub(text.chars().count() as i64).max(0) as usize;
                let mut result = String::new();
                result
                    .try_reserve(
                        padding
                            .checked_mul(fill.len_utf8())
                            .and_then(|n| n.checked_add(text.len()))
                            .ok_or(Error::IntegerOverflow)?,
                    )
                    .map_err(|_| error("MemoryError: string too large"))?;
                result.extend(std::iter::repeat_n(fill, padding));
                result.push_str(text);
                text_heap(heap, result)
            }
            "isdigit" => {
                arity(args, 0, 0)?;
                Value::Bool(!text.is_empty() && text.chars().all(crate::unicode::is_digit))
            }
            "upper" | "lower" | "strip" => {
                arity(args, 0, 0)?;
                text_heap(
                    heap,
                    match name {
                        "upper" => convert_case(text, true),
                        "lower" => convert_case(text, false),
                        _ => text.trim().into(),
                    },
                )
            }
            "replace" => {
                arity(args, 2, 2)?;
                let (Some(from), Some(to)) = (heap.as_str(&args[0]), heap.as_str(&args[1])) else {
                    return Err(Error::Type("replace requires strings"));
                };
                text_heap(heap, text.replace(from, to))
            }
            _ => return Err(error("unknown string method")),
        };
        Ok(result)
    }
}

fn byte_range(
    heap: &Heap,
    receiver: Value,
    bounds: Option<[Option<i64>; 2]>,
) -> Result<Option<(usize, usize)>> {
    let text = heap
        .as_str(&receiver)
        .ok_or(Error::Type("expected string"))?;
    let Some([start, end]) = bounds else {
        return Ok(Some((0, text.len())));
    };
    if start.unwrap_or(0) == 0 && end.is_none() {
        return Ok(Some((0, text.len())));
    }
    let length = heap.len(receiver)? as i128;
    let start = i128::from(start.unwrap_or(0));
    let start = if start < 0 {
        (length + start).max(0)
    } else {
        start
    };
    let end = end.map(i128::from).unwrap_or(length);
    let end = if end < 0 {
        (length + end).max(0)
    } else {
        end.min(length)
    };
    // Do not clamp a positive start past the end: even an empty needle must then miss.
    if start > end {
        return Ok(None);
    }
    Ok(Some((
        heap.string_byte_offset(receiver, start as usize)?,
        heap.string_byte_offset(receiver, end as usize)?,
    )))
}
fn search(
    heap: &Heap,
    receiver: Value,
    name: &str,
    needle: Value,
    bounds: Option<[Option<i64>; 2]>,
) -> Result<Value> {
    let text = heap
        .as_str(&receiver)
        .ok_or(Error::Type("expected string"))?;
    let range = byte_range(heap, receiver, bounds)?;
    if matches!(name, "startswith" | "endswith") {
        let matches = |part: Value| -> Result<bool> {
            let part = heap
                .as_str(&part)
                .ok_or(Error::Type("affix must be string or tuple of strings"))?;
            Ok(range.is_some_and(|(start, end)| {
                if name == "startswith" {
                    text[start..end].starts_with(part)
                } else {
                    text[start..end].ends_with(part)
                }
            }))
        };
        if let Ok(Object::Tuple(parts)) = heap.get(needle) {
            for &part in parts {
                if matches(part)? {
                    return Ok(Value::Bool(true));
                }
            }
            return Ok(Value::Bool(false));
        }
        return matches(needle).map(Value::Bool);
    }
    let needle = heap
        .as_str(&needle)
        .ok_or(Error::Type("substring must be string"))?;
    if name == "count" {
        let count = match range {
            None => 0,
            Some((start, end)) if needle.is_empty() => {
                heap.string_char_offset(receiver, end)?
                    - heap.string_char_offset(receiver, start)?
                    + 1
            }
            Some((start, end)) => text[start..end].matches(needle).count(),
        };
        return i64::try_from(count)
            .map(Value::Int)
            .map_err(|_| Error::IntegerOverflow);
    }
    let found = range.and_then(|(start, end)| {
        let found = if matches!(name, "rfind" | "rindex") {
            text[start..end].rfind(needle)
        } else {
            text[start..end].find(needle)
        };
        found.map(|byte| start + byte)
    });
    if let Some(byte) = found {
        let position = heap.string_char_offset(receiver, byte)?;
        return i64::try_from(position)
            .map(Value::Int)
            .map_err(|_| Error::IntegerOverflow);
    }
    if matches!(name, "index" | "rindex") {
        return Err(error("ValueError: substring not found"));
    }
    Ok(Value::Int(-1))
}

fn convert_case(text: &str, upper: bool) -> String {
    if text.is_ascii() {
        return if upper {
            text.to_ascii_uppercase()
        } else {
            text.to_ascii_lowercase()
        };
    }
    // Dense Unicode keeps the standard decoder; runs are useful only when ASCII spans exist.
    let high_bits = usize::MAX / 255 * 128;
    let ascii_word = text
        .as_bytes()
        .as_chunks::<{ size_of::<usize>() }>()
        .0
        .iter()
        .take(256 / size_of::<usize>())
        .any(|bytes| usize::from_ne_bytes(*bytes) & high_bits == 0);
    if !ascii_word {
        return if upper {
            text.to_uppercase()
        } else {
            text.to_lowercase()
        };
    }
    // Rust's full-string lowercase preserves contextual final sigma across ASCII spans.
    if !upper && text.contains('Σ') {
        return text.to_lowercase();
    }
    let mut output = String::with_capacity(text.len());
    let mut rest = text;
    while !rest.is_empty() {
        let bytes = rest.as_bytes();
        let mut ascii = 0;
        for chunk in bytes.as_chunks::<{ size_of::<usize>() }>().0 {
            if usize::from_ne_bytes(*chunk) & high_bits != 0 {
                break;
            }
            ascii += size_of::<usize>();
        }
        ascii += bytes[ascii..]
            .iter()
            .position(|b| !b.is_ascii())
            .unwrap_or(bytes.len() - ascii);
        if ascii != 0 {
            let start = output.len();
            output.push_str(&rest[..ascii]);
            if upper {
                output[start..].make_ascii_uppercase();
            } else {
                output[start..].make_ascii_lowercase();
            }
            rest = &rest[ascii..];
        }
        if let Some(character) = rest.chars().next() {
            if upper {
                output.extend(character.to_uppercase());
            } else {
                output.extend(character.to_lowercase());
            }
            rest = &rest[character.len_utf8()..];
        }
    }
    output
}

impl Vm {
    pub(super) fn percent_format(&mut self, template: Value, arguments: Value) -> Result<Value> {
        self.protocol_roots(&[template, arguments], |vm| {
            let template = vm
                .heap
                .as_str(&template)
                .ok_or(Error::Type("format requires str"))?
                .to_owned();
            let arguments = match vm.heap.get(arguments) {
                Ok(Object::Tuple(values)) => values.clone(),
                _ => vec![arguments],
            };
            let mut values = arguments.iter();
            let mut chars = template.chars();
            let mut output = String::new();
            while let Some(ch) = chars.next() {
                if ch != '%' {
                    output.push(ch);
                    continue;
                }
                let code = chars
                    .next()
                    .ok_or_else(|| error("ValueError: incomplete format"))?;
                if code == '%' {
                    output.push('%');
                    continue;
                }
                if !matches!(code, 's' | 'r' | 'd' | 'i' | 'u' | 'c') {
                    return Err(Error::Unsupported("percent format flags or conversion"));
                }
                let &value = values
                    .next()
                    .ok_or(Error::Type("not enough arguments for format string"))?;
                match code {
                    's' | 'r' => output.push_str(&vm.py_repr(value, code == 'r')?),
                    'd' | 'i' | 'u' => {
                        output.push_str(&crate::integer::get(&vm.heap, value)?.to_string())
                    }
                    'c' => {
                        if let Some(text) = vm.heap.as_str(&value) {
                            let mut chars = text.chars();
                            let ch = chars.next().ok_or(Error::Type("%c requires a character"))?;
                            if chars.next().is_some() {
                                return Err(Error::Type("%c requires a character"));
                            }
                            output.push(ch);
                        } else {
                            let code = vm.py_index(value)?;
                            output.push(
                                u32::try_from(code)
                                    .ok()
                                    .and_then(char::from_u32)
                                    .ok_or_else(|| {
                                        error("OverflowError: %c argument not in range")
                                    })?,
                            );
                        }
                    }
                    _ => unreachable!(),
                }
            }
            if values.next().is_some() {
                return Err(Error::Type(
                    "not all arguments converted during string formatting",
                ));
            }
            Ok(vm.heap.string_owned(output))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::convert_case;
    #[test]
    fn ascii_runs_preserve_expansions_and_context_at_word_boundaries() {
        for n in [0, 1, 7, 8, 9, 15, 16, 17, 1000] {
            for middle in ["ü中🐍ßİ", "AΣ AΣB", "中文代理请求", "ﬃᾀ"] {
                let text = format!("{}{middle}{}", "AbC".repeat(n), "xYz".repeat(n));
                assert_eq!(convert_case(&text, false), text.to_lowercase());
                assert_eq!(convert_case(&text, true), text.to_uppercase());
            }
        }
    }
}
