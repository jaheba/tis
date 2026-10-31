use tis::{Object, Tiering, Value, Vm, compile};

fn native(source: &str) -> Vm {
    in_tier(source, Tiering::Native)
}

fn in_tier(source: &str, tier: Tiering) -> Vm {
    let mut vm = Vm::new();
    vm.tiering = tier;
    vm.thresholds.function_entries = 32;
    vm.thresholds.loop_backedges = 32;
    vm.run_program(&mut compile(source).unwrap()).unwrap();
    vm
}

#[test]
fn copied_float_body_removes_callee_frames_and_handles_polymorphic_misses() {
    let mut vm = native(
        "def square(x):\n    return x*x\ndef caller(x):\n    return square(x)+1.0\nfor i in range(500):\n    assert caller(3.0)==10.0\n",
    );
    let caller = vm.get_global("caller").unwrap();
    let hits = vm.tier_stats.native_body_inlines;
    let frames = vm.tier_stats.positional_frames;
    assert_eq!(
        vm.call(caller, &[Value::Float(4.0)]).unwrap(),
        Value::Float(17.0)
    );
    assert!(
        vm.tier_stats.native_body_inlines > hits,
        "no copied body executed: {:?}",
        vm.tier_stats
    );
    // The host call creates one caller frame; the copied callee creates none.
    assert_eq!(vm.tier_stats.positional_frames - frames, 1);
    assert_eq!(
        vm.call(caller, &[Value::Int(4)]).unwrap(),
        Value::Float(17.0)
    );
    assert!(vm.call(caller, &[Value::None]).is_err());
    assert_eq!(vm.stack_len(), 0);
}

#[test]
fn readonly_method_body_binds_actual_receiver_and_replacement_misses() {
    let mut vm = native(
        "class R:\n    def __init__(self,x,w):\n        self.x=x\n        self.w=w\n    def edge(self):\n        return self.x+self.w\ndef caller(r):\n    return r.edge()\na=R(2.0,3.0)\nb=R(7.0,4.0)\nfor i in range(500):\n    assert caller(a)==5.0\n",
    );
    let caller = vm.get_global("caller").unwrap();
    let a = vm.get_global("a").unwrap();
    let b = vm.get_global("b").unwrap();
    let hits = vm.tier_stats.native_body_inlines;
    assert_eq!(vm.call(caller, &[b]).unwrap(), Value::Float(11.0));
    assert!(vm.tier_stats.native_body_inlines > hits);
    vm.run_program(
        &mut compile("def changed(self):\n    return self.x-self.w\nR.edge=changed\n").unwrap(),
    )
    .unwrap();
    assert_eq!(vm.call(caller, &[a]).unwrap(), Value::Float(-1.0));
    assert_eq!(vm.stack_len(), 0);
}

#[test]
fn replacement_and_gc_miss_execute_effectful_callee_once() {
    let mut vm = native(
        "def square(x):\n    return x*x\ndef caller(x):\n    return square(x)\nfor i in range(500):\n    caller(3.0)\nevents=[]\ndef changed(x):\n    events.append(x)\n    for i in range(12000):\n        trash=[i]\n    return x\nsquare=changed\n",
    );
    let caller = vm.get_global("caller").unwrap();
    let child = vm.heap.allocate(Object::List(vec![Value::Int(99)].into()));
    assert_eq!(vm.call(caller, &[child]).unwrap(), child);
    let events = vm.get_global("events").unwrap();
    assert_eq!(vm.heap.len(events).unwrap(), 1);
    assert_eq!(
        vm.heap.get_item(child, Value::Int(0)).unwrap(),
        Value::Int(99)
    );
    assert_eq!(vm.stack_len(), 0);
}

#[test]
fn readonly_short_circuit_cfg_is_copied_and_speculative_missing_fields_fall_back() {
    let mut vm = native(
        "class P:\n    def __init__(self,x,y):\n        self.x=x\n        self.y=y\nclass R:\n    def __init__(self):\n        self.x=0.0\n        self.y=0.0\n        self.w=2.0\n        self.h=2.0\n    def contains(self,p):\n        return p.x>=self.x-self.w and p.x<self.x+self.w and p.y>=self.y-self.h and p.y<self.y+self.h\ndef caller(r,p):\n    return r.contains(p)\nr=R()\np=P(1.0,1.0)\nfor i in range(500):\n    assert caller(r,p)\nclass OnlyX:\n    def __init__(self):\n        self.x=9.0\noutside=OnlyX()\n",
    );
    let caller = vm.get_global("caller").unwrap();
    let r = vm.get_global("r").unwrap();
    let p = vm.get_global("p").unwrap();
    let outside = vm.get_global("outside").unwrap();
    let hits = vm.tier_stats.native_body_inlines;
    assert_eq!(vm.call(caller, &[r, p]).unwrap(), Value::Bool(true));
    assert!(vm.tier_stats.native_body_inlines > hits);
    // The unexecuted y read is missing. Preflight misses rather than raising speculatively.
    assert_eq!(vm.call(caller, &[r, outside]).unwrap(), Value::Bool(false));
    assert_eq!(vm.stack_len(), 0);
}

