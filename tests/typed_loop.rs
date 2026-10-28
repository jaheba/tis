use tis::{
    List, Object, Tiering, Value, Vm, compile,
    lir::{Effects, LoweringMode, Operation, Type},
    profile::ProfileConfig,
};
const TIERS: [Tiering; 3] = [Tiering::Lir, Tiering::Native, Tiering::CopyPatch];
fn setup(source: &str, tier: Tiering) -> (Vm, Value) {
    let mut p = compile(source).unwrap();
    let mut vm = Vm::new();
    vm.tiering = tier;
    vm.thresholds.function_entries = 1;
    vm.thresholds.loop_backedges = 1;
    vm.profiling.window_ticks = 16;
    vm.run_program(&mut p).unwrap();
    let f = vm.get_global("f").unwrap();
    (vm, f)
}
#[test]
fn float_storage_preserves_values_and_widens_for_incompatible_mutation() {
    let mut list: List = vec![Value::Float(-0.0), Value::Float(f64::NAN)].into();
    assert!(list.is_float());
    let Value::Float(zero) = list.value(0).unwrap() else {
        panic!()
    };
    assert_eq!(zero.to_bits(), (-0.0f64).to_bits());
    list.set(0, Value::Float(3.5));
    assert!(list.is_float());
    assert_eq!(list[0], Value::Float(3.5));
    list.push(Value::Int(7));
    assert!(!list.is_float());
    assert_eq!(list[2], Value::Int(7));
    assert!(matches!(list[1],Value::Float(v) if v.is_nan()));
}
#[test]
fn typed_float_access_is_guarded_and_reconstructs_after_shape_changes() {
    for tier in TIERS {
        let (mut vm, f) = setup(
            "def f(xs,x):\n    a=xs[0]\n    xs[1]=a+x\n    return xs[1]\n",
            tier,
        );
        let xs = vm.heap.allocate(Object::List(
            vec![Value::Float(2.0), Value::Float(3.0)].into(),
        ));
        for _ in 0..24 {
            assert_eq!(
                vm.call(f, &[xs, Value::Float(0.5)]).unwrap(),
                Value::Float(2.5)
            );
        }
        let lir = vm
            .function_hir(f)
            .unwrap()
            .lower_lir(LoweringMode::Profiled)
            .unwrap();
        assert!(
            lir.blocks
                .iter()
                .flat_map(|b| &b.instructions)
                .any(|i| matches!(i.operation, Operation::FloatListLoad { .. }))
        );
        assert!(
            lir.blocks
                .iter()
                .flat_map(|b| &b.instructions)
                .any(|i| matches!(i.operation, Operation::FloatListStore { .. }))
        );
        vm.heap.set_item(xs, Value::Int(0), Value::Int(7)).unwrap();
        assert_eq!(
            vm.call(f, &[xs, Value::Float(0.5)]).unwrap(),
            Value::Float(7.5)
        );
        assert!(vm.tier_stats.deoptimizations > 0);
        let Object::List(items) = vm.heap.get(xs).unwrap() else {
            panic!()
        };
        assert!(!items.is_float());
    }
}
#[test]
fn scalar_loop_parameters_preserve_parallel_swaps_and_osr() {
    for tier in TIERS {
        let (mut vm, f) = setup(
            "def f(n):\n    a,b=0.0,1.0\n    for i in range(n):\n        a,b=b,a+b\n    return a\n",
            tier,
        );
        for _ in 0..24 {
            vm.call(f, &[Value::Int(2)]).unwrap();
        }
        let lir = vm
            .function_hir(f)
            .unwrap()
            .lower_lir(LoweringMode::Profiled)
            .unwrap();
        assert!(
            lir.blocks
                .iter()
                .flat_map(|b| &b.parameters)
                .any(|r| lir.registers[r.0] == Type::F64)
        );
        assert_eq!(vm.call(f, &[Value::Int(20)]).unwrap(), Value::Float(6765.0));
        assert!(vm.tier_stats.osr_entries > 0);
    }
}
#[test]
fn phi_type_miss_after_a_call_does_not_repeat_its_effects() {
    let code = "calls=0\ndef flip(i):\n    global calls\n    calls+=1\n    if i==4:\n        return 7\n    return 2.0\ndef f(n):\n    a=0.0\n    for i in range(n):\n        a=flip(i)\n    return a\n";
    for tier in TIERS {
        let (mut vm, f) = setup(code, tier);
        for _ in 0..24 {
            vm.call(f, &[Value::Int(1)]).unwrap();
        }
        assert_eq!(vm.call(f, &[Value::Int(5)]).unwrap(), Value::Int(7));
        assert_eq!(vm.get_global("calls").unwrap(), Value::Int(29));
        assert!(vm.tier_stats.deoptimizations > 0);
    }
}
#[test]
fn float_view_cannot_survive_a_callback() {
    let (mut vm, f) = setup(
        "def f(xs):\n    xs[0]=xs[0]+1.0\n    return xs[0]\n",
        Tiering::Bytecode,
    );
    let xs = vm
        .heap
        .allocate(Object::List(vec![Value::Float(1.0)].into()));
    for _ in 0..24 {
        vm.call(f, &[xs]).unwrap();
    }
    let mut lir = vm
        .function_hir(f)
        .unwrap()
        .lower_lir(LoweringMode::Profiled)
        .unwrap();
    let (block, index) = lir
        .blocks
        .iter()
        .enumerate()
        .find_map(|(b, block)| {
            block
                .instructions
                .iter()
                .position(|i| matches!(i.operation, Operation::FloatListView { .. }))
                .map(|i| (b, i))
        })
        .unwrap();
    let original = lir.blocks[block].instructions[index].clone();
    let Operation::FloatListView { value, failure } = original.operation else {
        panic!()
    };
    let state = lir.exits[failure.0].state;
    let result = tis::lir::Register(lir.registers.len());
    lir.registers.push(Type::Bool);
    lir.blocks[block].instructions.insert(
        index + 1,
        tis::lir::Instruction {
            result: Some(result),
            operation: Operation::Truth {
                value,
                state,
                roots: vec![],
                effects: Effects::READ_ONLY,
            },
            origin: original.origin,
            point: original.point,
        },
    );
    assert!(lir.verify().is_err());
}
#[test]
fn aliasing_and_eager_feedback_remain_valid_with_float_regions() {
    for tier in TIERS {
        let (mut vm, f) = setup(
            "def f(a,b):\n    a[0]=a[0]+1.0\n    b[1]=a[0]+b[1]\n    return b[1]\n",
            tier,
        );
        let xs = vm.heap.allocate(Object::List(
            vec![Value::Float(1.0), Value::Float(2.0)].into(),
        ));
        for _ in 0..24 {
            vm.call(f, &[xs, xs]).unwrap();
        }
        vm.heap
            .set_item(xs, Value::Int(0), Value::Float(1.0))
            .unwrap();
        vm.heap
            .set_item(xs, Value::Int(1), Value::Float(2.0))
            .unwrap();
        vm.profiling = ProfileConfig::eager();
        assert_eq!(vm.call(f, &[xs, xs]).unwrap(), Value::Float(4.0));
        assert_eq!(
            vm.heap.get_item(xs, Value::Int(0)).unwrap(),
            Value::Float(2.0)
        );
    }
}

