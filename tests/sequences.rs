use tis::{
    Tiering, Value, Vm, compile,
    lir::{ExitReason, LoweringMode, Operation, SequenceKind},
    profile::{HotThresholds, ProfileConfig},
};
const TIERS: [Tiering; 4] = [
    Tiering::Bytecode,
    Tiering::Lir,
    Tiering::Native,
    Tiering::CopyPatch,
];
fn machine(tier: Tiering) -> Vm {
    let mut vm = Vm::new();
    vm.tiering = tier;
    vm.thresholds = HotThresholds {
        function_entries: 4,
        loop_backedges: 4,
    };
    vm.profiling.window_ticks = 16;
    vm
}
fn run(source: &str, tier: Tiering) -> Vm {
    let mut p = compile(source).unwrap();
    let mut vm = machine(tier);
    vm.run_program(&mut p).unwrap();
    vm
}

#[test]
fn hot_unpack_uses_a_checked_view_and_eager_element_reads() {
    let mut vm = machine(Tiering::Bytecode);
    vm.profiling = ProfileConfig::eager();
    let mut p = compile(
        "def f(xs):\n    a,b=xs\n    return a+b\nfor i in range(20):\n    assert f([1,2])==3\n",
    )
    .unwrap();
    vm.run_program(&mut p).unwrap();
    let f = vm.function_hir(vm.get_global("f").unwrap()).unwrap();
    let lir = f.lower_lir(LoweringMode::Profiled).unwrap();
    assert!(
        lir.blocks
            .iter()
            .flat_map(|b| &b.instructions)
            .any(|i| matches!(
                i.operation,
                Operation::SequenceUnpack {
                    kind: SequenceKind::List,
                    count: 2,
                    ..
                }
            ))
    );
    assert_eq!(
        lir.blocks
            .iter()
            .flat_map(|b| &b.instructions)
            .filter(|i| matches!(i.operation, Operation::SequenceGet { .. }))
            .count(),
        2
    );
    for tier in TIERS {
        let mut vm = run(
            "def f(xs):\n    a,b=xs\n    return a+b\nfor i in range(40):\n    assert f([1,2])==3\n",
            tier,
        );
        let f = vm.get_global("f").unwrap();
        let tuple = vm
            .heap
            .allocate(tis::Object::Tuple(vec![Value::Int(3), Value::Int(4)]));
        assert_eq!(vm.call(f, &[tuple]).unwrap(), Value::Int(7));
        if tier != Tiering::Bytecode {
            assert!(vm.tier_stats.deoptimizations > 0);
        }
        assert_eq!(vm.stack_len(), 0);
    }
}

#[test]
fn unpack_aliases_are_snapshotted_before_mutating_assignment_targets() {
    let source = "def index(a):\n    a[1]=999\n    return 0\ndef f(a):\n    a[1],a[0]=a\n    return a\ndef g(a):\n    a[index(a)],a[1]=a\n    return a\nfor i in range(40):\n    assert f([1,2])==[2,1]\n    assert g([10,20])==[10,20]\n";
    for tier in TIERS {
        let mut vm = run(source, tier);
        vm.collect_garbage(&[]).unwrap();
    }
}

#[test]
fn nested_unpack_errors_preserve_prefix_assignments_and_do_not_repeat_effects() {
    for tier in TIERS {
        let mut vm = run(
            "calls=[0]\nassigned=[0]\ndef f(xs):\n    calls[0]+=1\n    assigned[0],(a,b)=xs\n    return a+b\nfor i in range(40):\n    assert f([1,[2,3]])==5\n",
            tier,
        );
        let inner = vm.heap.allocate(tis::Object::List(vec![Value::Int(2)]));
        let outer = vm
            .heap
            .allocate(tis::Object::List(vec![Value::Int(99), inner]));
        let error = vm.call(vm.get_global("f").unwrap(), &[outer]).unwrap_err();
        assert!(error.to_string().contains("unpacking length mismatch"));
        assert_eq!(
            vm.heap
                .get_item(vm.get_global("calls").unwrap(), Value::Int(0))
                .unwrap(),
            Value::Int(41)
        );
        assert_eq!(
            vm.heap
                .get_item(vm.get_global("assigned").unwrap(), Value::Int(0))
                .unwrap(),
            Value::Int(99)
        );
        assert_eq!(vm.stack_len(), 0);
    }
}

