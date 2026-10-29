# Shared optimizing lowering for copied bodies

Copied callees now use ordinary profiled HIR → LIR lowering with recursive expansion disabled.
A pure graph remapper relocates registers, states, exits and edges into the caller.
The separate arithmetic/comparison/sequence lowering implementation has been removed.
Typed float phi parameters and their boxed sources retain their relationship after relocation.
Nested runtime operations retain the callee environment and logical recursion depth.
Nested readonly expression-call plans currently use a scoped generic-call fallback.

Entry-guard failure retains the original pre-call state.
After entering the copied body, physical state retains the suspended caller continuation rather than the complete pre-call values.
Logical caller/callee snapshots still retain all values and locals needed for GC and deoptimization.
Unused callee states are not copied.
Native callbacks no longer spill original caller-call observations for copied operations that will not observe them.
Copied raw comparisons are recognized as scoped so reopened windows do not record their booleans as caller return types.

## Validation

**302 tests pass in debug and release**, with formatting and all-target Clippy with warnings denied.
New coverage checks ordinary sequence/comparison/integer specialization inside copied bodies,
typed float phis across helpers and collection, guard and overflow exits after writes, and reopened comparison feedback.
A forced collection retains both a detached heap reference and live float phi values.
Backend-entry assertions verify execution through Cranelift and copy-and-patch rather than a silent LIR fallback.
Those assertions were checked afterward in both debug and release; runtime sources did not change.
No unsafe operations, heap-storage changes or stencil changes were added.

## Quick measurements

Per the requested fast workflow, the final comparisons use **one timed sample** per mode.
Barnes–Hut uses two warmups because the initial one-warmup trial still compiled/deoptimized during its first sample.
Ticket Search uses one warmup.
All final warmed samples validate the fixture and have zero compilation and deoptimizations during measurement.
Builds/tests/inspection did not overlap timings.
These are quick directional results, not a stability study.

| Native workload | Previous | Shared lowering |
| --- | ---: | ---: |
| Barnes–Hut, CFG enabled | 3.383593 s | 2.834507 s |
| Barnes–Hut, default CFG disabled | 2.327628 s | 2.319641 s |
| Ticket Search, CFG enabled | 52.408 ms | 52.968 ms |

The experimental Barnes path is about **16.2% faster**, with callbacks
**42,928,650 → 33,035,019** and root synchronizations **11,674,902 → 6,386,499**.
Its physical frame count also changes **1,241,377 → 1,618,713**, and copied entries change **1,439,670 → 1,062,334**.
The measurements compare the complete runtime behavior, not equal inlining coverage.
Default Barnes and Ticket have unchanged execution counters; their small timing differences are not established gains/regressions.
The experimental path still trails default execution, so `cfg_inlining` remains false by default.

Single fresh-process samples include startup, parsing, JIT compilation, execution and exit.
The old/new default comparison is **2.338708 → 2.314589 s**.
A separate new-binary CFG-off/on comparison is **2.840574 → 5.274869 s**.
The variation between separate default samples reinforces that these cold results are exploratory.
No new PyPy comparison was run and parity is not claimed.

Raw data, executable hashes, commands and validation logs accompany this report.
The initial three-sample, one-warmup smoke is retained separately; its first samples include compilation and a deoptimization.
`source-sha256.json` records measured inputs; `delivered-source-sha256.json` adds only the later backend-test assertions.
`change.patch` is relative to the uncommitted baseline recorded in `baseline-source-sha256.json`.
No commits or pushes were made.
