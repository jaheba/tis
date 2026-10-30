use tis::{Object, Tiering, Value, Vm, compile, profile::HotThresholds};
fn warm(source: &str) -> Vm {
    warm_with_window(source, 8)
}
fn warm_with_window(source: &str, ticks: u32) -> Vm {
    let mut program = compile(source).unwrap();
    let mut vm = Vm::new();
    vm.profiling.window_ticks = ticks;
    vm.tiering = Tiering::Native;
    vm.thresholds = HotThresholds {
        function_entries: 12,
        loop_backedges: 16,
    };
    vm.run_program(&mut program).unwrap();
    vm
}
#[test]
fn native_integer_code_executes_and_deoptimizes() {
    let mut vm = warm(
        "calls = [0]\ndef add(a,b):\n    calls[0] += 1\n    return a + b\nfor i in range(30):\n    assert add(i, 1) == i + 1\n",
    );
    assert!(vm.tier_stats.native_entries > 0);
    assert!(vm.tier_stats.native_code_bytes > 0);
    let f = vm.get_global("add").unwrap();
    let value = vm
        .call(
            f,
            &[
                vm.heap.allocate(Object::String("hello ".into())),
                vm.heap.allocate(Object::String("tis".into())),
            ],
        )
        .unwrap();
    assert_eq!(vm.heap.as_str(&(value)).unwrap(), "hello tis");
    assert!(vm.tier_stats.deoptimizations > 0);
    let calls = vm.get_global("calls").unwrap();
    assert_eq!(
        vm.heap.get_item(calls, Value::Int(0)).unwrap(),
        Value::Int(31)
    );
    assert_eq!(vm.stack_len(), 0);
}
#[test]
fn native_float_ops_comparisons_and_power() {
    let mut vm = warm(
        "def calc(a,b):\n    return (a + b) * 2.0 / (a - b)\ndef power(a,b):\n    return a ** b\ndef less(a,b):\n    return a < b\nfor i in range(30):\n    assert calc(4.0, 2.0) == 6.0\n    assert power(4.0, 0.5) == 2.0\n    assert less(1.0, 2.0)\nassert not less(float('nan'), 2.0)\n",
    );
    assert!(vm.tier_stats.native_entries > 0);
    assert!(
        vm.call(
            vm.get_global("calc").unwrap(),
            &[Value::Float(2.0), Value::Float(2.0)]
        )
        .is_err()
    );
    assert!(
        vm.call(
            vm.get_global("power").unwrap(),
            &[Value::Float(0.0), Value::Float(-1.0)]
        )
        .is_err()
    );
    assert!(vm.tier_stats.deoptimizations >= 2);
    assert_eq!(vm.stack_len(), 0);
}
#[test]
fn native_osr_and_gc_during_nested_calls() {
    let vm = warm(
        "def churn():\n    for i in range(5000):\n        x = [i]\ndef f(x):\n    y = [x]\n    churn()\n    return y[0]\nfor i in range(20):\n    assert f([42])[0] == 42\n",
    );
    assert!(vm.tier_stats.native_entries > 0);
    assert!(vm.tier_stats.native_osr_entries > 0);
    assert_eq!(vm.stack_len(), 0);
}
#[test]
fn native_language_fixture_matches_bytecode() {
    let vm = warm(include_str!("fixtures/language.py"));
    assert_eq!(vm.output, "language core ok\n");
    assert!(vm.tier_stats.native_entries > 0);
}
#[test]
fn native_module_lifetimes_survive_recompilation_and_retirement() {
    for _ in 0..12 {
        let mut vm = warm("def f(a,b):\n    return a + b\nfor i in range(20):\n    f(1,2)\n");
        let f = vm.get_global("f").unwrap();
        assert!(vm.call(f, &[Value::Int(i64::MAX), Value::Int(1)]).is_err());
        let hir = vm.function_hir(f).unwrap();
        vm.compile_lir(&hir).unwrap();
        assert_eq!(
            vm.call(f, &[Value::Int(3), Value::Int(4)]).unwrap(),
            Value::Int(7)
        );
        assert_eq!(vm.stack_len(), 0);
    }
}
#[test]
fn native_nbody_acceptance() {
    let mut vm = warm_with_window(include_str!("fixtures/game_nbody.py"), 128);
    assert_eq!(vm.output, "-0.169075164\n-0.169089263\n");
    assert!(vm.tier_stats.native_osr_entries > 0);
    assert_eq!(vm.tier_stats.compilation_errors, 0);
    vm.collect_garbage(&[]).unwrap();
    assert!(vm.heap.allocation_count() < 300);
}

#[test]
fn retiring_recursive_code_does_not_free_active_native_callers() {
    let mut vm = warm_with_window(
        "def f(n, x):\n    if n > 0:\n        return f(n - 1, x) + x\n    return x + x\nfor i in range(20):\n    f(4, 1)\n",
        32,
    );
    let f = vm.get_global("f").unwrap();
    assert_eq!(
        vm.call(f, &[Value::Int(2), Value::Float(0.5)]).unwrap(),
        Value::Float(2.0)
    );
    assert!(vm.tier_stats.deoptimizations >= 1);
    assert_eq!(vm.stack_len(), 0);
}
#[test]
fn helper_panics_do_not_unwind_across_native_frames() {
    let mut vm = warm("def repeat(n):\n    return 'abc' * n\nfor i in range(20):\n    repeat(1)\n");
    let f = vm.get_global("repeat").unwrap();
    let error = vm.call(f, &[Value::Int(i64::MAX)]).unwrap_err();
    assert!(error.to_string().contains("panic in native runtime helper"));
    assert_eq!(vm.stack_len(), 0);
    vm.collect_garbage(&[]).unwrap();
    let value = vm.call(f, &[Value::Int(2)]).unwrap();
    assert_eq!(vm.heap.as_str(&(value)).unwrap(), "abcabc");
}
