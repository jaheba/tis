use tis::{Object, Tiering, Value, Vm, compile, profile::ProfileConfig};

const TIERS: [Tiering; 2] = [Tiering::Native, Tiering::CopyPatch];
fn setup(source: &str, tier: Tiering) -> (Vm, Value) {
    let mut program = compile(source).unwrap();
    let mut vm = Vm::new();
    vm.tiering = tier;
    vm.thresholds.function_entries = 1;
    vm.profiling.window_ticks = 16;
    vm.run_program(&mut program).unwrap();
    let f = vm.get_global("f").unwrap();
    (vm, f)
}

#[test]
fn scalar_truth_has_no_callback_and_heap_truth_keeps_its_fallback() {
    for tier in TIERS {
        let (mut vm, f) = setup(
            "def f(x):\n    if x:\n        return 1\n    return 2\n",
            tier,
        );
        for _ in 0..24 {
            vm.call(f, &[Value::Bool(true)]).unwrap();
        }
        let before = vm.tier_stats.native_truth_fallbacks;
        for (value, truth) in [
            (Value::None, false),
            (Value::Bool(false), false),
            (Value::Bool(true), true),
            (Value::Int(0), false),
            (Value::Int(i64::MIN), true),
            (Value::Float(0.0), false),
            (Value::Float(-0.0), false),
            (Value::Float(f64::NAN), true),
            (Value::Float(-f64::NAN), true),
            (Value::Float(f64::INFINITY), true),
            (Value::Float(f64::NEG_INFINITY), true),
            (Value::Float(f64::from_bits(1)), true),
        ] {
            assert_eq!(
                vm.call(f, &[value]).unwrap(),
                Value::Int(if truth { 1 } else { 2 })
            );
        }
        assert_eq!(vm.tier_stats.native_truth_fallbacks, before);
        for (object, truth) in [
            (Object::List(vec![].into()), false),
            (Object::String("x".into()), true),
            (Object::Dict(Default::default()), false),
        ] {
            let value = vm.heap.allocate(object);
            assert_eq!(
                vm.call(f, &[value]).unwrap(),
                Value::Int(if truth { 1 } else { 2 })
            );
        }
        assert_eq!(vm.tier_stats.native_truth_fallbacks, before + 3);
    }
}

fn source(vm: &mut Vm) -> Value {
    let values = (0..3)
        .map(|i| vm.heap.allocate(Object::List(vec![Value::Int(i)].into())))
        .collect();
    vm.heap.allocate(Object::List(values))
}

#[test]
fn batched_references_survive_source_mutation_and_nested_collections() {
    let program = "
def churn(n):
    for i in range(n):
        garbage=[i]
def f(items,n):
    a,b,c=items
    items[0]=None
    items[1]=None
    items[2]=None
    churn(n)
    return (a,b,c)
";
    for tier in TIERS {
        let (mut vm, f) = setup(program, tier);
        for _ in 0..24 {
            let items = source(&mut vm);
            vm.call(f, &[items, Value::Int(1)]).unwrap();
        }
        let batches = vm.tier_stats.native_unpack_batches;
        let items = source(&mut vm);
        let root = vm.pin(items).unwrap();
        let result = vm.call(f, &[items, Value::Int(10000)]).unwrap();
        assert!(vm.tier_stats.native_unpack_batches > batches);
        let Object::Tuple(values) = vm.heap.get(result).unwrap() else {
            panic!()
        };
        for (i, value) in values.iter().enumerate() {
            assert!(
                matches!(vm.heap.get(*value).unwrap(),Object::List(items) if items==&[Value::Int(i as i64)])
            );
        }
        assert!(
            matches!(vm.heap.get(items).unwrap(),Object::List(values) if values==&[Value::None;3])
        );
        vm.unpin(root);
    }
}

#[test]
fn eager_mode_rebuilds_batched_artifacts_and_observes_each_element() {
    for tier in TIERS {
        let (mut vm, f) = setup("def f(items):\n    a,b,c=items\n    return (a,b,c)\n", tier);
        for _ in 0..24 {
            let items = source(&mut vm);
            vm.call(f, &[items]).unwrap();
        }
        assert!(vm.tier_stats.native_unpack_batches > 0);
        let before = vm.function_hir(f).unwrap().profile();
        let batches = vm.tier_stats.native_unpack_batches;
        vm.profiling = ProfileConfig::eager();
        let items = source(&mut vm);
        vm.call(f, &[items]).unwrap();
        assert_eq!(vm.tier_stats.native_unpack_batches, batches);
        let hir = vm.function_hir(f).unwrap();
        let lir = hir.lower_lir(tis::lir::LoweringMode::Profiled).unwrap();
        let mut elements = 0;
        for instruction in lir.blocks.iter().flat_map(|b| &b.instructions) {
            if matches!(
                instruction.operation,
                tis::lir::Operation::SequenceGet { .. }
            ) {
                let site = instruction.origin.0;
                assert_eq!(
                    hir.profile().sites[site].executions,
                    before.sites[site].executions + 1
                );
                assert_eq!(
                    hir.profile().sites[site].result.observations(),
                    before.sites[site].result.observations() + 1
                );
                elements += 1;
            }
        }
        assert_eq!(elements, 3);
    }
}

