use tis::{Object, Tiering, Vm, compile};

#[test]
fn hashed_sets_keep_numeric_equivalence_first_values_and_mutation_semantics() {
    let source = r#"
s = {True, 1, 1.0, 2, -0.0, 0}
assert len(s) == 3
assert list(s)[0] is True
assert 1.0 in s and False in s and 3 not in s
assert s == {0,2,1}
assert s != {0,1}
s.discard(99)
s.remove(1.0)
assert s == {0,2}
s.add(1)
assert s == {True, False, 2.0}
t = s.copy()
s.clear()
assert len(s) == 0 and len(t) == 3
assert repr(s) == 'set()'
first = 'a long heap allocated string'
same = ''.join(['a long heap ', 'allocated string'])
assert first is not same
s.add(first)
s.add(same)
assert len(s) == 1 and list(s)[0] is first
s.add(('é😀', (1, True)))
assert ('é😀', (1.0, 1)) in s
s.remove(('é😀', (True, 1.0)))
assert s == {first}
for value in [[], {}, set()]:
    try:
        s.add(value)
    except TypeError:
        pass
    else:
        assert False
    try:
        value in s
    except TypeError:
        pass
    else:
        assert False
events = []
def item(i):
    events.append(i)
    return []
try:
    set(item(i) for i in range(3))
except TypeError:
    pass
assert events == [0]
many = set(i for i in range(3000))
assert len(many) == 3000 and 2500 in many and 4000 not in many
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
        let set = vm.get_global("s").unwrap();
        vm.collect_garbage(&[set]).unwrap();
        let Object::Set(items) = vm.heap.get(set).unwrap() else {
            panic!()
        };
        let text = items[0].1;
        assert_eq!(vm.heap.as_str(&text), Some("a long heap allocated string"));
        assert_eq!(vm.stack_len(), 0);
        assert_eq!(vm.tier_stats.compilation_errors, 0);
    }
}
