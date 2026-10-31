use std::{
    cell::{Cell, RefCell},
    ops::{Deref, DerefMut},
    sync::atomic::{AtomicU32, Ordering},
};

pub(crate) use crate::native_abi::{NativeFieldDirectory, NativeFields};
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
    BigInt(std::rc::Rc<(Value, num_bigint::BigInt)>),
    String(compact_str::CompactString),
    Bytes(std::rc::Rc<(Value, Vec<u8>)>),
    Float(u64),
    Tuple(Vec<Key>),
    Protocol(Value, i64),
}

impl Key {
    pub(crate) fn has_protocol(&self) -> bool {
        matches!(self, Self::Protocol(..))
            || matches!(self,Self::Tuple(keys) if keys.iter().any(Self::has_protocol))
    }
    pub(crate) fn visit_values(&self, f: &mut impl FnMut(Value)) {
        match self {
            Self::Protocol(value, _) => f(*value),
            Self::Bytes(value) => f(value.0),
            Self::BigInt(value) => f(value.0),
            Self::Tuple(keys) => {
                for key in keys {
                    key.visit_values(f);
                }
            }
            _ => {}
        }
    }
}

#[derive(Debug, Clone)]
pub enum Object {
    String(String),
    Bytes(Vec<u8>),
    BigInt(num_bigint::BigInt),
    List(crate::List),
    Tuple(Vec<Value>),
    Dict(crate::Dict),
    Set(crate::Dict),
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
        shape: std::rc::Rc<crate::Shape>,
    },
    Instance {
        class: Value,
        attributes: crate::Attributes,
    },
    Range {
        start: i64,
        stop: i64,
        step: i64,
    },
    Iterator {
        source: Value,
        /// Builtin ranges are immutable in Python; cache their numeric parameters at construction.
        range: Option<(i64, i64)>,
        // UTF-8 byte offset for strings; element index for other sequences.
        index: Cell<usize>,
        size: usize,
        // Native builtin exhaustion shares the same interior-mutation contract as the cursor.
        exhausted: Cell<bool>,
    },
    Slice {
        start: Value,
        stop: Value,
        step: Value,
    },
    Generator {
        parts: std::rc::Rc<[Value]>,
        binding: Option<std::rc::Rc<(Value, String)>>,
        running: bool,
        done: bool,
    },
    Property {
        get: Value,
        set: Value,
        delete: Value,
        doc: Value,
    },
    Super {
        anchor: Value,
        receiver: Value,
    },
    StaticMethod(Value),
    ClassMethod(Value),
    Traceback {
        next: Option<Value>,
        function: crate::profile::FunctionId,
        depth: usize,
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
    Exception(Box<ExceptionData>),
}

/// Cold exception metadata stays boxed so it does not enlarge every heap entry.
#[derive(Debug, Clone)]
pub struct ExceptionData {
    pub kind: String,
    pub arguments: Value,
    pub attributes: std::collections::BTreeMap<String, Value>,
    pub cause: Option<Value>,
    pub context: Option<Value>,
    pub suppress_context: bool,
    pub traceback: Option<Value>,
}

impl Object {
    pub(crate) fn values(&self) -> Option<&[Value]> {
        match self {
            Self::List(v) => Some(v),
            Self::Tuple(v) => Some(v),
            _ => None,
        }
    }
}

#[derive(Debug)]
struct Entry {
    object: Object,
}

