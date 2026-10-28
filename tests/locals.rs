use tis::{
    Tiering, Value, Vm, compile,
    hir::{CoreOp, Op},
    lir::LoweringMode,
    profile::HotThresholds,
};

const TIERS: [Tiering; 4] = [
    Tiering::Bytecode,
    Tiering::Lir,
    Tiering::Native,
    Tiering::CopyPatch,
];

fn execute(source: &str, tier: Tiering) -> Vm {
    let mut program = compile(source).unwrap();
    let mut vm = Vm::new();
    vm.profiling.window_ticks = 8;
    vm.tiering = tier;
    vm.thresholds = HotThresholds {
        function_entries: 2,
        loop_backedges: 4,
    };
    vm.run_program(&mut program).unwrap();
    assert_eq!(vm.stack_len(), 0);
    vm
}

#[test]
fn hir_distinguishes_slots_from_shared_bindings() {
    let mut program = compile("def outer(a):\n    scratch = 1\n    def inner():\n        nonlocal a\n        a += 1\n        return a\n    return inner, scratch\n").unwrap();
    let function = program.function_mut("outer").unwrap();
    assert!(function.local_slots().contains(&"scratch".into()));
    assert!(!function.local_slots().contains(&"a".into()));
    let dump = function.dump();
    assert!(!dump.contains("StoreLocal"));
    assert!(!dump.contains("LoadLocal"));
    let bytecode = function.bytecode().unwrap().disassemble().unwrap();
    assert!(!bytecode.contains("LoadLocal"));
    assert!(!bytecode.contains("StoreLocal"));
    let lir = function.lower_lir(LoweringMode::Generic).unwrap();
    assert_eq!(lir.local_count, function.local_slots().len());
    assert!(
        lir.states
            .iter()
            .all(|state| state.local_count == lir.local_count)
    );
}

#[test]
fn closures_classes_comprehensions_and_argument_binding_keep_scope_semantics() {
    let source = "g = 3\ndef make(a, /, b=2, *args, flag=4, **kw):\n    scratch = a+b+flag+len(args)+kw['extra']\n    def middle():\n        def inner():\n            nonlocal a\n            a += 1\n            return a\n        return inner\n    class C:\n        copied = a\n        def get(self):\n            return a\n    xs = [a+i for i in range(3)]\n    return middle(), C(), scratch, xs\ndef change():\n    global g\n    g += 1\nfor i in range(20):\n    f, obj, result, xs = make(10, 3, 1, 2, flag=5, extra=7)\n    assert result == 27\n    assert xs == [10,11,12]\n    assert obj.copied == 10\n    assert f() == 11\n    assert obj.get() == 11\n    assert f() == 12\n    change()\nassert g == 23\n";
    for tier in TIERS {
        let mut vm = execute(source, tier);
        vm.collect_garbage(&[]).unwrap();
    }
}

#[test]
fn unbound_slots_do_not_fall_back_to_globals_and_errors_release_locals() {
    for tier in TIERS {
        let mut vm = execute(
            "x=99\ndef f(set_it):\n    if set_it:\n        x=7\n    return x\nfor i in range(10):\n    assert f(True)==7\n",
            tier,
        );
        let function = vm.get_global("f").unwrap();
        let error = vm.call(function, &[Value::Bool(false)]).unwrap_err();
        assert!(error.to_string().contains("UnboundLocalError: x"));
        assert_eq!(vm.stack_len(), 0);
        assert_eq!(
            vm.call(function, &[Value::Bool(true)]).unwrap(),
            Value::Int(7)
        );
        vm.collect_garbage(&[]).unwrap();
    }
}

