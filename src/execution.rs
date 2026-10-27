//! Lexical environments and Python object operations shared by execution backends.

use crate::{
    Error, Heap, Key, Object, Result, Value, Vm,
    ir::{CoreOp, Function},
    runtime,
};
use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
    rc::Rc,
};

#[derive(Debug, Clone)]
pub struct Environment {
    pub parent: Option<Value>,
    pub values: BTreeMap<String, Value>,
    pub locals: BTreeSet<String>,
    pub globals: BTreeSet<String>,
    pub nonlocals: BTreeSet<String>,
    pub class_scope: bool,
}

#[derive(Debug, Clone)]
pub struct Closure {
    pub function: Rc<RefCell<Function>>,
    pub environment: Value,
    pub defaults: Vec<Value>,
}

fn error(message: impl Into<String>) -> Error {
    Error::Runtime(message.into())
}

pub(crate) fn children(object: &Object) -> Vec<Value> {
    match object {
        Object::List(values) | Object::Tuple(values) | Object::Set(values) => values.clone(),
        Object::Enumerate { iterator, .. } => vec![*iterator],
        Object::Zip { iterators, .. } => iterators.clone(),
        Object::Dict(values) => values.iter().map(|(_, value)| *value).collect(),
        Object::Environment(env) => env.values.values().copied().chain(env.parent).collect(),
        Object::Function(closure) => std::iter::once(closure.environment)
            .chain(closure.defaults.iter().copied())
            .collect(),
        Object::Bound(value, _)
        | Object::Iterator { source: value, .. }
        | Object::View { source: value, .. } => vec![*value],
        Object::Method(function, receiver) => vec![*function, *receiver],
        Object::Class {
            namespace, bases, ..
        } => std::iter::once(*namespace)
            .chain(bases.iter().copied())
            .collect(),
        Object::Instance { class, attributes } => std::iter::once(*class)
            .chain(attributes.values().copied())
            .collect(),
        Object::Slice { start, stop, step } => vec![*start, *stop, *step],
        _ => vec![],
    }
}

impl Vm {
    pub(crate) fn ensure_globals(&mut self) -> Value {
        if let Some(env) = self.globals {
            return env;
        }
        let mut values = BTreeMap::new();
        for name in [
            "len",
            "range",
            "list",
            "tuple",
            "dict",
            "set",
            "print",
            "int",
            "float",
            "str",
            "repr",
            "bool",
            "abs",
            "sum",
            "min",
            "max",
            "enumerate",
            "zip",
            "iter",
            "next",
            "isinstance",
            "object",
            "sorted",
            "reversed",
            "all",
            "any",
        ] {
            values.insert(name.into(), self.heap.allocate(Object::Native(name.into())));
        }
        values.insert(
            "__name__".into(),
            self.heap.allocate(Object::String("__main__".into())),
        );
        let env = self.heap.allocate(Object::Environment(Environment {
            parent: None,
            values,
            locals: BTreeSet::new(),
            globals: BTreeSet::new(),
            nonlocals: BTreeSet::new(),
            class_scope: false,
        }));
        self.globals = Some(env);
        env
    }

    pub fn get_global(&self, name: &str) -> Result<Value> {
        self.lookup(
            self.globals
                .ok_or_else(|| error("module has not executed"))?,
            name,
        )
    }

    pub fn function_ir(&self, callable: Value) -> Result<Function> {
        match self.heap.get(callable)? {
            Object::Function(closure) => Ok(closure.function.borrow().clone()),
            _ => Err(Error::Type("expected Python function")),
        }
    }

    fn env(&self, value: Value) -> Result<&Environment> {
        match self.heap.get(value)? {
            Object::Environment(env) => Ok(env),
            _ => Err(Error::InvalidIr("expected environment")),
        }
    }

    fn lookup(&self, mut environment: Value, name: &str) -> Result<Value> {
        loop {
            let env = self.env(environment)?;
            if env.globals.contains(name) {
                environment = self.globals.ok_or(Error::InvalidIr("globals"))?;
                continue;
            }
            if let Some(value) = env.values.get(name) {
                return Ok(*value);
            }
            if env.locals.contains(name) && !env.class_scope {
                return Err(error(format!("UnboundLocalError: {name}")));
            }
            if let Some(parent) = env.parent {
                environment = parent;
            } else {
                return Err(error(format!("NameError: {name}")));
            }
        }
    }

    fn store(&mut self, environment: Value, name: &str, value: Value) -> Result<()> {
        let env = self.env(environment)?;
        let target = if env.globals.contains(name) {
            self.globals.unwrap()
        } else if env.nonlocals.contains(name) {
            let mut parent = env.parent;
            loop {
                let candidate =
                    parent.ok_or_else(|| error(format!("no binding for nonlocal {name}")))?;
                let env = self.env(candidate)?;
                if env.parent.is_some() && env.locals.contains(name) && !env.class_scope {
                    break candidate;
                }
                parent = env.parent;
            }
        } else {
            environment
        };
        let mut guard = self.heap.get_mut(target)?;
        let Object::Environment(env) = &mut *guard else {
            unreachable!()
        };
        env.values.insert(name.into(), value);
        Ok(())
    }

