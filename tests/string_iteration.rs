use tis::{Object, Tiering, Value, Vm, compile, lir::LoweringMode};

#[test]
fn native_string_iteration_uses_inline_characters_with_guarded_exhaustion() {
    let mut program = compile(
        "def count(source):\n    n = 0\n    for item in source:\n        n += 1\n    return n\n",
    )
    .unwrap();
    let mut vm = Vm::new();
    vm.tiering = Tiering::Native;
    vm.profiling.window_ticks = 8;
    vm.thresholds.function_entries = 2;
    vm.thresholds.loop_backedges = 8;
    vm.run_program(&mut program).unwrap();
    let f = vm.get_global("count").unwrap();
    let text = vm.heap.string(&"aé😀\0".repeat(1000));
    for _ in 0..5 {
        assert_eq!(vm.call(f, &[text]).unwrap(), Value::Int(4000));
    }
    let lir = vm
        .function_hir(f)
        .unwrap()
        .lower_lir(LoweringMode::Profiled)
        .unwrap();
    assert!(format!("{lir:?}").contains("StringNext"));
    let roots = vm.tier_stats.native_root_syncs;
    assert_eq!(vm.call(f, &[text]).unwrap(), Value::Int(4000));
    assert!(vm.tier_stats.native_root_syncs - roots < 100);
    for value in [
        vm.heap.string(""),
        vm.heap.string("é😀"),
        vm.heap
            .allocate(Object::List(vec![Value::Int(1), Value::None].into())),
        vm.heap.allocate(Object::Tuple(vec![Value::Int(1)])),
    ] {
        let expected = vm.heap.len(value).unwrap();
        assert_eq!(vm.call(f, &[value]).unwrap(), Value::Int(expected as i64));
    }
    vm.collect_garbage(&[f, text]).unwrap();
    assert_eq!(vm.call(f, &[text]).unwrap(), Value::Int(4000));
    assert!(vm.tier_stats.native_compilations > 0);
    assert_eq!(vm.tier_stats.compilation_errors, 0);
    assert_eq!(vm.stack_len(), 0);
}
