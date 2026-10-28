use tis::{BytecodeMode, Tiering, Value, Vm, compile};

#[test]
fn literals_are_shared_across_sites_and_remain_rooted_through_collection() {
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
        let mut program = compile("a='shared literal'\ndef f():\n    return 'shared literal'\nfor i in range(500):\n    assert f() is a\n").unwrap();
        vm.run_program(&mut program).unwrap();
        let a = vm.get_global("a").unwrap();
        vm.collect_garbage(&[]).unwrap();
        let f = vm.get_global("f").unwrap();
        assert_eq!(vm.call(f, &[]).unwrap(), a);
        assert!(vm.tier_stats.literal_hits > 0);
        let before = vm.tier_stats.interned_literals;
        for _ in 0..100 {
            assert_eq!(vm.call(f, &[]).unwrap(), a);
        }
        assert_eq!(vm.tier_stats.interned_literals, before);
    }
}

#[test]
fn shared_code_caches_do_not_cross_vm_heaps_or_namespace_layouts() {
    let mut program =
        compile("value='shared literal'\nassert value == 'shared literal'\n").unwrap();
    let mut a = Vm::new();
    let mut b = Vm::new();
    b.run_program(&mut compile("padding=123\nother=456").unwrap())
        .unwrap();
    for _ in 0..5 {
        a.run_program(&mut program).unwrap();
        b.run_program(&mut program).unwrap();
        let av = a.get_global("value").unwrap();
        let bv = b.get_global("value").unwrap();
        assert_ne!(av, bv);
        assert!(a.heap.get(bv).is_err());
        assert!(b.heap.get(av).is_err());
        a.collect_garbage(&[]).unwrap();
        b.collect_garbage(&[]).unwrap();
    }
}

#[test]
fn cached_globals_observe_rebinding_and_missing_names_can_be_defined_later() {
    for mode in [BytecodeMode::Stack, BytecodeMode::Register] {
        let mut vm = Vm::new();
        vm.bytecode = mode;
        vm.run_program(&mut compile("x=1\ndef read():\n    return x\ndef change():\n    global x\n    x=2\ndef missing():\n    return late\n").unwrap()).unwrap();
        let read = vm.get_global("read").unwrap();
        let change = vm.get_global("change").unwrap();
        let missing = vm.get_global("missing").unwrap();
        assert_eq!(vm.call(read, &[]).unwrap(), Value::Int(1));
        assert_eq!(vm.call(read, &[]).unwrap(), Value::Int(1));
        vm.call(change, &[]).unwrap();
        assert_eq!(vm.call(read, &[]).unwrap(), Value::Int(2));
        assert!(vm.call(missing, &[]).is_err());
        vm.run_program(&mut compile("another=999\nlate=42\nx=3").unwrap())
            .unwrap();
        assert_eq!(vm.call(missing, &[]).unwrap(), Value::Int(42));
        assert_eq!(vm.call(read, &[]).unwrap(), Value::Int(3));
        if mode == BytecodeMode::Register {
            assert!(vm.tier_stats.name_cache_hits > 0);
        }
    }
}

#[test]
fn lexical_class_and_nonlocal_scopes_keep_python_lookup_rules() {
    let source = r#"
x=10
def outer():
    x=20
    def read():
        return x
    def change():
        nonlocal x
        x=30
    return read,change
read,change=outer()
assert read()==20
change()
assert read()==30
class A:
    x=40
    y=x
    def read(self):
        return x
assert A.y==40
assert A().read()==10
assert x==10
for i in range(300):
    assert read()==30
assert ('dynamic'+str(123)) is not ('dynamic'+str(123))
"#;
    for tier in [
        Tiering::Bytecode,
        Tiering::Lir,
        Tiering::Native,
        Tiering::CopyPatch,
    ] {
        for mode in [BytecodeMode::Stack, BytecodeMode::Register] {
            let mut vm = Vm::new();
            vm.bytecode = mode;
            vm.tiering = tier;
            vm.thresholds.function_entries = 1;
            vm.thresholds.loop_backedges = 4;
            vm.run_program(&mut compile(source).unwrap()).unwrap();
        }
    }
}