#[test]
fn helper_mutation_ends_views_and_does_not_replay_on_widening() {
    let code = "def append(xs,x):\n    xs.append(x)\ndef f(xs,x):\n    saved=xs[0]\n    append(xs,x)\n    return xs[0]+saved\n";
    for tier in TIERS {
        let (mut vm, f) = setup(code, tier);
        for _ in 0..24 {
            let xs = vm
                .heap
                .allocate(Object::List(vec![Value::Float(2.0)].into()));
            assert_eq!(
                vm.call(f, &[xs, Value::Float(1.0)]).unwrap(),
                Value::Float(4.0)
            );
        }
        let xs = vm
            .heap
            .allocate(Object::List(vec![Value::Float(2.0)].into()));
        assert_eq!(vm.call(f, &[xs, Value::Int(7)]).unwrap(), Value::Float(4.0));
        assert_eq!(vm.heap.len(xs).unwrap(), 2);
        assert_eq!(vm.heap.get_item(xs, Value::Int(1)).unwrap(), Value::Int(7));
        assert!(vm.tier_stats.deoptimizations > 0);
    }
}

#[test]
fn boxed_read_snapshots_refresh_after_native_float_writes() {
    for tier in TIERS {
        let (mut vm, f) = setup(
            "def f(xs):\n    before=repr(xs)\n    xs[0]=xs[0]+1.0\n    return (before,repr(xs))\n",
            tier,
        );
        for _ in 0..24 {
            let xs = vm
                .heap
                .allocate(Object::List(vec![Value::Float(1.0)].into()));
            vm.call(f, &[xs]).unwrap();
        }
        let xs = vm
            .heap
            .allocate(Object::List(vec![Value::Float(1.0)].into()));
        let result = vm.call(f, &[xs]).unwrap();
        let Object::Tuple(values) = vm.heap.get(result).unwrap() else {
            panic!()
        };
        assert_eq!(
            vm.heap.get(values[0]).unwrap(),
            &Object::String("[1.0]".into())
        );
        assert_eq!(
            vm.heap.get(values[1]).unwrap(),
            &Object::String("[2.0]".into())
        );
    }
}

