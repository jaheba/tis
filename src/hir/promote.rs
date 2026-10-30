//! Mutable uncaptured locals become SSA values; reconstruction maps retain Python bindings.
use super::*;
use crate::Error;
use std::collections::BTreeSet;

fn edges(term: &mut Terminator) -> Vec<&mut Edge> {
    match term {
        Terminator::Return(_) => vec![],
        Terminator::Jump(edge) => vec![edge],
        Terminator::Branch { yes, no, .. } => vec![yes, no],
    }
}
fn resolve(mut value: ValueId, aliases: &[Option<ValueId>]) -> ValueId {
    while let Some(next) = aliases[value.0] {
        value = next;
    }
    value
}
fn rewrite(op: &mut Op, map: impl Fn(ValueId) -> ValueId) {
    let remap = |v: &mut ValueId| *v = map(*v);
    match op {
        Op::Constant(_) => {}
        Op::Add(a, b) | Op::Subtract(a, b) | Op::Less(a, b) | Op::GetItem(a, b) => {
            remap(a);
            remap(b);
        }
        Op::List(values) | Op::Tuple(values) | Op::Runtime(_, values) => {
            for v in values {
                remap(v);
            }
        }
        Op::Dict(values) => {
            for (k, v) in values {
                remap(k);
                remap(v);
            }
        }
        Op::SetItem { object, key, value } => {
            remap(object);
            remap(key);
            remap(value);
        }
        Op::Len(v) => remap(v),
    }
}

