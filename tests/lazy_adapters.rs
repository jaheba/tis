use tis::{Tiering, Vm, compile};

#[test]
fn adapters_are_lazy_and_mutable_bytes_preserve_values() {
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
from itertools import count, islice
seen = []
def record(x):
    seen.append(x)
    return x * 2
m = map(record, count(3, 2))
assert seen == []
assert list(islice(m, 1, 4, 2)) == [10, 18]
assert seen == [3, 5, 7, 9]
assert next(m) == 22
assert list(filter(None, [0, 1, False, 2])) == [1, 2]
assert list(map(lambda a, b: a + b, [1,2,3], [10,20])) == [11,22]
a = bytearray(b'abc')
a[0] = 255
a.append(0)
a.extend(b'xy')
assert bytes(a) == b'\xffbc\x00xy'
assert bytes(a[1:4]) == b'bc\x00'
assert bytes(a * 2) == bytes(a) + bytes(a)
assert a.pop() == 121
assert [*range(3), 4] == [0,1,2,4]
assert (*range(3), 4) == (0,1,2,4)
def f():
    pass
f.data = [1]
assert f.data == [1]
del f.data
"#,
            )
            .unwrap(),
        )
        .unwrap();
        vm.collect_garbage(&[]).unwrap();
    }
}
