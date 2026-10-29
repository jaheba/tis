use tis::{
    BytecodeMode, Object, Tiering, Value, Vm, compile,
    lir::{Function, Helper, LoweringMode, Operation},
};

fn warm(source: &str, tier: Tiering) -> Vm {
    let mut vm = Vm::new();
    vm.tiering = tier;
    vm.cfg_inlining = true;
    vm.bytecode = BytecodeMode::Register;
    vm.thresholds.function_entries = 16;
    vm.thresholds.loop_backedges = 32;
    vm.run_program(&mut compile(source).unwrap()).unwrap();
    vm
}

#[test]
fn copied_unpack_comparison_and_integer_add_use_ordinary_specializations() {
    for tier in [Tiering::Lir, Tiering::Native, Tiering::CopyPatch] {
        let mut vm = warm(
            "class Box:\n    def __init__(self):\n        self.count=0\ndef leaf(box,xs):\n    box.count+=1\n    if box.count>0:\n        a,b=xs\n        return [a+b]\n    return []\ndef caller(box,xs):\n    return leaf(box,xs)\nbox=Box()\nfor i in range(500):\n    caller(box,(1,2))\n",
            tier,
        );
        let caller = vm.get_global("caller").unwrap();
        let boxed = vm.get_global("box").unwrap();
        let source = vm.function_hir(caller).unwrap();
        let lir = Function::lower_with_cfg(&source, LoweringMode::Profiled).unwrap();
        for predicate in [
            (|op: &Operation| matches!(op, Operation::SequenceUnpack { .. }))
                as fn(&Operation) -> bool,
            |op| matches!(op, Operation::SequenceGet { .. }),
            |op| matches!(op, Operation::NumberCompare { .. }),
            |op| matches!(op, Operation::CheckedAdd { .. }),
        ] {
            assert!(
                lir.blocks
                    .iter()
                    .flat_map(|b| &b.instructions)
                    .any(|i| predicate(&i.operation)),
                "{tier:?} {}",
                lir.dump()
            );
        }
        assert!(
            !lir.blocks
                .iter()
                .flat_map(|b| &b.instructions)
                .any(|i| matches!(
                    i.operation,
                    Operation::Call {
                        helper: Helper::Python(
                            tis::hir::CoreOp::Unpack(_) | tis::hir::CoreOp::UnpackItem(_)
                        ),
                        ..
                    }
                ))
        );
        let list = vm
            .heap
            .allocate(Object::List(vec![Value::Int(7), Value::Int(8)].into()));
        let exits = vm.tier_stats.deoptimizations;
        let before = vm.tier_stats.clone();
        let result = vm.call(caller, &[boxed, list]).unwrap();
        assert_backend_entry(&vm, tier, &before, 1);
        assert!(vm.tier_stats.deoptimizations > exits);
        assert_eq!(
            vm.heap.get_item(result, Value::Int(0)).unwrap(),
            Value::Int(15)
        );
        vm.run_program(&mut compile("assert box.count==501\n").unwrap())
            .unwrap();
        assert_eq!(vm.stack_len(), 0);
    }
}

#[test]
fn copied_float_phis_survive_helpers_gc_and_guard_misses() {
    for tier in [Tiering::Lir, Tiering::Native, Tiering::CopyPatch] {
        let mut vm = warm(
            "class Box:\n    def __init__(self):\n        self.count=0\ndef leaf(box,flag,x):\n    box.count+=1\n    if flag:\n        v=x*2.0\n    else:\n        v=x/2.0\n    keep=[v]\n    size=len(keep)\n    return [v+1.0,size]\ndef caller(box,flag,x):\n    return leaf(box,flag,x)\nbox=Box()\nfor i in range(500):\n    caller(box,i%2==0,4.0)\n",
            tier,
        );
        let caller = vm.get_global("caller").unwrap();
        let boxed = vm.get_global("box").unwrap();
        let source = vm.function_hir(caller).unwrap();
        let lir = Function::lower_with_cfg(&source, LoweringMode::Profiled).unwrap();
        assert!(
            lir.scalar_sources
                .values()
                .any(|r| r.0 >= lir.hir_values.len())
        );
        let before = vm.tier_stats.clone();
        for i in 0..3000 {
            let flag = i % 2 == 0;
            let result = vm
                .call(caller, &[boxed, Value::Bool(flag), Value::Float(4.0)])
                .unwrap();
            assert_eq!(
                vm.heap.get_item(result, Value::Int(0)).unwrap(),
                Value::Float(if flag { 9.0 } else { 3.0 })
            );
            assert_eq!(
                vm.heap.get_item(result, Value::Int(1)).unwrap(),
                Value::Int(1)
            );
        }
        assert_backend_entry(&vm, tier, &before, 3000);
        let exits = vm.tier_stats.deoptimizations;
        let result = vm
            .call(caller, &[boxed, Value::Bool(true), Value::Int(4)])
            .unwrap();
        assert!(vm.tier_stats.deoptimizations > exits);
        assert_eq!(
            vm.heap.get_item(result, Value::Int(0)).unwrap(),
            Value::Float(9.0)
        );
        vm.run_program(&mut compile("assert box.count==3501\n").unwrap())
            .unwrap();
        assert_eq!(vm.stack_len(), 0);
    }
}

