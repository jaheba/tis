//! Recognize a call-free Boolean store body; native consumers guard exact runtime storage.
use crate::{
    hir::{BlockId, Constant, CoreOp},
    lir::{self, Helper, Operation, Register, Terminator},
};

#[derive(Debug)]
pub(crate) struct BoolFill {
    pub header: BlockId,
    pub body: BlockId,
    pub iterator: Register,
    pub list: Register,
    pub last: Option<Register>,
    pub constant: Register,
    pub result: Option<Register>,
    pub value: bool,
}

pub(crate) fn find(lir: &lir::Function) -> Vec<BoolFill> {
    if lir.block_origins.iter().any(Option::is_none) {
        return vec![];
    }
    lir.blocks
        .iter()
        .enumerate()
        .filter_map(|(id, header)| {
            let [next, done, truth] = header.instructions.as_slice() else {
                return None;
            };
            let Operation::Call {
                helper: Helper::Python(CoreOp::Next),
                arguments: iterator,
                ..
            } = &next.operation
            else {
                return None;
            };
            let [iterator] = iterator.as_slice() else {
                return None;
            };
            let value = next.result?;
            let Operation::Call {
                helper: Helper::Python(CoreOp::IsDone),
                arguments: input,
                ..
            } = &done.operation
            else {
                return None;
            };
            if input.as_slice() != [value] {
                return None;
            }
            let done_value = done.result?;
            let Operation::Truth { value: input, .. } = &truth.operation else {
                return None;
            };
            if *input != done_value {
                return None;
            }
            let Terminator::Branch { condition, yes, no } = &header.terminator else {
                return None;
            };
            if Some(*condition) != truth.result || yes.block == no.block || no.block.0 == id {
                return None;
            }
            let body = lir.blocks.get(no.block.0)?;
            let [constant, store] = body.instructions.as_slice() else {
                return None;
            };
            let Operation::Constant(Constant::Bool(fill)) = &constant.operation else {
                return None;
            };
            let constant_value = constant.result?;
            let Operation::Call {
                helper: Helper::SetItem,
                arguments,
                ..
            } = &store.operation
            else {
                return None;
            };
            let [list, index, stored] = arguments.as_slice() else {
                return None;
            };
            if *index != value || *stored != constant_value {
                return None;
            }
            let Terminator::Jump(edge) = &body.terminator else {
                return None;
            };
            if edge.block.0 != id {
                return None;
            }
            let last = match (header.parameters.as_slice(), edge.arguments.as_slice()) {
                ([], []) => None,
                ([last], [source]) if *source == value => Some(*last),
                _ => return None,
            };
            // The destination and iterator must remain invariant through the body and header.
            let mut defined = header
                .parameters
                .iter()
                .copied()
                .chain(header.instructions.iter().filter_map(|i| i.result))
                .chain(body.parameters.iter().copied())
                .chain(body.instructions.iter().filter_map(|i| i.result));
            if defined.any(|r| r == *list || r == *iterator) || !body.parameters.is_empty() {
                return None;
            }
            if !yes.arguments.is_empty() || !no.arguments.is_empty() {
                return None;
            }
            Some(BoolFill {
                header: BlockId(id),
                body: no.block,
                iterator: *iterator,
                list: *list,
                last,
                constant: constant_value,
                result: store.result,
                value: *fill,
            })
        })
        .collect()
}
