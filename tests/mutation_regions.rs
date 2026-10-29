use tis::{BytecodeMode, Tiering, Value, Vm, compile};
fn warm(source: &str, mode: BytecodeMode) -> Vm {
    let mut vm = Vm::new();
    vm.tiering = Tiering::Native;
    vm.mutation_regions = true;
    vm.bytecode = mode;
    vm.capture_native_code = true;
    vm.thresholds.function_entries = 16;
    vm.thresholds.loop_backedges = 32;
    vm.run_program(&mut compile(source).unwrap()).unwrap();
    vm
}
const SOURCE: &str = "class Box:\n    def __init__(self):\n        self.x=0.0\n        self.y=0.0\n    def step(self,divisor):\n        self.x+=1.0\n        self.y=10.0/divisor\n        self.x+=self.y\nbox=Box()\nbound=box.step\nfor i in range(500):\n    bound(2.0)\n";
#[test]
fn buffered_stores_complete_without_inner_callbacks() {
    for mode in [BytecodeMode::Stack, BytecodeMode::Register] {
        let mut vm = warm(SOURCE, mode);
        let bound = vm.get_global("bound").unwrap();
        assert!(vm.tier_stats.mutation_entries > 0, "{:?}", vm.tier_stats);
        let tis::Object::Method(function, _) = vm.heap.get(bound).unwrap() else {
            panic!("bound method");
        };
        let clif = vm
            .native_diagnostics(*function)
            .unwrap()
            .mutation_clif
            .as_ref()
            .unwrap();
        assert!(!clif.contains("call "));
        let before = vm.tier_stats.clone();
        let allocations = vm.heap.allocation_count();
        assert_eq!(vm.call(bound, &[Value::Float(2.0)]).unwrap(), Value::None);
        assert_eq!(vm.tier_stats.mutation_entries, before.mutation_entries + 1);
        assert_eq!(vm.tier_stats.mutation_writes, before.mutation_writes + 3);
        assert_eq!(
            vm.tier_stats.native_runtime_calls,
            before.native_runtime_calls
        );
        assert_eq!(vm.heap.allocation_count(), allocations);
        vm.run_program(&mut compile("assert box.x==3006.0\nassert box.y==5.0\n").unwrap())
            .unwrap();
    }
}
#[test]
fn guard_exit_commits_prefix_then_resumes_without_repeating_writes() {
    for mode in [BytecodeMode::Stack, BytecodeMode::Register] {
        let mut vm = warm(SOURCE, mode);
        let bound = vm.get_global("bound").unwrap();
        let before = vm.tier_stats.mutation_guard_exits;
        let error = vm.call(bound, &[Value::Float(0.0)]).unwrap_err();
        assert!(error.to_string().contains("ZeroDivisionError"), "{error}");
        assert_eq!(vm.tier_stats.mutation_guard_exits, before + 1);
        let tis::Object::Method(function, _) = vm.heap.get(bound).unwrap() else {
            panic!("bound method");
        };
        let profile = vm.function_hir(*function).unwrap().profile();
        assert!(profile.sites.iter().any(|site| {
            site.exit_reasons
                .contains_key(&tis::lir::ExitReason::FloatArithmetic)
        }));

        vm.run_program(&mut compile("assert box.x==3001.0\nassert box.y==5.0\n").unwrap())
            .unwrap();
        vm.call(bound, &[Value::Float(2.0)]).unwrap();
        vm.run_program(&mut compile("assert box.x==3007.0\n").unwrap())
            .unwrap();
        assert_eq!(vm.stack_len(), 0);
    }
}
#[test]
fn field_reads_forward_across_aliased_receivers() {
    let source = "class Point:\n    def __init__(self):\n        self.x=2.0\nclass Box:\n    def __init__(self):\n        self.left=Point()\n        self.right=self.left\n        self.out=0.0\n    def step(self):\n        self.left.x+=1.0\n        self.right.x*=self.left.x\n        self.out=self.right.x\nbox=Box()\nbound=box.step\nfor i in range(500):\n    bound()\nbox.left.x=2.0\n";
    let mut vm = warm(source, BytecodeMode::Register);
    let bound = vm.get_global("bound").unwrap();
    let before = vm.tier_stats.mutation_entries;
    vm.call(bound, &[]).unwrap();
    assert_eq!(vm.tier_stats.mutation_entries, before + 1);
    vm.run_program(
        &mut compile("assert box.left is box.right\nassert box.left.x==9.0\nassert box.out==9.0\n")
            .unwrap(),
    )
    .unwrap();
}