impl NativeFields {
    fn of(object: &Object, generation: u32, ascii: Option<bool>) -> Self {
        match object {
            Object::Instance { class, attributes } => {
                let class = crate::native_abi::Slot::boxed(*class);
                let (shape, values, len) = attributes.native_fields();
                Self {
                    kind: 1,
                    class_tag: class.tag,
                    class_payload: class.a,
                    generation: generation as u64,
                    shape,
                    values,
                    writable_values: std::ptr::null_mut(),
                    len,
                }
            }
            Object::Native(name) if matches!(name.as_ref(), "len" | "math.sqrt") => Self {
                kind: 10,
                class_tag: if name.as_ref() == "len" {
                    crate::native_abi::NATIVE_LEN
                } else {
                    crate::native_abi::NATIVE_SQRT
                },
                generation: generation as u64,
                ..Self::default()
            },
            Object::String(text) if ascii.unwrap_or_else(|| text.is_ascii()) => Self {
                kind: 9,
                generation: generation as u64,
                values: text.as_ptr().cast(),
                len: text.len(),
                ..Self::default()
            },
            Object::List(values) => Self {
                kind: if values.is_float() {
                    4
                } else if values.bools().is_some() {
                    8
                } else {
                    2
                },
                generation: generation as u64,
                // Kind 8 interprets this address as u8 storage; never decode it as a Value.
                values: values.bools().map_or_else(
                    || {
                        values
                            .native_values()
                            .map_or(std::ptr::null(), |v| v.as_ptr())
                    },
                    |v| v.as_ptr().cast(),
                ),
                shape: if values.bools().is_some() || values.is_float() {
                    values.native_write_token() as u64
                } else {
                    0
                },
                len: values.len(),
                ..Self::default()
            },
            Object::Tuple(values) => Self {
                kind: 3,
                generation: generation as u64,
                values: values.as_ptr(),
                len: values.len(),
                ..Self::default()
            },
            Object::Range { start, stop, step } => {
                match crate::execution::range_len(*start, *stop, *step) {
                    Ok(len) => Self {
                        kind: 5,
                        generation: generation as u64,
                        class_tag: *start as u64,
                        class_payload: *step as u64,
                        len,
                        ..Self::default()
                    },
                    Err(_) => Self::default(),
                }
            }
            Object::Iterator {
                range: Some((start, step)),
                index,
                size,
                exhausted,
                ..
            } if !exhausted.get() => Self {
                kind: 7,
                generation: generation as u64,
                class_tag: *start as u64,
                class_payload: *step as u64,
                shape: index.as_ptr() as u64,
                writable_values: exhausted.as_ptr().cast(),
                len: *size,
                ..Self::default()
            },
            Object::Iterator {
                source,
                index,
                size,
                exhausted,
                ..
            } if !exhausted.get() => {
                let source = crate::native_abi::Slot::boxed(*source);
                Self {
                    kind: 6,
                    generation: generation as u64,
                    class_tag: source.tag,
                    class_payload: source.a,
                    shape: index.as_ptr() as u64,
                    writable_values: exhausted.as_ptr().cast(),
                    len: *size,
                    ..Self::default()
                }
            }
            _ => Self::default(),
        }
    }

    fn of_mut(object: &mut Object, generation: u32) -> Self {
        let mut row = Self::of(object, generation, None);
        if let Object::List(list) = object {
            if let Some(values) = list.native_values_mut() {
                row.values = values;
                row.writable_values = values;
            } else if let Some(values) = list.native_bools_mut() {
                row.values = values.cast();
                row.writable_values = values.cast();
            } else if let Some(values) = list.native_floats_mut() {
                row.values = values.cast();
                row.writable_values = values.cast();
            }
        }
        row
    }
}

#[derive(Debug)]
struct Generation {
    version: u32,
    live: bool,
    ascii: Option<bool>,
}

// The cached string property fits in padding beside the live bit.
const _: () = assert!(std::mem::size_of::<Generation>() == 8);

