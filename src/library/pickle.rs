//! Protocol 4/5 pickle streams use checked operands and interpreter-owned values.
use super::*;
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Date {
    pub year: u16,
    pub month: u8,
    pub day: u8,
}
fn corrupt(message: &str) -> Error {
    Error::Runtime(format!("pickle.UnpicklingError: {message}"))
}
fn put_bytes(out: &mut Vec<u8>, data: &[u8], text: bool) -> Result<()> {
    if data.len() < 256 {
        out.extend_from_slice(&[if text { 0x8c } else { b'C' }, data.len() as u8]);
    } else {
        out.push(if text { b'X' } else { b'B' });
        out.extend_from_slice(
            &u32::try_from(data.len())
                .map_err(|_| Error::IntegerOverflow)?
                .to_le_bytes(),
        );
    }
    out.extend_from_slice(data);
    Ok(())
}
struct Reader<'a> {
    data: &'a [u8],
    position: usize,
}
impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self
            .position
            .checked_add(n)
            .ok_or_else(|| corrupt("operand overflow"))?;
        let value = self
            .data
            .get(self.position..end)
            .ok_or_else(|| corrupt("truncated stream"))?;
        self.position = end;
        Ok(value)
    }
    fn byte(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }
    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn size(&mut self) -> Result<usize> {
        usize::try_from(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
            .map_err(|_| corrupt("oversized operand"))
    }
    fn line(&mut self) -> Result<&'a [u8]> {
        let n = self
            .data
            .get(self.position..)
            .ok_or_else(|| corrupt("truncated line"))?
            .iter()
            .position(|b| *b == b'\n')
            .ok_or_else(|| corrupt("unterminated line"))?;
        let value = self.take(n)?;
        self.take(1)?;
        Ok(value)
    }
}
fn pop(stack: &mut Vec<Value>) -> Result<Value> {
    stack.pop().ok_or_else(|| corrupt("stack underflow"))
}
fn marked(stack: &mut Vec<Value>, marks: &mut Vec<usize>) -> Result<Vec<Value>> {
    let mark = marks.pop().ok_or_else(|| corrupt("missing MARK"))?;
    if mark > stack.len() {
        return Err(corrupt("invalid MARK"));
    }
    Ok(stack.split_off(mark))
}
impl Vm {
    pub(crate) fn library_date(
        &mut self,
        args: &[Value],
        keywords: &[(String, Value)],
    ) -> Result<Value> {
        if args.len() > 3 {
            return Err(Error::Type("date requires year, month and day"));
        }
        let mut values = [None; 3];
        for (slot, &value) in values.iter_mut().zip(args) {
            *slot = Some(value);
        }
        for (name, &value) in keywords.iter().map(|(k, v)| (k, v)) {
            let slot = match name.as_str() {
                "year" => 0,
                "month" => 1,
                "day" => 2,
                _ => return Err(Error::Type("invalid date keyword")),
            };
            if values[slot].replace(value).is_some() {
                return Err(Error::Type("multiple values for date argument"));
            }
        }
        let [year, month, day] = values.map(|v| {
            v.ok_or(Error::Type("missing date argument"))
                .and_then(crate::execution::integer)
        });
        let (year, month, day) = (year?, month?, day?);
        if !(1..=9999).contains(&year) || !(1..=12).contains(&month) {
            return Err(invalid("date year or month out of range"));
        }
        let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
        let days = match month {
            2 => {
                if leap {
                    29
                } else {
                    28
                }
            }
            4 | 6 | 9 | 11 => 30,
            _ => 31,
        };
        if !(1..=days).contains(&day) {
            return Err(invalid("day out of range for month"));
        }
        Ok(self
            .heap
            .allocate(Object::Library(Box::new(Payload::Date(Date {
                year: year as u16,
                month: month as u8,
                day: day as u8,
            })))))
    }
    pub(crate) fn date_comparison(&self, op: &str, a: Value, b: Value) -> Option<Result<Value>> {
        let date = |value| match self.heap.get(value) {
            Ok(Object::Library(p)) => match &**p {
                Payload::Date(d) => Some(*d),
                _ => None,
            },
            _ => None,
        };
        let (a, b) = (date(a), date(b));
        if a.is_none() && b.is_none() {
            return None;
        }
        if !matches!(op, "==" | "!=" | "<" | "<=" | ">" | ">=") {
            return None;
        }
        Some(match (a, b) {
            (Some(a), Some(b)) => Ok(Value::Bool(match op {
                "==" => a == b,
                "!=" => a != b,
                "<" => a < b,
                "<=" => a <= b,
                ">" => a > b,
                _ => a >= b,
            })),
            _ if op == "==" => Ok(Value::Bool(false)),
            _ if op == "!=" => Ok(Value::Bool(true)),
            _ => Err(Error::Type("date ordering requires another date")),
        })
    }
    pub(crate) fn library_pickle(
        &mut self,
        name: &str,
        args: &[Value],
        keywords: &[(String, Value)],
    ) -> Result<Value> {
        if !keywords.is_empty() {
            return Err(Error::Unsupported("pickle keyword arguments"));
        }
        if matches!(name, "dumps" | "_dumps") {
            arity(args, 1, 2)?;
            let protocol = args
                .get(1)
                .map(|v| crate::execution::integer(*v))
                .transpose()?
                .unwrap_or(5);
            if !matches!(protocol, -1 | 4 | 5) {
                return Err(Error::Unsupported("pickle protocol before 4"));
            }
            let mut bytes = vec![0x80, if protocol == 4 { 4 } else { 5 }];
            self.pickle_encode(args[0], &mut bytes, &mut vec![], 0)?;
            bytes.push(b'.');
            return Ok(self.heap.allocate(Object::Bytes(bytes)));
        }
        arity(args, 1, 1)?;
        let object = self.heap.get(args[0])?;
        let (Object::Bytes(data) | Object::ByteArray(data)) = object else {
            return Err(Error::Type("pickle input must be bytes-like"));
        };
        let data = data.clone();
        let roots = self.transient_roots.len();
        let result = self.pickle_decode(&data);
        self.transient_roots.truncate(roots);
        result
    }
    fn pickle_memo(value: Value, out: &mut Vec<u8>, memo: &mut Vec<Value>) {
        if matches!(value, Value::Ref(_)) {
            memo.push(value);
            out.push(0x94);
        }
    }
    fn pickle_encode(
        &mut self,
        value: Value,
        out: &mut Vec<u8>,
        memo: &mut Vec<Value>,
        depth: usize,
    ) -> Result<()> {
        if depth > 128 {
            return Err(Error::Runtime("RecursionError: pickle nesting".into()));
        }
        if matches!(value, Value::Ref(_))
            && let Some(index) = memo.iter().position(|v| *v == value)
        {
            if index < 256 {
                out.extend_from_slice(&[b'h', index as u8]);
            } else {
                out.push(b'j');
                out.extend_from_slice(&(index as u32).to_le_bytes());
            }
            return Ok(());
        }
        if let Some(text) = self.heap.as_str(&value) {
            put_bytes(out, text.as_bytes(), true)?;
            Self::pickle_memo(value, out, memo);
            return Ok(());
        }
        match value {
            Value::None => out.push(b'N'),
            Value::Bool(b) => out.push(if b { 0x88 } else { 0x89 }),
            Value::Int(n) => {
                if (0..256).contains(&n) {
                    out.extend_from_slice(&[b'K', n as u8]);
                } else if (0..65536).contains(&n) {
                    out.push(b'M');
                    out.extend_from_slice(&(n as u16).to_le_bytes());
                } else if let Ok(n) = i32::try_from(n) {
                    out.push(b'J');
                    out.extend_from_slice(&n.to_le_bytes());
                } else {
                    let bytes = num_bigint::BigInt::from(n).to_signed_bytes_le();
                    out.push(0x8a);
                    out.push(bytes.len() as u8);
                    out.extend(bytes);
                }
            }
            Value::Float(n) => {
                out.push(b'G');
                out.extend_from_slice(&n.to_be_bytes());
            }
            Value::Ref(_) => match self.heap.get(value)?.clone() {
                Object::BigInt(n) => {
                    let bytes = n.to_signed_bytes_le();
                    if bytes.len() < 256 {
                        out.extend_from_slice(&[0x8a, bytes.len() as u8]);
                    } else {
                        out.push(0x8b);
                        out.extend_from_slice(
                            &u32::try_from(bytes.len())
                                .map_err(|_| Error::IntegerOverflow)?
                                .to_le_bytes(),
                        );
                    }
                    out.extend(bytes);
                    Self::pickle_memo(value, out, memo);
                }
                Object::Bytes(bytes) => {
                    put_bytes(out, &bytes, false)?;
                    Self::pickle_memo(value, out, memo);
                }
                Object::ByteArray(bytes) => {
                    if out.get(1) == Some(&4) {
                        return Err(Error::Unsupported("bytearray pickle protocol 4"));
                    }
                    out.push(0x96);
                    out.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
                    out.extend(bytes);
                    Self::pickle_memo(value, out, memo);
                }
                Object::List(items) => {
                    out.push(b']');
                    Self::pickle_memo(value, out, memo);
                    if !items.is_empty() {
                        out.push(b'(');
                        for item in items {
                            self.pickle_encode(item, out, memo, depth + 1)?;
                        }
                        out.push(b'e');
                    }
                }
                Object::Tuple(items) => {
                    if items.is_empty() {
                        out.push(b')');
                    } else {
                        if items.len() > 3 {
                            out.push(b'(');
                        }
                        for &item in &items {
                            self.pickle_encode(item, out, memo, depth + 1)?;
                        }
                        out.push(match items.len() {
                            1 => 0x85,
                            2 => 0x86,
                            3 => 0x87,
                            _ => b't',
                        });
                    }
                    Self::pickle_memo(value, out, memo);
                }
                Object::Dict(items) => {
                    out.push(b'}');
                    Self::pickle_memo(value, out, memo);
                    if !items.is_empty() {
                        out.push(b'(');
                        for (key, item) in items.iter() {
                            self.pickle_encode(
                                crate::execution::key_value(&self.heap, key),
                                out,
                                memo,
                                depth + 1,
                            )?;
                            self.pickle_encode(*item, out, memo, depth + 1)?;
                        }
                        out.push(b'u');
                    }
                }
                Object::Library(p) => {
                    let Payload::Date(date) = *p else {
                        return Err(Error::Unsupported("pickle library object"));
                    };
                    out.extend_from_slice(b"cdatetime\ndate\n");
                    put_bytes(
                        out,
                        &[
                            (date.year >> 8) as u8,
                            date.year as u8,
                            date.month,
                            date.day,
                        ],
                        false,
                    )?;
                    out.extend_from_slice(&[0x85, b'R']);
                    Self::pickle_memo(value, out, memo);
                }
                _ => return Err(Error::Unsupported("pickle object type")),
            },
            _ => return Err(Error::InvalidIr("pickle internal value")),
        }
        Ok(())
    }
    fn pickle_global(&mut self, module: &[u8], name: &[u8]) -> Result<Value> {
        if module == b"datetime" && name == b"date" {
            self.library_attribute("datetime", "date")
        } else {
            Err(Error::Unsupported("pickle global"))
        }
    }
    fn pickle_decode(&mut self, data: &[u8]) -> Result<Value> {
        let mut reader = Reader { data, position: 0 };
        let mut stack = Vec::new();
        let mut marks = vec![];
        let mut memo = std::collections::BTreeMap::<usize, Value>::new();
        loop {
            let opcode = reader.byte()?;
            let value = match opcode {
                b'.' => {
                    if stack.len() != 1 || !marks.is_empty() {
                        return Err(corrupt("invalid final stack"));
                    }
                    return pop(&mut stack);
                }
                0x80 => {
                    let protocol = reader.byte()?;
                    if protocol > 5 {
                        return Err(corrupt("unsupported protocol"));
                    }
                    None
                }
                0x95 => {
                    let size = reader.size()?;
                    if size > data.len() - reader.position {
                        return Err(corrupt("truncated frame"));
                    }
                    None
                }
                b'(' => {
                    marks.push(stack.len());
                    None
                }
                b'N' => Some(Value::None),
                0x88 => Some(Value::Bool(true)),
                0x89 => Some(Value::Bool(false)),
                b'K' => Some(Value::Int(i64::from(reader.byte()?))),
                b'M' => Some(Value::Int(i64::from(u16::from_le_bytes(
                    reader.take(2)?.try_into().unwrap(),
                )))),
                b'J' => Some(Value::Int(i64::from(i32::from_le_bytes(
                    reader.take(4)?.try_into().unwrap(),
                )))),
                b'G' => Some(Value::Float(f64::from_be_bytes(
                    reader.take(8)?.try_into().unwrap(),
                ))),
                0x8a | 0x8b => {
                    let size = if opcode == 0x8a {
                        usize::from(reader.byte()?)
                    } else {
                        reader.u32()? as usize
                    };
                    Some(crate::integer::value(
                        &self.heap,
                        num_bigint::BigInt::from_signed_bytes_le(reader.take(size)?),
                    ))
                }
                b'C' | b'B' | 0x8e | 0x96 | 0x8c | b'X' | 0x8d => {
                    let size = match opcode {
                        b'C' | 0x8c => usize::from(reader.byte()?),
                        b'B' | b'X' => reader.u32()? as usize,
                        _ => reader.size()?,
                    };
                    let bytes = reader.take(size)?;
                    Some(if matches!(opcode, 0x8c | b'X' | 0x8d) {
                        self.heap.string(
                            std::str::from_utf8(bytes).map_err(|_| corrupt("invalid UTF-8"))?,
                        )
                    } else {
                        self.heap.allocate(if opcode == 0x96 {
                            Object::ByteArray(bytes.to_vec())
                        } else {
                            Object::Bytes(bytes.to_vec())
                        })
                    })
                }
                b']' => Some(self.heap.allocate(Object::List(Vec::new().into()))),
                b'}' => Some(self.heap.allocate(Object::Dict(Default::default()))),
                b')' => Some(self.heap.allocate(Object::Tuple(vec![]))),
                b't' | 0x85..=0x87 => {
                    let values = if opcode == b't' {
                        marked(&mut stack, &mut marks)?
                    } else {
                        let count = usize::from(opcode - 0x84);
                        if stack.len() < count {
                            return Err(corrupt("tuple stack underflow"));
                        }
                        stack.split_off(stack.len() - count)
                    };
                    Some(self.heap.allocate(Object::Tuple(values)))
                }
                b'a' | b'e' => {
                    let values = if opcode == b'a' {
                        vec![pop(&mut stack)?]
                    } else {
                        marked(&mut stack, &mut marks)?
                    };
                    let target = *stack.last().ok_or_else(|| corrupt("missing list"))?;
                    let mut object = self.heap.get_mut(target)?;
                    let Object::List(items) = &mut *object else {
                        return Err(corrupt("APPENDS requires list"));
                    };
                    for value in values {
                        items.push(value);
                    }
                    None
                }
                b's' | b'u' => {
                    let values = if opcode == b's' {
                        let value = pop(&mut stack)?;
                        vec![pop(&mut stack)?, value]
                    } else {
                        marked(&mut stack, &mut marks)?
                    };
                    if values.len() % 2 != 0 {
                        return Err(corrupt("odd SETITEMS count"));
                    }
                    let target = *stack.last().ok_or_else(|| corrupt("missing dictionary"))?;
                    for pair in values.as_chunks::<2>().0 {
                        self.heap.set_item(target, pair[0], pair[1])?;
                    }
                    None
                }
                0x94 => {
                    memo.insert(
                        memo.len(),
                        *stack.last().ok_or_else(|| corrupt("empty memo stack"))?,
                    );
                    None
                }
                b'q' | b'r' => {
                    let index = if opcode == b'q' {
                        usize::from(reader.byte()?)
                    } else {
                        reader.u32()? as usize
                    };
                    if index > data.len() {
                        return Err(corrupt("memo index out of range"));
                    }
                    memo.insert(
                        index,
                        *stack.last().ok_or_else(|| corrupt("empty memo stack"))?,
                    );
                    None
                }
                b'h' | b'j' => {
                    let index = if opcode == b'h' {
                        usize::from(reader.byte()?)
                    } else {
                        reader.u32()? as usize
                    };
                    Some(
                        memo.get(&index)
                            .copied()
                            .ok_or_else(|| corrupt("unknown memo index"))?,
                    )
                }
                b'c' => {
                    let module = reader.line()?;
                    let name = reader.line()?;
                    Some(self.pickle_global(module, name)?)
                }
                0x93 => {
                    let name = pop(&mut stack)?;
                    let module = pop(&mut stack)?;
                    let name = self
                        .heap
                        .as_str(&name)
                        .ok_or_else(|| corrupt("global name must be str"))?
                        .as_bytes()
                        .to_vec();
                    let module = self
                        .heap
                        .as_str(&module)
                        .ok_or_else(|| corrupt("global module must be str"))?
                        .as_bytes()
                        .to_vec();
                    Some(self.pickle_global(&module, &name)?)
                }
                b'R' => {
                    let arguments = pop(&mut stack)?;
                    let callable = pop(&mut stack)?;
                    let Object::Native(name) = self.heap.get(callable)? else {
                        return Err(corrupt("unsupported callable"));
                    };
                    if name != "datetime.date" {
                        return Err(corrupt("unsupported reduce callable"));
                    }
                    let Object::Tuple(args) = self.heap.get(arguments)? else {
                        return Err(corrupt("REDUCE arguments must be tuple"));
                    };
                    if args.len() != 1 {
                        return Err(corrupt("invalid date reduction"));
                    }
                    let Object::Bytes(bytes) = self.heap.get(args[0])? else {
                        return Err(corrupt("date state must be bytes"));
                    };
                    if bytes.len() != 4 {
                        return Err(corrupt("invalid date state"));
                    }
                    let args = [
                        Value::Int(i64::from(u16::from_be_bytes([bytes[0], bytes[1]]))),
                        Value::Int(i64::from(bytes[2])),
                        Value::Int(i64::from(bytes[3])),
                    ];
                    Some(self.library_date(&args, &[])?)
                }
                _ => return Err(corrupt(&format!("unsupported opcode 0x{opcode:02x}"))),
            };
            if let Some(value) = value {
                self.transient_roots.push(value);
                stack.push(value);
            }
        }
    }
}
