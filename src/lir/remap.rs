//! Pure graph renaming keeps the instruction semantics in ordinary LIR lowering.
use super::{ExitId, Operation, Register, SequenceIndex, StateId};

impl Operation {
    pub(crate) fn remap(
        &self,
        registers: &[Register],
        states: &[StateId],
        exits: &[ExitId],
    ) -> Self {
        let r = |value: Register| registers[value.0];
        let s = |value: StateId| states[value.0];
        let e = |value: ExitId| exits[value.0];
        match self {
            Self::Copy(value) => Self::Copy(r(*value)),
            Self::FloatListView {
                value,
                length,
                failure,
            } => Self::FloatListView {
                value: r(*value),
                length: *length,
                failure: e(*failure),
            },
            Self::FloatListUnpack { value, view } => Self::FloatListUnpack {
                value: r(*value),
                view: r(*view),
            },
            Self::FloatListLoad {
                view,
                index,
                failure,
            } => Self::FloatListLoad {
                view: r(*view),
                index: *index,
                failure: e(*failure),
            },
            Self::FloatListStore {
                view,
                index,
                value,
                failure,
            } => Self::FloatListStore {
                view: r(*view),
                index: *index,
                value: r(*value),
                failure: e(*failure),
            },
            Self::SequenceUnpack {
                value,
                kind,
                count,
                failure,
            } => Self::SequenceUnpack {
                value: r(*value),
                kind: *kind,
                count: *count,
                failure: e(*failure),
            },
            Self::SequenceGet {
                value,
                kind,
                index,
                failure,
            } => Self::SequenceGet {
                value: r(*value),
                kind: *kind,
                index: match index {
                    SequenceIndex::Constant(i) => SequenceIndex::Constant(*i),
                    SequenceIndex::Value(v) => SequenceIndex::Value(r(*v)),
                },
                failure: e(*failure),
            },
            Self::Constant(value) => Self::Constant(value.clone()),
            Self::Truth {
                value,
                state,
                roots,
                effects,
            } => Self::Truth {
                value: r(*value),
                state: s(*state),
                roots: roots.iter().copied().map(r).collect(),
                effects: *effects,
            },
            Self::GuardInt { value, exit } => Self::GuardInt {
                value: r(*value),
                exit: e(*exit),
            },
            Self::GuardFloat { value, exit } => Self::GuardFloat {
                value: r(*value),
                exit: e(*exit),
            },
            Self::UnboxFloat(value) => Self::UnboxFloat(r(*value)),
            Self::IntToFloat(value) => Self::IntToFloat(r(*value)),
            Self::FloatBinary {
                operator,
                left,
                right,
                failure,
            } => Self::FloatBinary {
                operator: *operator,
                left: r(*left),
                right: r(*right),
                failure: e(*failure),
            },
            Self::NumberCompare {
                value,
                integer,
                operator,
                failure,
            } => Self::NumberCompare {
                value: r(*value),
                integer: *integer,
                operator: operator.clone(),
                failure: e(*failure),
            },
            Self::FloatCompare {
                operator,
                left,
                right,
            } => Self::FloatCompare {
                operator: operator.clone(),
                left: r(*left),
                right: r(*right),
            },
            Self::BoxFloat(value) => Self::BoxFloat(r(*value)),
            Self::BoxBool(value) => Self::BoxBool(r(*value)),
            Self::BoolNot(value) => Self::BoolNot(r(*value)),
            Self::UnboxInt(value) => Self::UnboxInt(r(*value)),
            Self::NumericAdd {
                left,
                right,
                guard,
                overflow,
            } => Self::NumericAdd {
                left: r(*left),
                right: r(*right),
                guard: e(*guard),
                overflow: e(*overflow),
            },
            Self::CheckedAdd {
                left,
                right,
                overflow,
            } => Self::CheckedAdd {
                left: r(*left),
                right: r(*right),
                overflow: e(*overflow),
            },
            Self::BoxInt(value) => Self::BoxInt(r(*value)),
            Self::Call {
                helper,
                arguments,
                state,
                roots,
                effects,
            } => Self::Call {
                helper: helper.clone(),
                arguments: arguments.iter().copied().map(r).collect(),
                state: s(*state),
                roots: roots.iter().copied().map(r).collect(),
                effects: *effects,
            },
        }
    }
}
