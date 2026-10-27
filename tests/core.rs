use tis::{
    Error, Heap, Key, Object, Value, Vm,
    hir::{AddFeedback, AddStrategy, Constant, Edge, Function, Op, Terminator, ValueId},
};

#[test]
fn addition_specializes_then_falls_back_for_new_types() {
    let (mut function, addition) = add_function();
    let mut vm = Vm::new();
    for value in 0..10 {
        assert_eq!(
            vm.run(&mut function, &[Value::Int(value), Value::Int(2)]),
            Ok(Value::Int(value + 2))
        );
    }
    let profile = function.feedback(addition).unwrap();
    assert_eq!(profile.strategy, AddStrategy::Integer);
    assert_eq!(profile.fast_paths, 2);
    assert!(function.to_string().contains("add.int [fallback=add]"));

    let left = vm.heap.allocate(Object::String("left".into()));
    let right = vm.heap.allocate(Object::String("right".into()));
    let result = vm.run(&mut function, &[left, right]).unwrap();
    assert_eq!(
        vm.heap.get(result).unwrap(),
        &Object::String("leftright".into())
    );
    assert_eq!(function.feedback(addition).unwrap().guard_failures, 1);
    assert_eq!(
        function.feedback(addition).unwrap().strategy,
        AddStrategy::Generic
    );
    assert_eq!(
        vm.run(&mut function, &[Value::Float(1.5), Value::Int(2)]),
        Ok(Value::Float(3.5))
    );
    assert_eq!(
        vm.run(&mut function, &[Value::Bool(true), Value::Bool(false)]),
        Ok(Value::Int(1))
    );
    assert_eq!(vm.stack_len(), 0);
}

#[test]
fn overflow_exits_the_specialization_and_releases_the_frame() {
    let (mut function, addition) = add_function();
    let mut vm = Vm::new();
    for _ in 0..AddFeedback::WARMUP {
        vm.run(&mut function, &[Value::Int(1), Value::Int(2)])
            .unwrap();
    }
    assert_eq!(
        vm.run(&mut function, &[Value::Int(i64::MAX), Value::Int(1)]),
        Err(Error::IntegerOverflow)
    );
    assert_eq!(function.feedback(addition).unwrap().guard_failures, 1);
    assert_eq!(
        function.feedback(addition).unwrap().strategy,
        AddStrategy::Generic
    );
    assert_eq!(vm.stack_len(), 0);
    assert_eq!(
        vm.run(&mut function, &[Value::Int(1), Value::Int(2)]),
        Ok(Value::Int(3))
    );
}

#[test]
fn loop_feedback_enriches_the_same_ir_instruction() {
    let mut function = Function::new(1);
    let entry = function.entry();
    let input = function.parameters(entry)[0];
    let zero = function.append(entry, Op::Constant(Constant::Int(0)));
    let one = function.append(entry, Op::Constant(Constant::Int(1)));
    let head = function.block(2);
    let n = function.parameters(head)[0];
    let sum = function.parameters(head)[1];
    let body = function.block(0);
    let exit = function.block(1);
    let result = function.parameters(exit)[0];
    function.terminate(entry, Terminator::Jump(Edge::new(head, vec![input, zero])));
    let condition = function.append(head, Op::Less(zero, n));
    function.terminate(
        head,
        Terminator::Branch {
            condition,
            yes: Edge::new(body, vec![]),
            no: Edge::new(exit, vec![sum]),
        },
    );
    let addition = function.append(body, Op::Add(sum, n));
    let next = function.append(body, Op::Subtract(n, one));
    function.terminate(
        body,
        Terminator::Jump(Edge::new(head, vec![next, addition])),
    );
    function.terminate(exit, Terminator::Return(result));

    let mut vm = Vm::new();
    assert_eq!(vm.run(&mut function, &[Value::Int(10)]), Ok(Value::Int(55)));
    let feedback = function.feedback(addition).unwrap();
    assert_eq!(feedback.executions, 10);
    assert_eq!(feedback.fast_paths, 2);
    assert_eq!(feedback.strategy, AddStrategy::Integer);
    assert_eq!(vm.stack_len(), 0);
}

#[test]
fn backedge_arguments_are_assigned_in_parallel() {
    let mut function = Function::new(0);
    let entry = function.entry();
    let one = function.append(entry, Op::Constant(Constant::Int(1)));
    let two = function.append(entry, Op::Constant(Constant::Int(2)));
    let zero = function.append(entry, Op::Constant(Constant::Int(0)));
    let head = function.block(3);
    let a = function.parameters(head)[0];
    let b = function.parameters(head)[1];
    let n = function.parameters(head)[2];
    let body = function.block(0);
    let exit = function.block(0);
    function.terminate(
        entry,
        Terminator::Jump(Edge::new(head, vec![one, two, one])),
    );
    let condition = function.append(head, Op::Less(zero, n));
    function.terminate(
        head,
        Terminator::Branch {
            condition,
            yes: Edge::new(body, vec![]),
            no: Edge::new(exit, vec![]),
        },
    );
    let next = function.append(body, Op::Subtract(n, one));
    function.terminate(body, Terminator::Jump(Edge::new(head, vec![b, a, next])));
    function.terminate(exit, Terminator::Return(a));
    assert_eq!(Vm::new().run(&mut function, &[]), Ok(Value::Int(2)));
}

