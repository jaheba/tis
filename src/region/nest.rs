//! A closed outer loop can reuse private iterators whose only executable uses are internal next operations.
use super::*;
use crate::lir::{Helper, StateId};
use std::collections::BTreeSet;
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrivateIterator {
    pub result: Register,
    pub source: Register,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoopNest {
    pub header: BlockId,
    pub tail: BlockId,
    pub iterator: Register,
    pub entry: lir::ExitId,
    pub blocks: Vec<BlockId>,
    pub children: Vec<usize>,
    pub imports: Vec<Register>,
    pub private_iterators: Vec<PrivateIterator>,
}
impl LoopNest {
    pub fn edges(&self, lir: &lir::Function) -> Vec<(BlockId, BlockId)> {
        std::iter::once((self.tail, self.header))
            .chain(
                self.children
                    .iter()
                    .map(|&i| (lir.regions[i].body, lir.regions[i].header)),
            )
            .collect()
    }
    pub(crate) fn discover(lir: &lir::Function) -> Option<(Self, StateId)> {
        let mut best = None;
        for (h, header) in lir.blocks.iter().enumerate() {
            if lir.regions.iter().any(|r| r.header.0 == h || r.body.0 == h) {
                continue;
            }
            let [next, done, truth] = header.instructions.as_slice() else {
                continue;
            };
            let Operation::Call {
                helper: Helper::Python(CoreOp::Next),
                arguments,
                state,
                ..
            } = &next.operation
            else {
                continue;
            };
            let [iterator] = arguments.as_slice() else {
                continue;
            };
            let (Some(n), Some(d), Some(t)) = (next.result, done.result, truth.result) else {
                continue;
            };
            if !matches!(&done.operation,Operation::Call {helper:Helper::Python(CoreOp::IsDone),arguments,..} if arguments.as_slice()==[n])
                || !matches!(truth.operation,Operation::Truth {value,..} if value==d)
            {
                continue;
            }
            let Terminator::Branch { condition, yes, no } = &header.terminator else {
                continue;
            };
            if *condition != t {
                continue;
            }
            let mut blocks = BTreeSet::from([BlockId(h)]);
            let mut queue = vec![no.block];
            let mut valid = true;
            while let Some(b) = queue.pop() {
                if b == yes.block {
                    continue;
                }
                if !blocks.insert(b) {
                    continue;
                }
                let Some(body) = lir.blocks.get(b.0) else {
                    valid = false;
                    break;
                };
                match &body.terminator {
                    Terminator::Jump(e) => queue.push(e.block),
                    Terminator::Branch { yes, no, .. } => {
                        queue.push(yes.block);
                        queue.push(no.block);
                    }
                    _ => {
                        valid = false;
                        break;
                    }
                }
            }
            if !valid
                || blocks.contains(&yes.block)
                || blocks.contains(&BlockId(0))
                || blocks.len() < 4
            {
                continue;
            }
            let children = lir
                .regions
                .iter()
                .enumerate()
                .filter(|(_, r)| blocks.contains(&r.header) && blocks.contains(&r.body))
                .map(|(i, _)| i)
                .collect::<Vec<_>>();
            if children.is_empty() {
                continue;
            }
            let child_blocks = children
                .iter()
                .flat_map(|&i| [lir.regions[i].header, lir.regions[i].body])
                .collect::<BTreeSet<_>>();
            let defined = blocks
                .iter()
                .flat_map(|b| {
                    lir.blocks[b.0]
                        .parameters
                        .iter()
                        .copied()
                        .chain(lir.blocks[b.0].instructions.iter().filter_map(|i| i.result))
                })
                .collect::<BTreeSet<_>>();
            if defined.contains(iterator) {
                continue;
            }
            let mut private_iterators = vec![];
            let mut tail = None;
            for &b in &blocks {
                for (from, body) in lir.blocks.iter().enumerate() {
                    if blocks.contains(&BlockId(from)) {
                        continue;
                    }
                    let incoming = match &body.terminator {
                        Terminator::Jump(e) => e.block == b,
                        Terminator::Branch { yes, no, .. } => yes.block == b || no.block == b,
                        _ => false,
                    };
                    if b.0 != h && incoming {
                        valid = false;
                    }
                }
                let body = &lir.blocks[b.0];
                if let Terminator::Jump(e) = &body.terminator
                    && e.block.0 == h
                    && tail.replace(b).is_some()
                {
                    valid = false;
                }
                if b.0 == h || child_blocks.contains(&b) {
                    continue;
                }
                // Administrative blocks may allocate only these proven non-escaping builtin iterators.
                if !matches!(body.terminator, Terminator::Jump(_)) {
                    valid = false;
                    break;
                }
                for ins in &body.instructions {
                    let Operation::Call {
                        helper: Helper::Python(CoreOp::Iter),
                        arguments,
                        ..
                    } = &ins.operation
                    else {
                        valid = false;
                        break;
                    };
                    let [source] = arguments.as_slice() else {
                        valid = false;
                        break;
                    };
                    let Some(result) = ins.result else {
                        valid = false;
                        break;
                    };
                    if defined.contains(source)
                        || !children.iter().any(|&i| lir.regions[i].iterator == result)
                    {
                        valid = false;
                        break;
                    }
                    private_iterators.push(PrivateIterator {
                        result,
                        source: *source,
                    });
                }
            }
            let Some(tail) = tail else { continue };
            if !valid || private_iterators.len() != children.len() {
                continue;
            }
            for private in &private_iterators {
                if lir.states.iter().any(|s| {
                    s.local_values
                        .iter()
                        .flatten()
                        .any(|r| *r == private.result)
                }) {
                    valid = false;
                }

                for body in &lir.blocks {
                    for i in &body.instructions {
                        if i.operation.inputs().contains(&private.result)
                            && !matches!(&i.operation,Operation::Call {helper:Helper::Python(CoreOp::Next),arguments,..} if arguments.as_slice()==[private.result])
                        {
                            valid = false;
                        }
                    }
                    let escaped = match &body.terminator {
                        Terminator::Return(r) => *r == private.result,
                        Terminator::Jump(e) => e.arguments.contains(&private.result),
                        Terminator::Branch { yes, no, condition } => {
                            *condition == private.result
                                || yes.arguments.contains(&private.result)
                                || no.arguments.contains(&private.result)
                        }
                    };
                    if escaped {
                        valid = false;
                    }
                }
            }
            let private = private_iterators
                .iter()
                .map(|p| p.result)
                .collect::<BTreeSet<_>>();
            let mut imports = BTreeSet::from([*iterator]);
            imports.extend(private_iterators.iter().map(|p| p.source));
            for &i in &children {
                for &r in &lir.regions[i].imports {
                    if !private.contains(&r) {
                        if defined.contains(&r) {
                            valid = false;
                        }
                        imports.insert(r);
                    }
                }
            }
            if !valid {
                continue;
            }
            let plan = Self {
                header: BlockId(h),
                tail,
                iterator: *iterator,
                entry: lir::ExitId(0),
                blocks: blocks.into_iter().collect(),
                children,
                imports: imports.into_iter().collect(),
                private_iterators,
            };
            if best
                .as_ref()
                .is_none_or(|(old, _): &(Self, StateId)| old.blocks.len() < plan.blocks.len())
            {
                best = Some((plan, *state));
            }
        }
        best
    }
}
