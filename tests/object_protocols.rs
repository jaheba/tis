use tis::{BytecodeMode, Tiering, Vm, compile};

fn check(source: &str) {
    for tier in [
        Tiering::Bytecode,
        Tiering::Lir,
        Tiering::Native,
        Tiering::CopyPatch,
    ] {
        for encoding in [BytecodeMode::Register, BytecodeMode::Stack] {
            let mut vm = Vm::new();
            vm.tiering = tier;
            vm.bytecode = encoding;
            vm.thresholds.function_entries = 2;
            vm.thresholds.loop_backedges = 4;
            vm.readonly_regions = true;
            vm.mutation_regions = true;
            vm.run_program(&mut compile(source).unwrap()).unwrap();
            assert_eq!(vm.tier_stats.compilation_errors, 0, "{tier:?} {encoding:?}");
        }
    }
}

#[test]
fn arithmetic_comparison_truth_and_builtin_container_callbacks() {
    check(
        r#"
log = []
class Number:
    def __init__(self, n): self.n = n
    def __add__(self, other): return NotImplemented
    def __radd__(self, other): return self.n + other
    def __eq__(self, other): return self.n == other
    def __lt__(self, other): return self.n < other.n
    def __bool__(self): return self.n != 0
    def __len__(self): return self.n
    def __call__(self, x): return self.n + x
class Derived(Number):
    def __radd__(self, other):
        log.append('derived')
        return 99
assert 3 + Number(4) == 7
assert Number(4) + Derived(5) == 99
assert log == ['derived']
assert Number(4) == 4
assert Number(4) != 3
assert [Number(4)] == [4]
assert [Number(4)].count(4) == 1
assert 4 in [Number(4)]
assert bool(Number(1)) and not Number(0)
assert len(Number(7)) == 7
assert Number(3)(4) == 7
assert [x.n for x in sorted([Number(3), Number(1), Number(2)])] == [1,2,3]
assert min([Number(3), Number(1)]).n == 1
class Rich:
    def __eq__(self, other): return [42]
assert Rich() == 1 == 1
assert (Rich() == 1) == [42]
"#,
    );
}

#[test]
fn descriptors_hooks_and_decorators_observe_python_precedence() {
    check(
        r#"
class Descriptor:
    def __get__(self, obj, owner): return obj._x + 1
    def __set__(self, obj, value): obj._x = value + 2
class A:
    x = Descriptor()
    def __init__(self): self._x = 3
    @property
    def doubled(self): return self._x * 2
    @doubled.setter
    def doubled(self, value): self._x = value // 2
    @staticmethod
    def static(x): return x + 1
    @classmethod
    def make(cls): return cls()
    def __getattr__(self, name): return 'missing:' + name
    def __setattr__(self, name, value): object.__setattr__(self, name, value)
a = A()
assert a.x == 4
a.x = 10
assert a._x == 12
assert a.doubled == 24
a.doubled = 16
assert a._x == 8
assert A.static(2) == 3 and a.static(3) == 4
assert A.make()._x == 3
assert a.unknown == 'missing:unknown'
assert getattr(1, 'unknown', 9) == 9
assert not hasattr(1, 'unknown')
setattr(a, 'field', 7)
assert a.field == 7
delattr(a, 'field')
assert a.field == 'missing:field'
class Override:
    def __getattribute__(self, name):
        if name == 'x': return 42
        return object.__getattribute__(self, name)
o = Override()
o.x = 1
assert o.x == 42
"#,
    );
}

#[test]
fn user_iteration_and_subscripts_work_in_builtins_and_loops() {
    check(
        r#"
class Counter:
    def __init__(self, n): self.n = n
    def __iter__(self): return self
    def __next__(self):
        if self.n == 0: raise StopIteration(17)
        self.n -= 1
        return self.n
assert list(Counter(4)) == [3,2,1,0]
assert sum(Counter(4)) == 6
assert 2 in Counter(4)
try:
    next(Counter(0))
except StopIteration as e:
    assert e.args == (17,)
assert next(Counter(0), 9) == 9
class Sequence:
    def __getitem__(self, key):
        if key >= 3: raise IndexError()
        return key * 2
assert list(Sequence()) == [0,2,4]
class Index:
    def __index__(self): return 1
assert [10,20][Index()] == 20
class Mapping:
    def __init__(self): self.data = {}
    def __getitem__(self, key): return self.data[key]
    def __setitem__(self, key, value): self.data[key] = value
    def __contains__(self, key): return key in self.data
m = Mapping()
m['a'] = 4
assert m['a'] == 4 and 'a' in m
"#,
    );
}