/// Monty's stable paged arena, with generation-checked handles and explicit-root collection.
#[derive(Debug)]
pub struct Heap {
    identity: u32,
    pub(crate) native_fields: Cell<NativeFieldDirectory>,
    field_rows: RefCell<Vec<NativeFields>>,
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
            native_fields: Cell::new(NativeFieldDirectory::default()),
            field_rows: RefCell::new(Vec::new()),
            entries: StableHeap::with_capacity(16),
            generations: RefCell::new(Vec::new()),
            live: Cell::new(0),
            collection_epoch: 0,
        }
    }

    /// Stable pages permit allocation while another object's shared reference is alive.
    pub fn allocate(&self, object: Object) -> Value {
        let ascii = match &object {
            Object::String(text) => Some(text.is_ascii()),
            _ => None,
        };
        let id = self.entries.allocate(Entry { object });
        self.live.set(self.live.get() + 1);
        let mut generations = self.generations.borrow_mut();
        let index = u32::try_from(id.index()).expect("heap slot index exhausted");
        if id.index() == generations.len() {
            generations.push(Generation {
                version: 1,
                live: true,
                ascii,
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
            slot.ascii = ascii;
        }
        let generation = generations[id.index()].version;
        if self.native_fields.get().heap != 0 {
            let mut rows = self.field_rows.borrow_mut();
            let fields = NativeFields::of(&self.entries.get(id).object, generation, ascii);
            if id.index() == rows.len() {
                rows.push(fields);
            } else {
                rows[id.index()] = fields;
            }
            self.publish_field_rows(&rows);
        }
        Value::Ref(ObjectId {
            heap: self.identity,
            index,
            generation: generations[id.index()].version,
        })
    }

    /// Returns inline UTF-8 when it fits; longer strings retain a heap identity.
    pub fn string(&self, text: &str) -> Value {
        match crate::InlineString::new(text) {
            Some(text) => Value::InlineString(text),
            None => self.allocate(Object::String(text.into())),
        }
    }

    pub fn string_owned(&self, text: String) -> Value {
        match crate::InlineString::new(&text) {
            Some(text) => Value::InlineString(text),
            None => self.allocate(Object::String(text)),
        }
    }

    pub fn as_str<'a>(&'a self, value: &'a Value) -> Option<&'a str> {
        match value {
            Value::InlineString(text) => Some(text.as_str()),
            Value::Ref(_) => match self.get(*value).ok()? {
                Object::String(text) => Some(text),
                _ => None,
            },
            _ => None,
        }
    }

    pub(crate) fn string_is_ascii(&self, value: Value) -> Result<bool> {
        if let Value::InlineString(text) = value {
            return Ok(text.as_str().is_ascii());
        }
        let id = self.checked_id(value)?;
        if let Some(ascii) = self.generations.borrow()[id.index()].ascii {
            return Ok(ascii);
        }
        let Object::String(text) = &self.entries.get(id).object else {
            return Err(Error::Type("expected string"));
        };
        let ascii = text.is_ascii();
        self.generations.borrow_mut()[id.index()].ascii = Some(ascii);
        Ok(ascii)
    }

    pub fn get(&self, value: Value) -> Result<&Object> {
        let id = self.checked_id(value)?;
        Ok(&self.entries.get(id).object)
    }

    pub fn get_mut(&mut self, value: Value) -> Result<impl DerefMut<Target = Object> + '_> {
        let id = self.checked_id(value)?;
        // A host may replace an object or edit a String through this borrow, even if it
        // forgets the guard. Recompute string metadata on the next read in that case.
        self.generations.get_mut()[id.index()].ascii = None;
        let fields = if self.native_fields.get().heap != 0 {
            let rows = self.field_rows.get_mut();
            let generation = self.generations.get_mut()[id.index()].version;
            // Invalidate before exposing mutable access. Forgetting the guard must stay safe.
            rows[id.index()] = NativeFields::default();
            let mut directory = self.native_fields.get();
            directory.entries = rows.as_ptr();
            self.native_fields.set(directory);
            Some(FieldMutation {
                rows,
                index: id.index(),
                generation,
                directory: &self.native_fields,
            })
        } else {
            None
        };
        Ok(ObjectMut(
            self.entries.entry(id).ok_or(Error::InvalidObject)?,
            fields,
        ))
    }

    /// Native backends share this directory; bytecode does not need it.
    pub(crate) fn enable_native_fields(&self) {
        if self.native_fields.get().heap != 0 {
            return;
        }
        let generations = self.generations.borrow();
        let mut rows = self.field_rows.borrow_mut();
        rows.extend(generations.iter().enumerate().map(|(index, generation)| {
            if generation.live {
                NativeFields::of(
                    &self.entries.get(HeapId::from_index(index)).object,
                    generation.version,
                    generation.ascii,
                )
            } else {
                NativeFields::default()
            }
        }));
        self.publish_field_rows(&rows);
    }

    fn publish_field_rows(&self, rows: &[NativeFields]) {
        // Allocation can move the row vector; generated code reloads this descriptor after callbacks.
        self.native_fields.set(NativeFieldDirectory {
            entries: rows.as_ptr(),
            len: rows.len(),
            heap: self.identity as u64,
        });
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
            Value::InlineString(text) => Ok(Key::String(text.as_str().into())),
            Value::None => Ok(Key::None),
            Value::Bool(value) => Ok(Key::Bool(value)),
            Value::Int(value) => Ok(Key::Int(value)),
            Value::Ref(_) => match self.get(value)? {
                Object::String(value) => Ok(Key::String(value.as_str().into())),
                Object::BigInt(n) => Ok(Key::BigInt(std::rc::Rc::new((value, n.clone())))),
                Object::Bytes(bytes) => Ok(Key::Bytes(std::rc::Rc::new((value, bytes.clone())))),
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
        if let Value::InlineString(text) = value {
            return Ok(text.as_str().chars().count());
        }
        Ok(match self.get(value)? {
            Object::String(text) => {
                if self.string_is_ascii(value)? {
                    text.len()
                } else {
                    text.chars().count()
                }
            }
            Object::Bytes(value) => value.len(),
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
        if let Value::InlineString(text) = object {
            return self.string_item(text.as_str(), key);
        }
        match self.get(object)? {
            Object::List(items) => items.value(index(key, items.len())?).ok_or(Error::Index),
            Object::Tuple(items) => Ok(items[index(key, items.len())?]),
            Object::Bytes(items) => Ok(Value::Int(i64::from(items[index(key, items.len())?]))),
            Object::String(text) => self.string_item(text, key),
            Object::Dict(items) => self.dict_get(items, key)?.ok_or(Error::MissingKey),
            _ => Err(Error::Type("object is not subscriptable")),
        }
    }

    fn string_item(&self, text: &str, key: Value) -> Result<Value> {
        let position = index(key, text.chars().count())?;
        let character = text.chars().nth(position).ok_or(Error::Index)?;
        Ok(self.string(character.encode_utf8(&mut [0; 4])))
    }

    pub(crate) fn dict_get(&self, items: &crate::Dict, key: Value) -> Result<Option<Value>> {
        // Borrow existing string storage for reads; no temporary owned Key is needed.
        if let Some(text) = self.as_str(&key) {
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
                if let Some(row) = self.field_rows.get_mut().get_mut(index) {
                    *row = NativeFields::default();
                }
                generation.live = false;
                freed += 1;
            }
        }
        if self.native_fields.get().heap != 0 {
            let rows = self.field_rows.get_mut();
            let mut directory = self.native_fields.get();
            directory.entries = rows.as_ptr();
            self.native_fields.set(directory);
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

struct FieldMutation<'a> {
    rows: &'a mut Vec<NativeFields>,
    index: usize,
    generation: u32,
    directory: &'a Cell<NativeFieldDirectory>,
}

struct ObjectMut<'a, G: DerefMut<Target = Entry>>(G, Option<FieldMutation<'a>>);

impl<G: DerefMut<Target = Entry>> Drop for ObjectMut<'_, G> {
    fn drop(&mut self) {
        if let Some(fields) = &mut self.1 {
            // Mutation can replace the object, change shape, or reallocate the field vector.
            // Publish fresh pointers from this exclusive borrow; no Rust payload borrow may
            // remain live when generated code uses the directory.
            fields.rows[fields.index] = NativeFields::of_mut(&mut self.0.object, fields.generation);
            let mut directory = fields.directory.get();
            directory.entries = fields.rows.as_ptr();
            fields.directory.set(directory);
        }
    }
}

impl<G: DerefMut<Target = Entry>> Deref for ObjectMut<'_, G> {
    type Target = Object;
    fn deref(&self) -> &Self::Target {
        &self.0.object
    }
}

impl<G: DerefMut<Target = Entry>> DerefMut for ObjectMut<'_, G> {
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
            (Self::BigInt(a), Self::BigInt(b)) => a == b,
            (Self::Bytes(a), Self::Bytes(b)) => a == b,
            (Self::List(a), Self::List(b)) => a == b,
            (Self::Tuple(a), Self::Tuple(b)) => a == b,
            (Self::Dict(a), Self::Dict(b)) => a == b,
            _ => false,
        }
    }
}

