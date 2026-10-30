use tis::{Object, Tiering, Value, Vm, compile};

const PYTHON: &str = include_str!("../experiments/json/codec.py");

fn run(tier: Tiering, source: &str) -> Vm {
    let mut vm = Vm::new();
    vm.tiering = tier;
    vm.thresholds.function_entries = 2;
    vm.thresholds.loop_backedges = 4;
    vm.run_program(&mut compile(source).unwrap()).unwrap();
    vm
}

#[test]
fn both_codecs_roundtrip_in_every_tier() {
    let exercise = r#"
import json
text = '{"name":"hello\\nworld","values":[null,true,false,-42,1.25,1e2],"unicode":"\\u00e9\\ud83d\\ude00"}'
for i in range(30):
    native = json.loads(text)
    python = json_loads(text)
    assert native == python
    assert json.loads(json_dumps(python)) == native
    assert json_loads(json.dumps(native)) == python
assert json_loads('"\\u0000"') == chr(0)
assert json.loads('9223372036854775807') == 9223372036854775807
assert json_loads('-9223372036854775808') == -9223372036854775808
assert json.loads('9223372036854775808') == 9223372036854775808
assert json_loads('18446744073709551615') == 18446744073709551615
assert json_loads('{"x":1,"x":2}') == {'x': 2}
assert json_loads(' [ 1, 2 ] \n') == [1, 2]
shared = [1]
assert json_loads(json_dumps([shared, shared])) == [[1], [1]]
assert json.loads(json.dumps([shared, shared])) == [[1], [1]]
result = native
"#;
    for tier in [
        Tiering::Bytecode,
        Tiering::Lir,
        Tiering::Native,
        Tiering::CopyPatch,
    ] {
        let mut vm = run(tier, &format!("{PYTHON}\n{exercise}"));
        let result = vm.get_global("result").unwrap();
        vm.collect_garbage(&[result]).unwrap();
        assert!(matches!(vm.heap.get(result).unwrap(), Object::Dict(_)));
        assert_eq!(vm.tier_stats.compilation_errors, 0);
        if tier == Tiering::Native {
            assert!(vm.tier_stats.native_entries > 0);
        }
        if tier == Tiering::CopyPatch {
            assert!(vm.tier_stats.copy_patch_entries > 0);
        }
        let parser = vm.get_global("json_loads").unwrap();
        assert!(vm.function_hir(parser).unwrap().profile().entries > 0);
    }
}

#[test]
fn both_decoders_reject_invalid_input() {
    for input in [
        "",
        "01",
        "[1,]",
        "{\"x\":}",
        "true false",
        "1.",
        "1e",
        "1e999",
        "\"\\ud800\"",
        "\"\n\"",
        "NaN",
    ] {
        for python in [false, true] {
            let source = format!(
                "{PYTHON}\nimport json\ndef parse(s):\n    return {}(s)\n",
                if python { "json_loads" } else { "json.loads" }
            );
            let mut vm = run(Tiering::Bytecode, &source);
            let parse = vm.get_global("parse").unwrap();
            let input = vm.heap.allocate(Object::String(input.into()));
            assert!(
                vm.call(parse, &[input]).is_err(),
                "python={python}, {input:?}"
            );
        }
    }
}

#[test]
fn both_encoders_reject_cycles_nonfinite_and_nonstring_keys() {
    for python in [false, true] {
        let source = format!(
            "{PYTHON}\nimport json\ndef encode(value):\n    return {}(value)\ncycle = []\ncycle.append(cycle)\n",
            if python { "json_dumps" } else { "json.dumps" }
        );
        let mut vm = run(Tiering::Bytecode, &source);
        let encode = vm.get_global("encode").unwrap();
        let cycle = vm.get_global("cycle").unwrap();
        for value in [cycle, Value::Float(f64::INFINITY), Value::Float(f64::NAN)] {
            assert!(vm.call(encode, &[value]).is_err());
        }
        let dict = vm
            .heap
            .allocate(Object::Dict(vec![(tis::Key::Int(1), Value::None)].into()));
        assert!(vm.call(encode, &[dict]).is_err());
    }
}

