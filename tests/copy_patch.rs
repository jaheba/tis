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

#[test]
fn parallel_edge_moves_preserve_disjoint_cycles_and_fanout() {
    let mut vm = warm(
        r#"
def rotate(n, a, b):
    c, d, e = a, b, a
    for i in range(n):
        a, b, c, d, e = b, a, e, c, c
        garbage = [i]
    return a, b, c, d, e
for i in range(40):
    rotate(i, [], [])
"#,
    );
    let rotate = vm.get_global("rotate").unwrap();
    let first = vm.heap.allocate(Object::List(vec![Value::Int(10)].into()));
    let second = vm.heap.allocate(Object::List(vec![Value::Int(20)].into()));
    for n in 0..50 {
        let mut expected = [first, second, first, second, first];
        for _ in 0..n {
            let old = expected;
            expected = [old[1], old[0], old[4], old[2], old[2]];
        }
        let result = vm.call(rotate, &[Value::Int(n), first, second]).unwrap();
        for (index, value) in expected.into_iter().enumerate() {
            assert_eq!(
                vm.heap.get_item(result, Value::Int(index as i64)).unwrap(),
                value
            );
        }
        vm.collect_garbage(&[first, second]).unwrap();
    }
    assert!(vm.tier_stats.copy_patch_entries > 0);
    assert_eq!(vm.tier_stats.compilation_errors, 0);
}

#[test]
fn direct_float_views_renew_after_shared_snapshots_and_storage_changes() {
    let mut vm = warm_with_window(
        "def move(a,d):\n    for step in range(2):\n        x,y=a\n        a[0]=x+d/2.0\n        a[1]=y-d/2.0\n    return a[0]+a[1]\nfor i in range(300): assert move([1.0,2.0],0.25)==3.0\na=[10.0,20.0]\nassert str(a)=='[10.0, 20.0]'\nassert move(a,2.0)==30.0\nassert str(a)=='[12.0, 18.0]'\nassert move(a,-1.0)==30.0\nassert str(a)=='[11.0, 19.0]'\na[0]='bad'\ntry: move(a,1.0)\nexcept TypeError: pass\nelse: assert False\nassert a==['bad',19.0]\n",
        128,
    );
    assert!(
        vm.tier_stats.native_inline_float_views > 0,
        "{:?}",
        vm.tier_stats
    );
    vm.profiling.mode = tis::profile::ProfileMode::Eager;
    let hits = vm.tier_stats.native_inline_float_views;
    vm.run_program(&mut compile("assert move([2.0,4.0],1.0)==6.0").unwrap())
        .unwrap();
    assert_eq!(vm.tier_stats.native_inline_float_views, hits);
    assert_eq!(vm.tier_stats.compilation_errors, 0);
    assert_eq!(vm.stack_len(), 0);
    vm.collect_garbage(&[]).unwrap();
}

#[test]
fn direct_small_unpacks_keep_types_guards_and_custom_iteration_effects() {
    let mut vm = warm(
        "def pair(v):\n    a,b=v\n    return a,b\ndef triple(v):\n    a,b,c=v\n    return a,b,c\nfor i in range(40):\n    assert pair((i,None))==(i,None)\n    assert triple([i,True,None])==(i,True,None)\nassert triple([True,False,True])==(True,False,True)\nclass Source:\n    def __init__(self,v): self.v=v; self.calls=0\n    def __iter__(self):\n        self.calls+=1\n        return iter(self.v)\ns=Source([1,2])\nassert pair(s)==(1,2)\nassert s.calls==1\ns=Source([1,2,3])\ntry: pair(s)\nexcept ValueError: pass\nelse: assert False\nassert s.calls==1\n",
    );
    assert!(vm.tier_stats.native_inline_unpack_batches > 0);
    vm.profiling.mode = tis::profile::ProfileMode::Eager;
    let hits = vm.tier_stats.native_inline_unpack_batches;
    vm.run_program(&mut compile("assert pair((7,8))==(7,8)").unwrap())
        .unwrap();
    assert_eq!(vm.tier_stats.native_inline_unpack_batches, hits);
    assert_eq!(vm.tier_stats.compilation_errors, 0);
    assert_eq!(vm.stack_len(), 0);
}

