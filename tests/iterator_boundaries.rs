use tis::{Tiering, Vm, compile};

#[test]
fn primitive_unicode_and_recursive_combinators_share_iterator_semantics() {
    let source = r#"
assert list('aé東京😀') == ['a', 'é', '東', '京', '😀']
source = iter([7])
for i in range(200):
    source = enumerate(source)
value = next(source)
for i in range(200):
    assert value[0] == 0
    value = value[1]
assert value == 7
assert next(source, 99) == 99
key = (('x', 1), ('é', 2))
d = {key: 5}
assert list(d) == [key]
assert list(d.items()) == [(key, 5)]
assert list(zip('é😀', range(3))) == [('é', 0), ('😀', 1)]
iterator = iter(d)
d['new'] = 6
try:
    next(iterator)
except RuntimeError:
    changed = True
assert changed
"#;
    for tier in [
        Tiering::Bytecode,
        Tiering::Lir,
        Tiering::Native,
        Tiering::CopyPatch,
    ] {
        let mut vm = Vm::new();
        vm.tiering = tier;
        vm.run_program(&mut compile(source).unwrap()).unwrap();
    }
}
