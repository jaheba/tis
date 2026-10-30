use tis::{Tiering, Vm, compile};

#[test]
fn protected_suites_preserve_control_and_shared_locals() {
    let source = r#"
log = []
def f(n):
    x = 0
    try:
        x = n + 1
        if n == 1:
            return x
        if n == 2:
            raise ValueError('bad')
        x += 2
    except ValueError as e:
        log.append(e.args[0])
        x = 9
    else:
        log.append('else')
    finally:
        log.append(x)
        x = 100
    return x
assert f(0) == 100
assert f(1) == 2
assert f(2) == 100
assert log == ['else', 3, 2, 'bad', 9]
def override():
    try:
        raise ValueError('discard')
    finally:
        return 42
assert override() == 42
out = []
for i in range(5):
    try:
        if i == 1:
            continue
        if i == 3:
            break
        out.append(i)
    finally:
        out.append(10 + i)
assert out == [0, 10, 11, 2, 12, 13]
def clear_binding():
    e = 1
    try:
        raise ValueError('x')
    except ValueError as e:
        assert e.args == ('x',)
    try:
        e
    except UnboundLocalError:
        return True
assert clear_binding()
try:
    try:
        raise ValueError('a')
    finally:
        raise TypeError('b')
except TypeError as e:
    assert e.args == ('b',)
"#;
    for tier in [
        Tiering::Bytecode,
        Tiering::Lir,
        Tiering::Native,
        Tiering::CopyPatch,
    ] {
        let mut vm = Vm::new();
        vm.tiering = tier;
        vm.thresholds.function_entries = 2;
        vm.thresholds.loop_backedges = 4;
        vm.run_program(&mut compile(source).unwrap()).unwrap();
    }
}

#[test]
fn exception_identity_hierarchy_chaining_and_gc() {
    let source = r#"
class Problem(ValueError):
    pass
class Other(Problem):
    pass
try:
    try:
        raise Problem('original')
    except ValueError as first:
        assert isinstance(first, Problem)
        assert first.args == ('original',)
        raise Other('second') from first
except Problem as second:
    assert isinstance(second, ValueError)
    assert second.__cause__.args == ('original',)
    assert second.__context__ is second.__cause__
    assert second.__suppress_context__
    saved = second
for i in range(10000):
    garbage = [i]
assert saved.__cause__.args == ('original',)
assert str(saved) == 'second'
assert repr(saved) == "Other('second')"
try:
    try:
        1 // 0
    except ArithmeticError as original:
        try:
            raise ValueError('hidden') from None
        except ValueError as masked:
            assert masked.__cause__ is None
            assert masked.__context__ is original
            assert masked.__suppress_context__
        raise
except ZeroDivisionError:
    pass
else:
    assert False
try:
    try:
        raise KeyError('key')
    finally:
        raise
except LookupError as exc:
    assert exc.args == ('key',)
"#;
    for tier in [
        Tiering::Bytecode,
        Tiering::Lir,
        Tiering::Native,
        Tiering::CopyPatch,
    ] {
        let mut vm = Vm::new();
        vm.tiering = tier;
        vm.thresholds.function_entries = 2;
        vm.thresholds.loop_backedges = 4;
        vm.run_program(&mut compile(source).unwrap()).unwrap();
        let saved = vm.get_global("saved").unwrap();
        vm.collect_garbage(&[saved]).unwrap();
        assert!(vm.heap.get(saved).is_ok());
    }
}

#[test]
fn context_managers_unwind_and_suppress_without_formatting_exceptions() {
    let source = r#"
log = []
class Failure(ValueError):
    def __str__(self):
        log.append('unexpected string conversion')
        raise TypeError()
class Manager:
    def __init__(self, name, suppress=False):
        self.name = name
        self.suppress = suppress
    def __enter__(self):
        log.append('enter:' + self.name)
        return self
    def __exit__(self, kind, value, traceback):
        log.append('exit:' + self.name)
        if kind is not None:
            assert isinstance(value, kind)
            assert traceback is not None
            assert type(traceback).__name__ == 'traceback'
        return self.suppress
with Manager('a') as a, Manager('b') as b:
    assert a.name == 'a' and b.name == 'b'
    log.append('body')
assert log == ['enter:a', 'enter:b', 'body', 'exit:b', 'exit:a']
log = []
with Manager('suppress', True):
    raise Failure('safe')
assert log == ['enter:suppress', 'exit:suppress']
log = []
def f():
    with Manager('return'):
        return 42
assert f() == 42
assert log == ['enter:return', 'exit:return']
log = []
for i in range(3):
    with Manager('loop'):
        if i == 0: continue
        break
assert log == ['enter:loop', 'exit:loop', 'enter:loop', 'exit:loop']
log = []
try:
    with Manager('propagate'):
        raise Failure('safe')
except ValueError as e:
    assert e.args == ('safe',)
    assert e.__traceback__ is not None
assert log == ['enter:propagate', 'exit:propagate']
try:
    len(1)
except TypeError:
    pass
else:
    assert False
assert type(True) is bool
assert type(1) is int
assert issubclass(bool, int)
assert issubclass(int, object)
"#;
    for tier in [
        Tiering::Bytecode,
        Tiering::Lir,
        Tiering::Native,
        Tiering::CopyPatch,
    ] {
        let mut vm = Vm::new();
        vm.tiering = tier;
        vm.thresholds.function_entries = 2;
        vm.thresholds.loop_backedges = 4;
        vm.run_program(&mut compile(source).unwrap()).unwrap();
        assert_eq!(vm.tier_stats.compilation_errors, 0);
    }
}

#[test]
fn exception_args_identity_attributes_and_chaining_cycles() {
    let source = r#"
e = ValueError('x')
assert e.args is e.args
args = (1, 2)
e.args = args
assert e.args is args
e.args = [3,4]
assert e.args == (3,4)
e.extra = [42]
assert e.extra == [42]
del e.extra
assert not hasattr(e, 'extra')
class Custom(ValueError): pass
c = Custom('x')
c.args = [5,6]
assert c.args == (5,6)
a = ValueError('a')
b = ValueError('b')
try:
    try:
        raise a
    except ValueError:
        try:
            raise b
        except ValueError:
            raise a
except ValueError:
    assert a.__context__ is b
    assert b.__context__ is None
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
    }
}
