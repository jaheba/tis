use tis::{Tiering, Vm, compile};
#[test]
fn list_subclasses_keep_storage_and_overrides() {
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
                r"
class L(list):
    pass
a = L([1, 2])
a.label = 'items'
assert 'v%s' % 42 == 'v42'
assert '%s:%d:%%' % ('x', 3) == 'x:3:%'
a.append(3)
assert a == [1,2,3]
assert len(a) == 3 and list(a) == [1,2,3]
assert isinstance(a, list) and isinstance(a, L)
assert type(a) is L
assert 2 in a and a[1] == 2
a[1] = 9
assert a.pop() == 3
assert a.label == 'items'
class Override(L):
    def __len__(self):
        return 42
    def append(self, value):
        super().append(value * 2)
b = Override([5])
b.append(3)
assert len(b) == 42
assert list.__len__(b) == 2
assert b == [5,6]
",
            )
            .unwrap(),
        )
        .unwrap();
        vm.collect_garbage(&[]).unwrap();
        vm.run_program(&mut compile("assert a == [1,9] and b == [5,6]\n").unwrap())
            .unwrap();
    }
}
