//! Builtin slicing shares bound normalization; user __getitem__ receives the original slice.
use super::*;

impl Vm {
    pub(super) fn normalized_slice(&mut self, key: Value) -> Result<Value> {
        let Object::Slice { start, stop, step } = *self.heap.get(key)? else {
            return Err(Error::Type("expected slice"));
        };
        let mut normalized = [Value::None; 3];
        // Python converts step before start and stop.
        for (index, value) in [step, start, stop].into_iter().enumerate() {
            if value != Value::None {
                normalized[index] = Value::Int(self.clamped_index(value)?);
            }
        }
        if normalized[0] == Value::Int(0) {
            return Err(error("ValueError: slice step cannot be zero"));
        }
        if normalized == [step, start, stop] {
            return Ok(key);
        }
        Ok(self.heap.allocate(Object::Slice {
            step: normalized[0],
            start: normalized[1],
            stop: normalized[2],
        }))
    }

    pub(super) fn assign_slice(&mut self, object: Value, key: Value, value: Value) -> Result<()> {
        self.protocol_roots(&[object, key, value], |vm| {
            let key = vm.normalized_slice(key)?;
            let limits = slice_limits(&vm.heap, key, vm.heap.len(object)?)?;
            let values = vm.collect_iter(value)?;
            let mut items = match vm.heap.get(object)? {
                Object::List(items) => items.to_vec(),
                _ => return Err(Error::Type("slice assignment requires list")),
            };
            let (start, stop, step) = limits;
            if step == 1 {
                let start = (start as usize).min(items.len());
                let stop = (stop as usize).min(items.len()).max(start);
                items.splice(start..stop, values);
            } else {
                let indices = slice_positions(start, stop, step);
                if indices.len() != values.len() {
                    return Err(error(
                        "ValueError: extended slice assignment length mismatch",
                    ));
                }
                for (index, value) in indices.into_iter().zip(values) {
                    *items.get_mut(index).ok_or(Error::Index)? = value;
                }
            }
            if let Object::List(target) = &mut *vm.heap.get_mut(object)? {
                *target = items.into();
            }
            Ok(())
        })
    }

    pub(super) fn delete_slice(&mut self, object: Value, key: Value) -> Result<Value> {
        self.protocol_roots(&[object, key], |vm| {
            let key = vm.normalized_slice(key)?;
            let limits = slice_limits(&vm.heap, key, vm.heap.len(object)?)?;
            let mut indices = slice_positions(limits.0, limits.1, limits.2);
            indices.sort_unstable();
            let mut items = match vm.heap.get(object)? {
                Object::List(items) => items.to_vec(),
                _ => return Err(Error::Type("slice deletion requires list")),
            };
            for index in indices.into_iter().rev() {
                items.remove(index);
            }
            if let Object::List(target) = &mut *vm.heap.get_mut(object)? {
                *target = items.into();
            }
            Ok(Value::None)
        })
    }
}
