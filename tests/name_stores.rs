use tis::{Tiering, Vm, compile};

#[test]
fn cached_native_stores_keep_references_visible_to_gc() {
    let source = r#"
saved = None
for i in range(400):
    saved = [i]
    for j in range(30):
        unused = [j]
    assert saved[0] == i
assert saved == [399]
"#;
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
        vm.run_program(&mut compile(source).unwrap()).unwrap();
        vm.collect_garbage(&[]).unwrap();
        vm.run_program(&mut compile("assert saved == [399]").unwrap())
            .unwrap();
        assert_eq!(vm.tier_stats.compilation_errors, 0);
        if tier == Tiering::Native {
            assert!(vm.tier_stats.native_compilations > 0);
        }
    }
}

#[test]
fn fresh_function_environments_globals_and_nonlocals_do_not_reuse_wrong_slots() {
    let source = r#"
x = -1
def outer():
    x = 0
    def update(n):
        nonlocal x
        for i in range(200):
            x = n + i
        return x
    return update
left = outer()
right = outer()
assert left(1) == 200
assert right(1000) == 1199
assert left(5) == 204
assert x == -1
def global_update(n):
    global x
    for i in range(200):
        x = n + i
    return x
assert global_update(1) == 200
assert global_update(1000) == 1199
assert x == 1199
"#;
    for tier in [Tiering::Lir, Tiering::Native, Tiering::CopyPatch] {
        let mut vm = Vm::new();
        vm.tiering = tier;
        vm.thresholds.function_entries = 1;
        vm.thresholds.loop_backedges = 4;
        vm.run_program(&mut compile(source).unwrap()).unwrap();
        vm.collect_garbage(&[]).unwrap();
        assert_eq!(vm.tier_stats.compilation_errors, 0);
    }
}