#[test]
fn indexed_reads_guard_current_kind_length_and_integer_index() {
    for tier in TIERS {
        for (case, index) in [
            (0, Value::Int(-1)),
            (1, Value::Bool(true)),
            (2, Value::Int(i64::MIN)),
            (3, Value::Int(2)),
        ] {
            let mut vm = run(
                "calls=[0]\ndef f(xs,index):\n    calls[0]+=1\n    return xs[index]\nfor i in range(40):\n    assert f([10,20],0)==10\n",
                tier,
            );
            let source = vm.heap.allocate(if case == 0 {
                tis::Object::Tuple(vec![Value::Int(10), Value::Int(20)])
            } else {
                tis::Object::List(vec![Value::Int(10), Value::Int(20)])
            });
            let result = vm.call(vm.get_global("f").unwrap(), &[source, index]);
            if case < 2 {
                assert_eq!(result.unwrap(), Value::Int(20));
            } else {
                assert!(result.is_err());
            }
            assert_eq!(
                vm.heap
                    .get_item(vm.get_global("calls").unwrap(), Value::Int(0))
                    .unwrap(),
                Value::Int(41)
            );
            if tier != Tiering::Bytecode {
                assert!(vm.tier_stats.deoptimizations > 0);
            }
            assert_eq!(vm.stack_len(), 0);
        }
    }
}

#[test]
fn mutable_length_changes_are_checked_after_compilation() {
    for tier in TIERS {
        let mut vm = run(
            "xs=[10,20]\ndef f(xs):\n    return xs[-1]\nfor i in range(40):\n    assert f(xs)==20\nxs.clear()\n",
            tier,
        );
        let f = vm.get_global("f").unwrap();
        let xs = vm.get_global("xs").unwrap();
        assert!(vm.call(f, &[xs]).is_err());
        assert_eq!(vm.stack_len(), 0);
    }
}

#[test]
fn generic_iterables_starred_targets_and_unicode_keep_their_semantics() {
    let source = "def f(xs):\n    a,b=xs\n    return a,b\nfor i in range(40):\n    assert f([1,2])==(1,2)\nassert f(iter([3,4]))==(3,4)\nassert f('éλ')==('é','λ')\na,*b,c=iter([1,2,3,4])\nassert a==1 and b==[2,3] and c==4\nx,y=range(2)\nassert x==0 and y==1\n";
    for tier in TIERS {
        run(source, tier);
    }
}

#[test]
fn sequence_result_references_survive_source_mutation_and_collection() {
    let source = "def churn(xs):\n    xs.clear()\n    for i in range(6000):\n        unused=[i]\ndef f(xs):\n    saved=xs[0]\n    churn(xs)\n    return saved[0]\nfor i in range(40):\n    assert f([[42]])==42\n";
    for tier in TIERS {
        let mut vm = run(source, tier);
        vm.collect_garbage(&[]).unwrap();
    }
}

#[test]
fn verifier_rejects_escaped_or_interrupted_unpack_views() {
    let mut vm = machine(Tiering::Bytecode);
    vm.profiling = ProfileConfig::eager();
    let mut p =
        compile("def f(xs):\n    a,b=xs\n    return a+b\nfor i in range(20):\n    f([1,2])\n")
            .unwrap();
    vm.run_program(&mut p).unwrap();
    let f = vm.function_hir(vm.get_global("f").unwrap()).unwrap();
    let lir = f.lower_lir(LoweringMode::Profiled).unwrap();
    let block = lir
        .blocks
        .iter()
        .position(|b| {
            b.instructions
                .iter()
                .any(|i| matches!(i.operation, Operation::SequenceUnpack { .. }))
        })
        .unwrap();
    let index = lir.blocks[block]
        .instructions
        .iter()
        .position(|i| matches!(i.operation, Operation::SequenceUnpack { .. }))
        .unwrap();
    let mut overflow = lir.clone();
    if let Operation::SequenceUnpack { count, .. } =
        &mut overflow.blocks[block].instructions[index].operation
    {
        *count = usize::MAX;
    }
    assert!(overflow.verify().is_err());
    let mut escaped = lir.clone();
    escaped.blocks[block].terminator =
        tis::lir::Terminator::Return(escaped.blocks[block].instructions[index].result.unwrap());
    assert!(escaped.verify().is_err());
    let mut interrupted = lir.clone();
    interrupted.blocks[block]
        .instructions
        .swap(index + 1, index + 2);
    assert!(interrupted.verify().is_err());
    assert!(
        lir.exits
            .iter()
            .any(|exit| exit.reason == ExitReason::SequenceGuard)
    );
}
