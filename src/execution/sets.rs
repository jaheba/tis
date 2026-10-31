//! Set algebra shares Python key hashing/equality with dictionary operations.
use super::*;
impl Vm {
    fn set_entries(&self, value: Value) -> Option<(&crate::Dict, bool)> {
        match self.heap.get(value).ok()? {
            Object::Set(items) => Some((items, false)),
            Object::FrozenSet(items) => Some((items, true)),
            _ => None,
        }
    }
    pub(super) fn set_binary(&mut self, op: &str, a: Value, b: Value) -> Result<Option<Value>> {
        if !matches!(op, "-" | "&" | "|" | "^") {
            return Ok(None);
        }
        let Some((left, frozen)) = self.set_entries(a) else {
            return Ok(None);
        };
        let Some((right, _)) = self.set_entries(b) else {
            return Ok(None);
        };
        let (left, right) = (left.clone(), right.clone());
        let mut roots = vec![a, b];
        for (key, value) in left.iter().chain(right.iter()) {
            key.visit_values(&mut |v| roots.push(v));
            roots.push(*value);
        }
        self.protocol_roots(&roots, |vm| {
            let mut output = crate::Dict::default();
            for (key, value) in &left {
                let present = vm.mapping_find(b, key)?.is_some();
                if match op {
                    "-" | "^" => !present,
                    "&" => present,
                    _ => true,
                } {
                    output.insert(key.clone(), *value);
                }
            }
            if matches!(op, "|" | "^") {
                for (key, value) in &right {
                    if op == "|" || vm.mapping_find(a, key)?.is_none() {
                        output.insert(key.clone(), *value);
                    }
                }
            }
            Ok(Some(vm.heap.allocate(if frozen {
                Object::FrozenSet(output)
            } else {
                Object::Set(output)
            })))
        })
    }
    pub(super) fn set_comparison(&mut self, op: &str, a: Value, b: Value) -> Result<Option<bool>> {
        if !matches!(op, "==" | "!=" | "<" | "<=" | ">" | ">=") {
            return Ok(None);
        }
        let Some((left, _)) = self.set_entries(a) else {
            return Ok(None);
        };
        let Some((right, _)) = self.set_entries(b) else {
            return Ok(None);
        };
        let (left, right) = (left.clone(), right.clone());
        let (small, big, target) = if matches!(op, ">" | ">=") {
            (&right, &left, a)
        } else {
            (&left, &right, b)
        };
        let cardinality = match op {
            "==" | "!=" => small.len() == big.len(),
            "<" | ">" => small.len() < big.len(),
            _ => small.len() <= big.len(),
        };
        let mut roots = vec![a, b];
        for (key, value) in left.iter().chain(right.iter()) {
            key.visit_values(&mut |v| roots.push(v));
            roots.push(*value);
        }
        self.protocol_roots(&roots, |vm| {
            let mut result = cardinality;
            if result {
                for (key, _) in small {
                    if vm.mapping_find(target, key)?.is_none() {
                        result = false;
                        break;
                    }
                }
            }
            Ok(Some(if op == "!=" { !result } else { result }))
        })
    }
}
