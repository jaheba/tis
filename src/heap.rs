use std::{
    cell::{Cell, RefCell},
    ops::{Deref, DerefMut},
    sync::atomic::{AtomicU32, Ordering},
};

use crate::{Error, ObjectId, Result, Value};

// These modules retain the copied storage invariants and stable arena addresses.
#[allow(unsafe_code, dead_code)]
#[rustfmt::skip]
mod free_list;
#[allow(unsafe_code, dead_code)]
#[rustfmt::skip]
mod stable_heap;

use stable_heap::StableHeap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct HeapId(usize);

impl HeapId {
    pub(crate) fn from_index(index: usize) -> Self {
        Self(index)
    }
    pub(crate) fn index(self) -> usize {
        self.0
    }
}

static NEXT_HEAP: AtomicU32 = AtomicU32::new(1);

/// Dictionary keys are owned, so string keys do not depend on another object.
#[derive(Debug, Clone)]
pub enum Key {
    None,
    Bool(bool),
    Int(i64),
    String(String),
    Float(u64),
    Tuple(Vec<Key>),
}

#[derive(Debug, Clone)]
pub enum Object {
    String(String),
    List(crate::List),
    Tuple(Vec<Value>),
    Dict(crate::Dict),
    Set(Vec<Value>),
    Environment(crate::execution::Environment),
    Function(crate::execution::Closure),
    Native(std::borrow::Cow<'static, str>),
    Bound(Value, std::borrow::Cow<'static, str>),
    Method(Value, Value),
    Class {
        code: crate::profile::FunctionId,
        name: String,
        namespace: Value,
        bases: Vec<Value>,
    },
    Instance {
        class: Value,
        attributes: std::collections::BTreeMap<String, Value>,
    },
    Range {
        start: i64,
        stop: i64,
        step: i64,
    },
    Iterator {
        source: Value,
        index: usize,
        size: usize,
        exhausted: bool,
    },
    Slice {
        start: Value,
        stop: Value,
        step: Value,
    },
    Done,
    Enumerate {
        iterator: Value,
        index: i64,
    },
    Zip {
        iterators: Vec<Value>,
        done: bool,
    },
    View {
        source: Value,
        kind: String,
    },
}

impl Object {
    pub(crate) fn values(&self) -> Option<&[Value]> {
        match self {
            Self::List(v) => Some(v),
            Self::Tuple(v) | Self::Set(v) => Some(v),
            _ => None,
        }
    }
}

#[derive(Debug)]
struct Entry {
    object: Object,
}

#[derive(Debug)]
struct Generation {
    version: u32,
    live: bool,
}

/// Monty's stable paged arena, with generation-checked handles and explicit-root collection.
#[derive(Debug)]
pub struct Heap {
    identity: u32,
    entries: StableHeap<Entry>,
    generations: RefCell<Vec<Generation>>,
    live: Cell<usize>,
    collection_epoch: usize,
}

impl Heap {
    pub fn new() -> Self {
        Self {
            identity: NEXT_HEAP
                .try_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
                .expect("heap identity exhausted"),
            entries: StableHeap::with_capacity(16),
            generations: RefCell::new(Vec::new()),
            live: Cell::new(0),
            collection_epoch: 0,
        }
    }

    /// Stable pages permit allocation while another object's shared reference is alive.
    pub fn allocate(&self, object: Object) -> Value {
        let id = self.entries.allocate(Entry { object });
        self.live.set(self.live.get() + 1);
        let mut generations = self.generations.borrow_mut();
        let index = u32::try_from(id.index()).expect("heap slot index exhausted");
        if id.index() == generations.len() {
            generations.push(Generation {
                version: 1,
                live: true,
            });
        } else {
            let slot = &mut generations[id.index()];
            // Count reuse per slot, rather than imposing a 32-bit allocation limit on the heap.
            // Exhaustion must fail instead of making a stale identity valid again.
            slot.version = slot
                .version
                .checked_add(1)
                .expect("heap slot generation exhausted");
            slot.live = true;
        }
        Value::Ref(ObjectId {
            heap: self.identity,
            index,
            generation: generations[id.index()].version,
        })
    }

    pub fn get(&self, value: Value) -> Result<&Object> {
        let id = self.checked_id(value)?;
        Ok(&self.entries.get(id).object)
    }

    pub fn get_mut(&mut self, value: Value) -> Result<impl DerefMut<Target = Object> + '_> {
        let id = self.checked_id(value)?;
        Ok(ObjectMut(
            self.entries.entry(id).ok_or(Error::InvalidObject)?,
        ))
    }

    pub fn key(&self, value: Value) -> Result<Key> {
        self.key_at(value, 0)
    }

    fn key_at(&self, value: Value, depth: usize) -> Result<Key> {
        if depth > 128 {
            return Err(Error::Runtime("RecursionError: tuple key depth".into()));
        }
        match value {
            Value::Unbound => Err(Error::InvalidIr("unbound dictionary key")),
            Value::None => Ok(Key::None),
            Value::Bool(value) => Ok(Key::Bool(value)),
            Value::Int(value) => Ok(Key::Int(value)),
            Value::Ref(_) => match self.get(value)? {
                Object::String(value) => Ok(Key::String(value.clone())),
                Object::Tuple(values) => values
                    .iter()
                    .map(|&value| self.key_at(value, depth + 1))
                    .collect::<Result<Vec<_>>>()
                    .map(Key::Tuple),
                _ => Err(Error::Type("unhashable key")),
            },
            Value::Float(value) => {
                if value.is_nan() {
                    return Err(Error::Unsupported("NaN dictionary keys"));
                }
                Ok(Key::Float(value.to_bits()))
            }
        }
    }

    pub fn len(&self, value: Value) -> Result<usize> {
        Ok(match self.get(value)? {
            Object::String(value) => value.chars().count(),
            Object::List(value) => value.len(),
            Object::Tuple(value) => value.len(),
            Object::Dict(value) => value.len(),
            Object::Set(value) => value.len(),
            Object::View { source, .. } => return self.len(*source),
            Object::Range { start, stop, step } => {
                crate::execution::range_len(*start, *stop, *step)?
            }
            _ => return Err(Error::Type("object has no length")),
        })
    }

    pub fn get_item(&self, object: Value, key: Value) -> Result<Value> {
        if matches!(key, Value::Ref(_)) && matches!(self.get(key)?, Object::Slice { .. }) {
            return crate::execution::slice(self, object, key);
        }
        match self.get(object)? {
            Object::List(items) => items.value(index(key, items.len())?).ok_or(Error::Index),
            Object::Tuple(items) => Ok(items[index(key, items.len())?]),
            Object::String(text) => {
                let position = index(key, text.chars().count())?;
                let character = text.chars().nth(position).ok_or(Error::Index)?;
                Ok(self.allocate(Object::String(character.to_string())))
            }
            Object::Dict(items) => self.dict_get(items, key)?.ok_or(Error::MissingKey),
            _ => Err(Error::Type("object is not subscriptable")),
        }
    }

    pub(crate) fn dict_get(&self, items: &crate::Dict, key: Value) -> Result<Option<Value>> {
        // Borrow existing string storage for reads; no temporary owned Key is needed.
        if let Ok(Object::String(text)) = self.get(key) {
            return Ok(items.get_str(text).copied());
        }
        Ok(items.get(&self.key(key)?).copied())
    }

    pub fn set_item(&mut self, object: Value, key: Value, value: Value) -> Result<()> {
        if matches!(value, Value::Ref(_)) {
            self.checked_id(value)?;
        }
        let dict_key = if matches!(self.get(object)?, Object::Dict(_)) {
            Some(self.key(key)?)
        } else {
            None
        };
        match &mut *self.get_mut(object)? {
            Object::List(items) => {
                let index = index(key, items.len())?;
                items.set(index, value);
                Ok(())
            }
            Object::Dict(items) => {
                let key = dict_key.expect("dictionary key resolved before mutation");
                items.insert(key, value);
                Ok(())
            }
            _ => Err(Error::Type("object does not support item assignment")),
        }
    }

    pub(crate) fn collection_epoch(&self) -> usize {
        self.collection_epoch
    }
    /// All external values that must survive collection must be listed as roots.
    /// Marking completes before freeing anything, so invalid roots leave the heap unchanged.
    pub fn collect(&mut self, roots: &[Value]) -> Result<usize> {
        self.collection_epoch = self
            .collection_epoch
            .checked_add(1)
            .expect("collection epoch exhausted");
        let mut marked = vec![false; self.entries.len()];
        let mut pending = roots.to_vec();
        while let Some(value) = pending.pop() {
            if !matches!(value, Value::Ref(_)) {
                continue;
            }
            let id = self.checked_id(value)?;
            if std::mem::replace(&mut marked[id.index()], true) {
                continue;
            }
            crate::execution::visit_children(&self.entries.get(id).object, |value| {
                pending.push(value)
            });
        }
        // Finish validation and marking before freeing anything. Invalid external roots
        // therefore leave all objects intact, without requiring a temporary garbage vector.
        let mut freed = 0;
        for (index, generation) in self.generations.get_mut().iter_mut().enumerate() {
            if generation.live && !marked[index] {
                self.entries
                    .entry(HeapId::from_index(index))
                    .expect("live generation has an entry")
                    .free();
                generation.live = false;
                freed += 1;
            }
        }
        self.live.set(self.live.get() - freed);
        Ok(freed)
    }

    pub fn allocation_count(&self) -> usize {
        self.live.get()
    }
    pub fn slot_count(&self) -> usize {
        self.entries.len()
    }

    fn checked_id(&self, value: Value) -> Result<HeapId> {
        let Value::Ref(id) = value else {
            return Err(Error::Type("expected an object"));
        };
        if id.heap != self.identity
            || !self
                .generations
                .borrow()
                .get(id.index as usize)
                .is_some_and(|slot| slot.live && slot.version == id.generation)
        {
            return Err(Error::InvalidObject);
        }
        Ok(HeapId::from_index(id.index as usize))
    }
}

impl Default for Heap {
    fn default() -> Self {
        Self::new()
    }
}

struct ObjectMut<G>(G);

impl<G: Deref<Target = Entry>> Deref for ObjectMut<G> {
    type Target = Object;
    fn deref(&self) -> &Self::Target {
        &self.0.object
    }
}

impl<G: DerefMut<Target = Entry>> DerefMut for ObjectMut<G> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0.object
    }
}

