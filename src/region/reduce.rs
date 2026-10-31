//! Conditional integer reductions over an exact Boolean predicate are guarded by native consumers.
use crate::{
    hir::{BlockId, Constant, CoreOp},
    lir::{self, Helper, Operation, Register, Terminator},
};
use std::collections::{BTreeMap, BTreeSet};
#[derive(Debug, Clone, PartialEq, Eq)]
enum Expr {
    Header(usize),
    Index,
    Constant(i64),
    Add(Box<Self>, Box<Self>),
}
#[derive(Debug)]
pub(crate) struct BoolReduce {
    pub header: BlockId,
    pub backedge: BlockId,
    pub iterator: Register,
    pub list: Register,
    pub outputs: Vec<(Register, u8)>,
    pub true_blocks: usize,
    pub false_blocks: usize,
}
fn value(map: &BTreeMap<Register, Expr>, r: Register, lir: &lir::Function) -> Option<Expr> {
    map.get(&r).cloned().or_else(|| {
        lir.blocks
            .iter()
            .flat_map(|b| &b.instructions)
            .find_map(|i| {
                if i.result == Some(r) {
                    if let Operation::Constant(Constant::Int(v)) = i.operation {
                        Some(Expr::Constant(v))
                    } else {
                        None
                    }
                } else {
                    None
                }
            })
    })
}
struct Trace {
    values: Vec<Expr>,
    blocks: BTreeSet<usize>,
    definitions: BTreeSet<Register>,
    backedge: usize,
    additions: usize,
}
fn trace(
    lir: &lir::Function,
    start: &lir::Edge,
    header: usize,
    mut map: BTreeMap<Register, Expr>,
) -> Option<Trace> {
    let mut edge = start.clone();
    let mut seen = BTreeSet::new();
    let mut definitions = BTreeSet::new();
    let mut additions = 0;
    loop {
        if edge.block.0 == header {
            return None;
        }
        if !seen.insert(edge.block.0)
            || seen.len() > 8
            || lir.osr_entries.iter().any(|e| e.header == edge.block)
        {
            return None;
        }
        let block = lir.blocks.get(edge.block.0)?;
        let incoming = edge
            .arguments
            .iter()
            .map(|r| value(&map, *r, lir))
            .collect::<Option<Vec<_>>>()?;
        if block.parameters.len() != incoming.len() {
            return None;
        }
        for (&param, expr) in block.parameters.iter().zip(incoming) {
            map.insert(param, expr);
            definitions.insert(param);
        }
        for i in &block.instructions {
            let result = i.result?;
            let expr = match &i.operation {
                Operation::Constant(Constant::Int(v)) => Expr::Constant(*v),
                Operation::NumericAdd { left, right, .. } => Expr::Add(
                    Box::new(value(&map, *left, lir)?),
                    Box::new(value(&map, *right, lir)?),
                ),
                Operation::Call {
                    helper, arguments, ..
                } if matches!(helper, Helper::Add)
                    || matches!(helper,Helper::Python(CoreOp::Inplace(op)) if op=="+") =>
                {
                    let [a, b] = arguments.as_slice() else {
                        return None;
                    };
                    Expr::Add(
                        Box::new(value(&map, *a, lir)?),
                        Box::new(value(&map, *b, lir)?),
                    )
                }
                _ => return None,
            };
            additions += usize::from(matches!(&expr, Expr::Add(..)));
            map.insert(result, expr);
            definitions.insert(result);
        }
        let Terminator::Jump(next) = &block.terminator else {
            return None;
        };
        if next.block.0 == header {
            let values = next
                .arguments
                .iter()
                .map(|r| value(&map, *r, lir))
                .collect::<Option<Vec<_>>>()?;
            return Some(Trace {
                values,
                blocks: seen,
                backedge: edge.block.0,
                definitions,
                additions,
            });
        }
        edge = next.clone();
    }
}
pub(crate) fn find(lir: &lir::Function) -> Vec<BoolReduce> {
    if !lir.inline_scopes.is_empty() || lir.block_origins.iter().any(Option::is_none) {
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
                arguments,
                ..
            } = &next.operation
            else {
                return None;
            };
            let [iterator] = arguments.as_slice() else {
                return None;
            };
            let index = next.result?;
            let Operation::Call {
                helper: Helper::Python(CoreOp::IsDone),
                arguments,
                ..
            } = &done.operation
            else {
                return None;
            };
            if arguments.as_slice() != [index] {
                return None;
            }
            let Operation::Truth { value: input, .. } = truth.operation else {
                return None;
            };
            if done.result != Some(input) {
                return None;
            }
            let Terminator::Branch { condition, yes, no } = &header.terminator else {
                return None;
            };
            if truth.result != Some(*condition)
                || yes.block == no.block
                || !yes.arguments.is_empty()
                || !no.arguments.is_empty()
            {
                return None;
            }
            let predicate_id = no.block.0;
            let predicate = lir.blocks.get(predicate_id)?;
            if !predicate.parameters.is_empty() {
                return None;
            }
            let [get, test] = predicate.instructions.as_slice() else {
                return None;
            };
            let (list, key) = match &get.operation {
                Operation::Call {
                    helper: Helper::GetItem,
                    arguments,
                    ..
                } if arguments.len() == 2 => (&arguments[0], &arguments[1]),
                Operation::SequenceGet {
                    value,
                    kind: lir::SequenceKind::List,
                    index: lir::SequenceIndex::Value(key),
                    ..
                } => (value, key),
                _ => return None,
            };
            if *key != index {
                return None;
            }
            let Operation::Truth { value: input, .. } = test.operation else {
                return None;
            };
            if get.result != Some(input) {
                return None;
            }
            let Terminator::Branch { condition, yes, no } = &predicate.terminator else {
                return None;
            };
            if test.result != Some(*condition) {
                return None;
            }
            let mut map = BTreeMap::new();
            for (n, &param) in header.parameters.iter().enumerate() {
                map.insert(param, Expr::Header(n));
            }
            map.insert(index, Expr::Index);
            let on = trace(lir, yes, id, map.clone())?;
            let off = trace(lir, no, id, map)?;
            let (a, b) = (&on.values, &off.values);
            let (ta, tb) = (on.blocks.len(), off.blocks.len());
            let (from, other) = (on.backedge, off.backedge);
            if from != other
                || a.len() != header.parameters.len()
                || b.len() != a.len()
                || a.len() > 4
            {
                return None;
            }
            let mut defined = on.definitions;
            defined.extend(off.definitions);
            defined.extend(header.parameters.iter().copied());
            defined.extend(header.instructions.iter().filter_map(|i| i.result));
            defined.extend(predicate.instructions.iter().filter_map(|i| i.result));
            if defined.contains(list) || defined.contains(iterator) {
                return None;
            }
            let mut blocks = on.blocks;
            blocks.extend(off.blocks);
            blocks.insert(id);
            blocks.insert(predicate_id);
            let internal = defined
                .iter()
                .copied()
                .filter(|r| !header.parameters.contains(r))
                .collect::<BTreeSet<_>>();
            // Only header parameters may carry state out of the skipped region.
            for (block_id, block) in lir
                .blocks
                .iter()
                .enumerate()
                .filter(|(n, _)| !blocks.contains(n))
            {
                let _ = block_id;
                if block
                    .instructions
                    .iter()
                    .any(|i| i.operation.inputs().iter().any(|r| internal.contains(r)))
                {
                    return None;
                }
                let inputs = match &block.terminator {
                    Terminator::Return(r) => vec![*r],
                    Terminator::Jump(e) => e.arguments.clone(),
                    Terminator::Branch { condition, yes, no } => std::iter::once(*condition)
                        .chain(yes.arguments.iter().copied())
                        .chain(no.arguments.iter().copied())
                        .collect(),
                };
                if inputs.iter().any(|r| internal.contains(r)) {
                    return None;
                }
            }
            let Operation::Call { state, .. } = next.operation else {
                return None;
            };
            let entry = &lir.states[state.0];
            if entry
                .values
                .iter()
                .map(|(_, r)| r)
                .chain(entry.local_values.iter().flatten())
                .any(|r| internal.contains(r))
            {
                return None;
            }
            let origins = blocks
                .iter()
                .filter_map(|&b| lir.block_origins[b])
                .collect::<BTreeSet<_>>();
            for state in &lir.states {
                if !origins.contains(&state.resume.block)
                    && state
                        .values
                        .iter()
                        .map(|(_, r)| r)
                        .chain(state.local_values.iter().flatten())
                        .any(|r| internal.contains(r))
                {
                    return None;
                }
            }

            let mut outputs = vec![];
            for (n, (&target, (on, off))) in
                header.parameters.iter().zip(a.iter().zip(b)).enumerate()
            {
                let self_value = Expr::Header(n);
                let kind = if *on == Expr::Index && *off == Expr::Index {
                    1
                } else if *off == self_value && *on == Expr::Index {
                    2
                } else if *off == self_value
                    && *on == Expr::Add(Box::new(self_value.clone()), Box::new(Expr::Constant(1)))
                {
                    3
                } else if *off == self_value
                    && *on == Expr::Add(Box::new(self_value.clone()), Box::new(Expr::Index))
                {
                    4
                } else if *on == self_value && *off == self_value {
                    0
                } else {
                    return None;
                };
                outputs.push((target, kind));
            }
            if !outputs.iter().any(|(_, kind)| *kind >= 2) {
                return None;
            }
            // Every arithmetic evaluation must be represented by one guarded recurrence.
            // Discarded additions can still raise or invoke Python protocols.
            if off.additions != 0 || on.additions != outputs.iter().filter(|(_, k)| *k >= 3).count()
            {
                return None;
            }
            Some(BoolReduce {
                header: BlockId(id),
                backedge: BlockId(from),
                iterator: *iterator,
                list: *list,
                outputs,
                true_blocks: ta + 2,
                false_blocks: tb + 2,
            })
        })
        .collect()
}
