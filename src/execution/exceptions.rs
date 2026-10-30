//! Python exception identity, builtin inheritance, and explicit/implicit chaining.
use super::*;

pub(super) fn exception_parent(name: &str) -> Option<&'static str> {
    Some(match name {
        "BaseException" => "object",
        "Exception" => "BaseException",
        "UnboundLocalError" => "NameError",
        "ZeroDivisionError" | "OverflowError" => "ArithmeticError",
        "IndexError" | "KeyError" => "LookupError",
        "NotImplementedError" | "RecursionError" => "RuntimeError",
        "ValueError" | "TypeError" | "RuntimeError" | "StopIteration" | "NameError"
        | "AttributeError" | "AssertionError" | "ArithmeticError" | "LookupError" => "Exception",
        _ => return None,
    })
}

pub(super) fn exception_subclass(mut actual: &str, expected: &str) -> bool {
    loop {
        if actual == expected {
            return true;
        }
        let Some(parent) = exception_parent(actual) else {
            return false;
        };
        actual = parent;
    }
}

impl Vm {
    pub(super) fn exception_class(&self, class: Value) -> bool {
        let mut pending = vec![class];
        let mut seen = std::collections::HashSet::new();
        while let Some(class) = pending.pop() {
            if let Value::Ref(id) = class
                && !seen.insert(id)
            {
                continue;
            }
            match self.heap.get(class) {
                Ok(Object::Native(name)) if is_exception(name) => return true,
                Ok(Object::Class { bases, .. }) => pending.extend(bases),
                _ => {}
            }
        }
        false
    }

    pub(super) fn exception_value_kind(&self, value: Value) -> Option<String> {
        match self.heap.get(value).ok()? {
            Object::Exception(exception) => Some(exception.kind.clone()),
            Object::Instance { class, .. } if self.exception_class(*class) => {
                match self.heap.get(*class).ok()? {
                    Object::Class { name, .. } => Some(name.clone()),
                    _ => None,
                }
            }
            _ => None,
        }
    }

    pub(super) fn is_exception_instance(&self, value: Value, kind: &str) -> bool {
        match self.heap.get(value) {
            Ok(Object::Exception(exception)) => exception_subclass(&exception.kind, kind),
            Ok(Object::Instance { class, .. }) => {
                let mut pending = vec![*class];
                let mut seen = std::collections::HashSet::new();
                while let Some(class) = pending.pop() {
                    if let Value::Ref(id) = class
                        && !seen.insert(id)
                    {
                        continue;
                    }
                    match self.heap.get(class) {
                        Ok(Object::Native(actual)) if exception_subclass(actual, kind) => {
                            return true;
                        }
                        Ok(Object::Class { bases, .. }) => pending.extend(bases),
                        _ => {}
                    }
                }
                false
            }
            _ => false,
        }
    }

    pub(super) fn exception_arguments(&self, value: Value) -> Result<Vec<Value>> {
        match self.heap.get(value)? {
            Object::Exception(exception) => match self.heap.get(exception.arguments)? {
                Object::Tuple(items) => Ok(items.clone()),
                _ => Err(Error::InvalidIr("exception arguments")),
            },
            Object::Instance { attributes, .. } => match attributes.get("args") {
                Some(value) => match self.heap.get(*value)? {
                    Object::Tuple(items) => Ok(items.clone()),
                    _ => Err(Error::Type("exception args must be tuple")),
                },
                None => Ok(vec![]),
            },
            _ => Err(Error::Type("not an exception")),
        }
    }

    pub(super) fn exception_attribute(&self, object: Value, name: &str) -> Result<Option<Value>> {
        let Object::Exception(exception) = self.heap.get(object)? else {
            return Ok(None);
        };
        let ExceptionData {
            arguments,
            attributes,
            cause,
            context,
            suppress_context,
            traceback,
            ..
        } = &**exception;
        Ok(match name {
            "__traceback__" => Some(traceback.unwrap_or(Value::None)),
            "args" => Some(*arguments),
            "__cause__" => Some(cause.unwrap_or(Value::None)),
            "__context__" => Some(context.unwrap_or(Value::None)),
            "__suppress_context__" => Some(Value::Bool(*suppress_context)),
            "value" if self.is_exception_instance(object, "StopIteration") => {
                Some(match self.heap.get(*arguments)? {
                    Object::Tuple(items) => items.first().copied().unwrap_or(Value::None),
                    _ => return Err(Error::InvalidIr("exception arguments")),
                })
            }
            _ => attributes.get(name).copied(),
        })
    }