#[test]
fn direct_iterators_preserve_growth_exhaustion_and_custom_effects() {
    let vm = warm_with_window(
        r#"
def consume(items):
    out=[]
    for x in items:
        out.append(x)
    return out
def pair(a):
    x,y=a
    for i in range(2):
        x=x+y
    return x
for repeat in range(100):
    assert pair([1.0,2.0]) == 5.0
    assert consume([1,2,3]) == [1,2,3]
    assert consume((True,False)) == [True,False]
    assert consume([1.5,2.5]) == [1.5,2.5]
    assert consume(range(9,-2,-3)) == [9,6,3,0]
a=[1,2]
i=iter(a)
assert next(i)==1
a.append(3)
assert consume(i)==[2,3]
a.append(4)
assert consume(i)==[]
class Source:
    def __init__(self):
        self.calls=0
    def __iter__(self):
        self.calls+=1
        return iter([7,8])
s=Source()
assert consume(s)==[7,8]
assert s.calls==1
assert consume([[], {}, s]) != []
"#,
        128,
    );
    assert!(vm.tier_stats.copy_patch_entries > 0);
    assert!(vm.tier_stats.native_inline_next > 0);
    assert_eq!(vm.tier_stats.native_compilations, 0);
}

#[test]
fn direct_items_preserve_snapshots_promotion_protocols_and_roots() {
    let mut vm = warm_with_window(
        r#"
def update(a,k,v):
    for repeat in range(2):
        a[k]=v
    return a[k]
a=[True,False,True]
f=[1.0,2.0]
b=[1,None,3]
for repeat in range(100):
    assert update(a,-1,False) is False
    assert update(a,True,True) is True
    assert update(f,0,3.5)==3.5
    assert update(b,0,repeat)==repeat
assert str(a)=='[True, True, False]'
assert update(a,0,False) is False
assert str(a)=='[False, True, False]'
assert str(f)=='[3.5, 2.0]'
assert update(f,-1,4.5)==4.5
assert str(f)=='[3.5, 4.5]'
assert update(a,0,7)==7
assert update(f,0,'x')=='x'
assert update(b,1,b) is b
class Key:
    def __init__(self):
        self.calls=0
    def __index__(self):
        self.calls+=1
        return 0
k=Key()
assert update(b,k,9)==9
assert k.calls==3
class Custom:
    def __init__(self):
        self.calls=0
        self.value=0
    def __setitem__(self,k,v):
        self.calls+=1
        self.value=v
    def __getitem__(self,k):
        self.calls+=1
        return self.value
c=Custom()
assert update(c,0,11)==11
assert c.calls==3
try:
    update(b,99,1)
    assert False
except IndexError:
    pass
assert b[0]==9
assert b[1] is b
"#,
        128,
    );
    assert!(vm.tier_stats.native_list_stores > 0);
    vm.collect_garbage(&[]).unwrap();
    assert_eq!(vm.stack_len(), 0);
}

#[test]
fn cached_exhaustion_keeps_sticky_iterators_and_survives_collection() {
    let mut vm = warm_with_window(
        r#"
def exhaust_many(a,n):
    for repeat in range(n):
        it=iter(a)
        for item in it:
            pass
    return it
for repeat in range(80):
    it=exhaust_many([1,2],100)
a=[1,2]
"#,
        128,
    );
    let inline = vm.tier_stats.native_inline_next;
    let syncs = vm.tier_stats.native_root_syncs;
    vm.run_program(&mut compile("it=exhaust_many(a,100)\n").unwrap())
        .unwrap();
    assert!(vm.tier_stats.native_inline_next - inline >= 395);
    assert!(
        vm.tier_stats.native_root_syncs - syncs < 230,
        "{}",
        vm.tier_stats.native_root_syncs - syncs
    );
    vm.run_program(
        &mut compile(
            r#"
a.append(3)
try:
    next(it)
    assert False
except StopIteration:
    pass
it=exhaust_many(a,10000)
a.append(4)
try:
    next(it)
    assert False
except StopIteration:
    pass
class Source:
    def __init__(self):
        self.calls=0
    def __iter__(self):
        return self
    def __next__(self):
        self.calls+=1
        raise StopIteration
s=Source()
assert exhaust_many(s,10) is s
assert s.calls==10
"#,
        )
        .unwrap(),
    )
    .unwrap();
    let inline = vm.tier_stats.native_inline_next;
    vm.profiling = tis::profile::ProfileConfig::eager();
    vm.run_program(&mut compile("it=exhaust_many(a,4)\n").unwrap())
        .unwrap();
    assert_eq!(vm.tier_stats.native_inline_next, inline);
    vm.collect_garbage(&[]).unwrap();
    assert_eq!(vm.stack_len(), 0);
}

