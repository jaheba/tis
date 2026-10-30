use tis::{BytecodeMode, Object, Value, Vm, compile};

#[test]
fn short_literal_comparisons_keep_heap_mixed_and_unicode_semantics() {
    let source = r#"
def check(value):
    return [value == 'é😀abcde', value != 'é😀abcde', 'é😀abcde' == value, 'é😀abcde' != value]
def empty(value):
    return [value == '', value != '']
def nul(value):
    return [value == 'a\0b', value != 'a\0b']
"#;
    let mut program = compile(source).unwrap();
    assert!(
        program
            .function_mut("check")
            .unwrap()
            .bytecode()
            .unwrap()
            .disassemble_register()
            .unwrap()
            .contains("StringEqual")
    );
    for mode in [BytecodeMode::Stack, BytecodeMode::Register] {
        let mut vm = Vm::new();
        vm.bytecode = mode;
        vm.run_program(&mut program).unwrap();
        let function = vm.get_global("check").unwrap();
        let equal = vm.heap.allocate(Object::String("é😀abcde".into()));
        let values = [
            (vm.heap.string("é😀abcde"), true),
            (equal, true),
            (vm.heap.string("abcdefghé"), false),
            (vm.heap.string("😀😀😀"), false),
            (Value::None, false),
            (Value::Bool(true), false),
            (Value::Int(1), false),
            (
                vm.heap.allocate(Object::List(Vec::<Value>::new().into())),
                false,
            ),
        ];
        for (value, expected) in values {
            let result = vm.call(function, &[value]).unwrap();
            let Object::List(items) = vm.heap.get(result).unwrap() else {
                panic!()
            };
            assert_eq!(
                items.iter().copied().collect::<Vec<_>>(),
                vec![
                    Value::Bool(expected),
                    Value::Bool(!expected),
                    Value::Bool(expected),
                    Value::Bool(!expected)
                ]
            );
        }
        for (name, text) in [("empty", ""), ("nul", "a\0b")] {
            let f = vm.get_global(name).unwrap();
            let text = vm.heap.allocate(Object::String(text.into()));
            let result = vm.call(f, &[text]).unwrap();
            let Object::List(items) = vm.heap.get(result).unwrap() else {
                panic!()
            };
            assert_eq!(
                items.iter().copied().collect::<Vec<_>>(),
                vec![Value::Bool(true), Value::Bool(false)]
            );
        }
    }
}

#[test]
fn native_inline_equality_avoids_callbacks_and_keeps_general_fallback() {
    use tis::Tiering;
    for (text, expression) in [
        ("é😀abcde", "'é😀abcde'"),
        ("a\0b", "'a\\0b'"),
        ("abcdefghé", "'abcdefghé'"),
    ] {
        for operator in ["==", "!="] {
            let mut vm = Vm::new();
            vm.tiering = Tiering::Native;
            vm.profiling.window_ticks = 1;
            vm.thresholds.function_entries = 1;
            let source = format!("def check(value):\n    return value {operator} {expression}\n");
            vm.run_program(&mut compile(&source).unwrap()).unwrap();
            let function = vm.get_global("check").unwrap();
            let equal = vm.heap.string(text);
            let unequal = vm.heap.string("different");
            for _ in 0..4 {
                vm.call(function, &[equal]).unwrap();
            }
            let before = vm.tier_stats.native_runtime_calls;
            let yes = Value::Bool(operator == "==");
            let no = Value::Bool(operator != "==");
            assert_eq!(vm.call(function, &[equal]).unwrap(), yes);
            assert_eq!(vm.call(function, &[unequal]).unwrap(), no);
            // Importing the boxed parameter still costs one callback per invocation.
            // The comparisons add none; the previous generic path required four callbacks here.
            assert!(vm.tier_stats.native_runtime_calls <= before + 2);
            let heap_string = vm.heap.allocate(Object::String(text.into()));
            assert_eq!(vm.call(function, &[heap_string]).unwrap(), yes);
            assert_eq!(vm.call(function, &[Value::Int(1)]).unwrap(), no);
            assert_eq!(vm.call(function, &[Value::None]).unwrap(), no);
            assert!(vm.tier_stats.native_runtime_calls > before);
            vm.collect_garbage(&[heap_string]).unwrap();
            assert_eq!(vm.call(function, &[heap_string]).unwrap(), yes);
            assert_eq!(vm.tier_stats.compilation_errors, 0);
            assert!(vm.tier_stats.native_compilations > 0);
        }
    }
}
