use tis::{
    Tiering, Value, Vm, compile,
    hir::{Constant, Function, Op, Terminator},
    lir::ExitReason,
    profile::{HotThresholds, ProfileConfig, ProfilePhase, ValueKind},
};

fn vm(tier: Tiering) -> Vm {
    let mut vm = Vm::new();
    vm.tiering = tier;
    vm.thresholds = HotThresholds {
        function_entries: 4,
        loop_backedges: 4,
    };
    vm.profiling = ProfileConfig {
        window_ticks: 8,
        site_samples: 8,
        ..ProfileConfig::default()
    };
    vm
}

fn add(vm: &mut Vm) -> Value {
    let mut program = compile("def add(a,b):\n    return a+b\n").unwrap();
    vm.run_program(&mut program).unwrap();
    vm.get_global("add").unwrap()
}

#[test]
fn cold_code_has_counters_but_no_type_or_call_metadata() {
    let mut vm = vm(Tiering::Bytecode);
    let f = add(&mut vm);
    for _ in 0..3 {
        vm.call(f, &[Value::Int(1), Value::Int(2)]).unwrap();
    }
    let p = vm.function_hir(f).unwrap().profile();
    assert_eq!(p.entries, 3);
    assert_eq!(p.returns, 3);
    assert_eq!(p.phase, ProfilePhase::Cold);
    assert!(p.arguments.is_empty());
    assert!(p.edges.is_empty());
    assert_eq!(p.return_types.observations(), 0);
    assert!(
        p.sites
            .iter()
            .all(|s| s.executions == 0 && s.operands.is_empty() && s.call_targets.is_empty())
    );
    assert!(vm.take_compilation_requests().is_empty());
}

#[test]
fn compilation_waits_for_a_bounded_observation_window_and_then_freezes_it() {
    for tier in [
        Tiering::Bytecode,
        Tiering::Lir,
        Tiering::Native,
        Tiering::CopyPatch,
    ] {
        let mut vm = vm(tier);
        let f = add(&mut vm);
        for i in 0..11 {
            vm.call(f, &[Value::Int(i), Value::Int(2)]).unwrap();
            assert_eq!(vm.tier_stats.lowered, 0);
        }
        assert_eq!(
            vm.function_hir(f).unwrap().profile().phase,
            ProfilePhase::Ready
        );
        vm.call(f, &[Value::Int(1), Value::Int(2)]).unwrap();
        let requests = vm.take_compilation_requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].count, 12);
        let before = vm.function_hir(f).unwrap().profile();
        assert_eq!(before.windows, 1);
        assert_eq!(
            before.phase,
            if tier == Tiering::Bytecode {
                ProfilePhase::Ready
            } else {
                ProfilePhase::Compiled
            }
        );
        assert!(before.sites.iter().any(|s| s.executions == 8));
        for i in 0..100 {
            vm.call(f, &[Value::Int(i), Value::Int(2)]).unwrap();
        }
        let after = vm.function_hir(f).unwrap().profile();
        assert_eq!(after.entries, before.entries + 100);
        assert_eq!(after.edges, before.edges);
        for (a, b) in before.sites.iter().zip(&after.sites) {
            assert_eq!(a.executions, b.executions);
            assert_eq!(a.result.observations(), b.result.observations());
        }
        assert!(vm.take_compilation_requests().is_empty());
    }
}

#[test]
fn hot_loop_profiles_and_enters_native_code_during_its_first_call() {
    for tier in [Tiering::Lir, Tiering::Native, Tiering::CopyPatch] {
        let mut vm = vm(tier);
        vm.thresholds.function_entries = 1000;
        vm.profiling.window_ticks = 32;
        let mut p = compile("def loop(n):\n    total=0\n    for i in range(n):\n        total=total+i\n    return total\nassert loop(500)==124750\n").unwrap();
        vm.run_program(&mut p).unwrap();
        let f = vm.function_hir(vm.get_global("loop").unwrap()).unwrap();
        assert_eq!(f.profile().entries, 1);
        assert_eq!(f.profile().phase, ProfilePhase::Compiled);
        assert_eq!(vm.tier_stats.osr_entries, 1);
        assert!(
            f.lower_lir(tis::lir::LoweringMode::Profiled)
                .unwrap()
                .exits
                .iter()
                .any(|exit| exit.reason == ExitReason::TypeGuard)
        );
        assert!(f.profile().sites.iter().all(|site| site.executions <= 8));
    }
}

