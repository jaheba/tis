//! Recognize a call-free Boolean store body; native consumers guard exact runtime storage.
use crate::{
    hir::{BlockId, Constant, CoreOp},
    lir::{self, Helper, Operation, Register, Terminator},
};

#[derive(Debug, Clone, Copy)]
pub(crate) struct Counter {
    pub limit: Register,
    pub limit_add: i64,
    pub step: Register,
    pub update: Register,
    pub inclusive: bool,
    pub descending: bool,
}

#[derive(Debug)]
pub(crate) struct BoolFill {
    pub header: BlockId,
    pub body: BlockId,
    pub iterator: Register,
    pub counter: Option<Counter>,
    pub list: Register,
    pub last: Option<Register>,
    pub constant: Register,
    pub result: Option<Register>,
    pub value: bool,
    pub literals: Vec<(Register, Constant)>,
}

pub(crate) fn find(lir: &lir::Function) -> Vec<BoolFill> {
    if lir.block_origins.iter().any(Option::is_none) {
        return vec![];
    }
    let mut plans = lir
        .blocks
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
                counter: None,
                literals: vec![],
                list: *list,
                last,
                constant: constant_value,
                result: store.result,
                value: *fill,
            })
        })
        .collect::<Vec<_>>();
    plans.extend(counter_fills(lir));
    plans
}

// A counter loop shares the fill kernel only when all skipped operations are exact integer
// comparison/addition and builtin stores. Runtime guards reject overflow and invalid bounds
// before any write, so misses retain Python callbacks and partial-write exception behavior.
fn counter_fills(lir: &lir::Function) -> Vec<BoolFill> {
    lir.blocks
        .iter()
        .enumerate()
        .filter_map(|(id, header)| counter_fill(lir, id, header))
        .collect()
}