impl Function {
    pub(crate) fn promote_locals(&mut self) -> Result<()> {
        for instruction in self
            .blocks
            .iter_mut()
            .flat_map(|block| &mut block.instructions)
        {
            if let Op::Runtime(
                CoreOp::Function(body)
                | CoreOp::TryCall(body)
                | CoreOp::With(body)
                | CoreOp::Class(_, body),
                _,
            ) = &mut instruction.op
            {
                body.promote_locals()?;
            }
        }
        if self.local_slots.is_empty() || !self.local_states.is_empty() {
            return Ok(());
        }
        let reachable = super::analysis::validated_reachable(self)?;
        let n = self.local_slots.len();
        let mut original = self.blocks.clone();
        let mut predecessors = vec![vec![]; original.len()];
        for (from, block) in original.iter_mut().enumerate() {
            for edge in edges(block.terminator.as_mut().unwrap()) {
                if reachable[from] {
                    predecessors[edge.block.0].push(from);
                }
            }
        }
        // Frontend functions have a distinct prologue. Hand-built entry backedges keep slot semantics.
        if !predecessors[0].is_empty() {
            return Ok(());
        }
        let parameters = self
            .parameter_names
            .iter()
            .chain(&self.keyword_only)
            .chain(self.vararg.iter())
            .chain(self.kwarg.iter())
            .collect::<BTreeSet<_>>();
        let initial = self
            .local_slots
            .iter()
            .enumerate()
            .filter_map(|(i, name)| parameters.contains(name).then_some(i))
            .collect::<BTreeSet<_>>();
        let all = (0..n).collect::<BTreeSet<_>>();
        let mut bound = vec![all.clone(); original.len()];
        bound[0] = initial.clone();
        let mut outgoing = bound.clone();
        loop {
            let mut changed = false;
            for (id, block) in original.iter().enumerate() {
                let mut input = if id == 0 {
                    initial.clone()
                } else {
                    all.clone()
                };
                if id != 0 {
                    for &pred in &predecessors[id] {
                        input = input.intersection(&outgoing[pred]).copied().collect();
                    }
                }
                let mut output = input.clone();
                for instruction in &block.instructions {
                    if let Op::Runtime(CoreOp::LoadLocal(slot) | CoreOp::StoreLocal(slot), _) =
                        instruction.op
                    {
                        output.insert(slot);
                    }
                }
                changed |= bound[id] != input || outgoing[id] != output;
                bound[id] = input;
                outgoing[id] = output;
            }
            if !changed {
                break;
            }
        }

        let mut entries = vec![vec![None; n]; original.len()];
        for id in 1..original.len() {
            for local in &mut entries[id] {
                let value = self.value();
                original[id].parameters.push(value);
                *local = Some(value);
            }
        }
        let unbound = self.value();
        let none = self.value();
        let mut prefix = vec![
            Instruction {
                result: unbound,
                op: Op::Constant(Constant::Unbound),
                feedback: None,
            },
            Instruction {
                result: none,
                op: Op::Constant(Constant::None),
                feedback: None,
            },
        ];
        for &slot in &initial {
            let value = self.value();
            prefix.push(Instruction {
                result: value,
                op: Op::Runtime(CoreOp::LoadLocal(slot), vec![]),
                feedback: None,
            });
        }
        let mut aliases = vec![None; self.value_count];
        let mut states = vec![vec![]; original.len()];
        let mut output = Vec::new();
        for (id, mut block) in original.into_iter().enumerate() {
            let mut locals = entries[id].clone();
            let mut assigned = bound[id].clone();
            let mut instructions = vec![];
            if id == 0 {
                for instruction in prefix.drain(..) {
                    states[id].push(locals.clone());
                    if instruction.result == unbound {
                        for (slot, local) in locals.iter_mut().enumerate() {
                            if !initial.contains(&slot) {
                                *local = Some(unbound);
                            }
                        }
                    } else if let Op::Runtime(CoreOp::LoadLocal(slot), _) = instruction.op {
                        locals[slot] = Some(instruction.result);
                    }
                    instructions.push(instruction);
                }
            }
            for mut instruction in block.instructions {
                match &instruction.op {
                    Op::Runtime(CoreOp::StoreLocal(slot), values) => {
                        locals[*slot] = Some(values[0]);
                        assigned.insert(*slot);
                        aliases[instruction.result.0] = Some(none);
                        continue;
                    }
                    Op::Runtime(CoreOp::LoadLocal(slot), _) => {
                        let value = locals[*slot].ok_or(Error::InvalidIr("missing SSA local"))?;
                        if assigned.contains(slot) {
                            aliases[instruction.result.0] = Some(value);
                            continue;
                        }
                        assigned.insert(*slot);
                        instruction.op = Op::Runtime(CoreOp::CheckLocal(*slot), vec![value]);
                    }
                    _ => {}
                }
                states[id].push(locals.clone());
                instructions.push(instruction);
            }
            states[id].push(locals.clone());
            for edge in edges(block.terminator.as_mut().unwrap()) {
                edge.arguments.extend(locals.iter().map(|v| v.unwrap()));
            }
            block.instructions = instructions;
            output.push(block);
        }
        // Collapse unchanged loop-carried bindings and single-predecessor joins.
        loop {
            let mut changed = false;
            for id in 1..output.len() {
                for (position, &parameter) in output[id].parameters.iter().enumerate() {
                    if aliases[parameter.0].is_some() {
                        continue;
                    }
                    let mut inputs = BTreeSet::new();
                    for source in &output {
                        let targets: Vec<&Edge> = match source.terminator.as_ref().unwrap() {
                            Terminator::Return(_) => vec![],
                            Terminator::Jump(edge) => vec![edge],
                            Terminator::Branch { yes, no, .. } => vec![yes, no],
                        };
                        for edge in targets.into_iter().filter(|e| e.block.0 == id) {
                            let input = resolve(edge.arguments[position], &aliases);
                            if input != parameter {
                                inputs.insert(input);
                            }
                        }
                    }
                    if inputs.len() == 1 {
                        aliases[parameter.0] = inputs.first().copied();
                        changed = true;
                    }
                }
            }
            if !changed {
                break;
            }
        }
        let retained = output
            .iter()
            .map(|block| {
                block
                    .parameters
                    .iter()
                    .map(|v| aliases[v.0].is_none())
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();
        for block in &mut output {
            block.parameters.retain(|v| aliases[v.0].is_none());
            for instruction in &mut block.instructions {
                rewrite(&mut instruction.op, |v| resolve(v, &aliases));
            }
            match block.terminator.as_mut().unwrap() {
                Terminator::Return(v) | Terminator::Branch { condition: v, .. } => {
                    *v = resolve(*v, &aliases)
                }
                _ => {}
            }
            for edge in edges(block.terminator.as_mut().unwrap()) {
                for v in &mut edge.arguments {
                    *v = resolve(*v, &aliases);
                }
                let mut index = 0;
                edge.arguments.retain(|_| {
                    let keep = retained[edge.block.0][index];
                    index += 1;
                    keep
                });
            }
        }
        for local in states.iter_mut().flatten().flatten().flatten() {
            *local = resolve(*local, &aliases);
        }
        // Promotion runs before publication: compact erased loads/stores instead of retaining empty VM slots.
        let mut ids = vec![None; self.value_count];
        let mut count = 0;
        for block in &output {
            for value in block
                .parameters
                .iter()
                .copied()
                .chain(block.instructions.iter().map(|i| i.result))
            {
                ids[value.0] = Some(ValueId(count));
                count += 1;
            }
        }
        let compact = |v: ValueId| ids[v.0].expect("SSA value without a definition");
        for block in &mut output {
            for parameter in &mut block.parameters {
                *parameter = compact(*parameter);
            }
            for instruction in &mut block.instructions {
                rewrite(&mut instruction.op, compact);
                instruction.result = compact(instruction.result);
            }
            match block.terminator.as_mut().unwrap() {
                Terminator::Return(v) | Terminator::Branch { condition: v, .. } => *v = compact(*v),
                _ => {}
            }
            for edge in edges(block.terminator.as_mut().unwrap()) {
                for arg in &mut edge.arguments {
                    *arg = compact(*arg);
                }
            }
        }
        for local in states.iter_mut().flatten().flatten().flatten() {
            *local = compact(*local);
        }
        self.blocks = output;
        self.local_states = states;
        self.value_count = count;
        self.locations = vec![None; count];
        for (block, body) in self.blocks.iter().enumerate() {
            for (index, instruction) in body.instructions.iter().enumerate() {
                self.locations[instruction.result.0] = Some((block, index));
            }
        }
        self.invalidate();
        self.validated_backedges()?;
        Ok(())
    }
}