#[test]
fn guard_miss_records_unexpected_types_then_reprofiles_without_replaying_effects() {
    for tier in [Tiering::Lir, Tiering::Native, Tiering::CopyPatch] {
        let mut vm = vm(tier);
        let mut p = compile("calls=[0]\ndef f(a,b):\n    calls[0]+=1\n    saved=[42]\n    value=a+b\n    return saved[0],value\n").unwrap();
        vm.run_program(&mut p).unwrap();
        let f = vm.get_global("f").unwrap();
        for _ in 0..12 {
            vm.call(f, &[Value::Int(1), Value::Int(2)]).unwrap();
        }
        assert_eq!(
            vm.function_hir(f).unwrap().profile().phase,
            ProfilePhase::Compiled
        );
        let result = vm.call(f, &[Value::Float(0.5), Value::Float(1.5)]).unwrap();
        assert_eq!(
            vm.heap.get_item(result, Value::Int(0)).unwrap(),
            Value::Int(42)
        );
        assert_eq!(
            vm.heap.get_item(result, Value::Int(1)).unwrap(),
            Value::Float(2.0)
        );
        let profile = vm.function_hir(f).unwrap().profile();
        assert_eq!(profile.phase, ProfilePhase::Profiling);
        assert_eq!(profile.windows, 2);
        assert!(profile.sites.iter().any(|site| {
            site.exit_reasons.get(&ExitReason::TypeGuard) == Some(&1)
                && site
                    .operands
                    .iter()
                    .any(|operand| operand.count(ValueKind::Float) > 0)
        }));
        for _ in 0..12 {
            vm.call(f, &[Value::Float(0.5), Value::Float(1.5)]).unwrap();
        }
        assert_eq!(
            vm.function_hir(f).unwrap().profile().phase,
            ProfilePhase::Compiled
        );
        assert_eq!(vm.tier_stats.deoptimizations, 1);
        assert_eq!(
            vm.heap
                .get_item(vm.get_global("calls").unwrap(), Value::Int(0))
                .unwrap(),
            Value::Int(25)
        );
        vm.collect_garbage(&[]).unwrap();
    }
}

#[test]
fn profiling_retry_limit_retires_automatic_compilation() {
    let mut vm = vm(Tiering::CopyPatch);
    vm.profiling.max_windows = 1;
    let f = add(&mut vm);
    for _ in 0..12 {
        vm.call(f, &[Value::Int(1), Value::Int(2)]).unwrap();
    }
    vm.call(f, &[Value::Float(0.5), Value::Float(1.5)]).unwrap();
    assert_eq!(
        vm.function_hir(f).unwrap().profile().phase,
        ProfilePhase::Retired
    );
    let lowered = vm.tier_stats.lowered;
    for _ in 0..100 {
        vm.call(f, &[Value::Int(1), Value::Int(2)]).unwrap();
    }
    assert_eq!(vm.tier_stats.lowered, lowered);
    assert_eq!(vm.function_hir(f).unwrap().profile().windows, 1);
}

#[test]
fn bytecode_guard_failures_are_observed_even_when_collection_is_frozen() {
    let mut vm = vm(Tiering::Bytecode);
    let f = add(&mut vm);
    for _ in 0..12 {
        vm.call(f, &[Value::Int(1), Value::Int(2)]).unwrap();
    }
    assert_eq!(
        vm.function_hir(f).unwrap().profile().phase,
        ProfilePhase::Ready
    );
    vm.call(f, &[Value::Float(0.5), Value::Float(1.5)]).unwrap();
    let profile = vm.function_hir(f).unwrap().profile();
    assert_eq!(profile.windows, 2);
    assert!(profile.sites.iter().any(|site| {
        site.guard_failures > 0
            && site
                .operands
                .iter()
                .any(|operand| operand.count(ValueKind::Float) > 0)
    }));
}

