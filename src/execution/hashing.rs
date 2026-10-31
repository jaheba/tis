//! Protocol keys retain their Python owner. Builtin-only mappings keep their indexed fast path.
use super::*;
use std::hash::{DefaultHasher, Hash, Hasher};

pub(super) fn key_hash(key: &Key) -> i64 {
    let hash = match key {
        Key::Protocol(_, hash) => *hash,
        Key::Bool(value) => i64::from(*value),
        Key::BigInt(value) => crate::integer::python_hash(&value.1),
        Key::Int(value) => {
            let modulus = (1i128 << 61) - 1;
            ((*value as i128) % modulus) as i64
        }
        Key::Float(bits)
            if f64::from_bits(*bits).fract() == 0.0
                && f64::from_bits(*bits) >= i64::MIN as f64
                && f64::from_bits(*bits) < -(i64::MIN as f64) =>
        {
            key_hash(&Key::Int(f64::from_bits(*bits) as i64))
        }
        Key::Float(bits)
            if f64::from_bits(*bits).is_finite() && f64::from_bits(*bits).fract() == 0.0 =>
        {
            use num_traits::FromPrimitive;
            crate::integer::python_hash(
                &num_bigint::BigInt::from_f64(f64::from_bits(*bits)).unwrap(),
            )
        }
        Key::Tuple(keys) => {
            let mut state = DefaultHasher::new();
            7u8.hash(&mut state);
            for key in keys {
                key_hash(key).hash(&mut state);
            }
            state.finish() as i64
        }
        other => {
            let mut state = DefaultHasher::new();
            other.hash(&mut state);
            state.finish() as i64
        }
    };
    if hash == -1 { -2 } else { hash }
}

