//! Reentrant Python protocols. Exact builtin operations remain in the heap/runtime services.
use super::*;

impl Vm {
    pub(crate) fn instance_class(&self, value: Value) -> Option<Value> {
        match self.heap.get(value) {
            Ok(Object::Instance { class, .. }) => Some(*class),
            _ => None,
        }
    }

    pub(super) fn subclass(&self, derived: Value, base: Value) -> Result<bool> {
        self.class_mro(base)?;
        Ok(self.class_mro(derived)?.contains(&base))
    }

    pub(crate) fn special(&self, receiver: Value, name: &str) -> Result<Option<Value>> {
        match self.instance_class(receiver) {
            Some(class) => self.class_attribute(class, name),
            None => Ok(None),
        }
    }

    pub(crate) fn call_special(
        &mut self,
        receiver: Value,
        name: &str,
        args: &[Value],
    ) -> Result<Option<Value>> {
        let Some(method) = self.special(receiver, name)? else {
            return Ok(None);
        };
        if matches!(self.heap.get(method), Ok(Object::Function(_)))
            || matches!(self.heap.get(method),Ok(Object::Native(name)) if name=="BaseException.__init__")
        {
            let args = std::iter::once(receiver)
                .chain(args.iter().copied())
                .collect::<Vec<_>>();
            return self.call(method, &args).map(Some);
        }
        let class = self.instance_class(receiver).unwrap();
        let callable = self.descriptor_get(method, Some(receiver), class)?;
        self.call(callable, args).map(Some)
    }

    fn not_implemented(&self, value: Value) -> bool {
        matches!(self.heap.get(value), Ok(Object::Native(name)) if name == "NotImplemented")
    }

    pub(crate) fn protocol_roots<T>(
        &mut self,
        values: &[Value],
        f: impl FnOnce(&mut Self) -> Result<T>,
    ) -> Result<T> {
        let start = self.transient_roots.len();
        self.transient_roots.extend_from_slice(values);
        let result = f(self);
        self.transient_roots.truncate(start);
        result
    }

    fn binary_special(
        &mut self,
        a: Value,
        b: Value,
        left: &str,
        right: &str,
    ) -> Result<Option<Value>> {
        let ca = self.instance_class(a);
        let cb = self.instance_class(b);
        let reflected_first = if let (Some(ca), Some(cb)) = (ca, cb) {
            ca != cb
                && self.subclass(cb, ca)?
                && self.class_attribute(cb, right)? != self.class_attribute(ca, right)?
        } else {
            false
        };
        if reflected_first
            && let Some(value) = self.call_special(b, right, &[a])?
            && !self.not_implemented(value)
        {
            return Ok(Some(value));
        }
        if let Some(value) = self.call_special(a, left, &[b])?
            && !self.not_implemented(value)
        {
            return Ok(Some(value));
        }
        if !reflected_first
            && ca != cb
            && let Some(value) = self.call_special(b, right, &[a])?
            && !self.not_implemented(value)
        {
            return Ok(Some(value));
        }
        Ok(None)
    }

