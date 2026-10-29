# Positional helper checkpoint

Active follow-up: `../2026-10-29-invariant-locals/CHECKPOINT.md`. Diagnostic overlay was restored exactly; helper-detail.json in numeric-calls records the untimed profile.

## Current status — 2026-10-28 23:46 UTC / 29 October 00:46 Europe/Berlin

This optimization is complete and validated. No build, test, profile or benchmark process remains active. No commits or pushes. All prior authorized uncommitted work remains intact. Current source matches frozen `/private/tmp/tis-profile/positional-final-{worker,run,nbody}`; use these as the baseline for the next experiment. All compiled source changes relative to the prior checkpoint are in `src/tier.rs`, plus the new tests. `source-sha256.json` records all Rust/build/fixture inputs.

Final measured results, ten samples per binary/backend:

- Freshly warmed native Barnes vs fields-hinted: 3.6279189585 -> 3.4156096665 s, 5.9% less time. Zero timed compilation/deopts across all 20 samples.
- Separate direct cumulative control vs original shared-frame/shape baseline: 4.68713575 -> 3.4295916045 s, 26.8% less time; also zero timed compilation/deopts.
- Reused native Barnes: 3.672210708 -> 3.4434613545 s, 6.2% less time.
- Cold native Barnes: 3.7034447915 -> 3.4382447705 s, 7.2% less time.
- Copy-patch Barnes: 5.8349901875 -> 5.6970539165 s, 2.4% less time.
- Fresh ticket: native 53.841 -> 51.935ms (-3.5% time), copy-patch 57.072 -> 54.757ms (-4.1%), LIR -2.5%; bytecode +3.0% time despite reused bytecode -0.8% time (short-workload/layout sensitivity remains documented).
- Nbody native/bytecode flat. Copy-patch's reused 6.3% gain disappears in fresh-worker control (+0.4% time), so treat it as flat.
- Runtime callbacks/root syncs remain 50,713,113 / 18,689,295; helpers are cheaper. Positional dispatch count becomes 6,722,672, leaf dispatch count 3,944,835. Legacy bytecode_* counters now also include shared LIR/native dispatch, not bytecode fallback.

Validation: fmt/all-target Clippy -D warnings, 223 debug + 223 release tests pass. Full report, raw JSON (including fresh-process controls), source/binary hashes, compressed logs and post-change CPU profiles are in this directory.

## Next authorized work

Hard cutoff remains 2026-10-29 07:00 UTC / 08:00 Europe/Berlin. Check current time before operations; do not start a long operation that can cross it. At/after cutoff stop implementation/benchmarking and finalize overall report only. Existing overnight heartbeat remains useful; do not duplicate it.

1. Use the new post-change profile: Context::step 18.16% exclusive, native entry 8.50%, root sync 6.94%, callback 4.19%, lir_helper 4.13%, Heap::get 4.00%, Vm::operation 2.74%. Samples are two six-second warmed runs, no builds/tests/timings overlapped; all worker outputs validate.
2. Before larger specialization, obtain an exact untimed helper-op histogram. Temporary diagnostic builds are fine if restored precisely to the current snapshot before final validation. Focus on generic mixed Int/Float +,-,* or comparisons; existing monomorphic numeric lowering misses polymorphic field types. Any numeric fast path must preserve integer overflow, exact mixed large-integer comparison, float/NaN/signed-zero behavior, evaluation order and full fallback. Keep guards/state/roots explicit in LIR.
3. Another measured lead: pool native entry rooted-index/helper-argument scratch across invocations, with independent buffers for every active recursive native frame and clearing on all success/error/panic paths. Native register buffers already pool. Do not retain Python roots in unused capacity.
4. A borrowed-leaf gateway could skip root sync only with proven no-Python/no-GC behavior, GC-pressure fallback before any side effects, correct view/region flushing, and observing/error fallbacks. Do not weaken effects/root maps based on feedback alone or retry a mutating builtin after execution.
5. Finish one substantive change plus full gates and >=10 serial interleaved validated warmed/cold checks before beginning another. No builds/tests/profiles during formal timing. Check ticket and nbody; retain raw outliers and explicitly report regressions. Do not commit/push.

## Earlier chronological notes


Started 2026-10-28 23:25 UTC / 29 October 00:25 Berlin. Hard cutoff remains 2026-10-29 07:00 UTC / 08:00 Berlin.

