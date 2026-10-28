use tis::{
    Object, Tiering, Value, Vm, compile,
    hir::{Constant, Function, Op, Terminator},
    profile::HotThresholds,
};

fn run(source: &str, tiering: Tiering) -> (Vm, tis::Result<Value>) {
    let mut program = compile(source).unwrap();
    let mut vm = Vm::new();
    vm.tiering = tiering;
    vm.thresholds = HotThresholds {
        function_entries: 12,
        loop_backedges: 16,
    };
    let result = vm.run_program(&mut program);
    (vm, result)
}
#[test]
fn real_guard_failure_switches_back_without_repeating_effects() {
    let source = "calls = [0]\ndef add(a, b):\n    calls[0] += 1\n    return a + b\nfor i in range(30):\n    assert add(i, 2) == i + 2\nassert add('a', 'b') == 'ab'\nassert calls[0] == 31\n";
    let (mut vm, result) = run(source, Tiering::Lir);
    result.unwrap();
    assert!(vm.tier_stats.lir_entries > 0);
    assert!(vm.tier_stats.deoptimizations > 0);
    assert!(vm.tier_stats.retired > 0);
    assert_eq!(vm.stack_len(), 0);
    let f = vm.function_hir(vm.get_global("add").unwrap()).unwrap();
    assert!(!vm.has_lir(&f));
    vm.compile_lir(&f).unwrap();
    assert_eq!(
        vm.call(
            vm.get_global("add").unwrap(),
            &[Value::Float(1.0), Value::Int(2)]
        )
        .unwrap(),
        Value::Float(3.0)
    );
}
#[test]
fn a_single_long_loop_enters_lir_with_osr() {
    let (vm, result) = run(
        "def f(n):\n    total = 0\n    for i in range(n):\n        total = total + i\n    return total\nassert f(500) == 124750\n",
        Tiering::Lir,
    );
    result.unwrap();
    assert!(vm.tier_stats.osr_entries > 0);
    assert_eq!(vm.stack_len(), 0);
}
#[test]
fn language_fixture_matches_both_tiers() {
    for tier in [Tiering::Bytecode, Tiering::Lir] {
        let (_, result) = run(include_str!("fixtures/language.py"), tier);
        result.unwrap();
    }
}
#[test]
fn root_maps_keep_values_alive_during_nested_calls() {
    let source = "def churn():\n    for i in range(6000):\n        x = [i]\n    return 1\ndef f(x):\n    y = [x]\n    churn()\n    return y[0]\nfor i in range(20):\n    assert f([42])[0] == 42\n";
    let (vm, result) = run(source, Tiering::Lir);
    result.unwrap();
    assert!(vm.tier_stats.lir_entries > 0);
    assert!(vm.tier_stats.osr_entries > 0);
    assert_eq!(vm.stack_len(), 0);
}
#[test]
fn checked_add_overflow_deoptimizes_and_cleans_up() {
    let (mut vm, result) = run(
        "def add(a,b):\n    return a + b\nfor i in range(20):\n    add(1,2)\n",
        Tiering::Lir,
    );
    result.unwrap();
    assert!(
        vm.call(
            vm.get_global("add").unwrap(),
            &[Value::Int(i64::MAX), Value::Int(1)]
        )
        .is_err()
    );
    assert!(vm.tier_stats.deoptimizations > 0);
    assert_eq!(vm.stack_len(), 0);
    vm.collect_garbage(&[]).unwrap();
}
#[test]
fn stale_artifacts_do_not_execute_after_hir_changes() {
    let mut f = Function::new(0);
    let b = f.entry();
    let one = f.append(b, Op::Constant(Constant::Int(1)));
    f.terminate(b, Terminator::Return(one));
    let mut vm = Vm::new();
    vm.tiering = Tiering::Lir;
    vm.compile_lir(&f).unwrap();
    assert_eq!(vm.run(&mut f, &[]).unwrap(), Value::Int(1));
    let two = f.append(b, Op::Constant(Constant::Int(2)));
    f.terminate(b, Terminator::Return(two));
    assert!(!vm.has_lir(&f));
    assert_eq!(vm.run(&mut f, &[]).unwrap(), Value::Int(2));
}
#[test]
fn deep_runtime_operations_return_errors_instead_of_aborting() {
    for source in [
        "a = []\nfor i in range(10000):\n    a = [a]\nprint(a)\n",
        "it = iter([1])\nfor i in range(10000):\n    it = enumerate(it)\nnext(it)\n",
    ] {
        let (mut vm, result) = run(source, Tiering::Lir);
        assert!(result.unwrap_err().to_string().contains("RecursionError"));
        assert_eq!(vm.stack_len(), 0);
        vm.collect_garbage(&[]).unwrap();
    }
}
#[test]
fn lir_parallel_backedges_preserve_swaps() {
    let (_, result) = run(
        "def f(n):\n    a = 0\n    b = 1\n    for i in range(n):\n        a, b = b, a + b\n    return a\nassert f(30) == 832040\n",
        Tiering::Lir,
    );
    result.unwrap();
}
#[test]
fn host_pins_survive_lir_collections() {
    let mut vm = Vm::new();
    vm.tiering = Tiering::Lir;
    vm.thresholds.loop_backedges = 8;
    let value = vm.heap.allocate(Object::List(vec![Value::Int(42)]));
    let pin = vm.pin(value).unwrap();
    let mut program = compile("for i in range(5000):\n    x = [i]\n").unwrap();
    vm.run_program(&mut program).unwrap();
    vm.collect_garbage(&[]).unwrap();
    assert!(vm.heap.get(value).is_ok());
    vm.unpin(pin);
    vm.collect_garbage(&[]).unwrap();
    assert!(vm.heap.get(value).is_err());
}

