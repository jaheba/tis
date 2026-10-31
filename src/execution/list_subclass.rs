//! List subclasses retain ordinary class identity and separately traced builtin storage.
use super::*;
pub(super) const LIST_METHODS: &[&str] = &[
    "__init__",
    "__len__",
    "__iter__",
    "__getitem__",
    "__setitem__",
    "__contains__",
    "__eq__",
    "__ne__",
    "__lt__",
    "__le__",
    "__gt__",
    "__ge__",
    "append",
    "extend",
    "insert",
    "pop",
    "clear",
    "copy",
    "count",
    "index",
    "remove",
    "reverse",
    "sort",
];
impl Vm {
    pub(super) fn list_storage(&self, object: Value) -> Option<Value> {
        match self.heap.get(object).ok()? {
            Object::List(_) => Some(object),
            Object::Instance { attributes, .. } => attributes.builtin(),
            _ => None,
        }
    }
    pub(super) fn initialize_list_storage(&mut self, instance: Value) -> Result<()> {
        let Some(class) = self.instance_class(instance) else {
            return Ok(());
        };
        let Some(&list) = self.modules.get("type.list") else {
            return Ok(());
        };
        if !self.class_mro(class)?.contains(&list) || self.list_storage(instance).is_some() {
            return Ok(());
        }
        let storage = self.heap.allocate(Object::List(Vec::new().into()));
        if let Object::Instance { attributes, .. } = &mut *self.heap.get_mut(instance)? {
            attributes.set_builtin(storage);
        }
        Ok(())
    }
    pub(super) fn builtin_type_descriptor(&self, kind: &str, name: &str) -> Option<Value> {
        if kind == "list" && name == "__hash__" {
            return Some(Value::None);
        }
        self.modules
            .get(&format!("descriptor.{kind}.{name}"))
            .copied()
    }
    pub(super) fn list_descriptor(
        &mut self,
        name: &str,
        args: &[Value],
        keywords: &[(String, Value)],
    ) -> Result<Value> {
        arity(args, 1, 4)?;
        let storage = self
            .list_storage(args[0])
            .ok_or(Error::Type("list descriptor requires a list instance"))?;
        let operands = &args[1..];
        self.protocol_roots(&[storage], |vm| {
            if name == "sort" {
                return vm.sort_list(storage, operands, keywords);
            }
            if !keywords.is_empty() {
                return Err(Error::Type("list method takes no keyword arguments"));
            }
            match name {
                "__init__" => {
                    arity(operands, 0, 1)?;
                    // Reinitialization clears first, so iterators over the same list observe the mutation.
                    vm.list_method(storage, "clear", &[])?;
                    if !operands.is_empty() {
                        vm.list_method(storage, "extend", operands)?;
                    }
                    Ok(Value::None)
                }
                "__len__" => {
                    arity(operands, 0, 0)?;
                    Ok(Value::Int(vm.heap.len(storage)? as i64))
                }
                "__iter__" => {
                    arity(operands, 0, 0)?;
                    vm.iter(storage)
                }
                "__getitem__" => {
                    arity(operands, 1, 1)?;
                    vm.py_get_item(storage, operands[0])
                }
                "__setitem__" => {
                    arity(operands, 2, 2)?;
                    vm.py_set_item(storage, operands[0], operands[1])?;
                    Ok(Value::None)
                }
                "__contains__" => {
                    arity(operands, 1, 1)?;
                    vm.py_compare("in", operands[0], storage)
                }
                "__eq__" | "__ne__" | "__lt__" | "__le__" | "__gt__" | "__ge__" => {
                    arity(operands, 1, 1)?;
                    let Some(other) = vm.list_storage(operands[0]) else {
                        return Ok(vm.heap.allocate(Object::Native("NotImplemented".into())));
                    };
                    let op = match name {
                        "__eq__" => "==",
                        "__ne__" => "!=",
                        "__lt__" => "<",
                        "__le__" => "<=",
                        "__gt__" => ">",
                        _ => ">=",
                    };
                    vm.py_compare(op, storage, other)
                }
                _ => vm.list_method(storage, name, operands),
            }
        })
    }
}
