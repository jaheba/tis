use tis::{BytecodeMode, Tiering, Vm, compile};

fn warm(source: &str, mode: BytecodeMode) -> Vm {
    let mut vm = Vm::new();
    vm.tiering = Tiering::Native;
    vm.readonly_regions = true;
    vm.capture_native_code = true;
    vm.bytecode = mode;
    vm.thresholds.function_entries = 16;
    vm.thresholds.loop_backedges = 32;
    vm.run_program(&mut compile(source).unwrap()).unwrap();
    vm
}
const SOURCE: &str = "import math\nOFFSET=1.0\nclass Node:\n    def __init__(self):\n        self.x=4.0\n        self.items=[self]\n    def recur(self,n):\n        if n==0:\n            return math.sqrt(self.x),len(self.items)\n        a,b=self.recur(n-1)\n        return a+OFFSET,b+1\nnode=Node()\nfor i in range(500):\n    node.recur(3)\n";

#[test]
fn native_recursive_region_removes_inner_frames_and_allocations() {
    for mode in [BytecodeMode::Stack, BytecodeMode::Register] {
        let mut vm = warm(SOURCE, mode);
        assert!(vm.tier_stats.readonly_entries > 0, "{:?}", vm.tier_stats);
        vm.run_program(&mut compile("bound=node.recur\n").unwrap())
            .unwrap();
        let bound = vm.get_global("bound").unwrap();
        let tis::Object::Method(function, _) = vm.heap.get(bound).unwrap() else {
            panic!("bound method");
        };
        let diagnostics = vm.native_diagnostics(*function).unwrap();
        let clif = diagnostics.readonly_clif.as_ref().unwrap();
        assert!(clif.contains("call fn0"));
        assert_eq!(
            clif.lines()
                .filter(|line| line.trim_start().starts_with("fn0 ="))
                .count(),
            1
        );
        assert!(!clif.contains("fn1 ="));
        let before = vm.tier_stats.clone();
        let allocations = vm.heap.allocation_count();
        let result = vm.call(bound, &[tis::Value::Int(20)]).unwrap();
        assert_eq!(
            vm.heap.get_item(result, tis::Value::Int(0)).unwrap(),
            tis::Value::Float(22.0)
        );
        assert_eq!(
            vm.heap.get_item(result, tis::Value::Int(1)).unwrap(),
            tis::Value::Int(21)
        );
        assert!(vm.tier_stats.readonly_entries > before.readonly_entries);
        assert_eq!(vm.tier_stats.readonly_misses, before.readonly_misses);
        assert_eq!(
            vm.tier_stats.positional_frames - before.positional_frames,
            1
        );
        assert_eq!(vm.heap.allocation_count() - allocations, 1);
        assert_eq!(vm.tier_stats.compilation_errors, 0);
    }
}

#[test]
fn globals_shapes_rebinding_and_errors_replay_correctly() {
    let mut vm = warm(SOURCE, BytecodeMode::Register);
    for snippet in [
        "OFFSET=2.0\nassert node.recur(3)==(8.0,4)\n",
        "node.extra=1\nassert node.recur(3)==(8.0,4)\n",
        "node.x=9.0\nassert node.recur(3)==(9.0,4)\n",
        "def replacement(x):\n    return 100\nlen=replacement\nassert node.recur(3)==(9.0,103)\n",
        "node.x=-1.0\n",
    ] {
        vm.run_program(&mut compile(snippet).unwrap()).unwrap();
    }
    let error = vm
        .run_program(&mut compile("node.recur(3)\n").unwrap())
        .unwrap_err();
    assert!(error.to_string().contains("math domain error"), "{error}");
    vm.run_program(&mut compile("node.x=4.0\nassert node.recur(3)==(8.0,103)\n").unwrap())
        .unwrap();
    assert_eq!(vm.stack_len(), 0);
}

#[test]
fn writes_and_unknown_calls_are_not_replayed() {
    for source in [
        "class Node:\n    def __init__(self):\n        self.count=0\n    def recur(self,n):\n        self.count+=1\n        if n==0:\n            return 0,0\n        a,b=self.recur(n-1)\n        return a+1,b+1\nnode=Node()\nfor i in range(500):\n    node.recur(3)\nassert node.count==2000\n",
        "events=[]\ndef effect(x):\n    events.append(x)\n    return x\nclass Node:\n    def recur(self,n):\n        if n==0:\n            return effect(n),0\n        a,b=self.recur(n-1)\n        return a+1,b+1\nnode=Node()\nfor i in range(500):\n    node.recur(3)\nassert len(events)==500\n",
    ] {
        let vm = warm(source, BytecodeMode::Register);
        assert_eq!(vm.tier_stats.readonly_entries, 0);
        assert_eq!(vm.tier_stats.compilation_errors, 0);
    }
}