    pub(crate) fn py_binary(&mut self, op: &str, a: Value, b: Value) -> Result<Value> {
        if self.instance_class(a).is_some() || self.instance_class(b).is_some() {
            let stem = match op {
                "+" => "add",
                "-" => "sub",
                "*" => "mul",
                "/" => "truediv",
                "//" => "floordiv",
                "%" => "mod",
                "**" => "pow",
                "&" => "and",
                "|" => "or",
                "^" => "xor",
                "<<" => "lshift",
                ">>" => "rshift",
                _ => return Err(Error::Unsupported("binary protocol")),
            };
            if let Some(value) =
                self.binary_special(a, b, &format!("__{stem}__"), &format!("__r{stem}__"))?
            {
                return Ok(value);
            }
            if op == "*" {
                let sequence = |value| {
                    self.heap.as_str(&value).is_some()
                        || matches!(
                            self.heap.get(value),
                            Ok(Object::List(_) | Object::Tuple(_) | Object::Bytes(_))
                        )
                };
                let operands = if sequence(a) {
                    Some((a, b))
                } else if sequence(b) {
                    Some((b, a))
                } else {
                    None
                };
                if let Some((sequence, index)) = operands
                    && self.special(index, "__index__")?.is_some()
                {
                    return self.protocol_roots(&[sequence, index], |vm| {
                        let count = vm.py_index(index)?;
                        repeat(&vm.heap, sequence, count)
                    });
                }
            }
            return Err(Error::Type("unsupported binary operands"));
        }
        match op {
            "+" => runtime::add(&mut self.heap, a, b),
            "-" => runtime::subtract(&self.heap, a, b),
            _ => binary(&self.heap, op, a, b),
        }
    }

    pub(super) fn py_divmod(&mut self, a: Value, b: Value) -> Result<Value> {
        if let Some(value) = self.binary_special(a, b, "__divmod__", "__rdivmod__")? {
            return Ok(value);
        }
        let (q, r) = crate::integer::divmod(&self.heap, a, b)?;
        Ok(self.heap.allocate(Object::Tuple(vec![q, r])))
    }

    pub(crate) fn py_inplace(&mut self, op: &str, a: Value, b: Value) -> Result<Value> {
        if self.instance_class(a).is_some() {
            let stem = match op {
                "+" => "add",
                "-" => "sub",
                "*" => "mul",
                "/" => "truediv",
                "//" => "floordiv",
                "%" => "mod",
                "**" => "pow",
                "&" => "and",
                "|" => "or",
                "^" => "xor",
                "<<" => "lshift",
                ">>" => "rshift",
                _ => return Err(Error::Unsupported("inplace protocol")),
            };
            if let Some(value) = self.call_special(a, &format!("__i{stem}__"), &[b])?
                && !self.not_implemented(value)
            {
                return Ok(value);
            }
        }
        self.py_binary(op, a, b)
    }

    pub(crate) fn py_unary(&mut self, op: &str, value: Value) -> Result<Value> {
        if op == "not" {
            return Ok(Value::Bool(!self.py_truth(value)?));
        }
        let name = match op {
            "+" => "__pos__",
            "-" => "__neg__",
            "~" => "__invert__",
            _ => return Err(Error::Unsupported("unary operator")),
        };
        if let Some(result) = self.call_special(value, name, &[])? {
            return Ok(result);
        }
        if crate::integer::is_integer(&self.heap, value) {
            return crate::integer::unary(&self.heap, op, value);
        }
        match op {
            "+" => {
                number(value)?;
                Ok(value)
            }
            "-" => match value {
                Value::Int(n) => n
                    .checked_neg()
                    .map(Value::Int)
                    .ok_or(Error::IntegerOverflow),
                v => Ok(Value::Float(-number(v)?)),
            },
            _ => Ok(Value::Int(!integer(value)?)),
        }
    }

    pub(crate) fn py_len(&mut self, value: Value) -> Result<i64> {
        if !matches!(value, Value::Ref(_) | Value::InlineString(_)) {
            return Err(Error::Type("object has no length"));
        }
        if let Some(length) = self.call_special(value, "__len__", &[])? {
            let length =
                integer(length).map_err(|_| Error::Type("__len__ must return an integer"))?;
            if length < 0 {
                return Err(error("ValueError: __len__ must be non-negative"));
            }
            return Ok(length);
        }
        i64::try_from(self.heap.len(value)?).map_err(|_| Error::IntegerOverflow)
    }