    pub(crate) fn invoke_function(
        &mut self,
        function: Rc<RefCell<Function>>,
        parent: Value,
        defaults: &[Value],
        arguments: &[Value],
        keywords: &[(String, Value)],
    ) -> Result<Value> {
        let template = function.borrow();
        if template.parameter_names.is_empty() && template.vararg.is_none() && !template.module {
            let expected = template.parameters(template.entry()).len();
            if expected != arguments.len() {
                return Err(Error::Arguments {
                    expected,
                    actual: arguments.len(),
                });
            }
        }
        for &value in arguments.iter().chain(defaults) {
            if matches!(value, Value::Ref(_)) {
                self.heap.get(value)?;
            }
        }
        let environment = if template.module {
            parent
        } else {
            self.heap.allocate(Object::Environment(Environment {
                parent: Some(parent),
                values: BTreeMap::new(),
                locals: template.local_names.iter().cloned().collect(),
                globals: template.global_names.iter().cloned().collect(),
                nonlocals: template.nonlocal_names.iter().cloned().collect(),
                class_scope: false,
            }))
        };
        let names = &template.parameter_names;
        if !names.is_empty()
            || template.vararg.is_some()
            || template.kwarg.is_some()
            || !template.keyword_only.is_empty()
        {
            if arguments.len() > names.len() && template.vararg.is_none() {
                return Err(Error::Arguments {
                    expected: names.len(),
                    actual: arguments.len(),
                });
            }
            let mut bound = BTreeMap::new();
            for (name, &value) in names.iter().zip(arguments) {
                bound.insert(name.clone(), value);
            }
            let extra = self.heap.allocate(Object::Dict(vec![]));
            for (name, value) in keywords {
                if (names.contains(name) && !names[..template.positional_only].contains(name))
                    || template.keyword_only.contains(name)
                {
                    if bound.insert(name.clone(), *value).is_some() {
                        return Err(error(format!("TypeError: multiple values for {name}")));
                    }
                } else if template.kwarg.is_some() {
                    let key = self.heap.allocate(Object::String(name.clone()));
                    self.heap.set_item(extra, key, *value)?;
                } else {
                    return Err(error(format!("TypeError: unexpected keyword {name}")));
                }
            }
            // Positional defaults precede keyword-only defaults; None is a valid default.
            let positional_defaults = defaults.len().saturating_sub(template.keyword_only.len());
            for (index, name) in names.iter().enumerate() {
                if !bound.contains_key(name) {
                    if index + positional_defaults >= names.len() {
                        bound.insert(
                            name.clone(),
                            defaults[index + positional_defaults - names.len()],
                        );
                    } else {
                        return Err(error(format!("TypeError: missing argument {name}")));
                    }
                }
            }
            for (index, name) in template.keyword_only.iter().enumerate() {
                if !bound.contains_key(name) {
                    let value = defaults
                        .get(positional_defaults + index)
                        .copied()
                        .ok_or_else(|| error(format!("TypeError: missing argument {name}")))?;
                    if self.is_done(value) {
                        return Err(error(format!("TypeError: missing argument {name}")));
                    }
                    bound.insert(name.clone(), value);
                }
            }
            if let Some(name) = &template.vararg {
                bound.insert(
                    name.clone(),
                    self.heap.allocate(Object::Tuple(
                        arguments.get(names.len()..).unwrap_or(&[]).to_vec(),
                    )),
                );
            }
            if let Some(name) = &template.kwarg {
                bound.insert(name.clone(), extra);
            }
            for (name, value) in bound {
                self.store(environment, &name, value)?;
            }
        } else if !keywords.is_empty() {
            return Err(error("TypeError: unexpected keyword argument"));
        }
        drop(template);
        self.execute_frame(function, environment, arguments)
    }

    pub fn call(&mut self, callable: Value, arguments: &[Value]) -> Result<Value> {
        let roots = self.transient_roots.len();
        self.transient_roots.push(callable);
        self.transient_roots.extend_from_slice(arguments);
        let result = self.call_keywords(callable, arguments, &[]);
        self.transient_roots.truncate(roots);
        result
    }

    fn call_keywords(
        &mut self,
        callable: Value,
        arguments: &[Value],
        keywords: &[(String, Value)],
    ) -> Result<Value> {
        let object = self.heap.get(callable)?.clone();
        match object {
            Object::Function(closure) => self.invoke_function(
                closure.function,
                closure.environment,
                &closure.defaults,
                arguments,
                keywords,
            ),
            Object::Native(name) => self.builtin(&name, arguments, keywords),
            Object::Bound(receiver, name) => {
                if !keywords.is_empty() {
                    return Err(error("method keyword arguments unsupported"));
                }
                self.method(receiver, &name, arguments)
            }
            Object::Method(function, receiver) => {
                let args = std::iter::once(receiver)
                    .chain(arguments.iter().copied())
                    .collect::<Vec<_>>();
                self.call_keywords(function, &args, keywords)
            }
            Object::Class { .. } => {
                let instance = self.heap.allocate(Object::Instance {
                    class: callable,
                    attributes: BTreeMap::new(),
                });
                if let Some(init) = self.class_attribute(callable, "__init__")? {
                    let args = std::iter::once(instance)
                        .chain(arguments.iter().copied())
                        .collect::<Vec<_>>();
                    if self.call_keywords(init, &args, keywords)? != Value::None {
                        return Err(error("TypeError: __init__ must return None"));
                    }
                } else if !arguments.is_empty() || !keywords.is_empty() {
                    return Err(error("TypeError: class takes no arguments"));
                }
                Ok(instance)
            }
            _ => Err(Error::Type("object is not callable")),
        }
    }

