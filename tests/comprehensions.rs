use tis::{BytecodeMode, Tiering, Vm, compile};

fn run(source: &str) {
    for tier in [
        Tiering::Bytecode,
        Tiering::Lir,
        Tiering::Native,
        Tiering::CopyPatch,
    ] {
        for encoding in [BytecodeMode::Register, BytecodeMode::Stack] {
            let mut vm = Vm::new();
            vm.tiering = tier;
            vm.bytecode = encoding;
            vm.thresholds.function_entries = 2;
            vm.thresholds.loop_backedges = 4;
            vm.profiling.window_ticks = 8;
            vm.run_program(&mut compile(source).unwrap()).unwrap();
            assert_eq!(vm.stack_len(), 0);
            assert_eq!(vm.tier_stats.compilation_errors, 0);
            vm.collect_garbage(&[]).unwrap();
        }
    }
}

#[test]
fn sieve_comprehension_keeps_marking_locals_in_ssa() {
    let source = include_str!("fixtures/sieve_comprehension.py").replace(
        "print(len(sieve(30_000_000)))",
        "assert len(sieve(5000)) == 669",
    );
    let program = compile(&source).unwrap();
    let function = program.function("sieve").unwrap();
    assert!(function.local_slots().iter().any(|n| n == "is_prime"));
    assert!(function.local_slots().iter().any(|n| n == "i"));
    assert!(!function.to_string().contains("LoadName(\"is_prime\")"));
    run(&source);
}

#[test]
fn private_targets_preserve_outer_bindings_and_nested_evaluation_order() {
    run(r#"
x = 40
def f():
    i = 3
    offset = 0
    j = 90
    first = [i + offset for i in range(i)]
    assert first == [0, 1, 2]
    assert i == 3
    assert [i for i in []] == []
    assert i == 3
    assert [(i,j) for i in range(3) for j in range(i) if j >= 0] == [(1,0),(2,0),(2,1)]
    assert [[i+j for j in range(2)] for i in range(2)] == [[0,1],[1,2]]
    assert {i:i+1 for i in range(3)} == {0:1,1:2,2:3}
    assert {i for i in [1,1,2]} == {1,2}
    assert i == 3 and j == 90
    return [x for x in [x]]
def g():
    global x
    assert [x for x in range(2)] == [0,1]
    assert x == 40
    return x
for repeat in range(8):
    assert f() == [40]
    assert g() == 40
"#);
}

#[test]
fn callbacks_keep_live_cells_and_gc_roots_and_errors_do_not_leak_targets() {
    run(r#"
def f():
    value = [0]
    def change(i):
        nonlocal value
        for j in range(2500):
            garbage = [j]
        value = [i]
        return True
    result = [value for i in range(5) if change(i)]
    assert result == [[0],[1],[2],[3],[4]]
    assert value == [4]
    i = 99
    try:
        [10 // i for i in [2,0,1]]
    except ZeroDivisionError:
        pass
    assert i == 99
    return result
for repeat in range(4):
    assert f() == [[0],[1],[2],[3],[4]]
"#);
}

#[test]
fn escaping_closures_keep_distinct_comprehension_cells() {
    run(r#"
def f():
    i = 99
    groups = []
    for n in [2,4]:
        groups.append([lambda: i for i in range(n)])
    assert i == 99
    return groups
for repeat in range(6):
    groups = f()
    assert [g() for g in groups[0]] == [1,1]
    assert [g() for g in groups[1]] == [3,3,3,3]
class C:
    xs = [i for i in range(3)]
assert C.xs == [0,1,2]
"#);
}

#[test]
fn unbound_comprehension_targets_keep_the_original_scope() {
    run(r#"
j = 80
def bad():
    offset = 1
    return [j + offset for i in [1] for j in [j]]
def repeated():
    offset = 0
    for start in [0,1]:
        values = [j + offset for i in [start] for j in ([7] if i == 0 else [j])]
    return values
for repeat in range(5):
    try:
        bad()
    except UnboundLocalError as exc:
        assert 'j' in str(exc)
        assert '.comp' not in str(exc)
    else:
        assert False
    try:
        repeated()
    except UnboundLocalError:
        pass
    else:
        assert False
assert j == 80
"#);
}