impl PartialEq for Key {
    fn eq(&self, other: &Self) -> bool {
        if let Self::BigInt(a) = self {
            return crate::integer::key_equal(&a.1, other);
        }
        if let Self::BigInt(b) = other {
            return crate::integer::key_equal(&b.1, self);
        }
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
            (Self::Bytes(a), Self::Bytes(b)) => a.1 == b.1,
            (Self::Tuple(a), Self::Tuple(b)) => a == b,
            (Self::Protocol(a, _), Self::Protocol(b, _)) => a == b,
            _ => false,
        }
    }
}

#[cfg(test)]
mod native_field_tests {
    use super::*;

    #[test]
    fn forgotten_mutation_leaves_a_miss_instead_of_a_stale_pointer() {
        let mut heap = Heap::new();
        let mut attributes = crate::Attributes::new(crate::Shape::new());
        attributes.insert("x".into(), Value::Int(42));
        let object = heap.allocate(Object::Instance {
            class: Value::None,
            attributes,
        });
        heap.enable_native_fields();
        let Value::Ref(id) = object else {
            unreachable!()
        };
        assert_ne!(heap.field_rows.borrow()[id.index as usize].generation, 0);
        let mut guard = heap.get_mut(object).unwrap();
        *guard = Object::Tuple(vec![]);
        std::mem::forget(guard);
        let rows = heap.field_rows.borrow();
        assert_eq!(rows[id.index as usize].generation, 0);
        assert!(rows[id.index as usize].values.is_null());
    }
}

