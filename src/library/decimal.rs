//! Finite base-ten coefficients retain their scale; context precision rounds arithmetic.
use super::*;
use num_bigint::BigInt;
use num_traits::{Signed, Zero};
#[derive(Debug, Clone)]
pub struct Decimal {
    pub coefficient: BigInt,
    pub exponent: i64,
}
#[derive(Debug, Clone)]
pub struct Context {
    pub precision: usize,
}
fn power(n: i64) -> Result<BigInt> {
    let n = u32::try_from(n).map_err(|_| invalid("decimal exponent outside supported range"))?;
    Ok(BigInt::from(10).pow(n))
}
impl Decimal {
    fn parse(text: &str) -> Result<Self> {
        let text = text.trim();
        let (mantissa, exponent) = if let Some((a, b)) = text.split_once(['e', 'E']) {
            (
                a,
                b.parse::<i64>()
                    .map_err(|_| invalid("invalid Decimal exponent"))?,
            )
        } else {
            (text, 0)
        };
        let (whole, fraction) = mantissa.split_once('.').unwrap_or((mantissa, ""));
        let digits = format!("{whole}{fraction}");
        let coefficient = digits
            .parse::<BigInt>()
            .map_err(|_| invalid("invalid Decimal literal"))?;
        Ok(Self {
            coefficient,
            exponent: exponent
                .checked_sub(fraction.len() as i64)
                .ok_or(Error::IntegerOverflow)?,
        })
    }
    fn rounded(mut self, precision: usize) -> Result<Self> {
        let digits = self.coefficient.abs().to_string().len();
        if digits > precision {
            let removed = digits - precision;
            let divisor = power(removed as i64)?;
            let quotient = &self.coefficient / &divisor;
            let remainder = (&self.coefficient % &divisor).abs();
            let twice = remainder * 2;
            let up = twice > divisor || (twice == divisor && (&quotient % 2u8) != BigInt::zero());
            self.coefficient = quotient
                + if up {
                    if self.coefficient.is_negative() {
                        -1
                    } else {
                        1
                    }
                } else {
                    0
                };
            self.exponent = self
                .exponent
                .checked_add(removed as i64)
                .ok_or(Error::IntegerOverflow)?;
            if self.coefficient.abs().to_string().len() > precision {
                self.coefficient /= 10;
                self.exponent += 1;
            }
        }
        Ok(self)
    }
}
impl Vm {
    fn decimal_value(&self, value: Value) -> Result<Decimal> {
        if let Ok(Object::Library(payload)) = self.heap.get(value)
            && let Payload::Decimal(decimal) = &**payload
        {
            return Ok(decimal.clone());
        }
        Ok(Decimal {
            coefficient: crate::integer::get(&self.heap, value)?,
            exponent: 0,
        })
    }
    fn decimal_context(&mut self) -> Value {
        if let Some(&value) = self.modules.get("decimal.current_context") {
            return value;
        }
        let value = self
            .heap
            .allocate(Object::Library(Box::new(Payload::DecimalContext(
                Context { precision: 28 },
            ))));
        self.modules.insert("decimal.current_context".into(), value);
        value
    }
    fn decimal_precision(&self) -> usize {
        self.modules
            .get("decimal.current_context")
            .and_then(|v| self.heap.get(*v).ok())
            .and_then(|o| match o {
                Object::Library(p) => match &**p {
                    Payload::DecimalContext(c) => Some(c.precision),
                    _ => None,
                },
                _ => None,
            })
            .unwrap_or(28)
    }
    pub(crate) fn is_decimal(&self, value: Value) -> bool {
        matches!(self.heap.get(value),Ok(Object::Library(p)) if matches!(&**p,Payload::Decimal(_)))
    }
    pub(crate) fn decimal_binary(&mut self, op: &str, a: Value, b: Value) -> Option<Result<Value>> {
        if !self.is_decimal(a) && !self.is_decimal(b) {
            return None;
        }
        Some((|| {
            let a = self.decimal_value(a)?;
            let b = self.decimal_value(b)?;
            let value = match op {
                "*" => Decimal {
                    coefficient: a.coefficient * b.coefficient,
                    exponent: a
                        .exponent
                        .checked_add(b.exponent)
                        .ok_or(Error::IntegerOverflow)?,
                },
                "+" | "-" | "//" => {
                    let exponent = a.exponent.min(b.exponent);
                    let a = a.coefficient * power(a.exponent - exponent)?;
                    let b = b.coefficient * power(b.exponent - exponent)?;
                    if op == "//" {
                        if b.is_zero() {
                            return Err(Error::Runtime("decimal.DivisionByZero".into()));
                        }
                        let coefficient = a / b;
                        if coefficient.abs().to_string().len() > self.decimal_precision() {
                            return Err(Error::Runtime(
                                "decimal.InvalidOperation: quotient exceeds precision".into(),
                            ));
                        }
                        Decimal {
                            coefficient,
                            exponent: 0,
                        }
                    } else {
                        Decimal {
                            coefficient: if op == "+" { a + b } else { a - b },
                            exponent,
                        }
                    }
                }
                _ => return Err(Error::Unsupported("Decimal arithmetic operator")),
            }
            .rounded(self.decimal_precision())?;
            Ok(self
                .heap
                .allocate(Object::Library(Box::new(Payload::Decimal(value)))))
        })())
    }
    pub(crate) fn decimal_comparison(&self, op: &str, a: Value, b: Value) -> Option<Result<Value>> {
        if !matches!(op, "==" | "!=" | "<" | "<=" | ">" | ">=")
            || (!self.is_decimal(a) && !self.is_decimal(b))
        {
            return None;
        }
        Some((|| {
            let a = self.decimal_value(a)?;
            let b = self.decimal_value(b)?;
            let e = a.exponent.min(b.exponent);
            let a = a.coefficient * power(a.exponent - e)?;
            let b = b.coefficient * power(b.exponent - e)?;
            Ok(Value::Bool(match op {
                "==" => a == b,
                "!=" => a != b,
                "<" => a < b,
                "<=" => a <= b,
                ">" => a > b,
                _ => a >= b,
            }))
        })())
    }
    pub(crate) fn library_decimal(
        &mut self,
        name: &str,
        args: &[Value],
        keywords: &[(String, Value)],
    ) -> Result<Value> {
        if !keywords.is_empty() {
            return Err(Error::Unsupported("Decimal keyword arguments"));
        }
        match name {
            "Decimal" => {
                arity(args, 0, 1)?;
                let value = if let Some(&value) = args.first() {
                    if self.is_decimal(value) {
                        return Ok(value);
                    }
                    if let Some(text) = self.heap.as_str(&value) {
                        Decimal::parse(text)?
                    } else {
                        self.decimal_value(value)?
                    }
                } else {
                    Decimal {
                        coefficient: BigInt::zero(),
                        exponent: 0,
                    }
                };
                Ok(self
                    .heap
                    .allocate(Object::Library(Box::new(Payload::Decimal(value)))))
            }
            "getcontext" => {
                arity(args, 0, 0)?;
                Ok(self.decimal_context())
            }
            "localcontext" => {
                arity(args, 0, 1)?;
                let value = args
                    .first()
                    .copied()
                    .unwrap_or_else(|| self.decimal_context());
                let Object::Library(payload) = self.heap.get(value)? else {
                    return Err(Error::Type("expected decimal context"));
                };
                let Payload::DecimalContext(context) = &**payload else {
                    return Err(Error::Type("expected decimal context"));
                };
                Ok(self
                    .heap
                    .allocate(Object::Library(Box::new(Payload::DecimalContext(
                        context.clone(),
                    )))))
            }
            _ => Err(Error::Unsupported("decimal library service")),
        }
    }
    pub(crate) fn decimal_tuple(&self, receiver: Value, args: &[Value]) -> Result<Value> {
        arity(args, 0, 0)?;
        let decimal = self.decimal_value(receiver)?;
        let digits = decimal
            .coefficient
            .abs()
            .to_string()
            .bytes()
            .map(|d| Value::Int(i64::from(d - b'0')))
            .collect();
        let digits = self.heap.allocate(Object::Tuple(digits));
        Ok(self
            .heap
            .allocate(Object::Library(Box::new(Payload::DecimalTuple {
                sign: decimal.coefficient.is_negative(),
                digits,
                exponent: decimal.exponent,
            }))))
    }
    pub(crate) fn library_set_attribute(
        &mut self,
        object: Value,
        name: &str,
        value: Value,
    ) -> Result<bool> {
        if !matches!(self.heap.get(object),Ok(Object::Library(p)) if matches!(&**p,Payload::DecimalContext(_)))
        {
            return Ok(false);
        }
        if name != "prec" {
            return Err(Error::Unsupported("decimal context attribute"));
        }
        let precision = usize::try_from(crate::execution::integer(value)?)
            .ok()
            .filter(|p| *p > 0)
            .ok_or_else(|| invalid("precision must be positive"))?;
        let mut object = self.heap.get_mut(object)?;
        let Object::Library(p) = &mut *object else {
            unreachable!()
        };
        let Payload::DecimalContext(c) = &mut **p else {
            unreachable!()
        };
        c.precision = precision;
        Ok(true)
    }
    pub(crate) fn with_decimal_context(
        &mut self,
        body: &crate::hir::Function,
        manager: Value,
        environment: Value,
    ) -> Result<Value> {
        let previous = self.decimal_context();
        self.protocol_roots(&[previous, manager], |vm| {
            vm.modules.insert("decimal.current_context".into(), manager);
            let function = vm
                .try_bodies
                .entry((body.id(), body.revision()))
                .or_insert_with(|| std::rc::Rc::new(std::cell::RefCell::new(body.clone())))
                .clone();
            let result = vm.execute_bound_frame(
                function,
                environment,
                &[manager],
                Some(crate::vm::PositionalBindings {
                    defaults: &[],
                    count: 1,
                }),
            );
            vm.modules
                .insert("decimal.current_context".into(), previous);
            result
        })
    }
}
