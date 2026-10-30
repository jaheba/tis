//! Arbitrary-precision integer fallback. Small arithmetic stays in the existing inline paths.
use crate::{Error, Heap, Key, Object, Result, Value};
use num_bigint::BigInt;
use num_integer::Integer;
use num_traits::{FromPrimitive, One, Signed, ToPrimitive, Zero};
use std::{
    cmp::Ordering,
    hash::{Hash, Hasher},
};

pub(crate) fn is_big(heap: &Heap, value: Value) -> bool {
    matches!(value, Value::Ref(_)) && matches!(heap.get(value), Ok(Object::BigInt(_)))
}
pub(crate) fn is_integer(heap: &Heap, value: Value) -> bool {
    matches!(value, Value::Int(_) | Value::Bool(_)) || is_big(heap, value)
}
pub(crate) fn value(heap: &Heap, n: BigInt) -> Value {
    match n.to_i64() {
        Some(n) => Value::Int(n),
        None => heap.allocate(Object::BigInt(n)),
    }
}
pub(crate) fn get(heap: &Heap, value: Value) -> Result<BigInt> {
    match value {
        Value::Int(n) => Ok(n.into()),
        Value::Bool(n) => Ok(i64::from(n).into()),
        _ => match heap.get(value) {
            Ok(Object::BigInt(n)) => Ok(n.clone()),
            _ => Err(Error::Type("expected integer")),
        },
    }
}
pub(crate) fn index(heap: &Heap, value: Value) -> Result<i64> {
    match value {
        Value::Int(n) => Ok(n),
        Value::Bool(n) => Ok(i64::from(n)),
        _ => get(heap, value)?.to_i64().ok_or(Error::IntegerOverflow),
    }
}
pub(crate) fn number(heap: &Heap, value: Value) -> Result<f64> {
    match value {
        Value::Float(n) => Ok(n),
        Value::Int(n) => Ok(n as f64),
        Value::Bool(n) => Ok(f64::from(n)),
        _ => {
            let n = get(heap, value)?.to_f64().filter(|n| n.is_finite());
            n.ok_or_else(|| {
                Error::Runtime("OverflowError: int too large to convert to float".into())
            })
        }
    }
}
fn fail(message: &str) -> Error {
    Error::Runtime(message.into())
}

pub(crate) fn parse(text: &str, base: u32) -> Result<BigInt> {
    if base != 0 && !(2..=36).contains(&base) {
        return Err(fail("ValueError: int() base must be >= 2 and <= 36, or 0"));
    }
    let invalid = || fail("ValueError: invalid literal for int()");
    let mut text = text.trim();
    let negative = text.starts_with('-');
    if text.starts_with(['+', '-']) {
        text = &text[1..];
    }
    let prefix = if text.starts_with("0x") || text.starts_with("0X") {
        16
    } else if text.starts_with("0o") || text.starts_with("0O") {
        8
    } else if text.starts_with("0b") || text.starts_with("0B") {
        2
    } else {
        0
    };
    let actual = if base == 0 {
        if prefix == 0 { 10 } else { prefix }
    } else {
        base
    };
    if prefix == actual {
        text = &text[2..];
        if text.starts_with('_') {
            text = &text[1..];
        }
    }
    if text.is_empty() || text.starts_with('_') || text.ends_with('_') || text.contains("__") {
        return Err(invalid());
    }
    let mut digits = String::with_capacity(text.len());
    for ch in text.chars() {
        if ch == '_' {
            continue;
        }
        if ch.to_digit(actual).is_none() {
            return Err(invalid());
        }
        digits.push(ch);
    }
    if base == 0 && prefix == 0 && digits.starts_with('0') && digits.chars().any(|c| c != '0') {
        return Err(invalid());
    }
    let n = BigInt::parse_bytes(digits.as_bytes(), actual).ok_or_else(invalid)?;
    Ok(if negative { -n } else { n })
}

pub(crate) fn unary(heap: &Heap, op: &str, n: Value) -> Result<Value> {
    if let Value::Int(n) = n {
        let result = match op {
            "+" => Some(n),
            "-" => n.checked_neg(),
            "~" => Some(!n),
            "abs" => n.checked_abs(),
            _ => None,
        };
        if let Some(n) = result {
            return Ok(Value::Int(n));
        }
    }
    let n = get(heap, n)?;
    Ok(value(
        heap,
        match op {
            "+" => n,
            "-" => -n,
            "~" => !n,
            "abs" => n.abs(),
            _ => return Err(Error::Unsupported("integer unary operator")),
        },
    ))
}

