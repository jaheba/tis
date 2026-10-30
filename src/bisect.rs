//! Binary search over builtin sequences; key callbacks retain ordinary Python evaluation order.
use crate::{
    Error, Heap, Object, Result, Value, Vm,
    execution::{compare, integer},
};

impl Vm {
    pub(crate) fn bisect(
        &mut self,
        name: &str,
        args: &[Value],
        keywords: &[(String, Value)],
    ) -> Result<Value> {
        if args.len() > 4 {
            return Err(Error::Type(
                "bisect accepts at most four positional arguments",
            ));
        }
        let mut bound = [None; 5];
        for (slot, value) in bound.iter_mut().zip(args) {
            *slot = Some(*value);
        }
        for (name, value) in keywords {
            let index = match name.as_str() {
                "a" => 0,
                "x" => 1,
                "lo" => 2,
                "hi" => 3,
                "key" => 4,
                _ => return Err(Error::Type("unexpected bisect keyword")),
            };
            if bound[index].replace(*value).is_some() {
                return Err(Error::Type("multiple values for bisect argument"));
            }
        }
        let sequence = bound[0].ok_or(Error::Type("bisect requires a sequence"))?;
        let value = bound[1].ok_or(Error::Type("bisect requires a search value"))?;
        let mut lo = integer(bound[2].unwrap_or(Value::Int(0)))?;
        if lo < 0 {
            return Err(Error::Runtime("ValueError: lo must be non-negative".into()));
        }
        let hi = match bound[3].unwrap_or(Value::None) {
            Value::None => -1,
            other => integer(other)?,
        };
        let key = bound[4].unwrap_or(Value::None);
        let insert = name.starts_with("bisect.insort");
        let left = name.ends_with("_left");
        if key == Value::None
            && let Ok(index) = search(&self.heap, sequence, value, lo, hi, left)
        {
            if insert {
                let mut object = self.heap.get_mut(sequence)?;
                let Object::List(values) = &mut *object else {
                    return Err(Error::Type("insort requires a list"));
                };
                values.insert((index as usize).min(values.len()), value);
                return Ok(Value::None);
            }
            return Ok(Value::Int(index));
        }
        let root_start = self.transient_roots.len();
        self.transient_roots.extend([sequence, value, key]);
        let result = (|| {
            // insort applies key to x once, then searches using key(midpoint). Search leaves x unchanged.
            let search = if insert && key != Value::None {
                self.call(key, &[value])?
            } else {
                value
            };
            self.transient_roots.push(search);
            let mut hi = if hi == -1 { self.py_len(sequence)? } else { hi };
            while lo < hi {
                let mid = lo + (hi - lo) / 2;
                let candidate = self.py_get_item(sequence, Value::Int(mid))?;
                let candidate = if key == Value::None {
                    candidate
                } else {
                    self.call(key, &[candidate])?
                };
                let advance = if left {
                    self.protocol_roots(&[candidate], |vm| {
                        vm.py_compare_bool("<", candidate, search)
                    })?
                } else {
                    !self.protocol_roots(&[candidate], |vm| {
                        vm.py_compare_bool("<", search, candidate)
                    })?
                };
                if advance {
                    lo = mid + 1;
                } else {
                    hi = mid;
                }
            }
            if insert {
                let mut object = self.heap.get_mut(sequence)?;
                let Object::List(values) = &mut *object else {
                    return Err(Error::Type("insort requires a list"));
                };
                values.insert((lo as usize).min(values.len()), value);
                Ok(Value::None)
            } else {
                Ok(Value::Int(lo))
            }
        })();
        self.transient_roots.truncate(root_start);
        result
    }
}

/// No Python calls, VM allocation or collection: list/tuple storage is borrowed once for the search.
/// Other supported builtin sequences use get_item; a string character always fits inline.
pub(crate) fn search(
    heap: &Heap,
    sequence: Value,
    value: Value,
    mut lo: i64,
    hi: i64,
    left: bool,
) -> Result<i64> {
    if lo < 0 {
        return Err(Error::Runtime("ValueError: lo must be non-negative".into()));
    }
    let mut hi = if hi == -1 {
        i64::try_from(heap.len(sequence)?).map_err(|_| Error::IntegerOverflow)?
    } else {
        hi
    };
    let object = heap.get(sequence).ok();
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        let index = usize::try_from(mid).map_err(|_| Error::Index)?;
        let candidate = match object {
            Some(Object::List(values)) => values.value(index).ok_or(Error::Index)?,
            Some(Object::Tuple(values)) => *values.get(index).ok_or(Error::Index)?,
            _ => heap.get_item(sequence, Value::Int(mid))?,
        };
        let (a, b) = if left {
            (candidate, value)
        } else {
            (value, candidate)
        };
        let less = if let (Value::Int(a), Value::Int(b)) = (a, b) {
            a < b
        } else {
            compare(heap, "<", a, b)?
        };
        if less == left {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    Ok(lo)
}
