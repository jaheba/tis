//! Remove snapshots belonging solely to eliminated allocation/unpack instructions.
use super::*;

impl Function {
    pub(crate) fn prune_inline_metadata(&mut self) {
        assert!(self.regions.is_empty() && self.nest.is_none());
        let mut exits = BTreeSet::new();
        let mut states = BTreeSet::new();
        for block in &self.blocks {
            for instruction in &block.instructions {
                match &instruction.operation {
                    Operation::GuardInt { exit, .. } | Operation::GuardFloat { exit, .. } => {
                        exits.insert(exit.0);
                    }
                    Operation::FloatListView { failure, .. }
                    | Operation::FloatListLoad { failure, .. }
                    | Operation::FloatListStore { failure, .. }
                    | Operation::SequenceUnpack { failure, .. }
                    | Operation::SequenceGet { failure, .. }
                    | Operation::FloatBinary { failure, .. }
                    | Operation::NumberCompare { failure, .. } => {
                        exits.insert(failure.0);
                    }
                    Operation::NumericAdd {
                        guard, overflow, ..
                    } => {
                        exits.insert(guard.0);
                        exits.insert(overflow.0);
                    }
                    Operation::CheckedAdd { overflow, .. } => {
                        exits.insert(overflow.0);
                    }
                    Operation::Call { state, .. } | Operation::Truth { state, .. } => {
                        states.insert(state.0);
                    }
                    _ => {}
                }
            }
        }
        exits.extend(self.scalar_entries.values().map(|id| id.0));
        states.extend(exits.iter().map(|&id| self.exits[id].state.0));
        states.extend(self.osr_entries.iter().map(|entry| entry.state.0));
        let mut state_map = vec![StateId(usize::MAX); self.states.len()];
        self.states = states
            .into_iter()
            .enumerate()
            .map(|(new, old)| {
                state_map[old] = StateId(new);
                self.states[old].clone()
            })
            .collect();
        let mut exit_map = vec![ExitId(usize::MAX); self.exits.len()];
        self.exits = exits
            .into_iter()
            .enumerate()
            .map(|(new, old)| {
                exit_map[old] = ExitId(new);
                let mut exit = self.exits[old].clone();
                exit.state = state_map[exit.state.0];
                exit
            })
            .collect();
        let registers = (0..self.registers.len()).map(Register).collect::<Vec<_>>();
        for block in &mut self.blocks {
            for instruction in &mut block.instructions {
                instruction.operation = instruction
                    .operation
                    .remap(&registers, &state_map, &exit_map);
            }
        }
        for exit in self.scalar_entries.values_mut() {
            *exit = exit_map[exit.0];
        }
        for entry in &mut self.osr_entries {
            entry.state = state_map[entry.state.0];
        }
    }
}
