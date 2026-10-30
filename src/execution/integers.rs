//! Python integer construction and methods; arithmetic lives in the frame-independent service.
use super::*;
use num_bigint::BigInt;
use num_traits::FromPrimitive;

impl Vm {
    pub(super) fn construct_integer(
        &mut self,
        args: &[Value],
        keywords: &[(String, Value)],
    ) -> Result<Value> {
        arity(args, 0, 2)?;
        let mut base = args.get(1).copied();
        for (name, value) in keywords {
            if name != "base" {
                return Err(error("TypeError: invalid keyword argument for int()"));
            }
            if base.replace(*value).is_some() {
                return Err(error("TypeError: multiple values for base"));
            }
        }
        let Some(&value) = args.first() else {
            return if base.is_some() {
                Err(error("TypeError: int() missing string argument"))
            } else {
                Ok(Value::Int(0))
            };
        };
        // Bytes accept ASCII input; UTF-8 decoding must not admit Unicode whitespace.
        let (text, bytes_ascii) = if let Some(text) = self.heap.as_str(&value) {
            (Some(text.to_owned()), true)
        } else if let Ok(Object::Bytes(bytes)) = self.heap.get(value) {
            (
                Some(String::from_utf8_lossy(bytes).into_owned()),
                bytes.is_ascii(),
            )
        } else {
            (None, true)
        };
        if let Some(text) = text {
            let bad_base = || error("ValueError: int() base must be >= 2 and <= 36, or 0");
            let base = match base {
                Some(n) => {
                    let n = self.integer_argument(n)?;
                    u32::try_from(crate::integer::index(&self.heap, n).map_err(|_| bad_base())?)
                        .map_err(|_| bad_base())?
                }
                None => 10,
            };
            if base != 0 && !(2..=36).contains(&base) {
                return Err(bad_base());
            }
            let parsed = if bytes_ascii {
                crate::integer::parse(&text, base)
            } else {
                Err(error("ValueError: invalid literal for int()"))
            };
            let n = parsed.map_err(|_| {
                error(format!(
                    "ValueError: invalid literal for int() with base {base}: {}",
                    self.repr(value, true).unwrap_or_default()
                ))
            })?;
            return Ok(crate::integer::value(&self.heap, n));
        }
        if base.is_some() {
            return Err(error(
                "TypeError: int() can't convert non-string with explicit base",
            ));
        }
        if crate::integer::is_integer(&self.heap, value) {
            return Ok(match value {
                Value::Bool(n) => Value::Int(i64::from(n)),
                _ => value,
            });
        }
        if let Value::Float(n) = value {
            if n.is_nan() {
                return Err(error("ValueError: cannot convert float NaN to integer"));
            }
            let n = BigInt::from_f64(n)
                .ok_or_else(|| error("OverflowError: cannot convert float infinity to integer"))?;
            return Ok(crate::integer::value(&self.heap, n));
        }
        for name in ["__int__", "__index__"] {
            if let Some(n) = self.call_special(value, name, &[])? {
                if !crate::integer::is_integer(&self.heap, n) {
                    return Err(error(format!("TypeError: {name} returned non-int")));
                }
                return Ok(match n {
                    Value::Bool(n) => Value::Int(i64::from(n)),
                    _ => n,
                });
            }
        }
        Err(Error::Type("int() requires a string or number"))
    }

    pub(super) fn integer_argument(&mut self, value: Value) -> Result<Value> {
        if !crate::integer::is_integer(&self.heap, value)
            && self.special(value, "__index__")?.is_none()
        {
            return Err(error(format!(
                "TypeError: '{}' object cannot be interpreted as an integer",
                self.value_type_name(value)?
            )));
        }
        self.py_index_value(value)
    }

    pub(super) fn integer_method(
        &self,
        receiver: Value,
        name: &str,
        args: &[Value],
    ) -> Result<Value> {
        arity(args, 0, 0)?;
        let n = crate::integer::get(&self.heap, receiver)?;
        let result = match name {
            "bit_length" => n.bits(),
            "bit_count" => n
                .magnitude()
                .iter_u64_digits()
                .map(|n| u64::from(n.count_ones()))
                .sum(),
            _ => return Err(Error::Unsupported("integer method")),
        };
        Ok(crate::integer::value(&self.heap, BigInt::from(result)))
    }
}