#[test]
fn hashing_mro_super_deletion_and_construction() {
    check(
        r#"
class Key:
    def __init__(self, n): self.n = n
    def __hash__(self): return hash(self.n)
    def __eq__(self, other):
        if isinstance(other, Key): return self.n == other.n
        return self.n == other
key = Key(7)
d = {key: 'first'}
assert d[Key(7)] == 'first'
assert d[7] == 'first'
d[Key(7)] = 'second'
assert len(d) == 1
assert list(d)[0] is key
assert d.get(7) == 'second'
assert {Key(7), Key(7), 7} == {7}
assert {Key('hello'): 3}['hello'] == 3
assert {('x', Key(7)): 3}[('x', 7)] == 3
assert {'x': Key(7)} == {'x': 7}
del d[7]
assert len(d) == 0
class Unhashable:
    def __eq__(self, other): return True
try:
    {Unhashable(): 1}
except TypeError:
    pass
else:
    assert False
class A:
    def value(self): return 'A'
class B(A):
    def value(self): return 'B' + super().value()
class C(A):
    def value(self): return 'C' + super().value()
class D(B, C):
    def value(self):
        try:
            return 'D' + super().value()
        finally:
            pass
assert D().value() == 'DBCA'
assert super(B, D()).value() == 'CA'
assert issubclass(D, A)
class Replaced:
    def __new__(cls): return 42
    def __init__(self): assert False
assert Replaced() == 42
class Allocated:
    def __new__(cls, n): return object.__new__(cls)
    def __init__(self, n): self.n = n
assert Allocated(3).n == 3
x = Allocated(4)
del x.n
assert not hasattr(x, 'n')
def deleted():
    value = 1
    del value
    try:
        value
    except UnboundLocalError:
        return True
assert deleted()
"#,
    );
}

#[test]
fn native_attribute_caches_observe_late_descriptors_and_methods() {
    check(
        r#"
class Plain:
    def __init__(self): self.x = 1
def read(o): return o.x
p = Plain()
for i in range(100):
    assert read(p) == 1
count = [0]
def getter(o):
    count[0] += 1
    for i in range(10000):
        garbage = [i]
    return [42]
Plain.x = property(getter)
assert read(p) == [42]
assert count == [1]
def changed(o, name):
    if name == 'x': return 99
    return object.__getattribute__(o, name)
Plain.__getattribute__ = changed
assert read(p) == 99
class Describer:
    def __set_name__(self, cls, name): self.name = name
    def __get__(self, instance, owner): return self.name
class Named:
    second = Describer()
    first = Describer()
assert Named().second == 'second'
assert Named().first == 'first'
"#,
    );
}

#[test]
fn protocol_callbacks_keep_transient_operands_and_keys_alive() {
    check(
        r#"
class Key:
    def __init__(self, n): self.n = n
    def __hash__(self):
        for i in range(10000): garbage = [i]
        return 7
    def __eq__(self, other):
        for i in range(10000): garbage = [i]
        return self.n == other.n
result = {Key(1): [3]}
assert result[Key(1)] == [3]
assert len({Key(2), Key(2)}) == 1
class Printed:
    def __repr__(self):
        for i in range(10000): garbage = [i]
        return 'printed'
assert repr([Printed(), {'x': Printed()}]) == "[printed, {'x': printed}]"
class Result:
    def __bool__(self):
        for i in range(10000): garbage = [i]
        return True
class Equal:
    def __eq__(self, other): return Result()
assert [Equal()].count(1) == 1
"#,
    );
}

#[test]
fn slice_protocols_and_builtin_mutation_preserve_evaluation() {
    check(
        r#"
a = [0,1,2,3,4]
a[1:3] = [9,8,7]
assert a == [0,9,8,7,3,4]
a[::-2] = [40,70,90]
assert a == [0,90,8,70,3,40]
del a[1::2]
assert a == [0,8,3]
a[:] = a
assert a == [0,8,3]
class Index:
    def __index__(self): return 1
assert a[Index():] == [8,3]
class Sequence:
    def __getitem__(self, key):
        assert isinstance(key, slice)
        return (key.start,key.stop,key.step)
assert Sequence()[1:3:2] == (1,3,2)
"#,
    );
}

