use tis::{
    Tiering, Value, Vm, compile,
    hir::{CoreOp, Op},
    lir::{ExitReason, LoweringMode, Operation},
    profile::{HotThresholds, ProfileConfig},
};
const TIERS: [Tiering; 4] = [
    Tiering::Bytecode,
    Tiering::Lir,
    Tiering::Native,
    Tiering::CopyPatch,
];
fn vm(tier: Tiering) -> Vm {
    let mut vm = Vm::new();
    vm.tiering = tier;
    vm.thresholds = HotThresholds {
        function_entries: 4,
        loop_backedges: 4,
    };
    vm.profiling.window_ticks = 32;
    vm
}

#[test]
fn straight_line_locals_are_values_and_scalar_chains_reuse_unboxed_registers() {
    let mut program =
        compile("def f(a,b):\n    dx=a-b\n    product=dx*dx\n    return product+a\n").unwrap();
    let f = program.function_mut("f").unwrap();
    assert!(!f.dump().contains("StoreLocal"));
    assert_eq!(
        f.bytecode()
            .unwrap()
            .disassemble()
            .unwrap()
            .matches("LoadLocal")
            .count(),
        2
    );
    let mut vm = vm(Tiering::Bytecode);
    vm.profiling = ProfileConfig::eager();
    vm.run_program(&mut program).unwrap();
    let function = vm.get_global("f").unwrap();
    for _ in 0..16 {
        assert_eq!(
            vm.call(function, &[Value::Float(3.0), Value::Float(2.0)])
                .unwrap(),
            Value::Float(4.0)
        );
    }
    let f = vm.function_hir(function).unwrap();
    let lir = f.lower_lir(LoweringMode::Profiled).unwrap();
    let operations = lir
        .blocks
        .iter()
        .flat_map(|b| &b.instructions)
        .map(|i| &i.operation)
        .collect::<Vec<_>>();
    assert_eq!(
        operations
            .iter()
            .filter(|op| matches!(op, Operation::FloatBinary { .. }))
            .count(),
        3
    );
    assert_eq!(
        operations
            .iter()
            .filter(|op| matches!(op, Operation::GuardFloat { .. }))
            .count(),
        2
    );
    assert_eq!(
        operations
            .iter()
            .filter(|op| matches!(op, Operation::UnboxFloat(_)))
            .count(),
        2
    );
    // Boxed shadows remain available for frame reconstruction while arithmetic reuses the primitive values.
    assert_eq!(
        operations
            .iter()
            .filter(|op| matches!(op, Operation::BoxFloat(_)))
            .count(),
        3
    );
    for tier in TIERS {
        let mut p =
            compile("def f(a,b):\n    dx=a-b\n    product=dx*dx\n    return product+a\n").unwrap();
        let mut machine = Vm::new();
        machine.profiling = ProfileConfig::eager();
        machine.run_program(&mut p).unwrap();
        let callable = machine.get_global("f").unwrap();
        for _ in 0..16 {
            machine
                .call(callable, &[Value::Float(3.0), Value::Float(2.0)])
                .unwrap();
        }
        machine.tiering = tier;
        machine
            .compile_lir(&machine.function_hir(callable).unwrap())
            .unwrap();
        assert_eq!(
            machine
                .call(callable, &[Value::Float(3.0), Value::Float(2.0)])
                .unwrap(),
            Value::Float(4.0)
        );
        assert_eq!(
            machine
                .call(callable, &[Value::Int(3), Value::Int(2)])
                .unwrap(),
            Value::Int(4)
        );
        if tier != Tiering::Bytecode {
            assert!(machine.tier_stats.deoptimizations > 0);
        }
    }
}

#[test]
fn branch_joins_loop_phis_and_parallel_swaps_preserve_control_flow() {
    let source = "def f(n,flag):\n    x,y=1,2\n    for i in range(n):\n        if i==2:\n            continue\n        x,y=y,x+y\n        if i==5:\n            break\n    if flag:\n        z=x\n    else:\n        z=y\n    return x,y,z\nfor repeat in range(30):\n    assert f(0,True)==(1,2,1)\n    assert f(2,False)==(3,5,5)\n    assert f(100,True)==(13,21,13)\n";
    for tier in TIERS {
        let mut p = compile(source).unwrap();
        let mut vm = vm(tier);
        vm.run_program(&mut p).unwrap();
        assert_eq!(vm.stack_len(), 0);
    }
}

#[test]
fn empty_loops_and_partial_assignments_keep_unbound_local_errors() {
    for tier in TIERS {
        let mut p=compile("x=99\ndef f(n):\n    for i in range(n):\n        x=i\n    return x\nfor repeat in range(40):\n    assert f(3)==2\n").unwrap();
        assert!(p.function("f").unwrap().dump().contains("CheckLocal"));
        let mut vm = vm(tier);
        vm.run_program(&mut p).unwrap();
        let f = vm.get_global("f").unwrap();
        assert!(
            vm.call(f, &[Value::Int(0)])
                .unwrap_err()
                .to_string()
                .contains("UnboundLocalError: x")
        );
        assert_eq!(vm.call(f, &[Value::Int(1)]).unwrap(), Value::Int(0));
        vm.collect_garbage(&[]).unwrap();
    }
}

