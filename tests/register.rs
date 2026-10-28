use tis::{BytecodeMode, Tiering, Value, Vm, compile};

const MODES: [BytecodeMode; 2] = [BytecodeMode::Stack, BytecodeMode::Register];
const TIERS: [Tiering; 4] = [
    Tiering::Bytecode,
    Tiering::Lir,
    Tiering::Native,
    Tiering::CopyPatch,
];

#[test]
fn encodings_share_parallel_edges_nested_calls_and_tier_handoffs() {
    let source = "
def outer(x):
    def inner(y):
        return x + y
    return inner

def f(n):
    a, b = 1, 2
    values = []
    for i in range(n):
        a, b = b, a
        values.append(outer(a)(b))
    return (a, b, sum(values))
";
    for tier in TIERS {
        for mode in MODES {
            let mut program = compile(source).unwrap();
            let mut vm = Vm::new();
            vm.tiering = tier;
            vm.bytecode = mode;
            vm.thresholds.function_entries = 4;
            vm.thresholds.loop_backedges = 4;
            vm.profiling.window_ticks = 8;
            vm.run_program(&mut program).unwrap();
            let function = vm.get_global("f").unwrap();
            for _ in 0..12 {
                let result = vm.call(function, &[Value::Int(21)]).unwrap();
                assert!(
                    matches!(vm.heap.get(result).unwrap(), tis::Object::Tuple(values)
                    if values == &[Value::Int(2), Value::Int(1), Value::Int(63)])
                );
            }
            // Encoding is an execution choice, so warmed profiles and artifacts remain usable.
            vm.bytecode = if mode == BytecodeMode::Stack {
                BytecodeMode::Register
            } else {
                BytecodeMode::Stack
            };
            let result = vm.call(function, &[Value::Int(20)]).unwrap();
            assert!(
                matches!(vm.heap.get(result).unwrap(), tis::Object::Tuple(values)
                if values == &[Value::Int(1), Value::Int(2), Value::Int(60)])
            );
        }
    }
}

#[test]
fn register_encoding_removes_stack_transport_instructions() {
    let mut program = compile("def f(a,b):\n    return a+b\n").unwrap();
    let code = program.function_mut("f").unwrap().bytecode().unwrap();
    assert!(code.register_instruction_count() < code.stack_instruction_count().unwrap());
    let listing = code.disassemble_register().unwrap();
    assert_eq!(listing.matches(" Add ").count(), 1);
    assert!(!listing.contains("Observe"));
}

#[test]
fn language_fixture_agrees_between_encodings() {
    for mode in MODES {
        let mut program = compile(include_str!("fixtures/language.py")).unwrap();
        let mut vm = Vm::new();
        vm.bytecode = mode;
        vm.run_program(&mut program).unwrap();
        assert_eq!(vm.output, "language core ok\n");
    }
}

#[test]
fn eager_feedback_and_guard_failures_agree_between_encodings() {
    let mut profiles = Vec::new();
    for mode in MODES {
        let mut program = compile("def f(a,b):\n    return a+b\n").unwrap();
        let mut vm = Vm::new();
        vm.bytecode = mode;
        vm.profiling = tis::profile::ProfileConfig::eager();
        vm.run_program(&mut program).unwrap();
        let function = vm.get_global("f").unwrap();
        for _ in 0..12 {
            assert_eq!(
                vm.call(function, &[Value::Int(2), Value::Int(3)]).unwrap(),
                Value::Int(5)
            );
        }
        assert_eq!(
            vm.call(function, &[Value::Float(2.5), Value::Float(3.0)])
                .unwrap(),
            Value::Float(5.5)
        );
        assert!(vm.call(function, &[Value::None, Value::Int(1)]).is_err());
        assert_eq!(
            vm.call(function, &[Value::Int(4), Value::Int(5)]).unwrap(),
            Value::Int(9)
        );
        profiles.push(vm.function_hir(function).unwrap().profile().to_string());
    }
    assert_eq!(profiles[0], profiles[1]);
}