#[test]
fn float_specialization_preserves_numeric_types_and_error_paths() {
    let source = "def f(a, b):\n    return (a + b) * 2.0 / b\nfor i in range(30):\n    assert f(3.0, 2) == 5.0\nassert f(3, 2) == 5.0\n";
    let (mut vm, result) = run(source, Tiering::Lir);
    result.unwrap();
    assert!(vm.tier_stats.lir_entries > 0);
    assert!(vm.tier_stats.deoptimizations > 0);
    let f = vm.get_global("f").unwrap();
    let hir = vm.function_hir(f).unwrap();
    vm.compile_lir(&hir).unwrap();
    assert!(
        vm.call(f, &[Value::Float(1.0), Value::Int(0)])
            .unwrap_err()
            .to_string()
            .contains("ZeroDivision")
    );
    assert_eq!(vm.stack_len(), 0);
}
#[test]
fn float_power_domain_errors_deoptimize_before_raising() {
    let (mut vm, result) = run(
        "def power(a,b):\n    return a ** b\nfor i in range(30):\n    assert power(4.0, 0.5) == 2.0\n",
        Tiering::Lir,
    );
    result.unwrap();
    let f = vm.get_global("power").unwrap();
    assert!(
        vm.call(f, &[Value::Float(-1.0), Value::Float(0.5)])
            .is_err()
    );
    assert!(vm.tier_stats.deoptimizations > 0);
    assert_eq!(vm.stack_len(), 0);
}
#[test]
fn float_comparison_keeps_nan_semantics() {
    let (vm, result) = run(
        "def less(a,b):\n    return a < b\nfor i in range(30):\n    assert less(1.0, 2.0)\nassert not less(float('nan'), 2.0)\n",
        Tiering::Lir,
    );
    result.unwrap();
    assert!(vm.tier_stats.lir_entries > 0);
}
#[test]
fn nbody_runs_through_the_executable_lir_tier() {
    let (mut vm, result) = run(include_str!("fixtures/game_nbody.py"), Tiering::Lir);
    result.unwrap();
    assert_eq!(vm.output, "-0.169075164\n-0.169089263\n");
    assert!(vm.tier_stats.osr_entries > 0);
    let advance = vm.function_hir(vm.get_global("advance").unwrap()).unwrap();
    assert!(
        advance
            .profile()
            .sites
            .iter()
            .any(|site| site.fast_paths > 0)
    );
    vm.collect_garbage(&[]).unwrap();
    assert!(vm.heap.allocation_count() < 300);
}

#[test]
fn tiering_can_be_enabled_after_bytecode_warmup() {
    let (mut vm, result) = run(
        "def f(a,b):\n    return a + b\nfor i in range(20):\n    f(1,2)\n",
        Tiering::Bytecode,
    );
    result.unwrap();
    vm.tiering = Tiering::Lir;
    let f = vm.get_global("f").unwrap();
    assert_eq!(
        vm.call(f, &[Value::Int(3), Value::Int(4)]).unwrap(),
        Value::Int(7)
    );
    assert!(vm.tier_stats.lir_entries > 0);
}
#[test]
fn invalid_helper_arity_is_rejected_before_execution() {
    let mut program = compile("def add(a,b):\n    return a + b\n").unwrap();
    let f = program.function_mut("add").unwrap();
    let mut lir = f.lower_lir(tis::lir::LoweringMode::Generic).unwrap();
    let call = lir
        .blocks
        .iter_mut()
        .flat_map(|b| &mut b.instructions)
        .find(|i| {
            matches!(
                i.operation,
                tis::lir::Operation::Call {
                    helper: tis::lir::Helper::Add,
                    ..
                }
            )
        })
        .unwrap();
    if let tis::lir::Operation::Call { arguments, .. } = &mut call.operation {
        arguments.clear();
    }
    assert!(lir.verify().is_err());
    let mut vm = Vm::new();
    assert!(vm.install_lir(f, lir).is_err());
}
