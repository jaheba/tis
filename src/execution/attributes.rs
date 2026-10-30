//! Descriptor precedence and attribute hooks, shared by all execution tiers.
use super::*;

impl Vm {
    pub(crate) fn descriptor_get(
        &mut self,
        descriptor: Value,
        receiver: Option<Value>,
        class: Value,
    ) -> Result<Value> {
        let object = self.heap.get(descriptor).ok().cloned();
        match object {
            Some(Object::Function(_)) if receiver.is_some() => Ok(self
                .heap
                .allocate(Object::Method(descriptor, receiver.unwrap()))),
            Some(Object::StaticMethod(function)) => Ok(function),
            Some(Object::ClassMethod(function)) => {
                Ok(self.heap.allocate(Object::Method(function, class)))
            }
            Some(Object::Property { get, .. }) if receiver.is_some() => {
                if get == Value::None {
                    return Err(error("AttributeError: property has no getter"));
                }
                self.call(get, &[receiver.unwrap()])
            }
            Some(Object::Instance { .. }) => {
                if let Some(value) = self.call_special(
                    descriptor,
                    "__get__",
                    &[receiver.unwrap_or(Value::None), class],
                )? {
                    Ok(value)
                } else {
                    Ok(descriptor)
                }
            }
            _ => Ok(descriptor),
        }
    }

    fn data_descriptor(&self, value: Value) -> Result<bool> {
        Ok(matches!(self.heap.get(value), Ok(Object::Property { .. }))
            || self.special(value, "__set__")?.is_some()
            || self.special(value, "__delete__")?.is_some())
    }

    pub(crate) fn default_get_attribute(
        &mut self,
        object: Value,
        name: &str,
        cached: Option<&crate::Attribute>,
    ) -> Result<Value> {
        if let Ok(Object::Super { anchor, receiver }) = self.heap.get(object) {
            let (anchor, receiver) = (*anchor, *receiver);
            return self.super_attribute(anchor, receiver, name);
        }
        if name == "__class__" {
            return self.python_type(object);
        }
        if let Ok(Object::Traceback { next, .. }) = self.heap.get(object) {
            return if name == "tb_next" {
                Ok(next.unwrap_or(Value::None))
            } else {
                Err(Error::Unsupported("traceback frame inspection"))
            };
        }
        if let Ok(Object::Native(kind)) = self.heap.get(object)
            && types::builtin_type_name(kind)
        {
            if name == "__name__" {
                return Ok(self.heap.string(kind));
            }
            if name == "__mro__" {
                return Ok(self.heap.allocate(Object::Tuple(self.class_mro(object)?)));
            }
        }
        if let Some(class) = self.instance_class(object) {
            if name == "__class__" {
                return Ok(class);
            }
            let descriptor = self.class_attribute(class, name)?;
            if let Some(value) = descriptor
                && self.data_descriptor(value)?
            {
                return self.descriptor_get(value, Some(object), class);
            }
            let (value, hit) = match self.heap.get(object)? {
                Object::Instance { attributes, .. } => {
                    if let Some(site) = cached {
                        attributes.read(site)
                    } else {
                        (attributes.get(name).copied(), false)
                    }
                }
                _ => unreachable!(),
            };
            if let Some(value) = value {
                if hit {
                    self.tier_stats.shape_read_hits += 1;
                }
                return Ok(value);
            }
            if let Some(value) = descriptor {
                return self.descriptor_get(value, Some(object), class);
            }
            return Err(error(format!("AttributeError: {name}")));
        }
        if matches!(self.heap.get(object), Ok(Object::Class { .. })) {
            if name == "__mro__" {
                return Ok(self.heap.allocate(Object::Tuple(self.class_mro(object)?)));
            }
            if name == "__bases__" {
                let Object::Class { bases, .. } = self.heap.get(object)? else {
                    unreachable!()
                };
                let bases = if bases.is_empty() {
                    self.modules
                        .get("type.object")
                        .copied()
                        .into_iter()
                        .collect()
                } else {
                    bases.clone()
                };
                return Ok(self.heap.allocate(Object::Tuple(bases)));
            }
            if name == "__name__" {
                let Object::Class { name, .. } = self.heap.get(object)? else {
                    unreachable!()
                };
                return Ok(self.heap.string(name));
            }
            if let Some(value) = self.class_attribute(object, name)? {
                return self.descriptor_get(value, None, object);
            }
        }
        self.builtin_attribute(object, name, cached)
    }

