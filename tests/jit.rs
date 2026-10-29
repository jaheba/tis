use tis::{
    Object, Value, Vm, compile,
    hir::{Constant, CoreOp, Edge, Function, Op, ResumePoint, Terminator, ValueId},
    lir::{self, ExitReason, LoweringMode, Operation, Type},
    profile::{CallTarget, HotThresholds, Hotspot, ValueKind},
};

fn add() -> (Function, ValueId) {
    let mut function = Function::new(2).with_name("add");
    let block = function.entry();
    let args = function.parameters(block);
    let value = function.append(block, Op::Add(args[0], args[1]));
    function.terminate(block, Terminator::Return(value));
    (function, value)
}

#[test]
fn profiles_arguments_results_errors_and_call_targets() {
    let mut program = compile("def f(a=2):\n    return a + 1\nf()\nf(1.5)\nf('x')\n").unwrap();
    let mut vm = Vm::new();
    vm.profiling = tis::profile::ProfileConfig::eager();
    assert!(vm.run_program(&mut program).is_err());
    let f = vm.function_ir(vm.get_global("f").unwrap()).unwrap();
    let profile = f.profile();
    assert_eq!(
        (profile.entries, profile.returns, profile.errors),
        (3, 2, 1)
    );
    assert_eq!(profile.arguments[0].count(ValueKind::Int), 1);
    assert_eq!(profile.arguments[0].count(ValueKind::Float), 1);
    assert_eq!(profile.arguments[0].count(ValueKind::String), 1);
    assert_eq!(profile.return_types.count(ValueKind::Int), 1);
    assert_eq!(profile.return_types.count(ValueKind::Float), 1);
    let module = program.function("<module>").unwrap().profile();
    assert!(module.sites.iter().any(|site| {
        site.call_targets
            .contains_key(&CallTarget::Function(f.id()))
    }));
}

#[test]
fn hot_requests_are_once_per_version_and_keep_a_detached_profile() {
    let (mut function, site) = add();
    let mut vm = Vm::new();
    vm.profiling.window_ticks = 8;
    vm.thresholds = HotThresholds {
        function_entries: 10,
        loop_backedges: 0,
    };
    for _ in 0..20 {
        assert_eq!(
            vm.run(&mut function, &[Value::Int(1), Value::Int(2)])
                .unwrap(),
            Value::Int(3)
        );
    }
    let requests = vm.take_compilation_requests();
    assert_eq!(requests.len(), 1);
    let request = &requests[0];
    assert_eq!(request.hotspot, Hotspot::Function);
    assert_eq!(request.count, 18);
    assert_eq!(request.hir().profile().entries, 18);
    assert_eq!(function.profile().entries, 20);
    assert_eq!(request.hir().profile().site(site).unwrap().executions, 8);
    assert!(request.is_current(&function));
    request.lower().unwrap();
    function.terminate(
        function.entry(),
        Terminator::Return(function.parameters(function.entry())[0]),
    );
    assert!(!request.is_current(&function));
    assert_eq!(function.profile().entries, 0);
    for _ in 0..18 {
        vm.run(&mut function, &[Value::Int(1), Value::Int(2)])
            .unwrap();
    }
    assert_eq!(vm.take_compilation_requests().len(), 1);
}

#[test]
fn loops_are_hot_after_backedges_even_with_a_single_invocation() {
    let mut program = compile(
        "def loop(n):\n    x = 0\n    while x < n:\n        x += 1\n    return x\nloop(20)\n",
    )
    .unwrap();
    let mut vm = Vm::new();
    vm.profiling.window_ticks = 8;
    vm.thresholds = HotThresholds {
        function_entries: 100,
        loop_backedges: 5,
    };
    vm.run_program(&mut program).unwrap();
    let requests = vm.take_compilation_requests();
    assert_eq!(requests.len(), 1);
    let request = &requests[0];
    assert!(matches!(request.hotspot, Hotspot::Loop { .. }));
    assert_eq!(request.count, 13);
    let function = vm.function_ir(vm.get_global("loop").unwrap()).unwrap();
    let profile = function.profile();
    assert_eq!(profile.entries, 1);
    assert_eq!(profile.backedges.values().sum::<u64>(), 20);
    let lir = request.lower().unwrap();
    assert_eq!(lir.osr_entries.len(), 1);
    assert_eq!(
        lir.osr_entries[0].header.0,
        if let Hotspot::Loop { header } = request.hotspot {
            header
        } else {
            unreachable!()
        }
    );
}

