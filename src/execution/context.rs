//! Context-manager callbacks preserve the same control outcomes as protected suites.
use super::*;

impl Vm {
    pub(super) fn with_context(
        &mut self,
        body: &Function,
        manager: Value,
        env: Value,
    ) -> Result<Value> {
        let class = self
            .instance_class(manager)
            .ok_or(Error::Type("object is not a context manager"))?;
        let exit = self
            .special(manager, "__exit__")?
            .ok_or(Error::Type("context manager has no __exit__"))?;
        let exit = self.descriptor_get(exit, Some(manager), class)?;
        self.protocol_roots(&[manager, exit], |vm| {
            let entered = vm
                .call_special(manager, "__enter__", &[])?
                .ok_or(Error::Type("context manager has no __enter__"))?;
            vm.protocol_roots(&[entered], |vm| {
                let function = vm
                    .try_bodies
                    .entry((body.id(), body.revision()))
                    .or_insert_with(|| Rc::new(RefCell::new(body.clone())))
                    .clone();
                let result = vm.execute_bound_frame(
                    function,
                    env,
                    &[entered],
                    Some(crate::vm::PositionalBindings {
                        defaults: &[],
                        count: 1,
                    }),
                );
                let flow = match result {
                    Ok(flow) => flow,
                    Err(err) => {
                        let exception = if let Some(value) = vm.pending_exception.take() {
                            value
                        } else {
                            vm.exception_from_error(err)?
                        };
                        vm.heap
                            .allocate(Object::Tuple(vec![Value::Int(4), exception]))
                    }
                };
                vm.protocol_roots(&[flow], |vm| {
                    let kind = vm.heap.get_item(flow, Value::Int(0))?;
                    let exception = vm.heap.get_item(flow, Value::Int(1))?;
                    if kind != Value::Int(4) {
                        vm.call(exit, &[Value::None, Value::None, Value::None])?;
                        return Ok(flow);
                    }
                    let class = vm.exception_type(exception)?;
                    let traceback = vm.exception_traceback(exception).unwrap_or(Value::None);
                    let depth = vm.active_exceptions.len();
                    vm.active_exceptions.push(exception);
                    let result = (|| {
                        let result = vm.call(exit, &[class, exception, traceback])?;
                        let suppress = vm.protocol_roots(&[result], |vm| vm.py_truth(result))?;
                        Ok(if suppress {
                            vm.heap
                                .allocate(Object::Tuple(vec![Value::Int(0), Value::None]))
                        } else {
                            flow
                        })
                    })();
                    vm.active_exceptions.truncate(depth);
                    result
                })
            })
        })
    }
}
