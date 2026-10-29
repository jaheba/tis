# Bounded effectful leaf inlining

The new pass removes physical callee frames and supports precise exits after completed effects.
It does **not** improve the application benchmarks yet. Final warmed native medians are
2.205831 → 2.257261 seconds for Barnes–Hut (+2.33%) and
50.136 → 50.724 ms for Ticket Search (+1.17%). These regressions are retained,
not presented as noise-free equivalence or a speedup. PyPy parity is not reached.

## Implementation

Monomorphic positional call sites can incorporate closed single-block callee HIR into caller LIR.
The pass accepts one to four positional parameters including a bound receiver, at most 96 HIR
instructions per callee and 192 copied instructions per artifact. Defaults, keyword/star calls,
lexical access, Python calls, branches and loops keep their ordinary execution paths. The existing
readonly expression inliner remains preferred for the bodies it supports. Eager feedback retains
physical callee execution.

Each expansion guards the captured callable's semantic identity/revision, binding, arity, defaults,
receiver validity and recursion limit before effects. Parameters become explicit register aliases.
Copied operations use the same runtime services and scalar operators as ordinary execution.
Numeric addition and floating arithmetic carry nested snapshots; a guard failure after a write
resumes the callee at the failing operation and installs its return in the suspended caller.
Completed writes and argument evaluation are not replayed.

Additional boxed registers have bounded shadow-root storage. Safepoints retain both caller and
callee environments, live values and locals. GC publication excludes only producers proven to
return immediate values; it does not infer that from sampled types. Deferred native floats remain
materialization recipes, not heap roots. Scope records and register-to-HIR origin maps appear in
LIR dumps. The verifier checks scope ownership, dominance, cached roots, primitive proofs, shadow
bounds, pending continuations and helper-state coverage.

A loop may stop sampling its call site before the callee becomes hot. Compilation therefore
refreshes feedback through a weak profile reference while retaining the captured HIR body and
revision. Detached profile copies prevent profile/plan ownership cycles. Live profiles retain
logical call/return/error counts and unexpected operand kinds from nested exits.

Ordinary frame-state records remain 10 machine words (80 bytes here). `Current`/`Inline` environment
modes reuse the previous environment flag's word; inline metadata is allocated separately. Artifacts
without copied bodies bypass shadow-frame setup and choose a runtime callback specialized to omit
inline-root processing. Both native backends use this selection.

## Validation

All-target Clippy with warnings denied, formatting, **289 debug tests** and **289 release tests**
pass on macOS arm64. This includes the concurrently added JSON tests; JSON and branding changes
were preserved. New coverage exercises actual frame elimination in all three compiled tiers and
both bytecode modes, collection after clearing the last ordinary owner, deferred floats across
collection, guard/error exits after writes, division by signed zero, callable replacement during
argument evaluation, unchanged identity with a new revision, recursion limits, live callee
feedback, late-arriving profile data, eager exclusion, and rejected root/shadow corruption.

Final release validation used an isolated target directory after a shared-target cache selected
another checkout's older library. The frozen executable hashes and source manifest identify the
measured build. No builds, tests or profiles overlapped final timings.

## Before/after measurements

All samples validate the unchanged fixture output. Each before/after sample uses a fresh worker,
two complete warmups, then one timed invocation. Ten interleaved randomized pairs include process
layout variation; setup, validation and formatting are outside these warmed timings.

| Native workload | Baseline | Candidate | Median change |
| --- | ---: | ---: | ---: |
| Barnes–Hut | 2.205831 s | 2.257261 s | +2.33% |
| Ticket Search | 50.136 ms | 50.724 ms | +1.17% |

Barnes–Hut eliminates **20,000** `apply_force` frames per invocation:
2,522,535 → 2,502,535 physical positional frames, about 0.79%. Runtime callbacks increase by
160,000 and root synchronizations by 20,000. All measured samples have zero deoptimizations and
zero backend compilation. Ticket Search has zero new leaf hits and unchanged callback/frame counts.
The remaining bootstrap bridges and general helpers offset this small frame reduction.

Fresh-process, no-warmup execution includes startup, parsing, compilation, execution and exit.
Ten randomized samples per runtime give Barnes–Hut
2.260666 → 2.319305 seconds and Ticket Search
68.331 → 68.258 ms.

## Matched PyPy comparison

A separate matched reused-worker protocol uses ten warmups and ten validated samples per runtime,
with the same fixture hash and `(1, 200, 100, 0.5)` parameters. Native tis takes **2.212278 s**,
PyPy **0.151729 s**, and CPython **0.617381 s**. Tis is **14.58×** slower than PyPy,
with an absolute median gap of 2.060549 seconds. Versions, executable hashes, commands,
warmup times and allowed PyPy startup diagnostics are retained in `pypy-warm/`.

## Remaining work and provenance

The recursive/multi-block `calculate_force` path remains outside this pass. It dominates remaining
physical calls. Reaching longer paths, guarding Rust builtins and lexical reads, removing bootstrap
callbacks, propagating types/shapes, and virtualizing tuple/bound-method values remain necessary.
This change is reconstruction and incorporation machinery, not a claim of useful application speedup.

The final baseline is an isolated snapshot containing the prior logical-frame layer plus concurrent
JSON/branding work, before this pass. `integrated-baseline-source.tar.gz` and its manifest preserve
that exact state. `original-baseline-source.tar.gz` preserves the earlier pre-concurrency baseline.
Earlier attempts in `trials/` are diagnostic only: cold captured profiles lost useful field/type
feedback; a large-metadata trial had substantial timing variance; an extra-pointer layout cost
Ticket Search time. No favorable sample or trial was substituted for the final comparison.

Frozen executables remain under `/private/tmp/tis-profile/leaf-integrated-baseline-*` and
`leaf-fastpath-*`; their hashes are in the raw comparison files. Source checksums, compiler/platform
metadata and validation logs accompany the final data. No commits or pushes were made by this task.