fn index(value: Value, len: usize) -> Result<usize> {
    let value = match value {
        Value::Int(value) => value,
        Value::Bool(value) => i64::from(value),
        _ => return Err(Error::Type("index must be an integer")),
    };
    let index = if value < 0 {
        i64::try_from(len)
            .map_err(|_| Error::Index)?
            .checked_add(value)
            .ok_or(Error::Index)?
    } else {
        value
    };
    usize::try_from(index)
        .ok()
        .filter(|&index| index < len)
        .ok_or(Error::Index)
}

impl PartialEq for Object {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::String(a), Self::String(b)) => a == b,
            (Self::List(a), Self::List(b)) => a == b,
            (Self::Tuple(a), Self::Tuple(b)) => a == b,
            (Self::Dict(a), Self::Dict(b)) => a == b,
            _ => false,
        }
    }
}

impl PartialEq for Key {
    fn eq(&self, other: &Self) -> bool {
        let numeric = |key: &Key| match key {
            Key::Bool(value) => Some(Value::Bool(*value)),
            Key::Int(value) => Some(Value::Int(*value)),
            Key::Float(bits) => Some(Value::Float(f64::from_bits(*bits))),
            _ => None,
        };
        if let (Some(a), Some(b)) = (numeric(self), numeric(other)) {
            return crate::execution::numeric_order(a, b) == Some(Some(std::cmp::Ordering::Equal));
        }
        match (self, other) {
            (Self::None, Self::None) => true,
            (Self::String(a), Self::String(b)) => a == b,
            (Self::Tuple(a), Self::Tuple(b)) => a == b,
            _ => false,
        }
    }
}
