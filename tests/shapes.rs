use tis::{BytecodeMode, Tiering, Value, Vm, compile};

fn modes(source: &str) {
    for tier in [
        Tiering::Bytecode,
        Tiering::Lir,
        Tiering::Native,
        Tiering::CopyPatch,
    ] {
        for bytecode in [BytecodeMode::Stack, BytecodeMode::Register] {
            let mut vm = Vm::new();
            vm.tiering = tier;
            vm.bytecode = bytecode;
            vm.thresholds.function_entries = 1;
            vm.thresholds.loop_backedges = 4;
            vm.run_program(&mut compile(source).unwrap()).unwrap();
            assert_eq!(vm.stack_len(), 0);
        }
    }
}

#[test]
fn field_sites_handle_different_layouts_additions_and_value_types() {
    modes(
        r#"
class C:
    def __init__(self,reverse):
        if reverse:
            self.y=20
            self.x=40
        else:
            self.x=10
            self.y=30
def read(o):
    return o.x+1
def write(o,x):
    o.x=x
a=C(False)
b=C(True)
for i in range(300):
    assert read(a)==11
    assert read(b)==41
    write(a,10)
a.extra=[a,b]
assert read(a)==11
write(a,0.5)
assert read(a)==1.5
write(a,42)
assert read(a)==43
assert a.extra[1] is b
assert a.y==30
assert b.y==20
"#,
    );
}

#[test]
fn class_fallback_and_inherited_method_replacement_remain_dynamic() {
    modes(
        r#"
class Base:
    x=10
    def method(self):
        return 20
class Child(Base):
    pass
def read(o):
    return o.x
a=Child()
b=Child()
b.x=99
for i in range(300):
    assert read(b)==99
    assert read(a)==10
Base.x=30
assert read(a)==30
assert read(b)==99
assert a.method()==20
Base.method=lambda self:40
assert a.method()==40
a.x=123
assert read(a)==123
"#,
    );
}

#[test]
fn cached_field_sites_do_not_retain_instances_or_assume_a_new_class_layout() {
    for tier in [
        Tiering::Bytecode,
        Tiering::Lir,
        Tiering::Native,
        Tiering::CopyPatch,
    ] {
        let mut vm = Vm::new();
        vm.tiering = tier;
        vm.thresholds.function_entries = 1;
        vm.thresholds.loop_backedges = 4;
        vm.run_program(&mut compile("class A:\n    pass\na=A()\na.x=[123]\ndef read(o):\n    return o.x\nfor i in range(300):\n    assert read(a)[0]==123\n").unwrap()).unwrap();
        let old = vm.get_global("a").unwrap();
        let read = vm.get_global("read").unwrap();
        vm.run_program(&mut compile("a=None\nA=None").unwrap())
            .unwrap();
        vm.collect_garbage(&[]).unwrap();
        assert!(vm.heap.get(old).is_err());
        vm.run_program(&mut compile("class B:\n    pass\nb=B()\nb.y=11\nb.x=22").unwrap())
            .unwrap();
        let b = vm.get_global("b").unwrap();
        assert_eq!(vm.call(read, &[b]).unwrap(), Value::Int(22));
    }
}
