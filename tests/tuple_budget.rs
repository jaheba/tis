use tis::{Object, Tiering, Value, Vm, compile, profile::ProfileMode};

fn native(source: &str) -> Vm {
    let mut vm = Vm::new();
    vm.tiering = Tiering::Native;
    vm.thresholds.function_entries = 1;
    vm.thresholds.loop_backedges = 4;
    vm.run_program(&mut compile(source).unwrap()).unwrap();
    vm
}

#[test]
fn tuple_fast_allocation_preserves_aliases_bits_roots_and_eager_fallback() {
    let mut vm = native(
        "def pair(x):\n    return x,x,None,-0.0\nfor i in range(300):\n    pair(i)\n    pair([])\n",
    );
    let pair = vm.get_global("pair").unwrap();
    let child = vm.heap.allocate(Object::List(vec![Value::Int(123)].into()));
    let roots = vm.tier_stats.native_root_syncs;
    let entries = vm.tier_stats.native_entries;
    let result = vm.call(pair, &[child]).unwrap();
    assert_eq!(vm.heap.get_item(result, Value::Int(0)).unwrap(), child);
    assert_eq!(vm.heap.get_item(result, Value::Int(1)).unwrap(), child);
    let Value::Float(zero) = vm.heap.get_item(result, Value::Int(3)).unwrap() else {
        panic!("not float")
    };
    assert_eq!(zero.to_bits(), (-0.0_f64).to_bits());
    assert_eq!(vm.tier_stats.native_root_syncs, roots);
    assert!(vm.tier_stats.native_entries > entries);
    vm.collect_garbage(&[result]).unwrap();
    assert_eq!(
        vm.heap.get_item(child, Value::Int(0)).unwrap(),
        Value::Int(123)
    );
    vm.collect_garbage(&[]).unwrap();
    assert!(vm.heap.get(child).is_err());

    vm.profiling.mode = ProfileMode::Eager;
    let probes = vm.tier_stats.native_feedback_probes;
    let result = vm.call(pair, &[Value::Int(7)]).unwrap();
    assert_eq!(
        vm.heap.get_item(result, Value::Int(0)).unwrap(),
        Value::Int(7)
    );
    assert!(vm.tier_stats.native_root_syncs > roots);
    assert!(vm.tier_stats.native_feedback_probes > probes);
    assert_eq!(vm.stack_len(), 0);
}

#[test]
fn recursive_tuple_results_survive_callee_gc_and_later_errors() {
    let mut vm = native(
        "churn=False\nevents=[]\ndef recurse(x,n):\n    head=x,n\n    if n:\n        return head,recurse(x,n-1)\n    if churn:\n        for i in range(12000):\n            trash=[i]\n    return head,None\ndef fail(x):\n    orphan=(x,)\n    events.append('before')\n    return orphan,1//x\nfor i in range(300):\n    recurse([i],3)\n    fail(1)\nchurn=True\n",
    );
    let recurse = vm.get_global("recurse").unwrap();
    let child = vm.heap.allocate(Object::List(vec![Value::Int(99)].into()));
    let result = vm.call(recurse, &[child, Value::Int(20)]).unwrap();
    let mut cursor = result;
    for n in (0..=20).rev() {
        let head = vm.heap.get_item(cursor, Value::Int(0)).unwrap();
        assert_eq!(vm.heap.get_item(head, Value::Int(0)).unwrap(), child);
        assert_eq!(
            vm.heap.get_item(head, Value::Int(1)).unwrap(),
            Value::Int(n)
        );
        cursor = vm.heap.get_item(cursor, Value::Int(1)).unwrap();
    }
    assert_eq!(cursor, Value::None);
    vm.collect_garbage(&[result]).unwrap();
    assert!(vm.heap.get(child).is_ok());
    let events = vm.get_global("events").unwrap();
    let before = vm.heap.len(events).unwrap();
    let fail = vm.get_global("fail").unwrap();
    assert!(vm.call(fail, &[Value::Int(0)]).is_err());
    assert_eq!(vm.heap.len(events).unwrap(), before + 1);
    vm.collect_garbage(&[]).unwrap();
    assert!(vm.heap.get(child).is_err());
    assert_eq!(vm.stack_len(), 0);
}

#[test]
fn tuple_chains_keep_osr_state_and_generic_backends() {
    for tier in [Tiering::Lir, Tiering::Native, Tiering::CopyPatch] {
        let mut vm = Vm::new();
        vm.tiering = tier;
        vm.thresholds.function_entries = u64::MAX;
        vm.thresholds.loop_backedges = 4;
        vm.profiling.window_ticks = 4;
        vm.run_program(&mut compile("def build(n):\n    value=None\n    for i in range(n):\n        value=value,i\n    return value\n").unwrap()).unwrap();
        let build = vm.get_global("build").unwrap();
        let result = vm.call(build, &[Value::Int(9000)]).unwrap();
        let mut cursor = result;
        for i in (0..9000).rev() {
            assert_eq!(
                vm.heap.get_item(cursor, Value::Int(1)).unwrap(),
                Value::Int(i)
            );
            cursor = vm.heap.get_item(cursor, Value::Int(0)).unwrap();
        }
        assert_eq!(cursor, Value::None);
        assert!(
            match tier {
                Tiering::Native => vm.tier_stats.native_osr_entries,
                Tiering::CopyPatch => vm.tier_stats.copy_patch_osr_entries,
                _ => vm.tier_stats.osr_entries,
            } > 0
        );
        vm.collect_garbage(&[result]).unwrap();
        assert!(vm.heap.get(result).is_ok());
        assert_eq!(vm.stack_len(), 0);
    }
}