#[test]
fn closures_share_code_profiles_without_using_heap_identity() {
    let mut program=compile("def factory():\n    def inner(x):\n        return x + 1\n    return inner\na = factory()\nb = factory()\na(1)\nb(2.0)\n").unwrap();
    let mut vm = Vm::new();
    vm.profiling = tis::profile::ProfileConfig::eager();
    vm.run_program(&mut program).unwrap();
    let a = vm.function_ir(vm.get_global("a").unwrap()).unwrap();
    let b = vm.function_ir(vm.get_global("b").unwrap()).unwrap();
    assert_eq!(a.id(), b.id());
    assert_eq!(a.profile().entries, 2);
    assert_eq!(a.profile().arguments[0].count(ValueKind::Float), 1);
    vm.collect_garbage(&[]).unwrap();
    assert_eq!(
        vm.function_ir(vm.get_global("a").unwrap()).unwrap().id(),
        a.id()
    );
}

#[test]
fn profiled_integer_addition_has_explicit_guards_overflow_and_boxing() {
    let (mut function, _) = add();
    let mut vm = Vm::new();
    vm.profiling = tis::profile::ProfileConfig::eager();
    for _ in 0..16 {
        vm.run(&mut function, &[Value::Int(1), Value::Int(2)])
            .unwrap();
    }
    let profile = function.profile();
    assert_eq!(
        profile
            .sites
            .iter()
            .map(|site| site.fast_paths)
            .sum::<u64>(),
        8
    );
    let lir = function.lower_lir(LoweringMode::Profiled).unwrap();
    assert_eq!(lir.exits.len(), 2);
    assert_eq!(lir.exits[0].reason, ExitReason::TypeGuard);
    assert_eq!(lir.exits[1].reason, ExitReason::IntegerOverflow);
    let operations = &lir.blocks[0].instructions;
    assert_eq!(
        operations
            .iter()
            .filter(|i| matches!(i.operation, Operation::GuardInt { .. }))
            .count(),
        2
    );
    assert!(
        operations
            .iter()
            .any(|i| matches!(i.operation, Operation::CheckedAdd { .. }))
    );
    assert!(lir.registers.contains(&Type::I64));
    let entry = function.entry();
    let code = function.bytecode().unwrap();
    for exit in &lir.exits {
        assert_eq!(
            lir.resume_offset(exit.state, code).unwrap(),
            code.resume_offset(ResumePoint {
                block: entry,
                instruction: 0
            })
            .unwrap()
        );
    }
}

#[test]
fn cold_or_unsupported_polymorphic_sites_keep_a_generic_runtime_call() {
    let (mut function, _) = add();
    let lir = function.lower_lir(LoweringMode::Profiled).unwrap();
    assert!(lir.exits.is_empty());
    let mut vm = Vm::new();
    vm.profiling = tis::profile::ProfileConfig::eager();
    for _ in 0..10 {
        vm.run(&mut function, &[Value::Int(1), Value::Int(2)])
            .unwrap();
    }
    vm.run(&mut function, &[Value::Float(1.0), Value::Int(2)])
        .unwrap();
    // Exact numeric unions now specialize; bool remains outside that guarded representation.
    vm.run(&mut function, &[Value::Bool(true), Value::Int(2)])
        .unwrap();
    assert!(
        function
            .lower_lir(LoweringMode::Profiled)
            .unwrap()
            .exits
            .is_empty()
    );
    assert!(
        function
            .lower_lir(LoweringMode::Generic)
            .unwrap()
            .exits
            .is_empty()
    );
}

