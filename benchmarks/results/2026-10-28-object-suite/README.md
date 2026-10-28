# Barnes–Hut and ticket-search starting baselines

Implementation: `a5bf5d0`. These are the second and third benchmark targets alongside nbody.
Release build, Rust 1.99.0, Apple M1 Pro, macOS 27, ARM64.
CPython 3.15.0 without its JIT; PyPy 7.3.16 / Python 3.10.14 with its JIT.

## Ten-sample medians

| Workload | tis bytecode | tis Cranelift | CPython | PyPy |
| --- | ---: | ---: | ---: | ---: |
| Barnes–Hut, 200 particles / 100 steps | 8.192 s | 11.116 s | 0.619 s | 0.204 s |
| Ticket search, 1,600 tickets / five queries | 98.098 ms | 114.860 ms | 6.043 ms | 3.867 ms |

Native execution is 1.36x slower than bytecode on Barnes–Hut and 1.17x slower on ticket search.
Against PyPy, native execution is 54.5x and 29.7x slower respectively. Nbody's arithmetic-region
improvements have not generalized to these object-heavy workloads. These are starting baselines,
not evidence of a successful optimization.

Each runtime uses one process, two warmup calls, then ten serial interleaved samples in seeded
random order. No builds, tests or profiling overlap measurements. Every call builds a fresh scene
or index. Barnes–Hut includes galaxy setup, tree construction, recursive forces, updates and initial/final
energy calculations. Ticket search includes index building, TF-IDF weighting, sorting and constructing
its output. Compilation and module definitions precede timing. These differ from nbody's timed
advance-only protocol and must not be compared as equal units of work.

Every sample validates the complete output against the CPython-generated fixture. All samples pass;
Barnes–Hut returns `3386.616186`, and ticket search validates all seven output lines including ranked IDs
and integer scores. JSON retains binary/source hashes, runtime versions, warmup timings, raw samples,
compiler counters and deoptimizations. Full min/max and quartiles are retained.

Two warmups do not fully finish tis's staged compilation on these workloads. Some measured Barnes–Hut
calls compile additional code (maximum 5.946 ms); ticket search has one additional 0.672 ms compilation.
All measured deoptimization counts are zero. These compilation times are included rather than removed.
PyPy compiler/deoptimization counters are unknown; its complete warmup and timing samples are retained.

Barnes–Hut native calls make roughly 110.1 million runtime callbacks and 53.7 million root synchronizations.
Ticket search makes about 883 thousand callbacks and 323 thousand synchronizations. These counters establish
that the new workloads spend substantial execution outside the allocation-free nbody regions; no CPU profile
was taken, so they do not by themselves attribute elapsed time to a specific runtime service.
The next work should profile method/attribute access, calls, allocation/GC and dictionary operations.

## Correctness and provenance

Both fixtures come from Flying Circus commit `6b41faf5462a4acaa43b4f1343e3be170c207ac5`.
Barnes–Hut changes only its final output format from `.6e` to `.6f`.
Ticket search changes only the generator expression inside final `join` to a list comprehension.
Original sources and pyperformance license are retained under `tests/vendor/`.
The simulation, deterministic input, indexing, scoring, sort key and query results remain unchanged.

The VM adds builtin `math` imports (`pi`, `sqrt`, `sin`, `cos`, `log`) and stable keyed `list.sort`.
Imports have explicit HIR operations; module identities remain GC-rooted independently of user bindings.
Sorting evaluates keys once, roots detached items and callback-created keys, supports reverse stability,
and restores list contents on errors. Other imports and general generator execution remain unsupported.

185 tests pass in debug and release; all-target Clippy is clean. Additional smoke cases exercise both
workloads through bytecode, LIR, Cranelift and copy-and-patch, along with math aliases/domain errors,
module cache collection, keyed-sort stability, allocating key callbacks and mutation/error recovery.
Default-size native outputs also match CPython before the benchmark run.

## Reproduction

```sh
cargo build --release --examples
python3 tools/bench_suite.py --worker target/release/examples/bench \
  --python /path/to/python3 --pypy /path/to/pypy3 --output /tmp/tis-object-suite
```

Use `--benchmarks barnes-hut` or `--benchmarks ticket-search` to select one target.
Use `--tiers lir copy-patch` for the other execution backends. Nbody retains its existing worker/tools.
