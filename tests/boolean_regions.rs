use tis::{Object, Tiering, Vm, compile, profile::HotThresholds};

fn warm_with_window(source: &str, ticks: u32, tier: Tiering) -> Vm {
    let mut vm = Vm::new();
    vm.tiering = tier;
    vm.thresholds = HotThresholds {
        function_entries: 12,
        loop_backedges: 16,
    };
    vm.profiling.window_ticks = ticks;
    vm.run_program(&mut compile(source).unwrap()).unwrap();
    vm
}

#[test]
fn bool_fill_regions_preserve_bounds_snapshots_and_loop_state() {
    for tier in [Tiering::CopyPatch, Tiering::Native] {
        let vm = warm_with_window(
            r#"
def fill(a,start,stop,step,value):
    last=77
    for i in range(start,stop,step):
        a[i]=False
    return i if start!=stop else last
for repeat in range(100):
    a=[True]*40
    assert fill(a,1,39,3,False)==37
    assert a[1] is False
    assert a[2] is True
    assert a[37] is False
    assert fill(a,0,0,1,False)==77
b=[True]*8
assert str(b)=='[True, True, True, True, True, True, True, True]'
assert fill(b,-1,-9,-2,False)==-7
assert b==[True,False,True,False,True,False,True,False]
c=[True]*5
try:
    fill(c,0,8,1,False)
    assert False
except IndexError:
    pass
assert c==[False]*5
# Crossing zero normalizes negative indices individually.
d=[True]*5
assert fill(d,-3,3,1,False)==2
assert d==[False]*5
# Boxed storage retains ordinary mutation/promotion semantics.
e=[True,1,True]
assert fill(e,0,3,1,False)==2
assert e==[False]*3
"#,
            128,
            tier,
        );
        assert!(vm.tier_stats.native_region_entries > 0);
        assert!(vm.tier_stats.native_region_iterations > 100);
        assert_eq!(
            vm.tier_stats.native_compilations == 0,
            tier == Tiering::CopyPatch
        );
        assert_eq!(vm.stack_len(), 0);
    }
}

#[test]
fn bool_fill_regions_keep_osr_backedges_and_unbound_targets() {
    fn run(tier: Tiering) -> (std::collections::BTreeMap<(usize, usize), u64>, Vm) {
        let mut program = compile(
            r#"
def fill(a,n):
    for i in range(n):
        a[i]=True
    return i
a=[False]*24
for repeat in range(60):
    assert fill(a,24)==23
try:
    fill(a,0)
    assert False
except UnboundLocalError:
    pass
"#,
        )
        .unwrap();
        let mut vm = Vm::new();
        vm.tiering = tier;
        vm.thresholds = HotThresholds {
            function_entries: 8,
            loop_backedges: 8,
        };
        vm.profiling.window_ticks = 32;
        vm.run_program(&mut program).unwrap();
        let function = match vm.heap.get(vm.get_global("fill").unwrap()).unwrap() {
            Object::Function(c) => c.function.clone(),
            _ => panic!(),
        };
        let edges = function.borrow().profile().backedges;
        (edges, vm)
    }
    let (expected, _) = run(Tiering::Bytecode);
    for tier in [Tiering::CopyPatch, Tiering::Native] {
        let (actual, vm) = run(tier);
        assert_eq!(actual, expected, "{tier:?}");
        assert!(vm.tier_stats.native_region_iterations > 0, "{tier:?}");
        assert_eq!(vm.stack_len(), 0);
    }
}

#[test]
fn bool_fill_regions_keep_partial_iterators_and_custom_store_effects() {
    for tier in [Tiering::CopyPatch, Tiering::Native] {
        let mut vm = warm_with_window(
            r#"
def fill_it(a,it):
    for i in it:
        a[i]=False
    return i
for repeat in range(100):
    a=[True]*8
    assert fill_it(a,iter(range(5)))==4
it=iter(range(7))
assert next(it)==0
b=[True]*8
assert fill_it(b,it)==6
assert b==[True,False,False,False,False,False,False,True]
try:
    next(it)
    assert False
except StopIteration:
    pass
class Custom:
    def __init__(self):
        self.keys=[]
    def __setitem__(self,k,v):
        self.keys.append(k)
c=Custom()
assert fill_it(c,iter(range(-2,2)))==1
assert c.keys==[-2,-1,0,1]
"#,
            128,
            tier,
        );
        assert!(vm.tier_stats.native_region_entries > 0);
        let before = vm.tier_stats.native_region_iterations;
        vm.profiling = tis::profile::ProfileConfig::eager();
        let mut check = compile("assert fill_it([True]*4,iter(range(4)))==3\n").unwrap();
        vm.run_program(&mut check).unwrap();
        assert_eq!(vm.tier_stats.native_region_iterations, before);
        vm.collect_garbage(&[]).unwrap();
        assert_eq!(vm.stack_len(), 0);
    }
}

