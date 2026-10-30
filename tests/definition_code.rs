use tis::{
    Object, Value, Vm, compile,
    hir::{Constant, Op, Terminator},
};

#[test]
fn repeated_definitions_share_buffers_but_keep_captures_and_body_edits_independent() {
    let source = "def make(x):\n    def add(y):\n        return x + y\n    return add\nfunctions = [make(i) for i in range(100)]\n";
    let mut program = compile(source).unwrap();
    for _ in 0..2 {
        let mut vm = Vm::new();
        vm.run_program(&mut program).unwrap();
        let functions = vm.get_global("functions").unwrap();
        let mut bodies = vec![];
        let before = vm.tier_stats.bytecode_compilations;
        for i in 0..100 {
            let f = vm.heap.get_item(functions, Value::Int(i)).unwrap();
            assert_eq!(vm.call(f, &[Value::Int(5)]).unwrap(), Value::Int(i + 5));
            let Object::Function(closure) = vm.heap.get(f).unwrap() else {
                panic!()
            };
            bodies.push(closure.function.clone());
        }
        assert_eq!(vm.tier_stats.bytecode_compilations - before, 1);
        let mut first = bodies[0].borrow_mut();
        let entry = first.entry();
        let value = first.append(entry, Op::Constant(Constant::Int(999)));
        first.terminate(entry, Terminator::Return(value));
        drop(first);
        let first = vm.heap.get_item(functions, Value::Int(0)).unwrap();
        let second = vm.heap.get_item(functions, Value::Int(1)).unwrap();
        assert_eq!(vm.call(first, &[Value::Int(5)]).unwrap(), Value::Int(999));
        assert_eq!(vm.call(second, &[Value::Int(5)]).unwrap(), Value::Int(6));
        assert_eq!(vm.tier_stats.bytecode_compilations - before, 2);
        vm.collect_garbage(&[functions]).unwrap();
        assert_eq!(vm.call(second, &[Value::Int(6)]).unwrap(), Value::Int(7));
    }
}

#[test]
fn repeated_try_thunks_compile_once_and_unused_definitions_stay_lazy() {
    let mut vm = Vm::new();
    vm.run_program(&mut compile("def unused(x):\n    return x + 100\ndef identity(x):\n    return x\nfor i in range(100):\n    try:\n        result = identity(i)\n    except Exception:\n        assert False\nassert result == 99\n").unwrap()).unwrap();
    assert_eq!(vm.tier_stats.bytecode_compilations, 3);
    assert!(vm.tier_stats.bytecode_compile_ns > 0);
}
