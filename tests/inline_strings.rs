use tis::{BytecodeMode, Heap, InlineString, Object, Tiering, Value, Vm, compile};

#[test]
fn inline_capacity_counts_bytes_and_preserves_the_value_abi() {
    assert_eq!(std::mem::size_of::<Value>(), 16);
    assert_eq!(std::mem::size_of::<Option<Value>>(), 16);
    let heap = Heap::new();
    for text in ["", "a", "abcdefghijk", "é😀abcde", "a\0b"] {
        let value = heap.string(text);
        assert!(matches!(value, Value::InlineString(_)));
        assert_eq!(heap.as_str(&value), Some(text));
        assert!(heap.get(value).is_err());
    }
    assert_eq!(heap.allocation_count(), 0);
    for text in ["abcdefghijkl", "😀😀😀"] {
        assert!(InlineString::new(text).is_none());
        let value = heap.string(text);
        assert!(matches!(value, Value::Ref(_)));
        assert_eq!(heap.as_str(&value), Some(text));
    }
}

#[test]
fn inline_and_heap_strings_share_operations_keys_and_native_fields() {
    let source = r#"
import json
class Box:
    def __init__(self, value):
        self.value = value
box = Box('é😀abcde')
def read(box):
    return box.value
for i in range(300):
    assert read(box) == 'é😀abcde'
a = 'abc'
assert a == heap_short
assert a in [heap_short]
assert a in {heap_short}
d = {a: 42}
assert d[heap_short] == 42
assert {heap_short: 43}[a] == 43
assert list(d) == [a]
assert (a + 'def') == 'abcdef'
assert (a + 'defghijkl') == 'abcdefghijkl'
assert ('abcdefghijkl'[:3]) == a
assert '東京😀'[::-1] == '😀京東'
assert list('é😀') == ['é', '😀']
assert 'é😀'[1] == '😀'
assert 'é😀'[-1] == '😀'
assert 'ab' * 3 == 'ababab'
assert 'ab' * 0 == ''
assert 'x' * 20 == 'xxxxxxxxxxxxxxxxxxxx'
assert 'a' < heap_short
assert 'b' in a
assert bool(a) and not bool('')
assert isinstance(a, str)
assert chr(128512) == '😀' and ord('😀') == 128512
assert int('42') == 42 and float('1.5') == 1.5
assert '-'.join(['a','é']) == 'a-é'
assert 'a,b'.split(',') == ['a','b']
assert 'a\nb'.splitlines() == ['a','b']
assert 'hi'.rjust(4, 'é') == 'ééhi'
assert 'abc'.upper().lower() == a
assert ' abc '.strip() == a
assert a.startswith('a') and a.endswith('bc')
assert a.replace('b', 'é') == 'aéc'
assert json.loads('"é😀"') == 'é😀'
assert json.loads(json.dumps([a, 'abcdefghijkl'])) == [a, 'abcdefghijkl']
assert f'{a}' == a
assert repr('a\nb') == "'a\\nb'"
assert str('') == ''
assert ('dyn'+str(123)) is ('dyn'+str(123))
print(a, end='!', sep='')
"#;
    for bytecode in [BytecodeMode::Stack, BytecodeMode::Register] {
        for tier in [
            Tiering::Bytecode,
            Tiering::Lir,
            Tiering::Native,
            Tiering::CopyPatch,
        ] {
            let mut vm = Vm::new();
            vm.bytecode = bytecode;
            vm.tiering = tier;
            vm.thresholds.function_entries = 1;
            vm.thresholds.loop_backedges = 4;
            vm.readonly_regions = true;
            vm.mutation_regions = true;
            let heap_short = vm.heap.allocate(Object::String("abc".into()));
            let source = format!(
                "def check(heap_short):\n    global box, d, saved_short\n    saved_short = heap_short\n{}",
                source
                    .lines()
                    .map(|line| format!("    {line}\n"))
                    .collect::<String>()
            );
            vm.run_program(&mut compile(&source).unwrap()).unwrap();
            let check = vm.get_global("check").unwrap();
            vm.call(check, &[heap_short]).unwrap();
            assert_eq!(vm.output, "abc!");
            assert_eq!(vm.tier_stats.compilation_errors, 0);
            if tier == Tiering::Native {
                assert!(vm.tier_stats.native_compilations > 0);
            }
            if tier == Tiering::CopyPatch {
                assert!(vm.tier_stats.copy_patch_compilations > 0);
            }
            vm.collect_garbage(&[]).unwrap();
            vm.run_program(
                &mut compile("assert box.value == 'é😀abcde'\nassert d[saved_short] == 42")
                    .unwrap(),
            )
            .unwrap();
        }
    }
}