#[test]
fn bool_reductions_preserve_values_bounds_and_fallback_effects() {
    for tier in [Tiering::CopyPatch, Tiering::Native] {
        let mut vm = warm_with_window(
            r#"
def reduce_it(a,it,count,total,last):
    for i in it:
        if a[i]:
            count+=1
            total+=i
            last=i
    return (count,total,last,i)
a=[False,True,False,True,True]
for repeat in range(100):
    assert reduce_it(a,iter(range(5)),0,0,None)==(3,8,4,4)
assert reduce_it(a,iter(range(-1,-6,-1)),0,0,None)==(3,-7,-4,-5)
sentinel=[]
assert reduce_it([False]*5,iter(range(5)),0,0,sentinel)[2] is sentinel
it=iter(range(5))
assert next(it)==0
assert reduce_it(a,it,0,0,None)==(3,8,4,4)
try:
    next(it)
    assert False
except StopIteration:
    pass
try:
    reduce_it(a,iter(range(0)),0,0,None)
    assert False
except UnboundLocalError:
    pass
assert reduce_it([True]*3,iter(range(3)),9223372036854775807,0,None)==(9223372036854775810,3,2,2)
assert reduce_it([True]*3,iter(range(3)),0,9223372036854775807,None)==(3,9223372036854775810,2,2)
class Counter:
    def __init__(self):
        self.calls=0
    def __iadd__(self,value):
        self.calls+=1
        return self
c=Counter()
assert reduce_it([True,False,True],iter(range(3)),c,0,None)[0] is c
assert c.calls==2
class Source:
    def __init__(self):
        self.calls=0
    def __getitem__(self,k):
        self.calls+=1
        return True
s=Source()
assert reduce_it(s,iter(range(4)),0,0,None)==(4,6,3,3)
assert s.calls==4
s2=Source()
# Invalid bounds on an exact list must not suppress prior accumulator effects.
c2=Counter()
try:
    reduce_it([True]*3,iter(range(6)),c2,0,None)
    assert False
except IndexError:
    pass
assert c2.calls==3
# A discarded arithmetic result must retain its protocol effects.
def discarded(a,x):
    last=None
    for i in range(4):
        if a[i]:
            x+1
            last=i
    return last
for repeat in range(100):
    assert discarded([True]*4,0)==3
class Adder:
    def __init__(self):
        self.calls=0
    def __add__(self,other):
        self.calls+=1
        return 0
x=Adder()
assert discarded([True]*4,x)==3
assert x.calls==4
"#,
            128,
            tier,
        );
        assert!(vm.tier_stats.native_region_iterations > 100);
        let before = vm.tier_stats.native_region_iterations;
        vm.profiling = tis::profile::ProfileConfig::eager();
        vm.run_program(
            &mut compile("assert reduce_it([True]*4,iter(range(4)),0,0,None)==(4,6,3,3)\n")
                .unwrap(),
        )
        .unwrap();
        assert_eq!(vm.tier_stats.native_region_iterations, before);
        vm.collect_garbage(&[]).unwrap();
        assert_eq!(vm.stack_len(), 0);
    }
}

#[test]
fn bool_reduction_regions_preserve_backedge_profiles() {
    fn run(tier: Tiering) -> (std::collections::BTreeMap<(usize, usize), u64>, Vm) {
        let mut program = compile(
            r#"
def count_selected(a,n):
    count=0
    for i in range(n):
        if a[i]:
            count+=1
    return count
for repeat in range(100):
    assert count_selected([False,True]*12,24)==12
"#,
        )
        .unwrap();
        let mut vm = Vm::new();
        vm.tiering = tier;
        vm.thresholds = HotThresholds {
            function_entries: 8,
            loop_backedges: 8,
        };
        vm.profiling.window_ticks = 32;
        vm.run_program(&mut program).unwrap();
        let function = match vm
            .heap
            .get(vm.get_global("count_selected").unwrap())
            .unwrap()
        {
            Object::Function(c) => c.function.clone(),
            _ => panic!(),
        };
        let edges = function.borrow().profile().backedges;
        (edges, vm)
    }
    let (expected, _) = run(Tiering::Bytecode);
    for tier in [Tiering::CopyPatch, Tiering::Native] {
        let (actual, vm) = run(tier);
        assert_eq!(actual, expected, "{tier:?}");
        assert!(vm.tier_stats.native_region_iterations > 0, "{tier:?}");
        assert_eq!(vm.stack_len(), 0);
    }
}

