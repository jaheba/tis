use tis::{BytecodeMode, NumericMode, Tiering, Value, Vm, compile};

fn setup(body: &str, mode: NumericMode) -> (Vm, Value) {
    let mut program = compile(body).unwrap();
    let mut vm = Vm::new();
    vm.numeric = mode;
    vm.thresholds.function_entries = 1;
    vm.thresholds.loop_backedges = 1;
    vm.profiling.window_ticks = 16;
    vm.run_program(&mut program).unwrap();
    let f = vm.get_global("f").unwrap();
    (vm, f)
}
fn warm(vm: &mut Vm, f: Value, a: Value, b: Value) {
    for _ in 0..24 {
        vm.call(f, &[a, b]).unwrap();
    }
    assert!(vm.tier_stats.numeric_specializations > 0);
    assert!(vm.tier_stats.numeric_fast_paths > 0);
}

#[test]
fn float_and_integer_paths_match_generic_results() {
    for op in ["+", "-", "*", "/", "**", "==", "!=", "<", "<=", ">", ">="] {
        for (a, b) in [
            (Value::Float(2.5), Value::Float(3.0)),
            (Value::Int(7), Value::Int(3)),
        ] {
            // Integer power is deliberately left to checked generic arithmetic.
            if op == "**" && matches!(a, Value::Int(_)) {
                continue;
            }
            let source = format!("def f(a,b):\n    return a {op} b\n");
            let (mut vm, f) = setup(&source, NumericMode::Adaptive);
            warm(&mut vm, f, a, b);
            let (mut generic, g) = setup(&source, NumericMode::Disabled);
            assert_eq!(
                vm.call(f, &[a, b]).unwrap(),
                generic.call(g, &[a, b]).unwrap()
            );
        }
    }
}

#[test]
fn type_misses_record_feedback_and_preserve_container_inplace_behavior() {
    let (mut vm, f) = setup(
        "def f(a,b):\n    a *= b\n    return a\n",
        NumericMode::Adaptive,
    );
    warm(&mut vm, f, Value::Float(2.0), Value::Float(3.0));
    let list = vm
        .heap
        .allocate(tis::Object::List(vec![Value::Int(7)].into()));
    assert_eq!(vm.call(f, &[list, Value::Int(2)]).unwrap(), list);
    assert!(matches!(vm.heap.get(list).unwrap(), tis::Object::List(items) if items.len() == 2));
    assert_eq!(vm.tier_stats.numeric_type_misses, 1);
    let hir = vm.function_hir(f).unwrap();
    assert!(hir.profile().sites.iter().any(|site| {
        site.guard_failures > 0
            && site
                .operands
                .iter()
                .any(|p| p.count(tis::profile::ValueKind::List) > 0)
    }));
    for _ in 0..32 {
        vm.call(f, &[Value::Float(2.0), Value::Float(3.0)]).unwrap();
    }
    assert_eq!(vm.tier_stats.numeric_type_misses, 1);
}

#[test]
fn arithmetic_failures_fall_back_once_without_replaying_effects() {
    for (op, a, b, bad_a, bad_b) in [
        (
            "/",
            Value::Float(2.0),
            Value::Float(3.0),
            Value::Float(2.0),
            Value::Float(-0.0),
        ),
        (
            "**",
            Value::Float(2.0),
            Value::Float(3.0),
            Value::Float(-1.0),
            Value::Float(0.5),
        ),
        (
            "*",
            Value::Int(2),
            Value::Int(3),
            Value::Int(i64::MAX),
            Value::Int(2),
        ),
        (
            "+",
            Value::Int(2),
            Value::Int(3),
            Value::Int(i64::MAX),
            Value::Int(1),
        ),
        (
            "-",
            Value::Int(2),
            Value::Int(3),
            Value::Int(i64::MIN),
            Value::Int(1),
        ),
    ] {
        let source = format!(
            "count=0\ndef f(a,b):\n    global count\n    count += 1\n    return a {op} b\n"
        );
        let (mut vm, f) = setup(&source, NumericMode::Adaptive);
        warm(&mut vm, f, a, b);
        let before = vm.get_global("count").unwrap();
        let (mut generic, g) = setup(&source, NumericMode::Disabled);
        match (
            vm.call(f, &[bad_a, bad_b]),
            generic.call(g, &[bad_a, bad_b]),
        ) {
            (Err(a), Err(b)) => assert_eq!(a.to_string(), b.to_string()),
            (Ok(a), Ok(b)) => assert!(
                matches!((vm.heap.get(a),generic.heap.get(b)),(Ok(tis::Object::BigInt(a)),Ok(tis::Object::BigInt(b))) if a==b)
            ),
            other => panic!("numeric fallback mismatch: {other:?}"),
        }
        let Value::Int(before) = before else { panic!() };
        assert_eq!(vm.get_global("count").unwrap(), Value::Int(before + 1));
        assert_eq!(vm.tier_stats.numeric_arithmetic_fallbacks, 1);
        vm.call(f, &[a, b]).unwrap();
    }
}

