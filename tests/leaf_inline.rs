use tis::{BytecodeMode, Tiering, Value, Vm, compile};

fn run(source: &str, tier: Tiering, mode: BytecodeMode) -> Vm {
    let mut vm = Vm::new();
    vm.tiering = tier;
    vm.bytecode = mode;
    vm.thresholds.function_entries = 16;
    vm.thresholds.loop_backedges = 32;
    vm.run_program(&mut compile(source).unwrap()).unwrap();
    vm
}

#[test]
fn effectful_leaf_calls_share_the_caller_frame() {
    for tier in [Tiering::Lir, Tiering::Native, Tiering::CopyPatch] {
        for mode in [BytecodeMode::Stack, BytecodeMode::Register] {
            let mut vm = run(
                "class Box:\n    def __init__(self):\n        self.x=0\n    def add(self,x):\n        self.x=self.x+x\n        return self.x\ndef caller(b,x):\n    return b.add(x)\nb=Box()\nfor i in range(500):\n    caller(b,1)\nassert b.x==500\n",
                tier,
                mode,
            );
            let frames = vm.tier_stats.positional_frames;
            let hits = vm.tier_stats.leaf_body_inlines;
            let batches = vm.tier_stats.native_leaf_entry_batches;
            let fields = vm.tier_stats.native_inline_field_hits;
            let caller = vm.get_global("caller").unwrap();
            let b = vm.get_global("b").unwrap();
            assert_eq!(
                vm.call(caller, &[b, Value::Int(2)]).unwrap(),
                Value::Int(502)
            );
            assert!(
                vm.tier_stats.leaf_body_inlines > hits,
                "{tier:?} {:?}",
                vm.tier_stats
            );
            assert_eq!(vm.tier_stats.positional_frames - frames, 1);
            if matches!(tier, Tiering::Native | Tiering::CopyPatch) {
                assert_eq!(vm.tier_stats.native_leaf_entry_batches - batches, 1);
            }
            if tier == Tiering::Native {
                assert!(vm.tier_stats.native_inline_field_hits > fields);
            }
            assert_eq!(vm.stack_len(), 0);
        }
    }
}

#[test]
fn a_guard_after_a_write_resumes_inside_the_callee() {
    for tier in [Tiering::Lir, Tiering::Native, Tiering::CopyPatch] {
        for mode in [BytecodeMode::Stack, BytecodeMode::Register] {
            let mut vm = run(
                "class Box:\n    def __init__(self):\n        self.count=0\n    def work(self,a,b):\n        self.count=self.count+1\n        return a+b\ndef caller(box,a,b):\n    return box.work(a,b)\nbox=Box()\nfor i in range(500):\n    assert caller(box,1,2)==3\nassert box.count==500\n",
                tier,
                mode,
            );
            let caller = vm.get_global("caller").unwrap();
            let boxed = vm.get_global("box").unwrap();
            let a = vm
                .heap
                .allocate(tis::Object::List(vec![Value::Int(4)].into()));
            let b = vm
                .heap
                .allocate(tis::Object::List(vec![Value::Int(5)].into()));
            let deopts = vm.tier_stats.deoptimizations;
            let value = vm.call(caller, &[boxed, a, b]).unwrap();
            assert!(
                vm.tier_stats.deoptimizations > deopts,
                "{tier:?} {:?}",
                vm.tier_stats
            );
            assert_eq!(vm.heap.len(value).unwrap(), 2);
            vm.run_program(&mut compile("assert box.count==501\n").unwrap())
                .unwrap();
            assert_eq!(vm.stack_len(), 0);
        }
    }
}

#[test]
fn errors_after_completed_effects_do_not_replay_them() {
    for tier in [Tiering::Lir, Tiering::Native, Tiering::CopyPatch] {
        let mut vm = run(
            "class Box:\n    def __init__(self):\n        self.count=0\n    def work(self,a,b):\n        self.count=self.count+1\n        return a+b\ndef caller(box,a,b):\n    return box.work(a,b)\nbox=Box()\nfor i in range(500):\n    caller(box,1,2)\n",
            tier,
            BytecodeMode::Register,
        );
        let caller = vm.get_global("caller").unwrap();
        let boxed = vm.get_global("box").unwrap();
        assert!(
            vm.call(caller, &[boxed, Value::None, Value::Int(1)])
                .is_err()
        );
        vm.run_program(&mut compile("assert box.count==501\n").unwrap())
            .unwrap();
        assert_eq!(vm.stack_len(), 0);
    }
}

