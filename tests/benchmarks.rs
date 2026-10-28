use tis::{Tiering, Vm, compile};

fn run(source: &str, tier: Tiering) -> Vm {
    let mut program = compile(source).unwrap();
    let mut vm = Vm::new();
    vm.tiering = tier;
    vm.thresholds.function_entries = 2;
    vm.thresholds.loop_backedges = 8;
    vm.profiling.window_ticks = 32;
    vm.run_program(&mut program).unwrap();
    vm
}
#[test]
fn barnes_and_ticket_search_run_in_every_tier() {
    let barnes = include_str!("fixtures/barnes_hut.py")
        .replace("DEFAULT_ITERATIONS = 100", "DEFAULT_ITERATIONS = 2")
        .replace("DEFAULT_PARTICLES = 200", "DEFAULT_PARTICLES = 16");
    let tickets =
        include_str!("fixtures/ticket_search.py").replace("TICKETS = 1600", "TICKETS = 32");
    for source in [&barnes, &tickets] {
        let reference = run(source, Tiering::Bytecode).output;
        assert!(!reference.is_empty());
        for tier in [Tiering::Lir, Tiering::Native, Tiering::CopyPatch] {
            let mut vm = run(source, tier);
            assert_eq!(vm.output, reference, "{tier:?}");
            vm.collect_garbage(&[]).unwrap();
        }
    }
}
#[test]
fn math_import_alias_identity_and_domain_errors() {
    let source = "import math as m\nimport math\nassert math is m\nassert math.sqrt(4)==2.0\nassert math.sin(0)==0.0\nassert math.cos(0)==1.0\nassert math.log(1)==0.0\ndef local():\n    import math as n\n    return n is math\nassert local()\n";
    for tier in [Tiering::Bytecode, Tiering::Native, Tiering::CopyPatch] {
        let mut vm = run(source, tier);
        let module = vm.get_global("math").unwrap();
        let mut program = compile("math=None\nm=None").unwrap();
        vm.run_program(&mut program).unwrap();
        vm.collect_garbage(&[]).unwrap();
        let mut program = compile("import math").unwrap();
        vm.run_program(&mut program).unwrap();
        assert_eq!(vm.get_global("math").unwrap(), module);
        for expression in ["math.sqrt(-1)", "math.log(0)"] {
            let mut program = compile(expression).unwrap();
            assert!(
                vm.run_program(&mut program)
                    .unwrap_err()
                    .to_string()
                    .contains("ValueError")
            );
        }
    }
}
#[test]
fn keyed_sort_is_stable_and_roots_callback_results() {
    let source = "calls=[]\ndef key(pair):\n    calls.append(pair[1])\n    for i in range(2000):\n        garbage=[i]\n    return (pair[0],)\nxs=[(2,0),(1,1),(2,2),(1,3)]\nxs.sort(key=key)\nassert xs==[(1,1),(1,3),(2,0),(2,2)]\nassert calls==[0,1,2,3]\nxs.sort(key=lambda p:p[0],reverse=True)\nassert xs==[(2,0),(2,2),(1,1),(1,3)]\n";
    for tier in [Tiering::Bytecode, Tiering::Native, Tiering::CopyPatch] {
        run(source, tier);
    }
}
#[test]
fn sort_errors_restore_detached_items() {
    let mut vm = run(
        "xs=[3,1,2]\ndef bad(x):\n    xs.append(10)\n    return x\n",
        Tiering::Native,
    );
    let mut p = compile("xs.sort(key=bad)").unwrap();
    assert!(
        vm.run_program(&mut p)
            .unwrap_err()
            .to_string()
            .contains("modified during sort")
    );
    let mut p = compile("assert xs==[1,2,3]\nxs.sort(key=lambda x:1/0)").unwrap();
    assert!(
        vm.run_program(&mut p)
            .unwrap_err()
            .to_string()
            .contains("ZeroDivision")
    );
    let mut p = compile("assert xs==[1,2,3]").unwrap();
    vm.run_program(&mut p).unwrap();
}

#[test]
fn native_field_reads_preserve_aliases_method_fallbacks_and_missing_errors() {
    let source = "class A:\n    def __init__(self):\n        self.x=[1]\n    def method(self):\n        return 42\ndef f(a):\n    return a.x\na=A()\nfor i in range(1000):\n    f(a)\n";
    let mut vm = run(source, Tiering::Native);
    let f = vm.get_global("f").unwrap();
    let a = vm.get_global("a").unwrap();
    let before = vm.tier_stats.native_field_hits;
    let value = vm.call(f, &[a]).unwrap();
    assert!(vm.tier_stats.native_field_hits > before);
    vm.collect_garbage(&[]).unwrap();
    assert_eq!(
        vm.heap.get_item(value, tis::Value::Int(0)).unwrap(),
        tis::Value::Int(1)
    );
    let mut p =
        compile("a.x=[2]\nassert f(a)==[2]\nassert a.method()==42\nclass B:\n    pass\nb=B()")
            .unwrap();
    vm.run_program(&mut p).unwrap();
    let b = vm.get_global("b").unwrap();
    assert!(
        vm.call(f, &[b])
            .unwrap_err()
            .to_string()
            .contains("AttributeError")
    );
    assert_eq!(vm.stack_len(), 0);
    assert!(vm.call(f, &[a]).is_ok());
}

#[test]
fn native_field_results_remain_rooted_across_allocating_calls() {
    let source = "class A:\n    def __init__(self,x):\n        self.x=x\ndef churn(a):\n    a.x=None\n    for i in range(6000):\n        garbage=[i]\ndef f(a):\n    saved=a.x\n    churn(a)\n    return saved[0]\nfor i in range(100):\n    assert f(A([123]))==123\n";
    let vm = run(source, Tiering::Native);
    assert!(vm.tier_stats.native_field_hits > 0);
}

#[test]
fn slot_only_calls_keep_recursion_defaults_and_closure_environments_distinct() {
    let source = "marker=7\ndef plain(x=3):\n    y=x+1\n    return y+marker\ndef recursive(n):\n    if n==0:\n        return [1]\n    saved=[n]\n    return [saved,recursive(n-1)]\ndef outer(x):\n    def middle():\n        def inner():\n            return x\n        return inner\n    return middle()\ndef defaults():\n    x=[123]\n    def inner(value=x):\n        return value\n    return inner\na=outer(10)\nb=outer(20)\nd=defaults()\nfor i in range(100):\n    assert plain()==11\n    assert plain(i)==i+8\n    assert a()==10\n    assert b()==20\n    assert d()==[123]\n    tree=recursive(4)\n    assert tree[0]==[4]\n    assert tree[1][0]==[3]\n";
    for tier in [
        Tiering::Bytecode,
        Tiering::Lir,
        Tiering::Native,
        Tiering::CopyPatch,
    ] {
        run(source, tier);
    }
}
