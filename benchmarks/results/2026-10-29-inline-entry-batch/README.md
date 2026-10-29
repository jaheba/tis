# Fused inline-call entry callbacks

Bound leaf calls now share one native callback for the callable guard, caller environment, callee environment and receiver.
Free leaf calls combine the first three operations.
Both Cranelift and copy-and-patch use the shared runtime helpers; the LIR interpreter keeps its existing instructions.
The original guard and side exit remain in LIR.
Setup reads execute only after the guard succeeds, before any copied effects.
The recognizer requires contiguous helpers with matching operands, states and read-only effects.
All produced values are reloaded into native register caches and published as ordinary roots.
When profiling restarts, setup uses the ordinary helper observation path.

## Validation

Formatting, all-target Clippy with warnings denied, **290 debug tests** and **290 release tests** pass.
Tests confirm fused entries for both native backends, free and bound calls, and observation after profiling restarts.
Existing tests cover callable/revision changes, argument evaluation, collection, and deoptimization after writes.
No stencil or calling-convention layout changed.

## Measurements

Ten randomized, interleaved before/after pairs use fresh workers with two warmups per sample.
Each result is checked against the unchanged benchmark fixture.
Builds, tests and profiles completed before serial timing started.
The baseline is the immediately preceding leaf-inlining build, including the concurrent JSON work.

| Native workload | Baseline warmed median | Candidate warmed median |
| --- | ---: | ---: |
| Barnes–Hut | 2.262062 s | 2.261621 s |
| Ticket Search | 51.710 ms | 49.052 ms |

Barnes–Hut runs 20,000 fused entries per invocation.
Callbacks decrease **32,325,625 → 32,265,625**, a reduction of 60,000 (0.19%).
Physical positional frames remain 2,502,535; root synchronizations are unchanged.
All measured warmed samples have zero deoptimizations and zero backend compilation.
Warmed Barnes–Hut is effectively unchanged; this does not establish an application speedup.
Ticket Search has zero fused entries, with unchanged call/frame counts, so its timing change cannot be credited to entry fusion.

Ten fresh-process, zero-warmup samples include startup, parsing, JIT compilation, execution and exit:
Barnes–Hut **2.328134 → 2.309427 seconds**, Ticket Search **66.949 → 68.689 ms**.
Ticket's warmed and cold timings move in opposite directions; executable layout and measurement variation remain relevant.
No new PyPy comparison was run, and no claim is made that the previously measured gap has closed.

## Next bottleneck

The recursive, multi-block `calculate_force` path still uses physical frames.
It includes global/math reads, several returns, tuple results, and recursive calls.
The current closed single-block inliner excludes those operations.
Reaching that path requires multi-block incorporation, guarded runtime/global operations and correct scoped call continuations.
Removing three setup callbacks from 20,000 leaf calls cannot address its dominant cost.

Raw samples, executable hashes, commands, source manifests and validation logs accompany this report.
`change.patch` records this change against the uncommitted baseline described by `baseline-source-sha256.json`.
No commits or pushes were made.
