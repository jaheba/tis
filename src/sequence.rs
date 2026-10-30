//! Non-allocating exact-sequence operations shared by execution tiers.
use crate::{Heap, Object, Result, Value};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SequenceKind {
    List,
    Tuple,
}

pub(crate) enum Items<'a> {
    Values(&'a [Value]),
    Floats(&'a [f64]),
    Bools(&'a [u8]),
}
impl Items<'_> {
    pub fn len(&self) -> usize {
        match self {
            Self::Values(v) => v.len(),
            Self::Floats(v) => v.len(),
            Self::Bools(v) => v.len(),
        }
    }
    pub fn get(&self, index: usize) -> Option<Value> {
        match self {
            Self::Values(v) => v.get(index).copied(),
            Self::Floats(v) => v.get(index).copied().map(Value::Float),
            Self::Bools(v) => crate::list::bool_item(v, index),
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
        } else if let Some(bools) = v.bools() {
            Items::Bools(bools)
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

/// Store only into existing boxed list storage. No Python calls, VM allocation or collection.
pub(crate) fn set(heap: &mut Heap, source: Value, key: Value, value: Value) -> Result<bool> {
    let Value::Int(index) = key else {
        return Ok(false);
    };
    let Ok(Object::List(items)) = heap.get(source) else {
        return Ok(false);
    };
    if items.native_values().is_none()
        && !(items.bools().is_some() && matches!(value, Value::Bool(_)))
    {
        return Ok(false);
    }
    let index = if index < 0 {
        i128::from(index) + items.len() as i128
    } else {
        i128::from(index)
    };
    let Ok(index) = usize::try_from(index) else {
        return Ok(false);
    };
    if index >= items.len() {
        return Ok(false);
    }
    if let Object::List(items) = &mut *heap.get_mut(source)? {
        items.set(index, value);
    }
    Ok(true)
}

/// Exact builtin sequences and primitive integer indices cannot enter Python.
pub(crate) fn get_builtin(heap: &Heap, source: Value, key: Value) -> Result<Option<Value>> {
    let index = match key {
        Value::Int(i) => i,
        Value::Bool(b) => i64::from(b),
        _ => return Ok(None),
    };
    let Ok(object) = heap.get(source) else {
        return Ok(None);
    };
    let len = match object {
        Object::List(items) => items.len(),
        Object::Tuple(items) => items.len(),
        _ => return Ok(None),
    };
    let index = if index < 0 {
        i128::from(index) + len as i128
    } else {
        i128::from(index)
    };
    let Ok(index) = usize::try_from(index) else {
        return Ok(None);
    };
    Ok(match object {
        Object::List(items) => items.value(index),
        Object::Tuple(items) => items.get(index).copied(),
        _ => unreachable!(),
    })
}
