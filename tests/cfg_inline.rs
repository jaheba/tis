use tis::{BytecodeMode, Tiering, Value, Vm, compile, lir::LoweringMode};

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
fn branching_methods_merge_returns_and_preserve_effects_without_callee_frames() {
    for tier in [Tiering::Lir, Tiering::Native, Tiering::CopyPatch] {
        for mode in [BytecodeMode::Stack, BytecodeMode::Register] {
            let mut vm = warm(
                "class Box:\n    def __init__(self):\n        self.count=0\n    def choose(self,x):\n        if x<0:\n            self.count+=1\n            return [x]\n        self.count+=2\n        return (x,)\ndef caller(b,x):\n    return b.choose(x)\nb=Box()\nfor i in range(500):\n    caller(b,i%2-1)\nassert b.count==750\n",
                tier,
                mode,
            );
            let caller = vm.get_global("caller").unwrap();
            let b = vm.get_global("b").unwrap();
            let before = vm.tier_stats.clone();
            let value = vm.call(caller, &[b, Value::Int(-2)]).unwrap();
            assert_eq!(
                vm.heap.get_item(value, Value::Int(0)).unwrap(),
                Value::Int(-2)
            );
            assert_eq!(
                vm.tier_stats.positional_frames - before.positional_frames,
                1,
                "{tier:?} {:?}",
                vm.tier_stats
            );
            assert!(vm.tier_stats.leaf_body_inlines > before.leaf_body_inlines);
            let hir = vm.function_hir(caller).unwrap();
            let lir = tis::lir::Function::lower_with_cfg(&hir, LoweringMode::Profiled).unwrap();
            assert!(lir.blocks.len() > lir.block_origins.iter().flatten().count());
            assert!(lir.block_origins.iter().any(Option::is_none));
            vm.run_program(&mut compile("assert b.count==751\n").unwrap())
                .unwrap();
            assert_eq!(vm.stack_len(), 0);
        }
    }
}

#[test]
fn branch_phi_values_survive_scoped_calls_and_collection() {
    for tier in [Tiering::Lir, Tiering::Native, Tiering::CopyPatch] {
        let mut vm = warm(
            "def allocate(x):\n    return [x]\ndef choose(x):\n    if x<0:\n        value=[x]\n    else:\n        value=(x,)\n    extra=allocate(x+1)\n    return [value,extra]\ndef caller(x):\n    return choose(x)\nfor i in range(500):\n    caller(i%2-1)\n",
            tier,
            BytecodeMode::Register,
        );

        let caller = vm.get_global("caller").unwrap();
        let hits = vm.tier_stats.leaf_body_inlines;
        for i in -3000..3000 {
            let value = vm.call(caller, &[Value::Int(i)]).unwrap();
            let first = vm.heap.get_item(value, Value::Int(0)).unwrap();
            let second = vm.heap.get_item(value, Value::Int(1)).unwrap();
            assert_eq!(
                vm.heap.get_item(first, Value::Int(0)).unwrap(),
                Value::Int(i)
            );
            assert_eq!(
                vm.heap.get_item(second, Value::Int(0)).unwrap(),
                Value::Int(i + 1)
            );
        }
        assert!(
            vm.tier_stats.leaf_body_inlines > hits,
            "{tier:?} {:?}",
            vm.tier_stats
        );
        assert_eq!(vm.stack_len(), 0);
    }
}

