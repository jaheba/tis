use tis::{Tiering, Vm, compile};

#[test]
fn text_binary_patterns_backreferences_and_match_owners() {
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
import re
p = re.compile(r'^(\S+) ([0-9]+)$')
m = p.match('héllo 42')
assert m.group(0, 1, 2) == ('héllo 42', 'héllo', '42')
assert p.match('bad') is None
assert re.search(r'(Python)\1', 'xxPythonPython').group(1) == 'Python'
assert re.findall(r'(a)(b)', 'abxxab') == [('a', 'b'), ('a', 'b')]
assert re.findall(b'[ab]', b'\xffab\x00') == [b'a', b'b']
assert re.sub(b'[ab]', b'X', b'\xffabc') == b'\xffXXc'
assert re.sub('é', 'ß', 'héé') == 'hßß'
"#,
            )
            .unwrap(),
        )
        .unwrap();
        vm.collect_garbage(&[]).unwrap();
        vm.run_program(&mut compile("assert m.group(1) == 'héllo'\n").unwrap())
            .unwrap();
    }
}
