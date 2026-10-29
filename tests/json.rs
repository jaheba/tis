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
        "9223372036854775808",
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