    pub(crate) fn protocol_get_attribute(
        &mut self,
        object: Value,
        name: &str,
        cached: Option<&crate::Attribute>,
    ) -> Result<Value> {
        // No descriptor or hook can precede an instance field until this guard is invalidated.
        if !self.attribute_protocols
            && name != "__class__"
            && let Ok(Object::Instance { attributes, .. }) = self.heap.get(object)
        {
            let (value, hit) = if let Some(site) = cached {
                attributes.read(site)
            } else {
                (attributes.get(name).copied(), false)
            };
            if let Some(value) = value {
                if hit {
                    self.tier_stats.shape_read_hits += 1;
                }
                return Ok(value);
            }
        }
        if self.instance_class(object).is_none() {
            return self.default_get_attribute(object, name, cached);
        }
        let result = if self.special(object, "__getattribute__")?.is_some() {
            let name = self.heap.string(name);
            self.call_special(object, "__getattribute__", &[name])
                .map(|v| v.unwrap())
        } else {
            self.default_get_attribute(object, name, cached)
        };
        match result {
            Err(err) if self.error_is(&err, "AttributeError") => {
                if self.special(object, "__getattr__")?.is_some() {
                    self.pending_exception = None;
                    let name = self.heap.string(name);
                    self.call_special(object, "__getattr__", &[name])
                        .map(|v| v.unwrap())
                } else {
                    Err(err)
                }
            }
            other => other,
        }
    }

    pub(crate) fn default_set_attribute(
        &mut self,
        object: Value,
        name: &str,
        value: Value,
    ) -> Result<Value> {
        if let Some(class) = self.instance_class(object)
            && let Some(descriptor) = self.class_attribute(class, name)?
        {
            let property = match self.heap.get(descriptor) {
                Ok(Object::Property { set, .. }) => Some(*set),
                _ => None,
            };
            if let Some(set) = property {
                if set == Value::None {
                    return Err(error("AttributeError: property has no setter"));
                }
                self.call(set, &[object, value])?;
                return Ok(Value::None);
            }
            if self.data_descriptor(descriptor)? {
                if self
                    .call_special(descriptor, "__set__", &[object, value])?
                    .is_none()
                {
                    return Err(error("AttributeError: descriptor has no setter"));
                }
                return Ok(Value::None);
            }
        }
        if self.set_exception_attribute(object, name, value)? {
            return Ok(Value::None);
        }
        self.raw_set_attribute(object, &crate::Attribute::from(name.to_owned()), value)
    }

    pub(crate) fn protocol_set_attribute(
        &mut self,
        object: Value,
        attribute: &crate::Attribute,
        value: Value,
    ) -> Result<Value> {
        if !self.attribute_protocols
            && matches!(self.heap.get(object), Ok(Object::Instance { .. }))
            && !matches!(
                attribute.text.as_str(),
                "args" | "__cause__" | "__context__" | "__suppress_context__" | "__traceback__"
            )
        {
            return self.raw_set_attribute(object, attribute, value);
        }
        if self.special(object, "__setattr__")?.is_some() {
            let name = self.heap.string(&attribute.text);
            self.call_special(object, "__setattr__", &[name, value])?;
            return Ok(Value::None);
        }
        if !self.attribute_protocols && self.exception_value_kind(object).is_none() {
            return self.raw_set_attribute(object, attribute, value);
        }
        self.default_set_attribute(object, &attribute.text, value)
    }

    pub(crate) fn delete_attribute(&mut self, object: Value, name: &str) -> Result<Value> {
        if self.special(object, "__delattr__")?.is_some() {
            let name = self.heap.string(name);
            self.call_special(object, "__delattr__", &[name])?;
            return Ok(Value::None);
        }
        self.default_delete_attribute(object, name)
    }