    pub(crate) fn operation(&mut self, operation: &CoreOp, values: &[Value]) -> Result<Value> {
        let env = *self
            .environments
            .last()
            .ok_or(Error::InvalidIr("missing frame environment"))?;
        Ok(match operation {
            CoreOp::Missing => self.heap.allocate(Object::Done),
            CoreOp::UnpackStar(before, after) => {
                let items = self.collect_iter(values[0])?;
                if items.len() < before + after {
                    return Err(error("ValueError: not enough values to unpack"));
                }
                let middle = self
                    .heap
                    .allocate(Object::List(items[*before..items.len() - after].to_vec()));
                let result = items[..*before]
                    .iter()
                    .copied()
                    .chain(std::iter::once(middle))
                    .chain(items[items.len() - after..].iter().copied())
                    .collect();
                self.heap.allocate(Object::Tuple(result))
            }
            CoreOp::LoadName(name) => self.lookup(env, name)?,
            CoreOp::StoreName(name) => {
                self.store(env, name, values[0])?;
                Value::None
            }
            CoreOp::Binary(op) => binary(&self.heap, op, values[0], values[1])?,
            CoreOp::Inplace(op) => {
                if op == "+" && matches!(self.heap.get(values[0]), Ok(Object::List(_))) {
                    self.method(values[0], "extend", &values[1..])?;
                    values[0]
                } else if op == "*" && matches!(self.heap.get(values[0]), Ok(Object::List(_))) {
                    let replacement = repeat(&self.heap, values[0], integer(values[1])?)?;
                    let object = self.heap.get(replacement)?.clone();
                    *self.heap.get_mut(values[0])? = object;
                    values[0]
                } else if op == "+" {
                    runtime::add(&mut self.heap, values[0], values[1])?
                } else if op == "-" {
                    runtime::subtract(values[0], values[1])?
                } else {
                    binary(&self.heap, op, values[0], values[1])?
                }
            }
            CoreOp::Compare(op) => Value::Bool(compare(&self.heap, op, values[0], values[1])?),
            CoreOp::Unary(op) => match op.as_str() {
                "not" => Value::Bool(!runtime::truth(&self.heap, values[0])?),
                "-" => match values[0] {
                    Value::Int(n) => Value::Int(n.checked_neg().ok_or(Error::IntegerOverflow)?),
                    other => Value::Float(-number(other)?),
                },
                "+" => {
                    number(values[0])?;
                    values[0]
                }
                "~" => Value::Int(!integer(values[0])?),
                _ => return Err(Error::Unsupported("unary operator")),
            },
            CoreOp::Call(names) => {
                let mut positional = vec![];
                let mut keywords = vec![];
                for (name, &value) in names.iter().zip(&values[1..]) {
                    match name.as_deref() {
                        None => positional.push(value),
                        Some("*") => positional.extend(self.collect_iter(value)?),
                        Some("**") => {
                            let Object::Dict(items) = self.heap.get(value)? else {
                                return Err(Error::Type("** requires dictionary"));
                            };
                            for (key, value) in items {
                                let Key::String(name) = key else {
                                    return Err(Error::Type("keywords must be strings"));
                                };
                                if keywords.iter().any(|(candidate, _)| candidate == name) {
                                    return Err(error("duplicate keyword"));
                                }
                                keywords.push((name.clone(), *value));
                            }
                        }
                        Some(name) => {
                            if keywords.iter().any(|(candidate, _)| candidate == name) {
                                return Err(error("duplicate keyword"));
                            }
                            keywords.push((name.into(), value));
                        }
                    }
                }
                self.call_keywords(values[0], &positional, &keywords)?
            }
            CoreOp::Function(function) => {
                let capture = if self.env(env)?.class_scope {
                    self.env(env)?.parent.unwrap()
                } else {
                    env
                };
                self.heap.allocate(Object::Function(Closure {
                    function: Rc::new(RefCell::new((**function).clone())),
                    environment: capture,
                    defaults: values.to_vec(),
                }))
            }
            CoreOp::Class(name, body) => {
                for &base in values {
                    if !matches!(self.heap.get(base)?, Object::Class { .. }) {
                        return Err(Error::Type("base must be class"));
                    }
                }
                if values.len() > 1 {
                    return Err(Error::Unsupported("multiple inheritance"));
                }
                let namespace = self.heap.allocate(Object::Environment(Environment {
                    parent: Some(env),
                    values: BTreeMap::new(),
                    locals: BTreeSet::new(),
                    globals: BTreeSet::new(),
                    nonlocals: BTreeSet::new(),
                    class_scope: true,
                }));
                self.execute_frame(Rc::new(RefCell::new((**body).clone())), namespace, &[])?;
                self.heap.allocate(Object::Class {
                    name: name.clone(),
                    namespace,
                    bases: values.to_vec(),
                })
            }
            CoreOp::GetAttr(name) => self.attribute(values[0], name)?,
            CoreOp::SetAttr(name) => {
                let mut guard = self.heap.get_mut(values[0])?;
                match &mut *guard {
                    Object::Instance { attributes, .. } => {
                        attributes.insert(name.clone(), values[1]);
                    }
                    Object::Class { namespace, .. } => {
                        let namespace = *namespace;
                        drop(guard);
                        self.store(namespace, name, values[1])?;
                    }
                    _ => return Err(Error::Type("object has no writable attributes")),
                }
                Value::None
            }
            CoreOp::Iter => self.iter(values[0])?,
            CoreOp::Next => self.next(values[0])?,
            CoreOp::IsDone => Value::Bool(self.is_done(values[0])),
            CoreOp::Unpack(count) => {
                let items = self.collect_iter(values[0])?;
                if items.len() != *count {
                    return Err(error("ValueError: unpacking length mismatch"));
                }
                self.heap.allocate(Object::Tuple(items))
            }
            CoreOp::UnpackItem(index) => {
                self.heap.get_item(values[0], Value::Int(*index as i64))?
            }
            CoreOp::Slice => self.heap.allocate(Object::Slice {
                start: values[0],
                stop: values[1],
                step: values[2],
            }),
            CoreOp::Set => {
                let set = self.heap.allocate(Object::Set(vec![]));
                for &value in values {
                    self.method(set, "add", &[value])?;
                }
                set
            }
            CoreOp::Format(spec, repr) => {
                let text = if spec.is_empty() {
                    self.repr(values[0], *repr)?
                } else if spec.starts_with('.') && spec.ends_with('f') {
                    let precision: usize = spec[1..spec.len() - 1]
                        .parse()
                        .map_err(|_| error("invalid format spec"))?;
                    if precision > 1000 {
                        return Err(error("format precision too large"));
                    }
                    format!("{:.*}", precision, number(values[0])?)
                } else {
                    return Err(Error::Unsupported("format specification"));
                };
                self.heap.allocate(Object::String(text))
            }
            CoreOp::Join => {
                let mut text = String::new();
                for &value in values {
                    let Object::String(part) = self.heap.get(value)? else {
                        return Err(Error::Type("join requires strings"));
                    };
                    text.push_str(part);
                }
                self.heap.allocate(Object::String(text))
            }
            CoreOp::Assert => {
                if !runtime::truth(&self.heap, values[0])? {
                    return Err(error("AssertionError"));
                }
                Value::None
            }
            CoreOp::Namespace => env,
        })
    }

    fn is_done(&self, value: Value) -> bool {
        matches!(value, Value::Ref(_)) && matches!(self.heap.get(value), Ok(Object::Done))
    }

    fn class_attribute(&self, class: Value, name: &str) -> Result<Option<Value>> {
        let Object::Class {
            namespace, bases, ..
        } = self.heap.get(class)?
        else {
            return Err(Error::Type("expected class"));
        };
        if let Some(value) = self.env(*namespace)?.values.get(name) {
            return Ok(Some(*value));
        }
        for &base in bases {
            if let Some(value) = self.class_attribute(base, name)? {
                return Ok(Some(value));
            }
        }
        Ok(None)
    }

    fn attribute(&mut self, object: Value, name: &str) -> Result<Value> {
        match self.heap.get(object)? {
            Object::Instance { class, attributes } => {
                if let Some(value) = attributes.get(name) {
                    return Ok(*value);
                }
                if let Some(value) = self.class_attribute(*class, name)? {
                    return Ok(if matches!(self.heap.get(value), Ok(Object::Function(_))) {
                        self.heap.allocate(Object::Method(value, object))
                    } else {
                        value
                    });
                }
            }
            Object::Class { .. } => {
                if let Some(value) = self.class_attribute(object, name)? {
                    return Ok(value);
                }
            }
            Object::List(_)
                if [
                    "append", "extend", "pop", "reverse", "copy", "clear", "insert", "count",
                    "index", "remove",
                ]
                .contains(&name) =>
            {
                return Ok(self.heap.allocate(Object::Bound(object, name.into())));
            }
            Object::Dict(_)
                if [
                    "values",
                    "keys",
                    "items",
                    "get",
                    "setdefault",
                    "update",
                    "pop",
                    "copy",
                    "clear",
                ]
                .contains(&name) =>
            {
                return Ok(self.heap.allocate(Object::Bound(object, name.into())));
            }
            Object::Set(_) if ["add", "discard", "remove", "copy", "clear"].contains(&name) => {
                return Ok(self.heap.allocate(Object::Bound(object, name.into())));
            }
            Object::String(_)
                if [
                    "join",
                    "split",
                    "upper",
                    "lower",
                    "strip",
                    "startswith",
                    "endswith",
                    "replace",
                ]
                .contains(&name) =>
            {
                return Ok(self.heap.allocate(Object::Bound(object, name.into())));
            }
            _ => {}
        }
        Err(error(format!("AttributeError: {name}")))
    }

