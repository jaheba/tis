# Deterministic Sieve of Eratosthenes; inclusive upper bound.
LIMIT = 1_000_000


def sieve(limit):
    if limit < 2:
        return (0, 0, 0)
    prime = [True] * (limit + 1)
    prime[0] = False
    prime[1] = False
    p = 2
    while p * p <= limit:
        if prime[p]:
            for multiple in range(p * p, limit + 1, p):
                prime[multiple] = False
        p += 1
    count = 0
    total = 0
    last = 0
    for p in range(2, limit + 1):
        if prime[p]:
            count += 1
            total += p
            last = p
    return (count, total, last)


def bench_sieve(limit):
    result = sieve(limit)
    print(result[0], result[1], result[2])


bench_sieve(LIMIT)
