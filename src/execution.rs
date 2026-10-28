//! Lexical environments and Python object operations shared by execution backends.

use crate::{
    Error, Heap, Key, Object, Result, Value, Vm,
    hir::{CoreOp, Function},
    runtime,
};
use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
    rc::Rc,
};

#[derive(Debug, Clone)]
pub struct Environment {
    pub(crate) parent: Option<Value>,
    pub(crate) values: crate::bindings::Bindings,
    pub(crate) locals: BTreeSet<String>,
    pub(crate) globals: BTreeSet<String>,
    pub(crate) nonlocals: BTreeSet<String>,
    pub(crate) class_scope: bool,
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

fn builtin_method_name(name: &str) -> Result<&'static str> {
    Ok(match name {
        "append" => "append",
        "extend" => "extend",
        "pop" => "pop",
        "reverse" => "reverse",
        "copy" => "copy",
        "clear" => "clear",
        "insert" => "insert",
        "count" => "count",
        "index" => "index",
        "remove" => "remove",
        "sort" => "sort",
        "values" => "values",
        "keys" => "keys",
        "items" => "items",
        "get" => "get",
        "setdefault" => "setdefault",
        "update" => "update",
        "add" => "add",
        "discard" => "discard",
        "join" => "join",
        "split" => "split",
        "upper" => "upper",
        "lower" => "lower",
        "strip" => "strip",
        "startswith" => "startswith",
        "endswith" => "endswith",
        "replace" => "replace",
        _ => return Err(Error::InvalidIr("builtin method name")),
    })
}

pub(crate) fn visit_children(object: &Object, mut visit: impl FnMut(Value)) {
    let mut edge = |value| {
        if matches!(value, Value::Ref(_)) {
            visit(value);
        }
    };
    match object {
        Object::List(values) if !values.is_float() => values.iter().copied().for_each(edge),
        Object::Tuple(values)
        | Object::Set(values)
        | Object::Zip {
            iterators: values, ..
        } => values.iter().copied().for_each(edge),
        Object::Dict(values) => values.iter().map(|(_, value)| *value).for_each(edge),
        Object::Environment(env) => env
            .values
            .values()
            .copied()
            .chain(env.parent)
            .for_each(edge),
        Object::Function(closure) => std::iter::once(closure.environment)
            .chain(closure.defaults.iter().copied())
            .for_each(edge),
        Object::Bound(value, _)
        | Object::Iterator { source: value, .. }
        | Object::View { source: value, .. }
        | Object::Enumerate {
            iterator: value, ..
        } => edge(*value),
        Object::Method(function, receiver) => {
            edge(*function);
            edge(*receiver);
        }
        Object::Class {
            namespace, bases, ..
        } => std::iter::once(*namespace)
            .chain(bases.iter().copied())
            .for_each(edge),
        Object::Instance { class, attributes } => std::iter::once(*class)
            .chain(attributes.values().copied())
            .for_each(edge),
        Object::Slice { start, stop, step } => {
            edge(*start);
            edge(*stop);
            edge(*step);
        }
        _ => {}
    }
}