#[test]
fn callable_replacement_and_argument_effects_keep_lookup_order() {
    for tier in [Tiering::Lir, Tiering::Native, Tiering::CopyPatch] {
        let mut vm = run(
            "class Box:\n    def __init__(self):\n        self.x=0\n    def add(self,x):\n        self.x=self.x+x\n        return self.x\ndef changed(self,x):\n    self.x=self.x+x+10\n    return self.x\ndef argument(change):\n    if change:\n        Box.add=changed\n    return 1\ndef caller(b,change):\n    return b.add(argument(change))\nb=Box()\nfor i in range(500):\n    caller(b,False)\n",
            tier,
            BytecodeMode::Register,
        );
        let caller = vm.get_global("caller").unwrap();
        let b = vm.get_global("b").unwrap();
        let hits = vm.tier_stats.leaf_body_inlines;
        assert_eq!(
            vm.call(caller, &[b, Value::Bool(true)]).unwrap(),
            Value::Int(501)
        );
        assert!(vm.tier_stats.leaf_body_inlines > hits);
        assert_eq!(
            vm.call(caller, &[b, Value::Bool(false)]).unwrap(),
            Value::Int(512)
        );
        assert_eq!(vm.stack_len(), 0);
    }
}

#[test]
fn replacement_with_the_same_identity_and_a_new_revision_misses() {
    for tier in [Tiering::Lir, Tiering::Native, Tiering::CopyPatch] {
        let mut vm = run(
            "def leaf(x):\n    result=[x]\n    return result\ndef caller(x):\n    return leaf(x)\nfor i in range(500):\n    caller(i)\n",
            tier,
            BytecodeMode::Register,
        );
        let leaf = vm.get_global("leaf").unwrap();
        let function = match vm.heap.get(leaf).unwrap() {
            tis::Object::Function(closure) => closure.function.clone(),
            _ => unreachable!(),
        };
        {
            let mut function = function.borrow_mut();
            let block = function.entry();
            let value = function.append(block, tis::hir::Op::Constant(tis::hir::Constant::Int(99)));
            function.terminate(block, tis::hir::Terminator::Return(value));
        }
        let caller = vm.get_global("caller").unwrap();
        assert_eq!(vm.call(caller, &[Value::Int(1)]).unwrap(), Value::Int(99));
        assert_eq!(vm.stack_len(), 0);
    }
}

#[test]
fn copied_division_guards_preserve_completed_writes_and_numeric_semantics() {
    for tier in [Tiering::Lir, Tiering::Native, Tiering::CopyPatch] {
        let mut vm = run(
            "class Box:\n    def __init__(self):\n        self.count=0\n    def divide(self,x,y):\n        self.count+=1\n        return x/y\ndef caller(b,x,y):\n    return b.divide(x,y)\nb=Box()\nfor i in range(500):\n    assert caller(b,4.0,2.0)==2.0\n",
            tier,
            BytecodeMode::Register,
        );
        let caller = vm.get_global("caller").unwrap();
        let b = vm.get_global("b").unwrap();
        let deopts = vm.tier_stats.deoptimizations;
        assert!(
            vm.call(caller, &[b, Value::Float(1.0), Value::Float(-0.0)])
                .is_err()
        );
        assert!(
            vm.tier_stats.deoptimizations > deopts,
            "{tier:?} {:?}",
            vm.tier_stats
        );
        vm.run_program(&mut compile("assert b.count==501\n").unwrap())
            .unwrap();
        assert_eq!(
            vm.call(caller, &[b, Value::Int(4), Value::Int(2)]).unwrap(),
            Value::Float(2.0)
        );
        vm.run_program(&mut compile("assert b.count==502\n").unwrap())
            .unwrap();
        assert_eq!(vm.stack_len(), 0);
    }
}

#[test]
fn allocating_leaf_return_and_caller_roots_survive_repeated_collection() {
    for tier in [Tiering::Lir, Tiering::Native, Tiering::CopyPatch] {
        let vm = run(
            "def leaf(x):\n    temporary=[x]\n    return (temporary, {'x':x})\ndef caller(x):\n    keep=[x]\n    result=leaf(x)\n    return keep,result\nfor i in range(9000):\n    result=caller(i)\n    assert result[0][0]==i\n    assert result[1][0][0]==i\n    assert result[1][1]['x']==i\n",
            tier,
            BytecodeMode::Register,
        );
        assert!(vm.tier_stats.leaf_body_inlines > 0);
        assert_eq!(vm.stack_len(), 0);
    }
}

