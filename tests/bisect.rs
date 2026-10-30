use tis::{Tiering, Vm, compile};

#[test]
fn searches_insertions_bounds_and_key_order_match_python() {
    let source = r#"
from bisect import bisect, bisect_left, bisect_right, insort, insort_left, insort_right
assert bisect is bisect_right
assert insort is insort_right
xs = [1,2,2,2,4]
assert bisect_left(xs,2) == 1
assert bisect_right(xs,2) == 4
assert bisect_left(xs,2,2,3) == 2
assert bisect_right(xs,2,2,3) == 3
assert bisect_left(xs,2,hi=-1) == 1
assert bisect_left(xs,2,hi=-2) == 0
assert bisect_left(xs,-3) == 0
assert bisect_right(xs,99) == 5
assert bisect_left((),3) == 0
assert bisect_right('abbbd','b') == 4
insort_left(xs,2)
insort_right(xs,0)
assert xs == [0,1,2,2,2,2,4]
insort(xs,9,99)
assert xs[-1] == 9
rows = [(1,'a'),(2,'b'),(2,'c'),(4,'d')]
seen = []
def key(row):
    seen.append(row[1])
    return row[0]
assert bisect_left(rows,2,key=key) == 1
assert seen == ['c','b','a']
seen.clear()
insort_right(rows,(2,'new'),key=key)
assert seen == ['new','c','d']
assert rows[3] == (2,'new')
for i in range(200):
    assert bisect(a=xs,x=2,lo=0,hi=None) == 6
"#;
    for tier in [
        Tiering::Bytecode,
        Tiering::Lir,
        Tiering::Native,
        Tiering::CopyPatch,
    ] {
        let mut vm = Vm::new();
        vm.tiering = tier;
        vm.run_program(&mut compile(source).unwrap()).unwrap();
        vm.collect_garbage(&[]).unwrap();
    }
}

#[test]
fn key_callbacks_may_allocate_and_mutate_the_source() {
    let source = r#"
from bisect import insort, bisect_left
xs = []
def key(value):
    if not xs:
        xs.append((0,))
    for i in range(3000):
        unused = [i]
    return [value[0]]
insort(xs,(2,),key=key)
assert xs == [(0,), (2,)]
assert bisect_left(xs,[2],key=key) == 1
"#;
    for tier in [Tiering::Bytecode, Tiering::Native, Tiering::CopyPatch] {
        let mut vm = Vm::new();
        vm.tiering = tier;
        vm.run_program(&mut compile(source).unwrap()).unwrap();
    }
}

#[test]
fn invalid_bounds_and_duplicate_parameters_fail() {
    for code in [
        "bisect_left([1],1,lo=-1)",
        "bisect_left([1],1,0,99)",
        "bisect_left([1],1,lo=0.5)",
        "bisect_left([1],1,0,lo=0)",
        "bisect_left([1],1,key=3)",
        "bisect_left([1],1,unknown=0)",
    ] {
        let mut vm = Vm::new();
        assert!(
            vm.run_program(
                &mut compile(&format!("from bisect import bisect_left\n{code}")).unwrap()
            )
            .is_err(),
            "{code}"
        );
    }
}

#[test]
fn guarded_search_handles_multiple_operands_and_rebound_callees() {
    use tis::lir::{Helper, LoweringMode, Operation, ScalarBuiltin};
    let source = "from bisect import bisect_left\ndef f(xs,x):\n    return bisect_left(xs,x,0,None)\nfor i in range(300):\n    assert f([0,1,1,2],1) == 1\n";
    for tier in [Tiering::Lir, Tiering::Native, Tiering::CopyPatch] {
        let mut vm = Vm::new();
        vm.tiering = tier;
        vm.thresholds.function_entries = 1;
        vm.thresholds.loop_backedges = 4;
        vm.run_program(&mut compile(source).unwrap()).unwrap();
        let hir = vm.function_hir(vm.get_global("f").unwrap()).unwrap();
        let lir = hir.lower_lir(LoweringMode::Profiled).unwrap();
        assert!(lir.blocks.iter().flat_map(|b| &b.instructions).any(|i| matches!(&i.operation, Operation::Call { helper: Helper::ScalarCall(ScalarBuiltin::BisectLeft), arguments, effects, roots, ..} if arguments.len()==5 && effects.may_call && effects.may_allocate && !roots.is_empty())), "{}", lir.dump());
        let source = "def replacement(xs,x,lo,hi):\n    for i in range(5000):\n        unused=[i]\n    return len(xs)+100\nbisect_left=replacement\nassert f([1,2],3)==102\n";
        vm.run_program(&mut compile(source).unwrap()).unwrap();
        vm.collect_garbage(&[]).unwrap();
    }
}
