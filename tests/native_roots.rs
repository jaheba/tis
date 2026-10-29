use tis::{Tiering, Value, Vm, compile, profile::ProfileMode};

#[test]
fn mixed_helper_results_survive_gc_and_scalar_deoptimization() {
    let mut vm = Vm::new();
    vm.tiering = Tiering::Native;
    vm.thresholds.function_entries = 1;
    vm.thresholds.loop_backedges = 4;
    vm.run_program(
        &mut compile(
            r#"
enabled=False
def make(flag):
    if flag:
        return [123]
    return 123
def probe(value,flag):
    result=make(flag)
    for i in range(7000 if enabled else 0):
        garbage=[i]
    return result,value+value
for i in range(300):
    probe(1,i%2==0)
enabled=True
"#,
        )
        .unwrap(),
    )
    .unwrap();
    let probe = vm.get_global("probe").unwrap();
    let entries = vm.tier_stats.native_entries;
    let deopts = vm.tier_stats.deoptimizations;

    // The call result alternates between a heap reference and a scalar at the same native site.
    // Allocations collect before the final arithmetic guard reconstructs its live values.
    let result = vm
        .call(probe, &[Value::Float(0.5), Value::Bool(true)])
        .unwrap();
    let list = vm.heap.get_item(result, Value::Int(0)).unwrap();
    assert_eq!(
        vm.heap.get_item(list, Value::Int(0)).unwrap(),
        Value::Int(123)
    );
    assert_eq!(
        vm.heap.get_item(result, Value::Int(1)).unwrap(),
        Value::Float(1.0)
    );
    assert!(vm.tier_stats.native_entries > entries);
    assert!(vm.tier_stats.deoptimizations > deopts);

    vm.profiling.mode = ProfileMode::Eager;
    let result = vm
        .call(probe, &[Value::Int(3), Value::Bool(false)])
        .unwrap();
    assert_eq!(
        vm.heap.get_item(result, Value::Int(0)).unwrap(),
        Value::Int(123)
    );
    assert_eq!(
        vm.heap.get_item(result, Value::Int(1)).unwrap(),
        Value::Int(6)
    );
    assert!(
        vm.call(probe, &[Value::Int(i64::MAX), Value::Bool(true)])
            .is_err()
    );
    vm.collect_garbage(&[]).unwrap();
    assert!(vm.heap.get(list).is_err());
    assert_eq!(vm.stack_len(), 0);
}
