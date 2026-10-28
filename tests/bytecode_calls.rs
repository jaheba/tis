use tis::{BytecodeMode, Vm, compile};

fn run(source: &str, mode: BytecodeMode) -> Vm {
    let mut p = compile(source).unwrap();
    let mut vm = Vm::new();
    vm.bytecode = mode;
    vm.run_program(&mut p).unwrap();
    vm
}

#[test]
fn positional_dispatch_preserves_lookup_order_and_argument_side_effects() {
    let source = r#"events=[]
class A:
    def method(self,x):
        events.append('old')
        return x
a=A()
def mutate():
    a.method=lambda x:events.append('new')
    events.append('arg')
    return 42
assert a.method(mutate())==42
assert events==['arg','old']
assert len([1,2])==2
d={}
d.setdefault('x',[]).append(123)
assert d.get('x')==[123]
"#;
    for mode in [BytecodeMode::Stack, BytecodeMode::Register] {
        let vm = run(source, mode);
        if mode == BytecodeMode::Register {
            assert!(vm.tier_stats.bytecode_leaf_calls > 0);
        }
    }
}

#[test]
fn register_call_arguments_survive_recursive_collection_and_sort_callbacks() {
    let source = r#"def churn(x):
    for i in range(7000):
        garbage=[i]
    return x
def f(x,y=2):
    return churn(x)[0]+y
assert f([40])==42
assert f([40],y=3)==43
assert f(*[[40]],**{'y':4})==44
xs=[('b',2),('a',1)]
xs.sort(key=lambda x:churn(x)[0])
assert xs==[('a',1),('b',2)]
"#;
    for mode in [BytecodeMode::Stack, BytecodeMode::Register] {
        run(source, mode);
    }
}

#[test]
fn collection_preserves_reachable_objects_and_rejects_invalid_roots() {
    let source = r#"class A:
    def method(self):
        return self.value[0]
a=A()
a.value=[123]
bound=a.method
def outer(x):
    def inner():
        return x
    return inner
f=outer(a.value)
d={'a':a,'bound':bound,'f':f}
views=[d.items(),iter(d),enumerate([a]),zip([a],[a])]
a.value.append(a)
"#;
    let mut vm = run(source, BytecodeMode::Register);
    vm.collect_garbage(&[]).unwrap();
    let mut p = compile(
        r#"assert bound()==123
assert f()[0]==123
assert d['a'] is a
assert next(views[1])=='a'
assert next(views[2])[1] is a
assert next(views[3])[0] is a"#,
    )
    .unwrap();
    vm.run_program(&mut p).unwrap();
    let foreign = tis::Heap::new().allocate(tis::Object::List(vec![].into()));
    let before = vm.heap.allocation_count();
    assert!(vm.collect_garbage(&[foreign]).is_err());
    assert_eq!(before, vm.heap.allocation_count());
}