#[test]
fn side_exits_reconstruct_slots_and_resume_without_replaying_effects() {
    let mut f = Function::new(3);
    let block = f.entry();
    let args = f.parameters(block).to_vec();
    let zero = f.append(block, Op::Constant(Constant::Int(0)));
    let previous = f.append(block, Op::GetItem(args[0], zero));
    let one = f.append(block, Op::Constant(Constant::Int(1)));
    let count = f.append(block, Op::Add(previous, one));
    f.append(
        block,
        Op::SetItem {
            object: args[0],
            key: zero,
            value: count,
        },
    );
    let add = f.append(block, Op::Add(args[1], args[2]));
    f.terminate(block, Terminator::Return(add));
    let mut vm = Vm::new();
    vm.profiling = tis::profile::ProfileConfig::eager();
    let list = vm.heap.allocate(Object::List(vec![Value::Int(0)].into()));
    let pin = vm.pin(list).unwrap();
    for _ in 0..10 {
        vm.run(&mut f, &[list, Value::Int(1), Value::Int(2)])
            .unwrap();
    }
    let lir = f.lower_lir(LoweringMode::Profiled).unwrap();
    let instruction = lir.blocks[0]
        .instructions
        .iter()
        .find(|ins| ins.origin == add && matches!(ins.operation, Operation::GuardInt { .. }))
        .unwrap();
    let Operation::GuardInt { exit, .. } = instruction.operation else {
        unreachable!()
    };
    let state = lir.exits[exit.0].state;
    let mut namespace = Function::new(0);
    let block = namespace.entry();
    let env = namespace.append(block, Op::Runtime(CoreOp::Namespace, vec![]));
    namespace.terminate(block, Terminator::Return(env));
    let env = vm.run(&mut namespace, &[]).unwrap();
    let left = vm.heap.allocate(Object::String("hello ".into()));
    let right = vm.heap.allocate(Object::String("tis".into()));
    let frame = lir
        .reconstruct(state, env, &[], |reg| {
            Ok(if reg.0 == args[1].0 {
                left
            } else if reg.0 == args[2].0 {
                right
            } else {
                panic!("unexpected live slot")
            })
        })
        .unwrap();
    let result = vm.resume_hir(&mut f, frame).unwrap();
    assert_eq!(
        vm.heap.get(result).unwrap(),
        &Object::String("hello tis".into())
    );
    assert_eq!(
        vm.heap.get_item(list, Value::Int(0)).unwrap(),
        Value::Int(10)
    );
    let frame = lir
        .reconstruct(state, env, &[], |reg| {
            Ok(if reg.0 == args[1].0 {
                Value::Int(i64::MAX)
            } else {
                Value::Int(1)
            })
        })
        .unwrap();
    assert!(vm.resume_hir(&mut f, frame).is_err());
    assert_eq!(vm.stack_len(), 0);
    assert_eq!(f.profile().deoptimizations, 2);
    vm.unpin(pin);
}

#[test]
fn root_maps_truth_conversion_and_string_allocation_are_explicit() {
    let program = compile("x = [1]\nif x:\n    print('ok')\n").unwrap();
    let lir = program
        .function("<module>")
        .unwrap()
        .lower_lir(LoweringMode::Generic)
        .unwrap();
    assert!(
        lir.blocks
            .iter()
            .flat_map(|b| &b.instructions)
            .any(|i| matches!(i.operation, Operation::Truth { .. }))
    );
    assert!(
        lir.blocks
            .iter()
            .flat_map(|b| &b.instructions)
            .any(|i| matches!(
                i.operation,
                Operation::Call {
                    helper: lir::Helper::StringConstant(_),
                    ..
                }
            ))
    );
    for state in &lir.states {
        assert!(matches!(
            state.environment_root,
            tis::lir::FrameEnvironment::Current
        ));
    }
    let mut invalid = lir.clone();
    let call = invalid
        .blocks
        .iter_mut()
        .flat_map(|b| &mut b.instructions)
        .find(|i| matches!(i.operation,Operation::Call{ref roots,..} if !roots.is_empty()))
        .unwrap();
    if let Operation::Call { roots, .. } = &mut call.operation {
        roots.clear();
    }
    assert!(invalid.verify().is_err());
    let mut vm = Vm::new();
    vm.profiling = tis::profile::ProfileConfig::eager();
    assert!(
        vm.install_lir(program.function("<module>").unwrap(), invalid)
            .is_err()
    );
}

#[test]
fn verifiers_reject_non_dominating_uses_and_unguarded_unboxing() {
    let (mut function, _) = add();
    let mut vm = Vm::new();
    vm.profiling = tis::profile::ProfileConfig::eager();
    for _ in 0..10 {
        vm.run(&mut function, &[Value::Int(1), Value::Int(2)])
            .unwrap();
    }
    let mut lir = function.lower_lir(LoweringMode::Profiled).unwrap();
    lir.blocks[0]
        .instructions
        .retain(|i| !matches!(i.operation, Operation::GuardInt { .. }));
    assert!(lir.verify().is_err());
    let mut f = Function::new(0);
    let entry = f.entry();
    let other = f.block(0);
    let value = f.append(other, Op::Constant(Constant::Int(1)));
    f.terminate(other, Terminator::Return(value));
    f.terminate(entry, Terminator::Jump(Edge::new(other, vec![])));
    f.append(entry, Op::Add(value, value));
    assert!(f.analyze().is_err());
}

