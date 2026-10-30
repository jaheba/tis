//! Lexical environments and Python object operations shared by execution backends.
mod attributes;
mod bytes;
mod exceptions;
mod integers;
use exceptions::exception_parent;
mod context;
pub(crate) mod expression;
mod hashing;
mod protocols;
mod representation;
mod slicing;
mod sorting;
mod types;

use crate::{
    Error, ExceptionData, Heap, Key, Object, Result, Value, Vm,
    hir::{CoreOp, Function},
    runtime,
};
use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
    rc::Rc,
};

/// Audited scalar builtins: read-only heap access, no Python, VM allocation, collection or frames.
/// Adding a variant requires preserving this contract on every success and error path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScalarBuiltin {
    Len,
    Sqrt,
    BisectLeft,
    BisectRight,
}
impl ScalarBuiltin {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Len => "len",
            Self::Sqrt => "math.sqrt",
            Self::BisectLeft => "bisect.bisect_left",
            Self::BisectRight => "bisect.bisect_right",
        }
    }
    pub(crate) fn from_name(name: &str) -> Option<Self> {
        match name {
            "len" => Some(Self::Len),
            "math.sqrt" => Some(Self::Sqrt),
            "bisect.bisect_left" => Some(Self::BisectLeft),
            "bisect.bisect_right" => Some(Self::BisectRight),
            _ => None,
        }
    }
    pub(crate) fn accepts(self, count: usize) -> bool {
        match self {
            Self::BisectLeft | Self::BisectRight => (2..=4).contains(&count),
            _ => count == 1,
        }
    }
    /// No VM allocation, collection or Python calls. Heap interior mutability makes this
    /// a body/callee contract; ordinary Rust error strings cannot trigger VM collection.
    fn evaluate(self, heap: &Heap, args: &[Value]) -> Result<Value> {
        if matches!(self, Self::BisectLeft | Self::BisectRight) {
            arity(args, 2, 4)?;
            let lo = args.get(2).map(|&v| integer(v)).transpose()?.unwrap_or(0);
            let hi = match args.get(3).copied().unwrap_or(Value::None) {
                Value::None => -1,
                v => integer(v)?,
            };
            return crate::bisect::search(heap, args[0], args[1], lo, hi, self == Self::BisectLeft)
                .map(Value::Int);
        }
        arity(args, 1, 1)?;
        Ok(match self {
            Self::BisectLeft | Self::BisectRight => unreachable!(),
            Self::Len => {
                if !matches!(args[0], Value::Ref(_) | Value::InlineString(_)) {
                    return Err(Error::Type("object has no length"));
                }
                Value::Int(heap.len(args[0])? as i64)
            }
            Self::Sqrt => {
                let value = crate::integer::number(heap, args[0])?;
                if value < 0.0 {
                    return Err(error("ValueError: math domain error"));
                }
                Value::Float(value.sqrt())
            }
        })
    }
}

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
        "encode" => "encode",
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
        "splitlines" => "splitlines",
        "rjust" => "rjust",
        "upper" => "upper",
        "lower" => "lower",
        "strip" => "strip",
        "isdigit" => "isdigit",
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
        Object::List(values) => {
            if let Some(values) = values.native_values() {
                values.iter().copied().for_each(edge);
            }
        }
        Object::Property {
            get,
            set,
            delete,
            doc,
        } => [*get, *set, *delete, *doc].into_iter().for_each(edge),
        Object::Traceback { next, .. } => next.iter().copied().for_each(edge),
        Object::Super { anchor, receiver } => {
            edge(*anchor);
            edge(*receiver);
        }
        Object::StaticMethod(function) | Object::ClassMethod(function) => edge(*function),
        Object::Exception(exception) => std::iter::once(exception.arguments)
            .chain(exception.attributes.values().copied())
            .chain(exception.cause)
            .chain(exception.context)
            .chain(exception.traceback)
            .for_each(edge),
        Object::Tuple(values)
        | Object::Zip {
            iterators: values, ..
        } => values.iter().copied().for_each(edge),
        Object::Generator { parts, binding, .. } => {
            parts.iter().copied().for_each(&mut edge);
            if let Some(binding) = binding {
                edge(binding.0);
            }
        }
        Object::Dict(values) | Object::Set(values) => {
            for (key, value) in values {
                key.visit_values(&mut edge);
                edge(*value);
            }
        }
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
            "slice",
            "type",
            "super",
            "issubclass",
            "hash",
            "getattr",
            "setattr",
            "delattr",
            "hasattr",
            "property",
            "staticmethod",
            "classmethod",
            "AttributeError",
            "NameError",
            "AssertionError",
            "ArithmeticError",
            "LookupError",
            "RecursionError",
            "NotImplementedError",
            "NotImplemented",
            "BaseException",
            "Exception",
            "ValueError",
            "UnicodeError",
            "UnicodeDecodeError",
            "UnicodeEncodeError",
            "TypeError",
            "RuntimeError",
            "StopIteration",
            "ZeroDivisionError",
            "UnboundLocalError",
            "OverflowError",
            "KeyError",
            "IndexError",
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
            "bytes",
            "abs",
            "hex",
            "oct",
            "bin",
            "divmod",
            "pow",
            "sum",
            "min",
            "max",
            "enumerate",
            "zip",
            "iter",
            "next",
            "isinstance",
            "ord",
            "chr",
            "object",
            "sorted",
            "reversed",
            "all",
            "any",
        ] {
            let value = self.heap.allocate(Object::Native(name.into()));
            if types::builtin_type_name(name) {
                self.modules.insert(format!("type.{name}"), value);
            }
            values.insert(name.into(), value);
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

    /// A cache hit only replaces an existing binding: no allocation, collection, or Python calls.
    pub(crate) fn try_store_name_slot(
        &mut self,
        value: Value,
        cache: &std::cell::Cell<Option<crate::bindings::NameSlot>>,
    ) -> Result<bool> {
        let environment = *self
            .environments
            .last()
            .ok_or(Error::InvalidIr("frame environment"))?;
        let Some(address) = cache
            .get()
            .filter(|address| address.environment == environment)
        else {
            return Ok(false);
        };
        let mut object = self.heap.get_mut(address.namespace)?;
        let Object::Environment(namespace) = &mut *object else {
            return Err(Error::InvalidIr("name namespace"));
        };
        namespace.values.set(address.slot, value);
        self.tier_stats.name_cache_hits += 1;
        Ok(true)
    }

    pub(crate) fn store_name_slot(
        &mut self,
        name: &str,
        value: Value,
        cache: &std::cell::Cell<Option<crate::bindings::NameSlot>>,
    ) -> Result<()> {
        if self.try_store_name_slot(value, cache)? {
            return Ok(());
        }
        let environment = *self
            .environments
            .last()
            .ok_or(Error::InvalidIr("frame environment"))?;
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
            let count = names.len();
            drop(template);
            return self.execute_bound_frame(
                function,
                parent,
                arguments,
                Some(crate::vm::PositionalBindings { defaults, count }),
            );
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
        let boundary = self.depth == 0 && self.transient_roots.is_empty();
        if boundary {
            self.pending_exception = None;
        }
        let roots = self.transient_roots.len();
        self.transient_roots.push(callable);
        self.transient_roots.extend_from_slice(arguments);
        let result = self.call_keywords(callable, arguments, &[]);
        self.transient_roots.truncate(roots);
        if boundary {
            self.pending_exception = None;
        }
        result
    }

    /// Guard the actual object rather than a binding name or cached heap identity. The evaluator's
    /// audited evaluator cannot collect/reenter while native operands are unpublished.
    pub(crate) fn try_scalar_call(
        &mut self,
        builtin: ScalarBuiltin,
        callable: Value,
        arguments: &[Value],
    ) -> Option<Result<Value>> {
        if !matches!(self.heap.get(callable), Ok(Object::Native(name)) if name.as_ref() == builtin.name())
        {
            return None;
        }
        self.tier_stats.bytecode_positional_calls += 1;
        self.tier_stats.bytecode_leaf_calls += 1;
        if builtin == ScalarBuiltin::Len
            && arguments
                .first()
                .is_some_and(|v| self.instance_class(*v).is_some())
        {
            return None;
        }
        let result = builtin.evaluate(&self.heap, arguments);
        if matches!(
            builtin,
            ScalarBuiltin::BisectLeft | ScalarBuiltin::BisectRight
        ) && matches!(result, Err(Error::Type(_)))
        {
            return None;
        }
        Some(result)
    }

    /// Register operands remain rooted in their caller's window until the result is stored.
    /// Builtin dispatch may invoke Python protocols; operands remain rooted in the caller window.
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
        if !matches!(callable, Value::Ref(_)) {
            return Err(Error::Type("object is not callable"));
        }
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
                    if (name == "decode" && matches!(self.heap.get(receiver), Ok(Object::Bytes(_))))
                        || (name == "encode" && self.heap.as_str(&receiver).is_some())
                    {
                        let bound = self.bytes_keywords(arguments, keywords, false)?;
                        return self.method(receiver, &name, &bound);
                    }
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
            Object::Class { shape, .. } => {
                let instance = if let Some(new) = self.class_attribute(callable, "__new__")? {
                    let new = if let Ok(Object::StaticMethod(function)) = self.heap.get(new) {
                        *function
                    } else {
                        new
                    };
                    let args = std::iter::once(callable)
                        .chain(arguments.iter().copied())
                        .collect::<Vec<_>>();
                    self.call_keywords(new, &args, keywords)?
                } else {
                    self.heap.allocate(Object::Instance {
                        class: callable,
                        attributes: crate::Attributes::new(shape),
                    })
                };
                if !self.instance_of(instance, callable)? {
                    return Ok(instance);
                }
                self.protocol_roots(&[instance],|vm| {
                    if vm.exception_class(callable) {
                        let args=vm.heap.allocate(Object::Tuple(arguments.to_vec()));
                        vm.raw_set_attribute(instance,&crate::Attribute::from("args".to_owned()),args)?;
                        for name in ["__cause__","__context__"] {vm.raw_set_attribute(instance,&crate::Attribute::from(name.to_owned()),Value::None)?;}
                        vm.raw_set_attribute(instance,&crate::Attribute::from("__suppress_context__".to_owned()),Value::Bool(false))?;
                    }
                    if vm.special(instance,"__init__")?.is_some() {
                        // Keyword binding is shared with ordinary calls after binding the descriptor.
                        let init=vm.special(instance,"__init__")?.unwrap();
                        let init=if matches!(vm.heap.get(init),Ok(Object::Native(name)) if name=="BaseException.__init__") {vm.heap.allocate(Object::Method(init,instance))}else{vm.descriptor_get(init,Some(instance),callable)?};
                        if vm.call_keywords(init,arguments,keywords)?!=Value::None {return Err(error("TypeError: __init__ must return None"));}
                    } else if !arguments.is_empty() || !keywords.is_empty() {return Err(error("TypeError: class takes no arguments"));}
                    Ok(instance)
                })
            }
            Object::Instance { .. } => {
                let method = self
                    .special(callable, "__call__")?
                    .ok_or(Error::Type("object is not callable"))?;
                let class = self.instance_class(callable).unwrap();
                let method = self.descriptor_get(method, Some(callable), class)?;
                self.call_keywords(method, arguments, keywords)
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
            CoreOp::IntegerLiteral(n) => {
                if let Some(&value) = self.integer_pool.get(n) {
                    value
                } else {
                    let value = crate::integer::value(&self.heap, n.clone());
                    self.integer_pool.insert(n.clone(), value);
                    value
                }
            }
            CoreOp::BytesLiteral(data) => self.intern_bytes(data),
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
            CoreOp::DeleteName(name) => {
                let target = self.store_target(env, name)?;
                let mut target = self.heap.get_mut(target)?;
                let Object::Environment(target) = &mut *target else {
                    return Err(Error::InvalidIr("exception binding environment"));
                };
                target.values.remove(name);
                Value::None
            }
            CoreOp::DeleteBinding(name) => {
                let target = self.store_target(env, name)?;
                let mut target = self.heap.get_mut(target)?;
                let Object::Environment(target) = &mut *target else {
                    return Err(Error::InvalidIr("exception binding environment"));
                };
                if target.values.remove(name).is_none() {
                    return Err(error(format!(
                        "{}: {name}",
                        if target.locals.contains(name) {
                            "UnboundLocalError"
                        } else {
                            "NameError"
                        }
                    )));
                }
                Value::None
            }
            CoreOp::DeleteAttr(name) => self.delete_attribute(values[0], name)?,
            CoreOp::DeleteItem => self.py_delete_item(values[0], values[1])?,
            CoreOp::ZeroSuper(parameter) => {
                if matches!(self.heap.get(values[0]),Ok(Object::Native(name)) if name=="super") {
                    let class = self
                        .lookup(env, "__class__")
                        .map_err(|_| error("RuntimeError: super(): __class__ cell not found"))?;
                    let name = parameter
                        .as_ref()
                        .ok_or_else(|| error("RuntimeError: super(): no arguments"))?;
                    let receiver = self.lookup(env, name)?;
                    self.heap.allocate(Object::Super {
                        anchor: class,
                        receiver,
                    })
                } else {
                    self.call(values[0], &[])?
                }
            }
            CoreOp::Binary(op) => self.py_binary(op, values[0], values[1])?,
            CoreOp::Inplace(op) => {
                if op == "+" && matches!(self.heap.get(values[0]), Ok(Object::List(_))) {
                    self.method(values[0], "extend", &values[1..])?;
                    values[0]
                } else if op == "*" && matches!(self.heap.get(values[0]), Ok(Object::List(_))) {
                    let replacement = repeat(&self.heap, values[0], integer(values[1])?)?;
                    let object = self.heap.get(replacement)?.clone();
                    *self.heap.get_mut(values[0])? = object;
                    values[0]
                } else {
                    self.py_inplace(op, values[0], values[1])?
                }
            }

            CoreOp::Compare(op) => self.py_compare(op, values[0], values[1])?,
            CoreOp::Unary(op) => self.py_unary(op, values[0])?,
            CoreOp::Call(names) => {
                let roots = self.transient_roots.len();
                let result = (|| {
                    let mut positional = vec![];
                    let mut keywords: Vec<(String, Value)> = vec![];
                    for (name, &value) in names.iter().zip(&values[1..]) {
                        match name.as_deref() {
                            None => positional.push(value),
                            Some("*") => {
                                let expanded = self.collect_iter(value)?;
                                self.transient_roots.extend_from_slice(&expanded);
                                positional.extend(expanded);
                            }
                            Some("**") => {
                                let Object::Dict(items) = self.heap.get(value)? else {
                                    return Err(Error::Type("** requires dictionary"));
                                };
                                for (key, value) in items {
                                    let Key::String(name) = key else {
                                        return Err(Error::Type("keywords must be strings"));
                                    };
                                    if keywords
                                        .iter()
                                        .any(|(candidate, _)| candidate.as_str() == name.as_str())
                                    {
                                        return Err(error("duplicate keyword"));
                                    }
                                    keywords.push((name.to_string(), *value));
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
                    self.call_keywords(values[0], &positional, &keywords)
                })();
                self.transient_roots.truncate(roots);
                result?
            }
            CoreOp::With(body) => self.with_context(body, values[0], env)?,
            CoreOp::TryCall(body) => {
                let key = (body.id(), body.revision());
                let function = self
                    .try_bodies
                    .entry(key)
                    .or_insert_with(|| Rc::new(RefCell::new((**body).clone())))
                    .clone();
                // Internal expressions have no Python function identity or private locals.
                // Their enclosing environment remains rooted throughout this ordinary frame.
                let result = self.execute_bound_frame(
                    function,
                    env,
                    &[],
                    Some(crate::vm::PositionalBindings {
                        defaults: &[],
                        count: 0,
                    }),
                );
                let (success, value) = match result {
                    Ok(value) => (true, value),
                    Err(err) => {
                        let value = if let Some(value) = self.pending_exception.take() {
                            value
                        } else {
                            self.exception_from_error(err)?
                        };
                        (false, value)
                    }
                };
                self.heap
                    .allocate(Object::Tuple(vec![Value::Bool(success), value]))
            }
            CoreOp::ExceptionMatch => Value::Bool(self.exception_matches(values[0], values[1])?),
            CoreOp::EnterExcept => {
                self.active_exceptions.push(values[0]);
                Value::None
            }
            CoreOp::ExitExcept => {
                self.active_exceptions
                    .pop()
                    .ok_or(Error::InvalidIr("exception stack underflow"))?;
                Value::None
            }
            CoreOp::ListAppend => {
                let mut object = self.heap.get_mut(values[0])?;
                let Object::List(items) = &mut *object else {
                    return Err(Error::Type("append requires list"));
                };
                items.push(values[1]);
                Value::None
            }
            CoreOp::Generator(filters, target) => {
                if values.len() != filters + 3 {
                    return Err(Error::InvalidIr("generator operands"));
                }
                self.heap.allocate(Object::Generator {
                    parts: values.to_vec().into(),
                    binding: target.as_ref().map(|name| Rc::new((env, name.clone()))),
                    running: false,
                    done: false,
                })
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
                    if !matches!(self.heap.get(base)?, Object::Class { .. })
                        && !self.exception_class(base)
                        && self.modules.get("type.object").copied() != Some(base)
                    {
                        return Err(Error::Type("base must be class"));
                    }
                }
                let lexical_parent = if self.env(env)?.class_scope {
                    self.env(env)?.parent.unwrap()
                } else {
                    env
                };
                let cell = self.heap.allocate(Object::Environment(Environment {
                    parent: Some(lexical_parent),
                    values: crate::bindings::Bindings::new(),
                    locals: BTreeSet::new(),
                    globals: BTreeSet::new(),
                    nonlocals: BTreeSet::new(),
                    class_scope: false,
                }));
                let namespace = self.heap.allocate(Object::Environment(Environment {
                    parent: Some(cell),
                    values: crate::bindings::Bindings::new(),
                    locals: BTreeSet::new(),
                    globals: BTreeSet::new(),
                    nonlocals: BTreeSet::new(),
                    class_scope: true,
                }));
                self.execute_frame(Rc::new(RefCell::new((**body).clone())), namespace, &[])?;
                if self.env(namespace)?.values.get("__eq__").is_some()
                    && self.env(namespace)?.values.get("__hash__").is_none()
                {
                    self.store(namespace, "__hash__", Value::None)?;
                }
                if [
                    "__getattribute__",
                    "__getattr__",
                    "__setattr__",
                    "__delattr__",
                    "__get__",
                    "__set__",
                    "__delete__",
                ]
                .iter()
                .any(|name| {
                    self.env(namespace)
                        .is_ok_and(|env| env.values.get(name).is_some())
                }) {
                    self.attribute_protocols = true;
                }
                let class = self.heap.allocate(Object::Class {
                    code: body.id(),
                    name: name.clone(),
                    namespace,
                    bases: values.to_vec(),
                    shape: crate::Shape::new(),
                });
                self.store(cell, "__class__", class)?;
                self.class_mro(class)?;
                let members = self
                    .env(namespace)?
                    .values
                    .iter()
                    .map(|(name, value)| (name.to_owned(), *value))
                    .collect::<Vec<_>>();
                self.protocol_roots(&[class], |vm| {
                    for (name, value) in members {
                        if vm.special(value, "__set_name__")?.is_some() {
                            let name = vm.heap.string(&name);
                            vm.call_special(value, "__set_name__", &[class, name])?;
                        }
                    }
                    Ok(class)
                })?
            }
            CoreOp::GetAttr(name) => self.attribute(values[0], name)?,
            CoreOp::SetAttr(name) => self.protocol_set_attribute(
                values[0],
                &crate::Attribute::from(name.clone()),
                values[1],
            )?,
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
                    _ => self.with_iterator(values[0], |vm, iterator| {
                        let mut items = Vec::new();
                        for _ in 0..=*count {
                            let value = vm.next(iterator)?;
                            if vm.is_done(value) {
                                break;
                            }
                            vm.transient_roots.push(value);
                            items.push(value);
                        }
                        Ok(items)
                    })?,
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
                let set = self.heap.allocate(Object::Set(Default::default()));
                for &value in values {
                    self.method(set, "add", &[value])?;
                }
                set
            }
            CoreOp::Format(spec, repr) => {
                let text = if spec.is_empty() {
                    self.py_repr(values[0], *repr)?
                } else if spec.starts_with('.') && (spec.ends_with('f') || spec.ends_with('e')) {
                    let precision: usize = spec[1..spec.len() - 1]
                        .parse()
                        .map_err(|_| error("invalid format spec"))?;
                    if precision > 1000 {
                        return Err(error("format precision too large"));
                    }
                    if spec.ends_with('e') {
                        let raw = format!("{:.*e}", precision, number(values[0])?);
                        // Python requires an explicit sign and at least two exponent digits.
                        if let Some((mantissa, exponent)) = raw.split_once('e') {
                            let exponent: i32 =
                                exponent.parse().map_err(|_| error("invalid exponent"))?;
                            format!("{mantissa}e{exponent:+03}")
                        } else {
                            raw
                        }
                    } else {
                        format!("{:.*}", precision, number(values[0])?)
                    }
                } else {
                    return Err(Error::Unsupported("format specification"));
                };
                self.heap.string_owned(text)
            }
            CoreOp::Join => {
                let mut text = String::new();
                for &value in values {
                    let Some(part) = self.heap.as_str(&(value)) else {
                        return Err(Error::Type("join requires strings"));
                    };
                    text.push_str(part);
                }
                self.heap.string_owned(text)
            }
            CoreOp::Raise => return self.raise_exception(values),
            CoreOp::PropagateException => return self.raise_python_exception(values, true),
            CoreOp::Assert => {
                if !self.py_truth(values[0])? {
                    return Err(error("AssertionError"));
                }
                Value::None
            }
            CoreOp::Import(name) => {
                if name != "math" && name != "json" && name != "bisect" {
                    return Err(Error::Unsupported("builtin module"));
                }
                if let Some(&module) = self.modules.get(name) {
                    module
                } else {
                    let module = self.heap.allocate(Object::Native(name.clone().into()));
                    self.modules.insert(name.clone(), module);
                    module
                }
            }
            CoreOp::Namespace => env,
        })
    }

    pub(crate) fn is_done(&self, value: Value) -> bool {
        matches!(value, Value::Ref(_)) && matches!(self.heap.get(value), Ok(Object::Done))
    }

    pub(crate) fn class_attribute(&self, class: Value, name: &str) -> Result<Option<Value>> {
        if matches!(self.heap.get(class),Ok(Object::Native(kind)) if is_exception(kind) || kind=="object")
        {
            return Ok(if name == "__init__" && self.exception_class(class) {
                Some(
                    self.heap
                        .allocate(Object::Native("BaseException.__init__".into())),
                )
            } else {
                None
            });
        }
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
        for base in self.class_mro(class)?.into_iter().skip(1) {
            match self.heap.get(base)? {
                Object::Class { namespace, .. } => {
                    if let Some(value) = self.env(*namespace)?.values.get(name) {
                        return Ok(Some(*value));
                    }
                }
                Object::Native(kind) if is_exception(kind) && name == "__init__" => {
                    return Ok(Some(
                        self.heap
                            .allocate(Object::Native("BaseException.__init__".into())),
                    ));
                }
                _ => {}
            }
        }

        Ok(None)
    }

    /// Only builtin containers and strings bind through this path; instances retain normal lookup.
    pub(crate) fn builtin_method(&self, object: Value, name: &str) -> Result<Option<&'static str>> {
        if crate::integer::is_integer(&self.heap, object) {
            return Ok(match name {
                "bit_length" => Some("bit_length"),
                "bit_count" => Some("bit_count"),
                _ => None,
            });
        }
        if matches!(object, Value::InlineString(_)) {
            return Ok(
                if [
                    "encode",
                    "join",
                    "split",
                    "splitlines",
                    "rjust",
                    "upper",
                    "lower",
                    "strip",
                    "isdigit",
                    "startswith",
                    "endswith",
                    "replace",
                ]
                .contains(&name)
                {
                    Some(builtin_method_name(name)?)
                } else {
                    None
                },
            );
        }
        if matches!(self.heap.get(object)?, Object::Bytes(_)) {
            return Ok(bytes::method_name(name));
        }
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
                    "encode",
                    "join",
                    "split",
                    "splitlines",
                    "rjust",
                    "upper",
                    "lower",
                    "strip",
                    "isdigit",
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

    pub(crate) fn get_attribute(
        &mut self,
        object: Value,
        attribute: &crate::Attribute,
    ) -> Result<Value> {
        self.protocol_get_attribute(object, &attribute.text, Some(attribute))
    }

    pub(crate) fn set_attribute(
        &mut self,
        object: Value,
        attribute: &crate::Attribute,
        value: Value,
    ) -> Result<Value> {
        self.protocol_set_attribute(object, attribute, value)
    }

    pub(crate) fn raw_set_attribute(
        &mut self,
        object: Value,
        attribute: &crate::Attribute,
        value: Value,
    ) -> Result<Value> {
        self.check_type_attribute_mutation(object, &attribute.text)?;
        let mut target = self.heap.get_mut(object)?;
        match &mut *target {
            Object::Instance { attributes, .. } => {
                if attributes.write(attribute, value) {
                    self.tier_stats.shape_write_hits += 1;
                }
            }
            Object::Class { namespace, .. } => {
                let namespace = *namespace;
                drop(target);
                self.attribute_protocols = true;
                self.store(namespace, &attribute.text, value)?;
            }
            _ => return Err(Error::Type("object has no writable attributes")),
        }
        Ok(Value::None)
    }

    fn attribute(&mut self, object: Value, name: &str) -> Result<Value> {
        self.protocol_get_attribute(object, name, None)
    }

    fn builtin_attribute(
        &mut self,
        object: Value,
        name: &str,
        cached: Option<&crate::Attribute>,
    ) -> Result<Value> {
        if crate::integer::is_integer(&self.heap, object) {
            return self
                .builtin_method(object, name)?
                .map(|name| self.heap.allocate(Object::Bound(object, name.into())))
                .ok_or_else(|| error(format!("AttributeError: {name}")));
        }
        if !matches!(object, Value::Ref(_) | Value::InlineString(_)) {
            return Err(error(format!("AttributeError: {name}")));
        }
        if matches!(object, Value::InlineString(_)) {
            return self
                .builtin_method(object, name)?
                .map(|name| self.heap.allocate(Object::Bound(object, name.into())))
                .ok_or_else(|| error(format!("AttributeError: {name}")));
        }
        if let Some(value) = self.exception_attribute(object, name)? {
            return Ok(value);
        }
        match self.heap.get(object)? {
            Object::Slice { start, stop, step } => {
                return match name {
                    "start" => Ok(*start),
                    "stop" => Ok(*stop),
                    "step" => Ok(*step),
                    _ => Err(error(format!("AttributeError: {name}"))),
                };
            }
            Object::StaticMethod(function) | Object::ClassMethod(function)
                if name == "__func__" =>
            {
                return Ok(*function);
            }

            Object::Native(module)
                if module == "object"
                    && matches!(
                        name,
                        "__getattribute__" | "__setattr__" | "__delattr__" | "__new__"
                    ) =>
            {
                return Ok(self
                    .heap
                    .allocate(Object::Native(format!("object.{name}").into())));
            }
            Object::Property {
                get,
                set,
                delete,
                doc,
            } => {
                return match name {
                    "fget" => Ok(*get),
                    "fset" => Ok(*set),
                    "fdel" => Ok(*delete),
                    "__doc__" => Ok(*doc),
                    "getter" | "setter" | "deleter" => Ok(self
                        .heap
                        .allocate(Object::Bound(object, name.to_owned().into()))),
                    _ => Err(error(format!("AttributeError: {name}"))),
                };
            }

            Object::Native(module) if module == "bytes" && name == "fromhex" => {
                return Ok(self.heap.allocate(Object::Native("bytes.fromhex".into())));
            }
            Object::Native(module) if module == "bisect" => {
                let key = match name {
                    "bisect_left" => "bisect.bisect_left",
                    "bisect_right" | "bisect" => "bisect.bisect_right",
                    "insort_left" => "bisect.insort_left",
                    "insort_right" | "insort" => "bisect.insort_right",
                    _ => return Err(error(format!("AttributeError: bisect.{name}"))),
                };
                if let Some(&function) = self.modules.get(key) {
                    return Ok(function);
                }
                let function = self.heap.allocate(Object::Native(key.into()));
                self.modules.insert(key.into(), function);
                return Ok(function);
            }
            Object::Native(module) if module == "json" => {
                let key = match name {
                    "loads" => "json.loads",
                    "dumps" => "json.dumps",
                    _ => return Err(error(format!("AttributeError: json.{name}"))),
                };
                if let Some(&function) = self.modules.get(key) {
                    return Ok(function);
                }
                let function = self.heap.allocate(Object::Native(key.into()));
                self.modules.insert(key.into(), function);
                return Ok(function);
            }
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

    pub(crate) fn iter(&mut self, source: Value) -> Result<Value> {
        if !matches!(source, Value::Ref(_) | Value::InlineString(_)) {
            return Err(Error::Type("object is not iterable"));
        }
        if let Value::InlineString(text) = source {
            return Ok(self.heap.allocate(Object::Iterator {
                source,
                range: None,
                index: 0.into(),
                size: text.as_str().chars().count(),
                exhausted: false,
            }));
        }
        if matches!(
            self.heap.get(source)?,
            Object::Iterator { .. }
                | Object::Enumerate { .. }
                | Object::Zip { .. }
                | Object::Generator { .. }
        ) {
            return Ok(source);
        }
        if self.instance_class(source).is_some() {
            if let Some(iterator) = self.call_special(source, "__iter__", &[])? {
                if matches!(
                    self.heap.get(iterator),
                    Ok(Object::Iterator { .. }
                        | Object::Enumerate { .. }
                        | Object::Zip { .. }
                        | Object::Generator { .. })
                ) || self.special(iterator, "__next__")?.is_some()
                {
                    return Ok(iterator);
                }
                return Err(Error::Type("__iter__ returned a non-iterator"));
            }
            if self.special(source, "__getitem__")?.is_some() {
                return Ok(self.heap.allocate(Object::Iterator {
                    source,
                    range: None,
                    index: 0.into(),
                    size: 0,
                    exhausted: false,
                }));
            }
            return Err(Error::Type("object is not iterable"));
        }
        let size = self.heap.len(source)?;
        let range = match self.heap.get(source)? {
            Object::Range { start, step, .. } => Some((*start, *step)),
            _ => None,
        };
        Ok(self.heap.allocate(Object::Iterator {
            source,
            range,
            index: 0.into(),
            size,
            exhausted: false,
        }))
    }

    /// Successful character production cannot allocate or enter Python. Miss before changing the cursor.
    pub(crate) fn try_string_next(&mut self, iterator: Value) -> Result<Option<Value>> {
        let Object::Iterator {
            source,
            index,
            exhausted: false,
            ..
        } = self.heap.get(iterator)?
        else {
            return Ok(None);
        };
        let Some(text) = self.heap.as_str(source) else {
            return Ok(None);
        };
        let Some(character) = text
            .get(index.get()..)
            .and_then(|suffix| suffix.chars().next())
        else {
            return Ok(None);
        };
        let value = self.heap.string(character.encode_utf8(&mut [0; 4]));
        let mut guard = self.heap.get_mut(iterator)?;
        let Object::Iterator { index, .. } = &mut *guard else {
            unreachable!()
        };
        index.set(index.get() + character.len_utf8());
        Ok(Some(value))
    }

    /// Probe an unfinished builtin range iterator without allocating the end sentinel.
    pub(crate) fn try_range_next(&self, iterator: Value) -> Result<Option<Value>> {
        let Ok(Object::Iterator {
            source,
            range,
            index,
            size,
            exhausted: false,
        }) = self.heap.get(iterator)
        else {
            return Ok(None);
        };
        let position = index.get();
        if position >= *size {
            return Ok(None);
        }
        let (start, step) = if let Some(range) = range {
            *range
        } else if let Ok(Object::Range { start, step, .. }) = self.heap.get(*source) {
            (*start, *step)
        } else {
            return Ok(None);
        };
        let Ok(value) = i64::try_from(i128::from(start) + position as i128 * i128::from(step))
        else {
            return Ok(None);
        };
        index.set(position + 1);
        Ok(Some(Value::Int(value)))
    }

    pub(crate) fn next(&mut self, iterator: Value) -> Result<Value> {
        if let Some(value) = self.try_range_next(iterator)? {
            return Ok(value);
        }
        self.next_at(iterator, 0)
    }

    fn next_at(&mut self, iterator: Value, depth: usize) -> Result<Value> {
        if depth >= 256 {
            return Err(error("RecursionError: iterator nesting"));
        }
        // Builtin iterators cannot recurse; keep stack growth at combinator boundaries.
        let state = match self.heap.get(iterator)? {
            Object::Iterator {
                source,
                index,
                size,
                exhausted,
                ..
            } => Some((*source, index.get(), *size, *exhausted)),
            _ => None,
        };
        let Some((source, index, size, exhausted)) = state else {
            return stacker::maybe_grow(128 * 1024, 2 * 1024 * 1024, || {
                self.next_complex(iterator, depth)
            });
        };
        if self.instance_class(source).is_some() && !exhausted {
            let value = self.py_get_item(source, Value::Int(index as i64));
            return match value {
                Ok(value) => {
                    if let Object::Iterator { index, .. } = &mut *self.heap.get_mut(iterator)? {
                        index.set(index.get() + 1);
                    }
                    Ok(value)
                }
                Err(err)
                    if matches!(err, Error::Index)
                        || self.stop_iteration(&err)
                        || self.error_is(&err, "IndexError") =>
                {
                    self.pending_exception = None;
                    if let Object::Iterator { exhausted, .. } = &mut *self.heap.get_mut(iterator)? {
                        *exhausted = true;
                    }
                    Ok(self.heap.allocate(Object::Done))
                }
                Err(err) => Err(err),
            };
        }
        if exhausted {
            return Ok(self.heap.allocate(Object::Done));
        }
        // String cursors are UTF-8 byte offsets, so each character is decoded once.
        if let Some(text) = self.heap.as_str(&source) {
            let character = text.get(index..).and_then(|suffix| suffix.chars().next());
            let value = match character {
                Some(character) => self.heap.string(character.encode_utf8(&mut [0; 4])),
                None => self.heap.allocate(Object::Done),
            };
            let mut guard = self.heap.get_mut(iterator)?;
            let Object::Iterator {
                index, exhausted, ..
            } = &mut *guard
            else {
                unreachable!()
            };
            if let Some(character) = character {
                index.set(index.get() + character.len_utf8());
            } else {
                *exhausted = true;
            }
            return Ok(value);
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
                range: None,
                index: index.into(),
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
            Object::Set(items) => items[index].1,
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
        index.set(index.get() + 1);
        Ok(value)
    }

    fn next_complex(&mut self, iterator: Value, depth: usize) -> Result<Value> {
        if self.instance_class(iterator).is_some() {
            return match self.call_special(iterator, "__next__", &[]) {
                Ok(Some(value)) => Ok(value),
                Ok(None) => Err(Error::Type("object is not an iterator")),
                Err(err) if self.stop_iteration(&err) => Ok(self.heap.allocate(Object::Done)),
                Err(err) => Err(err),
            };
        }
        if matches!(self.heap.get(iterator)?, Object::Generator { .. }) {
            return self.generator_next(iterator, depth);
        }
        match self.heap.get(iterator)? {
            Object::Enumerate {
                iterator: inner,
                index,
            } => {
                let (inner, index) = (*inner, *index);
                let value = self.next_at(inner, depth + 1)?;
                if self.is_done(value) {
                    return Ok(value);
                }
                let next_index = index.checked_add(1).ok_or(Error::IntegerOverflow)?;
                *self.heap.get_mut(iterator)? = Object::Enumerate {
                    iterator: inner,
                    index: next_index,
                };
                Ok(self
                    .heap
                    .allocate(Object::Tuple(vec![Value::Int(index), value])))
            }
            Object::Zip { iterators, done } => {
                let (iterators, done) = (iterators.clone(), *done);
                if done || iterators.is_empty() {
                    return Ok(self.heap.allocate(Object::Done));
                }
                let roots = self.transient_roots.len();
                self.transient_roots.push(iterator);
                let result = (|| {
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
                        self.transient_roots.push(value);
                        row.push(value);
                    }
                    Ok(self.heap.allocate(Object::Tuple(row)))
                })();
                self.transient_roots.truncate(roots);
                result
            }
            _ => Err(Error::Type("next requires iterator")),
        }
    }

    fn generator_next(&mut self, generator: Value, depth: usize) -> Result<Value> {
        let Object::Generator {
            parts,
            binding,
            running,
            done,
        } = self.heap.get(generator)?
        else {
            unreachable!()
        };
        if *running {
            return Err(error("ValueError: generator already executing"));
        }
        if *done {
            return Ok(self.heap.allocate(Object::Done));
        }
        let parts = parts.clone();
        let binding = binding.clone();
        if let Object::Generator { running, .. } = &mut *self.heap.get_mut(generator)? {
            *running = true;
        }
        let roots = self.transient_roots.len();
        self.transient_roots.push(generator);
        let result = (|| {
            loop {
                let value = self.next_at(parts[0], depth + 1)?;
                if self.is_done(value) {
                    return Ok(value);
                }
                if let Some(binding) = &binding {
                    self.store(binding.0, &binding.1, value)?;
                    self.tier_stats.generator_direct_bindings += 1;
                } else {
                    self.call(parts[1], &[value])?;
                }
                let mut accepted = true;
                for &filter in &parts[3..] {
                    let condition = self.generator_expression(filter)?;
                    if !self.py_truth(condition)? {
                        accepted = false;
                        break;
                    }
                }
                if accepted {
                    return self.generator_expression(parts[2]);
                }
            }
        })();
        let result = result.map_err(|err| match err {
            Error::Runtime(message)
                if message == "StopIteration" || message.starts_with("StopIteration:") =>
            {
                error("RuntimeError: generator raised StopIteration")
            }
            other => other,
        });
        let finished = result.as_ref().map_or(true, |value| self.is_done(*value));
        if let Object::Generator { running, done, .. } = &mut *self.heap.get_mut(generator)? {
            *running = false;
            *done = finished;
        }
        if finished
            && let Object::Generator { parts, binding, .. } = &mut *self.heap.get_mut(generator)?
        {
            *parts = Rc::from([]);
            *binding = None;
        }
        self.transient_roots.truncate(roots);
        result
    }

    fn with_iterator<T>(
        &mut self,
        source: Value,
        body: impl FnOnce(&mut Self, Value) -> Result<T>,
    ) -> Result<T> {
        let iterator = self.iter(source)?;
        let roots = self.transient_roots.len();
        self.transient_roots.push(iterator);
        let result = body(self, iterator);
        self.transient_roots.truncate(roots);
        result
    }

    fn collect_iter(&mut self, source: Value) -> Result<Vec<Value>> {
        let iterator = self.iter(source)?;
        let roots = self.transient_roots.len();
        self.transient_roots.push(iterator);
        let result = (|| {
            let mut values = vec![];
            loop {
                let value = self.next(iterator)?;
                if self.is_done(value) {
                    return Ok(values);
                }
                self.transient_roots.push(value);
                values.push(value);
            }
        })();
        self.transient_roots.truncate(roots);
        result
    }

    fn builtin(
        &mut self,
        name: &str,
        args: &[Value],
        keywords: &[(String, Value)],
    ) -> Result<Value> {
        if let Some(value) = self.attribute_builtin(name, args)? {
            if !keywords.is_empty() {
                return Err(Error::Type("unexpected attribute builtin keywords"));
            }
            return Ok(value);
        }
        if name == "int" {
            return self.construct_integer(args, keywords);
        }
        if name == "bytes" && !keywords.is_empty() {
            let bound = self.bytes_keywords(args, keywords, true)?;
            return self.construct_bytes(&bound);
        }
        if name == "sorted" {
            return self.sorted(args, keywords);
        }
        if name != "print"
            && name != "dict"
            && name != "json.dumps"
            && !name.starts_with("bisect.")
            && !keywords.is_empty()
        {
            return Err(Error::Unsupported("builtin keyword arguments"));
        }
        Ok(match name {
            "print" => {
                let mut sep = " ".to_owned();
                let mut end = "\n".to_owned();
                for (name, value) in keywords {
                    let Some(text) = self.heap.as_str(value) else {
                        return Err(Error::Type("print separator must be string"));
                    };
                    match name.as_ref() {
                        "sep" => sep = text.to_owned(),
                        "end" => end = text.to_owned(),
                        _ => return Err(error("unexpected print keyword")),
                    }
                }
                let parts = args
                    .iter()
                    .map(|&value| self.py_repr(value, false))
                    .collect::<Result<Vec<_>>>()?;
                self.output.push_str(&parts.join(&sep));
                self.output.push_str(&end);
                Value::None
            }
            name if name.starts_with("bisect.") => return self.bisect(name, args, keywords),
            "json.loads" | "json.dumps" => {
                arity(args, 1, 1)?;
                if name == "json.loads" {
                    crate::json::loads(&self.heap, args[0])?
                } else {
                    let mut options = crate::json::DumpOptions::default();
                    for (name, value) in keywords {
                        match name.as_str() {
                            "ensure_ascii" => {
                                options.ensure_ascii = runtime::truth(&self.heap, *value)?
                            }
                            "sort_keys" => options.sort_keys = runtime::truth(&self.heap, *value)?,
                            _ => return Err(Error::Unsupported("json.dumps keyword")),
                        }
                    }
                    crate::json::dumps(&self.heap, args[0], options)?
                }
            }
            "chr" => {
                arity(args, 1, 1)?;
                let code = u32::try_from(integer(args[0])?).ok();
                let character = code
                    .and_then(char::from_u32)
                    .ok_or_else(|| error("ValueError: invalid Unicode scalar"))?;
                self.heap.string(character.encode_utf8(&mut [0; 4]))
            }
            "ord" => {
                arity(args, 1, 1)?;
                let Some(text) = self.heap.as_str(&(args[0])) else {
                    return Err(Error::Type("ord requires a string"));
                };
                let mut chars = text.chars();
                let character = chars
                    .next()
                    .filter(|_| chars.next().is_none())
                    .ok_or(Error::Type("ord requires one character"))?;
                Value::Int(i64::from(u32::from(character)))
            }
            "slice" => {
                arity(args, 1, 3)?;
                let (start, stop, step) = match args {
                    [stop] => (Value::None, *stop, Value::None),
                    [start, stop] => (*start, *stop, Value::None),
                    [start, stop, step] => (*start, *stop, *step),
                    _ => unreachable!(),
                };
                self.heap.allocate(Object::Slice { start, stop, step })
            }
            "type" => {
                arity(args, 1, 1)?;
                self.python_type(args[0])?
            }
            "super" => {
                arity(args, 2, 2)?;
                self.class_mro(args[0])?;
                let actual = if matches!(self.heap.get(args[1]), Ok(Object::Class { .. })) {
                    args[1]
                } else {
                    self.instance_class(args[1])
                        .ok_or(Error::Type("super receiver must be instance or subclass"))?
                };
                if !self.class_mro(actual)?.contains(&args[0]) {
                    return Err(Error::Type("super receiver is not a subtype"));
                }
                self.heap.allocate(Object::Super {
                    anchor: args[0],
                    receiver: args[1],
                })
            }
            "issubclass" => {
                arity(args, 2, 2)?;
                Value::Bool(self.subclass(args[0], args[1])?)
            }
            "hash" => {
                arity(args, 1, 1)?;
                Value::Int(hashing::key_hash(&self.py_key(args[0])?))
            }
            "len" => {
                arity(args, 1, 1)?;
                Value::Int(self.py_len(args[0])?)
            }
            "BaseException.__init__" => {
                arity(args, 1, usize::MAX)?;
                let arguments = self.heap.allocate(Object::Tuple(args[1..].to_vec()));
                self.default_set_attribute(args[0], "args", arguments)?;
                Value::None
            }
            "range" => {
                arity(args, 1, 3)?;
                let (start, stop, step) = match args {
                    [stop] => (0, self.py_index(*stop)?, 1),
                    [start, stop] => (self.py_index(*start)?, self.py_index(*stop)?, 1),
                    [start, stop, step] => (
                        self.py_index(*start)?,
                        self.py_index(*stop)?,
                        self.py_index(*step)?,
                    ),
                    _ => unreachable!(),
                };
                if step == 0 {
                    return Err(error("ValueError: range step cannot be zero"));
                }
                self.heap.allocate(Object::Range { start, stop, step })
            }
            "list" | "tuple" => {
                arity(args, 0, 1)?;
                let items = if args.is_empty() {
                    vec![]
                } else {
                    self.collect_iter(args[0])?
                };
                if name == "list" {
                    self.heap.allocate(Object::List(items.into()))
                } else {
                    self.heap.allocate(Object::Tuple(items))
                }
            }
            "set" => {
                arity(args, 0, 1)?;
                let set = self.heap.allocate(Object::Set(Default::default()));
                if let Some(&source) = args.first() {
                    let roots = self.transient_roots.len();
                    self.transient_roots.push(set);
                    let result = self.with_iterator(source, |vm, iterator| {
                        loop {
                            let value = vm.next(iterator)?;
                            if vm.is_done(value) {
                                return Ok(());
                            }
                            vm.set_method(set, "add", &[value])?;
                        }
                    });
                    self.transient_roots.truncate(roots);
                    result?;
                }
                set
            }
            "dict" => {
                arity(args, 0, 1)?;
                let dict = self.heap.allocate(Object::Dict(Default::default()));
                if let Some(&source) = args.first() {
                    self.method(dict, "update", &[source])?;
                }
                for (name, value) in keywords {
                    let key = text_heap(&self.heap, name.clone());
                    self.py_set_item(dict, key, *value)?;
                }
                dict
            }
            "iter" => {
                arity(args, 1, 1)?;
                self.iter(args[0])?
            }
            "next" => {
                arity(args, 1, 2)?;
                if self.instance_class(args[0]).is_some() {
                    return match self.call_special(args[0], "__next__", &[]) {
                        Ok(Some(value)) => Ok(value),
                        Ok(None) => Err(Error::Type("object is not an iterator")),
                        Err(err) if args.len() == 2 && self.stop_iteration(&err) => Ok(args[1]),
                        Err(err) => Err(err),
                    };
                }
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
            "bytes" => self.construct_bytes(args)?,
            "bytes.fromhex" => self.bytes_fromhex(args)?,
            "int" => self.construct_integer(args, keywords)?,
            "float" => {
                arity(args, 0, 1)?;
                if let Some(&value) = args.first()
                    && self.instance_class(value).is_some()
                {
                    if let Some(converted) = self.call_special(value, "__float__", &[])? {
                        return match converted {
                            Value::Float(_) => Ok(converted),
                            _ => Err(Error::Type("__float__ must return float")),
                        };
                    }
                    let value = self.py_index_value(value)?;
                    return crate::integer::number(&self.heap, value).map(Value::Float);
                }
                let value = args.first().copied().unwrap_or(Value::Float(0.0));
                Value::Float(if self.heap.as_str(&value).is_some() {
                    let Some(text) = self.heap.as_str(&(value)) else {
                        return Err(Error::Type("invalid float"));
                    };
                    text.parse()
                        .map_err(|_| error("ValueError: invalid float"))?
                } else {
                    crate::integer::number(&self.heap, value)?
                })
            }
            "bool" => {
                arity(args, 0, 1)?;
                Value::Bool(if args.is_empty() {
                    false
                } else {
                    self.py_truth(args[0])?
                })
            }
            "str" | "repr" => {
                arity(args, 0, 1)?;
                let text = if args.is_empty() {
                    String::new()
                } else {
                    self.py_repr(args[0], name == "repr")?
                };
                self.heap.string_owned(text)
            }
            "math.sqrt" => ScalarBuiltin::Sqrt.evaluate(&self.heap, args)?,
            "math.sin" | "math.cos" | "math.log" => {
                arity(args, 1, 1)?;
                let value = number(args[0])?;
                if (name == "math.log" && value <= 0.0)
                    || (matches!(name, "math.sin" | "math.cos") && value.is_infinite())
                {
                    return Err(error("ValueError: math domain error"));
                }
                Value::Float(match name {
                    "math.sin" => value.sin(),
                    "math.log" => value.ln(),
                    _ => value.cos(),
                })
            }
            "hex" | "oct" | "bin" => {
                arity(args, 1, 1)?;
                let n = self.integer_argument(args[0])?;
                let n = crate::integer::get(&self.heap, n)?;
                let (base, prefix) = match name {
                    "hex" => (16, "0x"),
                    "oct" => (8, "0o"),
                    _ => (2, "0b"),
                };
                use num_traits::Signed;
                self.heap.string_owned(format!(
                    "{}{prefix}{}",
                    if n.is_negative() { "-" } else { "" },
                    n.abs().to_str_radix(base)
                ))
            }
            "divmod" => {
                arity(args, 2, 2)?;
                self.protocol_roots(args, |vm| vm.py_divmod(args[0], args[1]))?
            }
            "pow" => {
                arity(args, 2, 3)?;
                if args.get(2).is_none_or(|n| *n == Value::None) {
                    self.py_binary("**", args[0], args[1])?
                } else {
                    crate::integer::modular_power(&self.heap, args[0], args[1], args[2])?
                }
            }
            "abs" => {
                arity(args, 1, 1)?;
                if let Some(value) = self.call_special(args[0], "__abs__", &[])? {
                    return Ok(value);
                }
                if crate::integer::is_integer(&self.heap, args[0]) {
                    return crate::integer::unary(&self.heap, "abs", args[0]);
                }
                match args[0] {
                    Value::Int(n) => Value::Int(n.checked_abs().ok_or(Error::IntegerOverflow)?),
                    value => Value::Float(number(value)?.abs()),
                }
            }
            "sum" => {
                arity(args, 1, 2)?;
                let start = args.get(1).copied().unwrap_or(Value::Int(0));
                if self.heap.as_str(&start).is_some() {
                    return Err(Error::Type("sum cannot start with a string"));
                }
                self.with_iterator(args[0], |vm, iterator| {
                    let slot = vm.transient_roots.len();
                    vm.transient_roots.push(start);
                    loop {
                        let value = vm.next(iterator)?;
                        if vm.is_done(value) {
                            return Ok(vm.transient_roots[slot]);
                        }
                        vm.transient_roots[slot] = vm.protocol_roots(&[value], |vm| {
                            vm.py_binary("+", vm.transient_roots[slot], value)
                        })?;
                    }
                })?
            }
            "min" | "max" => {
                arity(args, 1, usize::MAX)?;
                let source = if args.len() == 1 {
                    args[0]
                } else {
                    self.heap.allocate(Object::Tuple(args.to_vec()))
                };
                self.with_iterator(source, |vm, iterator| {
                    let first = vm.next(iterator)?;
                    if vm.is_done(first) {
                        return Err(error("ValueError: empty sequence"));
                    }
                    let slot = vm.transient_roots.len();
                    vm.transient_roots.push(first);
                    loop {
                        let value = vm.next(iterator)?;
                        if vm.is_done(value) {
                            return Ok(vm.transient_roots[slot]);
                        }
                        if vm.protocol_roots(&[value], |vm| {
                            vm.py_compare_bool(
                                if name == "min" { "<" } else { ">" },
                                value,
                                vm.transient_roots[slot],
                            )
                        })? {
                            vm.transient_roots[slot] = value;
                        }
                    }
                })?
            }
            "all" | "any" => {
                arity(args, 1, 1)?;
                let result = self.with_iterator(args[0], |vm, iterator| {
                    loop {
                        let value = vm.next(iterator)?;
                        if vm.is_done(value) {
                            return Ok(name == "all");
                        }
                        let truth = vm.protocol_roots(&[value], |vm| vm.py_truth(value))?;
                        if truth == (name == "any") {
                            return Ok(truth);
                        }
                    }
                })?;
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
            "reversed" => {
                arity(args, 1, 1)?;
                let mut items = self.collect_iter(args[0])?;
                items.reverse();
                let list = self.heap.allocate(Object::List(items.into()));
                self.iter(list)?
            }
            "isinstance" => {
                arity(args, 2, 2)?;
                Value::Bool(self.instance_of(args[0], args[1])?)
            }
            "object" => {
                arity(args, 0, 0)?;
                return Err(Error::Unsupported("object()"));
            }
            name if is_exception(name) => self.construct_exception(name, args),
            _ => return Err(error(format!("unknown builtin {name}"))),
        })
    }

    #[cold]
    #[inline(never)]
    fn construct_exception(&self, name: &str, args: &[Value]) -> Value {
        self.heap
            .allocate(Object::Exception(Box::new(ExceptionData {
                kind: name.into(),
                arguments: self.heap.allocate(Object::Tuple(args.to_vec())),
                attributes: BTreeMap::new(),
                cause: None,
                context: None,
                suppress_context: false,
                traceback: None,
            })))
    }

    #[cold]
    #[inline(never)]
    fn raise_exception(&mut self, values: &[Value]) -> Result<Value> {
        self.raise_python_exception(values, false)
    }

    pub(crate) fn exception_from_error(&mut self, err: Error) -> Result<Value> {
        let kind = match &err {
            Error::Type(_) | Error::Arguments { .. } => "TypeError",
            Error::Index => "IndexError",
            Error::MissingKey => "KeyError",
            Error::IntegerOverflow => "OverflowError",
            Error::Uninitialized(_) => "UnboundLocalError",
            Error::Runtime(message) => {
                if message.starts_with("JSON error:") {
                    "ValueError"
                } else {
                    message
                        .split(':')
                        .next()
                        .filter(|kind| is_exception(kind))
                        .unwrap_or("RuntimeError")
                }
            }
            _ => return Err(err),
        };
        let message = if let Error::Type(reason) = &err {
            (*reason).to_owned()
        } else {
            err.to_string()
        };
        let text = message
            .strip_prefix(kind)
            .and_then(|s| s.strip_prefix(": "))
            .unwrap_or(&message);
        let args = if message == kind {
            vec![]
        } else {
            vec![self.heap.string(text)]
        };
        let value = self.construct_exception(kind, &args);
        self.attach_context(value)?;
        self.record_traceback(value, false)?;
        Ok(value)
    }

    fn exception_matches(&self, value: Value, class: Value) -> Result<bool> {
        let invalid = || Error::Type("catching classes that do not inherit from BaseException");
        let valid = |class| self.exception_class(class);
        if let Ok(Object::Tuple(classes)) = self.heap.get(class) {
            // Python validates the entire flat tuple before testing any matching class.
            if !classes.iter().copied().all(valid) {
                return Err(invalid());
            }
            for &class in classes {
                if self.instance_of(value, class)? {
                    return Ok(true);
                }
            }
            Ok(false)
        } else if valid(class) {
            self.instance_of(value, class)
        } else {
            Err(invalid())
        }
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
                name if is_exception(name) => self.is_exception_instance(value, name),
                "int" => crate::integer::is_integer(&self.heap, value),
                "bool" => matches!(value, Value::Bool(_)),
                "float" => matches!(value, Value::Float(_)),
                "object" => true,
                "list" => matches!(self.heap.get(value), Ok(Object::List(_))),
                "tuple" => matches!(self.heap.get(value), Ok(Object::Tuple(_))),
                "dict" => matches!(self.heap.get(value), Ok(Object::Dict(_))),
                "set" => matches!(self.heap.get(value), Ok(Object::Set(_))),
                "str" => self.heap.as_str(&value).is_some(),
                name if types::builtin_type_name(name) => self.value_type_name(value)? == name,
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
                if receiver == args[0] {
                    // The self-extension case consumes the original list exactly once.
                    let values = self.collect_iter(receiver)?;
                    if let Object::List(items) = &mut *self.heap.get_mut(receiver)? {
                        for value in values {
                            items.push(value);
                        }
                    }
                } else {
                    let roots = self.transient_roots.len();
                    self.transient_roots.push(receiver);
                    let result = self.with_iterator(args[0], |vm, iterator| {
                        loop {
                            let value = vm.next(iterator)?;
                            if vm.is_done(value) {
                                return Ok(());
                            }
                            if let Object::List(items) = &mut *vm.heap.get_mut(receiver)? {
                                items.push(value);
                            }
                        }
                    });
                    self.transient_roots.truncate(roots);
                    result?;
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
                let mut count = 0;
                let mut found = None;
                let mut i = 0;
                loop {
                    let value = match self.heap.get(receiver)? {
                        Object::List(items) => items.value(i),
                        _ => unreachable!(),
                    };
                    let Some(value) = value else {
                        break;
                    };
                    if value == args[0]
                        || self.protocol_roots(&[value], |vm| {
                            vm.py_compare_bool("==", value, args[0])
                        })?
                    {
                        count += 1;
                        found = Some(i);
                        if name != "count" {
                            break;
                        }
                    }
                    i += 1;
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
            "get" | "setdefault" | "pop" => self.mapping_method(receiver, name, args),
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
                if let Object::Dict(items) = self.heap.get(args[0])? {
                    let pairs = items
                        .iter()
                        .map(|(key, value)| (key_value(&self.heap, key), *value))
                        .collect::<Vec<_>>();
                    let roots = pairs
                        .iter()
                        .flat_map(|(k, v)| [*k, *v])
                        .chain(std::iter::once(receiver))
                        .collect::<Vec<_>>();
                    self.protocol_roots(&roots, |vm| {
                        for &(key, value) in &pairs {
                            vm.py_set_item(receiver, key, value)?;
                        }
                        Ok(())
                    })?;
                    return Ok(Value::None);
                }
                let roots = self.transient_roots.len();
                self.transient_roots.push(receiver);
                let result = self.with_iterator(args[0], |vm, iterator| {
                    loop {
                        let item = vm.next(iterator)?;
                        if vm.is_done(item) {
                            return Ok(Value::None);
                        }
                        let pair = vm.with_iterator(item, |vm, row| {
                            let mut pair = vec![];
                            for _ in 0..3 {
                                let value = vm.next(row)?;
                                if vm.is_done(value) {
                                    break;
                                }
                                vm.transient_roots.push(value);
                                pair.push(value);
                            }
                            Ok(pair)
                        })?;
                        if pair.len() != 2 {
                            return Err(error("ValueError: dictionary pair length"));
                        }
                        vm.py_set_item(receiver, pair[0], pair[1])?;
                    }
                });
                self.transient_roots.truncate(roots);
                result
            }

            _ => Err(error("unknown dictionary method")),
        }
    }

    fn set_method(&mut self, receiver: Value, name: &str, args: &[Value]) -> Result<Value> {
        match name {
            "add" | "discard" | "remove" => {
                arity(args, 1, 1)?;
                self.protocol_roots(&[receiver, args[0]], |vm| {
                    if name == "add" {
                        vm.mapping_set(receiver, args[0], args[0], true)?;
                    } else {
                        let key = vm.py_key(args[0])?;
                        if let Some(key) = vm.mapping_find(receiver, &key)? {
                            if let Object::Set(items) = &mut *vm.heap.get_mut(receiver)? {
                                items.remove(&key);
                            }
                        } else if name == "remove" {
                            return Err(Error::MissingKey);
                        }
                    }
                    Ok(Value::None)
                })
            }
            "clear" => {
                arity(args, 0, 0)?;
                if let Object::Set(items) = &mut *self.heap.get_mut(receiver)? {
                    items.clear();
                }
                Ok(Value::None)
            }
            "copy" => {
                arity(args, 0, 0)?;
                let Object::Set(items) = self.heap.get(receiver)? else {
                    unreachable!()
                };
                Ok(self.heap.allocate(Object::Set(items.clone())))
            }
            _ => Err(error("unknown set method")),
        }
    }

    fn method(&mut self, receiver: Value, name: &str, args: &[Value]) -> Result<Value> {
        if crate::integer::is_integer(&self.heap, receiver) {
            return self.integer_method(receiver, name, args);
        }
        if matches!(self.heap.get(receiver), Ok(Object::Bytes(_))) {
            return self.bytes_method(receiver, name, args);
        }
        if name == "encode"
            && let Some(text) = self.heap.as_str(&receiver)
        {
            let text = text.to_owned();
            return self.encode_bytes(&text, args);
        }
        if let Ok(Object::Property {
            get,
            set,
            delete,
            doc,
        }) = self.heap.get(receiver)
        {
            let (mut get, mut set, mut delete, doc) = (*get, *set, *delete, *doc);
            arity(args, 1, 1)?;
            match name {
                "getter" => get = args[0],
                "setter" => set = args[0],
                "deleter" => delete = args[0],
                _ => return Err(Error::Type("property method")),
            }
            return Ok(self.heap.allocate(Object::Property {
                get,
                set,
                delete,
                doc,
            }));
        }
        if let Value::InlineString(text) = receiver {
            return self.string_method(text.as_str(), name, args);
        }
        match self.heap.get(receiver)? {
            Object::List(_) => return self.list_method(receiver, name, args),
            Object::Dict(_) => return self.dict_method(receiver, name, args),
            Object::Set(_) => return self.set_method(receiver, name, args),
            _ => {}
        }
        let object = self.heap.get(receiver)?.clone();
        match object {
            Object::String(text) => self.string_method(&text, name, args),
            _ => Err(Error::Type("unsupported method")),
        }
    }

    fn string_method(&mut self, text: &str, name: &str, args: &[Value]) -> Result<Value> {
        let result = match name {
            "join" => {
                arity(args, 1, 1)?;
                match self.heap.get(args[0]) {
                    Ok(Object::List(items)) => {
                        if let Some(values) = items.native_values() {
                            return join_values(&self.heap, text, values);
                        }
                        if items.is_empty() {
                            return Ok(self.heap.string(""));
                        }
                        return Err(Error::Type("join requires strings"));
                    }
                    Ok(Object::Tuple(values)) => return join_values(&self.heap, text, values),
                    _ => {}
                }
                let values = self.collect_iter(args[0])?;
                join_values(&self.heap, text, &values)?
            }
            "split" => {
                arity(args, 0, 2)?;
                let limit = args
                    .get(1)
                    .map(|&value| integer(value))
                    .transpose()?
                    .unwrap_or(-1);
                let parts = if args.is_empty() || args[0] == Value::None {
                    let mut rest = text.trim_start();
                    let mut parts = vec![];
                    while !rest.is_empty() {
                        if limit >= 0 && parts.len() == limit as usize {
                            parts.push(rest.to_owned());
                            break;
                        }
                        let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
                        parts.push(rest[..end].to_owned());
                        rest = rest[end..].trim_start();
                    }
                    parts
                } else {
                    let Some(sep) = self.heap.as_str(&(args[0])) else {
                        return Err(Error::Type("separator must be string"));
                    };
                    if sep.is_empty() {
                        return Err(error("ValueError: empty separator"));
                    }
                    if limit < 0 {
                        text.split(sep).map(str::to_owned).collect()
                    } else {
                        text.splitn(limit as usize + 1, sep)
                            .map(str::to_owned)
                            .collect()
                    }
                };
                self.heap.allocate(Object::List(
                    parts
                        .into_iter()
                        .map(|s| text_heap(&self.heap, s))
                        .collect::<Vec<_>>()
                        .into(),
                ))
            }
            "splitlines" => {
                arity(args, 0, 1)?;
                let keep = args
                    .first()
                    .map(|&value| runtime::truth(&self.heap, value))
                    .transpose()?
                    .unwrap_or(false);
                let mut parts = vec![];
                let mut start = 0;
                let mut chars = text.char_indices().peekable();
                // str::lines omits Python's additional Unicode line boundaries.
                while let Some((index, ch)) = chars.next() {
                    if matches!(
                        ch,
                        '\n' | '\r' | '\u{000b}' | '\u{000c}' | '\u{001c}'
                            ..='\u{001e}' | '\u{0085}' | '\u{2028}' | '\u{2029}'
                    ) {
                        let mut end = index + ch.len_utf8();
                        if ch == '\r' && chars.peek().is_some_and(|&(_, next)| next == '\n') {
                            end = chars.next().unwrap().0 + 1;
                        }
                        parts.push(text_heap(
                            &self.heap,
                            text[start..if keep { end } else { index }].into(),
                        ));
                        start = end;
                    }
                }
                if start < text.len() {
                    parts.push(text_heap(&self.heap, text[start..].into()));
                }
                self.heap.allocate(Object::List(parts.into()))
            }
            "rjust" => {
                arity(args, 1, 2)?;
                let width = integer(args[0])?;
                let fill = if let Some(&value) = args.get(1) {
                    let Some(fill) = self.heap.as_str(&(value)) else {
                        return Err(Error::Type("fill character must be a string"));
                    };
                    if fill.chars().count() != 1 {
                        return Err(Error::Type("fill character must be exactly one character"));
                    }
                    fill.chars().next().unwrap()
                } else {
                    ' '
                };
                let padding = width.saturating_sub(text.chars().count() as i64).max(0) as usize;
                let mut result = String::new();
                result
                    .try_reserve(
                        padding
                            .checked_mul(fill.len_utf8())
                            .and_then(|n| n.checked_add(text.len()))
                            .ok_or(Error::IntegerOverflow)?,
                    )
                    .map_err(|_| error("MemoryError: string too large"))?;
                result.extend(std::iter::repeat_n(fill, padding));
                result.push_str(text);
                text_heap(&self.heap, result)
            }
            "isdigit" => {
                arity(args, 0, 0)?;
                Value::Bool(!text.is_empty() && text.chars().all(crate::unicode::is_digit))
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
                let Some(part) = self.heap.as_str(&(args[0])) else {
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
                let (Some(from), Some(to)) =
                    (self.heap.as_str(&args[0]), self.heap.as_str(&args[1]))
                else {
                    return Err(Error::Type("replace requires strings"));
                };
                text_heap(&self.heap, text.replace(from, to))
            }
            _ => return Err(error("unknown string method")),
        };
        Ok(result)
    }

    fn repr(&self, value: Value, quote: bool) -> Result<String> {
        render(&self.heap, value, quote, &mut vec![])
    }
}

fn text_heap(heap: &Heap, text: String) -> Value {
    heap.string_owned(text)
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
        Key::Protocol(value, _) => *value,
        Key::None => Value::None,
        Key::Bool(value) => Value::Bool(*value),
        Key::Int(n) => Value::Int(*n),
        Key::BigInt(n) => n.0,
        Key::String(s) => heap.string(s),
        Key::Bytes(data) => data.0,
        Key::Float(bits) => Value::Float(f64::from_bits(*bits)),
        Key::Tuple(keys) => stacker::maybe_grow(128 * 1024, 2 * 1024 * 1024, || {
            heap.allocate(Object::Tuple(
                keys.iter().map(|key| key_value(heap, key)).collect(),
            ))
        }),
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

fn slice_limits(heap: &Heap, key: Value, len: usize) -> Result<(i128, i128, i128)> {
    let Object::Slice { start, stop, step } = *heap.get(key)? else {
        return Err(Error::Type("expected slice"));
    };
    let len = len as i128;
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
    let i = bound(start, if step > 0 { 0 } else { len - 1 })?;
    let stop = bound(stop, if step > 0 { len } else { -1 })?;
    Ok((i, stop, step))
}
fn slice_positions(mut start: i128, stop: i128, step: i128) -> Vec<usize> {
    let mut indices = vec![];
    while if step > 0 { start < stop } else { start > stop } {
        indices.push(start as usize);
        start += step;
    }
    indices
}
pub(crate) fn slice(heap: &Heap, object: Value, key: Value) -> Result<Value> {
    if let Some(text) = heap.as_str(&object) {
        let ascii = heap.string_is_ascii(object)?;
        let len = if ascii {
            text.len()
        } else {
            text.chars().count()
        };
        let (start, stop, step) = slice_limits(heap, key, len)?;
        if (step > 0 && start >= stop) || (step < 0 && start <= stop) {
            return Ok(heap.string(""));
        }
        // ASCII positions are byte offsets. Short contiguous results stay inline without a
        // character array or temporary output String; Unicode paths iterate code points.
        if ascii && step == 1 {
            return Ok(heap.string(&text[start as usize..stop as usize]));
        }
        let stride = usize::try_from(step.unsigned_abs()).unwrap_or(usize::MAX);
        let result = if ascii {
            let bytes = text.as_bytes();
            if step > 0 {
                bytes[start as usize..stop as usize]
                    .iter()
                    .step_by(stride)
                    .map(|&byte| char::from(byte))
                    .collect()
            } else {
                bytes[(stop + 1) as usize..(start + 1) as usize]
                    .iter()
                    .rev()
                    .step_by(stride)
                    .map(|&byte| char::from(byte))
                    .collect()
            }
        } else if step > 0 {
            text.chars()
                .skip(start as usize)
                .take((stop - start) as usize)
                .step_by(stride)
                .collect()
        } else {
            text.chars()
                .rev()
                .skip((len as i128 - 1 - start) as usize)
                .take((start - stop) as usize)
                .step_by(stride)
                .collect()
        };
        return Ok(heap.string_owned(result));
    }
    let (start, stop, step) = slice_limits(heap, key, heap.len(object)?)?;
    let indices = slice_positions(start, stop, step);
    Ok(match heap.get(object)? {
        Object::List(items) => heap.allocate(Object::List(
            indices.iter().map(|&i| items[i]).collect::<Vec<_>>().into(),
        )),
        Object::Tuple(items) => {
            heap.allocate(Object::Tuple(indices.iter().map(|&i| items[i]).collect()))
        }
        Object::Bytes(data) => {
            heap.allocate(Object::Bytes(indices.iter().map(|&i| data[i]).collect()))
        }
        _ => return Err(Error::Type("object does not support slicing")),
    })
}

fn binary(heap: &Heap, op: &str, a: Value, b: Value) -> Result<Value> {
    if op == "*" {
        let pair = if crate::integer::is_big(heap, a) {
            Some((b, a))
        } else if crate::integer::is_big(heap, b) {
            Some((a, b))
        } else {
            None
        };
        if let Some((sequence, _count)) = pair
            && (heap.as_str(&sequence).is_some()
                || matches!(
                    heap.get(sequence),
                    Ok(Object::List(_) | Object::Tuple(_) | Object::Bytes(_))
                ))
        {
            return Err(Error::IntegerOverflow);
        }
    }
    if crate::integer::is_big(heap, a)
        || crate::integer::is_big(heap, b)
        || matches!(op, "<<" | ">>")
    {
        return crate::integer::binary(heap, op, a, b);
    }
    if op == "*" {
        if matches!(a, Value::Ref(_) | Value::InlineString(_))
            && let Ok(n) = integer(b)
        {
            return repeat(heap, a, n);
        }
        if matches!(b, Value::Ref(_) | Value::InlineString(_))
            && let Ok(n) = integer(a)
        {
            return repeat(heap, b, n);
        }
    }
    if let (Ok(a), Ok(b)) = (integer(a), integer(b)) {
        if op == "/" {
            return crate::integer::binary(heap, op, Value::Int(a), Value::Int(b));
        }
        let result = match op {
            "*" => a.checked_mul(b),
            "//" => {
                if b == 0 {
                    return Err(error("ZeroDivisionError"));
                }
                let Some(q) = a.checked_div(b) else {
                    return crate::integer::binary(heap, op, Value::Int(a), Value::Int(b));
                };
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
                let Some(r) = a.checked_rem(b) else {
                    return Ok(Value::Int(0));
                };
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
            return match result {
                Some(n) => Ok(Value::Int(n)),
                None => crate::integer::binary(heap, op, Value::Int(a), Value::Int(b)),
            };
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
        "//" => crate::integer::float_divmod(a, b)?.0,
        "%" => crate::integer::float_divmod(a, b)?.1,
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
    if let Some(text) = heap.as_str(&value) {
        return Ok(text_heap(heap, text.repeat(n)));
    }
    Ok(match heap.get(value)? {
        Object::String(s) => text_heap(heap, s.repeat(n)),
        Object::List(items) => heap.allocate(Object::List(items.repeat(n))),
        Object::Tuple(items) => heap.allocate(Object::Tuple(items.repeat(n))),
        Object::Bytes(items) => heap.allocate(Object::Bytes(items.repeat(n))),
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
    if let Some(ordering) = crate::integer::order(heap, a, b).or_else(|| numeric_order(a, b)) {
        return Ok(ordering == Some(std::cmp::Ordering::Equal));
    }
    if let (Some(a), Some(b)) = (heap.as_str(&a), heap.as_str(&b)) {
        return Ok(a == b);
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
            (Object::Bytes(a), Object::Bytes(b)) => return Ok(a == b),
            (Object::Set(a), Object::Set(b)) => {
                return Ok(a.len() == b.len() && a.iter().all(|(key, _)| b.get(key).is_some()));
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
pub(crate) fn compare(heap: &Heap, op: &str, a: Value, b: Value) -> Result<bool> {
    match op {
        "==" => return equal(heap, a, b, 0),
        "!=" => return equal(heap, a, b, 0).map(|v| !v),
        "is" => return Ok(a == b),
        "is not" => return Ok(a != b),
        "in" | "not in" => {
            if let Some(text) = heap.as_str(&b) {
                let part = heap
                    .as_str(&a)
                    .ok_or(Error::Type("string membership requires string"))?;
                return Ok(text.contains(part) == (op == "in"));
            }
            let found = match heap.get(b)? {
                Object::Dict(items) | Object::Set(items) => heap.dict_get(items, a)?.is_some(),
                object @ (Object::List(_) | Object::Tuple(_)) => {
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
                Object::Bytes(data) => {
                    if let Ok(n) = integer(a) {
                        let byte = u8::try_from(n)
                            .map_err(|_| error("ValueError: byte must be in range(0, 256)"))?;
                        data.contains(&byte)
                    } else if let Ok(Object::Bytes(part)) = heap.get(a) {
                        part.is_empty() || data.windows(part.len()).any(|v| v == part)
                    } else {
                        return Err(Error::Type("a bytes-like object is required"));
                    }
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
    if let Value::InlineString(text) = value {
        return Ok(if quote {
            format!(
                "'{}'",
                text.as_str()
                    .replace('\\', "\\\\")
                    .replace('\'', "\\'")
                    .replace('\n', "\\n")
                    .replace('\r', "\\r")
                    .replace('\t', "\\t")
            )
        } else {
            text.as_str().into()
        });
    }
    Ok(match value {
        Value::InlineString(_) => unreachable!(),
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
                Object::Bytes(data) => bytes::repr(data),
                Object::BigInt(n) => n.to_string(),
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
                object @ (Object::List(_) | Object::Tuple(_)) => {
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
                        _ => format!("[{}]", parts.join(", ")),
                    }
                }
                Object::Set(items) => {
                    let parts = items
                        .iter()
                        .map(|(_, value)| render(heap, *value, true, active))
                        .collect::<Result<Vec<_>>>()?;
                    if parts.is_empty() {
                        "set()".into()
                    } else {
                        format!("{{{}}}", parts.join(", "))
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
                Object::Exception(exception) => {
                    let kind = &exception.kind;
                    let Object::Tuple(arguments) = heap.get(exception.arguments)? else {
                        return Err(Error::InvalidIr("exception arguments"));
                    };
                    if quote {
                        let parts = arguments
                            .iter()
                            .map(|&arg| render(heap, arg, true, active))
                            .collect::<Result<Vec<_>>>()?;
                        format!("{kind}({})", parts.join(", "))
                    } else {
                        match arguments.as_slice() {
                            [] => String::new(),
                            [arg] => render(heap, *arg, false, active)?,
                            args => {
                                let parts = args
                                    .iter()
                                    .map(|&arg| render(heap, arg, true, active))
                                    .collect::<Result<Vec<_>>>()?;
                                format!("({})", parts.join(", "))
                            }
                        }
                    }
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
    if let Some(order) = crate::integer::order(heap, a, b).or_else(|| numeric_order(a, b)) {
        return Ok(order);
    }
    if let (Some(a), Some(b)) = (heap.as_str(&a), heap.as_str(&b)) {
        return Ok(Some(a.cmp(b)));
    }
    match (heap.get(a), heap.get(b)) {
        (Ok(Object::String(a)), Ok(Object::String(b))) => Ok(Some(a.cmp(b))),
        (Ok(Object::Bytes(a)), Ok(Object::Bytes(b))) => Ok(Some(a.cmp(b))),
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

fn is_exception(name: &str) -> bool {
    name == "BaseException" || exception_parent(name).is_some()
}

// Builtin sequences cannot execute Python while their strings are borrowed. Allocate only the result.
fn join_values(heap: &Heap, separator: &str, values: &[Value]) -> Result<Value> {
    let mut size = separator
        .len()
        .checked_mul(values.len().saturating_sub(1))
        .ok_or(Error::IntegerOverflow)?;
    for value in values {
        let text = heap
            .as_str(value)
            .ok_or(Error::Type("join requires strings"))?;
        size = size.checked_add(text.len()).ok_or(Error::IntegerOverflow)?;
    }
    if let [only] = values {
        return Ok(*only);
    }
    if size <= crate::InlineString::CAPACITY {
        let mut bytes = [0u8; crate::InlineString::CAPACITY];
        let mut offset = 0;
        for (index, value) in values.iter().enumerate() {
            if index != 0 {
                bytes[offset..offset + separator.len()].copy_from_slice(separator.as_bytes());
                offset += separator.len();
            }
            let text = heap.as_str(value).unwrap();
            bytes[offset..offset + text.len()].copy_from_slice(text.as_bytes());
            offset += text.len();
        }
        return Ok(
            heap.string(std::str::from_utf8(&bytes[..offset]).expect("joined UTF-8 fragments"))
        );
    }
    let mut result = String::new();
    result
        .try_reserve_exact(size)
        .map_err(|_| error("MemoryError: string too large"))?;
    for (index, value) in values.iter().enumerate() {
        if index != 0 {
            result.push_str(separator);
        }
        result.push_str(heap.as_str(value).unwrap());
    }
    Ok(heap.string_owned(result))
}
