use tis::{Tiering, Vm, compile};

#[test]
fn text_and_import_workloads_preserve_python_behavior_in_every_tier() {
    let source = r#"
from math import sqrt as root, sin
def imported():
    from math import cos as cosine
    return cosine(0)
assert root(9) == 3.0
assert sin(0) == 0.0
assert imported() == 1.0
assert 'a b  c '.split(None, 1) == ['a', 'b  c ']
assert ' a b '.split(None, 0) == ['a b ']
assert 'a,b,c'.split(',', 1) == ['a', 'b,c']
assert 'a,b,'.split(',', -1) == ['a', 'b', '']
assert ''.splitlines() == []
assert 'a\r\nb\rc\nd\v\f\x1c\x1d\x1e\x85\u2028\u2029'.splitlines() == ['a','b','c','d','','','','','','','']
assert 'a\r\nb\n'.splitlines(True) == ['a\r\n', 'b\n']
assert '東京'.rjust(4, 'é') == 'éé東京'
assert 'long'.rjust(-2) == 'long'
assert '7'.rjust(3) == '  7'
assert f'{0.000012:.2e}' == '1.20e-05'
assert f'{1200000.0:.2e}' == '1.20e+06'
for i in range(100):
    assert str(i).rjust(4, '0').splitlines() == [str(i).rjust(4, '0')]
"#;

    for tier in [
        Tiering::Bytecode,
        Tiering::Lir,
        Tiering::Native,
        Tiering::CopyPatch,
    ] {
        let mut program = compile(source).unwrap();
        let mut vm = Vm::new();
        vm.tiering = tier;
        vm.thresholds.function_entries = 2;
        vm.thresholds.loop_backedges = 8;
        vm.profiling.window_ticks = 32;
        vm.run_program(&mut program).unwrap();
        vm.collect_garbage(&[]).unwrap();
    }
}
