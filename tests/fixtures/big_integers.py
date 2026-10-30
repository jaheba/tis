large = 1267650600228229401496703205376
assert large == 2 ** 100
assert 0x10000000000000000000000000 == large
assert 9223372036854775807 + 1 == 9223372036854775808
assert -9223372036854775808 - 1 == -9223372036854775809
assert -(-9223372036854775808) == 9223372036854775808
assert abs(-9223372036854775808) == 9223372036854775808
assert large * large == 2 ** 200
assert large - (large - 1) == 1
assert type(large - (large - 1)) is int
assert type(large) is int
assert isinstance(large, int)
assert str(large) == '1267650600228229401496703205376'
assert repr(-large) == '-1267650600228229401496703205376'
assert bool(large) and bool(-large)
assert not (large - large)
assert -large // 3 == -422550200076076467165567735126
assert -large % 3 == 2
assert large // -3 == -422550200076076467165567735126
assert large % -3 == -2
assert -9223372036854775808 // -1 == 9223372036854775808
assert -9223372036854775808 % -1 == 0
assert large / large == 1.0
assert (2 ** 2000) / (2 ** 1999) == 2.0
assert (2 ** 2000 + 1) / (2 ** 2000) == 1.0
assert 1 / (2 ** 1074) > 0.0
assert 1 / (2 ** 1075) == 0.0
assert large + 0.5 == float(large)
assert large > float(large - 1) - 1e16
assert large + 1 > float(large)
assert large - 1 < float(large)
assert large == float(large)
assert large != float('nan')
assert not (large < float('nan'))
assert -large > float('-inf')
assert large < float('inf')
assert large & (large - 1) == 0
assert large | 1 == large + 1
assert large ^ 1 == large + 1
assert ~large == -large - 1
assert 1 << 100 == large
assert large >> 100 == 1
assert -large >> 200 == -1
assert -large >> large == -1
assert 1 >> large == 0
assert (-1) ** (-large - 1) == 1.0
assert type((-1) ** (-large - 1)) is float
assert True + large == large + 1
assert True * large == large
assert large.bit_length() == 101
assert (large - 1).bit_count() == 100
assert (-large).bit_count() == 1
assert (5).bit_length() == 3
assert True.bit_count() == 1
assert int('1267650600228229401496703205376') == large
assert int('0x1_0000000000000000000000000', 0) == large
assert int(b'1267650600228229401496703205376') == large
assert int('10000000000000000000000000', base=16) == large
assert int(float(large)) == large
assert int(large) is large
class Converted:
    def __int__(self):
        for i in range(1000):
            garbage = [i]
        return large + 1
assert int(Converted()) == large + 1
class Indexed:
    def __index__(self):
        return large + 2
assert int(Indexed()) == large + 2
key = int(str(large))
d = {key: 'first'}
assert next(iter(d)) is key
assert d[large] == 'first'
assert d[float(large)] == 'first'
d[float(large)] = 'second'
assert len(d) == 1
assert d[large] == 'second'
assert large in {large, float(large)}
assert len({large, float(large)}) == 1
assert hash(large) == hash(float(large))
assert hash(large) == 549755813888
assert hash(-large) == -549755813888
assert hash(2 ** 61 - 1) == 0
import json
assert json.loads(str(large)) == large
assert json.dumps([large, -large]) == '[1267650600228229401496703205376, -1267650600228229401496703205376]'
for op in range(3):
    try:
        if op == 0:
            bad = large // 0
        elif op == 1:
            bad = large % 0
        else:
            bad = large / 0
    except ZeroDivisionError:
        pass
    else:
        assert False
try:
    bad = 1 << -1
except ValueError:
    pass
else:
    assert False

assert [1, 2, 3][-large:large] == [1, 2, 3]
assert [1, 2, 3][::large] == [1]
assert [1, 2, 3][::-large] == [3]
for count in [large, -large]:
    try:
        bad = [1, 2] * count
    except OverflowError:
        pass
    else:
        assert False
try:
    bad = [1, 2][large]
except IndexError:
    pass
else:
    assert False

assert hex(large) == '0x10000000000000000000000000'
assert bin(large) == '0b1' + '0' * 100
assert oct(-large) == '-0o2' + '0' * 33
assert pow(2, 100) == large
assert pow(2, 100, 97) == 16
assert pow(3, -1, 11) == 4
assert pow(2, 100, -97) == -81
assert divmod(-large, 3) == (-422550200076076467165567735126, 2)
assert divmod(1.0, 0.1) == (9.0, 0.09999999999999995)
assert 1.0 // 0.1 == 9.0
assert 1.0 % 0.1 == 0.09999999999999995
class Division:
    def __rdivmod__(self, left):
        return (left, large)
assert divmod(3, Division()) == (3, large)

for invalid in ['abc', ' ', 'a\tbc']:
    try:
        int(invalid)
    except ValueError as e:
        assert str(e) == 'invalid literal for int() with base 10: ' + repr(invalid)
    else:
        assert False
try:
    int(b'\xc2\xa012\xc2\xa0')
except ValueError:
    pass
else:
    assert False
for base in [1, 37, -2, large]:
    try:
        int('1', base)
    except ValueError:
        pass
    else:
        assert False

for repeated in range(24):
    values = [1]
    source = (values[large] for ignored in range(1))
    try:
        next(source)
    except IndexError:
        pass
    else:
        assert False
