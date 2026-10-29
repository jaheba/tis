use tis::{BytecodeMode, Object, Tiering, Value, Vm, compile};

#[test]
fn reused_frames_do_not_keep_arguments_alive_after_return_or_error() {
    for tier in [
        Tiering::Bytecode,
        Tiering::Lir,
        Tiering::Native,
        Tiering::CopyPatch,
    ] {
        let mut vm = Vm::new();
        vm.tiering = tier;
        vm.thresholds.function_entries = 1;
        vm.run_program(
            &mut compile("def use(x):\n    return len(x)\ndef fail(x):\n    return 1//0\n")
                .unwrap(),
        )
        .unwrap();
        let use_fn = vm.get_global("use").unwrap();
        let fail_fn = vm.get_global("fail").unwrap();
        for function in [use_fn, fail_fn, use_fn] {
            let object = vm.heap.allocate(Object::List(vec![Value::Int(123)].into()));
            let result = vm.call(function, &[object]);
            if function == fail_fn {
                assert!(result.is_err());
            } else {
                assert_eq!(result.unwrap(), Value::Int(1));
            }
            assert_eq!(vm.stack_len(), 0);
            vm.collect_garbage(&[]).unwrap();
            assert!(vm.heap.get(object).is_err());
        }
    }
}

#[test]
fn positional_defaults_and_bound_receivers_survive_nested_collection() {
    let source = r#"
def churn():
    for i in range(7000):
        garbage=[i]
    return 1
seed=[40]
def f(x=seed,y=2):
    churn()
    return x[0]+y
class A:
    def method(self,x=seed,y=3):
        churn()
        return x[0]+y
assert f()==42
assert f([10])==12
assert f([10],5)==15
assert f(y=4)==44
assert f(*[[20]],**{'y':6})==26
assert A().method()==43
assert A().method([30],4)==34
"#;
    for tier in [
        Tiering::Bytecode,
        Tiering::Lir,
        Tiering::Native,
        Tiering::CopyPatch,
    ] {
        for mode in [BytecodeMode::Stack, BytecodeMode::Register] {
            let mut vm = Vm::new();
            vm.tiering = tier;
            vm.bytecode = mode;
            vm.thresholds.function_entries = 1;
            vm.thresholds.loop_backedges = 4;
            vm.run_program(&mut compile(source).unwrap()).unwrap();
            assert_eq!(vm.stack_len(), 0);
        }
    }
}

#[test]
fn recursion_errors_and_unbound_locals_leave_reusable_frames_clean() {
    for mode in [BytecodeMode::Stack, BytecodeMode::Register] {
        let mut vm = Vm::new();
        vm.bytecode = mode;
        vm.run_program(&mut compile("def recurse(n):\n    return recurse(n+1)\ndef maybe(flag):\n    if flag:\n        x=123\n    return x\n").unwrap()).unwrap();
        let recurse = vm.get_global("recurse").unwrap();
        let maybe = vm.get_global("maybe").unwrap();
        assert!(vm.call(recurse, &[Value::Int(0)]).is_err());
        assert_eq!(vm.stack_len(), 0);
        assert_eq!(
            vm.call(maybe, &[Value::Bool(true)]).unwrap(),
            Value::Int(123)
        );
        assert!(vm.call(maybe, &[Value::Bool(false)]).is_err());
        assert_eq!(vm.stack_len(), 0);
        assert_eq!(
            vm.call(maybe, &[Value::Bool(true)]).unwrap(),
            Value::Int(123)
        );
    }
}

#[test]
fn abandoned_prepared_methods_do_not_escape_into_reused_scratch_buffers() {
    let mut vm = Vm::new();
    vm.run_program(&mut compile("def fail():\n    return 1//0\ndef bad():\n    [].append(fail())\ndef good():\n    [].append(123)\n").unwrap()).unwrap();
    let bad = vm.get_global("bad").unwrap();
    let good = vm.get_global("good").unwrap();
    assert!(vm.call(bad, &[]).is_err());
    vm.collect_garbage(&[]).unwrap();
    assert_eq!(vm.call(good, &[]).unwrap(), Value::None);
    assert_eq!(vm.stack_len(), 0);
}