#[test]
fn float_pairs_preserve_rounding_snapshots_and_late_guard_effects() {
    let mut source = String::from(
        r#"
events=[]
def independent(x,y,z):
    a=x+y
    b=x-y
    return (a,b,a*b)
def dependent(x,y,z):
    a=x+y
    return a*a
def cancellation(x,y,z):
    a=x+y
    b=x-y
    c=z+z
    d=a+a
    return a*b+c
def update(a,x,y):
    a[0]+=x*y
    return a[0]
def late(x,y):
    events.append(1)
    a=x+x
    b=a*a
    return (b+a)+y
"#,
    );
    let mut names = vec![
        "independent".to_owned(),
        "dependent".to_owned(),
        "cancellation".to_owned(),
    ];
    for (a, first) in [('a', '+'), ('s', '-'), ('m', '*')] {
        for (b, second) in [('a', '+'), ('s', '-'), ('m', '*')] {
            let name = format!("pair_{a}{b}");
            source.push_str(&format!(
                "def {name}(x,y,z):\n    a=x{first}x\n    return a{second}x\n"
            ));
            names.push(name);
        }
    }
    source.push_str(
        "row=[1.0]\nfor repeat in range(60):\n    update(row,0.0,1.0)\n    late(1.0,1.0)\n",
    );
    for name in &names {
        source.push_str(&format!("    {name}(1.25,0.75,-0.5)\n"));
    }
    fn setup(source: &str, tier: Tiering) -> Vm {
        let mut vm = Vm::new();
        vm.tiering = tier;
        vm.profiling.window_ticks = 32;
        vm.thresholds = HotThresholds {
            function_entries: 12,
            loop_backedges: 16,
        };
        vm.run_program(&mut compile(source).unwrap()).unwrap();
        vm
    }
    fn bits(vm: &Vm, value: Value) -> Vec<u64> {
        let values = match value {
            Value::Float(_) => vec![value],
            _ => match vm.heap.get(value).unwrap() {
                Object::Tuple(v) => v.clone(),
                _ => panic!("float result"),
            },
        };
        values
            .into_iter()
            .map(|v| match v {
                // Arithmetic does not specify NaN payload/sign; other bits, including zero, must match.
                Value::Float(f) => {
                    if f.is_nan() {
                        f64::NAN.to_bits()
                    } else {
                        f.to_bits()
                    }
                }
                _ => panic!("float element"),
            })
            .collect()
    }
    let mut expected = setup(&source, Tiering::Bytecode);
    let mut actual = setup(&source, Tiering::CopyPatch);
    for sample in [
        [1.0, 2f64.powi(-27), -0.5],
        [0.0, -0.0, 0.0],
        [-0.0, 0.0, -0.0],
        [1.25, -3.75, 0.125],
        [f64::MAX, 1.0, -1.0],
        [f64::MIN_POSITIVE, 0.5, 0.0],
        [f64::from_bits(1), -0.0, 0.0],
        [f64::INFINITY, 1.0, -1.0],
        [f64::NEG_INFINITY, 1.0, 0.0],
        [f64::from_bits(0x7ff8_0000_0000_1234), 1.0, -0.5],
    ] {
        let args = sample.map(Value::Float);
        for name in &names {
            let f = expected.get_global(name).unwrap();
            let e = expected.call(f, &args).unwrap();
            let f = actual.get_global(name).unwrap();
            let a = actual.call(f, &args).unwrap();
            assert_eq!(bits(&actual, a), bits(&expected, e), "{name} {sample:?}");
        }
    }
    let f = actual.get_global("cancellation").unwrap();
    assert_eq!(
        actual
            .call(
                f,
                &[
                    Value::Float(1.0),
                    Value::Float(2f64.powi(-27)),
                    Value::Float(-0.5)
                ]
            )
            .unwrap(),
        Value::Float(0.0)
    );
    actual
        .run_program(
            &mut compile(
                r#"
a=[1.0]
snapshot=a.copy()
assert update(a,2.0,3.0)==7.0
assert snapshot==[1.0]
before=len(events)
try:
    late(1.0,'bad')
    assert False
except TypeError:
    pass
assert len(events)==before+1
"#,
            )
            .unwrap(),
        )
        .unwrap();
    actual.collect_garbage(&[]).unwrap();
    assert_eq!(actual.stack_len(), 0);
    assert_eq!(actual.tier_stats.native_compilations, 0);
}

