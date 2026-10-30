//! Stable sorting with rooted callback values, shared by sorted() and list.sort().
use super::*;

impl Vm {
    pub(super) fn sorted(&mut self, args: &[Value], keywords: &[(String, Value)]) -> Result<Value> {
        arity(args, 1, 1)?;
        // Python consumes the iterable before list.sort validates keyword arguments.
        let items = self.collect_iter(args[0])?;
        let list = self.heap.allocate(Object::List(items.into()));
        self.protocol_roots(&[list], |vm| {
            vm.sort_list(list, &[], keywords)?;
            Ok(list)
        })
    }

    pub(super) fn sort_list(
        &mut self,
        receiver: Value,
        args: &[Value],
        keywords: &[(String, Value)],
    ) -> Result<Value> {
        arity(args, 0, 0)?;
        let mut key = Value::None;
        let mut reverse = Value::Bool(false);
        let (mut saw_key, mut saw_reverse) = (false, false);
        for (name, value) in keywords {
            let (slot, seen) = match name.as_str() {
                "key" => (&mut key, &mut saw_key),
                "reverse" => (&mut reverse, &mut saw_reverse),
                _ => {
                    return Err(error(format!(
                        "TypeError: '{name}' is an invalid keyword argument for sort()"
                    )));
                }
            };
            if *seen {
                return Err(error(format!(
                    "TypeError: multiple values for keyword argument '{name}'"
                )));
            }
            *seen = true;
            *slot = *value;
        }
        self.protocol_roots(&[receiver, key, reverse], |vm| {
            // Reverse conversion can call Python and mutate the source before it is detached.
            let reverse = vm.py_truth(reverse)?;
            vm.sort_list_values(receiver, key, reverse)
        })
    }

    fn sort_list_values(&mut self, receiver: Value, key: Value, reverse: bool) -> Result<Value> {
        let items = match self.heap.get(receiver)? {
            Object::List(items) => items.to_vec(),
            _ => return Err(Error::Type("sort requires list")),
        };
        // Key callbacks see an empty list. Detached items and callback-created keys must remain
        // rooted until sorting finishes, including error paths and collections inside a key.
        if let Object::List(list) = &mut *self.heap.get_mut(receiver)? {
            *list = vec![].into();
        }
        let roots = self.transient_roots.len();
        self.transient_roots.extend_from_slice(&items);
        self.transient_roots.push(key);
        let result = (|| {
            let mut keys = Vec::with_capacity(items.len());
            for &item in &items {
                let value = if key == Value::None {
                    item
                } else {
                    self.call(key, &[item])?
                };
                self.transient_roots.push(value);
                keys.push(value);
            }
            // Stable bottom-up merging permits fallible Python comparisons without a Rust
            // comparator that must invent an ordering after an exception.
            let mut order = (0..items.len()).collect::<Vec<_>>();
            let mut scratch = order.clone();
            let mut width = 1;
            while width < order.len() {
                for start in (0..order.len()).step_by(width * 2) {
                    let middle = (start + width).min(order.len());
                    let end = (middle + width).min(order.len());
                    let (mut left, mut right) = (start, middle);
                    for slot in &mut scratch[start..end] {
                        let take_right = if left == middle {
                            true
                        } else if right == end {
                            false
                        } else if reverse {
                            self.py_compare_bool("<", keys[order[left]], keys[order[right]])?
                        } else {
                            self.py_compare_bool("<", keys[order[right]], keys[order[left]])?
                        };
                        *slot = if take_right {
                            let i = order[right];
                            right += 1;
                            i
                        } else {
                            let i = order[left];
                            left += 1;
                            i
                        };
                    }
                }
                std::mem::swap(&mut order, &mut scratch);
                width = width.saturating_mul(2);
            }
            Ok(order.into_iter().map(|i| items[i]).collect::<Vec<_>>())
        })();
        let modified = match self.heap.get(receiver)? {
            Object::List(list) => !list.is_empty(),
            _ => false,
        };
        let restored = match &result {
            Ok(sorted) => sorted.clone(),
            Err(_) => items,
        };
        if let Object::List(list) = &mut *self.heap.get_mut(receiver)? {
            *list = restored.into();
        }
        self.transient_roots.truncate(roots);
        result?;
        if modified {
            return Err(error("ValueError: list modified during sort"));
        }
        Ok(Value::None)
    }
}
