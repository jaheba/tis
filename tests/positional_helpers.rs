use tis::{Object, Tiering, Value, Vm, compile};

fn vm(tier: Tiering, source: &str) -> Vm {
    let mut vm = Vm::new();
    vm.tiering = tier;
    vm.thresholds.function_entries = 1;
    vm.thresholds.loop_backedges = 4;
    vm.run_program(&mut compile(source).unwrap()).unwrap();
    vm
}

#[test]
fn warmed_lir_backends_share_the_borrowed_builtin_call_path() {
    for tier in [Tiering::Lir, Tiering::Native, Tiering::CopyPatch] {
        let mut vm = vm(
            tier,
            r#"
import math
def leaves(xs):
    return len(xs)+int(math.sqrt(16.0))+xs.count(1)
for i in range(300):
    assert leaves([1,2,1])==9
"#,
        );
        let f = vm.get_global("leaves").unwrap();
        let xs = vm.heap.allocate(Object::List(
            vec![Value::Int(1), Value::Int(2), Value::Int(1)].into(),
        ));
        let before = vm.tier_stats.bytecode_leaf_calls;
        assert_eq!(vm.call(f, &[xs]).unwrap(), Value::Int(9));
        assert!(vm.tier_stats.bytecode_leaf_calls >= before + 4);
        assert_eq!(vm.stack_len(), 0);
    }
}

#[test]
fn shared_positional_calls_keep_gc_defaults_binders_and_errors() {
    for tier in [Tiering::Lir, Tiering::Native, Tiering::CopyPatch] {
        let mut vm = vm(
            tier,
            r#"
def identity(x):
    return x
def answer():
    return 42
def call1(f,x):
    return f(x)
def call0(f):
    return f()
for i in range(300):
    assert call1(identity,[i])==[i]
    assert call0(answer)==42
def churn(x):
    for i in range(7000):
        garbage=[i]
    return x
class C:
    def __init__(self):
        self.value=[40]
    def method(self,x=2):
        return churn(self.value)[0]+x
def make(seed):
    def closure(x=2):
        return churn(seed)[0]+x
    return closure
c=C()
closure=make([40])
assert call1(churn,[123])==[123]
assert call0(c.method)==42
assert call0(closure)==42
assert c.method(x=3)==43
assert c.method(*[4])==44
assert closure(**{'x':5})==45
xs=[('b',2),('a',1)]
xs.sort(key=lambda x:churn(x)[0])
assert xs==[('a',1),('b',2)]
"#,
        );
        let call1 = vm.get_global("call1").unwrap();
        let identity = vm.get_global("identity").unwrap();
        assert!(vm.call(call1, &[Value::Int(0), Value::Int(1)]).is_err());
        assert_eq!(vm.stack_len(), 0);
        assert_eq!(
            vm.call(call1, &[identity, Value::Int(123)]).unwrap(),
            Value::Int(123)
        );
        vm.collect_garbage(&[]).unwrap();
        assert_eq!(vm.stack_len(), 0);
    }
}
