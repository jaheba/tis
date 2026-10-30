use tis::{Tiering, Value, Vm, compile};

#[test]
fn generator_expressions_are_lazy_and_keep_shared_capture_scope() {
    let source = r#"
events = []
def source():
    events.append(100)
    return [1, 2, 3, 4]
def accept(x):
    events.append(x)
    return x % 2 == 0
def body(x):
    events.append(x + 10)
    return x * 3
x = 99
g = (body(x) for x in source() if accept(x))
assert events == [100]
assert iter(g) is g
assert next(g) == 6
assert events == [100, 1, 2, 12]
assert next(g) == 12
assert next(g, 'end') == 'end'
assert next(g, 'end') == 'end'
assert x == 99
fs = list((lambda: x) for x in [1, 2, 3])
assert [f() for f in fs] == [3, 3, 3]
assert list(a + b for (a, b) in [(1,2), (3,4)]) == [3,7]
assert sum(i for i in range(10) if i > 3) == 39
assert ''.join(s for s in ['é', '😀', 'a']) == 'é😀a'
assert dict((i, i + 1) for i in range(3)) == {0:1,1:2,2:3}
events = []
assert any(accept(x) for x in [1,2,3])
assert events == [1,2]
events = []
assert not all(accept(x) for x in [2,3,4])
assert events == [2,3]
assert min(i for i in [4,1,3]) == 1
assert max(i for i in [4,1,3]) == 4
assert list(zip((i for i in range(3)), (i + 1 for i in range(3)))) == [(0,1),(1,2),(2,3)]
assert list(enumerate(i for i in [4,5])) == [(0,4),(1,5)]
xs = []
xs.extend(len(xs) for i in range(4))
assert xs == [0,1,2,3]
xs.extend(xs)
assert xs == [0,1,2,3,0,1,2,3]
"#;
    for tier in [
        Tiering::Bytecode,
        Tiering::Lir,
        Tiering::Native,
        Tiering::CopyPatch,
    ] {
        let mut vm = Vm::new();
        vm.tiering = tier;
        vm.thresholds.function_entries = 2;
        vm.thresholds.loop_backedges = 4;
        vm.profiling.window_ticks = 4;
        vm.run_program(&mut compile(source).unwrap()).unwrap();
        assert_eq!(vm.stack_len(), 0);
        assert_eq!(vm.tier_stats.compilation_errors, 0);
    }
}

#[test]
fn lazy_consumers_root_partial_results_and_close_on_errors() {
    let source = r#"
def churn(value):
    for i in range(5000):
        garbage = [i]
    return [value]
items = list(churn(i) for i in range(5))
assert items == [[0],[1],[2],[3],[4]]
rows = list(zip((churn(i) for i in range(3)), (churn(i+10) for i in range(3))))
assert rows == [([0],[10]),([1],[11]),([2],[12])]
assert sum((churn(i) for i in range(3)), []) == [0,1,2]
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
        vm.run_program(
            &mut compile("g = (1 // x for x in [1,0,2])\ndef step():\n    return next(g, 99)\n")
                .unwrap(),
        )
        .unwrap();
        let step = vm.get_global("step").unwrap();
        assert_eq!(vm.call(step, &[]).unwrap(), Value::Int(1));
        assert!(vm.call(step, &[]).is_err());
        assert_eq!(vm.call(step, &[]).unwrap(), Value::Int(99));
        vm.run_program(
            &mut compile(
                "bad = (next(iter([])) for i in [1])\ndef stop():\n    return next(bad)\n",
            )
            .unwrap(),
        )
        .unwrap();
        let stop = vm.get_global("stop").unwrap();
        assert!(
            vm.call(stop, &[])
                .unwrap_err()
                .to_string()
                .contains("generator raised StopIteration")
        );
        assert_eq!(vm.stack_len(), 0);
    }
}

#[test]
fn unpacking_and_errors_do_not_eagerly_consume_the_generator() {
    let mut vm = Vm::new();
    vm.run_program(&mut compile("events = []\ndef probe(x):\n    events.append(x)\n    return x\ng = (probe(x) for x in [1,2,3,4])\ndef unpack():\n    a,b = g\n").unwrap()).unwrap();
    let unpack = vm.get_global("unpack").unwrap();
    assert!(vm.call(unpack, &[]).is_err());
    vm.run_program(&mut compile("assert events == [1,2,3]\nassert next(g) == 4\nd = {}\ndef update():\n    d.update(row for row in [('ok', 1), ('bad',)])\n").unwrap()).unwrap();
    let update = vm.get_global("update").unwrap();
    assert!(vm.call(update, &[]).is_err());
    vm.run_program(&mut compile("assert d == {'ok': 1}\nevents = []\ndef bad_value(x):\n    events.append(x)\n    return 'bad'\ndef total():\n    return sum(bad_value(x) for x in [1,2,3])\n").unwrap()).unwrap();
    let total = vm.get_global("total").unwrap();
    assert!(vm.call(total, &[]).is_err());
    vm.run_program(&mut compile("assert events == [1]\ng = (next(g) for i in [1])\ndef reenter():\n    return next(g, 99)\n").unwrap()).unwrap();
    let reenter = vm.get_global("reenter").unwrap();
    assert!(
        vm.call(reenter, &[])
            .unwrap_err()
            .to_string()
            .contains("generator already executing")
    );
    assert_eq!(vm.call(reenter, &[]).unwrap(), Value::Int(99));
    assert_eq!(vm.stack_len(), 0);
}
