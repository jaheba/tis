# Actual readonly Python body inlining

Validated 29 October 2026. Fresh warmed native Barnes–Hut improves **0.8%, 2.179 → 2.162 s** beyond the name-load baseline; one-shot improves **0.9%, 2.224 → 2.204 s**. A separate direct cumulative comparison against the original shared-shape baseline measures **52.7%, 4.561 → 2.158 s** less time. The incremental Barnes gain is modest; stage percentages are not added.

## What is inlined

This is callee-body incorporation into caller native code. Runtime profiling copies a bounded typed expression plan from actual callee HIR, retaining its stable FunctionId and code revision. LIR carries this body with the original caller Call's arguments, state, effects and root map. Native compilation emits the copied arithmetic, comparisons, boolean inversion and branch selects directly in the caller. It neither evaluates the callee body in a runtime helper nor invokes a separate callee entry.

A single closed C preflight validates actual Function/Method identity and revision, receiver/positional binding, argument handles, recursion limit, collection pressure and primitive operands. It reads float arguments or own instance fields with existing shape/slot/bounds caches into private primitive scratch. It performs no callee arithmetic, VM allocation, collection or Python execution. Repeated field/argument leaves share guards because the supported body cannot write or reenter. All arithmetic executes in generated caller code after preflight succeeds.

Supported: at most four exact positional parameters, twelve acyclic HIR blocks, sixty-four source instructions and 128 expression nodes; exact Float operands; direct instance-field reads from arguments/receiver; float constants; +, −, ×, six numeric comparisons, boolean not and readonly short-circuit CFGs converted to typed selects. Physical locals map to positional arguments by parameter name, independently of their sorted slot order. Barnes–Hut Rectangle.contains meets this subset and **255,303 actual bodies inline per warmed run**.

Excluded: defaults, keyword-only/variadic/keyword calls, lexical/global reads inside the callee, recursion/loops, Python calls, writes, allocation, integer operands, division/erroring arithmetic, arbitrary attribute lookup and reference returns. Missing/nonfloat fields, target replacement or unsupported bindings take the original call. Method object allocation before the call is still present. LIR and copy-patch use the authoritative original call; this first body emitter is native-only. Eager/collecting and region/nest execution also retain original calls.

## Safety and exits

All guards complete before copied body execution. A miss materializes full original caller state and executes the original Call exactly once. No callee effect, allocation, Python call or speculative guard can occur after a hit, so nested frame reconstruction and replay are unnecessary for this subset. A missing field in a normally untaken short-circuit branch causes a miss, not an early error. Speculated float operations and existing field reads cannot raise or mutate Python state.

The actual callable is guarded, including same FunctionId with a changed HIR revision. Closure environment independence follows from rejecting lexical reads; defaults and nontrivial binding are rejected. Even unused arguments retain original Unbound/stale-reference validation. Recursion depth and due collection use the original rooted call so skipped callee entry cannot hide those checkpoints. Existing caller root maps remain explicit. New scratch is sized once during native compilation, placed after all return/region/local metadata, initialized and cleared by the existing invocation-owned scratch protocol, and contains only primitive leaves. Its pointers exist only for the synchronous callback. Copied storage modules and the 16-byte Value/Slot ABI are unchanged.

Callee frames **2,777,838 → 2,522,535** and root synchronizations **7,714,644 → 7,459,341**, each **255,303 fewer**. Callbacks remain **32,165,647**: preflight replaces the original call callback. These are actual counters, not inferred from timing. Earlier inline-field/block counters fall because the copied body no longer enters the callee artifact; they are not lost semantic operations.

## Measurements

Frozen baseline `/private/tmp/tis-profile/name-load-*`, final candidate `body-inline-checked-*`; cumulative original `fields-baseline-worker`. At least ten validated serial randomized interleaved samples per binary/backend. Fresh workers warm twice; one-shot includes startup, parsing, compilation, execution and exit. Nbody and copy-patch Barnes use warmed reused workers. No builds/tests/profiles overlap timings. All fresh native timed samples have zero compilation/deoptimization. Raw JSON includes binary hashes, validation, counters and dispersion; negative reduction means slower execution.

| Comparison | Before (ms) | After (ms) | Time reduction |
| --- | ---: | ---: | ---: |
| Barnes–Hut fresh native | 2179.202 | 2161.703 | 0.8% |
| Barnes–Hut one-shot native | 2223.838 | 2204.405 | 0.9% |
| Cumulative native vs original shapes | 4560.848 | 2157.755 | 52.7% |
| Ticket fresh bytecode | 31.303 | 31.177 | 0.4% |
| Ticket one-shot bytecode | 40.374 | 39.487 | 2.2% |
| Nbody warmed bytecode | 240.139 | 241.075 | -0.4% |
| Ticket fresh native | 47.169 | 46.944 | 0.5% |
| Ticket one-shot native | 64.062 | 64.258 | -0.3% |
| Nbody warmed native | 8.386 | 8.366 | 0.2% |
| Ticket fresh copy-patch | 52.260 | 52.273 | -0.0% |
| Ticket one-shot copy-patch | 60.873 | 60.637 | 0.4% |
| Nbody warmed copy-patch | 161.008 | 162.440 | -0.9% |
| Barnes–Hut warmed copy-patch | 4877.104 | 4916.174 | -0.8% |