#[test]
fn lir_embeds_only_heap_independent_string_constants() {
    use tis::{
        hir::Constant,
        lir::{Helper, LoweringMode, Operation},
    };
    let mut program =
        compile("def short():\n    return 'é😀abcde'\ndef long():\n    return '😀😀😀'\n").unwrap();
    let mut short = program
        .function_mut("short")
        .unwrap()
        .lower_lir(LoweringMode::Generic)
        .unwrap();
    assert!(short.blocks.iter().flat_map(|b| &b.instructions).any(|i| matches!(&i.operation, Operation::Constant(Constant::String(text)) if text == "é😀abcde")));
    assert!(
        !short
            .blocks
            .iter()
            .flat_map(|b| &b.instructions)
            .any(|i| matches!(
                &i.operation,
                Operation::Call {
                    helper: Helper::StringConstant(_),
                    ..
                }
            ))
    );
    let long = program
        .function_mut("long")
        .unwrap()
        .lower_lir(LoweringMode::Generic)
        .unwrap();
    assert!(long.blocks.iter().flat_map(|b| &b.instructions).any(|i| matches!(&i.operation, Operation::Call { helper: Helper::StringConstant(_), effects, .. } if effects.needs_safepoint())));
    for instruction in short.blocks.iter_mut().flat_map(|b| &mut b.instructions) {
        if let Operation::Constant(Constant::String(text)) = &mut instruction.operation {
            *text = "😀😀😀".into();
        }
    }
    assert!(short.verify().is_err());
}

#[test]
fn compiled_inline_literals_return_without_runtime_helpers() {
    for tier in [Tiering::Lir, Tiering::Native, Tiering::CopyPatch] {
        let mut vm = Vm::new();
        vm.tiering = tier;
        vm.profiling.window_ticks = 1;
        vm.thresholds.function_entries = 1;
        vm.run_program(&mut compile("def literal():\n    return 'é😀abcde'\n").unwrap())
            .unwrap();
        let function = vm.get_global("literal").unwrap();
        for _ in 0..3 {
            vm.call(function, &[]).unwrap();
        }
        let calls = vm.tier_stats.native_runtime_calls;
        for _ in 0..3 {
            let result = vm.call(function, &[]).unwrap();
            assert_eq!(vm.heap.as_str(&result), Some("é😀abcde"));
            assert!(matches!(result, Value::InlineString(_)));
        }
        assert_eq!(vm.tier_stats.compilation_errors, 0);
        if tier == Tiering::Native {
            assert!(vm.tier_stats.native_compilations > 0);
            assert_eq!(vm.tier_stats.native_runtime_calls, calls);
        }
        if tier == Tiering::CopyPatch {
            assert!(vm.tier_stats.copy_patch_compilations > 0);
        }
    }
}

#[test]
fn bytecode_string_iteration_preserves_unicode_and_sticky_exhaustion() {
    let mut vm = Vm::new();
    vm.tiering = Tiering::Bytecode;
    let mut program = compile(
        r#"
for text in ['a' * 500, 'é😀東京' * 200, '', 'abc']:
    iterator = iter(text)
    result = ''
    for character in iterator:
        result += character
    assert result == text
    assert next(iterator, 'end') == 'end'
    assert next(iterator, 'end') == 'end'
"#,
    )
    .unwrap();
    vm.run_program(&mut program).unwrap();
}

#[test]
fn indexed_ascii_cache_is_invalidated_by_host_string_mutation() {
    let mut heap = Heap::new();
    let value = heap.allocate(Object::String("abcdefghijkl".into()));
    let item = heap.get_item(value, Value::Int(-1)).unwrap();
    assert_eq!(heap.as_str(&item), Some("l"));
    *heap.get_mut(value).unwrap() = Object::String("é😀abcdefghijkl".into());
    for (index, expected) in [(0, "é"), (1, "😀"), (-1, "l"), (-14, "é")] {
        let item = heap.get_item(value, Value::Int(index)).unwrap();
        assert_eq!(heap.as_str(&item), Some(expected));
    }
    assert!(heap.get_item(value, Value::Int(14)).is_err());
    assert!(heap.get_item(value, Value::Int(-15)).is_err());
    *heap.get_mut(value).unwrap() = Object::String("abcdefghijkl".into());
    let item = heap.get_item(value, Value::Int(0)).unwrap();
    assert_eq!(heap.as_str(&item), Some("a"));
}