    pub(crate) fn py_truth(&mut self, value: Value) -> Result<bool> {
        if !matches!(value, Value::Ref(_)) {
            return runtime::truth(&self.heap, value);
        }
        if self.not_implemented(value) {
            return Err(Error::Type("NotImplemented has no truth value"));
        }
        if self.instance_class(value).is_some() {
            if let Some(result) = self.call_special(value, "__bool__", &[])? {
                return match result {
                    Value::Bool(v) => Ok(v),
                    _ => Err(Error::Type("__bool__ must return bool")),
                };
            }
            if self.special(value, "__len__")?.is_some() {
                return self.py_len(value).map(|n| n != 0);
            }
        }
        runtime::truth(&self.heap, value)
    }

    pub(crate) fn py_index_value(&mut self, value: Value) -> Result<Value> {
        if crate::integer::is_integer(&self.heap, value) {
            return Ok(value);
        }
        match self.call_special(value, "__index__", &[])? {
            Some(v) if crate::integer::is_integer(&self.heap, v) => Ok(v),
            Some(_) => Err(Error::Type("__index__ must return an integer")),
            None => Err(Error::Type("index must be an integer")),
        }
    }

    pub(crate) fn py_index(&mut self, value: Value) -> Result<i64> {
        if let Ok(n) = integer(value) {
            return Ok(n);
        }
        let value = self.py_index_value(value)?;
        crate::integer::index(&self.heap, value)
    }

    pub(super) fn clamped_index(&mut self, value: Value) -> Result<i64> {
        let value = self.py_index_value(value)?;
        if let Ok(n) = integer(value) {
            return Ok(n);
        }
        let n = crate::integer::get(&self.heap, value)?;
        Ok(if n < num_bigint::BigInt::from(0) {
            i64::MIN
        } else {
            i64::MAX
        })
    }

    pub(crate) fn py_get_item(&mut self, object: Value, key: Value) -> Result<Value> {
        if let Some(value) = crate::sequence::get_builtin(&self.heap, object, key)? {
            return Ok(value);
        }
        if !matches!(object, Value::Ref(_) | Value::InlineString(_)) {
            return Err(Error::Type("object is not subscriptable"));
        }
        if let Some(result) = self.call_special(object, "__getitem__", &[key])? {
            return Ok(result);
        }
        if matches!(self.heap.get(object), Ok(Object::Dict(_))) {
            return self.mapping_get(object, key)?.ok_or(Error::MissingKey);
        }
        let sequence = matches!(
            self.heap.get(object),
            Ok(Object::List(_)
                | Object::Tuple(_)
                | Object::String(_)
                | Object::Bytes(_)
                | Object::Range { .. })
        ) || matches!(object, Value::InlineString(_));
        if sequence && crate::integer::is_big(&self.heap, key) {
            return Err(Error::Index);
        }
        let key = if sequence && self.instance_class(key).is_some() {
            Value::Int(self.protocol_roots(&[object, key], |vm| vm.py_index(key))?)
        } else {
            key
        };
        if matches!(self.heap.get(key), Ok(Object::Slice { .. })) {
            return self.protocol_roots(&[object, key], |vm| {
                let key = vm.normalized_slice(key)?;
                vm.heap.get_item(object, key)
            });
        }
        self.heap.get_item(object, key)
    }

    pub(crate) fn py_set_item(&mut self, object: Value, key: Value, value: Value) -> Result<()> {
        if crate::sequence::set(&mut self.heap, object, key, value)? {
            return Ok(());
        }
        if !matches!(object, Value::Ref(_)) {
            return Err(Error::Type("object does not support item assignment"));
        }
        if self.special(object, "__setitem__")?.is_some() {
            self.call_special(object, "__setitem__", &[key, value])?;
            return Ok(());
        }
        if matches!(self.heap.get(object), Ok(Object::Dict(_))) {
            return self.mapping_set(object, key, value, false);
        }
        if matches!(self.heap.get(object), Ok(Object::List(_)))
            && matches!(self.heap.get(key), Ok(Object::Slice { .. }))
        {
            return self.assign_slice(object, key, value);
        }
        if matches!(self.heap.get(object), Ok(Object::List(_)))
            && crate::integer::is_big(&self.heap, key)
        {
            return Err(Error::Index);
        }
        let key = if matches!(self.heap.get(object), Ok(Object::List(_)))
            && self.instance_class(key).is_some()
        {
            Value::Int(self.protocol_roots(&[object, key, value], |vm| vm.py_index(key))?)
        } else {
            key
        };
        self.heap.set_item(object, key, value)
    }

