# Native builtin sieve optimization

The unchanged sieve through 1,000,000 exposed a Cranelift path slower than the bytecode interpreter.
A warmed baseline call made 9,529,494 runtime callbacks and 6,405,938 root synchronizations, with no
compilation or deoptimization during the sample. The macOS sample profile is retained alongside its worker responses.
Its largest stacks include callback dispatch, Heap::get, root synchronization and iterator bookkeeping.
The initial marking loop compiled before the final scan had operand feedback; that scan retained generic GetItem calls.

## Changes

- Guarded Cranelift reads for exact boxed lists and tuples, including generic GetItem sites without operand feedback.
- Direct indexed stores into existing boxed list storage, guarded by current heap identity, generation, representation,
  integer index, bounds and a mutable pointer published by an exclusive heap borrow.
  Artifacts with cached native heap views retain the existing runtime path.
- Direct builtin range stepping updates the real iterator's Cell<usize> cursor. Aliases and side exits see every step.
  Exhaustion, custom iteration and invalid probes fall back before mutation.
- Quiet native loop backedges accumulate exact counts in invocation-owned scratch. Counts merge on return, error or
  deoptimization; active feedback keeps immediate callbacks. CFG-expanded artifacts retain their existing policy.
- Shared non-collecting runtime probes preserve the original fully rooted helper for misses and active feedback.

The source workload and algorithm are unchanged. No answers are precomputed and no tis execution delegates to Python.
Native rows are refreshed after mutation, invalidated before mutable access and on collection, and reloaded after callbacks.
Only pointers from exclusive list borrows and Cell cursors can be written. Rust bool bytes are normalized to 0/1.
All generic LIR effects, guards and pre-effect reconstruction states remain available on fallback.

## Validation

- cargo fmt --all, all-target Clippy with -D warnings, full cargo test: 393 tests pass.
- New tests cover exact backedge counts against bytecode, shared iterator aliases, negative/extreme range steps,
  exhaustion, errors, list growth/shrinkage, representation changes, boxed value kinds, GC and custom protocols.
- Cold sequence blocks and foreign kinds are checked. Region tests retain exact cursor assertions using Cell::get.
- Three extracted Python assertion programs independently pass CPython 3.15 through uv.
- Integrated source hashes match the tested source; executable hashes and build details are in provenance.json.

## Sieve timings

Three serial, interleaved samples per runtime. Warmed measurements retain one VM and use one preparatory call;
source definitions and compilation precede timing. Every invocation allocates a fresh sieve.
One-shot measurements use fresh processes and include parsing, compilation, execution, output and exit.
Executable/file caches are primed by --version only; no workload execution warms a one-shot process.
These are short diagnostic samples, not statistical significance claims. PyPy has only one warmed preparatory call.

| Runtime | Warmed ms | One-shot ms |
| --- | ---: | ---: |
| before-bytecode | 387.745 | 387.303 |
| after-bytecode | 377.878 | 379.060 |
| before-native | 505.802 | 513.190 |
| after-native | 45.650 | 54.493 |
| cpython | 51.739 | 76.984 |
| pypy | 17.486 | 46.085 |

A warmed optimized call records 1,000,998 direct sequence loads, 2,122,049 direct list stores and 3,122,047 direct range steps.
Runtime callbacks fall to 161,184; root synchronizations fall to 160,843.
All measured optimized warmed samples have zero backend compilation, deoptimizations and compilation errors.

A separate three-sample native/CPython follow-up measured 44.608 / 50.997 ms
(native/CPython 0.875). PyPy remains faster than tis.

## One-shot regression checks

The same full-size source runs on each runtime and complete stdout is checked against CPython.
Barnes-Hut enables the readonly/mutation regions for both native binaries; all other tiers use their normal flags.

| Workload | Bytecode before → after ms | Native before → after ms |
| --- | ---: | ---: |
| nbody | 289.329 → 285.941 | 36.877 → 35.634 |
| barnes-hut | 2859.774 → 2852.595 | 672.234 → 668.738 |
| unpacking | 184.397 → 183.477 | 228.426 → 227.414 |

These checks show no material regression at this sampling level. No wider benchmark speedup is claimed.
Full commands, output validation and individual timings are in measure.py, warmed.json and oneshot.json.
Baseline executables are from the original sieve report at runtime revision c084b35; new benchmark work remains uncommitted.
Native in this report means Cranelift. Copy-and-patch keeps the shared safe runtime probes, but does not receive these
Cranelift-specific emitted loads/stores/range steps or quiet block batching; no copy-and-patch speed claim is made.