#[test]
fn deferred_float_boxes_reconstruct_after_a_store_and_guard_failure() {
    for tier in TIERS {
        let (mut vm, f) = setup(
            "def f(xs,x):\n    a=x+0.5\n    xs[1]=a\n    b=xs[0]\n    return a+b\n",
            tier,
        );
        let xs = vm.heap.allocate(Object::List(
            vec![Value::Float(2.0), Value::Float(0.0)].into(),
        ));
        for _ in 0..24 {
            assert_eq!(
                vm.call(f, &[xs, Value::Float(3.0)]).unwrap(),
                Value::Float(5.5)
            );
        }
        let lir = vm
            .function_hir(f)
            .unwrap()
            .lower_lir(LoweringMode::Profiled)
            .unwrap();
        assert!(!lir.boxing_recipes().is_empty());
        vm.heap.set_item(xs, Value::Int(0), Value::Int(7)).unwrap();
        assert_eq!(
            vm.call(f, &[xs, Value::Float(8.0)]).unwrap(),
            Value::Float(15.5)
        );
        assert_eq!(
            vm.heap.get_item(xs, Value::Int(1)).unwrap(),
            Value::Float(8.5)
        );
        assert!(vm.tier_stats.deoptimizations > 0);
    }
}

#[test]
fn deferred_float_boxes_materialize_for_allocating_helpers_and_return() {
    for tier in TIERS {
        let (mut vm, f) = setup(
            "def f(x):\n    a=x+0.5\n    xs=[a,a+1.0]\n    return xs[0]+xs[1]+a\n",
            tier,
        );
        for _ in 0..24 {
            assert_eq!(vm.call(f, &[Value::Float(2.0)]).unwrap(), Value::Float(8.5));
        }
        assert_eq!(
            vm.call(f, &[Value::Float(-3.0)]).unwrap(),
            Value::Float(-6.5)
        );
    }
}

#[test]
fn deferred_floats_and_heap_roots_survive_collection_inside_native_loops() {
    let (mut vm, f) = setup(
        "def f(n):\n    token=[42]\n    a=0.0\n    for i in range(n):\n        a=a+0.5\n        junk=[a]\n    return a+token[0]\n",
        Tiering::Native,
    );
    for _ in 0..24 {
        vm.call(f, &[Value::Int(4)]).unwrap();
    }
    let before = vm.tier_stats.native_entries;
    assert_eq!(
        vm.call(f, &[Value::Int(10000)]).unwrap(),
        Value::Float(5042.0)
    );
    assert!(vm.tier_stats.native_entries > before);
    assert!(vm.heap.allocation_count() < 4096);
}