#[cfg(test)]
mod string_property_tests {
    use super::*;

    #[test]
    fn string_metadata_is_invalidated_before_exposing_mutable_access() {
        let mut heap = Heap::new();
        let text = heap.allocate(Object::String("abcdefghijklmnop".into()));
        heap.enable_native_fields();
        let Value::Ref(id) = text else { unreachable!() };
        assert_eq!(heap.field_rows.borrow()[id.index as usize].kind, 9);
        assert!(heap.string_is_ascii(text).unwrap());
        {
            let mut guard = heap.get_mut(text).unwrap();
            let Object::String(value) = &mut *guard else {
                unreachable!()
            };
            // Keep the byte length unchanged while changing the Python character count.
            value.replace_range(..2, "é");
            std::mem::forget(guard);
        }
        assert_eq!(heap.field_rows.borrow()[id.index as usize].generation, 0);
        assert!(!heap.string_is_ascii(text).unwrap());
        assert_eq!(heap.len(text).unwrap(), 15);
        let key = heap.allocate(Object::Slice {
            start: Value::Int(1),
            stop: Value::Int(3),
            step: Value::None,
        });
        let sliced = heap.get_item(text, key).unwrap();
        assert_eq!(heap.as_str(&sliced), Some("cd"));
        if let Object::String(value) = &mut *heap.get_mut(text).unwrap() {
            *value = "abcdefghijklmnop".into();
        }
        assert!(heap.string_is_ascii(text).unwrap());
        assert_eq!(heap.len(text).unwrap(), 16);
        assert_eq!(heap.field_rows.borrow()[id.index as usize].kind, 9);
    }

    #[test]
    fn string_metadata_checks_identity_and_resets_on_slot_reuse() {
        let mut heap = Heap::new();
        let old = heap.allocate(Object::String("abcdefghijklmnop".into()));
        assert!(heap.string_is_ascii(old).unwrap());
        assert!(Heap::new().string_is_ascii(old).is_err());
        heap.collect(&[]).unwrap();
        let new = heap.allocate(Object::String("éabcdefghijklmnop".into()));
        let (Value::Ref(a), Value::Ref(b)) = (old, new) else {
            unreachable!()
        };
        assert_eq!(a.index, b.index);
        assert_ne!(a.generation, b.generation);
        assert!(heap.string_is_ascii(old).is_err());
        assert!(!heap.string_is_ascii(new).unwrap());
        assert_eq!(heap.len(new).unwrap(), 17);
    }

    #[test]
    fn replacing_a_non_string_object_recomputes_its_metadata() {
        let mut heap = Heap::new();
        let value = heap.allocate(Object::List(Vec::new().into()));
        *heap.get_mut(value).unwrap() = Object::String("aé😀z".into());
        assert!(!heap.string_is_ascii(value).unwrap());
        assert_eq!(heap.len(value).unwrap(), 4);
    }
}