#[test]
fn nested_calls_use_the_actual_closure_environment_after_rebinding() {
    for tier in [Tiering::Lir, Tiering::Native, Tiering::CopyPatch] {
        let mut vm = warm(
            "def make(offset):\n    def choose(x):\n        if x<0:\n            return [offset-x]\n        return [offset+x]\n    return choose\nleaf=make(10)\nother=make(100)\ndef caller(x):\n    return leaf(x)\nfor i in range(500):\n    caller(i%2-1)\n",
            tier,
            BytecodeMode::Register,
        );
        let caller = vm.get_global("caller").unwrap();
        let before = vm.tier_stats.leaf_body_inlines;
        let value = vm.call(caller, &[Value::Int(2)]).unwrap();
        assert_eq!(
            vm.heap.get_item(value, Value::Int(0)).unwrap(),
            Value::Int(12)
        );
        vm.run_program(&mut compile("leaf=other\n").unwrap())
            .unwrap();
        let value = vm.call(caller, &[Value::Int(2)]).unwrap();
        assert_eq!(
            vm.heap.get_item(value, Value::Int(0)).unwrap(),
            Value::Int(102)
        );
        assert!(vm.tier_stats.leaf_body_inlines > before);
        assert_eq!(vm.stack_len(), 0);
    }
}

