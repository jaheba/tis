use super::{BlockId, Function, Terminator, ValueId};
use crate::{Error, Result};
use std::collections::BTreeSet;

#[derive(Debug, Clone)]
pub struct Analysis {
    pub reachable: Vec<bool>,
    pub dominators: Vec<BTreeSet<usize>>,
    pub backedges: BTreeSet<(usize, usize)>,
    pub live_in: Vec<Vec<ValueId>>,
    pub live_before: Vec<Vec<Vec<ValueId>>>,
    pub live_terminator: Vec<Vec<ValueId>>,
}
fn edges(term: &Terminator) -> Vec<&super::Edge> {
    match term {
        Terminator::Return(_) => vec![],
        Terminator::Jump(edge) => vec![edge],
        Terminator::Branch { yes, no, .. } => vec![yes, no],
    }
}

struct ControlFlow {
    reachable: Vec<bool>,
    dominators: Vec<Vec<u64>>,
    backedges: BTreeSet<(usize, usize)>,
}

fn control_flow(function: &Function) -> Result<ControlFlow> {
    if !function.local_states.is_empty()
        && (function.local_states.len() != function.blocks.len()
            || function
                .blocks
                .iter()
                .zip(&function.local_states)
                .any(|(block, states)| {
                    states.len() != block.instructions.len() + 1
                        || states.iter().any(|s| s.len() != function.local_slots.len())
                }))
    {
        return Err(Error::InvalidIr("local reconstruction layout"));
    }
    for instruction in function.blocks.iter().flat_map(|b| &b.instructions) {
        if let super::Op::Runtime(operation, inputs) = &instruction.op {
            let local = match operation {
                super::CoreOp::LoadLocal(slot) => Some((*slot, 0)),
                super::CoreOp::StoreLocal(slot) => Some((*slot, 1)),
                super::CoreOp::CheckLocal(slot) => Some((*slot, 1)),
                _ => None,
            };
            if let Some((slot, arity)) = local
                && (slot >= function.local_slots.len() || inputs.len() != arity)
            {
                return Err(Error::InvalidIr("invalid local operation"));
            }
        }
    }
    let count = function.blocks.len();
    if count == 0 {
        return Err(Error::InvalidIr("missing entry block"));
    }
    let mut predecessors = vec![vec![]; count];
    let mut definitions = vec![None; function.value_count];
    for (block, body) in function.blocks.iter().enumerate() {
        for (value, position) in body.parameters.iter().map(|v| (*v, 0)).chain(
            body.instructions
                .iter()
                .enumerate()
                .map(|(i, ins)| (ins.result, i + 1)),
        ) {
            let definition = definitions
                .get_mut(value.0)
                .ok_or(Error::InvalidIr("invalid value definition"))?;
            if definition.replace((block, position)).is_some() {
                return Err(Error::InvalidIr("duplicate value definition"));
            }
        }
        let term = body
            .terminator
            .as_ref()
            .ok_or(Error::InvalidIr("missing terminator"))?;
        for edge in edges(term) {
            let target = function
                .blocks
                .get(edge.block.0)
                .ok_or(Error::InvalidIr("invalid block"))?;
            if edge.arguments.len() != target.parameters.len() {
                return Err(Error::InvalidIr("block argument count"));
            }
            predecessors[edge.block.0].push(block);
        }
    }
    let mut reachable = vec![false; count];
    let mut pending = vec![0];
    while let Some(block) = pending.pop() {
        if std::mem::replace(&mut reachable[block], true) {
            continue;
        }
        pending.extend(
            edges(function.blocks[block].terminator.as_ref().unwrap())
                .iter()
                .map(|edge| edge.block.0),
        );
    }
    // Validation needs membership/intersection only; materialize public sets on demand.
    let mut all = vec![0u64; count.div_ceil(64)];
    for (block, &is_reachable) in reachable.iter().enumerate() {
        if is_reachable {
            all[block / 64] |= 1u64 << (block % 64);
        }
    }
    let mut dominators = vec![all.clone(); count];
    dominators[0].fill(0);
    dominators[0][0] = 1;
    loop {
        let mut changed = false;
        for block in 1..count {
            if !reachable[block] {
                continue;
            }
            let mut next = all.clone();
            for &pred in &predecessors[block] {
                if reachable[pred] {
                    for (word, predecessor) in next.iter_mut().zip(&dominators[pred]) {
                        *word &= predecessor;
                    }
                }
            }
            next[block / 64] |= 1u64 << (block % 64);
            if next != dominators[block] {
                dominators[block] = next;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    let check = |value: ValueId, block: usize, position: usize| -> Result<()> {
        let &(definition, at) = definitions
            .get(value.0)
            .ok_or(Error::InvalidIr("invalid value"))?
            .as_ref()
            .ok_or(Error::InvalidIr("undefined value"))?;
        if reachable[block]
            && (dominators[block][definition / 64] & (1u64 << (definition % 64)) == 0
                || (definition == block && at >= position))
        {
            return Err(Error::InvalidIr("value does not dominate use"));
        }
        Ok(())
    };
    let mut backedges = BTreeSet::new();
    for (block, body) in function.blocks.iter().enumerate() {
        for (index, instruction) in body.instructions.iter().enumerate() {
            if !function.local_states.is_empty() {
                match &instruction.op {
                    super::Op::Runtime(super::CoreOp::StoreLocal(_), _) => {
                        return Err(Error::InvalidIr("store into an SSA local"));
                    }
                    super::Op::Runtime(super::CoreOp::LoadLocal(slot), _)
                        if function.local_state(super::ResumePoint {
                            block: BlockId(block),
                            instruction: index,
                        })[*slot]
                            .is_some() =>
                    {
                        return Err(Error::InvalidIr("load from an SSA local"));
                    }
                    _ => {}
                }
            }
            for input in instruction.op.inputs() {
                check(input, block, index + 1)?;
            }
            for input in function
                .local_state(super::ResumePoint {
                    block: BlockId(block),
                    instruction: index,
                })
                .iter()
                .flatten()
            {
                check(*input, block, index + 1)?;
            }
        }
        for input in function
            .local_state(super::ResumePoint {
                block: BlockId(block),
                instruction: body.instructions.len(),
            })
            .iter()
            .flatten()
        {
            check(*input, block, body.instructions.len() + 1)?;
        }
        let term = body.terminator.as_ref().unwrap();
        match term {
            Terminator::Return(value) => check(*value, block, body.instructions.len() + 1)?,
            Terminator::Branch { condition, .. } => {
                check(*condition, block, body.instructions.len() + 1)?
            }
            _ => {}
        }
        for edge in edges(term) {
            for &arg in &edge.arguments {
                check(arg, block, body.instructions.len() + 1)?;
            }
            if reachable[block]
                && dominators[block][edge.block.0 / 64] & (1u64 << (edge.block.0 % 64)) != 0
            {
                backedges.insert((block, edge.block.0));
            }
        }
    }
    Ok(ControlFlow {
        reachable,
        dominators,
        backedges,
    })
}

pub(super) fn validated_reachable(function: &Function) -> Result<Vec<bool>> {
    Ok(control_flow(function)?.reachable)
}

pub(super) fn validated_backedges(function: &Function) -> Result<BTreeSet<(usize, usize)>> {
    Ok(control_flow(function)?.backedges)
}

pub(super) fn analyze(function: &Function) -> Result<Analysis> {
    let ControlFlow {
        reachable,
        dominators,
        backedges,
    } = control_flow(function)?;
    let count = function.blocks.len();
    let mut live_in = vec![BTreeSet::new(); count];
    // Fixed-point iterations need sets only; snapshot vectors are built after convergence.
    let transfer = |block: usize, live_in: &Vec<BTreeSet<ValueId>>, snapshots: bool| {
        let body = &function.blocks[block];
        let term = body.terminator.as_ref().unwrap();
        let mut live = BTreeSet::new();
        for edge in edges(term) {
            let parameters = &function.blocks[edge.block.0].parameters;
            live.extend(
                live_in[edge.block.0]
                    .iter()
                    .filter(|id| !parameters.contains(id))
                    .copied(),
            );
            live.extend(edge.arguments.iter().copied());
        }
        match term {
            Terminator::Return(value) => {
                live.insert(*value);
            }
            Terminator::Branch { condition, .. } => {
                live.insert(*condition);
            }
            _ => {}
        }
        live.extend(
            function
                .local_state(super::ResumePoint {
                    block: BlockId(block),
                    instruction: body.instructions.len(),
                })
                .iter()
                .flatten()
                .copied(),
        );
        let terminator_live = if snapshots {
            live.iter().copied().collect()
        } else {
            vec![]
        };
        let mut before = if snapshots {
            vec![vec![]; body.instructions.len()]
        } else {
            vec![]
        };
        for (index, instruction) in body.instructions.iter().enumerate().rev() {
            live.remove(&instruction.result);
            live.extend(instruction.op.inputs());
            live.extend(
                function
                    .local_state(super::ResumePoint {
                        block: BlockId(block),
                        instruction: index,
                    })
                    .iter()
                    .flatten()
                    .copied(),
            );
            if snapshots {
                before[index] = live.iter().copied().collect();
            }
        }
        (live, before, terminator_live)
    };
    loop {
        let mut changed = false;
        for block in (0..count).rev() {
            let (next, _, _) = transfer(block, &live_in, false);
            if next != live_in[block] {
                live_in[block] = next;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    let (live_before, live_terminator): (Vec<_>, Vec<_>) = (0..count)
        .map(|block| {
            let (_, before, terminator) = transfer(block, &live_in, true);
            (before, terminator)
        })
        .unzip();
    Ok(Analysis {
        reachable,
        dominators: dominators
            .into_iter()
            .map(|words| {
                (0..count)
                    .filter(|&block| words[block / 64] & (1u64 << (block % 64)) != 0)
                    .collect()
            })
            .collect(),
        backedges,
        live_in: live_in
            .into_iter()
            .map(|s| s.into_iter().collect())
            .collect(),
        live_terminator,
        live_before,
    })
}

impl Analysis {
    pub fn is_backedge(&self, from: BlockId, to: BlockId) -> bool {
        self.backedges.contains(&(from.0, to.0))
    }
}