The rejected first version scanned full LIR to size scratch on every entry and regressed substantially. The second cached scratch sizing and was flat (2.176111 → 2.175275 s). The retained version also shares identical primitive leaves and instance borrows. All three trials remain archived; no speed claim uses the first two candidates.

## Validation

Formatting, all-target Clippy with warnings denied, **266 debug tests**, **266 release tests**, release examples and whitespace checks pass. Twelve added tests cover an actual copied body and frame elimination; actual method receiver and method replacement; function replacement executing effects once across 12,000 garbage allocations; untaken missing-field short circuit; exclusions for defaults/closures/recursion/keywords; reordered parameter names versus local slots; unchanged FunctionId with a new HIR revision; and preserved callee recursion-limit errors. Additional tests cover NaN/signed-zero behavior and eager fallback, hidden caller references across GC after a successful inline, and all-tier loop OSR. The plan builder runs the normal HIR graph verifier before indexing host-edited source; a malformed-HIR regression verifies an IR error instead of a profiling panic. Existing GC/deoptimization and native scratch suites also pass.

Post-timing profiles and the pure-call one-shot benchmark are archived separately. See CHECKPOINT.md for continuation, limitations and remaining bottlenecks. No commits or pushes. Hard stop07:00 UTC /08:00 Europe/Berlin.

## PyPy parity target

**Parity is not reached.** Verified local `/opt/homebrew/bin/pypy3`: PyPy7.3.16/Python3.10.14 on arm64. Full versions and executable hashes are archived. The exact fixture hash and parameters `(1 loop, 200 particles, 100 steps, theta=0.5)` match across runtimes. Every sample validates the same energy output. No PyPy JIT-disabling/logging, custom GC or CPython JIT/GIL environment variables were set. The known PyPy startup CPU-L2-cache warning is preserved; other diagnostics are rejected.

| Matched protocol (10 samples each) | tis native | PyPy | CPython | tis / PyPy | Absolute tis–PyPy gap |
| --- | ---: | ---: | ---: | ---: | ---: |
| Warmed reused worker, 10 warmups each | 2.146900 s | 0.149115 s | 0.610217 s | **14.40×** | **1.997785 s** |
| Fresh process, no explicit warmup | 2.204917 s | 0.279834 s | 0.638477 s | **7.88×** | **1.925083 s** |

Warmed timings cover one identical function invocation, excluding setup, formatting and validation for all runtimes. In this final comparison, 1 of ten native samples include 1.557ms total backend compilation; all samples are retained; all ten have zero deoptimizations. PyPy JIT activity was not instrumented; raw warmup timings are retained. Fresh-process timings include startup, parsing, JIT warmup/execution and exit for all runtimes. Samples are serial with no overlapping builds/tests/profiles. Do not compare a warmed tis number to a cold PyPy number. Matching the measured warm PyPy time requires another **93.1% reduction** from current tis time.

## Pure arithmetic-call control

The archived `pure_calls.py` runs500 caller warmups and200,000 numeric calls, with exact assertions/output validation. Ten serial randomized fresh-process pairs measure **97.214 → 77.405ms (20.4% less time)**, including startup/parsing/JIT/execution/exit. This is not a warmed kernel timing and does not replace the Barnes/PyPy comparison.

## Remaining work

The prior callee histogram identifies about2.14million calculate_force calls, dominating the remaining2.52million physical positional frames. The current inliner handles contains but excludes calculate_force's nested field reads, global bindings, sqrt, division/error edges, tuple allocation/returns and recursion. General inlining needs scoped origins, nested caller/callee states and GC roots, exact resume positions and pending-return continuations. Restarting the original Call after completed callee writes/calls is invalid.

[Next inlining design](NEXT-INLINING.md) records concrete current API limits and a bounded prefix/trace alternative. Longer compiled paths, cross-body type/shape propagation, machine identity/field guards, less boxing/materialization and separate bound-method allocation elimination remain unfinished. The existing preflight alone cannot close the measured gap. No recursive/effectful inliner or PyPy parity claim is included.

Earlier v3 evidence is preserved in `v3/`; the final hardened source also verifies HIR before copying indices. This validation fix changes no supported Python body semantics and has its own final timings and266-test gates. No commits or pushes.

## Final post-timing profile

Two six-second samples after every formal timing; raw samples and validated worker responses are archived. These are exclusive percentages, except where a name identifies unattributed generated code. No cause is inferred for `???`.

| Function | Exclusive samples |
| --- | ---: |
| tis::native_runtime::execute::<<tis::native::NativeCode>::execute::{closure#4}> | 12.83% |
| ??? | 11.51% |
| <tis::native_runtime::Context>::step | 10.00% |
| <tis::vm::Vm>::execute_window | 7.18% |
| <tis::heap::Heap>::get | 5.51% |
| <tis::vm::Vm>::sync_lir_roots::<<tis::native_runtime::Context>::step::{closure#3}> | 4.05% |
| tis::native_runtime::guarded_helper_callback | 3.30% |
| <tis::heap::Heap>::allocate | 2.99% |
| <tis::heap::Heap>::collect | 2.44% |
| tis::native_runtime::callback | 2.42% |

The final controls also retain regressions: copy-patch Barnes–Hut is0.8% slower and copy-patch nbody0.9% slower than the name-load baseline; their cause is unresolved. See the full table rather than treating the native gain as universal.
