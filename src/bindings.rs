//! Namespace names resolve to stable slots; replacing or removing a binding never renumbers them.
use crate::Value;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Default)]
pub struct Bindings {
    names: BTreeMap<String, usize>,
    slots: Vec<Option<Value>>,
}

impl Bindings {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get(&self, name: &str) -> Option<&Value> {
        self.slot(name).and_then(|slot| self.at(slot))
    }

    pub fn insert(&mut self, name: String, value: Value) -> Option<Value> {
        if let Some(&slot) = self.names.get(&name) {
            return self.slots[slot].replace(value);
        }
        self.names.insert(name, self.slots.len());
        self.slots.push(Some(value));
        None
    }

    pub fn remove(&mut self, name: &str) -> Option<Value> {
        let slot = self.slot(name)?;
        self.slots[slot].take()
    }

    /// Definition order follows stable slots, rather than the name index's lexical order.
    pub(crate) fn iter(&self) -> impl Iterator<Item = (&str, &Value)> {
        let mut names = self.names.iter().collect::<Vec<_>>();
        names.sort_by_key(|(_, slot)| **slot);
        names.into_iter().filter_map(move |(name, slot)| {
            self.slots[*slot]
                .as_ref()
                .map(|value| (name.as_str(), value))
        })
    }

    pub fn values(&self) -> impl Iterator<Item = &Value> {
        self.slots.iter().filter_map(Option::as_ref)
    }

    pub(crate) fn slot(&self, name: &str) -> Option<usize> {
        self.names.get(name).copied()
    }
    pub(crate) fn at(&self, slot: usize) -> Option<&Value> {
        self.slots.get(slot)?.as_ref()
    }
    pub(crate) fn set(&mut self, slot: usize, value: Value) {
        self.slots[slot] = Some(value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn removed_and_replaced_names_retain_their_slots() {
        let mut names = Bindings::new();
        names.insert("x".into(), Value::Int(1));
        let slot = names.slot("x").unwrap();
        names.insert("y".into(), Value::Int(2));
        assert_eq!(names.remove("x"), Some(Value::Int(1)));
        assert_eq!(names.at(slot), None);
        names.insert("x".into(), Value::Int(3));
        assert_eq!(names.slot("x"), Some(slot));
        assert_eq!(names.at(slot), Some(&Value::Int(3)));
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct NameSlot {
    pub environment: Value,
    pub namespace: Value,
    pub slot: usize,
}

/// A resolved-name site keeps a weak, environment-guarded slot address outside Python state.
#[derive(Clone)]
pub struct Name {
    pub(crate) text: String,
    pub(crate) cache: std::cell::Cell<Option<NameSlot>>,
}
impl From<String> for Name {
    fn from(text: String) -> Self {
        Self {
            text,
            cache: std::cell::Cell::new(None),
        }
    }
}
impl std::fmt::Debug for Name {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.text.fmt(f)
    }
}
