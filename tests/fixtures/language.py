# Core semantics exercised independently of the n-body benchmark.
def fib(n):
    if n < 2:
        return n
    return fib(n - 1) + fib(n - 2)
assert fib(12) == 144

def counter(start=0):
    value = start
    def increment(step=1):
        nonlocal value
        value += step
        return value
    def read():
        return value
    return increment, read
inc, read = counter(5)
assert inc() == 6
assert inc(4) == read() == 10
assert counter()[1]() == 0

def defaults(value, items=[]):
    items.append(value)
    return items
assert defaults(1) is defaults(2)
assert defaults(3) == [1, 2, 3]

def arguments(a, /, b=2, *rest, c=3, **kw):
    return a, b, rest, c, kw
assert arguments(1, 4, 5, 6, c=7, extra=8) == (1, 4, (5, 6), 7, {'extra': 8})
assert arguments(*(1, 2), **{'c': 9}) == (1, 2, (), 9, {})
assert (lambda x, y=3: x + y)(4) == 7
assert (lambda: 8)() == 8

x = 90
assert [x * y for x in range(4) for y in range(3) if x != y] == [0, 0, 0, 2, 0, 2, 0, 3, 6]
assert x == 90
assert {x: x * x for x in range(4) if x % 2} == {1: 1, 3: 9}
assert len({x % 3 for x in range(10)}) == 3
funcs = [lambda: x for x in range(3)]
assert [f() for f in funcs] == [2, 2, 2]

class Point:
    kind = 'point'
    def __init__(self, x, y=0):
        self.x = x
        self.y = y
    def move(self, dx):
        self.x += dx
        return self.x
    def squared(self):
        return self.x ** 2 + self.y ** 2
class Child(Point):
    kind = 'child'
p = Child(3, y=4)
assert p.squared() == 25
move = p.move
assert move(2) == 5
assert p.x == 5 and p.kind == 'child'
assert isinstance(p, Point) and isinstance(p, Child)
Point.extra = 12
assert p.extra == 12
p.extra = 13
assert p.extra == 13 and Point.extra == 12

a = [1, 2]
b = a
a += (3, 4)
assert a is b and b == [1, 2, 3, 4]
a *= 2
assert a is b and len(b) == 8
assert a[::-2] == [4, 2, 4, 2]
assert 'hé😊'[::-1] == '😊éh'
first, *middle, last = range(5)
assert (first, middle, last) == (0, [1, 2, 3], 4)
a, (b, c) = [1, [2, 3]]
assert (a, b, c) == (1, 2, 3)
d = {(1, 'x'): 3, 1.5: 4, True: 5}
assert d[(1, 'x')] == 3 and d[1] == 5 and d[1.5] == 4
view = d.values()
d['new'] = 7
assert len(view) == 4 and list(view) == [3, 4, 5, 7]
assert list(d.items())[0] == ((1, 'x'), 3)

seen = []
for i in range(5):
    if i == 1:
        continue
    if i == 4:
        break
    seen.append(i)
else:
    seen.append(99)
assert seen == [0, 2, 3]
for i in []:
    pass
else:
    seen.append(8)
assert seen == [0, 2, 3, 8]
assert 0 or [] or 'last' == 'last'
assert (0 and 1 / 0) == 0
assert not (2 < 1 < 1 / 0)
assert -7 // 3 == -3 and -7 % 3 == 2
assert list(zip([1, 2], [3])) == [(1, 3)]
assert list(enumerate(['a', 'b'], 5)) == [(5, 'a'), (6, 'b')]
print('language core ok')
