//! Immutable instance layouts share transitions; cached field slots are guarded by layout identity.
use crate::Value;
pub(crate) use crate::native_abi::FieldSlot;
use std::{
    cell::{Cell, RefCell},
    collections::BTreeMap,
    rc::Rc,
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT_SHAPE: AtomicU64 = AtomicU64::new(1);

#[derive(Debug)]
pub struct Shape {
    id: u64,
    fields: BTreeMap<String, usize>,
    transitions: RefCell<BTreeMap<String, Rc<Shape>>>,
}
impl Shape {
    pub fn new() -> Rc<Self> {
        Self::layout(BTreeMap::new())
    }

    fn layout(fields: BTreeMap<String, usize>) -> Rc<Self> {
        Rc::new(Self {
            id: NEXT_SHAPE
                .try_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
                .expect("shape identity exhausted"),
            fields,
            transitions: RefCell::new(BTreeMap::new()),
        })
    }

    fn transition(&self, name: &str) -> Rc<Self> {
        if let Some(shape) = self.transitions.borrow().get(name).cloned() {
            return shape;
        }
        let mut fields = self.fields.clone();
        fields.insert(name.into(), fields.len());
        let shape = Self::layout(fields);
        self.transitions
            .borrow_mut()
            .insert(name.into(), shape.clone());
        shape
    }
}

#[derive(Debug, Clone)]
pub struct Attributes {
    shape: Rc<Shape>,
    values: Vec<Value>,
}
impl Attributes {
    pub fn new(shape: Rc<Shape>) -> Self {
        Self {
            shape,
            values: Vec::new(),
        }
    }
    pub fn get(&self, name: &str) -> Option<&Value> {
        self.values.get(*self.shape.fields.get(name)?)
    }
    pub(crate) fn remove(&mut self, name: &str) -> Option<Value> {
        let index = *self.shape.fields.get(name)?;
        let value = self.values.remove(index);
        let mut fields = self.shape.fields.clone();
        fields.remove(name);
        for slot in fields.values_mut() {
            if *slot > index {
                *slot -= 1;
            }
        }
        self.shape = Shape::layout(fields);
        Some(value)
    }
    pub fn values(&self) -> impl Iterator<Item = &Value> {
        self.values.iter()
    }
    pub fn insert(&mut self, name: String, value: Value) -> Option<Value> {
        self.insert_slot(&name, value).1
    }
    fn insert_slot(&mut self, name: &str, value: Value) -> (usize, Option<Value>) {
        if let Some(&index) = self.shape.fields.get(name) {
            return (
                index,
                Some(std::mem::replace(&mut self.values[index], value)),
            );
        }
        let shape = self.shape.transition(name);
        let index = self.values.len();
        self.values.push(value);
        self.shape = shape;
        (index, None)
    }
    /// Native buffered stores commit only existing slots, retaining the shape and backing storage.
    pub(crate) fn write_existing(&mut self, slot: usize, value: Value) -> crate::Result<()> {
        *self
            .values
            .get_mut(slot)
            .ok_or(crate::Error::InvalidIr("native field slot"))? = value;
        Ok(())
    }

    pub(crate) fn native_fields(&self) -> (u64, *const Value, usize) {
        (self.shape.id, self.values.as_ptr(), self.values.len())
    }

    pub(crate) fn read(&self, attribute: &Attribute) -> (Option<Value>, bool) {
        if let slot = attribute.cache.get()
            && slot.shape == self.shape.id
        {
            return (self.values.get(slot.index).copied(), true);
        }
        if let Some(&index) = self.shape.fields.get(&attribute.text) {
            attribute.cache.set(FieldSlot {
                shape: self.shape.id,
                index,
            });
            (self.values.get(index).copied(), false)
        } else {
            (None, false)
        }
    }
    pub(crate) fn write(&mut self, attribute: &Attribute, value: Value) -> bool {
        if let slot = attribute.cache.get()
            && slot.shape == self.shape.id
        {
            self.values[slot.index] = value;
            return true;
        }
        let (index, _) = self.insert_slot(&attribute.text, value);
        attribute.cache.set(FieldSlot {
            shape: self.shape.id,
            index,
        });
        false
    }
}

/// A field site caches layout metadata, never an instance or a pointer into its values.
#[repr(C)]
#[derive(Clone)]
pub struct Attribute {
    pub(crate) cache: Cell<FieldSlot>,
    pub(crate) text: String,
    pub(crate) inline_instances: bool,
}
impl From<String> for Attribute {
    fn from(text: String) -> Self {
        Self {
            text,
            cache: Cell::new(FieldSlot::default()),
            inline_instances: false,
        }
    }
}
impl std::fmt::Debug for Attribute {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.text.fmt(f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn transitions_are_shared_and_overwrites_preserve_layout() {
        let root = Shape::new();
        let mut a = Attributes::new(root.clone());
        let mut b = Attributes::new(root);
        for attrs in [&mut a, &mut b] {
            attrs.insert("x".into(), Value::Int(1));
            attrs.insert("y".into(), Value::Int(2));
        }
        assert!(Rc::ptr_eq(&a.shape, &b.shape));
        let x: Attribute = "x".to_string().into();
        assert_eq!(a.read(&x), (Some(Value::Int(1)), false));
        assert_eq!(b.read(&x), (Some(Value::Int(1)), true));
        let id = a.shape.id;
        assert!(a.write(&x, Value::Float(3.5)));
        assert_eq!(a.shape.id, id);
        assert_eq!(a.read(&x), (Some(Value::Float(3.5)), true));
        assert_eq!(b.get("x"), Some(&Value::Int(1)));
    }
    #[test]
    fn different_orders_and_new_fields_miss_cached_layouts() {
        let root = Shape::new();
        let mut a = Attributes::new(root.clone());
        let mut b = Attributes::new(root);
        a.insert("x".into(), Value::Int(1));
        b.insert("y".into(), Value::Int(2));
        b.insert("x".into(), Value::Int(3));
        let x: Attribute = "x".to_string().into();
        assert_eq!(a.read(&x), (Some(Value::Int(1)), false));
        assert_eq!(b.read(&x), (Some(Value::Int(3)), false));
        assert_eq!(b.read(&x), (Some(Value::Int(3)), true));
        a.insert("y".into(), Value::Int(4));
        assert_eq!(a.read(&x), (Some(Value::Int(1)), false));
    }
}
