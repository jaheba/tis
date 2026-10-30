use tis::{Error, Object, Tiering, Vm, compile};

#[test]
fn join_uses_borrowed_strings_with_unicode_boundaries_and_iterables() {
    let source = r#"
assert '-'.join([]) == ''
assert '-'.join(()) == ''
assert 'é'.join(['東京', '😀']) == '東京é😀'
assert ''.join(['abcdefgh', 'é']) == 'abcdefghé'
assert ''.join(['abcdefgh', 'é', 'x']) == 'abcdefghéx'
assert ''.join(['abcdefgh', 'é', 'xy']) == 'abcdefghéxy'
assert '\0'.join(('a', 'b')) == 'a\0b'
assert '-'.join(iter(['a', 'b'])) == 'a-b'
assert '-'.join('ab') == 'a-b'
assert '-'.join({'a': 1, 'b': 2}) == 'a-b'
long = 'a string that stays on the heap'
assert ''.join([long]) is long
assert 'sep'.join((long,)) is long
for i in range(200):
    assert ''.join(['é', '😀', 'abcde']) == 'é😀abcde'
"#;
    for tier in [
        Tiering::Bytecode,
        Tiering::Lir,
        Tiering::Native,
        Tiering::CopyPatch,
    ] {
        let mut vm = Vm::new();
        vm.tiering = tier;
        vm.profiling.window_ticks = 1;
        vm.thresholds.function_entries = 1;
        vm.thresholds.loop_backedges = 4;
        vm.run_program(&mut compile(source).unwrap()).unwrap();
        for source in [
            "''.join([1])",
            "''.join([1.0, 2.0])",
            "''.join(('a', 1))",
            "''.join(iter(['a', 1]))",
        ] {
            assert_eq!(
                vm.run_program(&mut compile(source).unwrap()).unwrap_err(),
                Error::Type("join requires strings")
            );
            assert_eq!(vm.stack_len(), 0);
        }
        assert_eq!(vm.tier_stats.compilation_errors, 0);
    }
}

#[test]
fn builtin_small_joins_create_no_python_heap_temporaries() {
    let mut vm = Vm::new();
    vm.run_program(&mut compile("def join(values):\n    return ''.join(values)\n").unwrap())
        .unwrap();
    let function = vm.get_global("join").unwrap();
    let values = vm.heap.allocate(Object::List(
        vec![
            vm.heap.string("é"),
            vm.heap.string("😀"),
            vm.heap.string("abcde"),
        ]
        .into(),
    ));
    let before = vm.heap.allocation_count();
    let result = vm.call(function, &[values]).unwrap();
    assert_eq!(vm.heap.as_str(&result), Some("é😀abcde"));
    // One lexical environment belongs to this function call; join itself needs no heap objects.
    assert!(vm.heap.allocation_count() <= before + 1);
}
