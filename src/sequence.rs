//! Non-allocating exact-sequence operations shared by execution tiers.
use crate::{Heap, Object, Result, Value};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SequenceKind {
    List,
    Tuple,
}

pub(crate) enum Items<'a> {
    Values(&'a [Value]),
    Floats(&'a [f64]),
}
impl Items<'_> {
    pub fn len(&self) -> usize {
        match self {
            Self::Values(v) => v.len(),
            Self::Floats(v) => v.len(),
        }
    }
    pub fn get(&self, index: usize) -> Option<Value> {
        match self {
            Self::Values(v) => v.get(index).copied(),
            Self::Floats(v) => v.get(index).copied().map(Value::Float),
        }
    }
}
pub(crate) fn items(heap: &Heap, source: Value, kind: SequenceKind) -> Result<Option<Items<'_>>> {
    if !matches!(source, Value::Ref(_)) {
        return Ok(None);
    }
    Ok(match (heap.get(source)?, kind) {
        (Object::List(v), SequenceKind::List) => Some(if let Some(f) = v.floats() {
            Items::Floats(f)
        } else {
            Items::Values(v)
        }),
        (Object::Tuple(v), SequenceKind::Tuple) => Some(Items::Values(v)),
        _ => None,
    })
}
pub(crate) fn unpack(
    heap: &Heap,
    source: Value,
    kind: SequenceKind,
    count: usize,
) -> Result<Option<Value>> {
    Ok(items(heap, source, kind)?
        .filter(|items| items.len() == count)
        .map(|_| source))
}
pub(crate) fn get(
    heap: &Heap,
    source: Value,
    kind: SequenceKind,
    index: Value,
) -> Result<Option<Value>> {
    let Value::Int(index) = index else {
        return Ok(None);
    };
    let Some(items) = items(heap, source, kind)? else {
        return Ok(None);
    };
    let index = if index < 0 {
        i128::from(index) + items.len() as i128
    } else {
        i128::from(index)
    };
    Ok(usize::try_from(index)
        .ok()
        .and_then(|index| items.get(index)))
}
