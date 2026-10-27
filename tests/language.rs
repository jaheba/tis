use tis::{Object, Value, Vm, compile};

fn run(source: &str) -> (Vm, tis::Result<Value>) {
    let mut program = compile(source).unwrap();
    let mut vm = Vm::new();
    let result = vm.run_program(&mut program);
    (vm, result)
}

#[test]
fn python_core_semantics() {
    let (vm, result) = run(include_str!("fixtures/language.py"));
    result.unwrap();
    assert_eq!(vm.output, "language core ok\n");
}

#[test]
fn game_nbody_full_interpreter_acceptance() {
    let (mut vm, result) = run(include_str!("fixtures/game_nbody.py"));
    result.unwrap();
    assert_eq!(vm.output, "-0.169075164\n-0.169089263\n");
    vm.collect_garbage(&[]).unwrap();
    assert!(
        vm.heap.allocation_count() < 300,
        "benchmark temporaries must be collectible"
    );
    assert!(
        vm.heap.slot_count() < 10_000,
        "automatic collection must bound arena growth"
    );
}

#[test]
fn recursive_calls_and_feedback_share_the_executing_ir() {
    let (mut vm, result) =
        run("def add(a, b):\n    return a + b\nfor i in range(20):\n    add(i, 1)\n");
    result.unwrap();
    let function = vm.get_global("add").unwrap();
    assert!(vm.function_ir(function).unwrap().dump().contains("add.int"));
    assert_eq!(
        vm.call(function, &[Value::Float(1.5), Value::Int(2)])
            .unwrap(),
        Value::Float(3.5)
    );
    assert!(!vm.function_ir(function).unwrap().dump().contains("add.int"));
}

#[test]
fn closures_and_defaults_survive_automatic_collection() {
    let source = "def outer():\n    captured = [40]\n    def inner(x=[2]):\n        return captured[0] + x[0]\n    return inner\nf = outer()\nfor i in range(10000):\n    temporary = [[i], [i + 1]]\nassert f() == 42\n";
    let (mut vm, result) = run(source);
    result.unwrap();
    let function = vm.get_global("f").unwrap();
    vm.collect_garbage(&[]).unwrap();
    assert_eq!(vm.call(function, &[]).unwrap(), Value::Int(42));
    assert_eq!(vm.stack_len(), 0);
}

#[test]
fn class_bases_and_ephemeral_callables_are_roots_during_nested_execution() {
    let source = "def churn():\n    for i in range(5000):\n        x = [i]\n    return 42\ndef base():\n    class A:\n        value = 9\n    return A\nclass B(base()):\n    churn()\nassert B().value == 9\nassert (lambda x=[8]: churn() + x[0])() == 50\n";
    let (_, result) = run(source);
    result.unwrap();
}

#[test]
fn host_handles_can_be_pinned_across_collections() {
    let mut vm = Vm::new();
    let value = vm.heap.allocate(Object::List(vec![Value::Int(42)]));
    let root = vm.pin(value).unwrap();
    let mut program = compile("for i in range(10000):\n    temporary = [i]\n").unwrap();
    vm.run_program(&mut program).unwrap();
    vm.collect_garbage(&[]).unwrap();
    assert!(vm.heap.get(value).is_ok());
    assert_eq!(vm.unpin(root), Some(value));
    vm.collect_garbage(&[]).unwrap();
    assert!(vm.heap.get(value).is_err());
}

#[test]
fn runtime_errors_clean_up_nested_frames() {
    for source in [
        "def f(x):\n    if x:\n        y = 1\n    return y\nf(False)\n",
        "def f():\n    return absent\nf()\n",
        "def f():\n    return f()\nf()\n",
        "def f(a):\n    return a\nf(1, a=2)\n",
        "def f(a, /):\n    return a\nf(a=2)\n",
        "def f(*, a):\n    return a\nf()\n",
        "def f():\n    x = 1\nf(1)\n",
        "a, b = [1]\n",
        "1 / 0\n",
        "range(1, 3, 0)\n",
        "[1][::0]\n",
        "d = {1: 2}\nfor key in d:\n    d[3] = 4\n",
    ] {
        let (mut vm, result) = run(source);
        assert!(result.is_err(), "{source}");
        assert_eq!(vm.stack_len(), 0, "{source}");
        vm.collect_garbage(&[]).unwrap();
        let mut program = compile("42").unwrap();
        assert_eq!(vm.run_program(&mut program).unwrap(), Value::Int(42));
    }
}

#[test]
fn enumeration_and_zip_are_lazy_and_keep_their_inputs_live() {
    let source = "a = [1]\ne = enumerate(a, 2)\na.append(3)\nassert next(e) == (2, 1)\nassert next(e) == (3, 3)\nassert next(e, None) is None\na = [1]\nz = zip(a, a)\na.append(2)\nassert list(z) == [(1, 1), (2, 2)]\n";
    let (_, result) = run(source);
    result.unwrap();
}
