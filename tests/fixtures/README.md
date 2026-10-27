# Fixtures

`game_nbody.py` is copied unchanged from Flying Circus's
`src/flying_circus/workloads/game_nbody.py` on 2026-10-28.
It is adapted from pyperformance and the Computer Language Benchmarks Game.
`LICENSE-pyperformance` retains the upstream license; author credits remain in the fixture.
The default workload runs 20,000 steps and prints two energy values.
CPython and tis both produce `-0.169075164` and `-0.169089263`.

`language.py` tests the core independently of the benchmark.
It runs without modifications under both CPython and tis and prints `language core ok`.
