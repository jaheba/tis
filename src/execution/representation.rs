//! String conversion can invoke user code, including inside builtin containers.
use super::*;

impl Vm {
    pub(crate) fn py_repr(&mut self, value: Value, quote: bool) -> Result<String> {
        self.protocol_roots(&[value], |vm| {
            vm.represent(value, quote, &mut std::collections::HashSet::new())
        })
    }

    fn represent(
        &mut self,
        value: Value,
        quote: bool,
        active: &mut std::collections::HashSet<crate::ObjectId>,
    ) -> Result<String> {
        if active.len() > 256 {
            return Err(error("RecursionError: recursive representation"));
        }
        if self.instance_class(value).is_some() {
            let result = if quote {
                self.call_special(value, "__repr__", &[])?
            } else {
                match self.call_special(value, "__str__", &[])? {
                    Some(value) => Some(value),
                    None => self.call_special(value, "__repr__", &[])?,
                }
            };
            if let Some(result) = result {
                return self
                    .heap
                    .as_str(&result)
                    .map(str::to_owned)
                    .ok_or(Error::Type("string conversion must return str"));
            }
        }
        if let Some(kind) = self.exception_value_kind(value) {
            let arguments = self.exception_arguments(value)?;
            return self.protocol_roots(&arguments, |vm| {
                if quote {
                    let mut parts = vec![];
                    for &value in &arguments {
                        parts.push(vm.represent(value, true, active)?);
                    }
                    Ok(format!("{kind}({})", parts.join(", ")))
                } else {
                    match arguments.as_slice() {
                        [] => Ok(String::new()),
                        [value] => vm.represent(*value, false, active),
                        _ => {
                            let mut parts = vec![];
                            for value in &arguments {
                                parts.push(vm.represent(*value, true, active)?);
                            }
                            Ok(format!("({})", parts.join(", ")))
                        }
                    }
                }
            });
        }
        let container = match self.heap.get(value) {
            Ok(Object::List(items)) => Some(("list", items.iter().copied().collect::<Vec<_>>())),
            Ok(Object::Tuple(items)) => Some(("tuple", items.clone())),
            Ok(Object::Set(items)) => Some(("set", items.iter().map(|(_, v)| *v).collect())),
            Ok(Object::Dict(items)) => Some((
                "dict",
                items
                    .iter()
                    .flat_map(|(k, v)| [key_value(&self.heap, k), *v])
                    .collect(),
            )),
            Ok(Object::Native(name)) if name == "NotImplemented" => {
                return Ok("NotImplemented".into());
            }
            _ => None,
        };
        let Some((kind, items)) = container else {
            return self.repr(value, quote);
        };
        let Value::Ref(id) = value else {
            unreachable!()
        };
        if !active.insert(id) {
            return Ok(match kind {
                "list" => "[...]",
                "tuple" => "(...)",
                _ => "{...}",
            }
            .into());
        }
        let result = self.protocol_roots(&items, |vm| {
            let mut parts = vec![];
            if kind == "dict" {
                for pair in items.as_chunks::<2>().0 {
                    parts.push(format!(
                        "{}: {}",
                        vm.represent(pair[0], true, active)?,
                        vm.represent(pair[1], true, active)?
                    ));
                }
            } else {
                for value in &items {
                    parts.push(vm.represent(*value, true, active)?);
                }
            }
            let joined = parts.join(", ");
            Ok(match kind {
                "list" => format!("[{joined}]"),
                "tuple" => format!("({joined}{})", if items.len() == 1 { "," } else { "" }),
                "set" if items.is_empty() => "set()".into(),
                _ => format!("{{{joined}}}"),
            })
        });
        active.remove(&id);
        result
    }
}