#[test]
fn collections_are_constructed_and_read_by_ir() {
    let mut function = Function::new(0);
    let entry = function.entry();
    let key = function.append(entry, Op::Constant(Constant::String("key".into())));
    let value = function.append(entry, Op::Constant(Constant::Int(42)));
    let tuple = function.append(entry, Op::Tuple(vec![value, value]));
    let dict = function.append(entry, Op::Dict(vec![(key, tuple)]));
    let found = function.append(entry, Op::GetItem(dict, key));
    let len = function.append(entry, Op::Len(found));
    let output = function.append(entry, Op::List(vec![found, dict, len]));
    function.terminate(entry, Terminator::Return(output));
    let mut vm = Vm::new();
    let result = vm.run(&mut function, &[]).unwrap();
    let Object::List(items) = vm.heap.get(result).unwrap() else {
        panic!("expected list")
    };
    assert_eq!(items[2], Value::Int(2));
    assert_eq!(
        vm.heap.get(items[0]).unwrap(),
        &Object::Tuple(vec![Value::Int(42), Value::Int(42)])
    );
    let Object::Dict(entries) = vm.heap.get(items[1]).unwrap() else {
        panic!("expected dictionary")
    };
    assert_eq!(entries[0].0, Key::String("key".into()));
    assert_eq!(entries[0].1, items[0]);
}

#[test]
fn item_assignment_preserves_aliases() {
    let mut function = Function::new(1);
    let entry = function.entry();
    let object = function.parameters(entry)[0];
    let key = function.append(entry, Op::Constant(Constant::Int(-1)));
    let value = function.append(entry, Op::Constant(Constant::Int(42)));
    function.append(entry, Op::SetItem { object, key, value });
    let result = function.append(entry, Op::GetItem(object, key));
    function.terminate(entry, Terminator::Return(result));
    let mut vm = Vm::new();
    let list = vm.heap.allocate(Object::List(vec![Value::None]));
    assert_eq!(vm.run(&mut function, &[list]), Ok(Value::Int(42)));
    assert_eq!(
        vm.heap.get(list).unwrap(),
        &Object::List(vec![Value::Int(42)])
    );
}

#[test]
fn dictionaries_preserve_order_and_canonicalize_boolean_keys() {
    let mut heap = Heap::new();
    let dict = heap.allocate(Object::Dict(vec![]));
    heap.set_item(dict, Value::Bool(true), Value::Int(10))
        .unwrap();
    heap.set_item(dict, Value::Int(2), Value::Int(20)).unwrap();
    heap.set_item(dict, Value::Int(1), Value::Int(30)).unwrap();
    assert_eq!(
        heap.get(dict).unwrap(),
        &Object::Dict(vec![
            (Key::Int(1), Value::Int(30)),
            (Key::Int(2), Value::Int(20))
        ])
    );
    assert_eq!(heap.get_item(dict, Value::Bool(true)), Ok(Value::Int(30)));
    assert_eq!(heap.get_item(dict, Value::Int(3)), Err(Error::MissingKey));
}

#[test]
fn unicode_indexing_and_immutable_tuples() {
    let mut heap = Heap::new();
    let text = heap.allocate(Object::String("é🙂".into()));
    assert_eq!(heap.len(text), Ok(2));
    let character = heap.get_item(text, Value::Int(-1)).unwrap();
    assert_eq!(heap.get(character).unwrap(), &Object::String("🙂".into()));
    assert_eq!(heap.get_item(text, Value::Int(2)), Err(Error::Index));
    let tuple = heap.allocate(Object::Tuple(vec![Value::Int(1)]));
    assert_eq!(
        heap.set_item(tuple, Value::Int(0), Value::None),
        Err(Error::Type("object does not support item assignment"))
    );
}

#[test]
fn heap_supports_cycles_without_reference_counting() {
    let mut heap = Heap::new();
    let list = heap.allocate(Object::List(vec![Value::None]));
    heap.set_item(list, Value::Int(0), list).unwrap();
    assert_eq!(heap.get_item(list, Value::Int(0)), Ok(list));
    assert_eq!(heap.allocation_count(), 1);
}

#[test]
fn errors_leave_no_frame_values_behind() {
    let (mut function, _) = add_function();
    let mut vm = Vm::new();
    assert_eq!(
        vm.run(&mut function, &[]),
        Err(Error::Arguments {
            expected: 2,
            actual: 0
        })
    );
    assert_eq!(
        vm.run(&mut function, &[Value::None, Value::None]),
        Err(Error::Type("unsupported addition operands"))
    );
    assert_eq!(vm.stack_len(), 0);

    let mut missing = Function::new(0);
    assert_eq!(
        vm.run(&mut missing, &[]),
        Err(Error::InvalidIr("missing terminator"))
    );
    let block = missing.entry();
    missing.terminate(block, Terminator::Return(ValueId(999)));
    assert_eq!(
        vm.run(&mut missing, &[]),
        Err(Error::InvalidIr("invalid value"))
    );
    assert_eq!(vm.stack_len(), 0);
}

#[test]
fn heap_handles_cannot_be_used_in_another_vm() {
    let first = Vm::new();
    let mut second = Vm::new();
    let object = first.heap.allocate(Object::String("first".into()));
    assert_eq!(second.heap.get(object), Err(Error::InvalidObject));
    let (mut function, _) = add_function();
    assert_eq!(
        second.run(&mut function, &[object, Value::Int(1)]),
        Err(Error::InvalidObject)
    );
    assert_eq!(second.stack_len(), 0);
}

fn add_function() -> (Function, ValueId) {
    let mut function = Function::new(2);
    let entry = function.entry();
    let lhs = function.parameters(entry)[0];
    let rhs = function.parameters(entry)[1];
    let sum = function.append(entry, Op::Add(lhs, rhs));
    function.terminate(entry, Terminator::Return(sum));
    (function, sum)
}
