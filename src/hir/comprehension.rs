//! Eager comprehensions without escaping lexical scopes share their caller's HIR.
use super::*;
use std::collections::BTreeMap;

impl Function {
    pub(crate) fn inline_comprehension(
        &mut self,
        body: &Function,
        from: BlockId,
        iterator: ValueId,
    ) -> Option<(BlockId, ValueId)> {
        // Escaping callables need a fresh cell per invocation. Frame-sensitive operations and
        // suspended generators also retain their original separate-frame implementation.
        if body.blocks.iter().flat_map(|b| &b.instructions).any(|i| {
            matches!(
                i.op,
                Op::Runtime(
                    CoreOp::Function(_)
                        | CoreOp::Class(_, _)
                        | CoreOp::TryCall(_)
                        | CoreOp::With(_)
                        | CoreOp::Generator(_, _)
                        | CoreOp::ZeroSuper(_)
                        | CoreOp::Namespace,
                    _
                )
            )
        }) {
            return None;
        }
        // Removing an enclosing-local capture can unlock SSA throughout the caller. Without
        // that benefit, retain the smaller separate function to limit cold native compilation.
        if !body.blocks.iter().flat_map(|b| &b.instructions).any(|i| {
            matches!(&i.op, Op::Runtime(CoreOp::LoadName(name), _)
                if self.local_names.contains(name) && !body.local_names.contains(name))
        }) {
            return None;
        }
        // An unbound target must report its Python name from the original comprehension frame.
        // A dominating store lets target reads become values without frame-relative error checks.
        let analysis = body.analyze().ok()?;
        let mut stores = BTreeMap::<&str, Vec<(usize, usize)>>::new();
        for (block, contents) in body.blocks.iter().enumerate() {
            for (position, instruction) in contents.instructions.iter().enumerate() {
                if let Op::Runtime(CoreOp::StoreName(name), _) = &instruction.op {
                    stores.entry(name).or_default().push((block, position));
                }
            }
        }
        for (block, contents) in body.blocks.iter().enumerate() {
            if !analysis.reachable[block] {
                continue;
            }
            for (position, instruction) in contents.instructions.iter().enumerate() {
                if let Op::Runtime(CoreOp::LoadName(name), _) = &instruction.op
                    && name != ".iter"
                    && body.local_names.contains(name)
                    && !stores.get(name.as_str()).is_some_and(|definitions| {
                        definitions.iter().any(|&(defined, at)| {
                            (defined == block && at < position)
                                || (defined != block
                                    && analysis.dominators[block].contains(&defined))
                        })
                    })
                {
                    return None;
                }
            }
        }
        // Promote the private scope before splicing it. Its locals become SSA values confined to
        // these blocks, rather than adding frame slots/phi arguments throughout the caller.
        let mut body = body.clone();
        body.index_locals(false);
        body.promote_locals().ok()?;
        if body
            .blocks
            .iter()
            .flat_map(|b| &b.instructions)
            .any(|i| match &i.op {
                Op::Runtime(CoreOp::LoadLocal(slot), _) => body.local_slots[*slot] != ".iter",
                Op::Runtime(CoreOp::StoreLocal(_) | CoreOp::CheckLocal(_), _) => true,
                _ => false,
            })
        {
            return None;
        }
        let mut values = (0..body.value_count)
            .map(|_| self.value())
            .collect::<Vec<_>>();
        for instruction in body.blocks.iter().flat_map(|b| &b.instructions) {
            if matches!(instruction.op, Op::Runtime(CoreOp::LoadLocal(_), _)) {
                values[instruction.result.0] = iterator;
            }
        }
        let first = self.blocks.len();
        for block in &body.blocks {
            self.blocks.push(Block {
                parameters: block.parameters.iter().map(|v| values[v.0]).collect(),
                instructions: vec![],
                terminator: None,
            });
        }
        let join = self.block(1);
        let edge = |edge: &Edge| {
            Edge::new(
                BlockId(first + edge.block.0),
                edge.arguments.iter().map(|v| values[v.0]).collect(),
            )
        };
        for (index, block) in body.blocks.iter().enumerate() {
            let target = BlockId(first + index);
            for instruction in &block.instructions {
                if matches!(instruction.op, Op::Runtime(CoreOp::LoadLocal(_), _)) {
                    continue;
                }
                let mut instruction = instruction.clone();
                instruction.result = values[instruction.result.0];
                super::promote::rewrite(&mut instruction.op, |v| values[v.0]);
                self.locations[instruction.result.0] =
                    Some((target.0, self.blocks[target.0].instructions.len()));
                self.blocks[target.0].instructions.push(instruction);
            }
            let terminator = match block.terminator.as_ref().expect("finished comprehension") {
                Terminator::Return(value) => {
                    Terminator::Jump(Edge::new(join, vec![values[value.0]]))
                }
                Terminator::Jump(next) => Terminator::Jump(edge(next)),
                Terminator::Branch { condition, yes, no } => Terminator::Branch {
                    condition: values[condition.0],
                    yes: edge(yes),
                    no: edge(no),
                },
            };
            self.terminate(target, terminator);
        }
        self.terminate(from, Terminator::Jump(Edge::new(BlockId(first), vec![])));
        Some((join, self.parameters(join)[0]))
    }
}