    fn iter(&mut self, source: Value) -> Result<Value> {
        if matches!(
            self.heap.get(source)?,
            Object::Iterator { .. } | Object::Enumerate { .. } | Object::Zip { .. }
        ) {
            return Ok(source);
        }
        let size = self.heap.len(source)?;
        Ok(self.heap.allocate(Object::Iterator {
            source,
            index: 0,
            size,
        }))
    }

    fn next(&mut self, iterator: Value) -> Result<Value> {
        match self.heap.get(iterator)?.clone() {
            Object::Enumerate {
                iterator: inner,
                index,
            } => {
                let value = self.next(inner)?;
                if self.is_done(value) {
                    return Ok(value);
                }
                let next_index = index.checked_add(1).ok_or(Error::IntegerOverflow)?;
                *self.heap.get_mut(iterator)? = Object::Enumerate {
                    iterator: inner,
                    index: next_index,
                };
                return Ok(self
                    .heap
                    .allocate(Object::Tuple(vec![Value::Int(index), value])));
            }
            Object::Zip { iterators, done } => {
                if done || iterators.is_empty() {
                    return Ok(self.heap.allocate(Object::Done));
                }
                let mut row = vec![];
                for &inner in &iterators {
                    let value = self.next(inner)?;
                    if self.is_done(value) {
                        *self.heap.get_mut(iterator)? = Object::Zip {
                            iterators,
                            done: true,
                        };
                        return Ok(value);
                    }
                    row.push(value);
                }
                return Ok(self.heap.allocate(Object::Tuple(row)));
            }
            _ => {}
        }

        let Object::Iterator {
            source,
            index,
            size,
        } = self.heap.get(iterator)?.clone()
        else {
            return Err(Error::Type("next requires iterator"));
        };
        let len = self.heap.len(source)?;
        if matches!(
            self.heap.get(source)?,
            Object::Dict(_) | Object::Set(_) | Object::View { .. }
        ) && len != size
        {
            return Err(error(
                "RuntimeError: container changed size during iteration",
            ));
        }
        if index >= len {
            return Ok(self.heap.allocate(Object::Done));
        }
        let value = match self.heap.get(source)? {
            Object::Range { start, step, .. } => Value::Int(
                i64::try_from(i128::from(*start) + index as i128 * i128::from(*step))
                    .map_err(|_| Error::IntegerOverflow)?,
            ),
            Object::Dict(items) => key_value(&self.heap, &items[index].0),
            Object::Set(items) => items[index],
            Object::View { source, kind } => {
                let Object::Dict(items) = self.heap.get(*source)? else {
                    return Err(Error::Type("invalid dictionary view"));
                };
                let (key, value) = &items[index];
                match kind.as_str() {
                    "values" => *value,
                    "keys" => key_value(&self.heap, key),
                    _ => self
                        .heap
                        .allocate(Object::Tuple(vec![key_value(&self.heap, key), *value])),
                }
            }
            _ => self.heap.get_item(source, Value::Int(index as i64))?,
        };
        let mut guard = self.heap.get_mut(iterator)?;
        let Object::Iterator { index, .. } = &mut *guard else {
            unreachable!()
        };
        *index += 1;
        Ok(value)
    }

    fn collect_iter(&mut self, source: Value) -> Result<Vec<Value>> {
        let iterator = self.iter(source)?;
        let mut values = vec![];
        loop {
            let value = self.next(iterator)?;
            if self.is_done(value) {
                return Ok(values);
            }
            values.push(value);
        }
    }

