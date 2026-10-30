use tis::{
    Tiering, Vm, compile,
    lir::{LoweringMode, Operation},
    profile::ProfileConfig,
};

#[test]
fn adjacent_builtin_unpackings_reuse_checked_values_but_calls_are_barriers() {
    let mut vm = Vm::new();
    vm.profiling = ProfileConfig::eager();
    vm.run_program(&mut compile("def f(xs):\n    a,b=xs\n    a,b=xs\n    xs.reverse()\n    c,d=xs\n    c,d=xs\n    return a,b,c,d\nfor i in range(20):\n    assert f([1,2]) == (1,2,2,1)\n").unwrap()).unwrap();
    let hir = vm.function_hir(vm.get_global("f").unwrap()).unwrap();
    let lir = hir.lower_lir(LoweringMode::Profiled).unwrap();
    assert_eq!(
        lir.blocks
            .iter()
            .flat_map(|b| &b.instructions)
            .filter(|i| matches!(i.operation, Operation::SequenceUnpack { .. }))
            .count(),
        2
    );
    assert_eq!(
        lir.blocks
            .iter()
            .flat_map(|b| &b.instructions)
            .filter(|i| matches!(i.operation, Operation::Copy(_)))
            .count(),
        6
    );
    lir.verify().unwrap();
}

#[test]
fn reused_results_survive_callbacks_gc_and_guard_failures() {
    let source = r#"
def churn(xs):
    xs.clear()
    for i in range(3000):
        unused = [i]
def f(xs):
    a,b=xs
    a,b=xs
    churn(xs)
    return a[0], b[0]
for i in range(160):
    assert f([[1],[2]]) == (1,2)
def mutate(xs):
    a,b=xs
    a,b=xs
    xs[0]=99
    c,d=xs
    c,d=xs
    return a,b,c,d
for i in range(160):
    assert mutate([1,2]) == (1,2,99,2)
def fail(xs):
    print('once')
    a,b=xs
    a,b=xs
    return a+b
for i in range(160):
    assert fail([1,2]) == 3
"#;
    for tier in [Tiering::Lir, Tiering::Native, Tiering::CopyPatch] {
        let mut vm = Vm::new();
        vm.tiering = tier;
        vm.readonly_regions = true;
        vm.mutation_regions = true;
        vm.run_program(&mut compile(source).unwrap()).unwrap();
        vm.output.clear();
        assert!(vm.run_program(&mut compile("fail([1])").unwrap()).is_err());
        assert_eq!(vm.output, "once\n");
        vm.output.clear();
        assert!(
            vm.run_program(&mut compile("fail(iter([1,2]))").unwrap())
                .is_err()
        );
        assert_eq!(vm.output, "once\n");
        vm.collect_garbage(&[]).unwrap();
    }
}

#[test]
fn general_iterators_are_unpacked_each_time() {
    let source = "def f(xs):\n    a,b=xs\n    a,b=xs\n    return a,b\nfor i in range(160):\n    assert f(iter([1,2])) == (1,2)\n";
    // Each unpack exhausts its input; an optimizer must not turn the second into a reuse.
    for tier in [
        Tiering::Bytecode,
        Tiering::Lir,
        Tiering::Native,
        Tiering::CopyPatch,
    ] {
        let mut vm = Vm::new();
        vm.tiering = tier;
        assert!(vm.run_program(&mut compile(source).unwrap()).is_err());
    }
}
