use tis::{
    BytecodeMode, NumericMode, Tiering, Value, Vm, compile,
    profile::{HotThresholds, ProfileConfig, ProfilePhase},
};

fn vm(encoding: BytecodeMode) -> Vm {
    let mut vm = Vm::new();
    vm.bytecode = encoding;
    vm.thresholds = HotThresholds {
        function_entries: 0,
        loop_backedges: 4,
    };
    vm.profiling.window_ticks = 8;
    vm
}

#[test]
fn completed_windows_still_discover_later_hot_loops() {
    let source = "def f():\n    for i in range(100):\n        first=i\n    for j in range(100):\n        second=j\n    return first+second\nassert f()==198\n";
    for encoding in [BytecodeMode::Register, BytecodeMode::Stack] {
        let mut vm = vm(encoding);
        vm.run_program(&mut compile(source).unwrap()).unwrap();
        let f = vm.get_global("f").unwrap();
        let id = vm.function_hir(f).unwrap().id();
        let requests = vm
            .take_compilation_requests()
            .into_iter()
            .filter(|r| r.function == id)
            .collect::<Vec<_>>();
        assert_eq!(requests.len(), 2);
        let profile = vm.function_hir(f).unwrap().profile();
        assert_eq!(profile.backedges.values().sum::<u64>(), 200);
        assert_eq!(profile.phase, ProfilePhase::Ready);
    }
}

#[test]
fn ready_code_keeps_vm_local_requests_and_numeric_and_tier_switches() {
    let mut program = compile("for i in range(100):\n    total=i+1\n").unwrap();
    let mut first = vm(BytecodeMode::Register);
    first.run_program(&mut program).unwrap();
    assert!(!first.take_compilation_requests().is_empty());
    let mut second = vm(BytecodeMode::Register);
    second.run_program(&mut program).unwrap();
    assert!(!second.take_compilation_requests().is_empty());

    let mut vm = vm(BytecodeMode::Register);
    vm.numeric = NumericMode::Disabled;
    vm.run_program(&mut compile("def f(n):\n    total=0\n    for i in range(n):\n        total=total+i\n    return total\nassert f(100)==4950\n").unwrap()).unwrap();
    let f = vm.get_global("f").unwrap();
    let before = vm.tier_stats.numeric_fast_paths;
    vm.numeric = NumericMode::Adaptive;
    assert_eq!(vm.call(f, &[Value::Int(100)]).unwrap(), Value::Int(4950));
    assert!(vm.tier_stats.numeric_fast_paths > before);
    vm.tiering = Tiering::Native;
    assert_eq!(vm.call(f, &[Value::Int(100)]).unwrap(), Value::Int(4950));
    assert!(
        vm.tier_stats.native_entries + vm.tier_stats.native_osr_entries > 0,
        "{:?}",
        vm.tier_stats
    );
    assert_eq!(
        vm.function_hir(f)
            .unwrap()
            .profile()
            .backedges
            .values()
            .sum::<u64>(),
        300
    );
}

#[test]
fn fused_iterators_keep_gc_control_and_eager_feedback() {
    let source = r#"
class Iter:
    def __init__(self): self.n=0
    def __iter__(self): return self
    def __next__(self):
        if self.n==40: raise StopIteration()
        value=[self.n, self.n+1]
        self.n+=1
        for i in range(2000): garbage=[i]
        return value
out=[]
for a,b in Iter():
    if a==10: continue
    if a==30: break
    out.append(a+b)
assert len(out)==29 and out[0]==1 and out[-1]==59
out=[]
for i in range(5, -1, -2): out.append(i)
else: out.append(99)
assert out==[5,3,1,99]
assert list('é😀')==['é','😀']
assert list('a longer heap string')==list('a longer heap string')
def fail(it):
    for x in it:
        if x==2: raise ValueError('stop')
it=iter(range(5))
try: fail(it)
except ValueError: pass
assert next(it)==3
"#;
    for encoding in [BytecodeMode::Register, BytecodeMode::Stack] {
        for eager in [false, true] {
            let mut vm = vm(encoding);
            if eager {
                vm.profiling = ProfileConfig::eager();
            }
            vm.run_program(&mut compile(source).unwrap()).unwrap();
            if eager || encoding == BytecodeMode::Stack {
                assert_eq!(vm.tier_stats.bytecode_fused_iterations, 0);
            } else {
                assert!(vm.tier_stats.bytecode_fused_iterations > 0);
            }
            vm.collect_garbage(&[]).unwrap();
        }
    }
}

#[test]
fn truth_callbacks_reopen_feedback_before_the_next_block() {
    let source = r#"
def calc(x,flag):
    value=x+1
    if flag: return value
    return value
for i in range(40): assert calc(i,False)==i+1
class Reenter:
    def __bool__(self): return calc(1.5,False)==2.5
assert calc(2,Reenter())==3
"#;
    let mut profiles = vec![];
    for encoding in [BytecodeMode::Register, BytecodeMode::Stack] {
        let mut vm = vm(encoding);
        vm.thresholds.function_entries = 4;
        vm.run_program(&mut compile(source).unwrap()).unwrap();
        profiles.push(
            vm.function_hir(vm.get_global("calc").unwrap())
                .unwrap()
                .profile(),
        );
    }
    assert_eq!(profiles[0].windows, profiles[1].windows);
    assert!(profiles[0].windows >= 2);
    assert_eq!(profiles[0].blocks, profiles[1].blocks);
    assert_eq!(profiles[0].edges, profiles[1].edges);
}