Baseline is the completed native-fields checkpoint: /private/tmp/tis-profile/fields-hinted-{worker,run,nbody}; preserve all existing uncommitted work. No commits/pushes.

Current experiment: src/tier.rs::lir_helper routes only all-positional CoreOp::Call through call_from_registers. Existing LIR/native gateways already root all operands through calls. Keyword/starred calls retain the general binder. New positional_helpers tests exercise warmed shared leaf dispatch, defaults, bound methods, closures, recursive GC, keyed sort, general binders, and error cleanup. No unsafe or ABI changes.

Focused tests and release examples build running in exec session 48661. Logs /private/tmp/tis-profile/positional-{focused,build}.log. Next: inspect outcomes, freeze candidate, collect >=10 serial interleaved native Barnes samples vs fields-hinted. No builds/tests/profiles during formal timing. Retain only measured improvement; revert only this experiment from /private/tmp/tis-profile/positional-before-tier.rs if it fails. Then full fmt/Clippy/debug+release and ticket/nbody/one-shot checks.

First ten-sample native Barnes comparison is complete: fields-hinted baseline 3.672210708 -> candidate 3.4434613545 s (6.2% less time). Both have identical runtime callback/root-sync counts. Shared positional dispatch increases positional fast-path count from 2,050 to 6,722,672 and leaf calls from 1,147 to 3,944,835 per run. Focused tests pass. Full fmt/Clippy/debug/release/build now running in session 99223. Next freeze final executables and complete serial regression/cold/fresh checks before reporting.

Full gates complete: fmt/all-target Clippy, 223 debug + 223 release tests pass. Candidate frozen as positional-final-{worker,run,nbody}, Rust/build/fixture inputs hashed in source-sha256.json. Serial benchmark pipeline /private/tmp/tis-profile/positional-final-compare.py is starting: ticket (including LIR), nbody, copy-patch Barnes, cold ticket/Barnes, freshly warmed native vs prior checkpoint, freshly warmed native vs original shapes baseline. No code work/builds/tests/profiles until it finishes.

Pipeline active in session 24190. Early regression checks complete: warmed ticket native 53.656 -> 52.667ms, LIR/copy-patch generally improve; nbody native/bytecode flat, copy-patch 172.808 -> 161.870ms (possible process-layout sensitivity, verify fresh workers before attributing). Copy-patch Barnes 5.8349901875 -> 5.6970539165 s (2.4%). Cold/fresh native comparisons still running. Do not overlap them.

Next-stage ideas only (NOT implemented): collect an untimed exact helper-op histogram, then investigate guarded numeric +,-,* (mixed Int/Float sites currently prevent monomorphic lowering) with checked integer overflow and full semantic fallback, and/or a noncollecting borrowed-leaf-call gateway that avoids root synchronization only when GC is not due. The latter needs an explicit proven no-Python/no-GC leaf contract, must reject sort/keyed callbacks, retain observation fallback, avoid replaying mutations if allocations cross the GC threshold, flush/invalidate any active region/heap views, and preserve full roots at the allocating/calling fallback. Do not weaken LIR effects/root maps based on observed types alone.

Main pipeline complete: freshly warmed native vs prior checkpoint 3.6279189585 -> 3.4156096665 s (-5.9% time), zero timed compilation/deopts. Direct cumulative comparison vs original shapes: 4.68713575 -> 3.4295916045 s (-26.8%), also zero timed compilation/deopts. Cold native Barnes vs prior checkpoint 3.7034447915 -> 3.4382447705 s (-7.2%). Extra fresh-worker ticket/copy-patch nbody controls running in session 30450. After they finish, run the new candidate CPU profile script positional-profile.py (no timed benchmark overlap), archive report and promote latest checkpoint.

Additional next-stage option after profiling: pool native entry's rooted-index/helper-argument scratch buffers across invocations (each active nested invocation must own separate buffers; clear logical values on every success/error/panic path; never retain Python roots through unused capacity). Native register buffers and register bytecode scratch already pool, while Context currently creates rooted/helper_arguments vectors per entry. Do not start before this checkpoint is fully reported.

Latest completed checkpoint: ../2026-10-29-invariant-locals/CHECKPOINT.md; use frozen invariant-minimal binaries for further work.