impl Vm {
    /// Exact string-keyed dictionary writes cannot call Python or allocate heap objects.
    pub(crate) fn try_set_string_dict(
        &mut self,
        object: Value,
        key: Value,
        value: Value,
    ) -> Result<bool> {
        if self.heap.as_str(&key).is_some()
            && matches!(self.heap.get(object), Ok(Object::Dict(items)) if !items.has_protocol_keys())
        {
            let builtin_key = self.heap.key(key)?;
            if let Object::Dict(items) = &mut *self.heap.get_mut(object)? {
                // RHS callbacks may have removed or replaced an augmented target. Insertion
                // preserves the current equal key and uses the already computed value.
                items.insert(builtin_key, value);
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Returns true when the update neither calls Python nor allocates heap objects.
    pub(crate) fn try_inplace_item(
        &mut self,
        op: &str,
        object: Value,
        key: Value,
        old: Value,
        rhs: Value,
    ) -> Result<bool> {
        let scalar = Self::inplace_scalar(op, old, rhs);
        if let Some(value) = scalar {
            // Exact builtin updates neither call Python nor collect. Root installation
            // is only needed for the protocol fallback below.
            if self.try_set_string_dict(object, key, value)? {
                return Ok(true);
            }
            if crate::sequence::set(&mut self.heap, object, key, value)? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    pub(crate) fn inplace_item(
        &mut self,
        op: &str,
        object: Value,
        key: Value,
        old: Value,
        rhs: Value,
    ) -> Result<bool> {
        if self.try_inplace_item(op, object, key, old, rhs)? {
            return Ok(true);
        }
        let scalar = Self::inplace_scalar(op, old, rhs);
        self.protocol_roots(&[object, key, old, rhs], |vm| {
            let value = match scalar {
                Some(value) => value,
                None => vm.inplace_value(op, old, rhs)?,
            };
            vm.protocol_roots(&[value], |vm| vm.py_set_item(object, key, value))?;
            Ok(false)
        })
    }

    pub(crate) fn py_key(&mut self, value: Value) -> Result<Key> {
        self.protocol_roots(&[value], |vm| vm.key_protocol(value, 0))
    }

    fn key_protocol(&mut self, value: Value, depth: usize) -> Result<Key> {
        if depth > 128 {
            return Err(error("RecursionError: tuple key depth"));
        }
        if self.instance_class(value).is_some() {
            let hash = match self.special(value, "__hash__")? {
                Some(Value::None) => return Err(Error::Type("unhashable object")),
                Some(_) => {
                    let result = self.call_special(value, "__hash__", &[])?.unwrap();
                    integer(result).map_err(|_| Error::Type("__hash__ must return an integer"))?
                }
                None => {
                    let Value::Ref(id) = value else {
                        unreachable!()
                    };
                    ((u64::from(id.index) << 32) ^ u64::from(id.generation) ^ u64::from(id.heap))
                        as i64
                }
            };
            return Ok(Key::Protocol(value, if hash == -1 { -2 } else { hash }));
        }
        if let Ok(Object::Tuple(items)) = self.heap.get(value) {
            let items = items.clone();
            let mut keys = vec![];
            for value in items {
                keys.push(self.key_protocol(value, depth + 1)?);
            }
            return Ok(Key::Tuple(keys));
        }
        match self.heap.key(value) {
            Ok(key) => Ok(key),
            Err(err)
                if matches!(
                    self.heap.get(value),
                    Ok(Object::List(_) | Object::Dict(_) | Object::Set(_) | Object::Slice { .. })
                ) =>
            {
                Err(err)
            }
            Err(_) if matches!(value, Value::Ref(_)) => {
                self.heap.get(value)?;
                let Value::Ref(id) = value else {
                    unreachable!()
                };
                Ok(Key::Protocol(
                    value,
                    ((u64::from(id.index) << 32) ^ u64::from(id.generation) ^ u64::from(id.heap))
                        as i64,
                ))
            }
            Err(err) => Err(err),
        }
    }

    pub(super) fn mapping(&self, object: Value) -> Result<&crate::Dict> {
        match self.heap.get(object)? {
            Object::Dict(items) | Object::Set(items) => Ok(items),
            _ => Err(Error::Type("mapping required")),
        }
    }

    pub(super) fn mapping_find(&mut self, object: Value, key: &Key) -> Result<Option<Key>> {
        if !key.has_protocol() && !self.mapping(object)?.has_protocol_keys() {
            return Ok(self.mapping(object)?.get(key).map(|_| key.clone()));
        }
        let hash = key_hash(key);
        loop {
            let version = self.mapping(object)?.version();
            let candidates = self
                .mapping(object)?
                .iter()
                .filter(|(candidate, _)| key_hash(candidate) == hash)
                .map(|(k, _)| k.clone())
                .collect::<Vec<_>>();
            let mut roots = vec![object];
            key.visit_values(&mut |v| roots.push(v));
            for candidate in &candidates {
                candidate.visit_values(&mut |v| roots.push(v));
            }
            let result = self.protocol_roots(&roots, |vm| {
                for candidate in &candidates {
                    let a = key_value(&vm.heap, candidate);
                    let b = key_value(&vm.heap, key);
                    let equal = a == b
                        || vm.protocol_roots(&[a, b], |vm| vm.py_compare_bool("==", a, b))?;
                    if equal && vm.mapping(object)?.get(candidate).is_some() {
                        return Ok((false, Some(candidate.clone())));
                    }
                    if vm.mapping(object)?.version() != version {
                        return Ok((true, None));
                    }
                }
                Ok((false, None))
            })?;
            if !result.0 {
                return Ok(result.1);
            }
        }
    }

    pub(super) fn mapping_get(&mut self, object: Value, key: Value) -> Result<Option<Value>> {
        self.protocol_roots(&[object, key], |vm| {
            let items = vm.mapping(object)?;
            if !items.has_protocol_keys() {
                // Borrow string storage and convert other builtin keys only once.
                if let Some(text) = vm.heap.as_str(&key) {
                    return Ok(items.get_str(text).copied());
                }
                if let Ok(key) = vm.heap.key(key) {
                    return Ok(items.get(&key).copied());
                }
            }
            let key = vm.py_key(key)?;
            let found = vm.mapping_find(object, &key)?;
            Ok(found.and_then(|key| vm.mapping(object).ok()?.get(&key).copied()))
        })
    }

    pub(super) fn mapping_set(
        &mut self,
        object: Value,
        key: Value,
        value: Value,
        set: bool,
    ) -> Result<()> {
        self.protocol_roots(&[object, key, value], |vm| {
            let key = match vm.heap.key(key) {
                Ok(key) => key,
                Err(_) => vm.py_key(key)?,
            };
            if !key.has_protocol() && !vm.mapping(object)?.has_protocol_keys() {
                let mut guard = vm.heap.get_mut(object)?;
                let (Object::Dict(items) | Object::Set(items)) = &mut *guard else {
                    return Err(Error::Type("mapping required"));
                };
                // Insertion already probes and preserves an equal key's original identity.
                if !set || items.get(&key).is_none() {
                    items.insert(key, value);
                }
                return Ok(());
            }
            let found = vm.mapping_find(object, &key)?;
            if set && found.is_some() {
                return Ok(());
            }
            let mut guard = vm.heap.get_mut(object)?;
            let (Object::Dict(items) | Object::Set(items)) = &mut *guard else {
                return Err(Error::Type("mapping required"));
            };
            items.insert(found.unwrap_or(key), value);
            Ok(())
        })
    }

    pub(super) fn mapping_method(
        &mut self,
        object: Value,
        name: &str,
        args: &[Value],
    ) -> Result<Value> {
        arity(args, 1, 2)?;
        let mut roots = vec![object];
        roots.extend_from_slice(args);
        self.protocol_roots(&roots, |vm| {
            let key = vm.py_key(args[0])?;
            let found = vm.mapping_find(object, &key)?;
            if let Some(found) = found {
                let value = *vm.mapping(object)?.get(&found).ok_or(Error::MissingKey)?;
                if name == "pop"
                    && let Object::Dict(items) = &mut *vm.heap.get_mut(object)?
                {
                    items.remove(&found);
                }
                return Ok(value);
            }
            let default = args.get(1).copied().unwrap_or(Value::None);
            if name == "setdefault"
                && let Object::Dict(items) = &mut *vm.heap.get_mut(object)?
            {
                items.insert(key, default);
            }
            if name == "pop" && args.len() == 1 {
                return Err(Error::MissingKey);
            }
            Ok(default)
        })
    }

    pub(crate) fn py_delete_item(&mut self, object: Value, key: Value) -> Result<Value> {
        if self.special(object, "__delitem__")?.is_some() {
            self.call_special(object, "__delitem__", &[key])?;
            return Ok(Value::None);
        }
        if matches!(self.heap.get(object), Ok(Object::Dict(_))) {
            return self.protocol_roots(&[object, key], |vm| {
                let key = vm.py_key(key)?;
                let key = vm.mapping_find(object, &key)?.ok_or(Error::MissingKey)?;
                if let Object::Dict(items) = &mut *vm.heap.get_mut(object)? {
                    items.remove(&key);
                }
                Ok(Value::None)
            });
        }
        if matches!(self.heap.get(object), Ok(Object::List(_))) {
            if matches!(self.heap.get(key), Ok(Object::Slice { .. })) {
                return self.delete_slice(object, key);
            }
            let len = self.heap.len(object)?;
            let key = Value::Int(self.py_index(key)?);
            let index = normalize_index(key, len)?;
            if let Object::List(items) = &mut *self.heap.get_mut(object)? {
                items.remove(index);
            }
            return Ok(Value::None);
        }
        Err(Error::Type("object does not support item deletion"))
    }
}
