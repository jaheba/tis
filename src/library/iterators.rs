//! Lazy adapters advance their real source iterators, never precompute infinite input.
use super::*;
#[derive(Debug, Clone)]
pub enum Kind {
    Count {
        value: Value,
        step: Value,
    },
    Map {
        function: Value,
        iterators: Vec<Value>,
    },
    Filter {
        predicate: Value,
        iterator: Value,
    },
    Islice {
        iterator: Value,
        next: usize,
        start: usize,
        stop: Option<usize>,
        step: usize,
        consumed: usize,
    },
}
#[derive(Debug, Clone)]
pub struct State {
    pub kind: Kind,
    pub done: bool,
}
impl State {
    pub fn name(&self) -> &'static str {
        match self.kind {
            Kind::Count { .. } => "itertools.count",
            Kind::Map { .. } => "map",
            Kind::Filter { .. } => "filter",
            Kind::Islice { .. } => "itertools.islice",
        }
    }
    pub fn visit(&self, visit: &mut impl FnMut(Value)) {
        match &self.kind {
            Kind::Count { value, step } => {
                visit(*value);
                visit(*step);
            }
            Kind::Map {
                function,
                iterators,
            } => {
                visit(*function);
                for &v in iterators {
                    visit(v);
                }
            }
            Kind::Filter {
                predicate,
                iterator,
            } => {
                visit(*predicate);
                visit(*iterator);
            }
            Kind::Islice { iterator, .. } => visit(*iterator),
        }
    }
}
impl Vm {
    pub(crate) fn is_library_iterator(&self, value: Value) -> bool {
        matches!(self.heap.get(value),Ok(Object::Library(payload))if matches!(&**payload,Payload::Iterator(_)))
    }
    pub(crate) fn library_next(&mut self, receiver: Value) -> Result<Value> {
        self.protocol_roots(&[receiver], |vm| {
            let Object::Library(payload) = vm.heap.get(receiver)? else {
                return Err(Error::Type("iterator"));
            };
            let Payload::Iterator(state) = &**payload else {
                return Err(Error::Type("iterator"));
            };
            if state.done {
                return Ok(vm.heap.allocate(Object::Done));
            }
            let state = state.clone();
            let result = match state.kind {
                Kind::Count { value, step } => {
                    let next = vm.py_binary("+", value, step)?;
                    let mut object = vm.heap.get_mut(receiver)?;
                    let Object::Library(payload) = &mut *object else {
                        unreachable!()
                    };
                    let Payload::Iterator(state) = &mut **payload else {
                        unreachable!()
                    };
                    state.kind = Kind::Count { value: next, step };
                    value
                }
                Kind::Map {
                    function,
                    iterators,
                } => {
                    let start = vm.transient_roots.len();
                    let result = (|| {
                        let mut args = Vec::with_capacity(iterators.len());
                        for iterator in iterators {
                            let value = vm.next(iterator)?;
                            if vm.is_done(value) {
                                return Ok(value);
                            }
                            vm.transient_roots.push(value);
                            args.push(value);
                        }
                        vm.call(function, &args)
                    })();
                    vm.transient_roots.truncate(start);
                    result?
                }
                Kind::Filter {
                    predicate,
                    iterator,
                } => loop {
                    let value = vm.next(iterator)?;
                    if vm.is_done(value) {
                        break value;
                    }
                    let keep = vm.protocol_roots(&[value], |vm| {
                        let condition = if predicate == Value::None {
                            value
                        } else {
                            vm.call(predicate, &[value])?
                        };
                        vm.protocol_roots(&[condition], |vm| vm.py_truth(condition))
                    })?;
                    if keep {
                        break value;
                    }
                },
                Kind::Islice {
                    iterator,
                    next,
                    start,
                    stop,
                    step,
                    mut consumed,
                } => {
                    let target = if stop.is_some_and(|stop| next >= stop) {
                        start.max(stop.unwrap())
                    } else {
                        next
                    };
                    let mut exhausted = None;
                    while consumed < target {
                        let value = vm.next(iterator)?;
                        if vm.is_done(value) {
                            exhausted = Some(value);
                            break;
                        }
                        consumed += 1;
                    }
                    let value = if let Some(done) = exhausted {
                        done
                    } else if stop.is_some_and(|stop| next >= stop) {
                        vm.heap.allocate(Object::Done)
                    } else {
                        let value = vm.next(iterator)?;
                        consumed += 1;
                        value
                    };
                    let mut object = vm.heap.get_mut(receiver)?;
                    let Object::Library(payload) = &mut *object else {
                        unreachable!()
                    };
                    let Payload::Iterator(state) = &mut **payload else {
                        unreachable!()
                    };
                    state.kind = Kind::Islice {
                        iterator,
                        next: next.saturating_add(step),
                        start,
                        stop,
                        step,
                        consumed,
                    };
                    value
                }
            };
            if vm.is_done(result) {
                let mut object = vm.heap.get_mut(receiver)?;
                let Object::Library(payload) = &mut *object else {
                    unreachable!()
                };
                let Payload::Iterator(state) = &mut **payload else {
                    unreachable!()
                };
                state.done = true;
            }
            Ok(result)
        })
    }
    pub(super) fn library_iterator(&mut self, name: &str, args: &[Value]) -> Result<Value> {
        let kind = match name {
            "map" => {
                if args.len() < 2 {
                    return Err(Error::Type("map needs callable and iterables"));
                }
                let mut iterators = vec![];
                for &arg in &args[1..] {
                    let iterator = self.iter(arg)?;
                    self.transient_roots.push(iterator);
                    iterators.push(iterator);
                }
                Kind::Map {
                    function: args[0],
                    iterators,
                }
            }
            "filter" => {
                arity(args, 2, 2)?;
                Kind::Filter {
                    predicate: args[0],
                    iterator: self.iter(args[1])?,
                }
            }
            "itertools.count" => {
                arity(args, 0, 2)?;
                let value = args.first().copied().unwrap_or(Value::Int(0));
                let step = args.get(1).copied().unwrap_or(Value::Int(1));
                for v in [value, step] {
                    if !crate::integer::is_integer(&self.heap, v) && !matches!(v, Value::Float(_)) {
                        return Err(Error::Type("count arguments must be numbers"));
                    }
                }
                Kind::Count { value, step }
            }
            "itertools.islice" => {
                arity(args, 2, 4)?;
                let number = |vm: &mut Vm, v: Value| -> Result<Option<usize>> {
                    if v == Value::None {
                        Ok(None)
                    } else {
                        Ok(Some(usize::try_from(vm.py_index(v)?).map_err(|_| {
                            invalid("islice indices must be nonnegative")
                        })?))
                    }
                };
                let (start, stop, step) = if args.len() == 2 {
                    (0, number(self, args[1])?, 1)
                } else {
                    (
                        number(self, args[1])?.unwrap_or(0),
                        number(self, args[2])?,
                        if args.len() == 4 {
                            number(self, args[3])?.unwrap_or(1)
                        } else {
                            1
                        },
                    )
                };
                if step == 0 {
                    return Err(invalid("islice step must be positive"));
                }
                Kind::Islice {
                    iterator: self.iter(args[0])?,
                    next: start,
                    start,
                    stop,
                    step,
                    consumed: 0,
                }
            }
            _ => return Err(Error::Unsupported("iterator adapter")),
        };
        Ok(self
            .heap
            .allocate(Object::Library(Box::new(Payload::Iterator(State {
                kind,
                done: false,
            })))))
    }
}