    fn builtin(
        &mut self,
        name: &str,
        args: &[Value],
        keywords: &[(String, Value)],
    ) -> Result<Value> {
        if name != "print" && !keywords.is_empty() {
            return Err(Error::Unsupported("builtin keyword arguments"));
        }
        Ok(match name {
            "print" => {
                let mut sep = " ".to_owned();
                let mut end = "\n".to_owned();
                for (name, value) in keywords {
                    let Object::String(text) = self.heap.get(*value)? else {
                        return Err(Error::Type("print separator must be string"));
                    };
                    match name.as_str() {
                        "sep" => sep = text.clone(),
                        "end" => end = text.clone(),
                        _ => return Err(error("unexpected print keyword")),
                    }
                }
                let parts = args
                    .iter()
                    .map(|&value| self.repr(value, false))
                    .collect::<Result<Vec<_>>>()?;
                self.output.push_str(&parts.join(&sep));
                self.output.push_str(&end);
                Value::None
            }
            "len" => {
                arity(args, 1, 1)?;
                Value::Int(self.heap.len(args[0])? as i64)
            }
            "range" => {
                arity(args, 1, 3)?;
                let (start, stop, step) = match args {
                    [stop] => (0, integer(*stop)?, 1),
                    [start, stop] => (integer(*start)?, integer(*stop)?, 1),
                    [start, stop, step] => (integer(*start)?, integer(*stop)?, integer(*step)?),
                    _ => unreachable!(),
                };
                if step == 0 {
                    return Err(error("ValueError: range step cannot be zero"));
                }
                self.heap.allocate(Object::Range { start, stop, step })
            }
            "list" | "tuple" | "set" => {
                arity(args, 0, 1)?;
                let items = if args.is_empty() {
                    vec![]
                } else {
                    self.collect_iter(args[0])?
                };
                match name {
                    "list" => self.heap.allocate(Object::List(items)),
                    "tuple" => self.heap.allocate(Object::Tuple(items)),
                    _ => {
                        let set = self.heap.allocate(Object::Set(vec![]));
                        for value in items {
                            self.method(set, "add", &[value])?;
                        }
                        set
                    }
                }
            }
            "dict" => {
                arity(args, 0, 1)?;
                let dict = self.heap.allocate(Object::Dict(vec![]));
                if let Some(&source) = args.first() {
                    self.method(dict, "update", &[source])?;
                }
                dict
            }
            "iter" => {
                arity(args, 1, 1)?;
                self.iter(args[0])?
            }
            "next" => {
                arity(args, 1, 2)?;
                let value = self.next(args[0])?;
                if self.is_done(value) {
                    if args.len() == 2 {
                        args[1]
                    } else {
                        return Err(error("StopIteration"));
                    }
                } else {
                    value
                }
            }
            "int" => {
                arity(args, 0, 1)?;
                match args.first().copied().unwrap_or(Value::Int(0)) {
                    Value::Int(n) => Value::Int(n),
                    Value::Bool(b) => Value::Int(i64::from(b)),
                    Value::Float(n)
                        if n.is_finite() && n >= i64::MIN as f64 && n < -(i64::MIN as f64) =>
                    {
                        Value::Int(n as i64)
                    }
                    value => {
                        let Object::String(text) = self.heap.get(value)? else {
                            return Err(Error::Type("invalid integer"));
                        };
                        Value::Int(
                            text.trim()
                                .parse()
                                .map_err(|_| error("ValueError: invalid integer"))?,
                        )
                    }
                }
            }
            "float" => {
                arity(args, 0, 1)?;
                let value = args.first().copied().unwrap_or(Value::Float(0.0));
                Value::Float(if let Value::Ref(_) = value {
                    let Object::String(text) = self.heap.get(value)? else {
                        return Err(Error::Type("invalid float"));
                    };
                    text.parse()
                        .map_err(|_| error("ValueError: invalid float"))?
                } else {
                    number(value)?
                })
            }
            "bool" => {
                arity(args, 0, 1)?;
                Value::Bool(if args.is_empty() {
                    false
                } else {
                    runtime::truth(&self.heap, args[0])?
                })
            }
            "str" | "repr" => {
                arity(args, 0, 1)?;
                let text = if args.is_empty() {
                    String::new()
                } else {
                    self.repr(args[0], name == "repr")?
                };
                self.heap.allocate(Object::String(text))
            }
            "abs" => {
                arity(args, 1, 1)?;
                match args[0] {
                    Value::Int(n) => Value::Int(n.checked_abs().ok_or(Error::IntegerOverflow)?),
                    value => Value::Float(number(value)?.abs()),
                }
            }
            "sum" => {
                arity(args, 1, 2)?;
                let mut result = args.get(1).copied().unwrap_or(Value::Int(0));
                for value in self.collect_iter(args[0])? {
                    result = runtime::add(&mut self.heap, result, value)?;
                }
                result
            }
            "min" | "max" => {
                arity(args, 1, usize::MAX)?;
                let items = if args.len() == 1 {
                    self.collect_iter(args[0])?
                } else {
                    args.to_vec()
                };
                let mut result = *items
                    .first()
                    .ok_or_else(|| error("ValueError: empty sequence"))?;
                for value in items.into_iter().skip(1) {
                    if compare(
                        &self.heap,
                        if name == "min" { "<" } else { ">" },
                        value,
                        result,
                    )? {
                        result = value;
                    }
                }
                result
            }
            "all" | "any" => {
                arity(args, 1, 1)?;
                let iter = self.iter(args[0])?;
                let mut result = name == "all";
                loop {
                    let value = self.next(iter)?;
                    if self.is_done(value) {
                        break;
                    }
                    let truth = runtime::truth(&self.heap, value)?;
                    if truth == (name == "any") {
                        result = truth;
                        break;
                    }
                }
                Value::Bool(result)
            }
            "enumerate" => {
                arity(args, 1, 2)?;
                let index = if args.len() == 2 {
                    integer(args[1])?
                } else {
                    0
                };
                let iterator = self.iter(args[0])?;
                self.heap.allocate(Object::Enumerate { iterator, index })
            }
            "zip" => {
                let iterators = args
                    .iter()
                    .map(|&value| self.iter(value))
                    .collect::<Result<Vec<_>>>()?;
                self.heap.allocate(Object::Zip {
                    iterators,
                    done: false,
                })
            }
            "reversed" | "sorted" => {
                arity(args, 1, 1)?;
                let mut items = self.collect_iter(args[0])?;
                if name == "reversed" {
                    items.reverse();
                } else {
                    // A fallible insertion sort keeps comparisons in the runtime service.
                    for i in 1..items.len() {
                        let mut j = i;
                        while j > 0 && compare(&self.heap, "<", items[j], items[j - 1])? {
                            items.swap(j, j - 1);
                            j -= 1;
                        }
                    }
                }
                let list = self.heap.allocate(Object::List(items));
                if name == "reversed" {
                    self.iter(list)?
                } else {
                    list
                }
            }
            "isinstance" => {
                arity(args, 2, 2)?;
                Value::Bool(self.instance_of(args[0], args[1])?)
            }
            "object" => {
                arity(args, 0, 0)?;
                return Err(Error::Unsupported("object()"));
            }
            _ => return Err(error(format!("unknown builtin {name}"))),
        })
    }

    fn instance_of(&self, value: Value, class: Value) -> Result<bool> {
        match self.heap.get(class)? {
            Object::Tuple(classes) => {
                for &class in classes {
                    if self.instance_of(value, class)? {
                        return Ok(true);
                    }
                }
                Ok(false)
            }
            Object::Native(name) => Ok(match name.as_str() {
                "int" => matches!(value, Value::Int(_) | Value::Bool(_)),
                "bool" => matches!(value, Value::Bool(_)),
                "float" => matches!(value, Value::Float(_)),
                "object" => true,
                "list" => matches!(self.heap.get(value), Ok(Object::List(_))),
                "tuple" => matches!(self.heap.get(value), Ok(Object::Tuple(_))),
                "dict" => matches!(self.heap.get(value), Ok(Object::Dict(_))),
                "set" => matches!(self.heap.get(value), Ok(Object::Set(_))),
                "str" => matches!(self.heap.get(value), Ok(Object::String(_))),
                _ => return Err(Error::Type("isinstance requires type")),
            }),
            Object::Class { .. } => {
                let Ok(Object::Instance { class: actual, .. }) = self.heap.get(value) else {
                    return Ok(false);
                };
                let mut pending = vec![*actual];
                while let Some(actual) = pending.pop() {
                    if actual == class {
                        return Ok(true);
                    }
                    if let Object::Class { bases, .. } = self.heap.get(actual)? {
                        pending.extend(bases);
                    }
                }
                Ok(false)
            }
            _ => Err(Error::Type("isinstance requires type")),
        }
    }

