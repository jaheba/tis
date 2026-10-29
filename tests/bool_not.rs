use tis::{
    Object, Tiering, Value, Vm, compile,
    lir::{LoweringMode, Operation, Register},
    profile::ProfileMode,
};

fn warmed(tier: Tiering) -> Vm {
    let mut vm = Vm::new();
    vm.tiering = tier;
    vm.thresholds.function_entries = 1;
    vm.thresholds.loop_backedges = 4;
    vm.run_program(&mut compile("def invert(x):\n    return not x\nfor i in range(300):\n    invert(i%2)\n    invert([])\nassert invert(None)\nassert invert(False)\nassert not invert(True)\nassert invert('')\nassert not invert('x')\nassert invert([])\nassert not invert([1])\nassert invert(())\nassert not invert((1,))\nassert invert({})\nassert not invert({1:2})\nassert invert(set())\nassert not invert({1})\nassert invert(range(0))\nassert not invert(range(2))\nassert not invert(invert)\n").unwrap()).unwrap();
    vm
}

#[test]
fn typed_not_preserves_truth_categories_and_exact_eager_observation() {
    for tier in [Tiering::Lir, Tiering::Native, Tiering::CopyPatch] {
        let mut vm = warmed(tier);
        let f = vm.get_global("invert").unwrap();
        let lir = vm
            .function_hir(f)
            .unwrap()
            .lower_lir(LoweringMode::Profiled)
            .unwrap();
        let origin = lir
            .blocks
            .iter()
            .flat_map(|b| &b.instructions)
            .find(|i| matches!(i.operation, Operation::BoolNot(_)))
            .unwrap()
            .origin;
        for (value, expected) in [
            (Value::None, true),
            (Value::Bool(false), true),
            (Value::Bool(true), false),
            (Value::Int(0), true),
            (Value::Int(i64::MIN), false),
            (Value::Int(i64::MAX), false),
            (Value::Float(0.0), true),
            (Value::Float(-0.0), true),
            (Value::Float(f64::NAN), false),
            (Value::Float(f64::INFINITY), false),
            (Value::Float(f64::NEG_INFINITY), false),
            (Value::Float(f64::from_bits(1)), false),
        ] {
            assert_eq!(vm.call(f, &[value]).unwrap(), Value::Bool(expected));
        }
        vm.profiling.mode = ProfileMode::Eager;
        let empty = vm.heap.allocate(Object::List(vec![].into()));
        let before = vm.function_hir(f).unwrap().profile().sites[origin.0].executions;
        assert_eq!(vm.call(f, &[Value::Int(0)]).unwrap(), Value::Bool(true));
        assert_eq!(vm.call(f, &[empty]).unwrap(), Value::Bool(true));
        assert_eq!(
            vm.function_hir(f).unwrap().profile().sites[origin.0].executions,
            before + 2
        );
        vm.collect_garbage(&[]).unwrap();
        assert!(vm.heap.get(empty).is_err());
        assert_eq!(vm.stack_len(), 0);
    }
}

#[test]
fn gc_and_osr_keep_effects_and_frame_state() {
    for tier in [Tiering::Lir, Tiering::Native, Tiering::CopyPatch] {
        let mut vm = warmed(tier);
        vm.run_program(&mut compile("events=[]\nenabled=False\ndef f(x):\n    events.append('before')\n    live=[x]\n    value=not x\n    for i in range(7000 if enabled else 0):\n        trash=[i]\n    events.append('after')\n    return live,value\nfor i in range(300):\n    f(i%2)\n    f([])\nenabled=True\nassert f([])==([[]],True)\nassert f([1])==([[1]],False)\n").unwrap()).unwrap();
        vm.collect_garbage(&[]).unwrap();
        assert_eq!(vm.stack_len(), 0);

        let mut osr = Vm::new();
        osr.tiering = tier;
        osr.thresholds.function_entries = u64::MAX;
        osr.thresholds.loop_backedges = 4;
        osr.profiling.window_ticks = 4;
        osr.run_program(&mut compile("def count(n):\n    total=0\n    while n:\n        if not n%2:\n            total+=1\n        n-=1\n    return total\nassert count(10000)==5000\n").unwrap()).unwrap();
        assert!(
            match tier {
                Tiering::Native => osr.tier_stats.native_osr_entries,
                Tiering::CopyPatch => osr.tier_stats.copy_patch_osr_entries,
                _ => osr.tier_stats.osr_entries,
            } > 0
        );
        assert_eq!(osr.stack_len(), 0);
    }
}

#[test]
fn bool_inversion_verifier_rejects_boxed_operands() {
    let vm = warmed(Tiering::Lir);
    let mut lir = vm
        .function_hir(vm.get_global("invert").unwrap())
        .unwrap()
        .lower_lir(LoweringMode::Generic)
        .unwrap();
    for i in lir.blocks.iter_mut().flat_map(|b| &mut b.instructions) {
        if let Operation::BoolNot(value) = &mut i.operation {
            *value = Register(0);
        }
    }
    assert!(lir.verify().is_err());
}

#[test]
fn truth_helper_errors_preserve_prior_effects() {
    use tis::hir::{Constant, CoreOp, Function, Op, Terminator};
    for tier in [Tiering::Lir, Tiering::Native, Tiering::CopyPatch] {
        let mut f = Function::new(0);
        let block = f.entry();
        let print = f.append(block, Op::Runtime(CoreOp::LoadName("print".into()), vec![]));
        let message = f.append(block, Op::Constant(Constant::String("before".into())));
        f.append(
            block,
            Op::Runtime(CoreOp::Call(vec![None]), vec![print, message]),
        );
        // An internal sentinel reaches Truth directly, so its error happens after the print.
        let unbound = f.append(block, Op::Constant(Constant::Unbound));
        let value = f.append(
            block,
            Op::Runtime(CoreOp::Unary("not".into()), vec![unbound]),
        );
        f.terminate(block, Terminator::Return(value));
        let mut vm = Vm::new();
        vm.tiering = tier;
        vm.thresholds.function_entries = 1;
        vm.profiling.window_ticks = 2;
        for _ in 0..20 {
            assert!(
                vm.run(&mut f, &[])
                    .unwrap_err()
                    .to_string()
                    .contains("unbound value escaped")
            );
        }
        assert!(
            match tier {
                Tiering::Native => vm.tier_stats.native_entries,
                Tiering::CopyPatch => vm.tier_stats.copy_patch_entries,
                _ => vm.tier_stats.lir_entries,
            } > 0
        );
        assert_eq!(vm.output, "before\n".repeat(20));
        assert_eq!(vm.stack_len(), 0);
    }
}
