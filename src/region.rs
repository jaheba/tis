//! Allocation-free native loop regions. Heap views are usable only until the next runtime boundary.
pub mod nest;
pub mod schema;
use crate::native_abi::{KIND_MASK, REF, Slot};
use crate::{
    Error, Heap, Object, ObjectId, Result, Value,
    hir::{BlockId, CoreOp},
    lir::{self, Operation, Register, Terminator},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoopRegion {
    pub header: BlockId,
    pub body: BlockId,
    pub iterator: Register,
    pub entry: lir::ExitId,
    pub imports: Vec<Register>,
    pub schema: Option<schema::Schema>,
}
impl LoopRegion {
    pub(crate) fn imports(lir: &lir::Function, header: BlockId, body: BlockId) -> Vec<Register> {
        let instructions = lir.blocks[header.0]
            .instructions
            .iter()
            .chain(&lir.blocks[body.0].instructions)
            .collect::<Vec<_>>();
        let defined = instructions
            .iter()
            .filter_map(|i| i.result)
            .collect::<std::collections::BTreeSet<_>>();
        instructions
            .iter()
            .flat_map(|i| i.operation.inputs())
            .filter(|r| lir.registers.get(r.0) == Some(&lir::Type::Boxed) && !defined.contains(r))
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect()
    }

    pub(crate) fn candidates(
        lir: &lir::Function,
    ) -> Vec<(BlockId, BlockId, Register, lir::StateId)> {
        let mut result = vec![];
        for (header, h) in lir.blocks.iter().enumerate() {
            let [next, done, truth] = h.instructions.as_slice() else {
                continue;
            };
            let (Some(next_result), Some(done_result)) = (next.result, done.result) else {
                continue;
            };
            let Operation::Call {
                helper: lir::Helper::Python(CoreOp::Next),
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
            // Only the invariant iterator prepared at entry may supply mutable field pointers.
            if h.parameters.contains(iterator) {
                continue;
            }

            if !matches!(&done.operation,Operation::Call {helper:lir::Helper::Python(CoreOp::IsDone),arguments,..} if arguments.as_slice()==[next_result])
            {
                continue;
            }
            if !matches!(truth.operation,Operation::Truth {value,..} if value==done_result) {
                continue;
            }
            let Terminator::Branch { condition, yes, no } = &h.terminator else {
                continue;
            };
            if Some(*condition) != truth.result {
                continue;
            }
            for (edge, other) in [(no, yes)] {
                let Some(b) = lir.blocks.get(edge.block.0) else {
                    continue;
                };
                if edge.block == other.block
                    || edge.block.0 == header
                    || edge.block.0 == 0
                    || other.block.0 == header
                    || lir
                        .osr_entries
                        .iter()
                        .any(|entry| entry.header == edge.block)
                {
                    continue;
                }
                if !matches!(&b.terminator,Terminator::Jump(back) if back.block.0==header) {
                    continue;
                }
                if b.instructions.iter().any(|i| i.result == Some(*iterator)) {
                    continue;
                }
                // Adding an operation requires reviewing its effects and the region pointer-lifetime contract.
                if b.instructions.iter().any(|i| {
                    !matches!(
                        i.operation,
                        Operation::FloatListView { .. }
                            | Operation::FloatListUnpack { .. }
                            | Operation::FloatListLoad { .. }
                            | Operation::FloatListStore { .. }
                            | Operation::SequenceUnpack { .. }
                            | Operation::SequenceGet {
                                index: lir::SequenceIndex::Constant(_),
                                ..
                            }
                            | Operation::Copy(_)
                            | Operation::Constant(_)
                            | Operation::GuardInt { .. }
                            | Operation::GuardFloat { .. }
                            | Operation::UnboxFloat(_)
                            | Operation::UnboxInt(_)
                            | Operation::IntToFloat(_)
                            | Operation::FloatBinary { .. }
                            | Operation::FloatCompare { .. }
                            | Operation::CheckedAdd { .. }
                            | Operation::BoxFloat(_)
                            | Operation::BoxInt(_)
                            | Operation::BoxBool(_)
                    )
                }) {
                    continue;
                }
                let predecessors = lir
                    .blocks
                    .iter()
                    .enumerate()
                    .filter(|(_, block)| match &block.terminator {
                        Terminator::Jump(e) => e.block == edge.block,
                        Terminator::Branch { yes, no, .. } => {
                            yes.block == edge.block || no.block == edge.block
                        }
                        _ => false,
                    })
                    .map(|(id, _)| id)
                    .collect::<Vec<_>>();
                if predecessors != [header] {
                    continue;
                }
                result.push((BlockId(header), edge.block, *iterator, *state));
            }
        }
        result
    }
}

pub(crate) const TUPLE: u64 = 1;
pub(crate) const LIST: u64 = 2;
pub(crate) const FLOATS: u64 = 3;
pub(crate) const ITERATOR: u64 = 4;
pub(crate) const DONE: u64 = 5;
pub(crate) const RANGE: u64 = 6;
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub(crate) struct Descriptor {
    pub kind: u64,
    pub heap: u64,
    pub generation: u64,
    pub len: u64,
    pub data: u64,
    pub source: Slot,
    pub index: u64,
    pub exhausted: u64,
}
#[derive(Default)]
pub(crate) struct HeapViews {
    pub descriptors: Vec<Descriptor>,
    buffers: Vec<Box<[Slot]>>,
    floats: Vec<ObjectId>,
    epoch: usize,
    pub floats_dirty: bool,
    pub schemas: std::collections::BTreeSet<Vec<u64>>,
    queue: Vec<Value>,
    visits: Vec<usize>,
    visit_epoch: usize,
}
impl HeapViews {
    pub fn clear(&mut self) {
        self.descriptors.clear();
        self.buffers.clear();
        self.floats.clear();
        self.floats_dirty = true;
        self.schemas.clear();
    }
    pub fn prepare(
        &mut self,
        heap: &mut Heap,
        roots: impl IntoIterator<Item = Result<Value>>,
        iterator: Value,
    ) -> Result<bool> {
        if self.epoch != heap.collection_epoch() {
            self.clear();
            self.epoch = heap.collection_epoch();
        }
        if heap.slot_count() > 65536 {
            return Ok(false);
        }
        self.descriptors
            .resize(heap.slot_count(), Descriptor::default());
        self.queue.clear();
        for value in roots {
            self.queue.push(value?);
        }
        self.queue.push(iterator);
        self.visits.resize(self.descriptors.len(), 0);
        if self.visit_epoch == usize::MAX {
            self.visits.fill(0);
            self.visit_epoch = 0;
        }
        self.visit_epoch += 1;
        while let Some(value) = self.queue.pop() {
            let Value::Ref(id) = value else { continue };
            let cached = self
                .descriptors
                .get(id.index as usize)
                .copied()
                .unwrap_or_default();
            if cached.generation == id.generation as u64
                && cached.heap == id.heap as u64
                && cached.kind != ITERATOR
            {
                continue;
            }
            let object = heap.get(value)?;
            if self.visits[id.index as usize] == self.visit_epoch {
                continue;
            }
            self.visits[id.index as usize] = self.visit_epoch;
            let mut d = Descriptor {
                heap: id.heap as u64,
                generation: id.generation as u64,
                ..Descriptor::default()
            };
            match object {
                Object::Tuple(values) => {
                    d.kind = TUPLE;
                    d.len = values.len() as u64;
                    self.queue.extend(values.iter().copied());
                    let buffer = values
                        .iter()
                        .copied()
                        .map(Slot::boxed)
                        .collect::<Vec<_>>()
                        .into_boxed_slice();
                    d.data = buffer.as_ptr() as u64;
                    self.buffers.push(buffer);
                }
                Object::List(values) => {
                    d.len = values.len() as u64;
                    if values.is_float()
                        || (!values.is_empty()
                            && values.iter().all(|v| matches!(v, Value::Float(_))))
                    {
                        d.kind = FLOATS;
                        self.floats.push(id);
                        self.floats_dirty = true;
                    } else {
                        d.kind = LIST;
                        self.queue.extend(values.iter().copied());
                        let buffer = values
                            .iter()
                            .copied()
                            .map(Slot::boxed)
                            .collect::<Vec<_>>()
                            .into_boxed_slice();
                        d.data = buffer.as_ptr() as u64;
                        self.buffers.push(buffer);
                    }
                }
                Object::Iterator { source, .. } => {
                    let source = *source;
                    self.queue.push(source);
                    d.kind = ITERATOR;
                    d.source = Slot::boxed(source);
                    let mut guard = heap.get_mut(value)?;
                    let Object::Iterator {
                        index, exhausted, ..
                    } = &mut *guard
                    else {
                        unreachable!()
                    };
                    // Stable arena payloads cannot move or be collected while the verified region runs.
                    // Refresh iterator field pointers after each boundary; generic next may replace the payload.
                    d.index = index.as_ptr() as u64;
                    d.exhausted = exhausted as *mut bool as u64;
                }
                Object::Range { start, stop, step } => {
                    d.kind = RANGE;
                    d.len = crate::execution::range_len(*start, *stop, *step)? as u64;
                    d.data = *start as u64;
                    d.index = *step as u64;
                }
                Object::Done => d.kind = DONE,
                _ => {}
            }
            self.descriptors[id.index as usize] = d;
        }
        // Rust helpers may have reborrowed float buffers. Renew every writable pointer at region entry.
        if self.floats_dirty {
            for &id in &self.floats {
                let mut guard = heap.get_mut(Value::Ref(id))?;
                let Object::List(list) = &mut *guard else {
                    return Err(Error::InvalidIr("region float owner"));
                };
                let Some(values) = list.float_view() else {
                    return Ok(false);
                };
                self.descriptors[id.index as usize].data = values.as_mut_ptr() as u64;
                self.descriptors[id.index as usize].len = values.len() as u64;
            }
            self.floats_dirty = false;
        }
        let Value::Ref(id) = iterator else {
            return Ok(false);
        };
        let Some(d) = self.descriptors.get(id.index as usize) else {
            return Ok(false);
        };
        if d.kind != ITERATOR || d.source.tag & KIND_MASK != REF {
            return Ok(false);
        }
        let Some(source) = self.descriptors.get(d.source.a as u32 as usize) else {
            return Ok(false);
        };
        Ok(matches!(source.kind, TUPLE | LIST | FLOATS | RANGE))
    }
}
