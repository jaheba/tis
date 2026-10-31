use tis::{Tiering, Vm, compile};
#[test]
fn dataclasses_and_copy_preserve_graphs_and_live_attributes() {
    for tier in [
        Tiering::Bytecode,
        Tiering::Lir,
        Tiering::Native,
        Tiering::CopyPatch,
    ] {
        let mut vm = Vm::new();
        vm.tiering = tier;
        vm.run_program(
            &mut compile(
                r#"
import copy
from dataclasses import dataclass
@dataclass
class A:
    name: str
    items: list
    active: bool = True
a=A('hello',[1,2])
b=copy.deepcopy(a)
assert a == b and a is not b and a.items is not b.items
assert repr(a) == "A(name='hello', items=[1, 2], active=True)"
assert A(name='x', items=[]) == A('x',[])
view=a.__dict__
assert view is a.__dict__
a.name='changed'
assert view.get('name') == 'changed'
view.update({'name':'live'})
assert a.name=='live'
xs=[]
xs.append(xs)
ys=copy.deepcopy(xs)
assert ys is ys[0] and ys is not xs
shared=[1]
cloned=copy.deepcopy({'x':[shared,shared], 'y':shared})
assert cloned['x'][0] is cloned['y'] and cloned['x'][1] is cloned['y']
class C:
    def __init__(self):
        self.value=[42]
    def __reduce__(self):
        return C, (), self.__dict__
    def __setstate__(self, state):
        self.__dict__.update(state)
c=C()
d=copy.deepcopy(c)
assert d.value==[42] and d.value is not c.value
@dataclass
class Cycle:
    child: object
node=Cycle(None)
node.child=node
assert repr(node)=='Cycle(child=...)'
@dataclass
class Guard:
    value: object
left=Guard(None)
right=Guard(None)
def fresh(self):
    for i in range(3000):
        garbage=[i]
    return [42]
Guard.value=property(fresh)
assert left==right
"#,
            )
            .unwrap(),
        )
        .unwrap();
        vm.collect_garbage(&[]).unwrap();
        vm.run_program(&mut compile("assert view.get('name')=='live'\n").unwrap())
            .unwrap();
    }
}