#[test]
fn branches_forward_only_executed_writes_and_resume_the_chosen_path() {
    let source = "class Box:\n    def __init__(self):\n        self.x=0.0\n        self.y=0.0\n    def step(self,flag,divisor):\n        self.x+=1.0\n        if flag:\n            self.x+=2.0\n        else:\n            self.x+=4.0\n        self.y=self.x/divisor\nbox=Box()\nbound=box.step\nfor i in range(500):\n    bound(i%2==0,2.0)\n";
    for mode in [BytecodeMode::Stack, BytecodeMode::Register] {
        let mut vm = warm(source, mode);
        let bound = vm.get_global("bound").unwrap();
        let before = vm.tier_stats.mutation_entries;
        vm.call(bound, &[Value::Bool(true), Value::Float(2.0)])
            .unwrap();
        assert_eq!(vm.tier_stats.mutation_entries, before + 1);
        vm.run_program(&mut compile("assert box.x==2003.0\nassert box.y==1001.5\n").unwrap())
            .unwrap();
        let before = vm.tier_stats.mutation_guard_exits;
        assert!(
            vm.call(bound, &[Value::Bool(false), Value::Float(0.0)])
                .is_err()
        );
        assert_eq!(vm.tier_stats.mutation_guard_exits, before + 1);
        vm.run_program(&mut compile("assert box.x==2008.0\nassert box.y==1001.5\n").unwrap())
            .unwrap();
    }
}

#[test]
fn unsupported_store_values_resume_after_the_prior_increment() {
    let source = "class Box:\n    def __init__(self):\n        self.x=0.0\n        self.y=0.0\n    def step(self,value):\n        updated=self.x+1.0\n        self.x=updated\n        self.y=value\nbox=Box()\nbound=box.step\nfor i in range(500):\n    bound(1.0)\n";
    for mode in [BytecodeMode::Stack, BytecodeMode::Register] {
        let mut vm = warm(source, mode);
        let bound = vm.get_global("bound").unwrap();
        let child = vm
            .heap
            .allocate(tis::Object::List(vec![Value::Int(42)].into()));
        let exits = vm.tier_stats.mutation_guard_exits;
        vm.call(bound, &[child]).unwrap();
        assert_eq!(vm.tier_stats.mutation_guard_exits, exits + 1);
        vm.collect_garbage(&[]).unwrap();
        assert_eq!(
            vm.heap.get_item(child, Value::Int(0)).unwrap(),
            Value::Int(42)
        );
        vm.run_program(&mut compile("assert box.x==501.0\nassert box.y==[42]\n").unwrap())
            .unwrap();
        vm.call(bound, &[Value::Bool(true)]).unwrap();
        vm.run_program(&mut compile("assert box.x==502.0\nassert box.y is True\n").unwrap())
            .unwrap();
        vm.call(bound, &[Value::Float(2.0)]).unwrap();
        vm.run_program(&mut compile("assert box.x==503.0\nassert box.y==2.0\n").unwrap())
            .unwrap();
    }
}

#[test]
fn barnes_mutation_regions_match_bytecode_after_collection() {
    let source = include_str!("fixtures/barnes_hut.py")
        .rsplit_once("\nprint(f'")
        .unwrap()
        .0;
    let mut reference = Vm::new();
    reference
        .run_program(&mut compile(source).unwrap())
        .unwrap();
    let args = [
        Value::Int(1),
        Value::Int(32),
        Value::Int(8),
        Value::Float(0.5),
    ];
    let function = reference.get_global("bench_quadtree_nbody").unwrap();
    let expected = reference.call(function, &args).unwrap();
    for mode in [BytecodeMode::Stack, BytecodeMode::Register] {
        let mut vm = warm(source, mode);
        vm.readonly_regions = true;
        let function = vm.get_global("bench_quadtree_nbody").unwrap();
        for _ in 0..3 {
            assert_eq!(vm.call(function, &args).unwrap(), expected);
        }
        vm.collect_garbage(&[]).unwrap();
        assert_eq!(vm.call(function, &args).unwrap(), expected);
        assert!(vm.tier_stats.mutation_entries > 0);
        assert!(vm.tier_stats.readonly_entries > 0);
        assert_eq!(vm.tier_stats.compilation_errors, 0);
        assert_eq!(vm.stack_len(), 0);
    }
}

#[test]
fn unknown_calls_and_unpromoted_locals_stay_on_the_existing_backend() {
    let source = "events=[]\nclass Box:\n    def __init__(self):\n        self.x=0.0\n    def step(self):\n        self.x+=1.0\n        events.append(self.x)\nbox=Box()\nfor i in range(500):\n    box.step()\nassert box.x==500.0\nassert len(events)==500\n";
    let vm = warm(source, BytecodeMode::Register);
    assert_eq!(vm.tier_stats.mutation_entries, 0);
    let mut vm = Vm::new();
    vm.tiering = Tiering::Native;
    vm.mutation_regions = true;
    vm.thresholds.function_entries = 16;
    let mut program = tis::compile_with_options(
        SOURCE,
        tis::CompileOptions {
            promote_locals: false,
        },
    )
    .unwrap();
    vm.run_program(&mut program).unwrap();
    assert_eq!(vm.tier_stats.mutation_entries, 0);
    vm.run_program(&mut compile("assert box.x==3000.0\n").unwrap())
        .unwrap();
}