    fn method(&mut self, receiver: Value, name: &str, args: &[Value]) -> Result<Value> {
        let object = self.heap.get(receiver)?.clone();
        match object {
            Object::List(mut items) => {
                let result = match name {
                    "append" => {
                        arity(args, 1, 1)?;
                        items.push(args[0]);
                        Value::None
                    }
                    "extend" => {
                        arity(args, 1, 1)?;
                        items.extend(self.collect_iter(args[0])?);
                        Value::None
                    }
                    "pop" => {
                        arity(args, 0, 1)?;
                        if items.is_empty() {
                            return Err(Error::Index);
                        }
                        let index = normalize_index(
                            args.first().copied().unwrap_or(Value::Int(-1)),
                            items.len(),
                        )?;
                        items.remove(index)
                    }
                    "insert" => {
                        arity(args, 2, 2)?;
                        let n = integer(args[0])?;
                        let index = if n < 0 {
                            (items.len() as i128 + n as i128).max(0) as usize
                        } else {
                            (n as usize).min(items.len())
                        };
                        items.insert(index, args[1]);
                        Value::None
                    }
                    "clear" => {
                        arity(args, 0, 0)?;
                        items.clear();
                        Value::None
                    }
                    "reverse" => {
                        arity(args, 0, 0)?;
                        items.reverse();
                        Value::None
                    }
                    "copy" => {
                        arity(args, 0, 0)?;
                        return Ok(self.heap.allocate(Object::List(items)));
                    }
                    "count" | "index" | "remove" => {
                        arity(args, 1, 1)?;
                        let mut matches = vec![];
                        for (i, &value) in items.iter().enumerate() {
                            if equal(&self.heap, value, args[0], 0)? {
                                matches.push(i);
                            }
                        }
                        match name {
                            "count" => return Ok(Value::Int(matches.len() as i64)),
                            "index" => {
                                return matches
                                    .first()
                                    .map(|&i| Value::Int(i as i64))
                                    .ok_or_else(|| error("ValueError: item not in list"));
                            }
                            _ => {
                                let index = *matches
                                    .first()
                                    .ok_or_else(|| error("ValueError: item not in list"))?;
                                items.remove(index);
                                Value::None
                            }
                        }
                    }
                    _ => return Err(error("unknown list method")),
                };
                *self.heap.get_mut(receiver)? = Object::List(items);
                Ok(result)
            }
            Object::Dict(items) => match name {
                "values" | "keys" | "items" => {
                    arity(args, 0, 0)?;
                    Ok(self.heap.allocate(Object::View {
                        source: receiver,
                        kind: name.into(),
                    }))
                }
                "get" | "setdefault" => {
                    arity(args, 1, 2)?;
                    let key = self.heap.key(args[0])?;
                    if let Some((_, value)) = items.iter().find(|(k, _)| *k == key) {
                        return Ok(*value);
                    }
                    let default = args.get(1).copied().unwrap_or(Value::None);
                    if name == "setdefault" {
                        self.heap.set_item(receiver, args[0], default)?;
                    }
                    Ok(default)
                }
                "pop" => {
                    arity(args, 1, 2)?;
                    let key = self.heap.key(args[0])?;
                    let mut items = items;
                    if let Some(index) = items.iter().position(|(k, _)| *k == key) {
                        let (_, value) = items.remove(index);
                        *self.heap.get_mut(receiver)? = Object::Dict(items);
                        Ok(value)
                    } else {
                        args.get(1).copied().ok_or(Error::MissingKey)
                    }
                }
                "clear" => {
                    arity(args, 0, 0)?;
                    *self.heap.get_mut(receiver)? = Object::Dict(vec![]);
                    Ok(Value::None)
                }
                "copy" => {
                    arity(args, 0, 0)?;
                    Ok(self.heap.allocate(Object::Dict(items)))
                }
                "update" => {
                    arity(args, 1, 1)?;
                    let pairs = if let Object::Dict(items) = self.heap.get(args[0])? {
                        items
                            .iter()
                            .map(|(k, v)| (key_value(&self.heap, k), *v))
                            .collect::<Vec<_>>()
                    } else {
                        let mut pairs = vec![];
                        for item in self.collect_iter(args[0])? {
                            let pair = self.collect_iter(item)?;
                            if pair.len() != 2 {
                                return Err(error("ValueError: dictionary pair length"));
                            }
                            pairs.push((pair[0], pair[1]));
                        }
                        pairs
                    };
                    for (key, value) in pairs {
                        self.heap.set_item(receiver, key, value)?;
                    }
                    Ok(Value::None)
                }
                _ => Err(error("unknown dictionary method")),
            },
            Object::Set(mut items) => {
                let result = match name {
                    "add" => {
                        arity(args, 1, 1)?;
                        self.heap.key(args[0])?;
                        let mut found = false;
                        for &value in &items {
                            found |= equal(&self.heap, value, args[0], 0)?;
                        }
                        if !found {
                            items.push(args[0]);
                        }
                        Value::None
                    }
                    "discard" | "remove" => {
                        arity(args, 1, 1)?;
                        let mut found = None;
                        for (i, &value) in items.iter().enumerate() {
                            if equal(&self.heap, value, args[0], 0)? {
                                found = Some(i);
                                break;
                            }
                        }
                        if let Some(i) = found {
                            items.remove(i);
                        } else if name == "remove" {
                            return Err(Error::MissingKey);
                        }
                        Value::None
                    }
                    "clear" => {
                        arity(args, 0, 0)?;
                        items.clear();
                        Value::None
                    }
                    "copy" => {
                        arity(args, 0, 0)?;
                        return Ok(self.heap.allocate(Object::Set(items)));
                    }
                    _ => return Err(error("unknown set method")),
                };
                *self.heap.get_mut(receiver)? = Object::Set(items);
                Ok(result)
            }
            Object::String(text) => {
                let result = match name {
                    "join" => {
                        arity(args, 1, 1)?;
                        let values = self.collect_iter(args[0])?;
                        let parts = values
                            .iter()
                            .map(|&v| match self.heap.get(v)? {
                                Object::String(s) => Ok(s.clone()),
                                _ => Err(Error::Type("join requires strings")),
                            })
                            .collect::<Result<Vec<_>>>()?;
                        text_heap(&self.heap, parts.join(&text))
                    }
                    "split" => {
                        arity(args, 0, 1)?;
                        let parts = if args.is_empty() || args[0] == Value::None {
                            text.split_whitespace()
                                .map(str::to_owned)
                                .collect::<Vec<_>>()
                        } else {
                            let Object::String(sep) = self.heap.get(args[0])? else {
                                return Err(Error::Type("separator must be string"));
                            };
                            if sep.is_empty() {
                                return Err(error("ValueError: empty separator"));
                            }
                            text.split(sep).map(str::to_owned).collect()
                        };
                        self.heap.allocate(Object::List(
                            parts
                                .into_iter()
                                .map(|s| text_heap(&self.heap, s))
                                .collect(),
                        ))
                    }
                    "upper" | "lower" | "strip" => {
                        arity(args, 0, 0)?;
                        text_heap(
                            &self.heap,
                            match name {
                                "upper" => text.to_uppercase(),
                                "lower" => text.to_lowercase(),
                                _ => text.trim().into(),
                            },
                        )
                    }
                    "startswith" | "endswith" => {
                        arity(args, 1, 1)?;
                        let Object::String(part) = self.heap.get(args[0])? else {
                            return Err(Error::Type("expected string"));
                        };
                        Value::Bool(if name == "startswith" {
                            text.starts_with(part)
                        } else {
                            text.ends_with(part)
                        })
                    }
                    "replace" => {
                        arity(args, 2, 2)?;
                        let (Object::String(from), Object::String(to)) =
                            (self.heap.get(args[0])?, self.heap.get(args[1])?)
                        else {
                            return Err(Error::Type("replace requires strings"));
                        };
                        text_heap(&self.heap, text.replace(from, to))
                    }
                    _ => return Err(error("unknown string method")),
                };
                Ok(result)
            }
            _ => Err(Error::Type("unsupported method")),
        }
    }

