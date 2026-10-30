use tis::{Tiering, Value, Vm, compile};

#[test]
fn comparisons_and_elif_chains_preserve_order_returns_and_loop_exits() {
    let source = r#"
events = []
def probe(label, value):
    events.append(label)
    return value
def choose(n):
    if probe(1, n == 1):
        result = 10
    elif probe(2, n == 2):
        result = 20
    elif probe(3, n == 3):
        return 30
    else:
        result = 40
    return result
def optional(n):
    if n == 1:
        value = 5
    elif n == 2:
        return 6
    elif n == 3:
        value = 7
    return value
for i in range(150):
    events.clear()
    assert choose(1) == 10 and events == [1]
    events.clear()
    assert choose(2) == 20 and events == [1, 2]
    events.clear()
    assert choose(3) == 30 and events == [1, 2, 3]
    events.clear()
    assert choose(4) == 40 and events == [1, 2, 3]
    events.clear()
    assert probe(1, 1) < probe(2, 2)
    assert events == [1, 2]
    events.clear()
    assert not (probe(1, 3) < probe(2, 2) < probe(3, 1))
    assert events == [1, 2]
    events.clear()
    assert probe(1, 1) < probe(2, 2) < probe(3, 3)
    assert events == [1, 2, 3]
    assert optional(1) == 5 and optional(2) == 6 and optional(3) == 7
events.clear()
if probe(1, False) and probe(2, True):
    raise ValueError('and evaluated false branch')
elif probe(3, True) or probe(4, False):
    selected = 1
else:
    selected = 2
assert selected == 1 and events == [1, 3]
events.clear()
count = 0
while probe(1, count < 3) and probe(2, True):
    count += 1
assert count == 3 and events == [1, 2, 1, 2, 1, 2, 1]
if not not probe(5, True):
    selected = 3
assert selected == 3
# Value contexts still return the original operand rather than a coerced bool.
left = []
right = [1]
assert (left and right) is left
assert (right or left) is right
total = 0
for i in range(10):
    if i == 0:
        continue
    elif i == 6:
        break
    elif i % 2:
        total += i
    else:
        total += 100
else:
    raise ValueError('break did not bypass loop else')
assert total == 209
"#;
    for tier in [
        Tiering::Bytecode,
        Tiering::Lir,
        Tiering::Native,
        Tiering::CopyPatch,
    ] {
        let mut vm = Vm::new();
        vm.tiering = tier;
        vm.profiling.window_ticks = 1;
        vm.thresholds.function_entries = 1;
        vm.thresholds.loop_backedges = 4;
        vm.run_program(&mut compile(source).unwrap()).unwrap();
        let optional = vm.get_global("optional").unwrap();
        assert!(vm.call(optional, &[Value::Int(0)]).is_err());
        assert_eq!(vm.stack_len(), 0);
        assert_eq!(vm.tier_stats.compilation_errors, 0);
        if tier == Tiering::Native {
            assert!(vm.tier_stats.native_compilations > 0);
        }
    }
}
