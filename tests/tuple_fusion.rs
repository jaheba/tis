use tis::{BytecodeMode, Tiering, Value, Vm, compile};

fn warm(source: &str, tier: Tiering, mode: BytecodeMode) -> Vm {
    let mut vm = Vm::new();
    vm.tiering = tier;
    vm.cfg_inlining = true;
    vm.bytecode = mode;
    vm.thresholds.function_entries = 16;
    vm.thresholds.loop_backedges = 32;
    vm.run_program(&mut compile(source).unwrap()).unwrap();
    vm
}

#[test]
fn direct_returns_unpack_without_allocating() {
    for tier in [Tiering::Lir, Tiering::Native, Tiering::CopyPatch] {
        for mode in [BytecodeMode::Stack, BytecodeMode::Register] {
            let mut vm = warm(
                "def pair(x):\n    if x<0:\n        return 0,0\n    return x+1,x+2\ndef caller(x):\n    a,b=pair(x)\n    return a+b\nfor i in range(500):\n    caller(i%2-1)\n",
                tier,
                mode,
            );
            let caller = vm.get_global("caller").unwrap();
            let before = vm.heap.allocation_count();
            let hits = vm.tier_stats.virtual_tuple_returns;
            let entries = vm.tier_stats.native_entries + vm.tier_stats.copy_patch_entries;
            assert_eq!(
                vm.call(caller, &[Value::Int(123)]).unwrap(),
                Value::Int(249)
            );
            assert_eq!(
                vm.heap.allocation_count(),
                before,
                "{tier:?} {:?}",
                vm.tier_stats
            );
            assert_eq!(
                vm.tier_stats.virtual_tuple_returns,
                hits + 1,
                "{tier:?} {:?}",
                vm.tier_stats
            );
            assert_eq!(vm.call(caller, &[Value::Int(-1)]).unwrap(), Value::Int(0));
            assert_eq!(vm.tier_stats.compilation_errors, 0);
            if tier != Tiering::Lir {
                assert!(vm.tier_stats.native_entries + vm.tier_stats.copy_patch_entries > entries);
            }
        }
    }
}

#[test]
fn deoptimization_preserves_effects_and_constructs_the_tuple() {
    for tier in [Tiering::Lir, Tiering::Native, Tiering::CopyPatch] {
        let mut vm = warm(
            "class Box:\n    def __init__(self):\n        self.count=0\ndef pair(box,a,b):\n    box.count+=1\n    return a+b,b\ndef caller(box,a,b):\n    x,y=pair(box,a,b)\n    return [x,y]\nbox=Box()\nfor i in range(500):\n    caller(box,1,2)\n",
            tier,
            BytecodeMode::Register,
        );
        let caller = vm.get_global("caller").unwrap();
        let boxed = vm.get_global("box").unwrap();
        let a = vm
            .heap
            .allocate(tis::Object::List(vec![Value::Int(4)].into()));
        let b = vm
            .heap
            .allocate(tis::Object::List(vec![Value::Int(5)].into()));
        let before = vm.tier_stats.deoptimizations;
        let result = vm.call(caller, &[boxed, a, b]).unwrap();
        assert!(vm.tier_stats.deoptimizations > before);
        let combined = vm.heap.get_item(result, Value::Int(0)).unwrap();
        assert_eq!(vm.heap.len(combined).unwrap(), 2);
        assert_eq!(vm.heap.get_item(result, Value::Int(1)).unwrap(), b);
        vm.run_program(&mut compile("assert box.count==501\n").unwrap())
            .unwrap();
        assert_eq!(vm.stack_len(), 0);
    }
}

#[test]
fn escaping_and_mixed_returns_keep_their_objects() {
    for source in [
        "def pair(x):\n    return x,x\ndef caller(x):\n    p=pair(x)\n    a,b=p\n    return p\nfor i in range(500):\n    caller(i)\n",
        "def pair(x):\n    if x<0:\n        return [x,x]\n    return x,x\ndef caller(x):\n    a,b=pair(x)\n    return a+b\nfor i in range(500):\n    caller(i%2-1)\n",
    ] {
        let vm = warm(source, Tiering::Native, BytecodeMode::Register);
        assert_eq!(vm.tier_stats.virtual_tuple_returns, 0);
    }
}

#[test]
fn scalarized_returns_preserve_aliases_and_rebound_callables() {
    for tier in [Tiering::Lir, Tiering::Native, Tiering::CopyPatch] {
        let mut vm = warm(
            "def pair(x):\n    return x,x\ndef other(x):\n    return x,[0]\ndef caller(x):\n    a,b=pair(x)\n    return a is b\nfor i in range(500):\n    caller(i)\n",
            tier,
            BytecodeMode::Register,
        );
        let caller = vm.get_global("caller").unwrap();
        let value = vm
            .heap
            .allocate(tis::Object::List(vec![Value::Int(1)].into()));
        assert_eq!(vm.call(caller, &[value]).unwrap(), Value::Bool(true));
        vm.run_program(&mut compile("pair=other\n").unwrap())
            .unwrap();
        assert_eq!(vm.call(caller, &[value]).unwrap(), Value::Bool(false));
        assert_eq!(vm.stack_len(), 0);
    }
}

#[test]
fn wrong_arity_still_raises_after_the_call_effect() {
    let mut vm = warm(
        "class Box:\n    def __init__(self):\n        self.count=0\ndef pair(box):\n    box.count+=1\n    return 1,2\ndef caller(box):\n    a,b,c=pair(box)\nbox=Box()\n",
        Tiering::Native,
        BytecodeMode::Register,
    );
    let caller = vm.get_global("caller").unwrap();
    let boxed = vm.get_global("box").unwrap();
    for _ in 0..100 {
        assert!(
            vm.call(caller, &[boxed])
                .unwrap_err()
                .to_string()
                .contains("ValueError")
        );
    }
    assert_eq!(vm.tier_stats.virtual_tuple_returns, 0);
    vm.run_program(&mut compile("assert box.count==100\n").unwrap())
        .unwrap();
}
