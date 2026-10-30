use super::{Constant, CoreOp, Function, Op, Terminator};
use crate::Result;

impl Function {
    pub(crate) fn specialize_lists(&mut self) -> Result<()> {
        let mut nested_changed = false;
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
                let revision = body.revision();
                body.specialize_lists()?;
                nested_changed |= revision != body.revision();
            }
        }
        self.validated_backedges()?;
        let mut incoming = vec![vec![]; self.value_count];
        let mut uses = vec![0usize; self.value_count];
        let mut types = vec![0u8; self.value_count];
        for parameter in &self.blocks[0].parameters {
            types[parameter.0] = 2;
        }
        for block in &self.blocks {
            for instruction in &block.instructions {
                types[instruction.result.0] = match instruction.op {
                    Op::List(_) => 1,
                    Op::Runtime(CoreOp::CheckLocal(_), _) => 0,
                    _ => 2,
                };
                for value in instruction.op.inputs() {
                    uses[value.0] += 1;
                }
            }
            let edges = match block.terminator.as_ref().unwrap() {
                Terminator::Return(value) => {
                    uses[value.0] += 1;
                    vec![]
                }
                Terminator::Jump(edge) => vec![edge],
                Terminator::Branch { condition, yes, no } => {
                    uses[condition.0] += 1;
                    vec![yes, no]
                }
            };
            for edge in edges {
                for (&parameter, &argument) in self.blocks[edge.block.0]
                    .parameters
                    .iter()
                    .zip(&edge.arguments)
                {
                    incoming[parameter.0].push(argument);
                    uses[argument.0] += 1;
                }
            }
        }
        // Bottom=0, exact List=1, unknown/other=2. Cycles need a concrete List source;
        // any unknown incoming value permanently prevents a proof.
        loop {
            let mut changed = false;
            for (index, inputs) in incoming.iter().enumerate() {
                if inputs.is_empty() {
                    continue;
                }
                let next = inputs
                    .iter()
                    .fold(types[index], |mask, value| mask | types[value.0]);
                changed |= next != types[index];
                types[index] = next;
            }
            for instruction in self.blocks.iter().flat_map(|block| &block.instructions) {
                if let Op::Runtime(CoreOp::CheckLocal(_), inputs) = &instruction.op {
                    let next = types[instruction.result.0] | types[inputs[0].0];
                    changed |= next != types[instruction.result.0];
                    types[instruction.result.0] = next;
                }
            }
            if !changed {
                break;
            }
        }
        let mut replacements = vec![];
        for (block, body) in self.blocks.iter().enumerate() {
            for (index, instruction) in body.instructions.iter().enumerate() {
                let Op::Runtime(CoreOp::Call(names), inputs) = &instruction.op else {
                    continue;
                };
                if names.len() != 1
                    || names[0].is_some()
                    || inputs.len() != 2
                    || uses[inputs[0].0] != 1
                {
                    continue;
                }
                let Some(lookup) = self.instruction(inputs[0]) else {
                    continue;
                };
                let Op::Runtime(CoreOp::GetAttr(name), receiver) = &lookup.op else {
                    continue;
                };
                if name == "append" && receiver.len() == 1 && types[receiver[0].0] == 1 {
                    replacements.push((block, index, inputs[0], receiver[0], inputs[1]));
                }
            }
        }
        if !replacements.is_empty() || nested_changed {
            for (block, index, lookup, receiver, argument) in replacements {
                self.blocks[block].instructions[index].op =
                    Op::Runtime(CoreOp::ListAppend, vec![receiver, argument]);
                let (block, index) = self.locations[lookup.0].unwrap();
                // Keep IDs and local reconstruction positions stable; this dead value cannot escape.
                self.blocks[block].instructions[index].op = Op::Constant(Constant::None);
            }
            self.invalidate();
            self.validated_backedges()?;
        }
        Ok(())
    }
}