    fn repr(&self, value: Value, quote: bool) -> Result<String> {
        render(&self.heap, value, quote, &mut vec![])
    }
}

fn text_heap(heap: &Heap, text: String) -> Value {
    heap.allocate(Object::String(text))
}
fn arity(args: &[Value], min: usize, max: usize) -> Result<()> {
    if args.len() < min || args.len() > max {
        Err(error(format!(
            "TypeError: expected {min}..={max} arguments, got {}",
            args.len()
        )))
    } else {
        Ok(())
    }
}
pub(crate) fn integer(value: Value) -> Result<i64> {
    match value {
        Value::Int(n) => Ok(n),
        Value::Bool(b) => Ok(i64::from(b)),
        _ => Err(Error::Type("expected integer")),
    }
}
fn number(value: Value) -> Result<f64> {
    match value {
        Value::Float(n) => Ok(n),
        other => integer(other).map(|n| n as f64),
    }
}
fn key_value(heap: &Heap, key: &Key) -> Value {
    match key {
        Key::None => Value::None,
        Key::Int(n) => Value::Int(*n),
        Key::String(s) => text_heap(heap, s.clone()),
        Key::Float(bits) => Value::Float(f64::from_bits(*bits)),
        Key::Tuple(keys) => heap.allocate(Object::Tuple(
            keys.iter().map(|key| key_value(heap, key)).collect(),
        )),
    }
}
fn normalize_index(value: Value, len: usize) -> Result<usize> {
    let n = integer(value)? as i128;
    let n = if n < 0 { len as i128 + n } else { n };
    if n < 0 || n >= len as i128 {
        Err(Error::Index)
    } else {
        Ok(n as usize)
    }
}

pub(crate) fn range_len(start: i64, stop: i64, step: i64) -> Result<usize> {
    if step == 0 {
        return Err(error("ValueError: zero step"));
    }
    let distance = if step > 0 {
        stop as i128 - start as i128
    } else {
        start as i128 - stop as i128
    };
    usize::try_from(if distance <= 0 {
        0
    } else {
        (distance - 1) / (step as i128).abs() + 1
    })
    .map_err(|_| Error::IntegerOverflow)
}

pub(crate) fn slice(heap: &Heap, object: Value, key: Value) -> Result<Value> {
    let Object::Slice { start, stop, step } = *heap.get(key)? else {
        return Err(Error::Type("expected slice"));
    };
    let len = heap.len(object)? as i128;
    let step = if step == Value::None {
        1
    } else {
        integer(step)? as i128
    };
    if step == 0 {
        return Err(error("ValueError: slice step cannot be zero"));
    }
    let bound = |value: Value, default: i128| -> Result<i128> {
        if value == Value::None {
            return Ok(default);
        }
        let n = integer(value)? as i128;
        let n = if n < 0 { len + n } else { n };
        Ok(if step > 0 {
            n.clamp(0, len)
        } else {
            n.clamp(-1, len - 1)
        })
    };
    let mut i = bound(start, if step > 0 { 0 } else { len - 1 })?;
    let stop = bound(stop, if step > 0 { len } else { -1 })?;
    let mut indices = vec![];
    while if step > 0 { i < stop } else { i > stop } {
        indices.push(i as usize);
        i += step;
    }
    Ok(match heap.get(object)? {
        Object::List(items) => {
            heap.allocate(Object::List(indices.iter().map(|&i| items[i]).collect()))
        }
        Object::Tuple(items) => {
            heap.allocate(Object::Tuple(indices.iter().map(|&i| items[i]).collect()))
        }
        Object::String(text) => {
            let chars = text.chars().collect::<Vec<_>>();
            text_heap(heap, indices.iter().map(|&i| chars[i]).collect())
        }
        _ => return Err(Error::Type("object does not support slicing")),
    })
}

fn binary(heap: &Heap, op: &str, a: Value, b: Value) -> Result<Value> {
    if op == "*" {
        if let (Value::Ref(_), Ok(n)) = (a, integer(b)) {
            return repeat(heap, a, n);
        }
        if let (Ok(n), Value::Ref(_)) = (integer(a), b) {
            return repeat(heap, b, n);
        }
    }
    if let (Ok(a), Ok(b)) = (integer(a), integer(b)) {
        let result = match op {
            "*" => a.checked_mul(b),
            "//" => {
                if b == 0 {
                    return Err(error("ZeroDivisionError"));
                }
                let q = a.checked_div(b).ok_or(Error::IntegerOverflow)?;
                Some(
                    q - if a % b != 0 && (a < 0) != (b < 0) {
                        1
                    } else {
                        0
                    },
                )
            }
            "%" => {
                if b == 0 {
                    return Err(error("ZeroDivisionError"));
                }
                let r = a.checked_rem(b).ok_or(Error::IntegerOverflow)?;
                if r != 0 && (r < 0) != (b < 0) {
                    r.checked_add(b)
                } else {
                    Some(r)
                }
            }
            "&" => Some(a & b),
            "|" => Some(a | b),
            "^" => Some(a ^ b),
            "**" if b >= 0 => u32::try_from(b).ok().and_then(|b| a.checked_pow(b)),
            _ => None,
        };
        if ["*", "//", "%", "&", "|", "^"].contains(&op) || (op == "**" && b >= 0) {
            return result.map(Value::Int).ok_or(Error::IntegerOverflow);
        }
    }
    let (a, b) = (number(a)?, number(b)?);
    Ok(Value::Float(match op {
        "*" => a * b,
        "/" => {
            if b == 0.0 {
                return Err(error("ZeroDivisionError"));
            }
            a / b
        }
        "//" => {
            if b == 0.0 {
                return Err(error("ZeroDivisionError"));
            }
            (a / b).floor()
        }
        "%" => {
            if b == 0.0 {
                return Err(error("ZeroDivisionError"));
            }
            let r = a % b;
            if r != 0.0 && (r < 0.0) != (b < 0.0) {
                r + b
            } else {
                r
            }
        }
        "**" => {
            if a == 0.0 && b < 0.0 {
                return Err(error("ZeroDivisionError"));
            }
            let result = a.powf(b);
            if result.is_nan() {
                return Err(Error::Unsupported("complex numbers"));
            }
            result
        }
        _ => return Err(Error::Unsupported("binary operator")),
    }))
}
fn repeat(heap: &Heap, value: Value, n: i64) -> Result<Value> {
    let n = usize::try_from(n.max(0)).map_err(|_| Error::IntegerOverflow)?;
    Ok(match heap.get(value)? {
        Object::String(s) => text_heap(heap, s.repeat(n)),
        Object::List(items) => heap.allocate(Object::List(items.repeat(n))),
        Object::Tuple(items) => heap.allocate(Object::Tuple(items.repeat(n))),
        _ => return Err(Error::Type("unsupported multiplication")),
    })
}

