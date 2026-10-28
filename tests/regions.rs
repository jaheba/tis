use tis::{Object, Tiering, Value, Vm, compile, lir::LoweringMode};
fn setup(source: &str) -> (Vm, Value) {
    let mut p = compile(source).unwrap();
    let mut vm = Vm::new();
    vm.tiering = Tiering::Native;
    vm.thresholds.function_entries = 1;
    vm.thresholds.loop_backedges = 4;
    vm.profiling.window_ticks = 16;
    vm.run_program(&mut p).unwrap();
    let f = vm.get_global("f").unwrap();
    (vm, f)
}
fn floats(vm: &Vm, values: &[f64]) -> Value {
    vm.heap.allocate(Object::List(
        values
            .iter()
            .copied()
            .map(Value::Float)
            .collect::<Vec<_>>()
            .into(),
    ))
}
fn rows(vm: &Vm, values: Vec<Value>) -> Value {
    vm.heap.allocate(Object::List(values.into()))
}
const UPDATE: &str = "def f(rows,k):\n    for xs,ys,m in rows:\n        a,b=xs\n        ys[0]+=a*k*m\n        ys[1]+=b*k*m\n";
#[test]
fn entire_sequence_loop_runs_without_per_iteration_runtime_calls() {
    let (mut vm, f) = setup(UPDATE);
    let x = floats(&vm, &[1.0, 2.0]);
    let y = floats(&vm, &[0.0, 0.0]);
    let row = vm
        .heap
        .allocate(Object::Tuple(vec![x, y, Value::Float(2.0)]));
    let input = rows(&vm, vec![row; 100]);
    for _ in 0..24 {
        vm.call(f, &[input, Value::Float(0.5)]).unwrap();
    }
    vm.heap
        .set_item(y, Value::Int(0), Value::Float(0.0))
        .unwrap();
    vm.heap
        .set_item(y, Value::Int(1), Value::Float(0.0))
        .unwrap();
    let before = vm.tier_stats.clone();
    vm.call(f, &[input, Value::Float(0.5)]).unwrap();
    assert_eq!(
        vm.heap.get_item(y, Value::Int(0)).unwrap(),
        Value::Float(100.0)
    );
    assert_eq!(
        vm.heap.get_item(y, Value::Int(1)).unwrap(),
        Value::Float(200.0)
    );
    assert_eq!(
        vm.tier_stats.native_region_entries - before.native_region_entries,
        1
    );
    assert_eq!(
        vm.tier_stats.native_region_iterations - before.native_region_iterations,
        100
    );
    assert!(vm.tier_stats.native_runtime_calls - before.native_runtime_calls < 20);
    assert_eq!(
        vm.tier_stats.native_unpack_batches - before.native_unpack_batches,
        0
    );
    assert_eq!(vm.tier_stats.deoptimizations - before.deoptimizations, 0);
}
#[test]
fn a_type_exit_after_prior_writes_does_not_repeat_completed_iterations() {
    let (mut vm, f) = setup(UPDATE);
    let x = floats(&vm, &[1.0, 2.0]);
    let y = floats(&vm, &[0.0, 0.0]);
    let good = vm
        .heap
        .allocate(Object::Tuple(vec![x, y, Value::Float(2.0)]));
    let warm = rows(&vm, vec![good; 10]);
    for _ in 0..24 {
        vm.call(f, &[warm, Value::Float(0.5)]).unwrap();
    }
    let bad = vm.heap.allocate(Object::Tuple(vec![x, y, Value::Int(2)]));
    let input = rows(&vm, vec![good, bad, good]);
    vm.heap
        .set_item(y, Value::Int(0), Value::Float(0.0))
        .unwrap();
    vm.heap
        .set_item(y, Value::Int(1), Value::Float(0.0))
        .unwrap();
    let before = vm.tier_stats.deoptimizations;
    vm.call(f, &[input, Value::Float(0.5)]).unwrap();
    assert!(vm.tier_stats.deoptimizations > before);
    assert_eq!(
        vm.heap.get_item(y, Value::Int(0)).unwrap(),
        Value::Float(3.0)
    );
    assert_eq!(
        vm.heap.get_item(y, Value::Int(1)).unwrap(),
        Value::Float(6.0)
    );
}
#[test]
fn length_failure_preserves_iterator_progress_and_prior_effects() {
    let (mut vm, f) = setup(UPDATE);
    let x = floats(&vm, &[1.0, 2.0]);
    let y = floats(&vm, &[0.0, 0.0]);
    let good = vm
        .heap
        .allocate(Object::Tuple(vec![x, y, Value::Float(2.0)]));
    let warm = rows(&vm, vec![good; 10]);
    for _ in 0..24 {
        vm.call(f, &[warm, Value::Float(0.5)]).unwrap();
    }
    let wrong = vm.heap.allocate(Object::Tuple(vec![x, y]));
    let input = rows(&vm, vec![good, wrong, good]);
    let it = vm.heap.allocate(Object::Iterator {
        source: input,
        index: 0,
        size: 3,
        exhausted: false,
    });
    vm.heap
        .set_item(y, Value::Int(0), Value::Float(0.0))
        .unwrap();
    assert!(
        vm.call(f, &[it, Value::Float(0.5)])
            .unwrap_err()
            .to_string()
            .contains("ValueError")
    );
    assert_eq!(
        vm.heap.get_item(y, Value::Int(0)).unwrap(),
        Value::Float(1.0)
    );

    assert!(matches!(
        vm.heap.get(it).unwrap(),
        Object::Iterator {
            index: 2,
            exhausted: false,
            ..
        }
    ));
    assert_eq!(vm.stack_len(), 0);
}
#[test]
fn arithmetic_failure_after_a_store_keeps_exact_partial_effects() {
    let source = "def f(rows):\n    for x,y in rows:\n        x[0]+=1.0\n        z=x[0]/y[0]\n        x[1]+=z\n";
    let (mut vm, f) = setup(source);
    let x = floats(&vm, &[1.0, 0.0]);
    let denominator = floats(&vm, &[1.0]);
    let good = vm.heap.allocate(Object::Tuple(vec![x, denominator]));
    let warm = rows(&vm, vec![good; 10]);
    for _ in 0..24 {
        vm.call(f, &[warm]).unwrap();
    }
    let other = floats(&vm, &[1.0, 0.0]);
    let zero = floats(&vm, &[0.0]);
    let bad = vm.heap.allocate(Object::Tuple(vec![other, zero]));
    let input = rows(&vm, vec![good, bad]);
    vm.heap
        .set_item(x, Value::Int(0), Value::Float(1.0))
        .unwrap();
    vm.heap
        .set_item(x, Value::Int(1), Value::Float(0.0))
        .unwrap();
    assert!(
        vm.call(f, &[input])
            .unwrap_err()
            .to_string()
            .contains("ZeroDivisionError")
    );
    assert_eq!(
        vm.heap.get_item(x, Value::Int(0)).unwrap(),
        Value::Float(2.0)
    );
    assert_eq!(
        vm.heap.get_item(x, Value::Int(1)).unwrap(),
        Value::Float(2.0)
    );
    assert_eq!(
        vm.heap.get_item(other, Value::Int(0)).unwrap(),
        Value::Float(2.0)
    );
    assert_eq!(
        vm.heap.get_item(other, Value::Int(1)).unwrap(),
        Value::Float(0.0)
    );
    assert_eq!(vm.stack_len(), 0);
}
#[test]
fn calls_between_regions_invalidate_cached_shapes() {
    let source = "calls=[0]\ndef change(rows,flag):\n    calls[0]+=1\n    if flag:\n        rows[0].append(7.0)\ndef f(rows,flag):\n    for i in range(2):\n        for xs in rows:\n            a,b=xs\n            xs[0]=a+1.0\n        change(rows,flag)\n";
    let (mut vm, f) = setup(source);
    let x = floats(&vm, &[1.0, 2.0]);
    let input = rows(&vm, vec![x; 10]);
    for _ in 0..24 {
        vm.call(f, &[input, Value::Bool(false)]).unwrap();
    }
    vm.heap
        .set_item(x, Value::Int(0), Value::Float(1.0))
        .unwrap();
    assert!(vm.tier_stats.native_region_entries > 0);
    let calls = vm.get_global("calls").unwrap();
    let Value::Int(before) = vm.heap.get_item(calls, Value::Int(0)).unwrap() else {
        panic!()
    };
    assert!(
        vm.call(f, &[input, Value::Bool(true)])
            .unwrap_err()
            .to_string()
            .contains("ValueError")
    );
    assert_eq!(
        vm.heap.get_item(x, Value::Int(0)).unwrap(),
        Value::Float(11.0)
    );
    assert_eq!(
        vm.heap.get_item(calls, Value::Int(0)).unwrap(),
        Value::Int(before + 1)
    );
}
#[test]
fn collection_between_regions_discards_old_heap_views() {
    let source = "def churn(n):\n    for i in range(n):\n        junk=[i]\ndef f(rows,n):\n    for i in range(3):\n        for xs in rows:\n            a,b=xs\n            xs[0]=a+1.0\n        churn(n)\n";
    let (mut vm, f) = setup(source);
    let x = floats(&vm, &[1.0, 2.0]);
    let input = rows(&vm, vec![x; 10]);
    for _ in 0..24 {
        vm.call(f, &[input, Value::Int(1)]).unwrap();
    }
    vm.heap
        .set_item(x, Value::Int(0), Value::Float(1.0))
        .unwrap();
    let before = vm.tier_stats.native_region_entries;
    vm.call(f, &[input, Value::Int(5000)]).unwrap();
    assert_eq!(
        vm.heap.get_item(x, Value::Int(0)).unwrap(),
        Value::Float(31.0)
    );
    assert!(vm.tier_stats.native_region_entries >= before + 3);
    assert!(vm.heap.allocation_count() < 4096);
}
#[test]
fn float_iteration_reads_updates_and_exhaustion_remains_sticky() {
    let (mut vm, f) =
        setup("def f(xs):\n    for x in xs:\n        xs[1]=x+1.0\n    return xs[1]\n");
    let warm = floats(&vm, &[1.0, 2.0, 3.0]);
    for _ in 0..24 {
        vm.call(f, &[warm]).unwrap();
    }
    let source = floats(&vm, &[2.0, 2.0, 2.0]);
    assert_eq!(vm.call(f, &[source]).unwrap(), Value::Float(3.0));
    let (mut other, g) = setup(
        "def f(xs):\n    total=0.0\n    for x in xs:\n        total=total+x\n    return total\n",
    );
    let warm = floats(&other, &[1.0, 2.0, 3.0]);
    for _ in 0..24 {
        other.call(g, &[warm]).unwrap();
    }
    let source = floats(&other, &[1.0, 2.0, 3.0]);
    let iter = other.heap.allocate(Object::Iterator {
        source,
        index: 0,
        size: 3,
        exhausted: false,
    });
    assert_eq!(other.call(g, &[iter]).unwrap(), Value::Float(6.0));
    if let Object::List(v) = &mut *other.heap.get_mut(source).unwrap() {
        v.push(Value::Float(9.0));
    }
    assert_eq!(other.call(g, &[iter]).unwrap(), Value::Float(0.0));
}
#[test]
fn signed_range_iteration_uses_wide_arithmetic() {
    let (mut vm, f) = setup(
        "def f(n):\n    total=0\n    for i in range(-3,n,2):\n        total=total+i\n    return total\n",
    );
    for _ in 0..24 {
        assert_eq!(vm.call(f, &[Value::Int(4)]).unwrap(), Value::Int(0));
    }
    assert_eq!(vm.call(f, &[Value::Int(8)]).unwrap(), Value::Int(12));
    assert!(vm.tier_stats.native_region_iterations > 0);
}
#[test]
fn verifier_rejects_missing_imports_and_new_effectful_region_instructions() {
    let (mut vm, f) = setup(UPDATE);
    let x = floats(&vm, &[1.0, 2.0]);
    let y = floats(&vm, &[0.0, 0.0]);
    let row = vm
        .heap
        .allocate(Object::Tuple(vec![x, y, Value::Float(2.0)]));
    let input = rows(&vm, vec![row; 10]);
    for _ in 0..24 {
        vm.call(f, &[input, Value::Float(0.5)]).unwrap();
    }
    let lir = vm
        .function_hir(f)
        .unwrap()
        .lower_lir(LoweringMode::Profiled)
        .unwrap();
    assert_eq!(lir.regions.len(), 1);
    let mut bad = lir.clone();
    bad.regions[0].schema.as_mut().unwrap().constraints.clear();
    assert!(bad.verify().is_err());
    let mut bad = lir.clone();
    let header = bad.regions[0].header.0;
    let tis::lir::Terminator::Branch { yes, no, .. } = &mut bad.blocks[header].terminator else {
        panic!()
    };
    std::mem::swap(yes, no);
    assert!(bad.verify().is_err());
    let mut bad = lir.clone();
    bad.regions[0].imports.clear();
    assert!(bad.verify().is_err());
    let mut bad = lir;
    let body = bad.regions[0].body.0;
    bad.blocks[body].instructions[0].operation = tis::lir::Operation::Call {
        helper: tis::lir::Helper::Len,
        arguments: vec![],
        state: tis::lir::StateId(0),
        roots: vec![],
        effects: tis::lir::Effects::READ_ONLY,
    };
    assert!(bad.verify().is_err());
}

