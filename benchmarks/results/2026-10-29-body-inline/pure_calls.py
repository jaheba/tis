def square(x):
    return x*x

def caller(x):
    return square(x)+1.0

def bench(n):
    total=0.0
    for i in range(n):
        total+=caller(3.0)
    return total

for i in range(500):
    assert caller(3.0)==10.0
assert bench(200000)==2000000.0
print('ok')
