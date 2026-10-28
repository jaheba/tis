# Fixtures

`game_nbody.py` is copied unchanged from Flying Circus's
`src/flying_circus/workloads/game_nbody.py` on 2026-10-28.
It is adapted from pyperformance and the Computer Language Benchmarks Game.
`LICENSE-pyperformance` retains the upstream license; author credits remain in the fixture.
The default workload runs 20,000 steps and prints two energy values.
CPython and tis both produce `-0.169075164` and `-0.169089263`.

`language.py` tests the core independently of the benchmark.
It runs without modifications under both CPython and tis and prints `language core ok`.

`barnes_hut.py` is the Flying Circus `perf_barnes_hut.py` workload, copied on 2026-10-28.
The simulation is unchanged; the final energy uses `.6f` instead of `.6e` because tis currently
supports fixed-point formatting. The default is 200 particles, 100 steps, theta 0.5.
CPython produces `3386.616186`. The pyperformance source and license are retained in
`tests/vendor/pyperformance/`.

`ticket_search.py` is copied from Flying Circus at commit
`6b41faf5462a4acaa43b4f1343e3be170c207ac5`, path `src/flying_circus/workloads/ticket_search.py`.
Only the generator expression passed to the final `join` becomes a list comprehension.
The complete upstream source is retained in `tests/vendor/flying-circus/ticket_search.py`.
The default indexes 1,600 deterministic tickets and ranks five queries.
The `.out` files are captured from CPython and validated on every benchmark sample.
