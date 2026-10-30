use tis::{
    Object, Tiering, Value, Vm, compile,
    hir::{Constant, Edge, Function, Op, Terminator},
    profile::HotThresholds,
};
fn warm(source: &str) -> Vm {
    warm_with_window(source, 8)
}
fn warm_with_window(source: &str, ticks: u32) -> Vm {
    let mut program = compile(source).unwrap();
    let mut vm = Vm::new();
    vm.profiling.window_ticks = ticks;
    vm.tiering = Tiering::CopyPatch;
    vm.thresholds = HotThresholds {
        function_entries: 12,
        loop_backedges: 16,
    };
    vm.run_program(&mut program).unwrap();
    vm
}
#[test]
fn copied_stencils_execute_without_cranelift() {
    let vm =
        warm("def add(a,b):\n    return a + b\nfor i in range(30):\n    assert add(i,1)==i+1\n");
    assert!(vm.tier_stats.copy_patch_entries > 0);
    assert!(vm.tier_stats.copy_patch_stencils > 0);
    assert!(vm.tier_stats.copy_patch_relocations > 0);
    assert!(vm.tier_stats.copy_patch_code_bytes > 0);
    assert!(vm.tier_stats.copy_patch_data_bytes > 0);
    assert_eq!(vm.tier_stats.native_compilations, 0);
}
#[test]
fn type_and_overflow_exits_do_not_repeat_effects() {
    let mut vm = warm(
        "calls = [0]\ndef add(a,b):\n    calls[0] += 1\n    return a+b\nfor i in range(30):\n    add(i,1)\n",
    );
    let add = vm.get_global("add").unwrap();
    let a = vm.heap.allocate(Object::String("a".into()));
    let b = vm.heap.allocate(Object::String("b".into()));
    let value = vm.call(add, &[a, b]).unwrap();
    assert_eq!(vm.heap.as_str(&(value)).unwrap(), "ab");
    let calls = vm.get_global("calls").unwrap();
    assert_eq!(
        vm.heap.get_item(calls, Value::Int(0)).unwrap(),
        Value::Int(31)
    );
    let hir = vm.function_hir(add).unwrap();
    vm.compile_lir(&hir).unwrap();
    let large = vm
        .call(add, &[Value::Int(i64::MAX), Value::Int(1)])
        .unwrap();
    assert!(
        matches!(vm.heap.get(large),Ok(tis::Object::BigInt(n)) if n.to_string()=="9223372036854775808")
    );
    assert_eq!(
        vm.heap.get_item(calls, Value::Int(0)).unwrap(),
        Value::Int(32)
    );
    assert_eq!(vm.stack_len(), 0);
    assert!(vm.tier_stats.deoptimizations > 0);
}
#[test]
fn float_stencils_share_scalar_semantics() {
    let mut vm = warm(
        "def calc(a,b):\n    return (a+b)*2.0/(a-b)\ndef power(a,b):\n    return a**b\ndef less(a,b):\n    return a<b\ndef unequal(a,b):\n    return a!=b\nfor i in range(30):\n    assert calc(4.0,2.0)==6.0\n    assert power(4.0,0.5)==2.0\n    assert less(1.0,2.0)\n    assert unequal(1.0,2.0)\nassert not less(float('nan'),2.0)\nassert unequal(float('nan'),2.0)\n",
    );
    assert!(
        vm.call(
            vm.get_global("calc").unwrap(),
            &[Value::Float(2.0), Value::Float(2.0)]
        )
        .is_err()
    );
    assert!(
        vm.call(
            vm.get_global("power").unwrap(),
            &[Value::Float(0.0), Value::Float(-1.0)]
        )
        .is_err()
    );
    assert!(vm.tier_stats.deoptimizations >= 2);
}
#[test]
fn language_fixture_uses_copied_code() {
    let vm = warm(include_str!("fixtures/language.py"));
    assert_eq!(vm.output, "language core ok\n");
    assert!(vm.tier_stats.copy_patch_entries > 0);
    assert_eq!(vm.tier_stats.native_compilations, 0);
}
#[test]
fn root_maps_and_nested_calls_keep_heap_values_alive() {
    let mut vm = warm(
        "def churn():\n    for i in range(6000):\n        temp = [i]\ndef f(x):\n    y = [x]\n    churn()\n    return y[0]\nfor i in range(20):\n    assert f([42])[0]==42\n",
    );
    assert!(vm.tier_stats.copy_patch_entries > 0);
    assert!(vm.tier_stats.copy_patch_osr_entries > 0);
    assert_eq!(vm.stack_len(), 0);
    vm.collect_garbage(&[]).unwrap();
}
#[test]
fn parallel_edge_moves_use_scratch_slots() {
    let mut f = Function::new(1);
    let entry = f.entry();
    let input = f.parameters(entry)[0];
    let head = f.block(3);
    let body = f.block(0);
    let exit = f.block(0);
    let zero = f.append(entry, Op::Constant(Constant::Int(0)));
    let one = f.append(entry, Op::Constant(Constant::Int(1)));
    f.terminate(
        entry,
        Terminator::Jump(Edge::new(head, vec![input, zero, one])),
    );
    let p = f.parameters(head).to_vec();
    let condition = f.append(head, Op::Less(zero, p[0]));
    f.terminate(
        head,
        Terminator::Branch {
            condition,
            yes: Edge::new(body, vec![]),
            no: Edge::new(exit, vec![]),
        },
    );
    let next = f.append(body, Op::Subtract(p[0], one));
    f.terminate(
        body,
        Terminator::Jump(Edge::new(head, vec![next, p[2], p[1]])),
    );
    f.terminate(exit, Terminator::Return(p[1]));
    let mut vm = Vm::new();
    vm.tiering = Tiering::CopyPatch;
    vm.compile_lir(&f).unwrap();
    assert_eq!(vm.run(&mut f, &[Value::Int(1001)]).unwrap(), Value::Int(1));
    assert!(vm.tier_stats.copy_patch_entries > 0);
}
#[test]
fn tail_threading_does_not_grow_the_stack_per_iteration() {
    std::thread::Builder::new()
        .stack_size(256 * 1024)
        .spawn(|| {
            let vm = warm("x=0\nfor i in range(100000):\n    x=x+1\nassert x==100000\n");
            assert!(vm.tier_stats.copy_patch_osr_entries > 0);
        })
        .unwrap()
        .join()
        .unwrap();
}
#[test]
fn recursive_retirement_preserves_active_stencil_images() {
    let mut vm = warm_with_window(
        "def f(n,x):\n    if n>0:\n        return f(n-1,x)+x\n    return x+x\nfor i in range(20):\n    f(4,1)\n",
        32,
    );
    assert_eq!(
        vm.call(
            vm.get_global("f").unwrap(),
            &[Value::Int(2), Value::Float(0.5)]
        )
        .unwrap(),
        Value::Float(2.0)
    );
    assert!(vm.tier_stats.retired > 0);
    assert_eq!(vm.stack_len(), 0);
}
#[test]
fn helper_panics_stay_inside_the_shared_gateway() {
    let mut vm = warm("def repeat(n):\n    return 'abc'*n\nfor i in range(20):\n    repeat(1)\n");
    let f = vm.get_global("repeat").unwrap();
    assert!(
        vm.call(f, &[Value::Int(i64::MAX)])
            .unwrap_err()
            .to_string()
            .contains("panic in native runtime helper")
    );
    assert_eq!(vm.stack_len(), 0);
    vm.collect_garbage(&[]).unwrap();
    let value = vm.call(f, &[Value::Int(2)]).unwrap();
    assert_eq!(vm.heap.as_str(&(value)).unwrap(), "abcabc");
}
#[test]
fn copy_patch_nbody_acceptance() {
    let mut vm = warm_with_window(include_str!("fixtures/game_nbody.py"), 128);
    assert_eq!(vm.output, "-0.169075164\n-0.169089263\n");
    assert!(vm.tier_stats.copy_patch_osr_entries > 0);
    assert_eq!(vm.tier_stats.native_compilations, 0);
    assert_eq!(vm.tier_stats.compilation_errors, 0);
    vm.collect_garbage(&[]).unwrap();
    assert!(vm.heap.allocation_count() < 300);
}
