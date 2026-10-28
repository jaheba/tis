# Checked sequence access and unpacking, 2026-10-28

List/tuple indexing and unpacking now have non-allocating LIR paths.
Generic unpacking also avoids unnecessary iteration for exact sequences.
This reduces warmed n-body execution time in all four tiers.

## Shared runtime and HIR

Generic tuple unpacking validates length and reuses the immutable tuple.
Generic list unpacking validates length and copies a tuple snapshot directly, without allocating an iterator.
Other iterables retain generic collection behavior.

The frontend loads every outer unpack element before evaluating or storing assignment targets.
This preserves snapshot behavior when targets mutate an aliased source, such as `a[1], a[0] = a`.
Nested unpacking still happens in assignment order, so earlier assignments remain visible if a later nested unpack fails.
Starred targets retain generic snapshot construction.

## LIR and native execution

Runtime observations of exact list/tuple receivers select `SequenceUnpack` and `SequenceGet`.
Known internal tuple snapshots also permit sequence reads without speculative type observations.
The operations check container kind, current length and integer bounds through shared Rust heap accessors.
Negative integer indices are supported; other key kinds and mismatches side-exit before the operation.
Generic bytecode then preserves the original result or exception without repeating earlier effects.

An unpack view can alias its source list only when all element reads are contiguous and precede assignment side effects.
HIR selection checks that the view cannot escape.
LIR verification checks the eager read bundle, indices, uses, binding maps and side-exit state, including malformed counts.
If those conditions do not hold, the generic tuple snapshot remains.

The accessors do not allocate VM objects, call Python, mutate a sequence or collect.
Payload borrows end inside each accessor; no list buffer pointer survives a call.
Native reads therefore skip full GC-root synchronization and publish returned heap references directly.
Heap identity/generation validation remains in the normal heap accessor.
Read operations still depend on mutable heap state and cannot be hoisted across writes or Python calls.
These are checked Rust gateways, not raw loads using an embedded Rust object layout.
Both native backends use the same callback protocol and side-exit mechanism.

## Evidence and warm n-body comparison

A separate warmed baseline profile identified root synchronization, callback dispatch, iteration and heap access costs.
The before/after `sample` captures are retained as `before.sample.txt.gz` and `after.sample.txt.gz`.
They were taken outside timed measurements, after process startup, while the worker repeated the workload.
These short intrusive captures are evidence of call paths, not precise elapsed-time attribution.
The final capture still shows callback dispatch and root synchronization as substantial remaining costs.

Apple M1 Pro, macOS 27.0, Rust 1.99.0, default Cargo release profile.
Baseline core revision: `2f729ae14e1803408bba6ab56a57c7b8584c63c4`.
The baseline executable is the exact SSA/safepoint candidate retained from the previous experiment; SHA-256 hashes match.
Candidate revision: `557c3b78dd25c2d78adf38ec9527b6e6d83abb4b`.

Each worker performs two warmups followed by ten timed `advance(0.01, 20000)` calls.
Workers run serially in shuffled round order, seed 20261028.
Initial scene restoration, energy validation, process startup and compilation are outside the timer.
Every result matches the expected energy, and optimized samples enter their selected tier once with no compilation or
deoptimization during timing.
Builds, tests and sampling run separately from the timed comparison.
Staged feedback remains enabled; `advance` retains one profiling window and 2,912 sampled site executions in each tier.

| Tier | SSA/safepoint baseline | Sequence candidate | Time reduction | Candidate interquartile range |
| --- | ---: | ---: | ---: | ---: |
| Bytecode | 1.045 s | 0.758 s | 27.49% | 0.757–0.765 s |
| Reference LIR | 1.407 s | 0.540 s | 61.60% | 0.539–0.541 s |
| Cranelift | 0.938 s | 0.380 s | 59.54% | 0.378–0.385 s |
| Copy-and-patch | 0.998 s | 0.425 s | 57.42% | 0.424–0.426 s |

The native tiers now take roughly half the time of candidate bytecode.
The comparison measures the combined generic-unpack, eager-read and LIR changes; it does not isolate each contribution.
These are warmed execution measurements for one workload on one machine.
`comparison.json` retains all individual samples, warmups, validation data, hashes and summaries.

A separate ten-sample CPython 3.15.0 control measured a median of 0.075896 s, with an interquartile range of
0.075248–0.076664 s.
Its JIT is unavailable and disabled.
The control uses the same worker scene restoration and energy checks, and runs after the tier comparison and sampling.
Cranelift remains approximately 5× slower than this control; this ratio spans separate runs, not interleaved paired samples.
`cpython.json` and `cpython_control.py` retain the observations and control procedure.

`advance.lir.txt` is the first n-body dump from the diagnostic `hotspots` example, which uses 16-entry/128-backedge thresholds.
It contains seven `SequenceUnpack` operations and 29 `SequenceGet` operations.
These are static counts rather than execution frequencies.

## Validation

All 118 tests pass in debug and release builds; formatting and Clippy with warnings denied pass.
Eight new sequence tests cover specialized lowering, container changes, negative/bool/extreme indices, changed lengths,
aliasing target mutations, nested error ordering, generic iterables, Unicode, starred targets, live child references during
source mutation/GC, and verification of escaped/interrupted/overflowing unpack bundles.
Existing deoptimization, native lifetime, eager-feedback, SSA and full n-body tests also pass.

## Reproduce

Save the baseline worker before rebuilding, then run from the repository root:

```sh
cargo build --release --examples
python3 tools/compare_nbody.py \
  --baseline /path/to/ssa-baseline-nbody-bench \
  --candidate target/release/examples/nbody_bench \
  --output /tmp/tis-sequence-results
```

Use a new output directory and keep builds/tests/profiling separate from timing.
The benchmark fixture and its license remain in `../2026-10-28-nbody/`.
