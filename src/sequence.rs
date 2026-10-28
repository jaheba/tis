//! Non-allocating exact-sequence operations shared by execution tiers.
use crate::{Heap, Object, Result, Value};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SequenceKind {
    List,
    Tuple,
}

fn items(heap: &Heap, source: Value, kind: SequenceKind) -> Result<Option<&[Value]>> {
    if !matches!(source, Value::Ref(_)) {
        return Ok(None);
    }
    Ok(match (heap.get(source)?, kind) {
        (Object::List(items), SequenceKind::List) | (Object::Tuple(items), SequenceKind::Tuple) => {
            Some(items)
        }
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
        .and_then(|index| items.get(index).copied()))
}
