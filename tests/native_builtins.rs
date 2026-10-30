use tis::{BytecodeMode, Tiering, Vm, compile};

fn machine(tier: Tiering) -> Vm {
    let mut vm = Vm::new();
    vm.tiering = tier;
    vm.thresholds.function_entries = 2;
    vm.thresholds.loop_backedges = 4;
    vm.profiling.window_ticks = 16;
    vm
}

#[test]
fn native_builtin_loops_keep_exact_backedge_counts() {
    let source = "def f(n):\n    total=0\n    for i in range(n):\n        if i % 2:\n            continue\n        total+=i\n    return total\nfor i in range(8):\n    assert f(100)==2450\n";
    let mut reference = machine(Tiering::Bytecode);
    reference
        .run_program(&mut compile(source).unwrap())
        .unwrap();
    let expected = reference
        .function_hir(reference.get_global("f").unwrap())
        .unwrap()
        .profile()
        .backedges;
    for encoding in [BytecodeMode::Register, BytecodeMode::Stack] {
        let mut vm = machine(Tiering::Native);
        vm.bytecode = encoding;
        vm.run_program(&mut compile(source).unwrap()).unwrap();
        let actual = vm
            .function_hir(vm.get_global("f").unwrap())
            .unwrap()
            .profile()
            .backedges;
        assert_eq!(actual, expected);
        assert!(vm.tier_stats.native_range_steps > 0);
        assert_eq!(vm.tier_stats.compilation_errors, 0);
    }
}

#[test]
fn range_cursor_aliases_extreme_steps_exhaustion_and_errors_agree() {
    let source = r#"
def collect(it, skip):
    result=[]
    for value in it:
        result.append(value)
        if skip:
            next(it, None)
    return result
for i in range(20):
    assert collect(iter(range(12)), False)==list(range(12))
assert collect(iter(range(12)), True)==[0,2,4,6,8,10]
assert collect(iter(range(12,0,-2)), True)==[12,8,4]
assert collect(iter(range(-9223372036854775808,9223372036854775807,9223372036854775807)),False)==[-9223372036854775808,-1,9223372036854775806]
it=iter(range(3))
assert collect(it,False)==[0,1,2]
assert next(it,99)==99 and next(it,42)==42
it=iter(range(5))
try:
    for value in it:
        if value==2: raise ValueError('stop')
except ValueError:
    pass
assert next(it)==3
class Custom:
    def __init__(self): self.n=0
    def __iter__(self): return self
    def __next__(self):
        if self.n==3: raise StopIteration()
        self.n+=1
        return self.n
assert collect(Custom(),False)==[1,2,3]
"#;
    for tier in [
        Tiering::Bytecode,
        Tiering::Lir,
        Tiering::Native,
        Tiering::CopyPatch,
    ] {
        let mut vm = machine(tier);
        vm.run_program(&mut compile(source).unwrap()).unwrap();
        assert_eq!(vm.tier_stats.compilation_errors, 0);
    }
}

#[test]
fn native_list_values_aliases_resize_gc_and_custom_fallbacks_agree() {
    let source = r#"
def store(xs,i,value):
    xs[i]=value
    return xs[i]
def read(xs,i): return xs[i]
a=[0,0]
for i in range(30):
    assert store(a,0,i)==i
    assert read(a,0)==i
values=[None,True,False,9223372036854775807,-9223372036854775808,-0.0,1.25,'é☃','a long heap allocated string',[1,2]]
for value in values:
    assert store(a,-1,value) is value
    assert read(a,-1) is value
for i in range(5000):
    a.append(i)
assert read(a,-1)==4999
assert store(a,-1,42)==42
for i in range(5000): a.pop()
assert read(a,-1)==[1,2]
class Index:
    def __index__(self):
        for i in range(2000): garbage=[i]
        return -1
assert store(a,Index(),[9])==[9]
assert read(a,Index())==[9]
assert read(a,True)==[9]
assert read((3,4),-1)==4
assert read([1.0,2.0],-1)==2.0
assert read('é☃',-1)=='☃'
assert read({'x':3},'x')==3
class Custom:
    def __getitem__(self,key): return key+10
    def __setitem__(self,key,value): self.saved=(key,value)
c=Custom()
assert read(c,2)==12
store(c,2,5)
assert c.saved==(2,5)
for bad in [2,-3]:
    try: store(a,bad,99)
    except IndexError: pass
    else: assert False
assert a==[29,[9]]
# A promoted float list must invalidate old boxed pointers before the next store.
f=[1.0,2.0]
assert store(f,0,'new')=='new'
assert store(f,1,False) is False
assert f==['new',False]
"#;
    for tier in [
        Tiering::Bytecode,
        Tiering::Lir,
        Tiering::Native,
        Tiering::CopyPatch,
    ] {
        let mut vm = machine(tier);
        vm.run_program(&mut compile(source).unwrap()).unwrap();
        if tier == Tiering::Native {
            assert!(vm.tier_stats.native_list_stores > 0);
            assert!(vm.tier_stats.native_sequence_loads > 0);
        }
        assert_eq!(vm.tier_stats.compilation_errors, 0);
        vm.collect_garbage(&[]).unwrap();
    }
}

#[test]
fn unseen_sequence_blocks_use_guarded_reads_and_foreign_kinds_miss() {
    let source = r#"
def cold(xs, active):
    if active: return xs[-1]
    return 0
for i in range(40): assert cold([],False)==0
assert cold([7,8],True)==8
assert cold((9,10),True)==10
class C:
    def __init__(self): self.field=7
def field(x): return x.field
c=C()
for i in range(40): assert field(c)==7
for x in [range(4), iter(range(4)), [1,2], (1,2)]:
    try: field(x)
    except AttributeError: pass
    else: assert False
"#;
    let mut vm = machine(Tiering::Native);
    vm.run_program(&mut compile(source).unwrap()).unwrap();
    assert!(vm.tier_stats.native_sequence_loads >= 2);
    assert_eq!(vm.tier_stats.compilation_errors, 0);
}