#[test]
fn changing_object_phis_keep_guards_inside_the_region() {
    let (mut vm, f) = setup(
        "def f(rows,start):\n    previous=start\n    result=0.0\n    for current in rows:\n        result=result+previous[0]\n        previous=current\n    return result\n",
    );
    let a = floats(&vm, &[1.0]);
    let b = floats(&vm, &[2.0]);
    let c = floats(&vm, &[4.0]);
    let warm = rows(&vm, vec![b, c, a, b, c, a, b, c, a, b]);
    for _ in 0..24 {
        vm.call(f, &[warm, a]).unwrap();
    }
    let lir = vm
        .function_hir(f)
        .unwrap()
        .lower_lir(LoweringMode::Profiled)
        .unwrap();
    assert_eq!(lir.regions.len(), 1);
    assert!(lir.regions[0].schema.is_none());
    let integer = vm.heap.allocate(Object::List(vec![Value::Int(9)].into()));
    let input = rows(&vm, vec![b, integer, c]);
    let before = vm.tier_stats.deoptimizations;
    assert_eq!(vm.call(f, &[input, a]).unwrap(), Value::Float(12.0));
    assert!(vm.tier_stats.deoptimizations > before);
}

#[test]
fn iterator_allocation_gc_preserves_views_and_bulk_backedge_counts() {
    let (mut vm, f) = setup(
        "def f(rows,n):\n    for i in range(n):\n        for xs in rows:\n            a,b=xs\n            xs[0]=a+1.0\n",
    );
    vm.disable_native_nests = true;
    let x = floats(&vm, &[1.0, 2.0]);
    let input = rows(&vm, vec![x; 10]);
    for _ in 0..24 {
        vm.call(f, &[input, Value::Int(1)]).unwrap();
    }
    vm.heap
        .set_item(x, Value::Int(0), Value::Float(1.0))
        .unwrap();
    let before = vm.tier_stats.clone();
    let edges: u64 = vm
        .function_hir(f)
        .unwrap()
        .profile()
        .backedges
        .values()
        .sum();
    vm.call(f, &[input, Value::Int(10000)]).unwrap();
    assert_eq!(
        vm.heap.get_item(x, Value::Int(0)).unwrap(),
        Value::Float(100001.0)
    );
    assert_eq!(
        vm.tier_stats.native_region_entries - before.native_region_entries,
        10000
    );
    assert_eq!(
        vm.tier_stats.native_region_iterations - before.native_region_iterations,
        100000
    );
    assert_eq!(
        vm.function_hir(f)
            .unwrap()
            .profile()
            .backedges
            .values()
            .sum::<u64>()
            - edges,
        110000
    );
    assert!(vm.heap.allocation_count() < 4096);
    assert!(vm.heap.slot_count() > vm.heap.allocation_count());
}
#[test]
fn eager_feedback_uses_the_observing_path_instead_of_fast_regions() {
    let (mut vm, f) = setup(UPDATE);
    let x = floats(&vm, &[1.0, 2.0]);
    let y = floats(&vm, &[0.0, 0.0]);
    let row = vm
        .heap
        .allocate(Object::Tuple(vec![x, y, Value::Float(2.0)]));
    let input = rows(&vm, vec![row; 10]);
    for _ in 0..24 {
        vm.call(f, &[input, Value::Float(0.5)]).unwrap();
    }
    let before = vm.tier_stats.native_region_entries;
    vm.profiling = tis::profile::ProfileConfig::eager();
    vm.call(f, &[input, Value::Float(0.5)]).unwrap();
    assert_eq!(vm.tier_stats.native_region_entries, before);
    assert!(vm.tier_stats.native_feedback_probes > 0);
}
#[test]
fn aliased_float_graphs_match_bytecode_bit_for_bit() {
    for seed in 1..=12u64 {
        let run = |native: bool| {
            let (mut vm, f) = setup(UPDATE);
            if !native {
                vm.tiering = Tiering::Bytecode;
            }
            let arrays = (0..5)
                .map(|i| floats(&vm, &[i as f64 + 0.25, 1.5 - i as f64]))
                .collect::<Vec<_>>();
            let mut state = seed;
            let mut random = || {
                state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
                (state >> 32) as usize
            };
            let mut items = vec![];
            for _ in 0..20 {
                let a = random() % 5;
                let b = random() % 5;
                let mass = (random() % 5) as f64 - 2.0;
                items.push(vm.heap.allocate(Object::Tuple(vec![
                    arrays[a],
                    arrays[b],
                    Value::Float(mass),
                ])));
            }
            let input = rows(&vm, items);
            if native {
                for _ in 0..24 {
                    vm.call(f, &[input, Value::Float(0.25)]).unwrap();
                }
            }
            for (i, array) in arrays.iter().enumerate() {
                vm.heap
                    .set_item(*array, Value::Int(0), Value::Float(i as f64 + 0.25))
                    .unwrap();
                vm.heap
                    .set_item(*array, Value::Int(1), Value::Float(1.5 - i as f64))
                    .unwrap();
            }
            let before = vm.tier_stats.native_region_entries;
            vm.call(f, &[input, Value::Float(0.25)]).unwrap();
            if native {
                assert!(vm.tier_stats.native_region_entries > before);
            }
            arrays
                .iter()
                .flat_map(|array| {
                    (0..2).map(|i| match vm.heap.get_item(*array, Value::Int(i)).unwrap() {
                        Value::Float(v) => v.to_bits(),
                        _ => panic!(),
                    })
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(run(true), run(false), "seed {seed}");
    }
}

#[test]
fn lazy_unbound_loop_locals_stay_unbound_on_an_empty_iteration() {
    let (mut vm, f) = setup(
        "def f(rows):\n    for xs in rows:\n        a,b=xs\n        xs[0]=a+1.0\n    return a\n",
    );
    let xs = floats(&vm, &[1.0, 2.0]);
    let warm = rows(&vm, vec![xs; 10]);
    for _ in 0..24 {
        vm.call(f, &[warm]).unwrap();
    }
    let empty = rows(&vm, vec![]);
    assert!(
        vm.call(f, &[empty])
            .unwrap_err()
            .to_string()
            .contains("UnboundLocalError")
    );
    assert_eq!(vm.stack_len(), 0);
    assert!(matches!(vm.call(f, &[warm]).unwrap(), Value::Float(_)));
}
#[test]
fn tagged_reference_phis_keep_parallel_swaps_and_none_tags() {
    let (mut vm, f) = setup(
        "def f(rows,token):\n    a=None\n    b=token\n    for xs in rows:\n        x,y=xs\n        xs[0]=x+1.0\n        a,b=b,a\n    return a,b\n",
    );
    let xs = floats(&vm, &[1.0, 2.0]);
    let token = vm.heap.allocate(Object::List(vec![Value::Int(123)].into()));
    let warm = rows(&vm, vec![xs; 10]);
    for _ in 0..24 {
        vm.call(f, &[warm, token]).unwrap();
    }
    for len in [0, 1, 2, 3, 4] {
        let input = rows(&vm, vec![xs; len]);
        let result = vm.call(f, &[input, token]).unwrap();
        assert_eq!(
            vm.heap.get_item(result, Value::Int(0)).unwrap(),
            if len % 2 == 0 { Value::None } else { token }
        );
        assert_eq!(
            vm.heap.get_item(result, Value::Int(1)).unwrap(),
            if len % 2 == 0 { token } else { Value::None }
        );
    }
}
#[test]
fn lazy_tagged_roots_survive_allocating_callbacks_and_return_fresh_results() {
    let source = "def churn(n):\n    for i in range(n):\n        junk=[i]\n    return [99]\ndef f(rows,n):\n    owned=[42]\n    for xs in rows:\n        a,b=xs\n        xs[0]=a+1.0\n    fresh=churn(n)\n    return owned,fresh,xs\n";
    let (mut vm, f) = setup(source);
    let xs = floats(&vm, &[1.0, 2.0]);
    let input = rows(&vm, vec![xs; 10]);
    for _ in 0..24 {
        vm.call(f, &[input, Value::Int(1)]).unwrap();
    }
    let before = vm.tier_stats.native_region_entries;
    let result = vm.call(f, &[input, Value::Int(5000)]).unwrap();
    let owned = vm.heap.get_item(result, Value::Int(0)).unwrap();
    let fresh = vm.heap.get_item(result, Value::Int(1)).unwrap();
    assert_eq!(
        vm.heap.get_item(owned, Value::Int(0)).unwrap(),
        Value::Int(42)
    );
    assert_eq!(
        vm.heap.get_item(fresh, Value::Int(0)).unwrap(),
        Value::Int(99)
    );
    assert_eq!(vm.heap.get_item(result, Value::Int(2)).unwrap(), xs);
    assert!(vm.tier_stats.native_region_entries > before);
}
#[test]
fn diagnostics_capture_the_live_compiled_image() {
    let (mut vm, f) =
        setup("def f(rows):\n    for xs in rows:\n        a,b=xs\n        xs[0]=a+1.0\n");
    vm.capture_native_code = true;
    let xs = floats(&vm, &[1.0, 2.0]);
    let input = rows(&vm, vec![xs; 10]);
    for _ in 0..24 {
        vm.call(f, &[input]).unwrap();
    }
    let d = vm.native_diagnostics(f).unwrap();
    assert!(!d.machine_code.is_empty());
    assert!(d.clif.contains("block"));
    assert!(d.disassembly.contains("block"));
}

const NEST: &str = "def f(steps,rows,k):\n    for step in steps:\n        for xs in rows:\n            a,b=xs\n            xs[0]=a+1.0\n            xs[1]=b/k\n";
#[test]
fn closed_nest_keeps_iterator_progress_and_feedback_in_native_state() {
    let (mut vm, f) = setup(NEST);
    let xs = floats(&vm, &[0.0, 8.0]);
    let input = rows(&vm, vec![xs; 3]);
    let steps = rows(&vm, vec![Value::Int(0); 7]);
    for _ in 0..24 {
        vm.call(f, &[steps, input, Value::Float(1.0)]).unwrap();
    }
    vm.heap
        .set_item(xs, Value::Int(0), Value::Float(0.0))
        .unwrap();
    let before = vm.tier_stats.clone();
    vm.call(f, &[steps, input, Value::Float(1.0)]).unwrap();
    assert_eq!(
        vm.heap.get_item(xs, Value::Int(0)).unwrap(),
        Value::Float(21.0)
    );
    assert_eq!(
        vm.tier_stats.native_region_entries - before.native_region_entries,
        1
    );
    assert_eq!(
        vm.tier_stats.native_region_iterations - before.native_region_iterations,
        28
    );
    assert!(vm.tier_stats.native_runtime_calls - before.native_runtime_calls < 20);
    assert_eq!(vm.tier_stats.deoptimizations - before.deoptimizations, 0);
    let lir = vm
        .function_hir(f)
        .unwrap()
        .lower_lir(LoweringMode::Profiled)
        .unwrap();
    assert!(lir.nest.is_some());
    let mut bad = lir.clone();
    bad.nest.as_mut().unwrap().imports.clear();
    assert!(bad.verify().is_err());
    let mut bad = lir;
    bad.nest.as_mut().unwrap().private_iterators[0].source = tis::lir::Register(usize::MAX);
    assert!(bad.verify().is_err());
}
#[test]
fn closed_nest_failure_preserves_partial_writes_and_consumed_outer_item() {
    let source = "def f(steps,rows):\n    for step in steps:\n        for xs in rows:\n            a,b=xs\n            xs[0]=a+1.0\n            xs[1]=b-1.0\n            xs[0]=a+1.0/b\n";
    let (mut vm, f) = setup(source);
    let xs = floats(&vm, &[0.0, 10000.0]);
    let input = rows(&vm, vec![xs; 3]);
    let steps = rows(&vm, vec![Value::Int(0); 7]);
    for _ in 0..24 {
        vm.call(f, &[steps, input]).unwrap();
    }
    vm.heap
        .set_item(xs, Value::Int(0), Value::Float(0.0))
        .unwrap();
    vm.heap
        .set_item(xs, Value::Int(1), Value::Float(4.0))
        .unwrap();
    let steps = vm.heap.allocate(Object::Iterator {
        source: steps,
        index: 0,
        size: 7,
        exhausted: false,
    });
    let before = vm.tier_stats.clone();
    let error = vm.call(f, &[steps, input]).unwrap_err();
    assert!(error.to_string().contains("ZeroDivisionError"));
    assert_eq!(
        vm.heap.get_item(xs, Value::Int(0)).unwrap(),
        Value::Float(0.25 + 1.0 / 3.0 + 0.5 + 1.0 + 1.0)
    );
    assert_eq!(
        vm.heap.get_item(xs, Value::Int(1)).unwrap(),
        Value::Float(-1.0)
    );
    assert_eq!(
        vm.tier_stats.native_region_entries - before.native_region_entries,
        1
    );
    assert!(vm.tier_stats.deoptimizations > before.deoptimizations);
    assert!(matches!(
        vm.heap.get(steps).unwrap(),
        Object::Iterator {
            index: 2,
            exhausted: false,
            ..
        }
    ));
    assert_eq!(vm.stack_len(), 0);
}

#[test]
fn empty_outer_nest_does_not_evaluate_invalid_inner_iterable() {
    let (mut vm, f) = setup(NEST);
    let xs = floats(&vm, &[0.0, 8.0]);
    let input = rows(&vm, vec![xs; 3]);
    let warm = rows(&vm, vec![Value::Int(0); 7]);
    for _ in 0..24 {
        vm.call(f, &[warm, input, Value::Float(1.0)]).unwrap();
    }
    let empty = rows(&vm, vec![]);
    assert_eq!(
        vm.call(f, &[empty, Value::Int(42), Value::Float(1.0)])
            .unwrap(),
        Value::None
    );
}

#[test]
fn inner_iterator_alias_preserves_shared_exhaustion() {
    let (mut vm, f) = setup(NEST);
    let xs = floats(&vm, &[0.0, 8.0]);
    let input = rows(&vm, vec![xs; 3]);
    let steps = rows(&vm, vec![Value::Int(0); 7]);
    for _ in 0..24 {
        vm.call(f, &[steps, input, Value::Float(1.0)]).unwrap();
    }
    vm.heap
        .set_item(xs, Value::Int(0), Value::Float(0.0))
        .unwrap();
    let iterator = vm.heap.allocate(Object::Iterator {
        source: input,
        index: 0,
        size: 3,
        exhausted: false,
    });
    vm.call(f, &[steps, iterator, Value::Float(1.0)]).unwrap();
    assert_eq!(
        vm.heap.get_item(xs, Value::Int(0)).unwrap(),
        Value::Float(3.0)
    );
    assert!(matches!(
        vm.heap.get(iterator).unwrap(),
        Object::Iterator {
            index: 3,
            exhausted: true,
            ..
        }
    ));
}

#[test]
fn cached_nest_sources_support_ranges_tuples_and_float_aliases() {
    let source = "def f(steps,indices,xs):\n    for step in steps:\n        for i in indices:\n            a,b=xs\n            xs[0]=a+1.0\n";
    let (mut vm, f) = setup(source);
    let xs = floats(&vm, &[0.0, 2.0]);
    let steps = vm.heap.allocate(Object::Tuple(vec![Value::Int(0); 7]));
    let indices = vm.heap.allocate(Object::Range {
        start: 5,
        stop: -2,
        step: -2,
    });
    for _ in 0..24 {
        vm.call(f, &[steps, indices, xs]).unwrap();
    }
    assert!(
        vm.function_hir(f)
            .unwrap()
            .lower_lir(LoweringMode::Profiled)
            .unwrap()
            .nest
            .is_some()
    );
    for input in [
        indices,
        vm.heap.allocate(Object::Tuple(vec![Value::Int(1); 4])),
        floats(&vm, &[1.0, 2.0, 3.0, 4.0]),
    ] {
        vm.heap
            .set_item(xs, Value::Int(0), Value::Float(0.0))
            .unwrap();
        let before = vm.tier_stats.clone();
        vm.call(f, &[steps, input, xs]).unwrap();
        assert_eq!(
            vm.heap.get_item(xs, Value::Int(0)).unwrap(),
            Value::Float(28.0)
        );
        assert_eq!(
            vm.tier_stats.native_region_entries - before.native_region_entries,
            1
        );
    }
    // An unconstrained item can itself be the VM's sentinel; do not replace that check with
    // iterator exhaustion unless entry validation proves an ordinary Python item type.
    vm.heap
        .set_item(xs, Value::Int(0), Value::Float(0.0))
        .unwrap();
    let sentinel = vm.heap.allocate(Object::Done);
    let input = rows(&vm, vec![Value::Int(1), sentinel, Value::Int(2)]);
    vm.call(f, &[steps, input, xs]).unwrap();
    assert_eq!(
        vm.heap.get_item(xs, Value::Int(0)).unwrap(),
        Value::Float(7.0)
    );
}
