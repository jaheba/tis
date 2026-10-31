//! Numeric plans use exact operand kinds; misses execute the original operation.
use crate::{
    Value,
    hir::{CoreOp, Op},
    profile::{SiteProfile, ValueKind},
    scalar::{FloatCondition, FloatOperator},
};

/// Selects guarded register execution from completed staged feedback windows.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum NumericMode {
    Disabled,
    #[default]
    Adaptive,
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum Operator {
    Arithmetic(FloatOperator),
    Compare(FloatCondition),
}
impl Operator {
    pub fn from_hir(op: &Op) -> Option<Self> {
        Some(match op {
            Op::Add(..) => Self::Arithmetic(FloatOperator::Add),
            Op::Subtract(..) => Self::Arithmetic(FloatOperator::Subtract),
            Op::Less(..) => Self::Compare(FloatCondition::Lt),
            // Binary + and - have dedicated HIR operations; the generic Binary helper rejects them.
            Op::Runtime(CoreOp::Binary(op), _) if matches!(op.as_str(), "+" | "-") => return None,
            Op::Runtime(CoreOp::Binary(op) | CoreOp::Inplace(op) | CoreOp::InplaceItem(op), _) => {
                Self::Arithmetic(match op.as_str() {
                    "+" => FloatOperator::Add,
                    "-" => FloatOperator::Subtract,
                    "*" => FloatOperator::Multiply,
                    "/" => FloatOperator::Divide,
                    "**" => FloatOperator::Power,
                    _ => return None,
                })
            }
            Op::Runtime(CoreOp::Compare(op), _) => Self::Compare(FloatCondition::from_symbol(op)?),
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct Plan {
    pub operator: Operator,
    kinds: [ValueKind; 2],
}
pub(crate) enum Outcome {
    Value(Value),
    TypeMiss,
    ArithmeticMiss,
}
impl Plan {
    pub fn from_profile(operator: Operator, profile: &SiteProfile, offset: usize) -> Option<Self> {
        if profile.executions < crate::hir::AddFeedback::WARMUP as u64
            || profile.operands.len() != offset + 2
        {
            return None;
        }
        let kinds = [
            profile.operands[offset].monomorphic()?,
            profile.operands[offset + 1].monomorphic()?,
        ];
        if !kinds
            .iter()
            .all(|kind| matches!(kind, ValueKind::Int | ValueKind::Float))
        {
            return None;
        }
        // Mixed integer/float comparisons need the generic runtime's exact large-integer ordering.
        if kinds[0] != kinds[1] && matches!(operator, Operator::Compare(_)) {
            return None;
        }
        // Integer power must retain checked integer arithmetic rather than round through f64.
        if kinds == [ValueKind::Int; 2]
            && matches!(operator, Operator::Arithmetic(FloatOperator::Power))
        {
            return None;
        }
        Some(Self { operator, kinds })
    }

    #[inline]
    pub fn evaluate(self, left: Value, right: Value) -> Outcome {
        let matches = |value, kind| {
            matches!(
                (value, kind),
                (Value::Int(_), ValueKind::Int) | (Value::Float(_), ValueKind::Float)
            )
        };
        if !matches(left, self.kinds[0]) || !matches(right, self.kinds[1]) {
            return Outcome::TypeMiss;
        }
        if let (Value::Int(a), Value::Int(b)) = (left, right) {
            let integer = match self.operator {
                Operator::Arithmetic(FloatOperator::Add) => Some(a.checked_add(b)),
                Operator::Arithmetic(FloatOperator::Subtract) => Some(a.checked_sub(b)),
                Operator::Arithmetic(FloatOperator::Multiply) => Some(a.checked_mul(b)),
                Operator::Compare(condition) => {
                    return Outcome::Value(Value::Bool(condition.evaluate_int(a, b)));
                }
                _ => None,
            };
            if let Some(value) = integer {
                return value
                    .map(Value::Int)
                    .map(Outcome::Value)
                    .unwrap_or(Outcome::ArithmeticMiss);
            }
        }
        let number = |value| match value {
            Value::Float(value) => value,
            Value::Int(value) => value as f64,
            _ => unreachable!(),
        };
        let (left, right) = (number(left), number(right));
        match self.operator {
            Operator::Arithmetic(operator) => operator
                .evaluate(left, right)
                .map(Value::Float)
                .map(Outcome::Value)
                .unwrap_or(Outcome::ArithmeticMiss),
            Operator::Compare(condition) => {
                Outcome::Value(Value::Bool(condition.evaluate(left, right)))
            }
        }
    }
}
