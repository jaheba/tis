use tis::{
    Tiering, Value, Vm, compile,
    profile::{HotThresholds, ProfileMode},
};

#[test]
fn eager_feedback_rebuilds_native_artifacts_and_reads_current_registers() {
    for tier in [Tiering::Native, Tiering::CopyPatch] {
        let mut program=compile("def f(a,b):\n    x=a-b\n    return x*x+a\nfor i in range(50):\n    assert f(3.0,2.0)==4.0\n").unwrap();
        let mut vm = Vm::new();
        vm.tiering = tier;
        vm.thresholds = HotThresholds {
            function_entries: 4,
            loop_backedges: 4,
        };
        vm.profiling.window_ticks = 16;
        vm.run_program(&mut program).unwrap();
        let function = vm.get_global("f").unwrap();
        let before = vm.function_hir(function).unwrap().profile();
        let compilations =
            vm.tier_stats.native_compilations + vm.tier_stats.copy_patch_compilations;
        vm.profiling.mode = ProfileMode::Eager;
        assert_eq!(
            vm.call(function, &[Value::Float(4.0), Value::Float(2.0)])
                .unwrap(),
            Value::Float(8.0)
        );
        assert_eq!(
            vm.tier_stats.native_compilations + vm.tier_stats.copy_patch_compilations,
            compilations + 1
        );
        let after = vm.function_hir(function).unwrap().profile();
        assert!(
            before
                .sites
                .iter()
                .zip(&after.sites)
                .any(|(a, b)| b.fast_paths > a.fast_paths
                    && b.result.observations() > a.result.observations())
        );
        vm.collect_garbage(&[]).unwrap();
        assert_eq!(vm.stack_len(), 0);
    }
}
