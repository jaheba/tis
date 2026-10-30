//! Python lists keep float storage compact; generic borrows materialize a read-only value snapshot.
use crate::Value;
use std::{
    cell::OnceCell,
    ops::{Deref, DerefMut},
};

#[derive(Debug, Clone)]
enum Storage {
    Values(Vec<Value>),
    Floats(Vec<f64>),
}
#[derive(Debug)]
pub struct List {
    storage: Storage,
    snapshot: OnceCell<Vec<Value>>,
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
        } else {
            Storage::Values(values)
        };
        Self {
            storage,
            snapshot: OnceCell::new(),
        }
    }
}
impl List {
    pub fn len(&self) -> usize {
        match &self.storage {
            Storage::Values(v) => v.len(),
            Storage::Floats(v) => v.len(),
        }
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    pub fn to_vec(&self) -> Vec<Value> {
        match &self.storage {
            Storage::Values(v) => v.clone(),
            Storage::Floats(v) => v.iter().copied().map(Value::Float).collect(),
        }
    }
    pub fn is_float(&self) -> bool {
        matches!(self.storage, Storage::Floats(_))
    }
    pub fn value(&self, index: usize) -> Option<Value> {
        match &self.storage {
            Storage::Values(v) => v.get(index).copied(),
            Storage::Floats(v) => v.get(index).copied().map(Value::Float),
        }
    }
    pub(crate) fn native_values(&self) -> Option<&[Value]> {
        match &self.storage {
            Storage::Values(values) => Some(values),
            Storage::Floats(_) => None,
        }
    }

    /// Only an exclusive heap borrow may publish a pointer for allocation-free native stores.
    pub(crate) fn native_values_mut(&mut self) -> Option<*mut Value> {
        self.snapshot.take();
        match &mut self.storage {
            Storage::Values(values) => Some(values.as_mut_ptr()),
            Storage::Floats(_) => None,
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
    /// Preserve compact float storage until a non-float requires the general representation.
    pub fn push(&mut self, value: Value) {
        self.snapshot.take();
        if let (Storage::Floats(values), Value::Float(value)) = (&mut self.storage, value) {
            values.push(value);
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
        }
    }
    pub fn clear(&mut self) {
        self.snapshot.take();
        match &mut self.storage {
            Storage::Values(values) => values.clear(),
            Storage::Floats(values) => values.clear(),
        }
    }
    pub fn reverse(&mut self) {
        self.snapshot.take();
        match &mut self.storage {
            Storage::Values(values) => values.reverse(),
            Storage::Floats(values) => values.reverse(),
        }
    }
    pub fn insert(&mut self, index: usize, value: Value) {
        self.snapshot.take();
        if let (Storage::Floats(values), Value::Float(value)) = (&mut self.storage, value) {
            values.insert(index, value);
            return;
        }
        self.deref_mut().insert(index, value);
    }
    pub fn set(&mut self, index: usize, value: Value) {
        self.snapshot.take();
        if let (Storage::Floats(values), Value::Float(value)) = (&mut self.storage, value) {
            values[index] = value;
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
        self.deref_mut().copy_from_slice(values);
    }
}
impl Deref for List {
    type Target = Vec<Value>;
    fn deref(&self) -> &Vec<Value> {
        match &self.storage {
            Storage::Values(v) => v,
            Storage::Floats(v) => self
                .snapshot
                .get_or_init(|| v.iter().copied().map(Value::Float).collect()),
        }
    }
}
impl DerefMut for List {
    fn deref_mut(&mut self) -> &mut Vec<Value> {
        if let Storage::Floats(v) = &self.storage {
            let values = self
                .snapshot
                .take()
                .unwrap_or_else(|| v.iter().copied().map(Value::Float).collect());
            self.storage = Storage::Values(values);
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
        }
    }
}
