//! Stable builtin type identities are independent of rebinding their global names.
use super::*;

pub(super) fn builtin_type_name(name: &str) -> bool {
    is_exception(name)
        || matches!(
            name,
            "object"
                | "int"
                | "float"
                | "bool"
                | "str"
                | "bytes"
                | "list"
                | "tuple"
                | "dict"
                | "set"
                | "range"
                | "type"
                | "NoneType"
                | "function"
                | "method"
                | "builtin_function_or_method"
                | "generator"
                | "property"
                | "staticmethod"
                | "classmethod"
                | "super"
                | "slice"
                | "enumerate"
                | "zip"
                | "iterator"
                | "traceback"
                | "NotImplementedType"
        )
}

impl Vm {
    pub(super) fn builtin_type(&mut self, name: &str) -> Value {
        let key = format!("type.{name}");
        if let Some(&value) = self.modules.get(&key) {
            return value;
        }
        let value = self.heap.allocate(Object::Native(name.to_owned().into()));
        self.modules.insert(key, value);
        value
    }

    pub(super) fn value_type_name(&self, value: Value) -> Result<&str> {
        Ok(match value {
            Value::None => "NoneType",
            Value::Bool(_) => "bool",
            Value::Int(_) => "int",
            Value::Float(_) => "float",
            Value::InlineString(_) => "str",
            Value::Unbound => return Err(Error::InvalidIr("unbound type")),
            Value::Ref(_) => match self.heap.get(value)? {
                Object::String(_) => "str",
                Object::Bytes(_) => "bytes",
                Object::BigInt(_) => "int",
                Object::List(_) => "list",
                Object::Tuple(_) => "tuple",
                Object::Dict(_) => "dict",
                Object::Set(_) => "set",
                Object::Range { .. } => "range",
                Object::Function(_) => "function",
                Object::Method(..) => "method",
                Object::Bound(..) => "builtin_function_or_method",
                Object::Generator { .. } => "generator",
                Object::Property { .. } => "property",
                Object::StaticMethod(_) => "staticmethod",
                Object::ClassMethod(_) => "classmethod",
                Object::Super { .. } => "super",
                Object::Slice { .. } => "slice",
                Object::Enumerate { .. } => "enumerate",
                Object::Zip { .. } => "zip",
                Object::Iterator { .. } => "iterator",
                Object::Class { .. } => "type",
                Object::Traceback { .. } => "traceback",
                Object::Exception(exception) => &exception.kind,
                Object::Native(name) if name == "NotImplemented" => "NotImplementedType",
                Object::Native(name) if builtin_type_name(name) => "type",
                Object::Native(_) => "builtin_function_or_method",
                Object::Instance { .. } => "instance",
                _ => return Err(Error::Type("internal value has no Python type")),
            },
        })
    }

    pub(super) fn python_type(&mut self, value: Value) -> Result<Value> {
        if let Some(class) = self.instance_class(value) {
            return Ok(class);
        }
        let name = self.value_type_name(value)?.to_owned();
        Ok(self.builtin_type(&name))
    }
}
