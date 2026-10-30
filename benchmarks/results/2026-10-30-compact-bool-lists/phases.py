def allocate(n):
    flags = [True] * (n + 1)
    flags[0] = flags[1] = False
    return flags


def mark(flags, n):
    for i in range(2, int(n**0.5) + 1):
        if flags[i]:
            for j in range(i * i, n + 1, i):
                flags[j] = False


def collect(flags, n):
    return [i for i in range(2, n + 1) if flags[i]]