#[test]
fn both_backends_run_fill_and_reduce_without_per_element_gateways() {
    for tier in [Tiering::CopyPatch, Tiering::Native] {
        let mut vm = warm_with_window(
            "def f(a,n):\n    for i in range(n):\n        a[i]=True\n    count=0\n    total=0\n    last=None\n    for i in range(n):\n        if a[i]:\n            count+=1\n            total+=i\n            last=i\n    return count,total,last\nfor repeat in range(100):\n    assert f([False]*16,16)==(16,120,15)\n",
            32,
            tier,
        );
        let f = vm.get_global("f").unwrap();
        let values = vm.heap.allocate(tis::Object::List(
            vec![tis::Value::Bool(false); 10000].into(),
        ));
        // Renew exclusive storage before measuring; a newly allocated row can otherwise
        // take one ordinary store before its bulk-write permission is published.
        vm.heap
            .set_item(values, tis::Value::Int(0), tis::Value::Bool(false))
            .unwrap();
        let before = vm.tier_stats.clone();
        let result = vm.call(f, &[values, tis::Value::Int(10000)]).unwrap();
        for (index, expected) in [10000, 49995000, 9999].into_iter().enumerate() {
            assert_eq!(
                vm.heap
                    .get_item(result, tis::Value::Int(index as i64))
                    .unwrap(),
                tis::Value::Int(expected)
            );
        }
        assert_eq!(
            vm.tier_stats.native_region_iterations - before.native_region_iterations,
            20000,
            "{tier:?}"
        );
        assert_eq!(
            vm.tier_stats.native_region_entries - before.native_region_entries,
            2,
            "{tier:?}"
        );
        assert!(
            vm.tier_stats.native_runtime_calls - before.native_runtime_calls < 30,
            "{tier:?}"
        );
        assert_eq!(vm.stack_len(), 0);
    }
}

#[test]
fn counter_fills_preserve_final_indices_bounds_and_operator_fallbacks() {
    let source = r#"
def up(a,start,stop,step):
    i=start
    while i<=stop:
        a[i]=False
        i+=step
    return i
def down(a,start,stop,step):
    i=start
    while i>=stop:
        a[i]=True
        i+=step
    return i
for repeat in range(100):
    a=[True]*40
    assert up(a,1,38,3)==40
    assert a==[i%3!=1 for i in range(40)]
    assert down(a,37,1,-3)==-2
    assert a==[True]*40
    assert up(a,5,3,1)==5
    assert up(a,-3,2,1)==3
    assert a[-3:]==[False]*3 and a[:3]==[False]*3
b=[True]*5
try:
    up(b,0,9,1)
    assert False
except IndexError:
    pass
assert b==[False]*5
class Step:
    def __radd__(self,value):
        return value+2
assert up([True]*10,0,8,Step())==10
assert up([True]*3,1,1,9223372036854775807)==9223372036854775808
"#;
    for tier in [Tiering::CopyPatch, Tiering::Native] {
        let vm = warm_with_window(source, 128, tier);
        assert!(vm.tier_stats.native_region_iterations > 1000, "{tier:?}");
        assert_eq!(vm.tier_stats.compilation_errors, 0);
        assert_eq!(vm.stack_len(), 0);
    }
}

#[test]
fn literal_stride_fill_and_boolean_count_keep_protocol_fallbacks() {
    for tier in [Tiering::CopyPatch, Tiering::Native] {
        let vm = warm_with_window(
            r#"
def fill(a,stop):
    i=1
    while i<stop:
        a[i]=False
        i+=3
    return i
for repeat in range(100):
    a=[True]*40
    assert fill(a,40)==40
    assert a.count(False)==13
    assert a.count(True)==27
    assert a.count(0)==13
calls=[]
class Equal:
    def __eq__(self,other):
        calls.append(other)
        return True
assert [True,False].count(Equal())==2
assert calls==[True,False]
b=[True,1,False]
assert b.count(True)==2
"#,
            128,
            tier,
        );
        assert!(vm.tier_stats.native_region_iterations > 1000);
        assert_eq!(vm.tier_stats.compilation_errors, 0);
    }
}

#[test]
fn affine_counter_bounds_guard_overflow_and_user_arithmetic_before_stores() {
    for tier in [Tiering::CopyPatch, Tiering::Native] {
        let vm = warm_with_window(
            r#"
def fill(a,stop):
    i=1
    while i<stop+1:
        a[i]=False
        i+=3
    return i
for repeat in range(100):
    a=[True]*40
    assert fill(a,39)==40
    assert a.count(False)==13
b=[True]*3
assert fill(b,-2)==1
assert b==[True]*3
try:
    fill(b,9223372036854775807)
    assert False
except IndexError:
    pass
assert b==[True,False,True]
events=[]
class Bound:
    def __add__(self,offset):
        assert offset==1
        events.append(1)
        return 7
c=[True]*10
assert fill(c,Bound())==7
assert events==[1,1,1]
assert c.count(False)==2
"#,
            128,
            tier,
        );
        assert!(vm.tier_stats.native_region_iterations > 1000);
        assert_eq!(vm.tier_stats.compilation_errors, 0);
    }
}