#[test]
fn eager_feedback_keeps_physical_callee_observation() {
    let mut vm = Vm::new();
    vm.tiering = Tiering::Native;
    vm.profiling = tis::profile::ProfileConfig::eager();
    vm.thresholds.function_entries = 16;
    vm.run_program(&mut compile("def leaf(x):\n    return [x]\ndef caller(x):\n    return leaf(x)\nfor i in range(500):\n    caller(i)\n").unwrap()).unwrap();
    assert_eq!(vm.tier_stats.leaf_body_inlines, 0);
    let leaf = vm.function_hir(vm.get_global("leaf").unwrap()).unwrap();
    assert_eq!(leaf.profile().entries, 500);
    assert!(
        leaf.profile()
            .sites
            .iter()
            .any(|site| site.executions >= 500)
    );
}

#[test]
fn inline_guard_misses_refresh_the_live_callee_profile() {
    let mut vm = run(
        "class Box:\n    def __init__(self):\n        self.count=0\ndef leaf(box,a,b):\n    box.count+=1\n    return a+b\ndef caller(box,a,b):\n    return leaf(box,a,b)\nbox=Box()\nfor i in range(500):\n    caller(box,1,2)\n",
        Tiering::Native,
        BytecodeMode::Register,
    );
    let leaf = vm.get_global("leaf").unwrap();
    let before = vm.function_hir(leaf).unwrap().profile();
    assert_eq!(before.entries, 500);
    assert_eq!(before.returns, 500);
    let caller = vm.get_global("caller").unwrap();
    let boxed = vm.get_global("box").unwrap();
    let a = vm
        .heap
        .allocate(tis::Object::List(vec![Value::Int(1)].into()));
    let b = vm
        .heap
        .allocate(tis::Object::List(vec![Value::Int(2)].into()));
    vm.call(caller, &[boxed, a, b]).unwrap();
    let after = vm.function_hir(leaf).unwrap().profile();
    assert_eq!(after.entries, 501);
    assert_eq!(after.returns, 501);
    assert!(after.deoptimizations > before.deoptimizations);
    assert!(after.sites.iter().any(|site| {
        site.guard_failures > 0
            && site
                .operands
                .iter()
                .any(|operand| operand.count(tis::profile::ValueKind::List) > 0)
    }));
}

#[test]
fn loop_inlining_uses_feedback_collected_after_the_call_site_stops_sampling() {
    let mut vm = Vm::new();
    vm.tiering = Tiering::Native;
    vm.thresholds.function_entries = 64;
    vm.thresholds.loop_backedges = 8;
    vm.run_program(&mut compile("class Box:\n    def __init__(self):\n        self.x=0\n    def touch(self,x):\n        self.x+=x\ndef driver(b,n):\n    for i in range(n):\n        b.touch(1)\nb=Box()\ndriver(b,500)\nassert b.x==500\n").unwrap()).unwrap();
    assert!(vm.tier_stats.leaf_body_inlines > 0);
    // The driver's method lookup has no own field hit. These come from the copied callee.
    assert!(vm.tier_stats.native_inline_field_hits > 0);
    assert_eq!(vm.tier_stats.compilation_errors, 0);
}

#[test]
fn inline_root_recipes_and_shadow_bounds_are_verified_before_installation() {
    let vm = run(
        "def leaf(x):\n    return [x]\ndef caller(x):\n    return leaf(x)\nfor i in range(500):\n    caller(i)\n",
        Tiering::Native,
        BytecodeMode::Register,
    );
    let source = vm.function_hir(vm.get_global("caller").unwrap()).unwrap();
    let lir = source.lower_lir(tis::lir::LoweringMode::Profiled).unwrap();
    assert!(!lir.inline_scopes.is_empty());
    let mut missing_root = lir.clone();
    let state = missing_root
        .states
        .iter_mut()
        .find_map(|state| match &mut state.environment_root {
            tis::lir::FrameEnvironment::Inline(state) => Some(state),
            _ => None,
        })
        .unwrap();
    assert!(std::rc::Rc::make_mut(state).roots.pop().is_some());
    assert!(missing_root.verify().is_err());
    let mut missing_state = lir.clone();
    let state = missing_state
        .states
        .iter_mut()
        .find(|state| state.inline().is_some())
        .unwrap();
    state.environment_root = tis::lir::FrameEnvironment::Current;
    assert!(missing_state.verify().is_err());
    let mut missing_storage = lir;
    missing_storage.shadow_slots = missing_storage.hir_values.len();
    assert!(missing_storage.verify().is_err());
}

#[test]
fn ordinary_safepoint_records_keep_their_original_footprint() {
    assert_eq!(
        std::mem::size_of::<tis::lir::FrameEnvironment>(),
        std::mem::size_of::<usize>()
    );
    assert_eq!(
        std::mem::size_of::<tis::lir::FrameState>(),
        10 * std::mem::size_of::<usize>()
    );
}
