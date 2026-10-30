//! Insertion-ordered entries with hashed lookup. Deletion preserves order and rebuilds the index.
use crate::{Key, Value};
use std::{
    collections::{HashMap, hash_map::RandomState},
    hash::{BuildHasher, BuildHasherDefault, Hash, Hasher},
    ops::{Deref, Index},
};
// Only the index uses identity hashing; Python keys still receive the keyed RandomState hash.
// Rehashing that u64 with SipHash again adds work without improving the collision chains.
#[derive(Debug, Default)]
struct Prehashed(u64);
impl Hasher for Prehashed {
    fn finish(&self) -> u64 {
        self.0
    }
    fn write_u64(&mut self, value: u64) {
        self.0 = value;
    }
    fn write(&mut self, bytes: &[u8]) {
        self.0 = bytes
            .iter()
            .take(8)
            .enumerate()
            .fold(0, |value, (i, byte)| value | (u64::from(*byte) << (i * 8)));
    }
}

const END: usize = usize::MAX;
#[derive(Debug, Clone, Default)]
pub struct Dict {
    entries: Vec<(Key, Value)>,
    heads: HashMap<u64, usize, BuildHasherDefault<Prehashed>>,
    next: Vec<usize>,
    hasher: RandomState,
    protocol_keys: bool,
    version: u64,
}
impl Dict {
    pub(crate) fn has_protocol_keys(&self) -> bool {
        self.protocol_keys
    }
    pub(crate) fn version(&self) -> u64 {
        self.version
    }
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
            if matches!(&self.entries[index].0,Key::String(candidate) if candidate.as_str()==key) {
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
        self.protocol_keys |= key.has_protocol();
        self.version = self.version.wrapping_add(1);
        let index = self.entries.len();
        self.next
            .push(self.heads.insert(hash, index).unwrap_or(END));
        self.entries.push((key, value));
        None
    }
    pub fn remove(&mut self, key: &Key) -> Option<Value> {
        let index = self.position(key, self.hasher.hash_one(key))?;
        let (_, value) = self.entries.remove(index);
        self.version = self.version.wrapping_add(1);
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
        self.protocol_keys = false;
        self.version = self.version.wrapping_add(1);
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
            Self::Protocol(_, hash) => {
                6u8.hash(state);
                hash.hash(state);
            }
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
            Self::Bytes(v) => {
                7u8.hash(state);
                v.1.hash(state);
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

#[cfg(test)]
mod index_tests {
    use super::*;

    #[test]
    fn growth_replacement_deletion_and_borrowed_strings_preserve_order() {
        let mut dict = Dict::default();
        for i in 0..2000 {
            dict.insert(Key::String(format!("key{i}").into()), Value::Int(i));
        }
        for i in 0..2000 {
            assert_eq!(dict.get_str(&format!("key{i}")), Some(&Value::Int(i)));
        }
        assert_eq!(
            dict.insert(Key::String("key5".into()), Value::Int(9000)),
            Some(Value::Int(5))
        );
        for i in (0..2000).step_by(17) {
            assert_eq!(
                dict.remove(&Key::String(format!("key{i}").into())),
                Some(Value::Int(i))
            );
        }
        assert_eq!(dict[0].0, Key::String("key1".into()));
        assert_eq!(dict.get_str("key5"), Some(&Value::Int(9000)));
        assert_eq!(dict.get_str("key0"), None);
        let cloned = dict.clone();
        assert_eq!(cloned.get_str("key5"), Some(&Value::Int(9000)));
        dict.clear();
        dict.insert(Key::String("new".into()), Value::None);
        assert_eq!(dict.len(), 1);
    }

    #[test]
    fn compact_keys_keep_borrowed_hashes_across_storage_boundaries() {
        let mut dict = Dict::default();
        let keys = [
            "",
            "id",
            "é\0😀",
            "abcdefghijklmnopqrstuvw",
            "abcdefghijklmnopqrstuvwx",
            "abcdefghijklmnopqrstuvwxy",
            "long key with enough bytes to require an allocation",
        ];
        for (index, text) in keys.iter().enumerate() {
            dict.insert(Key::String((*text).into()), Value::Int(index as i64));
        }
        let cloned = dict.clone();
        for (index, text) in keys.iter().enumerate() {
            assert_eq!(cloned.get_str(text), Some(&Value::Int(index as i64)));
            assert_eq!(
                dict.remove(&Key::String((*text).into())),
                Some(Value::Int(index as i64))
            );
        }
        assert!(dict.is_empty());
    }

    #[test]
    fn colliding_hashes_still_compare_keys() {
        let mut dict = Dict {
            entries: vec![
                (Key::Int(1), Value::Int(10)),
                (Key::Int(2), Value::Int(20)),
                (Key::Int(3), Value::Int(30)),
            ],
            next: vec![END, 0, 1],
            ..Dict::default()
        };
        dict.heads.insert(123, 2);
        for i in 1..=3 {
            assert_eq!(dict.position(&Key::Int(i), 123), Some((i - 1) as usize));
        }
        assert_eq!(dict.position(&Key::Int(4), 123), None);
    }
}