    pub(super) fn set_exception_attribute(
        &mut self,
        object: Value,
        name: &str,
        value: Value,
    ) -> Result<bool> {
        let builtin = matches!(self.heap.get(object), Ok(Object::Exception(_)));
        let custom =
            self.instance_class(object).is_some() && self.exception_value_kind(object).is_some();
        if !builtin && !custom {
            return Ok(false);
        }
        if name == "args" {
            let tuple = if matches!(self.heap.get(value), Ok(Object::Tuple(_))) {
                value
            } else {
                let values = self.collect_iter(value)?;
                self.heap.allocate(Object::Tuple(values))
            };
            if builtin {
                if let Object::Exception(exception) = &mut *self.heap.get_mut(object)? {
                    exception.arguments = tuple;
                }
            } else {
                self.raw_set_attribute(object, &crate::Attribute::from("args".to_owned()), tuple)?;
            }
            return Ok(true);
        }
        if matches!(name, "__cause__" | "__context__")
            && value != Value::None
            && self.exception_value_kind(value).is_none()
        {
            return Err(Error::Type("exception chain must contain exceptions"));
        }
        if name == "__traceback__"
            && value != Value::None
            && !matches!(self.heap.get(value), Ok(Object::Traceback { .. }))
        {
            return Err(Error::Type("__traceback__ must be traceback or None"));
        }
        if custom {
            if name == "__suppress_context__" && !matches!(value, Value::Bool(_)) {
                return Err(Error::Type("__suppress_context__ must be bool"));
            }
            if !matches!(
                name,
                "__cause__" | "__context__" | "__suppress_context__" | "__traceback__"
            ) {
                return Ok(false);
            }
            self.raw_set_attribute(object, &crate::Attribute::from(name.to_owned()), value)?;
            if name == "__cause__" {
                self.raw_set_attribute(
                    object,
                    &crate::Attribute::from("__suppress_context__".to_owned()),
                    Value::Bool(true),
                )?;
            }
            return Ok(true);
        }
        let mut guard = self.heap.get_mut(object)?;
        let Object::Exception(exception) = &mut *guard else {
            unreachable!()
        };
        let ExceptionData {
            attributes,
            cause,
            context,
            suppress_context,
            traceback,
            ..
        } = &mut **exception;
        match name {
            "__traceback__" => *traceback = (value != Value::None).then_some(value),
            "__cause__" => {
                *cause = (value != Value::None).then_some(value);
                *suppress_context = true;
            }
            "__context__" => *context = (value != Value::None).then_some(value),
            "__suppress_context__" => {
                let Value::Bool(v) = value else {
                    return Err(Error::Type("__suppress_context__ must be bool"));
                };
                *suppress_context = v;
            }
            "__class__" => return Err(Error::Type("exception class cannot be reassigned")),
            _ => {
                attributes.insert(name.to_owned(), value);
            }
        }
        Ok(true)
    }

    pub(super) fn attach_context(&mut self, value: Value) -> Result<()> {
        if let Some(context) = self.active_exceptions.last().copied()
            && context != value
        {
            let mut cursor = context;
            let mut seen = std::collections::HashSet::new();
            while let Value::Ref(id) = cursor {
                if !seen.insert(id) {
                    break;
                }
                let next = match self.heap.get(cursor)? {
                    Object::Exception(exception) => exception.context,
                    Object::Instance { attributes, .. } => attributes
                        .get("__context__")
                        .copied()
                        .filter(|v| *v != Value::None),
                    _ => None,
                };
                if next == Some(value) {
                    self.set_chain(cursor, "__context__", Value::None)?;
                    break;
                }
                let Some(next) = next else {
                    break;
                };
                cursor = next;
            }
            self.set_chain(value, "__context__", context)?;
        }
        Ok(())
    }

