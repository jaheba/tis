rows = [(2, 'a'), (1, 'b'), (2, 'c'), (1, 'd')]
assert sorted(rows, key=lambda p: p[0]) == [(1, 'b'), (1, 'd'), (2, 'a'), (2, 'c')]
assert sorted(rows, key=lambda p: p[0], reverse=True) == [(2, 'a'), (2, 'c'), (1, 'b'), (1, 'd')]
assert rows == [(2, 'a'), (1, 'b'), (2, 'c'), (1, 'd')]
assert sorted({'c': 1, 'b': 2, 'a': 3}) == ['a', 'b', 'c']
assert sorted((x for x in range(8) if x % 2), reverse=True) == [7, 5, 3, 1]
assert sorted([3, 2, 1], key=None, reverse=[]) == [1, 2, 3]
assert sorted([], key=42) == []
assert sorted(range(4096, -1, -1)) == list(range(4097))
events = []
class Source:
    def __iter__(self):
        events.append('iter')
        return iter([3, 1, 2])
class Reverse:
    def __bool__(self):
        events.append('reverse')
        for i in range(1500):
            garbage = [i]
        return True
def key(x):
    events.append(x)
    return x
assert sorted(Source(), key=key, reverse=Reverse()) == [3, 2, 1]
assert events == ['iter', 'reverse', 3, 1, 2]
events.clear()
try:
    sorted(Source(), reverse=Reverse(), unexpected=True)
except TypeError:
    pass
else:
    assert False
assert events == ['iter']
# Python 3.14+ uses truth conversion for reverse, rather than __index__.
class IndexOnly:
    def __index__(self):
        assert False
assert sorted([2, 1], reverse=IndexOnly()) == [2, 1]
original = [3, 1, 2]
class MutatingReverse:
    def __bool__(self):
        original.append(0)
        return True
assert sorted(original, reverse=MutatingReverse()) == [3, 2, 1]
assert original == [3, 1, 2, 0]
original = [3, 1, 2]
original.sort(reverse=MutatingReverse())
assert original == [3, 2, 1, 0]
class Row:
    def __init__(self, n, label):
        self.n = n
        self.label = label
class BoolResult:
    def __init__(self, result):
        self.result = result
    def __bool__(self):
        for i in range(1500):
            garbage = [i]
        return self.result
class Key:
    def __init__(self, n):
        self.n = n
    def __lt__(self, other):
        return BoolResult(self.n < other.n)
key_calls = []
def make_key(row):
    key_calls.append(row.label)
    for i in range(1500):
        garbage = [i]
    return Key(row.n)
result = sorted([Row(2, 'a'), Row(1, 'b'), Row(2, 'c'), Row(1, 'd')], key=make_key)
assert [row.label for row in result] == ['b', 'd', 'a', 'c']
assert key_calls == ['a', 'b', 'c', 'd']
class Marker(Exception):
    pass
def bad_key(x):
    if x == 2:
        raise Marker('key')
    return x
try:
    sorted([3, 2, 1], key=bad_key)
except Marker as exc:
    assert str(exc) == 'key'
else:
    assert False
class BadReverse:
    def __bool__(self):
        raise Marker('reverse')
try:
    sorted([3, 2, 1], reverse=BadReverse())
except Marker as exc:
    assert str(exc) == 'reverse'
else:
    assert False
assert sorted([3, 2, 1]) == [1, 2, 3]
print('sorted compatibility ok')
