use tis::{
    Object, Tiering, Value, Vm, compile,
    lir::{Helper, LoweringMode, Operation, ScalarBuiltin},
    profile::ProfileMode,
};

fn native(source: &str) -> Vm {
    let mut vm = Vm::new();
    vm.tiering = Tiering::Native;
    vm.thresholds.function_entries = 1;
    vm.thresholds.loop_backedges = 4;
    vm.run_program(&mut compile(source).unwrap()).unwrap();
    vm
}

#[test]
fn scalar_probes_keep_full_lir_effects_but_skip_quiet_root_synchronization() {
    let mut vm = native(
        "import math\nsqrtfn=math.sqrt\ndef size(x):\n    return len(x)\ndef root(x):\n    return sqrtfn(x)\nfor i in range(300):\n    size([1,2])\n    root(4.0)\n    root(i)\n    root(True)\n",
    );
    let size = vm.get_global("size").unwrap();
    let root = vm.get_global("root").unwrap();
    for (f, kind) in [(size, ScalarBuiltin::Len), (root, ScalarBuiltin::Sqrt)] {
        let lir = vm
            .function_hir(f)
            .unwrap()
            .lower_lir(LoweringMode::Profiled)
            .unwrap();
        assert!(lir.blocks.iter().flat_map(|b| &b.instructions).any(|i| matches!(&i.operation,Operation::Call{helper:Helper::ScalarCall(k),effects,roots,..} if *k==kind && effects.may_call && effects.may_allocate && !roots.is_empty())));
    }
    let list = vm
        .heap
        .allocate(Object::List(vec![Value::Int(1), Value::Int(2)].into()));
    let entries = vm.tier_stats.native_entries;
    let roots = vm.tier_stats.native_root_syncs;
    let callbacks = vm.tier_stats.native_runtime_calls;
    assert_eq!(vm.call(size, &[list]).unwrap(), Value::Int(2));
    for (input, expected) in [
        (Value::Int(9), Value::Float(3.0)),
        (Value::Bool(true), Value::Float(1.0)),
        (Value::Float(-0.0), Value::Float(-0.0)),
        (Value::Float(f64::INFINITY), Value::Float(f64::INFINITY)),
    ] {
        let Value::Float(actual) = vm.call(root, &[input]).unwrap() else {
            panic!("not float")
        };
        let Value::Float(expected) = expected else {
            unreachable!()
        };
        assert_eq!(actual.to_bits(), expected.to_bits());
    }
    assert!(
        matches!(vm.call(root,&[Value::Float(f64::NAN)]).unwrap(),Value::Float(v) if v.is_nan())
    );
    assert_eq!(vm.tier_stats.native_root_syncs, roots);
    assert!(vm.tier_stats.native_entries > entries);
    assert!(vm.tier_stats.native_runtime_calls > callbacks);

    vm.profiling.mode = ProfileMode::Eager;
    let probes = vm.tier_stats.native_feedback_probes;
    assert_eq!(vm.call(root, &[Value::Int(16)]).unwrap(), Value::Float(4.0));
    assert!(vm.tier_stats.native_root_syncs > roots);
    assert!(vm.tier_stats.native_feedback_probes > probes);
    assert_eq!(vm.stack_len(), 0);
}

#[test]
fn callable_miss_roots_unpublished_field_values_before_reentrant_gc() {
    let mut vm = native(
        "class Box:\n    def __init__(self,x):\n        self.value=x\nholder=Box([123])\ndef invoke(fn,holder,x):\n    keep=holder.value\n    result=fn(x)\n    return keep,result\nfor i in range(300):\n    invoke(len,holder,[])\ndef evil(x):\n    holder.value=None\n    for i in range(7000):\n        trash=[i]\n    return [x]\nholder.value=[456]\n",
    );
    let invoke = vm.get_global("invoke").unwrap();
    let evil = vm.get_global("evil").unwrap();
    let holder = vm.get_global("holder").unwrap();
    let fields = vm.tier_stats.native_inline_field_hits;
    let roots = vm.tier_stats.native_root_syncs;
    let result = vm.call(invoke, &[evil, holder, Value::Int(7)]).unwrap();
    let keep = vm.heap.get_item(result, Value::Int(0)).unwrap();
    assert_eq!(
        vm.heap.get_item(keep, Value::Int(0)).unwrap(),
        Value::Int(456)
    );
    let returned = vm.heap.get_item(result, Value::Int(1)).unwrap();
    assert_eq!(
        vm.heap.get_item(returned, Value::Int(0)).unwrap(),
        Value::Int(7)
    );
    assert!(vm.tier_stats.native_inline_field_hits > fields);
    assert!(vm.tier_stats.native_root_syncs > roots);
    vm.collect_garbage(&[]).unwrap();
    assert!(vm.heap.get(keep).is_err());
    assert_eq!(vm.stack_len(), 0);
}