    fn set_chain(&mut self, value: Value, name: &str, chain: Value) -> Result<()> {
        if matches!(self.heap.get(value), Ok(Object::Exception(_))) {
            self.set_exception_attribute(value, name, chain)?;
        } else {
            self.raw_set_attribute(value, &crate::Attribute::from(name.to_owned()), chain)?;
        }
        Ok(())
    }

    pub(super) fn normalize_exception(&mut self, value: Value) -> Result<Value> {
        if self.exception_class(value) {
            let exception = self.call(value, &[])?;
            if self.exception_value_kind(exception).is_some() {
                return Ok(exception);
            }
        }
        if self.exception_value_kind(value).is_some() {
            Ok(value)
        } else {
            Err(Error::Type("exceptions must derive from BaseException"))
        }
    }

    pub(super) fn raise_python_exception(
        &mut self,
        values: &[Value],
        propagating: bool,
    ) -> Result<Value> {
        let value = values
            .first()
            .copied()
            .or_else(|| self.active_exceptions.last().copied())
            .ok_or_else(|| error("RuntimeError: No active exception to reraise"))?;
        let value = self.normalize_exception(value)?;
        self.protocol_roots(&[value], |vm| {
            if values.len() > 1 {
                let cause = if values[1] == Value::None {
                    Value::None
                } else {
                    vm.normalize_exception(values[1])?
                };
                vm.set_chain(value, "__cause__", cause)?;
                if vm.instance_class(value).is_some() {
                    vm.raw_set_attribute(
                        value,
                        &crate::Attribute::from("__suppress_context__".to_owned()),
                        Value::Bool(true),
                    )?;
                }
            }
            if !propagating && !values.is_empty() {
                vm.attach_context(value)?;
            }
            vm.record_traceback(value, !propagating && !values.is_empty())?;
            let kind = vm.exception_value_kind(value).unwrap();
            // Raising must never invoke __str__/__repr__; formatting is a host diagnostic only.
            let arguments = vm.exception_arguments(value).unwrap_or_default();
            let text = match arguments.as_slice() {
                [] => String::new(),
                [argument] => vm.repr(*argument, false).unwrap_or_default(),
                _ => {
                    let tuple = vm.heap.allocate(Object::Tuple(arguments));
                    vm.repr(tuple, false).unwrap_or_default()
                }
            };
            vm.pending_exception = Some(value);
            Err(error(if text.is_empty() {
                kind
            } else {
                format!("{kind}: {text}")
            }))
        })
    }
}

impl Vm {
    pub(super) fn exception_type(&mut self, value: Value) -> Result<Value> {
        if let Some(class) = self.instance_class(value) {
            return Ok(class);
        }
        let kind = self
            .exception_value_kind(value)
            .ok_or(Error::Type("not an exception"))?;
        Ok(self.builtin_type(&kind))
    }

    pub(super) fn exception_traceback(&self, value: Value) -> Option<Value> {
        match self.heap.get(value).ok()? {
            Object::Exception(exception) => exception.traceback,
            Object::Instance { attributes, .. } => attributes
                .get("__traceback__")
                .copied()
                .filter(|v| *v != Value::None),
            _ => None,
        }
    }

    pub(crate) fn record_traceback(&mut self, value: Value, explicit: bool) -> Result<()> {
        // Tracebacks retain raise/unwind identity. Source/frame inspection is deliberately unsupported.
        let Some((function, depth)) = self.traceback_origin() else {
            return Ok(());
        };
        let old = self.exception_traceback(value);
        if !explicit && old.is_some_and(|v|matches!(self.heap.get(v),Ok(Object::Traceback{function:f,depth:d,..}) if *f==function && *d==depth)) {return Ok(());}
        let traceback = self.heap.allocate(Object::Traceback {
            next: old,
            function,
            depth,
        });
        self.set_chain(value, "__traceback__", traceback)
    }
}