pub(crate) fn binary(heap: &Heap, op: &str, a: Value, b: Value) -> Result<Value> {
    if matches!(a, Value::Float(_)) || matches!(b, Value::Float(_)) {
        let (a, b) = (number(heap, a)?, number(heap, b)?);
        let n = match op {
            "+" => a + b,
            "-" => a - b,
            "*" => a * b,
            "/" | "//" | "%" if b == 0.0 => {
                return Err(fail("ZeroDivisionError: float division by zero"));
            }
            "/" => a / b,
            "//" => float_divmod(a, b)?.0,
            "%" => float_divmod(a, b)?.1,
            "**" => {
                if a == 0.0 && b < 0.0 {
                    return Err(fail(
                        "ZeroDivisionError: zero cannot be raised to a negative power",
                    ));
                }
                let n = a.powf(b);
                if n.is_nan() {
                    return Err(Error::Unsupported("complex numbers"));
                }
                n
            }
            _ => return Err(Error::Type("integer operands required")),
        };
        return Ok(Value::Float(n));
    }
    let (a, b) = (get(heap, a)?, get(heap, b)?);
    if matches!(op, "/" | "//" | "%") && b.is_zero() {
        return Err(fail(
            "ZeroDivisionError: integer division or modulo by zero",
        ));
    }
    if op == "/" {
        return true_divide(&a, &b).map(Value::Float);
    }
    if op == "**" && b.is_negative() {
        if a.is_zero() {
            return Err(fail(
                "ZeroDivisionError: zero cannot be raised to a negative power",
            ));
        }
        let convert = |n: &BigInt| {
            n.to_f64()
                .filter(|n| n.is_finite())
                .ok_or_else(|| fail("OverflowError: int too large to convert to float"))
        };
        return Ok(Value::Float(convert(&a)?.powf(convert(&b)?)));
    }
    let n = match op {
        "+" => a + b,
        "-" => a - b,
        "*" => a * b,
        "//" => a.div_floor(&b),
        "%" => a.mod_floor(&b),
        "&" => a & b,
        "|" => a | b,
        "^" => a ^ b,
        "<<" | ">>" => {
            if b.is_negative() {
                return Err(fail("ValueError: negative shift count"));
            }
            if op == ">>" && b >= BigInt::from(a.bits()) {
                return Ok(Value::Int(if a.is_negative() { -1 } else { 0 }));
            }
            if a.is_zero() {
                return Ok(Value::Int(0));
            }
            let shift = b.to_usize().ok_or(Error::IntegerOverflow)?;
            if op == "<<" { a << shift } else { a >> shift }
        }
        "**" => {
            if b.is_zero() {
                BigInt::one()
            } else if a.is_zero() {
                if b.is_negative() {
                    return Err(fail(
                        "ZeroDivisionError: zero cannot be raised to a negative power",
                    ));
                }
                BigInt::zero()
            } else if a == BigInt::one() {
                BigInt::one()
            } else if a == -BigInt::one() {
                if b.is_even() {
                    BigInt::one()
                } else {
                    -BigInt::one()
                }
            } else {
                a.pow(b.to_u32().ok_or(Error::IntegerOverflow)?)
            }
        }
        _ => return Err(Error::Unsupported("integer operator")),
    };
    Ok(value(heap, n))
}

// Divide exact integers before rounding; converting both operands to f64 can overflow or
// discard the remainder that decides a halfway result. The quotient has at most 53 bits.
fn true_divide(a: &BigInt, b: &BigInt) -> Result<f64> {
    let negative = a.is_negative() != b.is_negative();
    if a.is_zero() {
        return Ok(if negative { -0.0 } else { 0.0 });
    }
    let (a, b) = (a.abs(), b.abs());
    let mut exponent = i128::from(a.bits()) - i128::from(b.bits());
    if exponent > 1024 {
        return Err(fail(
            "OverflowError: integer division result too large for a float",
        ));
    }
    if exponent < -1075 {
        return Ok(if negative { -0.0 } else { 0.0 });
    }
    if if exponent >= 0 {
        a < (&b << exponent as usize)
    } else {
        (&a << (-exponent) as usize) < b
    } {
        exponent -= 1;
    }
    let unit = (exponent - 52).max(-1074) as i32;
    let (numerator, denominator) = if unit < 0 {
        (a << (-unit) as usize, b)
    } else {
        (a, b << unit as usize)
    };
    let (mut q, r) = numerator.div_rem(&denominator);
    let twice = r << 1usize;
    if twice > denominator || (twice == denominator && q.is_odd()) {
        q += 1;
    }
    let power = if unit >= -1022 {
        f64::from_bits(((unit + 1023) as u64) << 52)
    } else {
        f64::from_bits(1u64 << (unit + 1074))
    };
    let result = q.to_f64().unwrap() * power;
    if !result.is_finite() {
        return Err(fail(
            "OverflowError: integer division result too large for a float",
        ));
    }
    Ok(if negative { -result } else { result })
}

