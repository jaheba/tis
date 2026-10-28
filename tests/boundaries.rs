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

#[test]
fn helpers_keep_full_deopt_state_but_only_safepoints_have_gc_maps() {
    use tis::lir::{Helper, LoweringMode, Operation};
    let mut program =
        compile("def f(a):\n    a[0]=1\n    if a:\n        return [a]\n    return []\n").unwrap();
    let f = program.function_mut("f").unwrap();
    let mut lir = f.lower_lir(LoweringMode::Generic).unwrap();
    let mut nonallocating = 0;
    for block in &lir.blocks {
        for instruction in &block.instructions {
            match &instruction.operation {
                Operation::Call {
                    helper: Helper::SetItem,
                    effects,
                    state,
                    roots,
                    ..
                }
                | Operation::Truth {
                    effects,
                    state,
                    roots,
                    ..
                } => {
                    assert!(!effects.needs_safepoint());
                    assert!(roots.is_empty());
                    assert!(!lir.states[state.0].values.is_empty());
                    nonallocating += 1;
                }
                _ => {}
            }
        }
    }
    assert!(nonallocating >= 2);
    // A forged non-allocating annotation cannot suppress the roots for a real allocator.
    let mut altered = false;
    for block in &mut lir.blocks {
        for instruction in &mut block.instructions {
            if let Operation::Call {
                helper: Helper::List,
                effects,
                roots,
                ..
            } = &mut instruction.operation
            {
                effects.may_allocate = false;
                effects.may_call = false;
                roots.clear();
                altered = true;
            }
        }
    }
    assert!(altered);
    assert!(lir.verify().is_err());
}

#[test]
fn nonallocating_mutation_and_truth_keep_live_values_at_later_collections() {
    let source = "
def churn(n):
    for i in range(n):
        garbage=[i]

def f(n):
    box=[[99]]
    saved=box[0]
    box[0]=[7]
    if saved:
        churn(n)
    return saved[0]+box[0][0]
";
    for tier in [Tiering::Lir, Tiering::Native, Tiering::CopyPatch] {
        let mut program = compile(source).unwrap();
        let mut vm = Vm::new();
        vm.tiering = tier;
        vm.thresholds.function_entries = 1;
        vm.thresholds.loop_backedges = 1;
        vm.profiling.window_ticks = 16;
        vm.run_program(&mut program).unwrap();
        let f = vm.get_global("f").unwrap();
        for _ in 0..24 {
            assert_eq!(vm.call(f, &[Value::Int(1)]).unwrap(), Value::Int(106));
        }
        for _ in 0..3 {
            assert_eq!(vm.call(f, &[Value::Int(10000)]).unwrap(), Value::Int(106));
        }
        vm.collect_garbage(&[f]).unwrap();
        assert_eq!(vm.call(f, &[Value::Int(10000)]).unwrap(), Value::Int(106));
    }
}

#[test]
fn recursive_guard_failure_refreshes_feedback_in_the_active_parent() {
    let source = "
def f(depth,x):
    y=x+1
    if depth:
        return f(0,2.5)+y
    return y*2
";
    for tier in [Tiering::Native, Tiering::CopyPatch] {
        let mut program = compile(source).unwrap();
        let mut vm = Vm::new();
        vm.tiering = tier;
        vm.thresholds.function_entries = 1;
        vm.profiling.window_ticks = 16;
        vm.run_program(&mut program).unwrap();
        let f = vm.get_global("f").unwrap();
        for _ in 0..24 {
            vm.call(f, &[Value::Int(0), Value::Int(3)]).unwrap();
        }
        let probes = vm.tier_stats.native_feedback_probes;
        assert_eq!(
            vm.call(f, &[Value::Int(1), Value::Int(3)]).unwrap(),
            Value::Float(11.0)
        );
        assert!(vm.tier_stats.deoptimizations > 0);
        assert!(vm.tier_stats.native_feedback_probes > probes);
        let hir = vm.function_hir(f).unwrap();
        let lir = hir.lower_lir(tis::lir::LoweringMode::Generic).unwrap();
        let adds = lir
            .blocks
            .iter()
            .flat_map(|b| &b.instructions)
            .filter_map(|i| {
                matches!(
                    i.operation,
                    tis::lir::Operation::Call {
                        helper: tis::lir::Helper::Add,
                        ..
                    }
                )
                .then_some(i.origin)
            });
        assert!(adds.into_iter().any(|id| {
            hir.profile().sites[id.0]
                .operands
                .first()
                .is_some_and(|p| p.count(tis::profile::ValueKind::Float) > 0)
        }));
    }
}

#[test]
fn allocation_free_native_bodies_still_collect_call_setup_garbage() {
    for tier in [Tiering::Lir, Tiering::Native, Tiering::CopyPatch] {
        let mut program = compile("def f(x):\n    return x+1\n").unwrap();
        let mut vm = Vm::new();
        vm.tiering = tier;
        vm.thresholds.function_entries = 1;
        vm.profiling.window_ticks = 16;
        vm.run_program(&mut program).unwrap();
        let f = vm.get_global("f").unwrap();
        for _ in 0..10000 {
            assert_eq!(vm.call(f, &[Value::Int(1)]).unwrap(), Value::Int(2));
        }
        assert!(vm.heap.allocation_count() < 4096);
    }
}
