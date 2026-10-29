use tis::{
    Tiering, Value, Vm, compile,
    lir::{LoweringMode, Operation},
};

fn execute(source: &str, tier: Tiering) -> Vm {
    let mut vm = Vm::new();
    vm.tiering = tier;
    vm.thresholds.function_entries = 1;
    vm.thresholds.loop_backedges = 4;
    vm.run_program(&mut compile(source).unwrap()).unwrap();
    vm
}

fn comparisons(integer: i64) -> String {
    let mut source = String::new();
    for (i, op) in ["==", "!=", "<", "<=", ">", ">="].iter().enumerate() {
        source += &format!(
            "def f{i}(x):\n    return x {op} {integer}\ndef r{i}(x):\n    return {integer} {op} x\n"
        );
    }
    source += "for i in range(300):\n";
    for i in 0..6 {
        source += &format!("    f{i}(i%2)\n    f{i}(0.5)\n    r{i}(i%2)\n    r{i}(0.5)\n");
    }
    source
}

#[test]
fn literal_comparisons_preserve_all_relations_and_integer_precision() {
    for literal in [0, 1, 9_007_199_254_740_992] {
        let source = comparisons(literal);
        let mut control = execute(&source, Tiering::Bytecode);
        for tier in [Tiering::Lir, Tiering::Native, Tiering::CopyPatch] {
            let mut vm = execute(&source, tier);
            for prefix in ["f", "r"] {
                for i in 0..6 {
                    let name = format!("{prefix}{i}");
                    let f = vm.get_global(&name).unwrap();
                    let baseline = control.get_global(&name).unwrap();
                    let lir = vm
                        .function_hir(f)
                        .unwrap()
                        .lower_lir(LoweringMode::Profiled)
                        .unwrap();
                    assert!(
                        lir.blocks
                            .iter()
                            .flat_map(|b| &b.instructions)
                            .any(|i| matches!(i.operation, Operation::NumberCompare { .. }))
                    );
                    for value in [
                        Value::Int(i64::MIN),
                        Value::Int(i64::MAX),
                        Value::Int(-1),
                        Value::Int(0),
                        Value::Int(1),
                        Value::Int(2),
                        Value::Int(9_007_199_254_740_991),
                        Value::Int(9_007_199_254_740_992),
                        Value::Int(9_007_199_254_740_993),
                        Value::Float(-0.0),
                        Value::Float(0.0),
                        Value::Float(0.5),
                        Value::Float(f64::NEG_INFINITY),
                        Value::Float(f64::INFINITY),
                        Value::Float(f64::NAN),
                        Value::Float(9_007_199_254_740_992.0),
                    ] {
                        assert_eq!(
                            vm.call(f, &[value]).unwrap(),
                            control.call(baseline, &[value]).unwrap(),
                            "{tier:?} {name} {literal} {value:?}"
                        );
                    }
                    let origin = lir
                        .blocks
                        .iter()
                        .flat_map(|b| &b.instructions)
                        .find(|i| matches!(i.operation, Operation::NumberCompare { .. }))
                        .unwrap()
                        .origin;
                    let before = vm.function_hir(f).unwrap().profile().sites[origin.0].executions;
                    vm.profiling.mode = tis::profile::ProfileMode::Eager;
                    assert_eq!(
                        vm.call(f, &[Value::Float(0.5)]).unwrap(),
                        control.call(baseline, &[Value::Float(0.5)]).unwrap()
                    );
                    assert_eq!(
                        vm.function_hir(f).unwrap().profile().sites[origin.0].executions,
                        before + 1
                    );
                    // Bool is deliberately outside the exact numeric guard and resumes Python semantics.
                    assert_eq!(
                        vm.call(f, &[Value::Bool(true)]).unwrap(),
                        control.call(baseline, &[Value::Bool(true)]).unwrap()
                    );
                }
            }
            assert!(vm.tier_stats.deoptimizations > 0);
            assert_eq!(vm.stack_len(), 0);
        }
    }
}

#[test]
fn inexact_literals_stay_generic_and_verifier_rejects_saturating_roundtrip() {
    for literal in [9_007_199_254_740_993, i64::MAX] {
        let vm = execute(&comparisons(literal), Tiering::Native);
        let f = vm.get_global("f0").unwrap();
        let lir = vm
            .function_hir(f)
            .unwrap()
            .lower_lir(LoweringMode::Profiled)
            .unwrap();
        assert!(
            !lir.blocks
                .iter()
                .flat_map(|b| &b.instructions)
                .any(|i| matches!(i.operation, Operation::NumberCompare { .. }))
        );
    }
    let vm = execute(&comparisons(1), Tiering::Native);
    let mut lir = vm
        .function_hir(vm.get_global("f0").unwrap())
        .unwrap()
        .lower_lir(LoweringMode::Profiled)
        .unwrap();
    for instruction in lir.blocks.iter_mut().flat_map(|b| &mut b.instructions) {
        if let Operation::NumberCompare { integer, .. } = &mut instruction.operation {
            *integer = i64::MAX;
        }
    }
    assert!(lir.verify().is_err());
}

#[test]
fn comparison_exits_keep_prefix_effects_gc_roots_and_osr_state() {
    for tier in [Tiering::Lir, Tiering::Native, Tiering::CopyPatch] {
        let mut vm = execute(
            "events=[]\nenabled=False\ndef f(x):\n    events.append('before')\n    live=[x]\n    result=x==0\n    for i in range(7000 if enabled else 0):\n        trash=[i]\n    events.append('after')\n    return live,result\nfor i in range(300):\n    f(0)\n    f(0.5)\nenabled=True\nassert f(0.0)==([0.0],True)\nn=len(events)\nassert f([0])==([[0]],False)\nassert len(events)==n+2\nassert events[-2:]==['before','after']\n",
            tier,
        );
        assert!(vm.tier_stats.deoptimizations > 0);
        vm.collect_garbage(&[]).unwrap();
        assert_eq!(vm.stack_len(), 0);

        let mut osr = Vm::new();
        osr.tiering = tier;
        osr.thresholds.function_entries = u64::MAX;
        osr.thresholds.loop_backedges = 4;
        osr.profiling.window_ticks = 4;
        osr.run_program(&mut compile("def count(n):\n    total=0\n    while n:\n        if n%2==0:\n            total+=1\n        n-=1\n    return total\nassert count(10000)==5000\n").unwrap()).unwrap();
        assert!(
            match tier {
                Tiering::Native => osr.tier_stats.native_osr_entries,
                Tiering::CopyPatch => osr.tier_stats.copy_patch_osr_entries,
                _ => osr.tier_stats.osr_entries,
            } > 0
        );
        assert_eq!(osr.stack_len(), 0);
    }
}