impl Vm {
    pub(crate) fn ensure_globals(&mut self) -> Value {
        if let Some(env) = self.globals {
            return env;
        }
        let mut values = crate::bindings::Bindings::new();
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

    pub(crate) fn bound_arguments(
        &self,
        function: &Function,
        env: Value,
        arguments: &[Value],
    ) -> Result<Vec<Value>> {
        if function.parameter_names.is_empty()
            && function.keyword_only.is_empty()
            && function.vararg.is_none()
            && function.kwarg.is_none()
        {
            return Ok(arguments.to_vec());
        }
        function
            .parameter_names
            .iter()
            .chain(&function.keyword_only)
            .chain(function.vararg.iter())
            .chain(function.kwarg.iter())
            .map(|name| self.lookup(env, name))
            .collect()
    }

    pub fn get_global(&self, name: &str) -> Result<Value> {
        self.lookup(
            self.globals
                .ok_or_else(|| error("module has not executed"))?,
            name,
        )
    }

    pub fn function_ir(&self, callable: Value) -> Result<Function> {
        self.function_hir(callable)
    }

    pub fn function_hir(&self, callable: Value) -> Result<Function> {
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

    pub(crate) fn load_name_slot(
        &mut self,
        name: &str,
        cache: &std::cell::Cell<Option<crate::bindings::NameSlot>>,
    ) -> Result<Value> {
        let environment = *self
            .environments
            .last()
            .ok_or(Error::InvalidIr("frame environment"))?;
        if let Some(address) = cache.get()
            && address.environment == environment
            && let Some(&value) = self.env(address.namespace)?.values.at(address.slot)
        {
            self.tier_stats.name_cache_hits += 1;
            return Ok(value);
        }
        let mut target = environment;
        loop {
            let namespace = self.env(target)?;
            // Function-local membership is fixed by compilation. Class namespaces can gain
            // nearer bindings during execution, so paths through them retain dynamic lookup.
            if namespace.class_scope {
                return self.lookup(environment, name);
            }
            if namespace.globals.contains(name) {
                target = self.globals.ok_or(Error::InvalidIr("globals"))?;
                continue;
            }
            if let Some(slot) = namespace.values.slot(name)
                && let Some(&value) = namespace.values.at(slot)
            {
                cache.set(Some(crate::bindings::NameSlot {
                    environment,
                    namespace: target,
                    slot,
                }));
                return Ok(value);
            }
            if namespace.locals.contains(name) {
                return Err(error(format!("UnboundLocalError: {name}")));
            }
            target = namespace
                .parent
                .ok_or_else(|| error(format!("NameError: {name}")))?;
        }
    }

    pub(crate) fn store_name_slot(
        &mut self,
        name: &str,
        value: Value,
        cache: &std::cell::Cell<Option<crate::bindings::NameSlot>>,
    ) -> Result<()> {
        let environment = *self
            .environments
            .last()
            .ok_or(Error::InvalidIr("frame environment"))?;
        if let Some(address) = cache
            .get()
            .filter(|address| address.environment == environment)
        {
            let mut object = self.heap.get_mut(address.namespace)?;
            let Object::Environment(namespace) = &mut *object else {
                return Err(Error::InvalidIr("name namespace"));
            };
            namespace.values.set(address.slot, value);
            self.tier_stats.name_cache_hits += 1;
            return Ok(());
        }
        let target = self.store_target(environment, name)?;
        self.store(environment, name, value)?;
        let slot = self
            .env(target)?
            .values
            .slot(name)
            .ok_or(Error::InvalidIr("stored name slot"))?;
        cache.set(Some(crate::bindings::NameSlot {
            environment,
            namespace: target,
            slot,
        }));
        Ok(())
    }

    pub(crate) fn lookup(&self, mut environment: Value, name: &str) -> Result<Value> {
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

    fn store_target(&self, environment: Value, name: &str) -> Result<Value> {
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
        Ok(target)
    }

    pub(crate) fn store(&mut self, environment: Value, name: &str, value: Value) -> Result<()> {
        let target = self.store_target(environment, name)?;
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
            if value == Value::Unbound {
                return Err(Error::InvalidIr("internal argument value"));
            }
            if matches!(value, Value::Ref(_)) {
                self.heap.get(value)?;
            }
        }
        // Indexed, uncaptured locals already live in the frame. Such a positional call can
        // use its lexical parent directly; no environment object or binding map is needed.
        let names = &template.parameter_names;
        if !template.environment_required()
            && keywords.is_empty()
            && template.keyword_only.is_empty()
            && template.vararg.is_none()
            && template.kwarg.is_none()
            && arguments.len() <= names.len()
            && arguments.len() + defaults.len() >= names.len()
        {
            let mut bound = arguments.to_vec();
            for index in arguments.len()..names.len() {
                bound.push(defaults[index + defaults.len() - names.len()]);
            }
            drop(template);
            return self.execute_bound_frame(function, parent, arguments, Some(&bound));
        }
        let environment = if template.module {
            parent
        } else {
            self.heap.allocate(Object::Environment(Environment {
                parent: Some(parent),
                values: crate::bindings::Bindings::new(),
                // Slot locals are never looked up through this environment. Captured names retain
                // their lexical membership so closures and nonlocal assignment keep working.
                locals: template
                    .local_names
                    .iter()
                    .filter(|name| template.local_slots.binary_search(name).is_err())
                    .cloned()
                    .collect(),
                globals: template.global_names.iter().cloned().collect(),
                nonlocals: template.nonlocal_names.iter().cloned().collect(),
                class_scope: false,
            }))
        };
        let names = &template.parameter_names;
        // Positional calls with only trailing defaults need neither a keyword dictionary nor
        // a temporary binding map. Keep captured arguments in the environment as before.
        if !names.is_empty()
            && keywords.is_empty()
            && template.keyword_only.is_empty()
            && template.vararg.is_none()
            && template.kwarg.is_none()
            && arguments.len() <= names.len()
            && arguments.len() + defaults.len() >= names.len()
        {
            let mut object = self.heap.get_mut(environment)?;
            let Object::Environment(env) = &mut *object else {
                return Err(Error::InvalidIr("call environment"));
            };
            for (index, name) in names.iter().enumerate() {
                let value = arguments
                    .get(index)
                    .copied()
                    .unwrap_or_else(|| defaults[index + defaults.len() - names.len()]);
                env.values.insert(name.clone(), value);
            }
            drop(object);
            drop(template);
            return self.execute_frame(function, environment, arguments);
        }
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
            let extra = self.heap.allocate(Object::Dict(Default::default()));
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

    /// Register operands remain rooted in their caller's window until the result is stored.
    /// Current builtins cannot execute Python or collect, except keyed list sorting.
    pub(crate) fn call_from_registers(
        &mut self,
        callable: Value,
        arguments: &[Value],
    ) -> Result<Value> {
        self.tier_stats.bytecode_positional_calls += 1;
        let leaf = match self.heap.get(callable)? {
            Object::Native(std::borrow::Cow::Borrowed(name)) => Some((None, *name)),
            Object::Bound(receiver, std::borrow::Cow::Borrowed(name)) if *name != "sort" => {
                Some((Some(*receiver), *name))
            }
            _ => None,
        };
        if let Some((receiver, name)) = leaf {
            self.tier_stats.bytecode_leaf_calls += 1;
            if let Some(receiver) = receiver {
                self.method(receiver, name, arguments)
            } else {
                self.builtin(name, arguments, &[])
            }
        } else {
            self.call_keywords(callable, arguments, &[])
        }
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
                if name == "sort" {
                    return self.sort_list(receiver, arguments, keywords);
                }
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
                let middle = self.heap.allocate(Object::List(
                    items[*before..items.len() - after].to_vec().into(),
                ));
                let result = items[..*before]
                    .iter()
                    .copied()
                    .chain(std::iter::once(middle))
                    .chain(items[items.len() - after..].iter().copied())
                    .collect();
                self.heap.allocate(Object::Tuple(result))
            }
            CoreOp::LoadName(name) => self.lookup(env, name)?,
            CoreOp::LoadLocal(slot) => self.load_local(*slot)?,
            CoreOp::CheckLocal(slot) => {
                self.check_local(*slot, values[0])?;
                values[0]
            }
            CoreOp::StoreLocal(slot) => {
                self.store_local(*slot, values[0])?;
                Value::None
            }
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
                    parent: Some(if self.env(env)?.class_scope {
                        self.env(env)?.parent.unwrap()
                    } else {
                        env
                    }),
                    values: crate::bindings::Bindings::new(),
                    locals: BTreeSet::new(),
                    globals: BTreeSet::new(),
                    nonlocals: BTreeSet::new(),
                    class_scope: true,
                }));
                self.execute_frame(Rc::new(RefCell::new((**body).clone())), namespace, &[])?;
                self.heap.allocate(Object::Class {
                    code: body.id(),
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
                let items = match self.heap.get(values[0]) {
                    Ok(Object::Tuple(items)) => {
                        if items.len() != *count {
                            return Err(error("ValueError: unpacking length mismatch"));
                        }
                        return Ok(values[0]);
                    }
                    Ok(Object::List(items)) => {
                        if items.len() != *count {
                            return Err(error("ValueError: unpacking length mismatch"));
                        }
                        items.to_vec()
                    }
                    _ => self.collect_iter(values[0])?,
                };
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
            CoreOp::Import(name) => {
                if name != "math" {
                    return Err(Error::Unsupported("builtin module"));
                }
                if let Some(&module) = self.modules.get(name) {
                    module
                } else {
                    let module = self.heap.allocate(Object::Native("math".into()));
                    self.modules.insert(name.clone(), module);
                    module
                }
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
        // Direct members and classes without bases need no traversal allocations. Keep the
        // cycle-checked walk for inherited lookup when the class itself has no matching member.
        if let Some(value) = self.env(*namespace)?.values.get(name) {
            return Ok(Some(*value));
        }
        if bases.is_empty() {
            return Ok(None);
        }
        let mut pending = vec![class];
        let mut seen = std::collections::HashSet::new();
        while let Some(class) = pending.pop() {
            let Value::Ref(id) = class else {
                return Err(Error::Type("expected class"));
            };
            if !seen.insert(id) {
                return Err(error("cyclic class hierarchy"));
            }
            let Object::Class {
                namespace, bases, ..
            } = self.heap.get(class)?
            else {
                return Err(Error::Type("expected class"));
            };
            if let Some(value) = self.env(*namespace)?.values.get(name) {
                return Ok(Some(*value));
            }
            pending.extend(bases.iter().rev().copied());
        }
        Ok(None)
    }

    /// Only builtin containers and strings bind through this path; instances retain normal lookup.
    pub(crate) fn builtin_method(&self, object: Value, name: &str) -> Result<Option<&'static str>> {
        let supported = match self.heap.get(object)? {
            Object::List(_)
                if [
                    "append", "extend", "pop", "reverse", "copy", "clear", "insert", "count",
                    "index", "remove", "sort",
                ]
                .contains(&name) =>
            {
                true
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
                true
            }
            Object::Set(_) if ["add", "discard", "remove", "copy", "clear"].contains(&name) => true,
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
                true
            }
            _ => false,
        };
        if supported {
            Ok(Some(builtin_method_name(name)?))
        } else {
            Ok(None)
        }
    }