#[test]
fn unsupported_defaults_closures_recursion_and_keywords_keep_python_binding() {
    let mut vm = native(
        "def defaulted(x,y=2.0):\n    return x*y\ndef outer(y):\n    def inner(x):\n        return x*y\n    return inner\nf=outer(3.0)\ndef recurse(x,n):\n    if n:\n        return recurse(x,n-1)\n    return x*x\ndef caller(x):\n    return defaulted(x)+f(x)+recurse(x,2)+defaulted(y=4.0,x=x)\nfor i in range(500):\n    assert caller(2.0)==22.0\n",
    );
    let caller = vm.get_global("caller").unwrap();
    let hits = vm.tier_stats.native_body_inlines;
    assert_eq!(
        vm.call(caller, &[Value::Float(2.0)]).unwrap(),
        Value::Float(22.0)
    );
    assert_eq!(vm.tier_stats.native_body_inlines, hits);
}

#[test]
fn positional_binding_is_independent_of_sorted_local_slots() {
    for tier in [Tiering::Native, Tiering::CopyPatch] {
        let mut vm = in_tier(
            "def subtract(z,a):\n    return z-a\ndef caller(z,a):\n    return subtract(z,a)\nfor i in range(500):\n    assert caller(7.0,2.0)==5.0\n",
            tier,
        );
        let caller = vm.get_global("caller").unwrap();
        let hits = vm.tier_stats.native_body_inlines;
        assert_eq!(
            vm.call(caller, &[Value::Float(9.0), Value::Float(4.0)])
                .unwrap(),
            Value::Float(5.0)
        );
        assert!(vm.tier_stats.native_body_inlines > hits);
    }
}

#[test]
fn same_function_identity_new_hir_revision_misses_before_old_body() {
    for tier in [Tiering::Native, Tiering::CopyPatch] {
        let mut vm = in_tier(
            "def square(x):\n    return x*x\ndef caller(x):\n    return square(x)\nfor i in range(500):\n    caller(3.0)\n",
            tier,
        );
        let caller = vm.get_global("caller").unwrap();
        let square = vm.get_global("square").unwrap();
        let Object::Function(closure) = vm.heap.get(square).unwrap() else {
            panic!("not function")
        };
        let function = closure.function.clone();
        let id = function.borrow().id();
        let revision = function.borrow().revision();
        {
            let mut function = function.borrow_mut();
            let entry = function.entry();
            let value = function.append(
                entry,
                tis::hir::Op::Constant(tis::hir::Constant::Float(42.0)),
            );
            function.terminate(entry, tis::hir::Terminator::Return(value));
        }
        assert_eq!(function.borrow().id(), id);
        assert_ne!(function.borrow().revision(), revision);
        let hits = vm.tier_stats.native_body_inlines;
        assert_eq!(
            vm.call(caller, &[Value::Float(4.0)]).unwrap(),
            Value::Float(42.0)
        );
        assert_eq!(vm.tier_stats.native_body_inlines, hits);
    }
}

#[test]
fn copied_float_comparisons_bits_and_eager_fallback_match_python() {
    for tier in [Tiering::Native, Tiering::CopyPatch] {
        let mut vm = in_tier(
            "def less(z,a):\n    return z<a\ndef caller(z,a):\n    return less(z,a)\ndef zero(x):\n    return x*0.0\ndef caller_zero(x):\n    return zero(x)\nfor i in range(500):\n    caller(1.0,2.0)\n    caller_zero(1.0)\n",
            tier,
        );
        let caller = vm.get_global("caller").unwrap();
        let zero = vm.get_global("caller_zero").unwrap();
        let hits = vm.tier_stats.native_body_inlines;
        assert_eq!(
            vm.call(caller, &[Value::Float(f64::NAN), Value::Float(2.0)])
                .unwrap(),
            Value::Bool(false)
        );
        let Value::Float(value) = vm.call(zero, &[Value::Float(-1.0)]).unwrap() else {
            panic!("not float")
        };
        assert_eq!(value.to_bits(), (-0.0_f64).to_bits());
        assert!(vm.tier_stats.native_body_inlines > hits);
        vm.profiling.mode = tis::profile::ProfileMode::Eager;
        let hits = vm.tier_stats.native_body_inlines;
        let probes = vm.tier_stats.native_feedback_probes;
        assert_eq!(
            vm.call(caller, &[Value::Float(1.0), Value::Float(2.0)])
                .unwrap(),
            Value::Bool(true)
        );
        assert_eq!(vm.tier_stats.native_body_inlines, hits);
        assert!(vm.tier_stats.native_feedback_probes > probes);
    }
}

