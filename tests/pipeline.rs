use tis::{Heap, Object, Value, Vm, compile};

#[test]
fn python_control_flow_runs_through_bytecode() {
    let mut program = compile("def sum(n):\n    total = 0\n    while 0 < n:\n        total = total + n\n        n = n - 1\n    else:\n        total = total + 1\n    if total > 10:\n        result = total\n    elif total > 0:\n        result = 2\n    else:\n        result = 3\n    return result\n").unwrap();
    let function = program.function_mut("sum").unwrap();
    let bytes = function.bytecode().unwrap().bytes().to_vec();
    let mut vm = Vm::new();
    for _ in 0..16 {
        assert_eq!(vm.run(function, &[Value::Int(5)]).unwrap(), Value::Int(16));
    }
    assert_eq!(function.bytecode().unwrap().bytes(), bytes);
    assert!(function.dump().contains("Int"));
    let listing = function.bytecode().unwrap().disassemble().unwrap();
    assert!(listing.contains("Jump"));
    assert!(listing.contains("Add"));
    assert_eq!(vm.run(function, &[Value::Int(0)]).unwrap(), Value::Int(2));
}

#[test]
fn source_collections_and_type_fallback() {
    let mut program = compile("def add(a, b):\n    return a + b\nx = [1, 2]\ny = x\ny[0] = 8\nd = {'x': x}\nlen(d['x']) + x[0]\n").unwrap();
    let mut vm = Vm::new();
    assert_eq!(
        vm.run(program.function_mut("<module>").unwrap(), &[])
            .unwrap(),
        Value::Int(10)
    );
    let add = program.function_mut("add").unwrap();
    for _ in 0..16 {
        assert_eq!(
            vm.run(add, &[Value::Int(1), Value::Int(2)]).unwrap(),
            Value::Int(3)
        );
    }
    assert_eq!(
        vm.run(add, &[Value::Float(1.5), Value::Int(2)]).unwrap(),
        Value::Float(3.5)
    );
    let a = vm.heap.allocate(Object::String("a".into()));
    let b = vm.heap.allocate(Object::String("b".into()));
    let result = vm.run(add, &[a, b]).unwrap();
    assert_eq!(vm.heap.get(result).unwrap(), &Object::String("ab".into()));
    assert!(vm.collect_garbage(&[result]).unwrap() > 0);
    assert!(vm.heap.get(result).is_ok());
    assert!(vm.heap.get(a).is_err());
}

#[test]
fn frontend_rejects_unsupported_code() {
    for source in ["def broken(:", "import os"] {
        assert!(compile(source).is_err(), "{source}");
    }
}

#[test]
fn collection_preserves_roots_and_reuses_slots_without_reviving_handles() {
    let mut heap = Heap::new();
    let child = heap.allocate(Object::String("child".into()));
    let root = heap.allocate(Object::List(vec![child].into()));
    let cycle = heap.allocate(Object::List(vec![Value::None].into()));
    heap.set_item(cycle, Value::Int(0), cycle).unwrap();
    assert_eq!(heap.collect(&[root]).unwrap(), 1);
    assert!(heap.get(cycle).is_err());
    let replacement = heap.allocate(Object::String("replacement".into()));
    assert_eq!(heap.slot_count(), 3);
    assert!(heap.get(cycle).is_err());
    assert!(heap.get(replacement).is_ok());
    assert!(heap.get(child).is_ok());
    let foreign = Heap::new().allocate(Object::List(vec![].into()));
    assert!(heap.collect(&[foreign]).is_err());
    assert_eq!(heap.allocation_count(), 3);
    assert_eq!(heap.collect(&[]).unwrap(), 3);
}
