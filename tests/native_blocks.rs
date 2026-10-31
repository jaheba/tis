use tis::{Tiering, Value, Vm, compile, profile::ProfileMode};

#[test]
fn quiet_blocks_keep_precise_backedges_and_resume_eager_observation() {
    for tier in [Tiering::Native, Tiering::CopyPatch] {
        let mut vm = Vm::new();
        vm.tiering = tier;
        vm.thresholds.function_entries = 1;
        vm.thresholds.loop_backedges = 4;
        vm.run_program(
            &mut compile(
                r#"
def f(n):
    x=0
    while n:
        if n%2:
            x=x+n
        else:
            x=x-n
        n=n-1
    return x
for i in range(300):
    assert f(10)==-5
"#,
            )
            .unwrap(),
        )
        .unwrap();
        let f = vm.get_global("f").unwrap();
        let edges = vm
            .function_hir(f)
            .unwrap()
            .profile()
            .backedges
            .values()
            .sum::<u64>();
        let before = vm.tier_stats.native_inline_blocks;
        assert_eq!(vm.call(f, &[Value::Int(20)]).unwrap(), Value::Int(-10));
        assert!(vm.tier_stats.native_inline_blocks > before);
        assert_eq!(
            vm.function_hir(f)
                .unwrap()
                .profile()
                .backedges
                .values()
                .sum::<u64>()
                - edges,
            20
        );

        vm.profiling.mode = ProfileMode::Eager;
        let blocks = vm
            .function_hir(f)
            .unwrap()
            .profile()
            .blocks
            .iter()
            .sum::<u64>();
        let before = vm.tier_stats.native_inline_blocks;
        assert_eq!(vm.call(f, &[Value::Int(10)]).unwrap(), Value::Int(-5));
        assert_eq!(vm.tier_stats.native_inline_blocks, before);
        assert!(
            vm.function_hir(f)
                .unwrap()
                .profile()
                .blocks
                .iter()
                .sum::<u64>()
                > blocks
        );
    }
}

#[test]
fn deferred_backedges_survive_exception_exits() {
    for tier in [Tiering::Native, Tiering::CopyPatch] {
        let mut vm = Vm::new();
        vm.tiering = tier;
        vm.thresholds.function_entries = 1;
        vm.thresholds.loop_backedges = 4;
        vm.run_program(&mut compile(
            "def f(n,fail):\n    while n:\n        if n==fail:\n            raise ValueError('stop')\n        n=n-1\n    return n\nfor i in range(300):\n    assert f(10,-1)==0\n",
        ).unwrap()).unwrap();
        let f = vm.get_global("f").unwrap();
        let edges = || {
            vm.function_hir(f)
                .unwrap()
                .profile()
                .backedges
                .values()
                .sum::<u64>()
        };
        let before = edges();
        assert!(vm.call(f, &[Value::Int(10), Value::Int(5)]).is_err());
        assert_eq!(
            vm.function_hir(f)
                .unwrap()
                .profile()
                .backedges
                .values()
                .sum::<u64>()
                - before,
            5
        );
        assert_eq!(vm.stack_len(), 0);
    }
}
