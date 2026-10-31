//! Python lists keep float and boolean storage compact; generic borrows cache a boxed snapshot.
use crate::Value;
use std::{
    cell::{Cell, OnceCell},
    ops::{Deref, DerefMut},
};

#[derive(Debug, Clone)]
enum Storage {
    Values(Vec<Value>),
    Floats(Vec<f64>),
    Bools(Vec<u8>),
}
#[derive(Debug)]
pub struct List {
    storage: Storage,
    snapshot: OnceCell<Vec<Value>>,
    native_writes: Cell<u8>,
}
impl From<Vec<Value>> for List {
    fn from(values: Vec<Value>) -> Self {
        let storage = if !values.is_empty() && values.iter().all(|v| matches!(v, Value::Float(_))) {
            Storage::Floats(
                values
                    .into_iter()
                    .map(|v| {
                        let Value::Float(v) = v else { unreachable!() };
                        v
                    })
                    .collect(),
            )
        } else if !values.is_empty() && values.iter().all(|v| matches!(v, Value::Bool(_))) {
            Storage::Bools(
                values
                    .into_iter()
                    .map(|v| {
                        let Value::Bool(v) = v else { unreachable!() };
                        u8::from(v)
                    })
                    .collect(),
            )
        } else {
            Storage::Values(values)
        };
        Self {
            storage,
            snapshot: OnceCell::new(),
            native_writes: Cell::new(0),
        }
    }
}
// Keep the short Bool payload separate from whole-Value copies. Combining both in one
// return path causes LLVM to scalarize boxed list/tuple copies into individual padding bytes.
#[inline(never)]
pub(crate) fn bool_item(values: &[u8], index: usize) -> Option<Value> {
    values.get(index).map(|value| Value::Bool(*value != 0))
}

