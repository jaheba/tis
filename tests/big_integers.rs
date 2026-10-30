use tis::{BytecodeMode, Object, Tiering, Value, Vm, compile};

#[test]
fn arbitrary_integers_run_in_every_tier_and_interpreter() {
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
            vm.run_program(&mut compile(include_str!("fixtures/big_integers.py")).unwrap())
                .unwrap();
            vm.collect_garbage(&[]).unwrap();
            assert_eq!(vm.stack_len(), 0);
            assert_eq!(vm.tier_stats.compilation_errors, 0);
        }
    }
}

#[test]
fn large_integer_keys_keep_identity_and_own_their_gc_edges() {
    let mut vm = Vm::new();
    vm.run_program(
        &mut compile("key=int('1267650600228229401496703205376')\nd={key:42}\ndel key\n").unwrap(),
    )
    .unwrap();
    vm.collect_garbage(&[]).unwrap();
    vm.run_program(&mut compile("key=next(iter(d))\nassert d[key]==42\n").unwrap())
        .unwrap();
    assert!(matches!(
        vm.heap.get(vm.get_global("key").unwrap()),
        Ok(Object::BigInt(_))
    ));
}

#[test]
fn overflow_resumes_the_integer_operation_once_and_keeps_large_results_rooted() {
    for tier in [Tiering::Lir, Tiering::Native, Tiering::CopyPatch] {
        let mut vm = Vm::new();
        vm.tiering = tier;
        vm.thresholds.function_entries = 2;
        vm.thresholds.loop_backedges = 4;
        vm.profiling.window_ticks = 8;
        vm.run_program(
            &mut compile(
                "calls=0\ndef add(a,b):\n    global calls\n    calls+=1\n    return a+b\n",
            )
            .unwrap(),
        )
        .unwrap();
        let f = vm.get_global("add").unwrap();
        for _ in 0..32 {
            assert_eq!(
                vm.call(f, &[Value::Int(20), Value::Int(22)]).unwrap(),
                Value::Int(42)
            );
        }
        let hir = vm.function_hir(f).unwrap();
        vm.compile_lir(&hir).unwrap();
        let result = vm.call(f, &[Value::Int(i64::MAX), Value::Int(1)]).unwrap();
        assert!(
            matches!(vm.heap.get(result),Ok(Object::BigInt(n)) if n.to_string()=="9223372036854775808")
        );
        assert_eq!(vm.get_global("calls").unwrap(), Value::Int(33));
        assert!(vm.tier_stats.deoptimizations > 0);
        vm.collect_garbage(&[result]).unwrap();
        assert!(vm.heap.get(result).is_ok());
    }
}