pub(crate) fn compare_float(integer: &BigInt, float: f64) -> Option<Ordering> {
    if float.is_nan() {
        return None;
    }
    if float == f64::INFINITY {
        return Some(Ordering::Less);
    }
    if float == f64::NEG_INFINITY {
        return Some(Ordering::Greater);
    }
    let truncated = BigInt::from_f64(float).unwrap();
    match integer.cmp(&truncated) {
        Ordering::Equal => 0.0_f64.partial_cmp(&float.fract()),
        other => Some(other),
    }
}
pub(crate) fn order(heap: &Heap, a: Value, b: Value) -> Option<Option<Ordering>> {
    if !is_big(heap, a) && !is_big(heap, b) {
        return None;
    }
    if let Value::Float(b) = b {
        return get(heap, a).ok().map(|a| compare_float(&a, b));
    }
    if let Value::Float(a) = a {
        return get(heap, b)
            .ok()
            .map(|b| compare_float(&b, a).map(Ordering::reverse));
    }
    get(heap, a)
        .ok()
        .zip(get(heap, b).ok())
        .map(|(a, b)| Some(a.cmp(&b)))
}
pub(crate) fn key_equal(a: &BigInt, b: &Key) -> bool {
    match b {
        Key::BigInt(b) => a == &b.1,
        Key::Int(b) => a == &BigInt::from(*b),
        Key::Bool(b) => a == &BigInt::from(i64::from(*b)),
        Key::Float(bits) => compare_float(a, f64::from_bits(*bits)) == Some(Ordering::Equal),
        _ => false,
    }
}
pub(crate) fn hash_key<H: Hasher>(n: &BigInt, state: &mut H) {
    if let Some(n) = n.to_i64() {
        2u8.hash(state);
        n.hash(state);
    } else {
        8u8.hash(state);
        n.hash(state);
    }
}
pub(crate) fn python_hash(n: &BigInt) -> i64 {
    let modulus: BigInt = (BigInt::one() << 61usize) - 1;
    let hash = (n % &modulus).to_i64().unwrap();
    if hash == -1 { -2 } else { hash }
}

pub(crate) fn modular_power(heap: &Heap, a: Value, b: Value, modulus: Value) -> Result<Value> {
    let (mut a, mut b, modulus) = (get(heap, a)?, get(heap, b)?, get(heap, modulus)?);
    if modulus.is_zero() {
        return Err(fail("ValueError: pow() 3rd argument cannot be 0"));
    }
    if b.is_negative() {
        a = a
            .modinv(&modulus)
            .ok_or_else(|| fail("ValueError: base is not invertible for the given modulus"))?;
        b = -b;
    }
    Ok(value(heap, a.modpow(&b, &modulus)))
}

pub(crate) fn divmod(heap: &Heap, a: Value, b: Value) -> Result<(Value, Value)> {
    if matches!(a, Value::Float(_)) || matches!(b, Value::Float(_)) {
        let (q, r) = float_divmod(number(heap, a)?, number(heap, b)?)?;
        return Ok((Value::Float(q), Value::Float(r)));
    }
    let (a, b) = (get(heap, a)?, get(heap, b)?);
    if b.is_zero() {
        return Err(fail(
            "ZeroDivisionError: integer division or modulo by zero",
        ));
    }
    let (q, r) = a.div_mod_floor(&b);
    Ok((value(heap, q), value(heap, r)))
}

pub(crate) fn float_divmod(a: f64, b: f64) -> Result<(f64, f64)> {
    if b == 0.0 {
        return Err(fail("ZeroDivisionError: float division by zero"));
    }
    let mut remainder = a % b;
    let mut quotient = (a - remainder) / b;
    if remainder != 0.0 {
        if remainder.is_sign_negative() != b.is_sign_negative() {
            remainder += b;
            quotient -= 1.0;
        }
    } else {
        remainder = 0.0_f64.copysign(b);
    }
    if quotient != 0.0 {
        let floor = quotient.floor();
        quotient = if quotient - floor > 0.5 {
            floor + 1.0
        } else {
            floor
        };
    } else {
        quotient = 0.0_f64.copysign(a / b);
    }
    Ok((quotient, remainder))
}
