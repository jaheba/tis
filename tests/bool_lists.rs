use tis::{BytecodeMode, Object, Tiering, Value, Vm, compile};

#[test]
fn compact_flags_keep_boolean_types_and_fall_back_for_mixed_writes() {
    let source = r#"
def fill(xs, value):
    for i in range(len(xs)):
        xs[i] = value
    return xs[-1]
flags = [True] * 128
for repeat in range(8):
    assert fill(flags, False) is False
    assert fill(flags, True) is True
assert flags[True] is True
assert flags[-1] is True
assert flags[:3] == [True, True, True]
assert list(iter(flags)) == [True] * 128
assert tuple(flags) == (True,) * 128
assert fill(flags, 1) is not True
assert type(flags[0]) is int
assert fill(flags, False) is False
assert fill(flags, [42]) == [42]
assert flags[0] is flags[-1]
assert flags[0][0] == 42
small = [True,False]
small.append(True)
small.insert(1,False)
assert small.pop() is True
small.reverse()
assert small == [False,False,True]
small[1] = 2
assert small == [False,2,True]
assert type(small[1]) is int
small.clear()
small.append(False)
assert small == [False]
assert [False] * 0 == []
assert [False] * -10 == []
"#;
    for tier in [
        Tiering::Bytecode,
        Tiering::Lir,
        Tiering::Native,
        Tiering::CopyPatch,
    ] {
        for encoding in [BytecodeMode::Register, BytecodeMode::Stack] {
            let mut vm = Vm::new();
            vm.tiering = tier;
            vm.bytecode = encoding;
            vm.thresholds.loop_backedges = 4;
            vm.thresholds.function_entries = 2;
            vm.profiling.window_ticks = 8;
            vm.run_program(&mut compile(source).unwrap()).unwrap();
            assert_eq!(vm.tier_stats.compilation_errors, 0);
            vm.collect_garbage(&[]).unwrap();
            let flags = vm.get_global("flags").unwrap();
            assert_eq!(
                vm.heap
                    .get_item(
                        vm.heap.get_item(flags, Value::Int(0)).unwrap(),
                        Value::Int(0)
                    )
                    .unwrap(),
                Value::Int(42)
            );
        }
    }
}

#[test]
fn shared_snapshots_are_invalidated_before_native_boolean_writes() {
    let mut vm = Vm::new();
    vm.tiering = Tiering::Native;
    vm.thresholds.loop_backedges = 4;
    vm.thresholds.function_entries = 2;
    vm.profiling.window_ticks = 8;
    vm.run_program(&mut compile("flags=[True]*128\ndef fill(flags,bit):\n    for i in range(len(flags)):\n        flags[i]=bit\n    return flags[0]\n").unwrap()).unwrap();
    let flags = vm.get_global("flags").unwrap();
    let fill = vm.get_global("fill").unwrap();
    for repeat in 0..12 {
        let bit = repeat % 2 == 0;
        assert_eq!(
            vm.call(fill, &[flags, Value::Bool(bit)]).unwrap(),
            Value::Bool(bit)
        );
        let Object::List(list) = vm.heap.get(flags).unwrap() else {
            unreachable!()
        };
        // This read caches a boxed snapshot after an earlier native writer was published.
        assert_eq!(list.as_slice(), vec![Value::Bool(bit); 128].as_slice());
        if repeat % 3 == 0 {
            vm.collect_garbage(&[]).unwrap();
        }
    }
    assert!(vm.tier_stats.native_list_stores > 0);
}
