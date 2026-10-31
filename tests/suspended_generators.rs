use tis::{BytecodeMode, Tiering, Vm, compile};
#[test]
fn real_generator_frames_suspend_resume_and_trace_locals() {
    for tier in [
        Tiering::Bytecode,
        Tiering::Lir,
        Tiering::Native,
        Tiering::CopyPatch,
    ] {
        for mode in [BytecodeMode::Stack, BytecodeMode::Register] {
            let mut vm = Vm::new();
            vm.tiering = tier;
            vm.bytecode = mode;
            vm.thresholds.loop_backedges = 2;
            vm.thresholds.function_entries = 2;
            vm.run_program(
                &mut compile(
                    r"
events=[]
def produce(n):
    events.append('start')
    kept=[42]
    for i in range(n):
        yield i + kept[0]
    return 99
g=produce(3)
assert events==[]
assert iter(g) is g
assert next(g)==42
assert events==['start']
def delegate():
    answer=yield from produce(2)
    yield answer
assert list(delegate())==[42,43,99]
def empty():
    return 1
    yield 2
assert list(empty())==[]
assert list(range(10)[1:9:2])==[1,3,5,7]
assert list(range(10)[::-2])==[9,7,5,3,1]
class Tree:
    def __iter__(self):
        yield from range(3)
assert list(Tree())==[0,1,2]
def nested(x=2, *, y=3):
    result=[x,(yield y),4]
    yield result
h=nested(y=7)
assert next(h)==7
assert next(h)==[2,None,4]
assert next(h,99)==99
def bad():
    yield 1
    raise StopIteration('done')
h=bad()
assert next(h)==1
try:
    next(h)
    assert False
except RuntimeError:
    pass
assert next(h,99)==99
",
                )
                .unwrap(),
            )
            .unwrap();
            vm.collect_garbage(&[]).unwrap();
            vm.run_program(
                &mut compile("assert list(g)==[43,44]\nassert next(g, None) is None\n").unwrap(),
            )
            .unwrap();
            let function = vm.get_global("produce").unwrap();
            let tis::Object::Function(closure) = vm.heap.get(function).unwrap() else {
                panic!("function");
            };
            let profile = closure.function.borrow().profile();
            assert_eq!(profile.entries, 2);
            assert_eq!(profile.returns, 2);
        }
    }
}
