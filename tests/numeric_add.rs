use tis::{Tiering, Value, Vm, compile, lir::Operation, profile::ProfileMode};

fn warmed(tier: Tiering) -> Vm {
    let mut vm = Vm::new();
    vm.tiering = tier;
    vm.thresholds.function_entries = 1;
    vm.thresholds.loop_backedges = 4;
    vm.run_program(&mut compile("events=[]\ndef add(a,b):\n    events.append('before')\n    a+=b\n    events.append('after')\n    return a\nfor i in range(300):\n    add(i,i)\n    add(0.5,i)\n    add(i,0.25)\n    add(0.5,0.25)\n").unwrap()).unwrap();
    vm
}

#[test]
fn numeric_union_preserves_result_types_bits_and_all_tier_fallbacks() {
    for tier in [Tiering::Lir, Tiering::Native, Tiering::CopyPatch] {
        let mut vm = warmed(tier);
        let add = vm.get_global("add").unwrap();
        let hir = vm.function_hir(add).unwrap();
        let lir = hir.lower_lir(tis::lir::LoweringMode::Profiled).unwrap();
        assert!(
            lir.blocks
                .iter()
                .flat_map(|b| &b.instructions)
                .any(|i| matches!(i.operation, Operation::NumericAdd { .. }))
        );
        for (a, b, expected) in [
            (Value::Int(2), Value::Int(3), Value::Int(5)),
            (
                Value::Int(i64::MAX),
                Value::Int(-1),
                Value::Int(i64::MAX - 1),
            ),
            (
                Value::Int(i64::MIN),
                Value::Int(1),
                Value::Int(i64::MIN + 1),
            ),
            (
                Value::Int(i64::MAX),
                Value::Float(0.5),
                Value::Float(i64::MAX as f64 + 0.5),
            ),
            (
                Value::Float(0.5),
                Value::Int(i64::MIN),
                Value::Float(0.5 + i64::MIN as f64),
            ),
            (Value::Float(-0.0), Value::Float(-0.0), Value::Float(-0.0)),
            (
                Value::Float(f64::INFINITY),
                Value::Float(1.0),
                Value::Float(f64::INFINITY),
            ),
        ] {
            let actual = vm.call(add, &[a, b]).unwrap();
            match (actual, expected) {
                (Value::Float(a), Value::Float(b)) => assert_eq!(a.to_bits(), b.to_bits()),
                (a, b) => assert_eq!(a, b),
            }
        }
        let nan = vm
            .call(add, &[Value::Float(f64::NAN), Value::Int(1)])
            .unwrap();
        assert!(matches!(nan,Value::Float(v) if v.is_nan()));
        vm.profiling.mode = ProfileMode::Eager;
        assert_eq!(
            vm.call(add, &[Value::Int(1), Value::Float(2.5)]).unwrap(),
            Value::Float(3.5)
        );

        // A nonnumeric miss must execute inplace list addition once and preserve receiver identity.
        vm.run_program(&mut compile("x=[1]\ny=add(x,[2])\nassert x==[1,2]\nassert y is x\nassert events[-2:]==['before','after']\n").unwrap()).unwrap();
        assert!(vm.tier_stats.deoptimizations > 0);
        assert_eq!(
            vm.call(add, &[Value::Bool(true), Value::Int(2)]).unwrap(),
            Value::Int(3)
        );
        assert_eq!(vm.stack_len(), 0);
    }
}

#[test]
fn numeric_union_overflow_and_type_exits_keep_prior_effects() {
    for tier in [Tiering::Lir, Tiering::Native, Tiering::CopyPatch] {
        for args in [
            [Value::Int(i64::MAX), Value::Int(1)],
            [Value::Int(i64::MIN), Value::Int(-1)],
            [Value::None, Value::Int(1)],
        ] {
            let mut vm = warmed(tier);
            let add = vm.get_global("add").unwrap();
            if args[0] == Value::None {
                assert!(vm.call(add, &args).is_err());
                vm.run_program(
                    &mut compile("assert events[-1]=='before'\nassert events[-2]=='after'\n")
                        .unwrap(),
                )
                .unwrap();
            } else {
                let result = vm.call(add, &args).unwrap();
                let expected = if args[1] == Value::Int(1) {
                    "9223372036854775808"
                } else {
                    "-9223372036854775809"
                };
                assert!(
                    matches!(vm.heap.get(result),Ok(tis::Object::BigInt(n)) if n.to_string()==expected)
                );
                vm.run_program(&mut compile("assert events[-1]=='after'\nassert events[-2]=='before'\nassert events[-3]=='after'\n").unwrap()).unwrap();
            }
            assert!(vm.tier_stats.deoptimizations > 0);
            assert_eq!(vm.stack_len(), 0);
            vm.collect_garbage(&[]).unwrap();
        }
    }
}

#[test]
fn numeric_union_keeps_live_references_across_gc_and_osr() {
    for tier in [Tiering::Lir, Tiering::Native, Tiering::CopyPatch] {
        let mut vm = warmed(tier);
        vm.run_program(&mut compile("enabled=False\ndef keep(a,b):\n    live=[a]\n    result=a+b\n    for i in range(7000 if enabled else 0):\n        trash=[i]\n    return live,result\nfor i in range(300):\n    keep(1,2)\n    keep(1.5,2)\nenabled=True\nassert keep(1.25,2)==([1.25],3.25)\nassert keep([1],[2])==([[1]],[1,2])\n").unwrap()).unwrap();
        vm.collect_garbage(&[]).unwrap();
        assert_eq!(vm.stack_len(), 0);

        let mut osr = Vm::new();
        osr.tiering = tier;
        osr.thresholds.function_entries = u64::MAX;
        osr.thresholds.loop_backedges = 4;
        osr.profiling.window_ticks = 4;
        osr.run_program(&mut compile("def total(n):\n    x=0\n    for i in range(n):\n        x+=0.5 if i%2 else 1\n    return x\nassert total(10000)==7500.0\n").unwrap()).unwrap();
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

#[test]
fn numeric_union_verifier_rejects_missing_overflow_exit() {
    let vm = warmed(Tiering::Lir);
    let add = vm.get_global("add").unwrap();
    let mut lir = vm
        .function_hir(add)
        .unwrap()
        .lower_lir(tis::lir::LoweringMode::Profiled)
        .unwrap();
    let instruction = lir
        .blocks
        .iter_mut()
        .flat_map(|b| &mut b.instructions)
        .find(|i| matches!(i.operation, Operation::NumericAdd { .. }))
        .unwrap();
    if let Operation::NumericAdd {
        guard, overflow, ..
    } = &mut instruction.operation
    {
        *overflow = *guard;
    }
    assert!(lir.verify().is_err());
}
