use tis::{
    CompileOptions, Object, Tiering, Value, Vm, compile, compile_with_options, profile::ProfileMode,
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
fn seeded_inputs_preserve_all_value_bits_and_do_not_retain_heap_values() {
    let source =
        "def f(x,y):\n    assert y==1\n    return x\nfor i in range(300):\n    assert f(i,1)==i\n";
    let mut vm = native(source);
    let f = vm.get_global("f").unwrap();
    let mut control = Vm::new();
    control.run_program(&mut compile(source).unwrap()).unwrap();
    let baseline = control.get_global("f").unwrap();
    let entries = vm.tier_stats.native_entries;
    assert_eq!(
        vm.call(f, &[Value::Int(42), Value::Int(1)]).unwrap(),
        Value::Int(42)
    );
    assert!(vm.tier_stats.native_entries > entries);
    for value in [
        Value::None,
        Value::Bool(false),
        Value::Bool(true),
        Value::Int(i64::MIN),
        Value::Int(i64::MAX),
        Value::Float(-0.0),
        Value::Float(f64::INFINITY),
        Value::Float(f64::from_bits(0x7ff8_0000_0000_1234)),
        Value::Unbound,
    ] {
        let expected = control.call(baseline, &[value, Value::Int(1)]);
        let actual = vm.call(f, &[value, Value::Int(1)]);
        match (expected, actual) {
            (Ok(Value::Float(a)), Ok(Value::Float(b))) => assert_eq!(a.to_bits(), b.to_bits()),
            (Ok(a), Ok(b)) => assert_eq!(a, b),
            (Err(a), Err(b)) => assert_eq!(a.to_string(), b.to_string()),
            (a, b) => panic!("different outcomes: {a:?}, {b:?}"),
        }
    }
    let object = vm.heap.allocate(Object::List(vec![Value::Int(42)].into()));
    assert_eq!(vm.call(f, &[object, Value::Int(1)]).unwrap(), object);
    vm.collect_garbage(&[]).unwrap();
    assert!(vm.heap.get(object).is_err());
    assert_eq!(vm.stack_len(), 0);
}

#[test]
fn explicit_slot_writes_keep_loads_dynamic_and_errors_keep_prior_effects() {
    let mut vm = Vm::new();
    vm.tiering = Tiering::Native;
    vm.thresholds.function_entries = 1;
    vm.thresholds.loop_backedges = 4;
    vm.run_program(&mut compile_with_options("events=[]\ndef f(x):\n    events.append('before')\n    x=x+1\n    events.append('after')\n    return x\nfor i in range(300):\n    assert f(i)==i+1\n",CompileOptions{promote_locals:false}).unwrap()).unwrap();
    let f = vm.get_global("f").unwrap();
    assert_eq!(vm.call(f, &[Value::Int(20)]).unwrap(), Value::Int(21));
    let bad = vm.heap.allocate(Object::List(vec![].into()));
    assert!(vm.call(f, &[bad]).is_err());
    vm.run_program(
        &mut compile("assert events[-1]=='before'\nassert events[-2]=='after'\n").unwrap(),
    )
    .unwrap();
    assert_eq!(vm.stack_len(), 0);
}

#[test]
fn cached_inputs_survive_recursion_gc_ssa_parameter_updates_and_osr() {
    let mut vm = native(
        r#"
enabled=False
def churn(x):
    if enabled:
        for i in range(7000):
            garbage=[i]
    return x
def recurse(x,n):
    if n:
        return recurse(x,n-1)
    x.append(42)
    return churn(x)
def loop(a,n):
    initial=a
    for i in range(n):
        a=a+1
    return initial,a
for i in range(300):
    assert loop(10,3)==(10,13)
    assert recurse([],2)==[42]
enabled=True
assert recurse([123],5)==[123,42]
assert loop(20,10000)==(20,10020)
"#,
    );
    let f = vm.get_global("loop").unwrap();
    vm.profiling.mode = ProfileMode::Eager;
    let result = vm.call(f, &[Value::Int(30), Value::Int(10)]).unwrap();
    assert_eq!(
        vm.heap.get_item(result, Value::Int(0)).unwrap(),
        Value::Int(30)
    );
    assert_eq!(
        vm.heap.get_item(result, Value::Int(1)).unwrap(),
        Value::Int(40)
    );

    let mut osr = Vm::new();
    osr.tiering = Tiering::Native;
    osr.thresholds.function_entries = u64::MAX;
    osr.thresholds.loop_backedges = 4;
    osr.profiling.window_ticks = 4;
    osr.run_program(&mut compile("def f(a,n):\n    initial=a\n    for i in range(n):\n        a=a+1\n    return initial,a\nassert f(10,10000)==(10,10010)\n").unwrap()).unwrap();
    assert!(osr.tier_stats.native_osr_entries > 0);
    assert_eq!(osr.stack_len(), 0);
}
