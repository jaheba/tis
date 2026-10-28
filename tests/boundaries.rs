use tis::{Tiering, Value, Vm, compile, profile::ProfileConfig};

#[test]
fn stable_native_execution_skips_feedback_probes_in_both_backends() {
    for tier in [Tiering::Native, Tiering::CopyPatch] {
        let mut program = compile("def f(a):\n    return a[0]+1\nxs=[2]\n").unwrap();
        let mut vm = Vm::new();
        vm.tiering = tier;
        vm.thresholds.function_entries = 1;
        vm.profiling.window_ticks = 16;
        vm.run_program(&mut program).unwrap();
        let f = vm.get_global("f").unwrap();
        let xs = vm.get_global("xs").unwrap();
        for _ in 0..24 {
            assert_eq!(vm.call(f, &[xs]).unwrap(), Value::Int(3));
        }
        let before = vm.function_hir(f).unwrap().profile();
        let probes = vm.tier_stats.native_feedback_probes;
        for _ in 0..24 {
            assert_eq!(vm.call(f, &[xs]).unwrap(), Value::Int(3));
        }
        let after = vm.function_hir(f).unwrap().profile();
        assert_eq!(vm.tier_stats.native_feedback_probes, probes);
        assert_eq!(
            before.sites.iter().map(|site| site.executions).sum::<u64>(),
            after.sites.iter().map(|site| site.executions).sum::<u64>()
        );
        assert!(vm.has_lir(&vm.function_hir(f).unwrap()));
    }
}

#[test]
fn eager_native_execution_keeps_operand_and_result_feedback() {
    for tier in [Tiering::Native, Tiering::CopyPatch] {
        let mut program = compile("def f(a):\n    return a[0]+1\nxs=[2]\n").unwrap();
        let mut vm = Vm::new();
        vm.profiling = ProfileConfig::eager();
        vm.run_program(&mut program).unwrap();
        let f = vm.get_global("f").unwrap();
        let xs = vm.get_global("xs").unwrap();
        for _ in 0..12 {
            vm.call(f, &[xs]).unwrap();
        }
        vm.compile_lir(&vm.function_hir(f).unwrap()).unwrap();
        vm.tiering = tier;
        let before = vm.function_hir(f).unwrap().profile();
        let probes = vm.tier_stats.native_feedback_probes;
        for _ in 0..4 {
            assert_eq!(vm.call(f, &[xs]).unwrap(), Value::Int(3));
        }
        let after = vm.function_hir(f).unwrap().profile();
        assert!(vm.tier_stats.native_feedback_probes > probes);
        assert!(
            after
                .sites
                .iter()
                .zip(&before.sites)
                .any(|(a, b)| a.executions > b.executions)
        );
        assert!(
            after
                .sites
                .iter()
                .zip(&before.sites)
                .any(|(a, b)| a.result.observations() > b.result.observations())
        );
    }
}
