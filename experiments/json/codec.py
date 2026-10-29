# This source is prepended to an experiment, so every codec loop passes through HIR.
# Assertions report malformed input until tis supports Python exception handling.
def json_space(s, i):
    while i < len(s) and s[i] in ' \t\r\n':
        i += 1
    return i


def json_hex(s, i):
    n = 0
    for j in range(4):
        assert i + j < len(s)
        c = s[i + j].lower()
        assert c in '0123456789abcdef'
        digit = ord(c) - ord('0') if c <= '9' else ord(c) - ord('a') + 10
        n = n * 16 + digit
    return n


def json_string(s, i):
    assert s[i] == '"'
    i += 1
    parts = []
    while i < len(s):
        c = s[i]
        i += 1
        if c == '"':
            return ''.join(parts), i
        assert ord(c) >= 32
        if c == '\\':
            assert i < len(s)
            c = s[i]
            i += 1
            if c == 'u':
                n = json_hex(s, i)
                i += 4
                # UTF-16 surrogate pairs encode a single Unicode scalar.
                if 55296 <= n <= 56319:
                    assert s[i:i + 2] == '\\u'
                    low = json_hex(s, i + 2)
                    assert 56320 <= low <= 57343
                    n = 65536 + (n - 55296) * 1024 + low - 56320
                    i += 6
                assert not (55296 <= n <= 57343)
                c = chr(n)
            else:
                escapes = {'"': '"', '\\': '\\', '/': '/', 'b': '\b', 'f': '\f', 'n': '\n', 'r': '\r', 't': '\t'}
                assert c in escapes
                c = escapes[c]
        parts.append(c)
    assert False


def json_value(s, i, depth):
    assert depth <= 64
    i = json_space(s, i)
    assert i < len(s)
    c = s[i]
    if c == '"':
        return json_string(s, i)
    if c == '[' or c == '{':
        array = c == '['
        end = ']' if array else '}'
        result = [] if array else {}
        i = json_space(s, i + 1)
        if i < len(s) and s[i] == end:
            return result, i + 1
        while True:
            if array:
                value, i = json_value(s, i, depth + 1)
                result.append(value)
            else:
                assert i < len(s) and s[i] == '"'
                key, i = json_string(s, i)
                i = json_space(s, i)
                assert i < len(s) and s[i] == ':'
                value, i = json_value(s, i + 1, depth + 1)
                result[key] = value
            i = json_space(s, i)
            assert i < len(s)
            if s[i] == end:
                return result, i + 1
            assert s[i] == ','
            i = json_space(s, i + 1)
    for literal, value in [('true', True), ('false', False), ('null', None)]:
        if s[i:i + len(literal)] == literal:
            return value, i + len(literal)
    start = i
    if s[i] == '-':
        i += 1
    assert i < len(s) and s[i] in '0123456789'
    if s[i] == '0':
        i += 1
    else:
        while i < len(s) and s[i] in '0123456789':
            i += 1
    floating = False
    if i < len(s) and s[i] == '.':
        floating = True
        i += 1
        digits = i
        while i < len(s) and s[i] in '0123456789':
            i += 1
        assert i > digits
    if i < len(s) and s[i] in 'eE':
        floating = True
        i += 1
        if i < len(s) and s[i] in '+-':
            i += 1
        digits = i
        while i < len(s) and s[i] in '0123456789':
            i += 1
        assert i > digits
    token = s[start:i]
    if floating:
        value = float(token)
        assert value == value and value != float('inf') and value != -float('inf')
        return value, i
    return int(token), i


def json_loads(s):
    assert isinstance(s, str)
    value, i = json_value(s, 0, 0)
    assert json_space(s, i) == len(s)
    return value


def json_quote(s):
    parts = ['"']
    escapes = {'"': '\\"', '\\': '\\\\', '\n': '\\n', '\r': '\\r', '\t': '\\t', '\b': '\\b', '\f': '\\f'}
    for c in s:
        if c in escapes:
            parts.append(escapes[c])
        elif ord(c) < 32:
            n = ord(c)
            parts.append('\\u00' + '0123456789abcdef'[n // 16] + '0123456789abcdef'[n % 16])
        else:
            parts.append(c)
    parts.append('"')
    return ''.join(parts)


def json_encode(value, active):
    assert len(active) <= 64
    if value is None:
        return 'null'
    if isinstance(value, bool):
        return 'true' if value else 'false'
    if isinstance(value, str):
        return json_quote(value)
    if isinstance(value, int):
        return str(value)
    if isinstance(value, float):
        assert value == value and value != float('inf') and value != -float('inf')
        return str(value)
    # Identity detects cycles without rejecting equal but distinct containers.
    for parent in active:
        assert value is not parent
    active.append(value)
    parts = []
    if isinstance(value, list) or isinstance(value, tuple):
        for item in value:
            parts.append(json_encode(item, active))
        result = '[' + ','.join(parts) + ']'
    else:
        assert isinstance(value, dict)
        for key in value:
            assert isinstance(key, str)
            parts.append(json_quote(key) + ':' + json_encode(value[key], active))
        result = '{' + ','.join(parts) + '}'
    active.pop()
    return result


def json_dumps(value):
    return json_encode(value, [])
