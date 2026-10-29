# Inline scalar caches and caller OSR imports

Fixed `advance_system` failing LIR verification during CFG inlining.
Copied callee float caches were added to `scalar_sources`, including caches whose boxed input aliases a caller argument.
The caller's loop-entry state contains that boxed argument, so the old verifier demanded a caller scalar-entry guard for the copied cache.
That guard does not belong at the caller loop entry: the copied body defines and guards the cache at its own execution point.

`Function::scalar_imports` now excludes scalar definitions owned by inline scopes.
The verifier, executable LIR and shared native/copy-and-patch entry protocol all use that physical-import set.
The full `scalar_sources` mapping remains available for typed phi pairing and unboxed-source verification.
Actual caller scalar imports retain their guard requirement, with a specific missing-guard diagnostic.
Benchmark readiness and samples now expose `compilation_errors_total` so valid output cannot hide a rejected artifact.
No heap-storage, stencil, unsafe-code or language-semantic changes were made.

## Validation

**304 tests pass in debug and release**, plus formatting and all-target Clippy with warnings denied.
The regression fails with the old import rule and passes with the fix.
It opens profiling after the caller's argument loads, enters a long loop through OSR, and checks successful compilation and execution.
Coverage includes LIR, Cranelift and copy-and-patch, both bytecode modes, and CFG inlining disabled/enabled.
A subsequent integer argument exercises the callee's own guard and safe deoptimization.
A separate corruption test confirms that removing a real caller scalar-entry guard is still rejected.

## Quick measurements

One timed sample per mode, with two warmups for Barnes–Hut and one for Ticket Search.
All outputs validate; measured samples have zero compilation and deoptimizations.
The candidate reports zero cumulative compilation errors, including warmup.
Builds/tests did not overlap timing.

| Native comparison | Before | Fixed |
| --- | ---: | ---: |
| Barnes–Hut, CFG enabled | 2.736017 s | 2.555374 s |
| Barnes–Hut, default CFG disabled | 2.233631 s | 2.262887 s |
| Ticket Search, CFG enabled | 52.404 ms | 51.490 ms |

The experimental Barnes sample is about **6.6% faster**.
Physical positional frames return **1,618,713 → 1,241,377** and copied entries **1,062,334 → 1,439,670**.
This restores the inlining coverage lost when `advance_system` failed lowering.
The small default/Ticket timing changes are single-sample variation, not established regressions or gains.
CFG inlining remains disabled by default because its complete execution is still slower than the default path.

The initial integer-zero/float transition in `Particle.update` remains a guarded re-specialization, not an invalid-IR error.
The first fixed smoke retains that event and its recompilation; the final timed sample runs after it settles.
No new PyPy or long stability run was performed.

Raw samples, binary hashes, commands, source manifests and validation logs accompany this report.
The initial failure log uses temporary line-labelled verifier diagnostics; those diagnostics are not in production code.
`change.patch` is relative to the uncommitted baseline recorded by `baseline-source-sha256.json`.
No commits or pushes were made.