#[test]
fn detached_maps_reconstruct_bound_and_unbound_locals_from_ssa() {
    let mut p =
        compile("def f(flag,a,b):\n    if flag:\n        spare=[99]\n    return a+b\n").unwrap();
    let mut vm = vm(Tiering::Bytecode);
    vm.profiling = ProfileConfig::eager();
    vm.run_program(&mut p).unwrap();
    let callable = vm.get_global("f").unwrap();
    for _ in 0..16 {
        vm.call(
            callable,
            &[Value::Bool(false), Value::Int(1), Value::Int(2)],
        )
        .unwrap();
    }
    let f = vm.function_hir(callable).unwrap();
    let lir = f.lower_lir(LoweringMode::Profiled).unwrap();
    let state = lir
        .exits
        .iter()
        .find(|e| e.reason == ExitReason::TypeGuard)
        .unwrap()
        .state;
    let mut namespace = tis::hir::Function::new(0);
    let entry = namespace.entry();
    let env = namespace.append(entry, Op::Runtime(CoreOp::Namespace, vec![]));
    namespace.terminate(entry, tis::hir::Terminator::Return(env));
    let env = vm.run(&mut namespace, &[]).unwrap();
    for spare in [Value::Unbound, Value::Int(99)] {
        let values = f
            .local_slots()
            .iter()
            .map(|name| match name.as_str() {
                "a" => Value::Float(0.5),
                "b" => Value::Float(1.5),
                "flag" => Value::Bool(false),
                "spare" => spare,
                _ => unreachable!(),
            })
            .collect::<Vec<_>>();
        let registers = lir.states[state.0]
            .local_values
            .iter()
            .zip(&values)
            .map(|(r, v)| (r.unwrap(), *v))
            .collect::<std::collections::BTreeMap<_, _>>();
        let stale = vec![Some(Value::Int(-1)); values.len()];
        let frame = lir
            .reconstruct(state, env, &stale, |r| Ok(registers[&r]))
            .unwrap();
        let index = f
            .local_slots()
            .iter()
            .position(|name| name == "spare")
            .unwrap();
        assert_eq!(
            frame.locals[index],
            if spare == Value::Unbound {
                None
            } else {
                Some(spare)
            }
        );
        let mut invalid = lir.clone();
        let local = invalid.states[state.0]
            .local_values
            .iter()
            .flatten()
            .next()
            .copied()
            .unwrap();
        invalid.states[state.0].values.retain(|(_, r)| *r != local);
        assert!(invalid.verify().is_err());
    }
}

#[test]
fn scalar_reuse_across_allocating_helpers_keeps_roots_and_side_effects() {
    let source = "calls=[0]\ndef churn():\n    calls[0]+=1\n    for i in range(5000):\n        item=[i]\ndef f(a,b):\n    x=a-b\n    saved=[42]\n    churn()\n    return x*x,saved[0]\nfor i in range(40):\n    assert f(3.0,2.0)==(1.0,42)\nassert calls[0]==40\n";
    for tier in TIERS {
        let mut p = compile(source).unwrap();
        let mut vm = vm(tier);
        vm.run_program(&mut p).unwrap();
        vm.collect_garbage(&[]).unwrap();
        assert_eq!(vm.stack_len(), 0);
    }
}

#[test]
fn mutable_slot_operations_cannot_be_mixed_into_promoted_hir() {
    let mut p = compile("def f(x):\n    return x\n").unwrap();
    let f = p.function_mut("f").unwrap();
    let entry = f.entry();
    let value = f.append(entry, Op::Constant(tis::hir::Constant::Int(1)));
    f.append(entry, Op::Runtime(CoreOp::StoreLocal(0), vec![value]));
    assert!(f.bytecode().is_err());
}

#[test]
fn promoted_and_slot_hir_agree_on_nested_control_flow_and_errors() {
    let sources = [
        "def f(n,choose):\n    total=0\n    x,y=1,2\n    while n>0:\n        n-=1\n        if n%3==0:\n            x,y=y,x+y\n            continue\n        if choose and n==2:\n            break\n        if n%2==0:\n            z=total+x\n        else:\n            z=total-y\n        total=z\n    else:\n        total+=100\n    return total,x,y\nfor n in range(20):\n    for choose in [False,True]:\n        print(f(n,choose))\n",
        "def f(n):\n    out=[]\n    for i in range(n):\n        for j in range(i):\n            if j==2:\n                break\n            out.append((i,j))\n        else:\n            out.append(i)\n    return out\nfor n in range(10):\n    print(f(n))\n",
        "def f(n):\n    for i in range(n):\n        if i%2:\n            value=[i]\n    return value\nfor n in range(2,20):\n    print(f(n))\nf(0)\n",
        "def f(x):\n    saved=[x]\n    if x:\n        value=saved\n    else:\n        value=[99]\n    saved=[0]\n    return value[0]\nfor i in range(40):\n    print(f(i))\n",
    ];
    for tier in TIERS {
        for source in sources {
            let results = [false, true].map(|promote_locals| {
                let mut program =
                    tis::compile_with_options(source, tis::CompileOptions { promote_locals })
                        .unwrap();
                let mut vm = vm(tier);
                let error = vm
                    .run_program(&mut program)
                    .err()
                    .map(|error| error.to_string());
                (vm.output, error)
            });
            assert_eq!(results[0], results[1], "{tier:?}: {source}");
        }
    }
}
