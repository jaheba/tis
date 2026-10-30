use tis::{Tiering, Vm, compile};

#[test]
fn proven_list_appends_keep_phi_alias_and_collection_semantics() {
    let source = r#"
def build(n):
    result = [1.0, 2.0]
    alias = result
    for i in range(n):
        if i % 2:
            result = []
        result.append([i])
    result.append(result)
    return result, alias
for i in range(20):
    result, alias = build(4)
    assert result[0] == [3]
    assert result[1] is result
    assert alias == [1.0, 2.0, [0]]
def churn(value):
    result = [1.0]
    result.append(value)
    for i in range(5000):
        garbage = [i]
    return result[1]
assert churn(['a heap allocated string']) == ['a heap allocated string']
"#;
    let mut program = compile(source).unwrap();
    assert!(format!("{:?}", program.function("build").unwrap()).contains("ListAppend"));
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
        vm.run_program(&mut program).unwrap();
        assert_eq!(vm.stack_len(), 0);
        assert_eq!(vm.tier_stats.compilation_errors, 0);
    }
}

#[test]
fn uncertain_receivers_keep_dynamic_method_lookup_before_argument_effects() {
    let source = r#"
events = []
class Box:
    def append(self, value):
        events.append(value + 10)
def mixed(flag):
    value = []
    if flag:
        value = Box()
    value.append(1)
    return value
assert mixed(False) == [1]
mixed(True)
assert events == [11]
box = Box()
def change():
    box.append = lambda value: events.append(99)
    return 2
def dynamic(value):
    value.append(change())
dynamic(box)
assert events == [11,12]
"#;
    let mut program = compile(source).unwrap();
    for name in ["mixed", "dynamic"] {
        assert!(!format!("{:?}", program.function(name).unwrap()).contains("ListAppend"));
    }
    for tier in [
        Tiering::Bytecode,
        Tiering::Lir,
        Tiering::Native,
        Tiering::CopyPatch,
    ] {
        let mut vm = Vm::new();
        vm.tiering = tier;
        vm.run_program(&mut program).unwrap();
        assert_eq!(vm.stack_len(), 0);
    }
}