fn equal(heap: &Heap, a: Value, b: Value, depth: usize) -> Result<bool> {
    if a == b {
        return Ok(true);
    }
    if depth > 128 {
        return Err(error("RecursionError: comparison depth"));
    }
    if let (Ok(a), Ok(b)) = (integer(a), integer(b)) {
        return Ok(a == b);
    }
    if let (Ok(a), Ok(b)) = (number(a), number(b)) {
        return Ok(a == b);
    }
    if let (Value::Ref(_), Value::Ref(_)) = (a, b) {
        match (heap.get(a)?, heap.get(b)?) {
            (Object::String(a), Object::String(b)) => return Ok(a == b),
            (Object::List(a), Object::List(b)) | (Object::Tuple(a), Object::Tuple(b)) => {
                if a.len() != b.len() {
                    return Ok(false);
                }
                for (&a, &b) in a.iter().zip(b) {
                    if !equal(heap, a, b, depth + 1)? {
                        return Ok(false);
                    }
                }
                return Ok(true);
            }
            (Object::Dict(a), Object::Dict(b)) => {
                if a.len() != b.len() {
                    return Ok(false);
                }
                for (key, a) in a {
                    let Some((_, b)) = b.iter().find(|(k, _)| k == key) else {
                        return Ok(false);
                    };
                    if !equal(heap, *a, *b, depth + 1)? {
                        return Ok(false);
                    }
                }
                return Ok(true);
            }
            _ => {}
        }
    }
    Ok(false)
}
fn compare(heap: &Heap, op: &str, a: Value, b: Value) -> Result<bool> {
    match op {
        "==" => return equal(heap, a, b, 0),
        "!=" => return equal(heap, a, b, 0).map(|v| !v),
        "is" => return Ok(a == b),
        "is not" => return Ok(a != b),
        "in" | "not in" => {
            let found = match heap.get(b)? {
                Object::Dict(items) => {
                    let key = heap.key(a)?;
                    items.iter().any(|(k, _)| *k == key)
                }
                Object::List(items) | Object::Tuple(items) | Object::Set(items) => {
                    let mut found = false;
                    for &item in items {
                        if equal(heap, a, item, 0)? {
                            found = true;
                            break;
                        }
                    }
                    found
                }
                Object::String(text) => {
                    let Object::String(part) = heap.get(a)? else {
                        return Err(Error::Type("string membership requires string"));
                    };
                    text.contains(part)
                }
                Object::Range { start, stop, step } => {
                    let n = integer(a)? as i128;
                    let start = *start as i128;
                    let stop = *stop as i128;
                    let step = *step as i128;
                    (if step > 0 {
                        n >= start && n < stop
                    } else {
                        n <= start && n > stop
                    }) && (n - start) % step == 0
                }
                _ => return Err(Error::Type("object does not support membership")),
            };
            return Ok(if op == "in" { found } else { !found });
        }
        _ => {}
    }
    let ordering = if let (Ok(a), Ok(b)) = (integer(a), integer(b)) {
        a.partial_cmp(&b)
    } else if let (Ok(a), Ok(b)) = (number(a), number(b)) {
        a.partial_cmp(&b)
    } else if let (Ok(Object::String(a)), Ok(Object::String(b))) = (heap.get(a), heap.get(b)) {
        a.partial_cmp(b)
    } else {
        return Err(Error::Type("unsupported ordering"));
    };
    Ok(match op {
        "<" => ordering == Some(std::cmp::Ordering::Less),
        "<=" => matches!(
            ordering,
            Some(std::cmp::Ordering::Less | std::cmp::Ordering::Equal)
        ),
        ">" => ordering == Some(std::cmp::Ordering::Greater),
        ">=" => matches!(
            ordering,
            Some(std::cmp::Ordering::Greater | std::cmp::Ordering::Equal)
        ),
        _ => return Err(Error::Unsupported("comparison")),
    })
}

fn render(heap: &Heap, value: Value, quote: bool, active: &mut Vec<Value>) -> Result<String> {
    Ok(match value {
        Value::None => "None".into(),
        Value::Bool(b) => if b { "True" } else { "False" }.into(),
        Value::Int(n) => n.to_string(),
        Value::Float(n) => {
            if n.fract() == 0.0 {
                format!("{n:.1}")
            } else {
                n.to_string()
            }
        }
        Value::Ref(_) => {
            if active.contains(&value) {
                return Ok(match heap.get(value)? {
                    Object::Dict(_) => "{...}",
                    _ => "[...]",
                }
                .into());
            }
            active.push(value);
            let result = match heap.get(value)? {
                Object::String(s) => {
                    if quote {
                        format!(
                            "'{}'",
                            s.replace('\\', "\\\\")
                                .replace('\'', "\\'")
                                .replace('\n', "\\n")
                                .replace('\r', "\\r")
                                .replace('\t', "\\t")
                        )
                    } else {
                        s.clone()
                    }
                }
                Object::List(items) | Object::Tuple(items) | Object::Set(items) => {
                    let parts = items
                        .iter()
                        .map(|&v| render(heap, v, true, active))
                        .collect::<Result<Vec<_>>>()?;
                    match heap.get(value)? {
                        Object::Tuple(_) => format!(
                            "({}{})",
                            parts.join(", "),
                            if items.len() == 1 { "," } else { "" }
                        ),
                        Object::Set(_) => {
                            if items.is_empty() {
                                "set()".into()
                            } else {
                                format!("{{{}}}", parts.join(", "))
                            }
                        }
                        _ => format!("[{}]", parts.join(", ")),
                    }
                }
                Object::Dict(items) => {
                    let parts = items
                        .iter()
                        .map(|(k, v)| {
                            Ok(format!(
                                "{}: {}",
                                render(heap, key_value(heap, k), true, active)?,
                                render(heap, *v, true, active)?
                            ))
                        })
                        .collect::<Result<Vec<_>>>()?;
                    format!("{{{}}}", parts.join(", "))
                }
                Object::Class { name, .. } => format!("<class '{name}'>"),
                Object::Instance { class, .. } => {
                    let Object::Class { name, .. } = heap.get(*class)? else {
                        unreachable!()
                    };
                    format!("<{name} object>")
                }
                Object::Function(_) => "<function>".into(),
                Object::Native(name) => format!("<built-in {name}>"),
                Object::Range { start, stop, step } => format!("range({start}, {stop}, {step})"),
                _ => "<object>".into(),
            };
            active.pop();
            result
        }
    })
}