#[test]
fn class_identity_and_bases_are_read_only_across_tiers() {
    check(
        r#"
class A:
    def value(self): return 1
class B:
    def value(self): return 2
class C(A): pass
c = C()
def store(obj, name, value):
    setattr(obj, name, value)
def direct(obj, change):
    if change:
        obj.__class__ = B
    else:
        obj.x = 1
for i in range(20):
    store(c, 'x', i)
    direct(c, False)
    assert c.value() == 1
failures = 0
for action in range(12):
    try:
        if action == 0: c.__class__ = B
        elif action == 1: C.__bases__ = (B,)
        elif action == 2: store(c, '__class__', B)
        elif action == 3: store(C, '__bases__', (B,))
        elif action == 4: object.__setattr__(c, '__class__', B)
        elif action == 5: object.__setattr__(C, '__bases__', (B,))
        elif action == 6: del c.__class__
        elif action == 7: del C.__bases__
        elif action == 8: delattr(c, '__class__')
        elif action == 9: delattr(C, '__bases__')
        elif action == 10: object.__delattr__(c, '__class__')
        else: object.__delattr__(C, '__bases__')
    except TypeError:
        failures += 1
assert failures == 12
try:
    direct(c, True)
except TypeError:
    failures += 1
assert failures == 13
assert type(c) is C and c.__class__ is C
assert C.__bases__ == (A,) and c.value() == 1
# Ordinary attributes and method replacement remain supported.
store(c, 'x', 42)
assert c.x == 42
C.value = B.value
assert c.value() == 2
calls = []
class Hook:
    def __setattr__(self, name, value): calls.append(name)
    def __delattr__(self, name): calls.append(name)
h = Hook()
try:
    h.__class__ = A
except TypeError:
    pass
else:
    assert False
try:
    del h.__class__
except TypeError:
    pass
else:
    assert False
assert calls == []
"#,
    );
}

#[test]
fn fused_slice_reads_preserve_bounds_hooks_and_assignment_identity() {
    check(
        r#"
log = []
class Index:
    def __init__(self, name, value): self.name, self.value = name, value
    def __index__(self):
        log.append(self.name)
        return self.value
class Custom:
    def __getitem__(self, key):
        self.key = key
        return 10
    def __setitem__(self, key, value):
        assert key is self.key
        self.result = value
start, stop, step = Index('start', 1), Index('stop', 5), Index('step', 2)
for _ in range(10):
    log.clear()
    assert 'abcdef'[start:stop:step] == 'bd'
    assert log == ['step', 'start', 'stop']
    assert [0,1,2,3,4,5][1:5:2] == [1,3]
    assert (0,1,2,3)[::-1] == (3,2,1,0)
    assert 'aé😀z'[::-1] == 'z😀éa'
    obj = Custom()
    log.clear()
    assert obj[start:stop:step] == 10
    assert log == []
    assert obj.key.start is start and obj.key.stop is stop and obj.key.step is step
    obj[1:3] += 2
    assert obj.result == 12
    assert obj[1:3, 4] == 10
    assert obj.key[0].start == 1 and obj.key[1] == 4
    log.clear()
    try:
        'abc'[start:stop:0]
        assert False
    except ValueError:
        pass
    assert log == []
items = [0,1,2,3]
class MutatingIndex:
    def __index__(self):
        items.clear()
        return 0
assert items[MutatingIndex():] == []
"#,
    );
}

#[test]
fn builtin_mapping_probes_keep_numeric_identity_and_custom_callbacks() {
    check(
        r#"
for _ in range(10):
    d = {True: 3, 'abcdefghijklmnopqrstuvwxyz': 4, (1, 'x'): 5}
    d[1.0] += 2
    assert d[True] == 5 and list(d)[0] is True
    d['abcdefghijklmnopqrstuvwxyz'] += 1
    d[(True, 'x')] += 2
    assert d[(1.0, 'x')] == 7
    assert list(d) == [True, 'abcdefghijklmnopqrstuvwxyz', (1, 'x')]
    d[2] = 8
    del d[True]
    d[True] = 9
    assert list(d) == ['abcdefghijklmnopqrstuvwxyz', (1, 'x'), 2, True]
    log = []
    class Key:
        def __hash__(self):
            log.append('hash')
            return hash('x')
        def __eq__(self, other):
            log.append('eq')
            return other == 'x'
    key = Key()
    mixed = {key: 1}
    log.clear()
    mixed['x'] = 2
    assert log == ['eq']
    assert list(mixed)[0] is key
    log.clear()
    assert mixed['x'] == 2
    assert log == ['eq']
    log.clear()
    mixed[key] += 1
    assert log == ['hash', 'hash']
    assert mixed[key] == 3
    values = {True, 1.0}
    values.add(1)
    assert len(values) == 1 and list(values)[0] is True
"#,
    );
}