fn counter_fill(lir: &lir::Function, id: usize, header: &lir::Block) -> Option<BoolFill> {
    let [counter] = header.parameters.as_slice() else {
        return None;
    };
    let prefix_len = header.instructions.len().checked_sub(2)?;
    let (prefix, tail) = header.instructions.split_at(prefix_len);
    let [compare, truth] = tail else {
        return None;
    };
    let Operation::Call {
        helper, arguments, ..
    } = &compare.operation
    else {
        return None;
    };
    let operator = match helper {
        Helper::Python(CoreOp::Compare(op)) => op.as_str(),
        Helper::Less => "<",
        _ => return None,
    };
    let [left, right] = arguments.as_slice() else {
        return None;
    };
    let (limit, operator) = if left == counter {
        (*right, operator)
    } else if right == counter {
        (
            *left,
            match operator {
                "<" => ">",
                "<=" => ">=",
                ">" => "<",
                ">=" => "<=",
                _ => return None,
            },
        )
    } else {
        return None;
    };
    let (inclusive, descending) = match operator {
        "<" => (false, false),
        "<=" => (true, false),
        ">" => (false, true),
        ">=" => (true, true),
        _ => return None,
    };
    let Operation::Truth { value, .. } = &truth.operation else {
        return None;
    };
    if Some(*value) != compare.result {
        return None;
    }
    let Terminator::Branch { condition, yes, no } = &header.terminator else {
        return None;
    };
    if Some(*condition) != truth.result
        || !yes.arguments.is_empty()
        || !no.arguments.is_empty()
        || yes.block == no.block
    {
        return None;
    }
    let body = lir.blocks.get(yes.block.0)?;
    if !body.parameters.is_empty() {
        return None;
    }
    // Immediate literals are effect-free and may be prepared before the guarded kernel.
    // SSA temporaries defined here are not live on entry; local bindings would require phis.
    let literals = body
        .instructions
        .iter()
        .chain(prefix)
        .filter_map(|i| match &i.operation {
            Operation::Constant(c) if c.immediate().is_some() => Some((i.result?, c.clone())),
            _ => None,
        })
        .collect::<Vec<_>>();
    let operations = body
        .instructions
        .iter()
        .filter(|i| !literals.iter().any(|(r, _)| i.result == Some(*r)))
        .collect::<Vec<_>>();
    let [store, update] = operations.as_slice() else {
        return None;
    };
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
    let Some((constant, Constant::Bool(fill))) = literals.iter().find(|(r, _)| r == stored) else {
        return None;
    };
    if index != counter {
        return None;
    }
    let inputs = match &update.operation {
        Operation::NumericAdd { left, right, .. } => [*left, *right],
        Operation::Call {
            helper, arguments, ..
        } if matches!(helper, Helper::Add)
            || matches!(helper,Helper::Python(CoreOp::Inplace(op)|CoreOp::Binary(op)) if op=="+") =>
        {
            let [left, right] = arguments.as_slice() else {
                return None;
            };
            [*left, *right]
        }
        _ => return None,
    };
    let step = if inputs[0] == *counter {
        inputs[1]
    } else if inputs[1] == *counter {
        inputs[0]
    } else {
        return None;
    };
    let update = update.result?;
    let Terminator::Jump(back) = &body.terminator else {
        return None;
    };
    if back.block.0 != id || back.arguments.as_slice() != [update] {
        return None;
    }
    let expressions = prefix
        .iter()
        .filter(|i| !literals.iter().any(|(r, _)| i.result == Some(*r)))
        .collect::<Vec<_>>();
    let (limit, limit_add) = if expressions.is_empty() {
        (limit, 0)
    } else {
        let inputs = affine_add_inputs(&expressions, limit)?;
        let offset = |r| {
            literals.iter().find_map(|(reg, c)| match c {
                Constant::Int(value) if *reg == r => Some(*value),
                _ => None,
            })
        };
        if let Some(offset) = offset(inputs[1]) {
            (inputs[0], offset)
        } else {
            let offset = offset(inputs[0])?;
            (inputs[1], offset)
        }
    };
    let defined = header
        .parameters
        .iter()
        .copied()
        .chain(header.instructions.iter().filter_map(|i| i.result))
        .chain(body.instructions.iter().filter_map(|i| i.result))
        .collect::<Vec<_>>();
    if [limit, step, *list]
        .iter()
        .any(|r| defined.contains(r) && !literals.iter().any(|(literal, _)| literal == r))
    {
        return None;
    }
    Some(BoolFill {
        header: BlockId(id),
        body: yes.block,
        iterator: *counter,
        counter: Some(Counter {
            limit,
            limit_add,
            step,
            update,
            inclusive,
            descending,
        }),
        list: *list,
        last: None,
        constant: *constant,
        result: store.result,
        value: *fill,
        literals,
    })
}

fn affine_add_inputs(expressions: &[&lir::Instruction], result: Register) -> Option<[Register; 2]> {
    match expressions {
        [add] if add.result == Some(result) => match &add.operation {
            Operation::NumericAdd { left, right, .. } => Some([*left, *right]),
            Operation::Call {
                helper, arguments, ..
            } if matches!(helper, Helper::Add)
                || matches!(helper,Helper::Python(CoreOp::Binary(op)) if op=="+") =>
            {
                let [left, right] = arguments.as_slice() else {
                    return None;
                };
                Some([*left, *right])
            }
            _ => None,
        },
        // Scalar lowering may already have expanded the same integer addition.
        [left_guard, left_raw, right_guard, right_raw, add, boxed]
            if boxed.result == Some(result) =>
        {
            let Operation::GuardInt { value: left, .. } = left_guard.operation else {
                return None;
            };
            let Operation::UnboxInt(a) = left_raw.operation else {
                return None;
            };
            let Operation::GuardInt { value: right, .. } = right_guard.operation else {
                return None;
            };
            let Operation::UnboxInt(b) = right_raw.operation else {
                return None;
            };
            let Operation::CheckedAdd {
                left: x, right: y, ..
            } = add.operation
            else {
                return None;
            };
            let Operation::BoxInt(raw) = boxed.operation else {
                return None;
            };
            (a == left
                && b == right
                && left_raw.result == Some(x)
                && right_raw.result == Some(y)
                && add.result == Some(raw))
            .then_some([left, right])
        }
        _ => None,
    }
}
