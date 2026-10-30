raw = b'\x00\x80\xffAB\n'
assert type(raw) is bytes
assert bytes(raw) is raw
assert str(raw) == repr(raw)
assert repr(raw) == "b'\\x00\\x80\\xffAB\\n'"
assert len(raw) == 6
assert list(raw) == [0, 128, 255, 65, 66, 10]
assert tuple(raw) == (0, 128, 255, 65, 66, 10)
assert raw[True] == 128
assert raw[-1] == 10
assert raw[::-2] == b'\nA\x80'
assert raw[20:] == b''
assert bool(b'') is False
assert raw + b'!' == b'\x00\x80\xffAB\n!'
assert b'\x80\xff' in raw
assert 128 in raw and False in raw
assert 1 not in raw
assert b'' in raw
assert b'a' < b'b'
assert b'a' != 'a'
assert bytes([65, False, True, 255]) == b'A\x00\x01\xff'
assert bytes(3) == b'\x00\x00\x00'
assert bytes() == b''
assert bytes(source=[65, 66]) == b'AB'
assert bytes('é', encoding='utf-8') == b'\xc3\xa9'
assert raw * 2 == 2 * raw
assert raw * -1 == b''
assert raw * False == b''
assert b','.join([b'a', b'b', b'']) == b'a,b,'
assert b'  AbC\xff \t'.strip().lower() == b'abc\xff'
assert b'abc\xff'.upper() == b'ABC\xff'
assert b'abc'.startswith((b'z', b'a'))
assert b'abc'.endswith(b'b', 0, 2)
assert b'abc'.startswith(b'', 4) is False
assert b'abc'.count(b'', 4) == 0
assert b'aaa'.count(b'aa') == 1
assert b'abcabc'.find(b'bc') == 1
assert b'abcabc'.rfind(b'bc') == 4
assert b'abc'.find(b'') == 0
assert b'abc'.rfind(b'') == 3
assert b'abc'.replace(b'', b'-') == b'-a-b-c-'
assert b'aaaa'.replace(b'aa', b'b', 1) == b'baa'
assert b'a--b--'.split(b'--') == [b'a', b'b', b'']
assert b'a--b--'.rsplit(b'--', 1) == [b'a--b', b'']
assert b' a b  '.split(None, 0) == [b'a b  ']
assert b' a b  '.rsplit(None, 0) == [b' a b']
assert b'ABC123'.isalnum()
assert b'123'.isdigit()
assert not b'\xff'.isalpha()
assert b'\t\r\n '.isspace()
assert bytes.fromhex('00 80 ff').hex() == '0080ff'
assert b'\xc3\xa9'.decode() == 'é'
assert 'é'.encode() == b'\xc3\xa9'
assert b'\xffa'.decode(errors='ignore') == 'a'
assert b'\xffa'.decode('utf-8', 'replace') == '�a'
assert 'éA'.encode('ascii', errors='replace') == b'?A'
assert b'\xffA'.decode('ascii', 'ignore') == 'A'
assert hash(bytes([65,66])) == hash(b'AB')
key = bytes([65,66])
d = {key: 1}
assert next(iter(d)) is key
assert d[b'AB'] == 1
assert b'AB' in {key}
for bad in [-1, 256]:
    try:
        bytes([bad])
    except ValueError:
        pass
    else:
        assert False
class Index:
    def __index__(self):
        for i in range(2000):
            garbage = [i]
        return 1
assert raw[Index()] == 128
assert Index() in b'\x01'
assert raw * Index() == raw
assert Index() * raw == raw
assert [1, 2] * Index() == [1, 2]
class ByteSource:
    def __bytes__(self):
        return b'custom'
assert bytes(ByteSource()) == b'custom'
for repeat in range(8):
    assert bytes([i for i in range(20)]) == bytes(range(20))
    assert len(b'abc' * 1000) == 3000

assert b'\x0b'.isspace()
assert b'\x0ba\x0b'.strip() == b'a'
assert b'a\x0bb'.split() == [b'a', b'b']
assert bytes.fromhex('41\x0b42') == b'AB'