#[test]
fn native_recursion_obeys_language_limit_and_returns_to_vm_after_miss() {
    let mut vm = warm(SOURCE, BytecodeMode::Register);
    let error = vm
        .run_program(&mut compile("node.recur(1000)\n").unwrap())
        .unwrap_err();
    assert!(error.to_string().contains("RecursionError"), "{error}");
    vm.run_program(&mut compile("assert node.recur(5)==(7.0,6)\n").unwrap())
        .unwrap();
    assert_eq!(vm.stack_len(), 0);
}

#[test]
fn old_bound_methods_do_not_recurse_into_stale_code() {
    let mut vm = warm(SOURCE, BytecodeMode::Register);
    vm.run_program(&mut compile("bound=node.recur\ndef replacement(self,n):\n    return 100,100\nNode.recur=replacement\nassert bound(3)==(101.0,101)\n").unwrap()).unwrap();
    assert_eq!(vm.stack_len(), 0);
}

#[test]
fn barnes_region_matches_bytecode_and_survives_collection() {
    let source = include_str!("fixtures/barnes_hut.py")
        .rsplit_once("\nprint(f'")
        .unwrap()
        .0;
    let mut reference = Vm::new();
    reference
        .run_program(&mut compile(source).unwrap())
        .unwrap();
    let function = reference.get_global("bench_quadtree_nbody").unwrap();
    let args = [
        tis::Value::Int(1),
        tis::Value::Int(32),
        tis::Value::Int(8),
        tis::Value::Float(0.5),
    ];
    let expected = reference.call(function, &args).unwrap();
    let mut vm = warm(source, BytecodeMode::Register);
    let function = vm.get_global("bench_quadtree_nbody").unwrap();
    for _ in 0..3 {
        assert_eq!(vm.call(function, &args).unwrap(), expected);
    }
    vm.collect_garbage(&[]).unwrap();
    assert_eq!(vm.call(function, &args).unwrap(), expected);
    assert!(vm.tier_stats.readonly_entries > 0, "{:?}", vm.tier_stats);
    assert_eq!(vm.tier_stats.compilation_errors, 0);
    assert_eq!(vm.stack_len(), 0);
}

#[test]
fn numeric_guards_preserve_integer_tags_overflow_and_float_errors() {
    let source = "class Node:\n    def recur(self,n,x,y):\n        if n==0:\n            return x+y,x*y\n        a,b=self.recur(n-1,x,y)\n        return a/x,b-y\nnode=Node()\nbound=node.recur\nfor i in range(500):\n    bound(2,1.0,2.0)\n";
    let mut reference = Vm::new();
    reference
        .run_program(&mut compile(source).unwrap())
        .unwrap();
    let slow = reference.get_global("bound").unwrap();
    use tis::Value::{Bool, Float, Int};
    for (x, y) in [
        (Int(2), Int(3)),
        (Int(9_007_199_254_740_993), Int(1)),
        (Int(i64::MAX), Int(2)),
        (Float(-0.0), Float(1.0)),
        (Float(f64::NAN), Float(1.0)),
        (Float(f64::INFINITY), Float(2.0)),
        (Bool(true), Int(2)),
    ] {
        let mut native = warm(source, BytecodeMode::Register);
        let fast = native.get_global("bound").unwrap();
        let args = [Int(2), x, y];
        match (native.call(fast, &args), reference.call(slow, &args)) {
            (Ok(a), Ok(b)) => {
                for index in [Int(0), Int(1)] {
                    let a = native.heap.get_item(a, index).unwrap();
                    let b = reference.heap.get_item(b, index).unwrap();
                    match (a, b) {
                        (Float(a), Float(b)) if a.is_nan() && b.is_nan() => {}
                        (a, b) => assert_eq!(a, b),
                    }
                }
            }
            (Err(a), Err(b)) => assert_eq!(a.to_string(), b.to_string()),
            other => panic!("numeric mismatch: {other:?}"),
        }
        assert!(native.tier_stats.readonly_entries > 0);
        assert_eq!(native.stack_len(), 0);
    }
}
