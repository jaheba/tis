use tis::{Tiering, Value, Vm, compile};

#[test]
fn expression_kernels_guard_rebound_calls_and_keep_interleaved_scopes() {
    let source = r#"
original_len = len
def make(rows):
    return (len(row['tags']) for row in rows)
a = make([{'tags': [1]}, {'tags': [1,2]}, {'tags': [1,2,3]}])
b = make([{'tags': [4,5]}, {'tags': []}])
assert next(a) == 1
assert next(b) == 2
def replacement(tags):
    for i in range(5000):
        garbage = [i]
    return original_len(tags) + 10
len = replacement
assert next(a) == 12
assert next(b) == 10
len = original_len
assert next(a) == 3
assert next(a, 99) == 99
assert list(x + 1 for x in range(10) if x < 4) == [1,2,3,4]
assert list(a+b for (a,b) in [(1,2),(3,4)]) == [3,7]
"#;
    for tier in [
        Tiering::Bytecode,
        Tiering::Lir,
        Tiering::Native,
        Tiering::CopyPatch,
    ] {
        let mut vm = Vm::new();
        vm.tiering = tier;
        vm.thresholds.function_entries = 2;
        vm.thresholds.loop_backedges = 4;
        vm.profiling.window_ticks = 4;
        vm.run_program(&mut compile(source).unwrap()).unwrap();
        assert!(vm.tier_stats.generator_expression_kernels > 0);
        assert!(vm.tier_stats.generator_direct_bindings > 0);
        assert_eq!(vm.tier_stats.compilation_errors, 0);
        assert_eq!(vm.stack_len(), 0);
    }
}

#[test]
fn noncollecting_kernel_respects_allocation_pressure_and_host_next_calls() {
    let mut vm = Vm::new();
    vm.run_program(&mut compile("data = [([1], [2])] * 10000\nassert sum(len(pair[0] + pair[1]) for pair in data) == 20000\ng = (i + 1 for i in range(10))\n").unwrap()).unwrap();
    assert!(vm.tier_stats.generator_expression_kernels > 9000);
    // Dead kernel temporaries must be collected through the ordinary-frame budget fallback.
    assert!(vm.heap.slot_count() < 8192);
    let next = vm.get_global("next").unwrap();
    let generator = vm.get_global("g").unwrap();
    for i in 1..=10 {
        assert_eq!(vm.call(next, &[generator]).unwrap(), Value::Int(i));
    }
    assert_eq!(
        vm.call(next, &[generator, Value::Int(99)]).unwrap(),
        Value::Int(99)
    );
    assert_eq!(vm.stack_len(), 0);
}