    fn default_delete_attribute(&mut self, object: Value, name: &str) -> Result<Value> {
        if let Some(class) = self.instance_class(object)
            && let Some(descriptor) = self.class_attribute(class, name)?
        {
            if let Ok(Object::Property { delete, .. }) = self.heap.get(descriptor) {
                let delete = *delete;
                if delete == Value::None {
                    return Err(error("AttributeError: property has no deleter"));
                }
                self.call(delete, &[object])?;
                return Ok(Value::None);
            }
            if self.data_descriptor(descriptor)? {
                if self
                    .call_special(descriptor, "__delete__", &[object])?
                    .is_none()
                {
                    return Err(error("AttributeError: descriptor has no deleter"));
                }
                return Ok(Value::None);
            }
        }
        if self.exception_value_kind(object).is_some()
            && matches!(
                name,
                "args" | "__cause__" | "__context__" | "__traceback__" | "__suppress_context__"
            )
        {
            return Err(Error::Type("exception attribute cannot be deleted"));
        }
        let mut guard = self.heap.get_mut(object)?;
        let removed = match &mut *guard {
            Object::Instance { attributes, .. } => attributes.remove(name),
            Object::Exception(exception) => exception.attributes.remove(name),
            Object::Class { namespace, .. } => {
                let namespace = *namespace;
                drop(guard);
                self.attribute_protocols = true;
                let mut env = self.heap.get_mut(namespace)?;
                let Object::Environment(env) = &mut *env else {
                    unreachable!()
                };
                env.values.remove(name)
            }
            _ => None,
        };
        removed.ok_or_else(|| error(format!("AttributeError: {name}")))?;
        Ok(Value::None)
    }

    pub(crate) fn error_is(&self, err: &Error, kind: &str) -> bool {
        if self
            .pending_exception
            .is_some_and(|value| self.is_exception_instance(value, kind))
        {
            return true;
        }
        matches!(err,Error::Runtime(message) if message==kind || message.starts_with(&format!("{kind}:")))
    }

    pub(crate) fn attribute_builtin(
        &mut self,
        name: &str,
        args: &[Value],
    ) -> Result<Option<Value>> {
        let value = match name {
            "getattr" | "hasattr" => {
                arity(args, 2, if name == "getattr" { 3 } else { 2 })?;
                let text = self
                    .heap
                    .as_str(&args[1])
                    .ok_or(Error::Type("attribute name must be a string"))?
                    .to_owned();
                match self.attribute(args[0], &text) {
                    Ok(value) => {
                        if name == "hasattr" {
                            Value::Bool(true)
                        } else {
                            value
                        }
                    }
                    Err(err)
                        if self.error_is(&err, "AttributeError")
                            && (name == "hasattr" || args.len() == 3) =>
                    {
                        self.pending_exception = None;
                        if name == "hasattr" {
                            Value::Bool(false)
                        } else {
                            args[2]
                        }
                    }
                    Err(err) => return Err(err),
                }
            }
            "setattr" | "object.__setattr__" => {
                arity(args, 3, 3)?;
                let text = self
                    .heap
                    .as_str(&args[1])
                    .ok_or(Error::Type("attribute name must be a string"))?
                    .to_owned();
                if name == "setattr" {
                    self.protocol_set_attribute(args[0], &crate::Attribute::from(text), args[2])?
                } else {
                    self.default_set_attribute(args[0], &text, args[2])?
                }
            }
            "delattr" | "object.__delattr__" => {
                arity(args, 2, 2)?;
                let text = self
                    .heap
                    .as_str(&args[1])
                    .ok_or(Error::Type("attribute name must be a string"))?
                    .to_owned();
                if name == "delattr" {
                    self.delete_attribute(args[0], &text)?
                } else {
                    self.default_delete_attribute(args[0], &text)?
                }
            }
            "object.__new__" => {
                arity(args, 1, usize::MAX)?;
                let Object::Class { shape, .. } = self.heap.get(args[0])? else {
                    return Err(Error::Type("object.__new__ requires a class"));
                };
                self.heap.allocate(Object::Instance {
                    class: args[0],
                    attributes: crate::Attributes::new(shape.clone()),
                })
            }
            "object.__getattribute__" => {
                arity(args, 2, 2)?;
                let text = self
                    .heap
                    .as_str(&args[1])
                    .ok_or(Error::Type("attribute name must be a string"))?
                    .to_owned();
                self.default_get_attribute(args[0], &text, None)?
            }
            "staticmethod" | "classmethod" => {
                arity(args, 1, 1)?;
                self.heap.allocate(if name == "staticmethod" {
                    Object::StaticMethod(args[0])
                } else {
                    Object::ClassMethod(args[0])
                })
            }
            "property" => {
                arity(args, 0, 4)?;
                self.attribute_protocols = true;
                self.heap.allocate(Object::Property {
                    get: args.first().copied().unwrap_or(Value::None),
                    set: args.get(1).copied().unwrap_or(Value::None),
                    delete: args.get(2).copied().unwrap_or(Value::None),
                    doc: args.get(3).copied().unwrap_or(Value::None),
                })
            }
            _ => return Ok(None),
        };
        Ok(Some(value))
    }
}