#[test]
fn copied_integer_overflow_resumes_after_prior_writes() {
    for tier in [Tiering::Lir, Tiering::Native, Tiering::CopyPatch] {
        let mut vm = warm(
            "class Box:\n    def __init__(self):\n        self.count=0\ndef leaf(box,a,b):\n    box.count+=1\n    return a+b\ndef caller(box,a,b):\n    return leaf(box,a,b)\nbox=Box()\nfor i in range(500):\n    caller(box,1,2)\n",
            tier,
        );
        let caller = vm.get_global("caller").unwrap();
        let boxed = vm.get_global("box").unwrap();
        let exits = vm.tier_stats.deoptimizations;
        assert!(
            vm.call(caller, &[boxed, Value::Int(i64::MAX), Value::Int(1)])
                .is_err()
        );
        assert!(vm.tier_stats.deoptimizations > exits);
        vm.run_program(&mut compile("assert box.count==501\n").unwrap())
            .unwrap();
        assert_eq!(vm.stack_len(), 0);
    }
}

fn assert_backend_entry(vm: &Vm, tier: Tiering, before: &tis::TierStats, count: u64) {
    assert_eq!(vm.tier_stats.compilation_errors, 0);
    match tier {
        Tiering::Native => assert_eq!(vm.tier_stats.native_entries - before.native_entries, count),
        Tiering::CopyPatch => assert_eq!(
            vm.tier_stats.copy_patch_entries - before.copy_patch_entries,
            count
        ),
        _ => {}
    }
}

#[test]
fn copied_float_caches_are_not_caller_loop_imports() {
    for tier in [Tiering::Lir, Tiering::Native, Tiering::CopyPatch] {
        for mode in [BytecodeMode::Stack, BytecodeMode::Register] {
            for cfg_inlining in [false, true] {
                let source = "class Box:\n    def __init__(self):\n        self.value=0.0\ndef touch(box,x):\n    box.value+=x*x\ndef driver(box,x,n):\n    for i in range(n):\n        touch(box,x)\nbox=Box()\ndriver(box,2.0,5000)\nassert box.value==20000.0\n";
                let mut vm = Vm::new();
                vm.tiering = tier;
                vm.bytecode = mode;
                vm.cfg_inlining = cfg_inlining;
                // Start sampling inside the sole loop invocation, after the caller's argument loads.
                vm.thresholds.function_entries = 64;
                vm.thresholds.loop_backedges = 8;
                vm.run_program(&mut compile(source).unwrap()).unwrap();
                assert_eq!(
                    vm.tier_stats.compilation_errors, 0,
                    "{tier:?} {mode:?} {:?}",
                    vm.tier_stats
                );
                assert!(vm.tier_stats.osr_entries > 0);
                let driver = vm.get_global("driver").unwrap();
                let boxed = vm.get_global("box").unwrap();
                let hir = vm.function_hir(driver).unwrap();
                let lir = Function::lower_with_cfg(&hir, LoweringMode::Profiled).unwrap();
                assert!(!lir.inline_scopes.is_empty());
                assert!(lir.scalar_sources.iter().any(|(raw, boxed)| {
                    boxed.0 < lir.hir_values.len()
                        && lir
                            .inline_scopes
                            .iter()
                            .any(|scope| scope.registers.contains(&raw.0))
                }));
                let before = vm.tier_stats.clone();
                vm.call(driver, &[boxed, Value::Float(3.0), Value::Int(20)])
                    .unwrap();
                assert!(vm.tier_stats.leaf_body_inlines > before.leaf_body_inlines);
                assert_backend_entry(&vm, tier, &before, 1);
                vm.run_program(&mut compile("assert box.value==20180.0\n").unwrap())
                    .unwrap();
                let deopts = vm.tier_stats.deoptimizations;
                vm.call(driver, &[boxed, Value::Int(3), Value::Int(2)])
                    .unwrap();
                assert!(vm.tier_stats.deoptimizations > deopts);
                vm.run_program(&mut compile("assert box.value==20198.0\n").unwrap())
                    .unwrap();
                assert_eq!(vm.stack_len(), 0);
            }
        }
    }
}

#[test]
fn physical_loop_scalar_imports_still_require_entry_guards() {
    let vm = warm(
        "def count(n):\n    total=0.0\n    for i in range(n):\n        total=total+1.0\n    return total\nassert count(5000)==5000.0\n",
        Tiering::Lir,
    );
    let hir = vm.function_hir(vm.get_global("count").unwrap()).unwrap();
    let mut lir = Function::lower_with_cfg(&hir, LoweringMode::Profiled).unwrap();
    let header = lir
        .osr_entries
        .iter()
        .find(|entry| lir.scalar_entries.contains_key(&entry.header))
        .unwrap()
        .header;
    lir.scalar_entries.remove(&header);
    assert!(matches!(
        lir.verify(),
        Err(tis::Error::InvalidIr("missing scalar OSR entry guard"))
    ));
}