#[test]
fn batch_guard_failure_does_not_repeat_prior_effects_or_partial_assignment() {
    for tier in TIERS {
        let (mut vm, f) = setup(
            "calls=0\ndef f(items):\n    global calls\n    calls+=1\n    a,b,c=items\n    return a+b+c\n",
            tier,
        );
        let items = vm.heap.allocate(Object::List(
            vec![Value::Int(1), Value::Int(2), Value::Int(3)].into(),
        ));
        for _ in 0..24 {
            vm.call(f, &[items]).unwrap();
        }
        let before = vm.get_global("calls").unwrap();
        {
            let Object::List(values) = &mut *vm.heap.get_mut(items).unwrap() else {
                panic!()
            };
            values.pop();
        }
        assert!(vm.call(f, &[items]).is_err());
        let Value::Int(before) = before else { panic!() };
        assert_eq!(vm.get_global("calls").unwrap(), Value::Int(before + 1));
        assert!(vm.tier_stats.deoptimizations > 0);
    }
}

#[test]
fn reopened_staged_window_observes_each_read_in_an_active_batch() {
    let program = "
def f(depth,x):
    y=x+1
    if depth:
        f(0,2.5)
    a,b,c=[y,y,y]
    return a+b+c
";
    for tier in TIERS {
        let (mut vm, f) = setup(program, tier);
        for _ in 0..24 {
            vm.call(f, &[Value::Int(0), Value::Int(3)]).unwrap();
        }
        let before = vm.function_hir(f).unwrap().profile();
        let batches = vm.tier_stats.native_unpack_batches;
        assert_eq!(
            vm.call(f, &[Value::Int(1), Value::Int(3)]).unwrap(),
            Value::Int(12)
        );
        assert!(vm.tier_stats.deoptimizations > 0);
        assert!(vm.tier_stats.native_unpack_batches > batches);
        let hir = vm.function_hir(f).unwrap();
        let lir = hir.lower_lir(tis::lir::LoweringMode::Profiled).unwrap();
        let mut elements = 0;
        for instruction in lir.blocks.iter().flat_map(|b| &b.instructions) {
            if matches!(
                instruction.operation,
                tis::lir::Operation::SequenceGet { .. }
            ) {
                let site = instruction.origin.0;
                assert_eq!(
                    hir.profile().sites[site].executions,
                    before.sites[site].executions + 2
                );
                assert_eq!(
                    hir.profile().sites[site].result.observations(),
                    before.sites[site].result.observations() + 2
                );
                elements += 1;
            }
        }
        assert_eq!(elements, 3);
    }
}

#[test]
fn reopened_window_observes_direct_float_unpack_operands_and_results() {
    let program = "def f(depth,x):\n    y=x+1.0\n    if depth:\n        f(0,3)\n    a,b,c=[y,y,y]\n    return a+b+c\n";
    for tier in TIERS {
        let (mut vm, f) = setup(program, tier);
        for _ in 0..24 {
            vm.call(f, &[Value::Int(0), Value::Float(3.0)]).unwrap();
        }
        let before = vm.function_hir(f).unwrap().profile();
        let lir = vm
            .function_hir(f)
            .unwrap()
            .lower_lir(tis::lir::LoweringMode::Profiled)
            .unwrap();
        let sites = lir
            .blocks
            .iter()
            .flat_map(|b| &b.instructions)
            .filter(|i| {
                matches!(
                    i.operation,
                    tis::lir::Operation::FloatListUnpack { .. }
                        | tis::lir::Operation::FloatListLoad { .. }
                )
            })
            .map(|i| i.origin.0)
            .collect::<Vec<_>>();
        assert_eq!(sites.len(), 4);
        let misses = vm.tier_stats.deoptimizations;
        assert_eq!(
            vm.call(f, &[Value::Int(1), Value::Float(3.0)]).unwrap(),
            Value::Float(12.0)
        );
        assert!(vm.tier_stats.deoptimizations > misses);
        let after = vm.function_hir(f).unwrap().profile();
        for site in sites {
            assert_eq!(
                after.sites[site].executions,
                before.sites[site].executions + 2
            );
            assert_eq!(
                after.sites[site].result.observations(),
                before.sites[site].result.observations() + 2
            );
        }
    }
}
