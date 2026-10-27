#[path = "../examples/support/mod.rs"]
mod support;

use tis::{
    Value, Vm,
    ir::{Constant, Function, Op, Terminator},
};

#[test]
fn dot_preserves_branch_backedge_and_parameter_transfers() {
    let dot = support::sum().to_dot();
    assert!(dot.starts_with("digraph ir {"));
    assert!(dot.contains("b0 -> b1 [label=\"jump: %0 → %3, %1 → %4\"]"));
    assert!(dot.contains("b1 -> b2 [label=\"true\"]"));
    assert!(dot.contains("b1 -> b3 [label=\"false: %4 → %5\"]"));
    assert!(dot.contains("b2 -> b1 [label=\"jump: %8 → %3, %7 → %4\"]"));
    assert!(dot.contains("b3(%5):\\l"));
}

#[test]
fn text_and_graph_views_report_warmup_and_guard_failure() {
    let mut function = support::addition();
    assert!(
        function
            .dump()
            .contains("executions=0 fast_paths=0 guard_failures=0")
    );
    let mut vm = Vm::new();
    for _ in 0..10 {
        vm.run(&mut function, &[Value::Int(1), Value::Int(2)])
            .unwrap();
    }
    assert!(function.dump().contains("add.int [fallback=add]"));
    assert!(
        function
            .dump()
            .contains("executions=10 fast_paths=2 guard_failures=0")
    );
    assert!(function.to_dot().contains("color=\"#16836c\""));
    assert!(function.to_html().contains("\"strategy\":\"Integer\""));

    vm.run(&mut function, &[Value::Float(1.0), Value::Int(2)])
        .unwrap();
    assert!(!function.dump().contains("add.int"));
    assert!(
        function
            .dump()
            .contains("executions=11 fast_paths=2 guard_failures=1")
    );
    assert!(function.to_html().contains("\"guard_failures\":\"1\""));
}

#[test]
fn html_and_dot_escape_instruction_text_without_executing_it() {
    let mut function = Function::new(0);
    let entry = function.entry();
    let value = function.append(
        entry,
        Op::Constant(Constant::String(
            "\"\\\n</script><img src=x onerror=alert(1)>\u{2028}".into(),
        )),
    );
    function.terminate(entry, Terminator::Return(value));
    let html = function.to_html();
    assert_eq!(html.matches("</script>").count(), 2);
    assert!(!html.contains("<img"));
    assert!(html.contains("\\u003c/script\\u003e"));
    assert!(!html.contains("__TIS_IR_DATA__"));
    assert!(!html.contains("__TIS_IR_SCRIPT__"));
    assert!(!html.contains("__TIS_IR_STYLE__"));
    assert!(function.to_dot().contains("\\\""));
    assert!(function.to_dot().contains("\\\\"));
}

#[test]
fn unfinished_and_unreachable_blocks_remain_visible() {
    let mut function = Function::new(0);
    let unreachable = function.block(1);
    let value = function.parameters(unreachable)[0];
    function.terminate(unreachable, Terminator::Return(value));
    assert!(function.dump().contains("<unterminated>"));
    assert!(function.to_dot().contains("b1(%0)"));
    assert!(function.to_html().contains("\"terminated\":false"));
}

#[test]
fn effectful_operations_expose_all_their_input_values() {
    let mut function = support::addition();
    let entry = function.entry();
    let a = function.parameters(entry)[0];
    let b = function.parameters(entry)[1];
    let value = function.append(entry, Op::Constant(Constant::None));
    assert_eq!(
        Op::SetItem {
            object: a,
            key: b,
            value
        }
        .inputs(),
        vec![a, b, value]
    );
    assert_eq!(
        Op::Dict(vec![(a, b), (b, value)]).inputs(),
        vec![a, b, b, value]
    );
}