impl List {
    pub fn len(&self) -> usize {
        match &self.storage {
            Storage::Values(v) => v.len(),
            Storage::Floats(v) => v.len(),
            Storage::Bools(v) => v.len(),
        }
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    pub fn to_vec(&self) -> Vec<Value> {
        match &self.storage {
            Storage::Values(v) => v.clone(),
            Storage::Floats(v) => v.iter().copied().map(Value::Float).collect(),
            Storage::Bools(v) => v.iter().map(|v| Value::Bool(*v != 0)).collect(),
        }
    }
    pub fn is_float(&self) -> bool {
        matches!(self.storage, Storage::Floats(_))
    }
    pub fn value(&self, index: usize) -> Option<Value> {
        match &self.storage {
            Storage::Values(v) => v.get(index).copied(),
            Storage::Floats(v) => v.get(index).copied().map(Value::Float),
            Storage::Bools(v) => bool_item(v, index),
        }
    }
    pub(crate) fn native_values(&self) -> Option<&[Value]> {
        match &self.storage {
            Storage::Values(values) => Some(values),
            _ => None,
        }
    }

    /// Only an exclusive heap borrow may publish a pointer for allocation-free native stores.
    pub(crate) fn native_values_mut(&mut self) -> Option<*mut Value> {
        self.snapshot.take();
        match &mut self.storage {
            Storage::Values(values) => Some(values.as_mut_ptr()),
            _ => None,
        }
    }

    pub(crate) fn bools(&self) -> Option<&[u8]> {
        match &self.storage {
            Storage::Bools(v) => Some(v),
            _ => None,
        }
    }
    pub(crate) fn native_write_token(&self) -> *mut u8 {
        self.native_writes.as_ptr()
    }
    /// The native writer checks this token because a later shared borrow can cache a snapshot.
    pub(crate) fn native_bools_mut(&mut self) -> Option<*mut u8> {
        self.snapshot.take();
        match &mut self.storage {
            Storage::Bools(v) => {
                self.native_writes.set(1);
                Some(v.as_mut_ptr())
            }
            _ => None,
        }
    }
    pub fn repeat(&self, count: usize) -> Self {
        let storage = match &self.storage {
            Storage::Values(v) => return v.repeat(count).into(),
            Storage::Floats(v) => Storage::Floats(v.repeat(count)),
            Storage::Bools(v) => Storage::Bools(v.repeat(count)),
        };
        Self {
            storage,
            snapshot: OnceCell::new(),
            native_writes: Cell::new(0),
        }
    }
    pub fn floats(&self) -> Option<&[f64]> {
        match &self.storage {
            Storage::Floats(v) => Some(v),
            _ => None,
        }
    }
    pub(crate) fn float_view(&mut self) -> Option<&mut [f64]> {
        // Native regions cannot call generic list code. Clear snapshots before lending a writable view.
        self.snapshot.take();
        if let Storage::Values(values) = &self.storage {
            if !values.iter().all(|v| matches!(v, Value::Float(_))) {
                return None;
            }
            self.storage = Storage::Floats(
                values
                    .iter()
                    .map(|v| {
                        let Value::Float(v) = v else { unreachable!() };
                        *v
                    })
                    .collect(),
            );
        }
        match &mut self.storage {
            Storage::Floats(v) => Some(v),
            _ => None,
        }
    }
    /// Native pointers come from Vec itself, not a temporary unique slice borrow.
    pub(crate) fn native_float_view(&mut self) -> Option<(*mut f64, usize)> {
        let _ = self.float_view()?;
        let len = self.len();
        Some((self.native_floats_mut()?, len))
    }
    pub(crate) fn native_floats_mut(&mut self) -> Option<*mut f64> {
        self.snapshot.take();
        match &mut self.storage {
            Storage::Floats(values) => {
                self.native_writes.set(1);
                Some(values.as_mut_ptr())
            }
            _ => None,
        }
    }
    /// Preserve homogeneous primitive storage until a different type requires boxed values.
    pub fn push(&mut self, value: Value) {
        self.snapshot.take();
        if let (Storage::Floats(values), Value::Float(value)) = (&mut self.storage, value) {
            values.push(value);
            return;
        }
        if let (Storage::Bools(values), Value::Bool(value)) = (&mut self.storage, value) {
            values.push(u8::from(value));
            return;
        }
        if let (Storage::Values(values), Value::Bool(value)) = (&self.storage, value)
            && values.is_empty()
        {
            self.storage = Storage::Bools(vec![u8::from(value)]);
            return;
        }
        if let (Storage::Values(values), Value::Float(value)) = (&self.storage, value)
            && values.is_empty()
        {
            self.storage = Storage::Floats(vec![value]);
            return;
        }
        self.deref_mut().push(value);
    }
    pub fn remove(&mut self, index: usize) -> Value {
        self.snapshot.take();
        match &mut self.storage {
            Storage::Values(values) => values.remove(index),
            Storage::Floats(values) => Value::Float(values.remove(index)),
            Storage::Bools(values) => Value::Bool(values.remove(index) != 0),
        }
    }
    pub fn clear(&mut self) {
        self.snapshot.take();
        match &mut self.storage {
            Storage::Values(values) => values.clear(),
            Storage::Floats(values) => values.clear(),
            Storage::Bools(values) => values.clear(),
        }
    }
    pub fn reverse(&mut self) {
        self.snapshot.take();
        match &mut self.storage {
            Storage::Values(values) => values.reverse(),
            Storage::Floats(values) => values.reverse(),
            Storage::Bools(values) => values.reverse(),
        }
    }
    pub fn insert(&mut self, index: usize, value: Value) {
        self.snapshot.take();
        if let (Storage::Floats(values), Value::Float(value)) = (&mut self.storage, value) {
            values.insert(index, value);
            return;
        }
        if let (Storage::Bools(values), Value::Bool(value)) = (&mut self.storage, value) {
            values.insert(index, u8::from(value));
            return;
        }
        self.deref_mut().insert(index, value);
    }
    pub fn set(&mut self, index: usize, value: Value) {
        self.snapshot.take();
        if let (Storage::Floats(values), Value::Float(value)) = (&mut self.storage, value) {
            values[index] = value;
        } else if let (Storage::Bools(values), Value::Bool(value)) = (&mut self.storage, value) {
            values[index] = u8::from(value);
        } else {
            self.deref_mut()[index] = value;
        }
    }
    pub fn copy_from_slice(&mut self, values: &[Value]) {
        assert_eq!(self.len(), values.len());
        if let Storage::Floats(floats) = &mut self.storage
            && values.iter().all(|v| matches!(v, Value::Float(_)))
        {
            self.snapshot.take();
            for (dst, src) in floats.iter_mut().zip(values) {
                let Value::Float(v) = src else { unreachable!() };
                *dst = *v;
            }
            return;
        }
        if let Storage::Bools(bools) = &mut self.storage
            && values.iter().all(|v| matches!(v, Value::Bool(_)))
        {
            self.snapshot.take();
            for (dst, src) in bools.iter_mut().zip(values) {
                let Value::Bool(v) = src else { unreachable!() };
                *dst = u8::from(*v);
            }
            return;
        }
        self.deref_mut().copy_from_slice(values);
    }
}
impl Deref for List {
    type Target = Vec<Value>;
    fn deref(&self) -> &Vec<Value> {
        match &self.storage {
            Storage::Values(v) => v,
            Storage::Bools(v) => {
                // A native store must miss until an exclusive borrow discards this snapshot.
                self.native_writes.set(0);
                self.snapshot
                    .get_or_init(|| v.iter().map(|v| Value::Bool(*v != 0)).collect())
            }
            Storage::Floats(v) => {
                self.native_writes.set(0);
                self.snapshot
                    .get_or_init(|| v.iter().copied().map(Value::Float).collect())
            }
        }
    }
}
impl DerefMut for List {
    fn deref_mut(&mut self) -> &mut Vec<Value> {
        if !matches!(self.storage, Storage::Values(_)) {
            let values = self.snapshot.take().unwrap_or_else(|| self.to_vec());
            self.storage = Storage::Values(values);
            self.native_writes.set(0);
        }
        let Storage::Values(v) = &mut self.storage else {
            unreachable!()
        };
        v
    }
}
impl<'a> IntoIterator for &'a List {
    type Item = &'a Value;
    type IntoIter = std::slice::Iter<'a, Value>;
    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}
impl IntoIterator for List {
    type Item = Value;
    type IntoIter = std::vec::IntoIter<Value>;
    fn into_iter(self) -> Self::IntoIter {
        match self.storage {
            Storage::Values(v) => v,
            Storage::Floats(v) => v.into_iter().map(Value::Float).collect(),
            Storage::Bools(v) => v.into_iter().map(|v| Value::Bool(v != 0)).collect(),
        }
        .into_iter()
    }
}
impl PartialEq<Vec<Value>> for List {
    fn eq(&self, rhs: &Vec<Value>) -> bool {
        self.deref() == rhs
    }
}
impl<const N: usize> PartialEq<[Value; N]> for List {
    fn eq(&self, rhs: &[Value; N]) -> bool {
        self.as_slice() == rhs
    }
}

impl PartialEq for List {
    fn eq(&self, rhs: &Self) -> bool {
        self.as_slice() == rhs.as_slice()
    }
}
impl FromIterator<Value> for List {
    fn from_iter<T: IntoIterator<Item = Value>>(iter: T) -> Self {
        Vec::from_iter(iter).into()
    }
}

impl Clone for List {
    fn clone(&self) -> Self {
        Self {
            storage: self.storage.clone(),
            snapshot: OnceCell::new(),
            native_writes: Cell::new(0),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeat_and_gc_do_not_box_primitive_flags() {
        let list = List::from(vec![Value::Bool(true)]).repeat(1_000_000);
        assert_eq!(list.bools().unwrap().len(), 1_000_000);
        let mut vm = crate::Vm::new();
        let value = vm.heap.allocate(crate::Object::List(list));
        vm.pin(value).unwrap();
        vm.collect_garbage(&[]).unwrap();
        let crate::Object::List(list) = vm.heap.get(value).unwrap() else {
            unreachable!()
        };
        assert!(list.snapshot.get().is_none());
        assert!(list.bools().is_some());
    }
    #[test]
    fn float_native_permission_is_renewed_after_boxed_snapshots() {
        let mut list = List::from(vec![Value::Float(1.0), Value::Float(2.0)]);
        let (_, len) = list.native_float_view().unwrap();
        assert_eq!(len, 2);
        assert_eq!(list.native_writes.get(), 1);
        let old = list.deref().clone();
        assert_eq!(list.native_writes.get(), 0);
        let (pointer, _) = list.native_float_view().unwrap();
        assert_eq!(list.native_writes.get(), 1);
        // The exclusive list owns this initialized element; no payload borrow remains.
        unsafe {
            pointer.write(9.0);
        }
        assert_eq!(old[0], Value::Float(1.0));
        assert_eq!(list.deref()[0], Value::Float(9.0));
        assert_eq!(list.native_writes.get(), 0);
    }
}
