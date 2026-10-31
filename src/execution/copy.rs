//! Dataclass construction and graph copying use the same attribute and protocol services as Python calls.
use super::*;
impl Vm {
    pub(crate) fn attributes_snapshot(&self, owner: Value) -> Result<Vec<(String, Value)>> {
        match self.heap.get(owner)? {
            Object::Instance { attributes, .. } => Ok(attributes.items()),
            Object::Function(closure) => Ok(closure
                .attributes
                .iter()
                .map(|(k, v)| (k.clone(), *v))
                .collect()),
            _ => Err(Error::Type("object has no instance dictionary")),
        }
    }
    pub(crate) fn attributes_dictionary(&mut self, owner: Value) -> Result<Value> {
        if let Object::Instance { attributes, .. } = self.heap.get(owner)?
            && let Some(value) = attributes.dictionary()
        {
            return Ok(value);
        }
        let value = self.heap.allocate(Object::Library(Box::new(
            crate::library::Payload::Attributes(owner),
        )));
        if let Object::Instance { attributes, .. } = &mut *self.heap.get_mut(owner)? {
            attributes.set_dictionary(value);
        }
        Ok(value)
    }
    pub(crate) fn attribute_mapping_method(
        &mut self,
        owner: Value,
        name: &str,
        args: &[Value],
    ) -> Result<Value> {
        match name {
            "update" => {
                arity(args, 0, 1)?;
                if let Some(&source) = args.first() {
                    let pairs = match self.heap.get(source)? {
                        Object::Dict(dict) => dict
                            .iter()
                            .map(|(key, value)| (key_value(&self.heap, key), *value))
                            .collect::<Vec<_>>(),
                        Object::Library(p) => {
                            let crate::library::Payload::Attributes(other) = &**p else {
                                return Err(Error::Type("expected mapping"));
                            };
                            self.attributes_snapshot(*other)?
                                .into_iter()
                                .map(|(k, v)| (self.heap.string(&k), v))
                                .collect()
                        }
                        _ => return Err(Error::Unsupported("instance dictionary update iterable")),
                    };
                    for (key, value) in pairs {
                        let key = self
                            .heap
                            .as_str(&key)
                            .ok_or(Error::Type("instance dictionary key must be str"))?
                            .to_owned();
                        self.raw_set_attribute(owner, &key.into(), value)?;
                    }
                }
                Ok(Value::None)
            }
            "copy" => {
                arity(args, 0, 0)?;
                let result = self.heap.allocate(Object::Dict(Default::default()));
                for (key, value) in self.attributes_snapshot(owner)? {
                    let key = self.heap.string(&key);
                    self.heap.set_item(result, key, value)?;
                }
                Ok(result)
            }
            "get" => {
                arity(args, 1, 2)?;
                let key = self
                    .heap
                    .as_str(&args[0])
                    .ok_or(Error::Type("instance dictionary key must be str"))?;
                Ok(self
                    .attributes_snapshot(owner)?
                    .into_iter()
                    .find_map(|(k, v)| (k == key).then_some(v))
                    .unwrap_or(args.get(1).copied().unwrap_or(Value::None)))
            }
            _ => Err(Error::Unsupported("instance dictionary method")),
        }
    }
    pub(crate) fn library_copy(
        &mut self,
        args: &[Value],
        keywords: &[(String, Value)],
    ) -> Result<Value> {
        arity(args, 1, 1)?;
        if !keywords.is_empty() {
            return Err(Error::Unsupported("deepcopy keyword arguments"));
        }
        let roots = self.transient_roots.len();
        let result = self.deep_copy(args[0], &mut vec![], 0);
        self.transient_roots.truncate(roots);
        result
    }
    fn copy_remember(&mut self, old: Value, new: Value, memo: &mut Vec<(Value, Value)>) {
        memo.push((old, new));
        self.transient_roots.push(new);
    }
    fn deep_copy(
        &mut self,
        value: Value,
        memo: &mut Vec<(Value, Value)>,
        depth: usize,
    ) -> Result<Value> {
        if depth > 128 {
            return Err(error("RecursionError: deepcopy nesting"));
        }
        if !matches!(value, Value::Ref(_)) {
            return Ok(value);
        }
        if let Some((_, copy)) = memo.iter().find(|(old, _)| *old == value) {
            return Ok(*copy);
        }
        self.transient_roots.push(value);
        let object = self.heap.get(value)?.clone();
        visit_children(&object, |value| self.transient_roots.push(value));
        match object {
            Object::List(items) => {
                let copy = self.heap.allocate(Object::List(Vec::new().into()));
                self.copy_remember(value, copy, memo);
                for item in items {
                    let item = self.deep_copy(item, memo, depth + 1)?;
                    if let Object::List(items) = &mut *self.heap.get_mut(copy)? {
                        items.push(item);
                    }
                }
                Ok(copy)
            }
            Object::Tuple(items) => {
                let copy = self.heap.allocate(Object::Tuple(vec![]));
                self.copy_remember(value, copy, memo);
                let values = items
                    .iter()
                    .map(|item| self.deep_copy(*item, memo, depth + 1))
                    .collect::<Result<Vec<_>>>()?;
                if values == items {
                    memo.iter_mut().find(|(old, _)| *old == value).unwrap().1 = value;
                    return Ok(value);
                }
                *self.heap.get_mut(copy)? = Object::Tuple(values);
                Ok(copy)
            }
            Object::Dict(items) => {
                let copy = self.heap.allocate(Object::Dict(Default::default()));
                self.copy_remember(value, copy, memo);
                for (key, item) in items.iter() {
                    let key = key_value(&self.heap, key);
                    let key = self.deep_copy(key, memo, depth + 1)?;
                    let item = self.deep_copy(*item, memo, depth + 1)?;
                    self.mapping_set(copy, key, item, false)?;
                }
                Ok(copy)
            }
            Object::ByteArray(bytes) => {
                let copy = self.heap.allocate(Object::ByteArray(bytes));
                self.copy_remember(value, copy, memo);
                Ok(copy)
            }
            Object::Library(payload)
                if matches!(&*payload, crate::library::Payload::Attributes(_)) =>
            {
                let crate::library::Payload::Attributes(owner) = *payload else {
                    unreachable!()
                };
                let dict = self.attribute_mapping_method(owner, "copy", &[])?;
                self.transient_roots.push(dict);
                self.deep_copy(dict, memo, depth + 1)
            }
            Object::Instance { class, attributes } => {
                if let Some(reduce) = self.special(value, "__reduce__")? {
                    let callable = self.descriptor_get(reduce, Some(value), class)?;
                    let reduction = self.call(callable, &[])?;
                    self.transient_roots.push(reduction);
                    let Object::Tuple(parts) = self.heap.get(reduction)? else {
                        return Err(Error::Type("__reduce__ must return tuple"));
                    };
                    let parts = parts.clone();
                    if !(2..=3).contains(&parts.len()) {
                        return Err(Error::Unsupported("deepcopy reduction with item iterators"));
                    }
                    let args = self.collect_iter(parts[1])?;
                    let copy = self.call(parts[0], &args)?;
                    self.copy_remember(value, copy, memo);
                    if let Some(&state) = parts.get(2) {
                        let state = self.deep_copy(state, memo, depth + 1)?;
                        self.transient_roots.push(state);
                        if self.special(copy, "__setstate__")?.is_some() {
                            self.call_special(copy, "__setstate__", &[state])?;
                        } else {
                            self.attribute_mapping_method(copy, "update", &[state])?;
                        }
                    }
                    return Ok(copy);
                }
                let copy = self.heap.allocate(Object::Instance {
                    class,
                    attributes: crate::Attributes::new(crate::Shape::new()),
                });
                self.copy_remember(value, copy, memo);
                for (name, item) in attributes.items() {
                    let item = self.deep_copy(item, memo, depth + 1)?;
                    self.raw_set_attribute(copy, &name.into(), item)?;
                }
                if let Some(storage) = attributes.builtin() {
                    let storage = self.deep_copy(storage, memo, depth + 1)?;
                    if let Object::Instance { attributes, .. } = &mut *self.heap.get_mut(copy)? {
                        attributes.set_builtin(storage);
                    }
                }
                Ok(copy)
            }
            Object::Set(items) => {
                let copy = self.heap.allocate(Object::Set(Default::default()));
                self.copy_remember(value, copy, memo);
                for (_, item) in items.iter() {
                    let item = self.deep_copy(*item, memo, depth + 1)?;
                    self.set_method(copy, "add", &[item])?;
                }
                Ok(copy)
            }
            Object::String(_)
            | Object::Bytes(_)
            | Object::BigInt(_)
            | Object::FrozenSet(_)
            | Object::Function(_)
            | Object::Native(_)
            | Object::Class { .. }
            | Object::Range { .. }
            | Object::Library(_) => Ok(value),
            _ => Err(Error::Unsupported("deepcopy object type")),
        }
    }
    pub(crate) fn library_dataclass(
        &mut self,
        name: &str,
        args: &[Value],
        keywords: &[(String, Value)],
    ) -> Result<Value> {
        if name == "dataclass" {
            arity(args, 1, 1)?;
            if !keywords.is_empty() {
                return Err(Error::Unsupported("dataclass options"));
            }
            let class = args[0];
            let Object::Class { namespace, .. } = self.heap.get(class)? else {
                return Err(Error::Type("dataclass requires class"));
            };
            let namespace = *namespace;
            let annotations = self.env(namespace)?.values.get("__annotations__").copied();
            let mut fields = vec![];
            let mut has_default = false;
            if let Some(annotations) = annotations {
                let Object::Dict(items) = self.heap.get(annotations)? else {
                    return Err(Error::Type("class annotations must be dict"));
                };
                for (key, _) in items.iter() {
                    let Key::String(name) = key else {
                        return Err(Error::Type("annotation key must be str"));
                    };
                    let default = self.env(namespace)?.values.get(name.as_str()).copied();
                    if has_default && default.is_none() {
                        return Err(Error::Type("non-default dataclass field follows default"));
                    }
                    has_default |= default.is_some();
                    fields.push(self.heap.allocate(Object::Tuple(vec![
                        self.heap.string(name),
                        Value::Bool(default.is_some()),
                        default.unwrap_or(Value::None),
                    ])));
                }
            }
            let fields = self.heap.allocate(Object::Tuple(fields));
            self.store(namespace, ".dataclass_fields", fields)?;
            for method in ["__init__", "__eq__", "__repr__"] {
                if self.env(namespace)?.values.get(method).is_none() {
                    let native = self
                        .heap
                        .allocate(Object::Native(format!("dataclasses.{method}").into()));
                    self.store(namespace, method, native)?;
                }
            }
            self.store(namespace, "__hash__", Value::None)?;
            self.attribute_protocols = true;
            return Ok(class);
        }
        arity(args, 1, usize::MAX)?;
        let instance = args[0];
        let class = self
            .instance_class(instance)
            .ok_or(Error::Type("dataclass method requires instance"))?;
        let fields = self
            .class_attribute(class, ".dataclass_fields")?
            .ok_or(Error::Type("missing dataclass fields"))?;
        let Object::Tuple(fields) = self.heap.get(fields)? else {
            return Err(Error::Type("invalid dataclass fields"));
        };
        let fields = fields.clone();
        let mut descriptions = vec![];
        for field in fields {
            let Object::Tuple(parts) = self.heap.get(field)? else {
                return Err(Error::Type("invalid dataclass field"));
            };
            descriptions.push((
                self.heap
                    .as_str(&parts[0])
                    .ok_or(Error::Type("invalid field name"))?
                    .to_owned(),
                parts[1] == Value::Bool(true),
                parts[2],
            ));
        }
        match name {
            "__init__" => {
                if args.len() - 1 > descriptions.len() {
                    return Err(Error::Type("too many dataclass arguments"));
                }
                let mut bound = BTreeMap::new();
                for ((name, _, _), &value) in descriptions.iter().zip(&args[1..]) {
                    bound.insert(name.clone(), value);
                }
                for (name, value) in keywords {
                    if !descriptions.iter().any(|(field, _, _)| field == name) {
                        return Err(Error::Type("unexpected dataclass keyword"));
                    }
                    if bound.insert(name.clone(), *value).is_some() {
                        return Err(Error::Type("multiple values for dataclass field"));
                    }
                }
                for (name, has_default, default) in descriptions {
                    let value = bound
                        .remove(&name)
                        .or(has_default.then_some(default))
                        .ok_or(Error::Type("missing dataclass field"))?;
                    self.raw_set_attribute(instance, &name.into(), value)?;
                }
                Ok(Value::None)
            }
            "__eq__" => {
                arity(args, 2, 2)?;
                if self.instance_class(args[1]) != Some(class) {
                    return Ok(self.heap.allocate(Object::Native("NotImplemented".into())));
                }
                for (name, _, _) in descriptions {
                    let a = self.attribute(instance, &name)?;
                    let equal = self.protocol_roots(&[a], |vm| {
                        let b = vm.attribute(args[1], &name)?;
                        vm.protocol_roots(&[b], |vm| vm.py_compare_bool("==", a, b))
                    })?;
                    if !equal {
                        return Ok(Value::Bool(false));
                    }
                }
                Ok(Value::Bool(true))
            }
            "__repr__" => {
                arity(args, 1, 1)?;
                if self.dataclass_repr.contains(&instance) {
                    return Ok(self.heap.string("..."));
                }
                self.dataclass_repr.push(instance);
                let result = (|| {
                    let Object::Class { name, .. } = self.heap.get(class)? else {
                        return Err(Error::Type("dataclass type"));
                    };
                    let name = name.clone();
                    let mut fields = vec![];
                    for (name, _, _) in descriptions {
                        let value = self.attribute(instance, &name)?;
                        fields.push(format!("{name}={}", self.py_repr(value, true)?));
                    }
                    Ok(self
                        .heap
                        .string_owned(format!("{name}({})", fields.join(", "))))
                })();
                self.dataclass_repr.pop();
                result
            }
            _ => Err(Error::Unsupported("dataclass service")),
        }
    }
}
