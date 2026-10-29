//! Fuse only direct tuple returns with a single uninterrupted unpack bundle.
//!
//! No exit or safepoint remains between the removed construction and unpack.
//! Earlier exits resume before construction, so bytecode materializes the tuple.
//! Escaping, named, and inconsistently shaped returns keep ordinary allocation.
use crate::{
    hir::{self, CoreOp, Op, ValueId},
    lir::{self, Helper, Operation, Register, SequenceIndex},
};
use std::collections::BTreeMap;

pub(super) struct Fusion {
    pub consumers: usize,
    pub fields: Vec<Register>,
    pub returns: BTreeMap<usize, (ValueId, Vec<ValueId>)>,
}

impl Fusion {
    pub fn find(
        caller: &hir::Function,
        call: &lir::Instruction,
        tail: &[lir::Instruction],
        callee: &hir::Function,
    ) -> Option<Self> {
        let body = caller.blocks.get(call.point.block.0)?;
        let index = call.point.instruction + 1;
        let unpack = body.instructions.get(index)?;
        let Op::Runtime(CoreOp::Unpack(count), inputs) = &unpack.op else {
            return None;
        };
        if !(1..=4).contains(count)
            || inputs != &[call.origin]
            || lir::temporary_uses(caller, call.origin) != Some(1)
            || !lir::unpack_bundle(caller, call.point.block.0, index, *count)
        {
            return None;
        }
        let unpack_register = Register(unpack.result.0);
        let first = tail.first()?;
        if first.result != Some(unpack_register)
            || first.origin != unpack.result
            || !match &first.operation {
                Operation::SequenceUnpack {
                    value, count: n, ..
                } => Some(*value) == call.result && n == count,
                Operation::Call {
                    helper: Helper::Python(CoreOp::Unpack(n)),
                    arguments,
                    ..
                } => arguments.as_slice() == [call.result?] && n == count,
                _ => false,
            }
        {
            return None;
        }
        let mut fields = Vec::new();
        for offset in 0..*count {
            let item = tail.get(offset + 1)?;
            let expected = body.instructions[index + 1 + offset].result;
            if item.result != Some(Register(expected.0))
                || item.origin != expected
                || !match &item.operation {
                    Operation::SequenceGet {
                        value,
                        index: SequenceIndex::Constant(i),
                        ..
                    } => *value == unpack_register && *i == offset as i64,
                    Operation::Call {
                        helper: Helper::Python(CoreOp::UnpackItem(i)),
                        arguments,
                        ..
                    } => *i == offset && arguments == &[unpack_register],
                    _ => false,
                }
            {
                return None;
            }
            fields.push(Register(expected.0));
        }
        let mut returns = BTreeMap::new();
        for (block, body) in callee.blocks.iter().enumerate() {
            let Some(hir::Terminator::Return(value)) = &body.terminator else {
                continue;
            };
            let tuple = body.instructions.last()?;
            let Op::Tuple(elements) = &tuple.op else {
                return None;
            };
            if tuple.result != *value
                || elements.len() != *count
                || lir::temporary_uses(callee, *value) != Some(1)
            {
                return None;
            }
            returns.insert(block, (*value, elements.clone()));
        }
        (!returns.is_empty()).then_some(Self {
            consumers: 1 + count,
            fields,
            returns,
        })
    }

    pub fn matches_lowering(&self, lowered: &lir::Function) -> bool {
        self.returns.iter().all(|(&block, (value, fields))| {
            let body = &lowered.blocks[block];
            if !matches!(&body.terminator, lir::Terminator::Return(r) if r.0 == value.0) {
                return false;
            }
            body.instructions.last().is_some_and(|instruction| {
                instruction.result == Some(Register(value.0))
                    && matches!(&instruction.operation,
                        Operation::Call { helper: Helper::Tuple, arguments, .. }
                        if arguments.iter().map(|r| r.0).eq(fields.iter().map(|v| v.0)))
            })
        })
    }

    pub fn producer(&self, block: usize, instruction: &lir::Instruction) -> bool {
        self.returns
            .get(&block)
            .is_some_and(|(value, _)| instruction.result == Some(Register(value.0)))
    }
}
