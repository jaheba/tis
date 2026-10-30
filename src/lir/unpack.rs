//! Reuse adjacent checked builtin unpackings without changing HIR result identities.
use super::*;

impl Function {
    pub(crate) fn reuse_unpack_bundles(&mut self) {
        if !self.inline_scopes.is_empty() {
            return;
        }
        let mut changed = false;
        for block in &mut self.blocks {
            let mut previous: Option<(Register, SequenceKind, usize, Vec<Register>)> = None;
            let mut index = 0;
            while index < block.instructions.len() {
                let Operation::SequenceUnpack {
                    value, kind, count, ..
                } = block.instructions[index].operation
                else {
                    previous = None;
                    index += 1;
                    continue;
                };
                let end = index + 1 + count;
                let Some(items) = block.instructions.get(index + 1..end) else {
                    break;
                };
                let token = block.instructions[index].result.unwrap();
                if !items.iter().enumerate().all(|(i, item)| {
                    matches!(item.operation,
                    Operation::SequenceGet { value, kind:k, index:SequenceIndex::Constant(j), .. }
                    if value == token && k == kind && j == i as i64)
                }) {
                    previous = None;
                    index += 1;
                    continue;
                }
                let results = items
                    .iter()
                    .map(|item| item.result.unwrap())
                    .collect::<Vec<_>>();
                if let Some((source, previous_kind, previous_count, ref saved)) = previous
                    && source == value
                    && previous_kind == kind
                    && previous_count == count
                {
                    // No intervening instruction can mutate the list or run Python. Keep the first
                    // exact-length/type check; general iterator unpackings never enter this path.
                    block.instructions[index].operation = Operation::Copy(value);
                    for (item, source) in block.instructions[index + 1..end].iter_mut().zip(saved) {
                        item.operation = Operation::Copy(*source);
                    }
                    changed = true;
                } else {
                    previous = Some((value, kind, count, results));
                }
                index = end;
            }
        }
        if changed {
            self.prune_inline_metadata();
        }
    }
}
