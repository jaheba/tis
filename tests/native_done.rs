use tis::{
    Object, Tiering, Value, Vm,
    hir::{CoreOp, Function, Op, Terminator, ValueId},
};

#[test]
fn native_exhaustion_checks_immediates_without_callbacks_and_guards_heap_sentinels() {
    let mut function = Function::new(1);
    let block = function.entry();
    let result = function.append(block, Op::Runtime(CoreOp::IsDone, vec![ValueId(0)]));
    function.terminate(block, Terminator::Return(result));
    let mut vm = Vm::new();
    vm.tiering = Tiering::Native;
    vm.profiling.window_ticks = 1;
    vm.thresholds.function_entries = 1;
    for _ in 0..4 {
        assert_eq!(
            vm.run(&mut function, &[Value::Int(42)]).unwrap(),
            Value::Bool(false)
        );
    }
    let calls = vm.tier_stats.native_runtime_calls;
    for value in [
        Value::None,
        Value::Bool(false),
        Value::Int(42),
        Value::Float(1.5),
        vm.heap.string("é😀abcde"),
    ] {
        assert_eq!(vm.run(&mut function, &[value]).unwrap(), Value::Bool(false));
    }
    assert_eq!(vm.tier_stats.native_runtime_calls, calls);
    for (object, expected) in [
        (Object::Done, true),
        (Object::String("heap".into()), false),
        (Object::List(Vec::<Value>::new().into()), false),
    ] {
        let value = vm.heap.allocate(object);
        assert_eq!(
            vm.run(&mut function, &[value]).unwrap(),
            Value::Bool(expected)
        );
        vm.collect_garbage(&[value]).unwrap();
        assert_eq!(
            vm.run(&mut function, &[value]).unwrap(),
            Value::Bool(expected)
        );
    }
    assert!(vm.tier_stats.native_runtime_calls > calls);
    assert!(vm.tier_stats.native_compilations > 0);
    assert_eq!(vm.tier_stats.compilation_errors, 0);
    assert_eq!(vm.stack_len(), 0);
}