#[test]
fn hidden_caller_reference_survives_gc_after_successfully_inlined_body() {
    for tier in [Tiering::Native, Tiering::CopyPatch] {
        let mut vm = in_tier(
            "class Holder:\n    def __init__(self,x):\n        self.value=x\nchurn=False\ndef square(x):\n    return x*x\ndef clear(holder):\n    holder.value=None\n    if churn:\n        for i in range(12000):\n            trash=[i]\ndef caller(holder,x):\n    keep=holder.value\n    number=square(x)\n    clear(holder)\n    return keep,number\nfor i in range(500):\n    holder=Holder([i])\n    caller(holder,3.0)\nholder=Holder([456])\nchurn=True\n",
            tier,
        );
        let caller = vm.get_global("caller").unwrap();
        let holder = vm.get_global("holder").unwrap();
        let hits = vm.tier_stats.native_body_inlines;
        let result = vm.call(caller, &[holder, Value::Float(4.0)]).unwrap();
        assert!(vm.tier_stats.native_body_inlines > hits);
        let child = vm.heap.get_item(result, Value::Int(0)).unwrap();
        assert_eq!(
            vm.heap.get_item(child, Value::Int(0)).unwrap(),
            Value::Int(456)
        );
        assert_eq!(
            vm.heap.get_item(result, Value::Int(1)).unwrap(),
            Value::Float(16.0)
        );
        vm.collect_garbage(&[result]).unwrap();
        assert!(vm.heap.get(child).is_ok());
        vm.collect_garbage(&[]).unwrap();
        assert!(vm.heap.get(child).is_err());
        assert_eq!(vm.stack_len(), 0);
    }
}

#[test]
fn inline_metadata_preserves_loop_osr_and_reference_backend_calls() {
    for tier in [Tiering::Lir, Tiering::Native, Tiering::CopyPatch] {
        let mut vm = Vm::new();
        vm.tiering = tier;
        vm.thresholds.function_entries = 32;
        vm.thresholds.loop_backedges = 4;
        vm.run_program(&mut compile("def square(x):\n    return x*x\ndef caller(n):\n    total=0.0\n    for i in range(n):\n        total+=square(2.0)\n    return total\nassert caller(9000)==36000.0\n").unwrap()).unwrap();
        assert!(vm.tier_stats.osr_entries > 0);
        assert_eq!(vm.stack_len(), 0);
    }
}

#[test]
fn copy_patch_arithmetic_body_guards_binding_fields_and_revision() {
    let mut vm = Vm::new();
    vm.tiering = Tiering::CopyPatch;
    vm.thresholds.function_entries = 16;
    vm.thresholds.loop_backedges = 16;
    vm.run_program(
        &mut compile(
            r#"
class Point:
    def __init__(self,x):
        self.x=x
    def square(self):
        return self.x*self.x
def square(x):
    return x*x
def call(x):
    return square(x)+1.0
def method(p):
    return p.square()
p=Point(3.0)
for repeat in range(500):
    assert call(3.0)==10.0
    assert method(p)==9.0
# A type miss must execute overloaded multiplication once, never the copied arithmetic.
events=[]
class Other:
    def __mul__(self,other):
        events.append(1)
        return 12
assert call(Other())==13.0
assert events==[1]
p.x=4.0
assert method(p)==16.0
# A new callable must invalidate the old arithmetic plan before effects.
def replacement(x):
    events.append(2)
    return 21.0
square=replacement
assert call(3.0)==22.0
assert events==[1,2]
"#,
        )
        .unwrap(),
    )
    .unwrap();
    assert!(vm.tier_stats.native_body_inlines > 0);
    assert_eq!(vm.tier_stats.compilation_errors, 0);
    assert_eq!(vm.stack_len(), 0);
}

#[test]
fn copied_field_bodies_fall_back_for_late_properties_and_attribute_hooks() {
    for tier in [Tiering::Native, Tiering::CopyPatch] {
        let vm = in_tier(
            r#"
class Point:
    def __init__(self):
        self.x=3.0
def square(point):
    return point.x*point.x
def caller(point):
    return square(point)
p=Point()
for repeat in range(500):
    assert caller(p)==9.0
events=[]
def get_x(self):
    events.append(1)
    return 4.0
Point.x=property(get_x)
assert caller(p)==16.0
assert events==[1,1]
class Hook:
    def __init__(self):
        self.x=2.0
    def __getattribute__(self,name):
        events.append(2)
        return 5.0
assert caller(Hook())==25.0
assert events==[1,1,2,2]
"#,
            tier,
        );
        assert_eq!(vm.tier_stats.compilation_errors, 0);
        assert_eq!(vm.stack_len(), 0);
    }
}