#[test]
fn nbody_hir_lowers_without_executing_python_or_using_bytecode_as_input() {
    let program = compile(include_str!("fixtures/game_nbody.py")).unwrap();
    for name in program.names() {
        let f = program.function(name).unwrap();
        f.lower_lir(LoweringMode::Generic)
            .unwrap()
            .verify()
            .unwrap();
    }
}

#[test]
fn forward_layout_edges_can_still_be_loop_backedges() {
    let mut f = Function::new(0);
    let entry = f.entry();
    let body = f.block(0);
    let head = f.block(1);
    let exit = f.block(0);
    let zero = f.append(entry, Op::Constant(Constant::Int(0)));
    f.terminate(entry, Terminator::Jump(Edge::new(head, vec![zero])));
    let n = f.parameters(head)[0];
    let limit = f.append(head, Op::Constant(Constant::Int(10)));
    let condition = f.append(head, Op::Less(n, limit));
    f.terminate(
        head,
        Terminator::Branch {
            condition,
            yes: Edge::new(body, vec![]),
            no: Edge::new(exit, vec![]),
        },
    );
    let one = f.append(body, Op::Constant(Constant::Int(1)));
    let next = f.append(body, Op::Add(n, one));
    f.terminate(body, Terminator::Jump(Edge::new(head, vec![next])));
    f.terminate(exit, Terminator::Return(n));
    assert!(body.0 < head.0);
    assert!(f.analyze().unwrap().is_backedge(body, head));
    let mut vm = Vm::new();
    vm.profiling = tis::profile::ProfileConfig::eager();
    assert_eq!(vm.run(&mut f, &[]).unwrap(), Value::Int(10));
    assert_eq!(f.profile().backedges.get(&(body.0, head.0)), Some(&10));
    f.lower_lir(LoweringMode::Profiled).unwrap();
}

#[test]
fn call_profiles_are_bounded_and_native_methods_keep_receiver_kinds() {
    let mut program=compile("functions = [lambda: 1, lambda: 2, lambda: 3, lambda: 4, lambda: 5, lambda: 6]\nfor f in functions:\n    f()\nfor value in [[], {}]:\n    value.copy()\n").unwrap();
    let mut vm = Vm::new();
    vm.profiling = tis::profile::ProfileConfig::eager();
    vm.run_program(&mut program).unwrap();
    let profile = program.function("<module>").unwrap().profile();
    let site = profile
        .sites
        .iter()
        .find(|site| site.other_targets > 0)
        .unwrap();
    assert_eq!(site.call_targets.len(), 4);
    assert_eq!(site.other_targets, 2);
    let method = profile
        .sites
        .iter()
        .find(|site| {
            site.call_targets.contains_key(&CallTarget::Method {
                receiver: ValueKind::List,
                name: "copy".into(),
            })
        })
        .unwrap();
    assert!(method.call_targets.contains_key(&CallTarget::Method {
        receiver: ValueKind::Dict,
        name: "copy".into()
    }));
}

#[test]
fn stale_lir_cannot_resume_changed_hir() {
    let (mut f, _) = add();
    let lir = f.lower_lir(LoweringMode::Generic).unwrap();
    let state = if let Operation::Call { state, .. } = lir.blocks[0].instructions[0].operation {
        state
    } else {
        unreachable!()
    };
    let frame = lir
        .reconstruct(state, Value::None, &[], |_| Ok(Value::Int(1)))
        .unwrap();
    f.terminate(f.entry(), Terminator::Return(f.parameters(f.entry())[0]));
    let mut vm = Vm::new();
    vm.profiling = tis::profile::ProfileConfig::eager();
    assert!(vm.resume_hir(&mut f, frame).is_err());
    assert_eq!(vm.stack_len(), 0);
    assert!(lir.resume_offset(state, f.bytecode().unwrap()).is_err());
}
