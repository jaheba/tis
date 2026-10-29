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
fn loaded_global_survives_unbinding_and_callee_collection() {
    let mut vm = native(
        "value=[0]\nchurn=False\ndef clear():\n    global value\n    value=None\n    if churn:\n        for i in range(12000):\n            trash=[i]\ndef take():\n    keep=value\n    clear()\n    return keep\nfor i in range(300):\n    value=[i]\n    take()\nvalue=[456]\nchurn=True\n",
    );
    let child = vm.get_global("value").unwrap();
    let take = vm.get_global("take").unwrap();
    let entries = vm.tier_stats.native_entries;
    let result = vm.call(take, &[]).unwrap();
    assert_eq!(result, child);
    assert_eq!(
        vm.heap.get_item(result, Value::Int(0)).unwrap(),
        Value::Int(456)
    );
    assert_eq!(vm.get_global("value").unwrap(), Value::None);
    assert!(vm.tier_stats.native_entries > entries);
    vm.collect_garbage(&[result]).unwrap();
    assert!(vm.heap.get(child).is_ok());
    vm.collect_garbage(&[]).unwrap();
    assert!(vm.heap.get(child).is_err());
    assert_eq!(vm.stack_len(), 0);
}

#[test]
fn cached_global_rebinding_and_eager_observation_remain_authoritative() {
    let mut vm = native(
        "value=1\ndef get():\n    return value\nfor i in range(300):\n    get()\nvalue=[9]\n",
    );
    let get = vm.get_global("get").unwrap();
    let value = vm.get_global("value").unwrap();
    assert!(matches!(vm.heap.get(value).unwrap(), Object::List(_)));
    assert_eq!(vm.call(get, &[]).unwrap(), value);
    vm.profiling.mode = ProfileMode::Eager;
    let probes = vm.tier_stats.native_feedback_probes;
    assert_eq!(vm.call(get, &[]).unwrap(), value);
    assert!(vm.tier_stats.native_feedback_probes > probes);
    assert_eq!(vm.stack_len(), 0);
}

#[test]
fn missing_name_preserves_prior_effect_and_error_cleanup() {
    let mut vm = native(
        "flag=False\nevents=[]\ndef f():\n    events.append(1)\n    if flag:\n        return missing_name\n    return 7\nfor i in range(300):\n    f()\nflag=True\n",
    );
    let f = vm.get_global("f").unwrap();
    let events = vm.get_global("events").unwrap();
    let before = vm.heap.len(events).unwrap();
    let error = vm.call(f, &[]).unwrap_err();
    assert!(error.to_string().contains("NameError: missing_name"));
    assert_eq!(vm.heap.len(events).unwrap(), before + 1);
    assert_eq!(vm.stack_len(), 0);
}

#[test]
fn closure_class_and_osr_names_match_all_tiers() {
    let source = "g=3\ndef outer(x):\n    def inner():\n        nonlocal x\n        x+=1\n        return x+g\n    class C:\n        first=x\n        second=first+1\n        def get(self):\n            return x+g\n    total=0\n    for i in range(9000):\n        total+=g\n    return inner,C(),total\nfor j in range(20):\n    f,c,total=outer(j)\n    assert total==27000\n    assert c.first==j\n    assert c.second==j+1\n    assert f()==j+4\n    assert c.get()==j+4\n";
    for tier in [Tiering::Lir, Tiering::Native, Tiering::CopyPatch] {
        let mut vm = Vm::new();
        vm.tiering = tier;
        vm.thresholds.function_entries = 1;
        vm.thresholds.loop_backedges = 4;
        vm.run_program(&mut compile(source).unwrap()).unwrap();
        assert_eq!(vm.stack_len(), 0);
    }
}