    pub(crate) fn call_prepared_method(
        &mut self,
        receiver: Value,
        name: &'static str,
        arguments: &[Value],
    ) -> Result<Value> {
        self.tier_stats.bytecode_method_calls += 1;
        self.tier_stats.bytecode_positional_calls += 1;
        if name == "sort" {
            self.sort_list(receiver, arguments, &[])
        } else {
            self.tier_stats.bytecode_leaf_calls += 1;
            self.method(receiver, name, arguments)
        }
    }

    fn attribute(&mut self, object: Value, name: &str) -> Result<Value> {
        match self.heap.get(object)? {
            Object::Native(module) if module == "math" => {
                return match name {
                    "pi" => Ok(Value::Float(std::f64::consts::PI)),
                    "sqrt" | "sin" | "cos" | "log" => {
                        let key = match name {
                            "sqrt" => "math.sqrt",
                            "sin" => "math.sin",
                            "cos" => "math.cos",
                            _ => "math.log",
                        };
                        if let Some(&function) = self.modules.get(key) {
                            Ok(function)
                        } else {
                            let function = self.heap.allocate(Object::Native(key.into()));
                            self.modules.insert(key.into(), function);
                            Ok(function)
                        }
                    }
                    _ => Err(error(format!("AttributeError: math.{name}"))),
                };
            }
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
            _ => {
                if let Some(name) = self.builtin_method(object, name)? {
                    return Ok(self.heap.allocate(Object::Bound(object, name.into())));
                }
            }
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
            exhausted: false,
        }))
    }

    fn next(&mut self, iterator: Value) -> Result<Value> {
        self.next_at(iterator, 0)
    }

    fn next_at(&mut self, iterator: Value, depth: usize) -> Result<Value> {
        if depth >= 256 {
            return Err(error("RecursionError: iterator nesting"));
        }
        stacker::maybe_grow(128 * 1024, 2 * 1024 * 1024, || {
            self.next_inner(iterator, depth)
        })
    }

    fn next_inner(&mut self, iterator: Value, depth: usize) -> Result<Value> {
        match self.heap.get(iterator)?.clone() {
            Object::Enumerate {
                iterator: inner,
                index,
            } => {
                let value = self.next_at(inner, depth + 1)?;
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
                    let value = self.next_at(inner, depth + 1)?;
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
            exhausted,
        } = self.heap.get(iterator)?.clone()
        else {
            return Err(Error::Type("next requires iterator"));
        };
        if exhausted {
            return Ok(self.heap.allocate(Object::Done));
        }
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
            *self.heap.get_mut(iterator)? = Object::Iterator {
                source,
                index,
                size,
                exhausted: true,
            };
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
        if name != "print" && name != "dict" && !keywords.is_empty() {
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
                    match name.as_ref() {
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
                    "list" => self.heap.allocate(Object::List(items.into())),
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
                let dict = self.heap.allocate(Object::Dict(Default::default()));
                if let Some(&source) = args.first() {
                    self.method(dict, "update", &[source])?;
                }
                for (name, value) in keywords {
                    let key = text_heap(&self.heap, name.clone());
                    self.heap.set_item(dict, key, *value)?;
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
            "math.sqrt" | "math.sin" | "math.cos" | "math.log" => {
                arity(args, 1, 1)?;
                let value = number(args[0])?;
                if (name == "math.sqrt" && value < 0.0)
                    || (name == "math.log" && value <= 0.0)
                    || (matches!(name, "math.sin" | "math.cos") && value.is_infinite())
                {
                    return Err(error("ValueError: math domain error"));
                }
                Value::Float(match name {
                    "math.sqrt" => value.sqrt(),
                    "math.sin" => value.sin(),
                    "math.log" => value.ln(),
                    _ => value.cos(),
                })
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
                let list = self.heap.allocate(Object::List(items.into()));
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
            Object::Native(name) => Ok(match name.as_ref() {
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

    fn sort_list(
        &mut self,
        receiver: Value,
        args: &[Value],
        keywords: &[(String, Value)],
    ) -> Result<Value> {
        arity(args, 0, 0)?;
        let mut key = Value::None;
        let mut reverse = false;
        for (name, value) in keywords {
            match name.as_ref() {
                "key" => key = *value,
                "reverse" => reverse = runtime::truth(&self.heap, *value)?,
                _ => return Err(error("TypeError: invalid sort keyword")),
            }
        }
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
                            compare(&self.heap, "<", keys[order[left]], keys[order[right]])?
                        } else {
                            compare(&self.heap, "<", keys[order[right]], keys[order[left]])?
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

    fn list_method(&mut self, receiver: Value, name: &str, args: &[Value]) -> Result<Value> {
        match name {
            "append" => {
                arity(args, 1, 1)?;
                if let Object::List(items) = &mut *self.heap.get_mut(receiver)? {
                    items.push(args[0]);
                }
            }
            "extend" => {
                arity(args, 1, 1)?;
                // Snapshot before mutation so extending a list with itself terminates.
                let values = self.collect_iter(args[0])?;
                if let Object::List(items) = &mut *self.heap.get_mut(receiver)? {
                    for value in values {
                        items.push(value);
                    }
                }
            }
            "insert" => {
                arity(args, 2, 2)?;
                let n = integer(args[0])?;
                if let Object::List(items) = &mut *self.heap.get_mut(receiver)? {
                    let index = if n < 0 {
                        (items.len() as i128 + n as i128).max(0) as usize
                    } else {
                        (n as usize).min(items.len())
                    };
                    items.insert(index, args[1]);
                }
            }
            "pop" => {
                arity(args, 0, 1)?;
                let len = self.heap.len(receiver)?;
                let index = normalize_index(args.first().copied().unwrap_or(Value::Int(-1)), len)?;
                if let Object::List(items) = &mut *self.heap.get_mut(receiver)? {
                    return Ok(items.remove(index));
                }
            }
            "clear" | "reverse" => {
                arity(args, 0, 0)?;
                if let Object::List(items) = &mut *self.heap.get_mut(receiver)? {
                    if name == "clear" {
                        items.clear();
                    } else {
                        items.reverse();
                    }
                }
            }
            "copy" => {
                arity(args, 0, 0)?;
                let Object::List(items) = self.heap.get(receiver)? else {
                    unreachable!()
                };
                return Ok(self.heap.allocate(Object::List(items.clone())));
            }
            "count" | "index" | "remove" => {
                arity(args, 1, 1)?;
                let Object::List(items) = self.heap.get(receiver)? else {
                    unreachable!()
                };
                let mut count = 0;
                let mut found = None;
                for i in 0..items.len() {
                    let value = items.value(i).ok_or(Error::Index)?;
                    if equal(&self.heap, value, args[0], 0)? {
                        count += 1;
                        found = Some(i);
                        if name != "count" {
                            break;
                        }
                    }
                }
                if name == "count" {
                    return Ok(Value::Int(count));
                }
                let index = found.ok_or_else(|| error("ValueError: item not in list"))?;
                if name == "index" {
                    return Ok(Value::Int(index as i64));
                }
                if let Object::List(items) = &mut *self.heap.get_mut(receiver)? {
                    items.remove(index);
                }
            }
            _ => return Err(error("unknown list method")),
        }
        Ok(Value::None)
    }

    fn dict_method(&mut self, receiver: Value, name: &str, args: &[Value]) -> Result<Value> {
        match name {
            "values" | "keys" | "items" => {
                arity(args, 0, 0)?;
                Ok(self.heap.allocate(Object::View {
                    source: receiver,
                    kind: name.into(),
                }))
            }
            "get" | "setdefault" | "pop" => {
                arity(args, 1, 2)?;
                let Object::Dict(items) = self.heap.get(receiver)? else {
                    unreachable!()
                };
                if name != "pop"
                    && let Some(value) = self.heap.dict_get(items, args[0])?
                {
                    return Ok(value);
                }
                let key = self.heap.key(args[0])?;
                let Object::Dict(items) = self.heap.get(receiver)? else {
                    unreachable!()
                };
                if let Some(&value) = items.get(&key) {
                    if name == "pop"
                        && let Object::Dict(items) = &mut *self.heap.get_mut(receiver)?
                    {
                        items.remove(&key);
                    }
                    return Ok(value);
                }
                let default = args.get(1).copied().unwrap_or(Value::None);
                if name == "setdefault" {
                    self.heap.set_item(receiver, args[0], default)?;
                }
                if name == "pop" && args.len() == 1 {
                    return Err(Error::MissingKey);
                }
                Ok(default)
            }
            "clear" => {
                arity(args, 0, 0)?;
                if let Object::Dict(items) = &mut *self.heap.get_mut(receiver)? {
                    items.clear();
                }
                Ok(Value::None)
            }
            "copy" => {
                arity(args, 0, 0)?;
                let Object::Dict(items) = self.heap.get(receiver)? else {
                    unreachable!()
                };
                Ok(self.heap.allocate(Object::Dict(items.clone())))
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
        }
    }

    fn method(&mut self, receiver: Value, name: &str, args: &[Value]) -> Result<Value> {
        match self.heap.get(receiver)? {
            Object::List(_) => return self.list_method(receiver, name, args),
            Object::Dict(_) => return self.dict_method(receiver, name, args),
            _ => {}
        }
        let object = self.heap.get(receiver)?.clone();
        match object {
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
                                .collect::<Vec<_>>()
                                .into(),
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
        Key::Bool(value) => Value::Bool(*value),
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
        Object::List(items) => heap.allocate(Object::List(
            indices.iter().map(|&i| items[i]).collect::<Vec<_>>().into(),
        )),
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
        Object::List(items) => heap.allocate(Object::List(items.repeat(n).into())),
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
    if let Some(ordering) = numeric_order(a, b) {
        return Ok(ordering == Some(std::cmp::Ordering::Equal));
    }
    if let (Value::Ref(_), Value::Ref(_)) = (a, b) {
        match (heap.get(a)?, heap.get(b)?) {
            (Object::String(a), Object::String(b)) => return Ok(a == b),
            (a @ Object::List(_), b @ Object::List(_))
            | (a @ Object::Tuple(_), b @ Object::Tuple(_)) => {
                let (a, b) = (a.values().unwrap(), b.values().unwrap());
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
            (Object::Set(a), Object::Set(b)) => {
                if a.len() != b.len() {
                    return Ok(false);
                }
                for &value in a {
                    let mut found = false;
                    for &candidate in b {
                        if equal(heap, value, candidate, depth + 1)? {
                            found = true;
                            break;
                        }
                    }
                    if !found {
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
                    let Some(b) = b.get(key) else {
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
                Object::Dict(items) => heap.dict_get(items, a)?.is_some(),
                object @ (Object::List(_) | Object::Tuple(_) | Object::Set(_)) => {
                    let items = object.values().unwrap();
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
    let ordering = order(heap, a, b, 0)?;
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
    if active.len() >= 256 {
        return Err(error("RecursionError: representation nesting"));
    }
    stacker::maybe_grow(128 * 1024, 2 * 1024 * 1024, || {
        render_inner(heap, value, quote, active)
    })
}
fn render_inner(heap: &Heap, value: Value, quote: bool, active: &mut Vec<Value>) -> Result<String> {
    Ok(match value {
        Value::Unbound => return Err(Error::InvalidIr("unbound value representation")),
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
                object @ (Object::List(_) | Object::Tuple(_) | Object::Set(_)) => {
                    let items = object.values().unwrap();
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

pub(crate) fn numeric_order(a: Value, b: Value) -> Option<Option<std::cmp::Ordering>> {
    use std::cmp::Ordering;
    fn mixed(integer: i64, float: f64) -> Option<Ordering> {
        if float.is_nan() {
            return None;
        }
        if float >= -(i64::MIN as f64) {
            return Some(Ordering::Less);
        }
        if float < i64::MIN as f64 {
            return Some(Ordering::Greater);
        }
        match integer.cmp(&(float as i64)) {
            Ordering::Equal => (0.0_f64).partial_cmp(&float.fract()),
            other => Some(other),
        }
    }
    match (a, b) {
        (Value::Float(a), Value::Float(b)) => Some(a.partial_cmp(&b)),
        (a, Value::Float(b)) => integer(a).ok().map(|a| mixed(a, b)),
        (Value::Float(a), b) => integer(b).ok().map(|b| mixed(b, a).map(Ordering::reverse)),
        (a, b) => integer(a)
            .ok()
            .zip(integer(b).ok())
            .map(|(a, b)| Some(a.cmp(&b))),
    }
}

fn order(heap: &Heap, a: Value, b: Value, depth: usize) -> Result<Option<std::cmp::Ordering>> {
    if depth > 128 {
        return Err(error("RecursionError: comparison depth"));
    }
    if let Some(order) = numeric_order(a, b) {
        return Ok(order);
    }
    match (heap.get(a), heap.get(b)) {
        (Ok(Object::String(a)), Ok(Object::String(b))) => Ok(Some(a.cmp(b))),
        (Ok(a @ Object::List(_)), Ok(b @ Object::List(_)))
        | (Ok(a @ Object::Tuple(_)), Ok(b @ Object::Tuple(_))) => {
            let (a, b) = (a.values().unwrap(), b.values().unwrap());
            for (&a, &b) in a.iter().zip(b) {
                if !equal(heap, a, b, depth + 1)? {
                    return order(heap, a, b, depth + 1);
                }
            }
            Ok(Some(a.len().cmp(&b.len())))
        }
        _ => Err(Error::Type("unsupported ordering")),
    }
}