impl Vm {
    pub(super) fn class_mro(&self, class: Value) -> Result<Vec<Value>> {
        self.class_mro_at(class, &mut vec![])
    }

    fn class_mro_at(&self, class: Value, path: &mut Vec<Value>) -> Result<Vec<Value>> {
        if path.len() > 128 || path.contains(&class) {
            return Err(error("TypeError: cyclic class hierarchy"));
        }
        let bases = match self.heap.get(class)? {
            Object::Class { bases, .. } => {
                if bases.is_empty() {
                    self.modules
                        .get("type.object")
                        .copied()
                        .into_iter()
                        .collect()
                } else {
                    bases.clone()
                }
            }
            Object::Native(name) if types::builtin_type_name(name) => {
                let parent = exception_parent(name).or_else(|| {
                    if name == "object" {
                        None
                    } else if name == "bool" {
                        Some("int")
                    } else {
                        Some("object")
                    }
                });
                parent
                    .and_then(|parent| self.modules.get(&format!("type.{parent}")).copied())
                    .into_iter()
                    .collect()
            }
            _ => return Err(Error::Type("expected class")),
        };
        path.push(class);
        let mut sequences = vec![];
        for base in &bases {
            sequences.push(self.class_mro_at(*base, path)?);
        }
        path.pop();
        sequences.push(bases);
        let mut result = vec![class];
        loop {
            sequences.retain(|s| !s.is_empty());
            if sequences.is_empty() {
                return Ok(result);
            }
            let candidate = sequences
                .iter()
                .map(|s| s[0])
                .find(|candidate| !sequences.iter().any(|s| s[1..].contains(candidate)))
                .ok_or_else(|| error("TypeError: inconsistent method resolution order"))?;
            result.push(candidate);
            for sequence in &mut sequences {
                if sequence[0] == candidate {
                    sequence.remove(0);
                }
            }
        }
    }

    pub(super) fn super_attribute(
        &mut self,
        anchor: Value,
        receiver: Value,
        name: &str,
    ) -> Result<Value> {
        let actual = if matches!(self.heap.get(receiver), Ok(Object::Class { .. })) {
            receiver
        } else {
            self.instance_class(receiver)
                .ok_or(Error::Type("super requires an instance or subclass"))?
        };
        let mro = self.class_mro(actual)?;
        let start = mro
            .iter()
            .position(|class| *class == anchor)
            .ok_or(Error::Type("super receiver is not a subtype"))?
            + 1;
        for class in &mro[start..] {
            let value = match self.heap.get(*class)? {
                Object::Class { namespace, .. } => self.env(*namespace)?.values.get(name).copied(),
                Object::Native(kind) if is_exception(kind) && name == "__init__" => Some(
                    self.heap
                        .allocate(Object::Native("BaseException.__init__".into())),
                ),
                _ => None,
            };
            if let Some(value) = value {
                if matches!(self.heap.get(value),Ok(Object::Native(kind)) if kind=="BaseException.__init__")
                {
                    return Ok(self.heap.allocate(Object::Method(value, receiver)));
                }
                return self.descriptor_get(value, Some(receiver), actual);
            }
        }
        Err(error(format!("AttributeError: super.{name}")))
    }
}
