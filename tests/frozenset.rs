use tis::{Tiering, Vm, compile};
#[test]
fn immutable_sets_share_numeric_keys_and_preserve_algebra_and_gc() {
    for tier in [
        Tiering::Bytecode,
        Tiering::Lir,
        Tiering::Native,
        Tiering::CopyPatch,
    ] {
        let mut vm = Vm::new();
        vm.tiering = tier;
        vm.run_program(
            &mut compile(
                r#"
a = frozenset([1, 2, 3, True])
b = frozenset([3.0, 2, 1])
assert a == b and a == {1, 2, 3}
assert hash(a) == hash(b)
assert {a: 42}[b] == 42
assert a - frozenset([2]) == {1,3}
assert a & frozenset([3,4]) == {3}
assert a | frozenset([4]) == {1,2,3,4}
assert a ^ frozenset([3,4]) == {1,2,4}
assert a < frozenset([1,2,3,4])
assert a <= b and a >= b
assert not (a < b) and not (a > b)
assert isinstance(a, frozenset)
assert not isinstance(a, set)
assert frozenset(a) is a
assert a.copy() is a
assert len(list(a)) == 3
"#,
            )
            .unwrap(),
        )
        .unwrap();
        let a = vm.get_global("a").unwrap();
        vm.collect_garbage(&[a]).unwrap();
    }
}