#[test]
fn integer_comparisons_do_not_round_and_mixed_comparisons_stay_generic() {
    let (mut vm, f) = setup("def f(a,b):\n    return a > b\n", NumericMode::Adaptive);
    warm(&mut vm, f, Value::Int(7), Value::Int(3));
    assert_eq!(
        vm.call(f, &[Value::Int(i64::MAX), Value::Int(i64::MAX - 1)])
            .unwrap(),
        Value::Bool(true)
    );
    let (mut vm, f) = setup("def f(a,b):\n    return a > b\n", NumericMode::Adaptive);
    for _ in 0..24 {
        vm.call(f, &[Value::Int(i64::MAX), Value::Float(i64::MAX as f64)])
            .unwrap();
    }
    assert_eq!(vm.tier_stats.numeric_fast_paths, 0);
}

#[test]
fn disabling_and_switching_encodings_preserves_warmed_code() {
    let (mut vm, f) = setup("def f(a,b):\n    return a*b\n", NumericMode::Adaptive);
    warm(&mut vm, f, Value::Float(2.0), Value::Float(3.0));
    for mode in [NumericMode::Disabled, NumericMode::Adaptive] {
        vm.numeric = mode;
        for encoding in [BytecodeMode::Stack, BytecodeMode::Register] {
            vm.bytecode = encoding;
            assert_eq!(
                vm.call(f, &[Value::Int(4), Value::Int(5)]).unwrap(),
                Value::Int(20)
            );
        }
    }
    vm.tiering = Tiering::Native;
    for _ in 0..24 {
        assert_eq!(
            vm.call(f, &[Value::Int(4), Value::Int(5)]).unwrap(),
            Value::Int(20)
        );
    }
}

#[test]
fn nan_infinity_and_signed_zero_match_generic_semantics() {
    for op in ["+", "-", "*", "/", "**", "==", "!=", "<", "<=", ">", ">="] {
        let source = format!("def f(a,b):\n    return a {op} b\n");
        let (mut vm, f) = setup(&source, NumericMode::Adaptive);
        warm(&mut vm, f, Value::Float(2.5), Value::Float(3.0));
        let (mut generic, g) = setup(&source, NumericMode::Disabled);
        for (a, b) in [
            (f64::NAN, 1.0),
            (f64::INFINITY, f64::INFINITY),
            (-0.0, 3.0),
            (0.0, -1.0),
        ] {
            let arguments = [Value::Float(a), Value::Float(b)];
            match (vm.call(f, &arguments), generic.call(g, &arguments)) {
                (Ok(Value::Float(a)), Ok(Value::Float(b))) => {
                    assert!(a.to_bits() == b.to_bits() || (a.is_nan() && b.is_nan()))
                }
                (Ok(a), Ok(b)) => assert_eq!(a, b),
                (Err(a), Err(b)) => assert_eq!(a.to_string(), b.to_string()),
                result => panic!("different results for {op}: {result:?}"),
            }
        }
    }
}

#[test]
fn changed_hir_drops_numeric_plans_with_the_old_code_revision() {
    use tis::hir::{Constant, Function, Op, Terminator};
    let mut f = Function::new(2);
    let block = f.entry();
    let result = f.append(
        block,
        Op::Subtract(tis::hir::ValueId(0), tis::hir::ValueId(1)),
    );
    f.terminate(block, Terminator::Return(result));
    let mut vm = Vm::new();
    vm.thresholds.function_entries = 1;
    vm.profiling.window_ticks = 16;
    for _ in 0..24 {
        vm.run(&mut f, &[Value::Float(4.0), Value::Float(1.0)])
            .unwrap();
    }
    assert!(
        f.bytecode()
            .unwrap()
            .disassemble_register()
            .unwrap()
            .contains("specialized @")
    );
    let old_revision = f.revision();
    let value = f.append(block, Op::Constant(Constant::Int(42)));
    f.terminate(block, Terminator::Return(value));
    assert_ne!(old_revision, f.revision());
    assert!(
        !f.bytecode()
            .unwrap()
            .disassemble_register()
            .unwrap()
            .contains("specialized @")
    );
    assert_eq!(
        vm.run(&mut f, &[Value::Float(4.0), Value::Float(1.0)])
            .unwrap(),
        Value::Int(42)
    );
}
