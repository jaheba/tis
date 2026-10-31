for text in ['', 'abcdef', 'aé中🐍' * 4]:
    characters = list(text)
    for needle in ['', 'a', '中', '中🐍', 'missing']:
        for start in [-1000, -1, 0, 2, 1000]:
            for stop in [-1000, -1, 0, 3, 1000]:
                low = max(len(characters) + start, 0) if start < 0 else start
                high = max(len(characters) + stop, 0) if stop < 0 else min(stop, len(characters))
                found = []
                if low <= high:
                    for position in range(low, high + 1):
                        if position + len(needle) <= high and ''.join(characters[position:position + len(needle)]) == needle:
                            found.append(position)
                assert text.find(needle, start, stop) == (found[0] if found else -1)
                assert text.rfind(needle, start, stop) == (found[-1] if found else -1)
                assert text.startswith(needle, start, stop) == (bool(found) and found[0] == low)
                assert text.endswith(needle, start, stop) == (bool(found) and found[-1] == high - len(needle))
                if needle == '':
                    assert text.count(needle, start, stop) == len(found)
assert 'aaaa'.count('aa') == 2
assert 'aé中🐍'.index('中') == 2
assert 'aé中🐍中'.rindex('中') == 4
assert 'abc'.find('', 10**100) == -1
assert 'abc'.find('a', -10**100, 10**100) == 0
assert 'abc'.find('a', None, None) == 0
text = 'aé中🐍' * 1000
class Bound:
    def __index__(self):
        assert text.find('中') == 2
        assert text[-2] == '中'
        temporary = [[i] for i in range(4500)]
        return 4
assert text.find('中', Bound()) == 6
assert text.rfind('中', 0, Bound()) == 2
assert text.endswith('🐍', 0, Bound())
assert text.startswith(('aé', 123))
assert text.endswith(('中🐍', 123))
try:
    text.startswith((123,), len(text) + 1)
    assert False
except TypeError:
    pass
class BadBound:
    def __index__(self):
        raise ValueError('bound first')
try:
    text.startswith(123, BadBound())
    assert False
except ValueError as error:
    assert str(error) == 'bound first'
for operation in [lambda: text.index('missing'), lambda: text.rindex('missing')]:
    try:
        operation()
        assert False
    except ValueError:
        pass
assert 'AΣ AΣB İ'.lower() == 'aς aσb i̇'
assert 'Straße 中🐍'.upper() == 'STRASSE 中🐍'
assert 'Api-ÄNDERUNG 中文'.lower() == 'api-änderung 中文'
print('search semantics ok')
