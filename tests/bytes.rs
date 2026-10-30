use tis::{BytecodeMode, Object, Tiering, Value, Vm, compile};

#[test]
fn byte_values_run_in_every_tier_and_interpreter() {
    let source = include_str!("fixtures/bytes_core.py");
    for tier in [
        Tiering::Bytecode,
        Tiering::Lir,
        Tiering::Native,
        Tiering::CopyPatch,
    ] {
        for mode in [BytecodeMode::Register, BytecodeMode::Stack] {
            let mut vm = Vm::new();
            vm.tiering = tier;
            vm.bytecode = mode;
            vm.thresholds.loop_backedges = 4;
            vm.thresholds.function_entries = 2;
            vm.profiling.window_ticks = 8;
            vm.run_program(&mut compile(source).unwrap()).unwrap();
            vm.collect_garbage(&[]).unwrap();
            assert_eq!(vm.stack_len(), 0);
            assert_eq!(vm.tier_stats.compilation_errors, 0);
        }
    }
}

#[test]
fn byte_keys_retain_the_original_owner_across_collection() {
    let mut vm = Vm::new();
    vm.run_program(&mut compile("key=bytes([65,66])\nd={key:42}\n").unwrap())
        .unwrap();
    let key = vm.get_global("key").unwrap();
    vm.run_program(&mut compile("del key\n").unwrap()).unwrap();
    vm.collect_garbage(&[]).unwrap();
    assert!(matches!(vm.heap.get(key), Ok(Object::Bytes(_))));
    let d = vm.get_global("d").unwrap();
    assert_eq!(vm.heap.get_item(d, key).unwrap(), Value::Int(42));
    let iter = vm.get_global("iter").unwrap();
    let next = vm.get_global("next").unwrap();
    let iterator = vm.call(iter, &[d]).unwrap();
    assert_eq!(vm.call(next, &[iterator]).unwrap(), key);
}

#[test]
fn byte_literals_are_vm_local_and_survive_explicit_gc() {
    let mut program = compile("def f():\n    return b'\\x00\\xffabc'\nvalue=f()\n").unwrap();
    let mut first = Vm::new();
    first.run_program(&mut program).unwrap();
    let value = first.get_global("value").unwrap();
    first.collect_garbage(&[]).unwrap();
    let f = first.get_global("f").unwrap();
    assert_eq!(first.call(f, &[]).unwrap(), value);
    let mut second = Vm::new();
    second.run_program(&mut program).unwrap();
    let other = second.get_global("value").unwrap();
    assert_ne!(value, other);
    assert!(second.heap.get(value).is_err());
    assert!(matches!(second.heap.get(other),Ok(Object::Bytes(data)) if data==&[0,255,97,98,99]));
}