#[test]
fn an_exit_in_a_branch_preserves_the_write_and_both_return_continuations() {
    for tier in [Tiering::Lir, Tiering::Native, Tiering::CopyPatch] {
        let mut vm = warm(
            "class Box:\n    def __init__(self):\n        self.count=0\ndef choose(box,flag,a,b):\n    box.count+=1\n    if flag:\n        value=a+b\n    else:\n        value=a-b\n    return value\ndef caller(box,flag,a,b):\n    keep=[box]\n    result=choose(box,flag,a,b)\n    return [keep,result]\nbox=Box()\nfor i in range(500):\n    caller(box,i%2==0,1,2)\n",
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
        let exits = vm.tier_stats.deoptimizations;
        let value = vm.call(caller, &[boxed, Value::Bool(true), a, b]).unwrap();
        assert!(
            vm.tier_stats.deoptimizations > exits,
            "{tier:?} {:?}",
            vm.tier_stats
        );
        let result = vm.heap.get_item(value, Value::Int(1)).unwrap();
        assert_eq!(vm.heap.len(result).unwrap(), 2);
        vm.run_program(&mut compile("assert box.count==501\n").unwrap())
            .unwrap();
        assert_eq!(vm.stack_len(), 0);
    }
}

#[test]
fn bounded_recursive_incorporation_preserves_the_language_recursion_limit() {
    let source = "class Box:\n    def __init__(self):\n        self.count=0\ndef descend(box,n):\n    box.count+=1\n    if n==0:\n        return [box.count]\n    return descend(box,n-1)\nbox=Box()\nfor i in range(500):\n    descend(box,2)\n";
    let mut expected = None;
    for tier in [
        Tiering::Bytecode,
        Tiering::Lir,
        Tiering::Native,
        Tiering::CopyPatch,
    ] {
        let mut vm = warm(source, tier, BytecodeMode::Register);
        let function = vm.get_global("descend").unwrap();
        let boxed = vm.get_global("box").unwrap();
        let before = vm.tier_stats.leaf_body_inlines;
        vm.call(function, &[boxed, Value::Int(12)]).unwrap();
        if tier != Tiering::Bytecode {
            assert!(
                vm.tier_stats.leaf_body_inlines > before,
                "{tier:?} {:?}",
                vm.tier_stats
            );
        }
        vm.run_program(&mut compile("box.count=0\n").unwrap())
            .unwrap();
        let error = vm.call(function, &[boxed, Value::Int(1000)]).unwrap_err();
        assert!(error.to_string().contains("RecursionError"), "{error}");
        vm.run_program(&mut compile("result=box.count\n").unwrap())
            .unwrap();
        let actual = vm.get_global("result").unwrap();
        if let Some(expected) = expected {
            assert_eq!(actual, expected, "{tier:?}");
        } else {
            expected = Some(actual);
        }
        assert_eq!(vm.stack_len(), 0);
    }
}

#[test]
fn scoped_scalar_probes_fall_back_to_replaced_python_callables_once() {
    for tier in [Tiering::Lir, Tiering::Native, Tiering::CopyPatch] {
        let mut vm = warm(
            "import math\nsqrtfn=math.sqrt\nevents=[]\nclass Box:\n    def __init__(self):\n        self.count=0\ndef changed(x):\n    events.append(x)\n    return x+100.0\ndef choose(box,x):\n    box.count+=1\n    if x==0:\n        return [x]\n    return [sqrtfn(x)+1.0]\ndef caller(box,x):\n    return choose(box,x)\nbox=Box()\nfor i in range(500):\n    caller(box,4.0)\n",
            tier,
            BytecodeMode::Register,
        );
        let caller = vm.get_global("caller").unwrap();
        let boxed = vm.get_global("box").unwrap();
        let hir = vm.function_hir(caller).unwrap();
        let lir = tis::lir::Function::lower_with_cfg(&hir, LoweringMode::Profiled).unwrap();
        assert!(lir.blocks.iter().flat_map(|b|&b.instructions).any(|i| matches!(&i.operation,tis::lir::Operation::Call{helper:tis::lir::Helper::Scoped(inner),..} if matches!(inner.as_ref(),tis::lir::Helper::ScalarCall(_)))));
        let value = vm.call(caller, &[boxed, Value::Float(4.0)]).unwrap();
        assert_eq!(
            vm.heap.get_item(value, Value::Int(0)).unwrap(),
            Value::Float(3.0)
        );
        let choose = vm.get_global("choose").unwrap();
        let errors = vm.function_hir(choose).unwrap().profile().errors;
        assert!(
            vm.call(caller, &[boxed, Value::Float(-4.0)])
                .unwrap_err()
                .to_string()
                .contains("math domain error")
        );
        assert_eq!(
            vm.function_hir(choose).unwrap().profile().errors,
            errors + 1
        );
        vm.run_program(&mut compile("sqrtfn=changed\n").unwrap())
            .unwrap();
        let value = vm.call(caller, &[boxed, Value::Float(4.0)]).unwrap();
        assert_eq!(
            vm.heap.get_item(value, Value::Int(0)).unwrap(),
            Value::Float(105.0)
        );
        vm.run_program(&mut compile("assert events==[4.0]\nassert box.count==503\n").unwrap())
            .unwrap();
        assert_eq!(vm.stack_len(), 0);
    }
}

#[test]
fn cfg_inlining_is_explicit_and_context_metadata_is_verified() {
    let source = "def choose(x):\n    if x<0:\n        return [x]\n    return (x,)\ndef caller(x):\n    return choose(x)\nfor i in range(500):\n    caller(i%2-1)\n";
    let mut ordinary = Vm::new();
    ordinary.tiering = Tiering::Native;
    ordinary.thresholds.function_entries = 16;
    ordinary.thresholds.loop_backedges = 32;
    ordinary.run_program(&mut compile(source).unwrap()).unwrap();
    assert_eq!(ordinary.tier_stats.leaf_body_inlines, 0);
    let vm = warm(source, Tiering::Native, BytecodeMode::Register);
    let hir = vm.function_hir(vm.get_global("caller").unwrap()).unwrap();
    let lir = tis::lir::Function::lower_with_cfg(&hir, LoweringMode::Profiled).unwrap();
    let mut broken = lir.clone();
    *broken
        .block_origins
        .iter_mut()
        .find(|origin| origin.is_none())
        .unwrap() = Some(tis::hir::BlockId(0));
    assert!(broken.verify().is_err());
    let mut broken = lir.clone();
    let snapshot = broken
        .states
        .iter_mut()
        .find_map(|state| match &mut state.environment_root {
            tis::lir::FrameEnvironment::Inline(state) => Some(state),
            _ => None,
        })
        .unwrap();
    std::rc::Rc::make_mut(snapshot).snapshot =
        std::rc::Rc::new(tis::inline_frames::Snapshot { frames: vec![] });
    assert!(broken.verify().is_err());
}
