# Instance shapes and guarded slot access

Each class owns an empty layout. Adding fields follows shared transitions; instances with the same class and field-addition order share immutable name-to-slot metadata. Values live in a vector. Overwrites preserve the shape. Constructor writes following existing transitions do not allocate name strings.

Register bytecode and LIR attribute helpers cache a shape ID and slot. Hits access the vector directly. Different layouts resolve by name and update the monomorphic cache. Shape IDs never wrap; caches contain neither object identities nor pointers into instance vectors. Class members and inherited methods remain dynamic fallbacks. GC traces the class and every instance value; shape metadata contains no Python values. `Rc` owns Rust layout metadata, not Python object references.

Cranelift's field-probe callback now reads these slots; copy-and-patch uses the same LIR helpers. Native code still calls Rust for field access. This does not implement inline machine-code shape guards/field loads, object descriptors or class-method caches. The stack interpreter uses shaped storage through the general runtime operations. The 16-byte Value/native ABI remains unchanged.

## Warmed execution

Baseline is `4e28b84` plus the preceding uncommitted shared-frame work, frozen before shapes. Apple M1 Pro, macOS 27, Rust 1.99 release defaults. Ten serial interleaved samples per backend, two warmups, every output validated and feedback enabled. No tests/builds/profiling overlapped timings. Raw JSON records binary hashes and counters.

| Benchmark/backend | Before | After | Time reduction |
| --- | ---: | ---: | ---: |
| Barnes–Hut, bytecode | 3.058 s | 2.598 s | 15.0% |
| Barnes–Hut, native | 4.958 s | 4.711 s | 5.0% |
| Ticket search, bytecode | 33.784 ms | 34.295 ms | -1.5% |
| Ticket search, native | 57.581 ms | 56.646 ms | 1.6% |
| Ticket search, copy-patch | 58.258 ms | 58.650 ms | -0.7% |
| Nbody (20k), bytecode | 243.147 ms | 242.937 ms | 0.1% |
| Nbody (20k), native | 8.840 ms | 8.760 ms | 0.9% |
| Nbody (20k), copy-patch | 165.102 ms | 163.707 ms | 0.8% |

Barnes–Hut bytecode improves 15.0% and native 5.0%. Other changes are below 2% and are treated as roughly flat rather than attributed to shapes. Bytecode and native each report 34,452,153 shape-read hits and 285,846 shape-write hits per warmed Barnes–Hut run. Counters count cache hits, not total reads/writes.

## Fresh process

Ten ticket-search samples without warmup, including startup and compilation:

| Backend | Before | After | Time reduction |
| --- | ---: | ---: | ---: |
| bytecode | 42.556 ms | 42.417 ms | 0.3% |
| native | 72.437 ms | 72.138 ms | 0.4% |
| copy-patch | 67.453 ms | 67.200 ms | 0.4% |

Cold ticket search is flat. Scheduling outliers remain in JSON. CPython measured 34.610 ms including startup, not a warmed comparison.

## Validation

All 216 debug and release tests pass; all-target Clippy with warnings denied passes. New tests cover transition sharing, value overwrites without layout changes, different field orders, field additions, polymorphic receivers, field value-type changes, class/inherited-method replacement, GC of cached instances, and reuse of a field site with a new class layout. Language tests exercise stack/register bytecode and all execution tiers.

Reproduce with `tools/compare_nbody.py` and `tools/bench_oneshot.py`, using the frozen baseline/candidate executables recorded in JSON. Generic workers handle Barnes/ticket; `nbody_bench` handles nbody.
