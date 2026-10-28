//! Scalar semantics shared with the offline Rust stencils.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FloatOperator {
    Add,
    Subtract,
    Multiply,
    Divide,
    Power,
}
impl FloatOperator {
    #[inline(always)]
    pub(crate) fn evaluate(self, left: f64, right: f64) -> Option<f64> {
        Some(match self {
            Self::Add => left + right,
            Self::Subtract => left - right,
            Self::Multiply => left * right,
            Self::Divide => {
                if right == 0.0 {
                    return None;
                }
                left / right
            }
            Self::Power => return power_with(left, right, f64::powf),
        })
    }
}

#[inline(always)]
pub(crate) fn power_with(
    left: f64,
    right: f64,
    power: impl FnOnce(f64, f64) -> f64,
) -> Option<f64> {
    if left == 0.0 && right < 0.0 {
        return None;
    }
    let result = power(left, right);
    if result.is_nan() { None } else { Some(result) }
}
#[derive(Clone, Copy)]
pub(crate) enum FloatCondition {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}
impl FloatCondition {
    pub(crate) fn from_symbol(op: &str) -> Option<Self> {
        Some(match op {
            "==" => Self::Eq,
            "!=" => Self::Ne,
            "<" => Self::Lt,
            "<=" => Self::Le,
            ">" => Self::Gt,
            ">=" => Self::Ge,
            _ => return None,
        })
    }
    #[inline(always)]
    pub(crate) fn evaluate(self, left: f64, right: f64) -> bool {
        match self {
            Self::Eq => left == right,
            Self::Ne => left != right,
            Self::Lt => left < right,
            Self::Le => left <= right,
            Self::Gt => left > right,
            Self::Ge => left >= right,
        }
    }
}
