use tis::{BytecodeMode, Heap, Object, Tiering, Value, Vm, compile};

#[test]
fn searches_bounds_callbacks_and_case_conversion_match_python() {
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
            vm.thresholds.function_entries = 2;
            vm.thresholds.loop_backedges = 8;
            vm.profiling.window_ticks = 8;
            vm.run_program(&mut compile(include_str!("fixtures/string_search.py")).unwrap())
                .unwrap();
            assert_eq!(vm.output, "search semantics ok\n", "{tier:?} {encoding:?}");
            assert_eq!(vm.tier_stats.compilation_errors, 0);
        }
    }
}

#[test]
fn unicode_offset_cache_is_invalidated_before_host_mutation_and_slot_reuse() {
    let mut heap = Heap::new();
    let value = heap.allocate(Object::String("aé中🐍".repeat(1000)));
    for i in 0..1000 {
        let position = i * 7919 % 4000;
        let item = heap.get_item(value, Value::Int(position)).unwrap();
        assert_eq!(
            heap.as_str(&item),
            Some(["a", "é", "中", "🐍"][position as usize % 4])
        );
    }
    heap.collect(&[value]).unwrap();
    let mut guard = heap.get_mut(value).unwrap();
    *guard = Object::String("😀xyz".repeat(700));
    std::mem::forget(guard);
    assert_eq!(heap.len(value).unwrap(), 2800);
    let item = heap.get_item(value, Value::Int(2400)).unwrap();
    assert_eq!(heap.as_str(&item), Some("😀"));
    heap.collect(&[]).unwrap();
    let new = heap.allocate(Object::String("東京é".repeat(1000)));
    assert!(heap.get_item(value, Value::Int(0)).is_err());
    assert_eq!(heap.len(new).unwrap(), 3000);
    let item = heap.get_item(new, Value::Int(2000)).unwrap();
    assert_eq!(heap.as_str(&item), Some("é"));
}

#[test]
fn short_unicode_and_repeated_lengths_keep_python_indices() {
    let heap = Heap::new();
    let value = heap.string("é中🐍".repeat(4).as_str());
    for i in 0..200 {
        assert_eq!(heap.len(value).unwrap(), 12);
        let key = i % 12;
        let item = heap.get_item(value, Value::Int(key)).unwrap();
        assert_eq!(
            heap.as_str(&item),
            Some(["é", "中", "🐍"][key as usize % 3])
        );
    }
}