#[test]
fn code_revision_starts_cold_with_a_new_window_budget() {
    let mut f = Function::new(0);
    let b = f.entry();
    let value = f.append(b, Op::Constant(Constant::Int(1)));
    f.terminate(b, Terminator::Return(value));
    let mut vm = vm(Tiering::Bytecode);
    for _ in 0..12 {
        vm.run(&mut f, &[]).unwrap();
    }
    assert_eq!(f.profile().phase, ProfilePhase::Ready);
    let revision = f.revision();
    f.terminate(b, Terminator::Return(value));
    assert_ne!(f.revision(), revision);
    vm.run(&mut f, &[]).unwrap();
    assert_eq!(f.profile().phase, ProfilePhase::Cold);
    assert_eq!(f.profile().windows, 0);
    assert!(f.profile().sites.iter().all(|site| site.executions == 0));
}

#[test]
fn polymorphic_sites_stop_sampling_before_the_window_finishes() {
    let mut vm = vm(Tiering::Bytecode);
    vm.profiling.window_ticks = 64;
    let f = add(&mut vm);
    for i in 0..20 {
        let value = if i % 2 == 0 {
            Value::Float(1.0)
        } else {
            Value::Int(1)
        };
        vm.call(f, &[value, Value::Int(2)]).unwrap();
    }
    let profile = vm.function_hir(f).unwrap().profile();
    assert_eq!(profile.phase, ProfilePhase::Profiling);
    assert!(profile.sites.iter().all(|site| site.executions <= 8));
    assert!(profile.sites.iter().any(|site| {
        site.operands
            .iter()
            .any(|operand| operand.count(ValueKind::Int) > 0 && operand.count(ValueKind::Float) > 0)
    }));
    for _ in 0..100 {
        vm.call(f, &[Value::Float(1.0), Value::Int(2)]).unwrap();
    }
    let after = vm.function_hir(f).unwrap().profile();
    assert_eq!(after.phase, ProfilePhase::Ready);
    assert!(after.sites.iter().all(|site| site.executions <= 8));
}

#[test]
fn a_shared_profile_can_install_artifacts_in_another_vm() {
    let mut f = Function::new(2);
    let b = f.entry();
    let args = f.parameters(b);
    let result = f.append(b, Op::Add(args[0], args[1]));
    f.terminate(b, Terminator::Return(result));
    let mut first = vm(Tiering::Lir);
    for _ in 0..12 {
        first.run(&mut f, &[Value::Int(1), Value::Int(2)]).unwrap();
    }
    assert_eq!(f.profile().phase, ProfilePhase::Compiled);
    let mut second = vm(Tiering::CopyPatch);
    assert_eq!(
        second.run(&mut f, &[Value::Int(1), Value::Int(2)]).unwrap(),
        Value::Int(3)
    );
    assert_eq!(second.tier_stats.copy_patch_entries, 1);
}

#[test]
fn hot_call_targets_are_sampled_and_bounded() {
    let mut vm = vm(Tiering::Bytecode);
    let mut program = compile("functions=[lambda:1,lambda:2,lambda:3,lambda:4,lambda:5,lambda:6]\ndef invoke(f):\n    return f()\n").unwrap();
    vm.run_program(&mut program).unwrap();
    let functions = vm.get_global("functions").unwrap();
    let invoke = vm.get_global("invoke").unwrap();
    for i in 0..100 {
        let f = vm.heap.get_item(functions, Value::Int(i % 6)).unwrap();
        vm.call(invoke, &[f]).unwrap();
    }
    let profile = vm.function_hir(invoke).unwrap().profile();
    let site = profile
        .sites
        .iter()
        .find(|site| !site.call_targets.is_empty())
        .unwrap();
    assert_eq!(site.call_targets.len(), 4);
    assert_eq!(site.executions, 8);
    assert_eq!(
        site.call_targets.values().sum::<u64>() + site.other_targets,
        8
    );
    assert!(site.other_targets > 0);
}
