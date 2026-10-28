//! Insertion-ordered entries with hashed lookup. Deletion preserves order and rebuilds the index.
use crate::{Key, Value};
use std::{
    collections::{HashMap, hash_map::RandomState},
    hash::{BuildHasher, Hash, Hasher},
    ops::{Deref, Index},
};
const END: usize = usize::MAX;
#[derive(Debug, Clone, Default)]
pub struct Dict {
    entries: Vec<(Key, Value)>,
    heads: HashMap<u64, usize>,
    next: Vec<usize>,
    hasher: RandomState,
}
impl Dict {
    fn position(&self, key: &Key, hash: u64) -> Option<usize> {
        let mut index = self.heads.get(&hash).copied().unwrap_or(END);
        while index != END {
            if self.entries[index].0 == *key {
                return Some(index);
            }
            index = self.next[index];
        }
        None
    }
    pub fn get(&self, key: &Key) -> Option<&Value> {
        self.position(key, self.hasher.hash_one(key))
            .map(|i| &self.entries[i].1)
    }
    pub fn get_str(&self, key: &str) -> Option<&Value> {
        let mut state = self.hasher.build_hasher();
        4u8.hash(&mut state);
        key.hash(&mut state);
        let mut index = self.heads.get(&state.finish()).copied().unwrap_or(END);
        while index != END {
            if matches!(&self.entries[index].0,Key::String(candidate) if candidate==key) {
                return Some(&self.entries[index].1);
            }
            index = self.next[index];
        }
        None
    }
    pub fn insert(&mut self, key: Key, value: Value) -> Option<Value> {
        let hash = self.hasher.hash_one(&key);
        if let Some(index) = self.position(&key, hash) {
            return Some(std::mem::replace(&mut self.entries[index].1, value));
        }
        let index = self.entries.len();
        self.next
            .push(self.heads.insert(hash, index).unwrap_or(END));
        self.entries.push((key, value));
        None
    }
    pub fn remove(&mut self, key: &Key) -> Option<Value> {
        let index = self.position(key, self.hasher.hash_one(key))?;
        let (_, value) = self.entries.remove(index);
        self.heads.clear();
        self.next.clear();
        for (index, (key, _)) in self.entries.iter().enumerate() {
            self.next.push(
                self.heads
                    .insert(self.hasher.hash_one(key), index)
                    .unwrap_or(END),
            );
        }
        Some(value)
    }
    pub fn clear(&mut self) {
        self.entries.clear();
        self.heads.clear();
        self.next.clear();
    }
}
impl From<Vec<(Key, Value)>> for Dict {
    fn from(entries: Vec<(Key, Value)>) -> Self {
        let mut dict = Self::default();
        for (k, v) in entries {
            dict.insert(k, v);
        }
        dict
    }
}
impl Deref for Dict {
    type Target = [(Key, Value)];
    fn deref(&self) -> &Self::Target {
        &self.entries
    }
}
impl Index<usize> for Dict {
    type Output = (Key, Value);
    fn index(&self, index: usize) -> &Self::Output {
        &self.entries[index]
    }
}
impl<'a> IntoIterator for &'a Dict {
    type Item = &'a (Key, Value);
    type IntoIter = std::slice::Iter<'a, (Key, Value)>;
    fn into_iter(self) -> Self::IntoIter {
        self.entries.iter()
    }
}
impl PartialEq for Dict {
    fn eq(&self, other: &Self) -> bool {
        self.entries == other.entries
    }
}

impl Hash for Key {
    fn hash<H: Hasher>(&self, state: &mut H) {
        match self {
            Self::Bool(v) => Key::Int(i64::from(*v)).hash(state),
            Self::Float(bits) => {
                let value = f64::from_bits(*bits);
                // Numeric equality crosses bool/int/float, including signed zero and the exact
                // i64 lower boundary. The upper boundary is exclusive: i64::MAX rounds upward.
                if value.fract() == 0.0 && value >= i64::MIN as f64 && value < -(i64::MIN as f64) {
                    Key::Int(value as i64).hash(state);
                } else {
                    3u8.hash(state);
                    bits.hash(state);
                }
            }
            Self::Int(v) => {
                2u8.hash(state);
                v.hash(state);
            }
            Self::None => 0u8.hash(state),
            Self::String(v) => {
                4u8.hash(state);
                v.hash(state);
            }
            Self::Tuple(v) => {
                5u8.hash(state);
                v.len().hash(state);
                for item in v {
                    item.hash(state);
                }
            }
        }
    }
}
