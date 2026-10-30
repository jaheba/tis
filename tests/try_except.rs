use tis::{Tiering, Value, Vm, compile};

#[test]
fn try_expressions_match_dynamic_types_and_keep_loop_control_and_else() {
    let source = r#"
import json
events = []
def parse(text):
    events.append(text)
    return json.loads(text)
total = 0
for text in ['1', 'bad', '2', 'bad', '3']:
    try:
        value = parse(text)
    except ValueError:
        continue
    else:
        total += value
assert total == 6
assert len(events) == 5
for text in ['bad', '1']:
    try:
        value = parse(text)
    except (TypeError, ValueError):
        break
assert text == 'bad'
def fail():
    raise ValueError('test')
Alias = ValueError
def recover():
    try:
        fail()
    except TypeError:
        return 0
    except Alias:
        return 8
assert recover() == 8
Alias = TypeError
def unmatched():
    try:
        fail()
    except Alias:
        return 0
def bare():
    raise
def nested():
    try:
        fail()
    except ValueError:
        bare()
def catch_all():
    try:
        fail()
    except:
        return 9
assert catch_all() == 9
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
        for name in ["unmatched", "nested"] {
            let f = vm.get_global(name).unwrap();
            assert_eq!(vm.call(f, &[]).unwrap_err().to_string(), "ValueError: test");
        }
        let bare = vm.get_global("bare").unwrap();
        assert!(
            vm.call(bare, &[])
                .unwrap_err()
                .to_string()
                .contains("No active exception")
        );
        assert_eq!(vm.stack_len(), 0);
        assert_eq!(vm.tier_stats.compilation_errors, 0);
    }
}

#[test]
fn caught_exceptions_remain_rooted_and_pending_errors_do_not_leak_between_calls() {
    let source = r#"
def fail():
    raise ValueError('a long heap allocated exception message')
def churn():
    for i in range(5000):
        garbage = [i]
def again():
    raise
def preserve():
    try:
        fail()
    except ValueError:
        churn()
        again()
def broken():
    try:
        fail()
    except (ValueError, 42):
        pass
def active():
    try:
        fail()
    except ValueError:
        for i in range(3):
            continue
        return 5
def nested_handler():
    try:
        fail()
    except ValueError:
        active()
        again()
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
        let preserve = vm.get_global("preserve").unwrap();
        assert_eq!(
            vm.call(preserve, &[]).unwrap_err().to_string(),
            "ValueError: a long heap allocated exception message"
        );
        let broken = vm.get_global("broken").unwrap();
        assert!(
            vm.call(broken, &[])
                .unwrap_err()
                .to_string()
                .contains("catching classes")
        );
        let active = vm.get_global("active").unwrap();
        assert_eq!(vm.call(active, &[]).unwrap(), Value::Int(5));
        let nested = vm.get_global("nested_handler").unwrap();
        assert_eq!(
            vm.call(nested, &[]).unwrap_err().to_string(),
            "ValueError: a long heap allocated exception message"
        );
        let again = vm.get_global("again").unwrap();
        assert!(
            vm.call(again, &[])
                .unwrap_err()
                .to_string()
                .contains("No active exception")
        );
        assert_eq!(vm.stack_len(), 0);
    }
}

#[test]
fn internal_try_bodies_share_lexical_scope_without_per_iteration_functions() {
    let source = r#"
class Example:
    x = 7
    try:
        y = x + 1
    except Exception:
        y = 0
assert Example.y == 8
def run(n):
    total = 0
    for i in range(n):
        try:
            value = i
        except Exception:
            return -1
        total += value
    return total
"#;
    let mut vm = Vm::new();
    vm.run_program(&mut compile(source).unwrap()).unwrap();
    let run = vm.get_global("run").unwrap();
    assert_eq!(vm.call(run, &[Value::Int(1)]).unwrap(), Value::Int(0));
    let allocations = vm.heap.allocation_count();
    assert_eq!(vm.call(run, &[Value::Int(100)]).unwrap(), Value::Int(4950));
    // One outcome per iteration, plus the enclosing frame and range/iterator/exhaustion objects.
    assert!(vm.heap.allocation_count() - allocations <= 106);
}