#[test]
fn dumps_default_spacing_unicode_and_sorted_keys_match_python() {
    let source = r#"
import json
assert json.dumps({'z': [1,True,None], 'a': 'é😀'}) == '{"z": [1, true, null], "a": "\\u00e9\\ud83d\\ude00"}'
assert json.dumps({'z': 1, 'a': 2}, sort_keys=True) == '{"a": 2, "z": 1}'
assert json.dumps('é😀', ensure_ascii=False) == '"é😀"'
assert json.dumps('\n\r\t\b\f' + chr(0) + '"' + chr(92)) == '"\\n\\r\\t\\b\\f\\u0000\\"\\\\"'
assert json.loads(json.dumps({'é': {'z': 1, 'a': 2}}, sort_keys=True)) == {'é': {'a': 2, 'z': 1}}
"#;
    for tier in [
        Tiering::Bytecode,
        Tiering::Lir,
        Tiering::Native,
        Tiering::CopyPatch,
    ] {
        run(tier, source);
    }
}

#[test]
fn streaming_parser_keeps_key_order_boundaries_and_releases_partial_results() {
    let source = r#"
import json
assert json.loads('{"x":1,"y":2,"x":3}') == {'x':3,'y':2}
assert list(json.loads('{"x":1,"y":2,"x":3}')) == ['x','y']
assert json.loads('{"$serde_json::private::Number":"123"}') == {'$serde_json::private::Number':'123'}
assert json.loads('"\\ud83d\\ude00"') == '😀'
assert json.loads('-0') == 0
assert json.loads('1e-999') == 0.0
assert json.loads('[1e2,-1.5,0.0]') == [100.0,-1.5,0.0]
"#;
    let mut vm = run(Tiering::Bytecode, source);
    vm.run_program(&mut compile("def parse(text):\n    return json.loads(text)").unwrap())
        .unwrap();
    let parse = vm.get_global("parse").unwrap();
    vm.collect_garbage(&[]).unwrap();
    let baseline = vm.heap.allocation_count();
    for text in [
        "[{}, {},",
        "{\"x\": [1,2,",
        "[1] trailing",
        "[1,]",
        "{\"x\":1,}",
        "Infinity",
        "-Infinity",
    ] {
        let value = vm.heap.string(text);
        assert!(vm.call(parse, &[value]).is_err(), "{text}");
        vm.collect_garbage(&[]).unwrap();
        assert!(vm.stack_len() == 0);
        assert_eq!(vm.heap.allocation_count(), baseline);
    }
    let nested = format!("{}0{}", "[".repeat(65), "]".repeat(65));
    let value = vm.heap.string(&nested);
    assert!(vm.call(parse, &[value]).is_err());
}

#[test]
fn dictionary_keys_own_text_after_source_collection() {
    for tier in [
        Tiering::Bytecode,
        Tiering::Lir,
        Tiering::Native,
        Tiering::CopyPatch,
    ] {
        let mut vm = run(
            tier,
            r#"
import json
keys = ['id', 'é\x00😀', 'abcdefghijklmnopqrstuvw', 'abcdefghijklmnopqrstuvwx', 'abcdefghijklmnopqrstuvwxy']
result = {}
for i in range(len(keys)):
    result[keys[i]] = i
assert json.loads(json.dumps(result)) == result
def f(**kwargs):
    return kwargs
assert f(**{'é': 3}) == {'é': 3}
keys = None
"#,
        );
        let result = vm.get_global("result").unwrap();
        vm.collect_garbage(&[result]).unwrap();
        let Object::Dict(items) = vm.heap.get(result).unwrap() else {
            panic!("dictionary");
        };
        assert_eq!(items.get_str("é\0😀"), Some(&Value::Int(1)));
        assert_eq!(
            items.get_str("abcdefghijklmnopqrstuvwxy"),
            Some(&Value::Int(4))
        );
    }
}
