# 50-million-step nbody: tis, PyPy and Node

Single serial wall-clock observations on the same Apple M1 Pro, macOS 27 machine.
tis release build at `22dfc7a`, Node v22.22.3, PyPy 7.3.16 / Python 3.10.14.
No build, tests or profiling ran concurrently. These are long-run spot checks, not repeated medians.

| Runtime / source | End-to-end wall time |
| --- | ---: |
| tis native, current Python fixture | 28.829 s |
| PyPy, identical Python script | 17.896 s |
| Node, matching nested arrays and arithmetic, explicit indexing | 27.801 s |
| Node, direct nested-destructuring translation | 63.733 s |
| Node, original Benchmark Game #6 | 3.065 s |

The matched indexed Node version is 3.6% lower in elapsed time than tis (1.037x speed ratio).
The original Node version is 9.4x faster than tis, but changes representation and arithmetic.
PyPy is 1.61x faster than tis on the same Python script.
Single runs cannot establish small differences reliably; setup and JIT warmup are included.
All programs print initial energy -0.169075164 and final energy -0.169059907 to nine decimals.
Python prints more digits; JSON retains its exact stdout and tis aggregate tier counters.

## What is matched

`nbody-50m.py` takes the unchanged Python fixture definitions, offsets momentum, saves initial positions
and velocities, runs two 20,000-step warmups with scene restoration, then advances 50,000,000 steps.
Both matched Node variants use the same warmups/restoration, plain nested arrays for positions,
velocities and body records, a precomputed list of body pairs, `d2 ** -1.5`, and the same arithmetic
association and mutation order. No typed arrays, flat body object fields, square-root substitution,
velocity accumulation across pairs, or changed pair traversal are introduced.
JavaScript arrays stand in for both Python tuples and lists; this is a close source translation,
not identical language semantics. Bounds checks, numeric representations and unpacking rules differ.

The direct translation uses nested destructuring. The indexed translation expands it into ordinary
array loads and uses integer loop indices. Both keep the same representation and numerical work.
This avoids making JavaScript's iterable unpacking cost a mandatory part of the runtime comparison.

The original source uses flat numeric object fields, accumulates a body's velocity in locals across
its inner loop, and computes `dt / (d2 * Math.sqrt(d2))`. It has no explicit warmup/restoration.
It is retained as a distinct optimized-program target, not a matching-kernel runtime comparison.
No claim is made here about the individual contribution of each source difference.

## Provenance and reproduction

Original JavaScript and its attribution header are from
[Benchmark Game Node #6](https://benchmarksgame-team.pages.debian.net/benchmarksgame/program/nbody-node-6.html),
which links the [3-Clause BSD license](https://benchmarksgame-team.pages.debian.net/benchmarksgame/license.html).
Adapted JavaScript retains the header; kernel changes follow tis's pyperformance-derived fixture,
whose provenance and license are in `tests/vendor/pyperformance/`.
The original script and both adaptations are retained here with hashes in `metadata.json`.

```sh
/path/to/release/examples/run --tier native --stats nbody-50m.py
pypy3 nbody-50m.py
node nbody-node-indexed.js 50000000
node nbody-node-matched.js 50000000
node nbody-node-6.js 50000000
```

Each result JSON records its exact command, elapsed time, return code, stdout and stderr.
Binaries were frozen before execution; metadata records their hashes. Run commands serially.