#[test]
fn ascii_slices_preserve_unicode_bounds_steps_and_index_effects() {
    let mut vm = warm_with_window(
        r#"
def cut(s,a,b,k):
    return s[a:b:k]
for repeat in range(100):
    assert cut('abcdefghijklmnopqrstuvwxyz',1,4,None)=='bcd'
    assert cut('abcdefghij',0,2,1)=='ab'
assert cut('abc\x00def',1,6,None)=='bc\x00de'
assert cut('abcdefghijklmnopqrstuvwxyz',0,11,None)=='abcdefghijk'
assert cut('abcdefghijklmnopqrstuvwxyz',0,12,None)=='abcdefghijkl'
assert cut('abcdefghijklmnopqrstuvwxyz',-3,None,1)=='xyz'
assert cut('abcdefghijklmnopqrstuvwxyz',None,-24,None)=='ab'
assert cut('abc',4,-9,None)==''
assert cut('',None,None,None)==''
assert cut('abc',-9223372036854775808,9223372036854775807,1)=='abc'
assert cut('abcdefghijklé😀xyz',-3,None,None)=='xyz'
assert cut('aé😀z',1,3,None)=='é😀'
assert cut('abcdé',0,2,None)=='ab'
assert cut('abc',None,None,-1)=='cba'
assert cut('abcdef',None,None,2)=='ace'
assert cut('abcdef',False,True,True)=='a'
try:
    cut('abc',None,None,0)
    assert False
except ValueError:
    pass
calls=[]
class Index:
    def __index__(self):
        calls.append(1)
        return 1
assert cut('abc',Index(),None,None)=='bc'
assert calls==[1]
class Source:
    def __getitem__(self,key):
        calls.append(2)
        return 'custom'
assert cut(Source(),None,None,None)=='custom'
assert calls==[1,2]
"#,
        128,
    );
    assert!(vm.tier_stats.native_string_slices > 0);
    vm.collect_garbage(&[]).unwrap();
    assert_eq!(vm.stack_len(), 0);
}

#[test]
fn ascii_slice_tails_stay_bounded_on_a_small_native_stack() {
    // Match the debug-parser headroom in the existing native-tail test.
    std::thread::Builder::new()
        .stack_size(256 * 1024)
        .spawn(|| {
            let vm = warm(
                r#"
def slices(s,n):
    result=''
    for i in range(n):
        result=s[1:4]
    return result
assert slices('abcdefghijklmnopqrstuvwxyz',100000)=='bcd'
assert slices('abcdefghij',100000)=='bcd'
"#,
            );
            assert!(vm.tier_stats.native_string_slices > 100000);
            assert_eq!(vm.stack_len(), 0);
        })
        .unwrap()
        .join()
        .unwrap();
}

#[test]
fn invariant_imports_preserve_captured_and_reassigned_bindings() {
    let vm = warm_with_window(
        r#"
def outer(a,b):
    def change():
        nonlocal a
        a += 1
    change()
    b += 2
    return a+b
def rotate(n,a,b):
    for i in range(n):
        a,b=b,a
    return a,b
def defaults(a,b=7,*,c=2):
    return a+b+c
for i in range(100):
    assert outer(i,3)==i+6
    assert rotate(i,4,9)==((4,9) if i%2==0 else (9,4))
    assert defaults(i)==i+9
    assert defaults(i,c=3)==i+10
"#,
        4,
    );
    assert!(vm.tier_stats.copy_patch_entries > 0);
    assert_eq!(vm.tier_stats.compilation_errors, 0);
    assert_eq!(vm.stack_len(), 0);
}

#[test]
fn direct_scalar_fallbacks_preserve_protocols_rebinding_and_error_order() {
    let vm = warm_with_window(
        r#"
from math import sqrt
calls=[0]
def root(x):
    calls[0]+=1
    return sqrt(x)
def length(x):
    return len(x)
for i in range(100):
    assert root(4)==2.0
    assert length([True,False])==2
    assert length((1,2,3))==3
    assert length("é🙂")==2
    assert length("é🙂abcdefghijklmnop")==18
try:
    root(-1)
except ValueError:
    pass
else:
    assert False
assert calls[0]==101
class Sized:
    def __len__(self):
        calls[0]+=1
        return 7
assert length(Sized())==7
assert calls[0]==102
def replacement(x):
    return 23
sqrt=replacement
assert root(-1)==23
assert calls[0]==103
len=replacement
assert length(Sized())==23
assert calls[0]==103
"#,
        4,
    );
    assert!(vm.tier_stats.copy_patch_entries > 0);
    assert_eq!(vm.tier_stats.compilation_errors, 0);
    assert_eq!(vm.stack_len(), 0);
}

#[test]
fn osr_imports_values_created_after_the_entry_parameters() {
    let mut vm = Vm::new();
    vm.tiering = Tiering::CopyPatch;
    vm.thresholds.function_entries = 0;
    vm.thresholds.loop_backedges = 8;
    vm.profiling.window_ticks = 2;
    vm.run_program(&mut compile("def make(n):\n    s=[0]\n    for i in range(n):\n        s[0]+=1\n        garbage=[i]\n    return s\n").unwrap()).unwrap();
    let function = vm.get_global("make").unwrap();
    let value = vm.call(function, &[Value::Int(10000)]).unwrap();
    assert_eq!(
        vm.heap.get_item(value, Value::Int(0)).unwrap(),
        Value::Int(10000)
    );
    assert!(vm.tier_stats.copy_patch_osr_entries > 0);
    assert_eq!(vm.tier_stats.compilation_errors, 0);
    assert_eq!(vm.stack_len(), 0);
}