#[test]
fn slots_remain_roots_and_survive_deoptimization_without_replaying_writes() {
    let source = "calls=[0]\ndef churn():\n    for i in range(5000):\n        unused=[i]\ndef f(a,b):\n    saved=[42]\n    calls[0]+=1\n    churn()\n    value=a+b\n    return saved[0],value\nfor i in range(10):\n    assert f(1,2)==(42,3)\n";
    for tier in TIERS {
        let mut vm = execute(source, tier);
        let function = vm.get_global("f").unwrap();
        if tier != Tiering::Bytecode {
            vm.compile_lir(&vm.function_hir(function).unwrap()).unwrap();
        }
        let result = vm
            .call(function, &[Value::Float(0.5), Value::Float(1.5)])
            .unwrap();
        assert_eq!(
            vm.heap.get_item(result, Value::Int(0)).unwrap(),
            Value::Int(42)
        );
        assert_eq!(
            vm.heap.get_item(result, Value::Int(1)).unwrap(),
            Value::Float(2.0)
        );
        let calls = vm.get_global("calls").unwrap();
        assert_eq!(
            vm.heap.get_item(calls, Value::Int(0)).unwrap(),
            Value::Int(11)
        );
        if tier != Tiering::Bytecode {
            assert!(vm.tier_stats.deoptimizations > 0);
        }
        vm.collect_garbage(&[]).unwrap();
    }
}

#[test]
fn invalid_local_operations_are_rejected_before_execution() {
    let mut program = compile("def f(x):\n    return x\n").unwrap();
    let function = program.function_mut("f").unwrap();
    let entry = function.entry();
    function.append(entry, Op::Runtime(CoreOp::LoadLocal(usize::MAX), vec![]));
    assert!(function.bytecode().is_err());
    assert!(function.lower_lir(LoweringMode::Generic).is_err());
}

#[test]
fn detached_side_exit_restores_local_values_and_rejects_wrong_layouts() {
    use tis::lir::ExitReason;
    let mut vm = execute(
        "calls=[0]\ndef f(a,b):\n    calls[0]+=1\n    saved=[42]\n    value=a+b\n    return saved[0],value\nfor i in range(10):\n    f(1,2)\n",
        Tiering::Bytecode,
    );
    let mut function = vm.function_hir(vm.get_global("f").unwrap()).unwrap();
    let lir = function.lower_lir(LoweringMode::Profiled).unwrap();
    let exit = lir
        .exits
        .iter()
        .find(|exit| exit.reason == ExitReason::TypeGuard)
        .unwrap();
    let saved = vm
        .heap
        .allocate(tis::Object::List(vec![Value::Int(42)].into()));
    let locals = function
        .local_slots()
        .iter()
        .map(|name| match name.as_str() {
            "a" => Some(Value::Float(1.25)),
            "b" => Some(Value::Float(2.5)),
            "saved" => Some(saved),
            _ => None,
        })
        .collect::<Vec<_>>();
    // Obtain a valid environment without exposing frame implementation details.
    let mut namespace = tis::hir::Function::new(0);
    let entry = namespace.entry();
    let env = namespace.append(entry, Op::Runtime(CoreOp::Namespace, vec![]));
    namespace.terminate(entry, tis::hir::Terminator::Return(env));
    let env = vm.run(&mut namespace, &[]).unwrap();
    assert!(
        lir.reconstruct(exit.state, env, &[], |_| Ok(Value::None))
            .is_err()
    );
    let registers = lir.states[exit.state.0]
        .local_values
        .iter()
        .zip(&locals)
        .filter_map(|(r, v)| r.map(|r| (r, v.unwrap_or(Value::Unbound))))
        .collect::<std::collections::BTreeMap<_, _>>();
    let frame = lir
        .reconstruct(
            exit.state,
            env,
            &locals,
            |register| Ok(registers[&register]),
        )
        .unwrap();
    let mut invalid = frame.clone();
    invalid.locals.clear();
    assert!(vm.resume_hir(&mut function, invalid).is_err());
    let result = vm.resume_hir(&mut function, frame).unwrap();
    assert_eq!(
        vm.heap.get_item(result, Value::Int(0)).unwrap(),
        Value::Int(42)
    );
    assert_eq!(
        vm.heap.get_item(result, Value::Int(1)).unwrap(),
        Value::Float(3.75)
    );
    assert_eq!(
        vm.heap
            .get_item(vm.get_global("calls").unwrap(), Value::Int(0))
            .unwrap(),
        Value::Int(10)
    );
}