    pub(crate) fn py_compare_bool(&mut self, op: &str, a: Value, b: Value) -> Result<bool> {
        let value = self.py_compare(op, a, b)?;
        self.protocol_roots(&[value], |vm| vm.py_truth(value))
    }

    pub(crate) fn py_compare(&mut self, op: &str, a: Value, b: Value) -> Result<Value> {
        // Exact scalar operands cannot call Python or recurse through containers.
        if matches!(a, Value::Int(_) | Value::Float(_) | Value::Bool(_))
            && matches!(b, Value::Int(_) | Value::Float(_) | Value::Bool(_))
        {
            return compare(&self.heap, op, a, b).map(Value::Bool);
        }
        if self.protocol_depth >= 256 {
            return Err(error("RecursionError: recursive comparison"));
        }
        self.protocol_depth += 1;
        let result = stacker::maybe_grow(128 * 1024, 2 * 1024 * 1024, || {
            self.compare_protocol(op, a, b)
        });
        self.protocol_depth -= 1;
        result
    }

    fn compare_protocol(&mut self, op: &str, a: Value, b: Value) -> Result<Value> {
        if op == "is" || op == "is not" {
            return compare(&self.heap, op, a, b).map(Value::Bool);
        }
        if op == "in" || op == "not in" {
            let found = if matches!(self.heap.get(b), Ok(Object::Bytes(_))) {
                let probe = if integer(a).is_ok() || self.special(a, "__index__")?.is_some() {
                    Value::Int(self.py_index(a)?)
                } else {
                    a
                };
                if integer(probe).is_err() && !matches!(self.heap.get(probe), Ok(Object::Bytes(_)))
                {
                    return Err(error(format!(
                        "TypeError: a bytes-like object is required, not '{}'",
                        self.value_type_name(a)?
                    )));
                }
                compare(&self.heap, "in", probe, b)?
            } else if let Some(value) = self.call_special(b, "__contains__", &[a])? {
                self.protocol_roots(&[value], |vm| vm.py_truth(value))?
            } else if matches!(self.heap.get(b), Ok(Object::Dict(_) | Object::Set(_))) {
                self.mapping_get(b, a)?.is_some()
            } else if self.instance_class(b).is_some()
                || matches!(
                    self.heap.get(b),
                    Ok(Object::List(_)
                        | Object::Tuple(_)
                        | Object::Generator { .. }
                        | Object::Iterator { .. }
                        | Object::Enumerate { .. }
                        | Object::Zip { .. })
                )
            {
                self.with_iterator(b, |vm, iterator| {
                    loop {
                        let value = vm.next(iterator)?;
                        if vm.is_done(value) {
                            return Ok(false);
                        }
                        if value == a
                            || vm
                                .protocol_roots(&[value], |vm| vm.py_compare_bool("==", value, a))?
                        {
                            return Ok(true);
                        }
                    }
                })?
            } else {
                compare(&self.heap, "in", a, b)?
            };
            return Ok(Value::Bool(found == (op == "in")));
        }
        if self.instance_class(a).is_some() || self.instance_class(b).is_some() {
            let (left, right) = match op {
                "==" => ("__eq__", "__eq__"),
                "!=" => ("__ne__", "__ne__"),
                "<" => ("__lt__", "__gt__"),
                "<=" => ("__le__", "__ge__"),
                ">" => ("__gt__", "__lt__"),
                ">=" => ("__ge__", "__le__"),
                _ => return Err(Error::Unsupported("comparison protocol")),
            };
            // Rich comparisons try both operands even when they have the same type.
            let ca = self.instance_class(a);
            let cb = self.instance_class(b);
            let right_first = if let (Some(ca), Some(cb)) = (ca, cb) {
                ca != cb && self.subclass(cb, ca)?
            } else {
                false
            };
            for (receiver, method, arg) in if right_first {
                [(b, right, a), (a, left, b)]
            } else {
                [(a, left, b), (b, right, a)]
            } {
                if let Some(value) = self.call_special(receiver, method, &[arg])?
                    && !self.not_implemented(value)
                {
                    return Ok(value);
                }
            }
            if op == "!=" {
                let equal = self.py_compare("==", a, b)?;
                return self.protocol_roots(&[equal], |vm| Ok(Value::Bool(!vm.py_truth(equal)?)));
            }
            if op == "==" {
                return Ok(Value::Bool(a == b));
            }
            return Err(Error::Type("objects do not support ordering"));
        }
        if matches!(op, "==" | "!=") {
            let mapping = match (self.heap.get(a), self.heap.get(b)) {
                (Ok(Object::Dict(x)), Ok(Object::Dict(y))) => Some((false, x.clone(), y.len())),
                (Ok(Object::Set(x)), Ok(Object::Set(y))) => Some((true, x.clone(), y.len())),
                _ => None,
            };
            if let Some((set, entries, len)) = mapping {
                if a == b {
                    return Ok(Value::Bool(op == "=="));
                }
                if entries.len() != len {
                    return Ok(Value::Bool(op == "!="));
                }
                let mut roots = vec![a, b];
                for (key, value) in &entries {
                    key.visit_values(&mut |v| roots.push(v));
                    roots.push(*value);
                }
                let equal = self.protocol_roots(&roots, |vm| {
                    for (key, value) in &entries {
                        let Some(other) = vm.mapping_find(b, key)? else {
                            return Ok(false);
                        };
                        if !set {
                            let candidate = *vm.mapping(b)?.get(&other).ok_or(Error::MissingKey)?;
                            if *value != candidate
                                && !vm.protocol_roots(&[candidate], |vm| {
                                    vm.py_compare_bool("==", *value, candidate)
                                })?
                            {
                                return Ok(false);
                            }
                        }
                    }
                    Ok(true)
                })?;
                return Ok(Value::Bool(equal == (op == "==")));
            }
        }
        let sequences = match (self.heap.get(a), self.heap.get(b)) {
            (Ok(Object::List(x)), Ok(Object::List(y))) => Some((x.to_vec(), y.to_vec())),
            (Ok(Object::Tuple(x)), Ok(Object::Tuple(y))) => Some((x.clone(), y.clone())),
            _ => None,
        };
        if let Some((x, y)) = sequences {
            let roots = x.iter().chain(&y).copied().collect::<Vec<_>>();
            return self.protocol_roots(&roots, |vm| {
                if (op == "==" || op == "!=") && x.len() != y.len() {
                    return Ok(Value::Bool(op == "!="));
                }
                for (&a, &b) in x.iter().zip(&y) {
                    if a != b && !vm.py_compare_bool("==", a, b)? {
                        return if op == "==" || op == "!=" {
                            Ok(Value::Bool(op == "!="))
                        } else {
                            vm.py_compare(op, a, b)
                        };
                    }
                }
                compare(
                    &vm.heap,
                    op,
                    Value::Int(x.len() as i64),
                    Value::Int(y.len() as i64),
                )
                .map(Value::Bool)
            });
        }
        compare(&self.heap, op, a, b).map(Value::Bool)
    }

    pub(crate) fn stop_iteration(&mut self, err: &Error) -> bool {
        let stop = self.error_is(err, "StopIteration");
        if stop {
            self.pending_exception = None;
        }
        stop
    }
}