#[test]
fn scalar_builtin_errors_and_descriptor_mutation_use_original_semantics() {
    let mut vm = native(
        "import math\nsqrtfn=math.sqrt\nevents=[]\ndef root(x):\n    events.append('before')\n    value=sqrtfn(x)\n    events.append('after')\n    return value\nfor i in range(300):\n    root(4.0)\n",
    );
    let root = vm.get_global("root").unwrap();
    assert!(
        vm.call(root, &[Value::Float(-1.0)])
            .unwrap_err()
            .to_string()
            .contains("math domain error")
    );
    vm.run_program(
        &mut compile("assert events[-1]=='before'\nassert events[-2]=='after'\n").unwrap(),
    )
    .unwrap();
    assert_eq!(vm.call(root, &[Value::Int(25)]).unwrap(), Value::Float(5.0));
    assert_eq!(vm.stack_len(), 0);

    let mut vm =
        native("target=len\ndef f(x):\n    return target(x)\nfor i in range(300):\n    f([])\n");
    let target = vm.get_global("target").unwrap();
    let f = vm.get_global("f").unwrap();
    // The identity stays the same while its Native descriptor changes. Guard its current name.
    *vm.heap.get_mut(target).unwrap() = Object::Native("math.sqrt".into());
    let roots = vm.tier_stats.native_root_syncs;
    assert_eq!(vm.call(f, &[Value::Int(9)]).unwrap(), Value::Float(3.0));
    assert!(vm.tier_stats.native_root_syncs > roots);
    assert_eq!(vm.stack_len(), 0);
}

#[test]
fn keyword_star_calls_and_invalid_scalar_metadata_keep_general_binding() {
    let mut vm = native(
        "import math\nsqrtfn=math.sqrt\ndef star(xs):\n    return sqrtfn(*xs)\ndef keyword(x):\n    return sqrtfn(x=x)\ndef plain(x):\n    return sqrtfn(x)\nfor i in range(300):\n    star((4.0,))\n    plain(4.0)\n",
    );
    let star = vm.get_global("star").unwrap();
    let keyword = vm.get_global("keyword").unwrap();
    let plain = vm.get_global("plain").unwrap();
    for f in [star, keyword] {
        let lir = vm
            .function_hir(f)
            .unwrap()
            .lower_lir(LoweringMode::Profiled)
            .unwrap();
        assert!(
            !lir.blocks
                .iter()
                .flat_map(|b| &b.instructions)
                .any(|i| matches!(
                    i.operation,
                    Operation::Call {
                        helper: Helper::ScalarCall(_),
                        ..
                    }
                ))
        );
    }
    assert!(vm.call(keyword, &[Value::Int(4)]).is_err());
    let mut lir = vm
        .function_hir(plain)
        .unwrap()
        .lower_lir(LoweringMode::Profiled)
        .unwrap();
    for i in lir.blocks.iter_mut().flat_map(|b| &mut b.instructions) {
        if let Operation::Call {
            helper: Helper::ScalarCall(_),
            arguments,
            ..
        } = &mut i.operation
        {
            arguments.pop();
        }
    }
    assert!(lir.verify().is_err());
    assert_eq!(vm.stack_len(), 0);
}

#[test]
fn shared_scalar_evaluator_preserves_all_tier_values_and_errors() {
    let source = "import math\nsqrtfn=math.sqrt\ndef size(x):\n    return len(x)\ndef root(x):\n    return sqrtfn(x)\nfor i in range(300):\n    size([])\n    root(4.0)\n    root(i)\n    root(True)\nd={1:2}\nassert size(d.keys())==1\nassert size(range(5))==5\n";
    let mut control = Vm::new();
    control.run_program(&mut compile(source).unwrap()).unwrap();
    for tier in [Tiering::Lir, Tiering::Native, Tiering::CopyPatch] {
        let mut vm = Vm::new();
        vm.tiering = tier;
        vm.thresholds.function_entries = 1;
        vm.thresholds.loop_backedges = 4;
        vm.run_program(&mut compile(source).unwrap()).unwrap();
        let size = vm.get_global("size").unwrap();
        let base_size = control.get_global("size").unwrap();
        for object in [
            Object::String("é☃".into()),
            Object::List(vec![Value::Int(1)].into()),
            Object::Tuple(vec![Value::Int(1), Value::Int(2)]),
            Object::Dict(Default::default()),
            Object::Set(vec![Value::Int(1)]),
        ] {
            let baseline = control.heap.allocate(object.clone());
            let input = vm.heap.allocate(object);
            assert_eq!(
                vm.call(size, &[input]).unwrap(),
                control.call(base_size, &[baseline]).unwrap()
            );
        }
        let root = vm.get_global("root").unwrap();
        let base_root = control.get_global("root").unwrap();
        for input in [
            Value::None,
            Value::Float(-1.0),
            Value::Float(f64::NEG_INFINITY),
            Value::Int(i64::MIN),
        ] {
            assert_eq!(
                vm.call(root, &[input]).unwrap_err().to_string(),
                control.call(base_root, &[input]).unwrap_err().to_string()
            );
        }
        assert_eq!(
            vm.call(size, &[Value::None]).unwrap_err().to_string(),
            control
                .call(base_size, &[Value::None])
                .unwrap_err()
                .to_string()
        );
        assert_eq!(vm.stack_len(), 0);
    }
}
