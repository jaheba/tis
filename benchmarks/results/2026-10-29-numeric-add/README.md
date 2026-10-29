# Guarded Int/Float union addition

Validated 29 October 2026. Freshly warmed native Barnes–Hut improves **7.3%**, **2.981 → 2.765 s**, beyond native scratch pooling. One-shot improves **6.6%**, **3.028 → 2.829 s**. A separate direct cumulative control against the original shared-frame/shape baseline measures **41.4%** less time, **4.718 → 2.766 s**. Percentages from stages are not added.

## Change and invariants

An untimed helper histogram found 4.29 million inplace-add and 104 thousand add callbacks per warmed run. Monomorphic Int and Float paths already existed, but numeric operand unions retained generic calls and conservative safepoints. Profiled LIR now emits `NumericAdd` for warmed numeric-only addition sites not covered by existing specializations.

The operation accepts exact Int/Float runtime tags. Int+Int produces checked Int; any Float operand produces Float with signed Int-to-f64 conversion. Bool and all nonnumeric values exit before the original operation; the general HIR fallback retains their semantics, including inplace list identity/mutation. Overflow has a distinct IntegerOverflow exit. Type/overflow exits share explicit pre-operation state, checked by LIR verification; boxed input values must be present and dominate the operation. It cannot allocate or call Python once the tag guards pass, so it needs no safepoint/root publication on that path. No profile alone removes a GC effect from an unguarded operation.

Cranelift emits guards, checked integer arithmetic and mixed floating arithmetic inline. The LIR interpreter and copy-and-patch gateway share a pure numeric evaluator. The copied backend uses its existing callback stencil, with explicit side-exit state and no collection on the numeric path. Eager result observation remains available. No unsafe code, native ABI, stencils, shape/storage invariants or HIR algorithms change. All prior uncommitted work is preserved; no commits/pushes.

## Measurement

Baseline `/private/tmp/tis-profile/native-scratch-*`, candidate `numeric-add-*`; original control `fields-baseline-worker`. At least ten validated samples per binary/backend, serial randomized interleaving. Fresh workers warm twice per sample; one-shot includes startup/parsing/compilation/execution/exit. Nbody and copy-patch Barnes reused-worker controls also warm twice. No builds/tests/profiles overlap formal timing. Raw outliers remain included. All twenty timed runs per fresh native comparison have zero backend compilation/deoptimization. JSON retains platform, executable hashes, samples, warmups and quartiles; source hashes/logs are archived.

| Fresh native comparison | Before (ms) | After (ms) | Time reduction |
| --- | ---: | ---: | ---: |
| Numeric union addition | 2981.053 | 2764.796 | 7.3% |
| Combined overnight work vs shapes | 4717.905 | 2766.117 | 41.4% |

| Warmed native Barnes counter | Before | After | Reduction |
| --- | ---: | ---: | ---: |
| Runtime callbacks | 42,926,781 | 38,534,717 | 10.2% |
| Root synchronizations | 18,689,295 | 14,297,231 | 23.5% |

| Warmed control | Before (ms) | After (ms) | Time reduction |
| --- | ---: | ---: | ---: |
| Ticket, bytecode | 33.537 | 33.346 | 0.6% |
| Ticket, native | 53.704 | 51.795 | 3.6% |
| Ticket, copy-patch | 55.394 | 56.326 | -1.7% |
| Nbody reused, bytecode | 243.316 | 259.387 | -6.6% |
| Nbody reused, native | 8.427 | 8.521 | -1.1% |
| Nbody reused, copy-patch | 164.071 | 164.532 | -0.3% |
| Nbody fresh, bytecode | 246.976 | 245.695 | 0.5% |
| Nbody fresh, native | 8.386 | 8.451 | -0.8% |
| Nbody fresh, copy-patch | 162.166 | 164.092 | -1.2% |
| Barnes–Hut, copy-patch | 5441.978 | 5208.963 | 4.3% |

| One-shot control | Before (ms) | After (ms) | Time reduction |
| --- | ---: | ---: | ---: |
| Barnes–Hut, native | 3028.400 | 2829.015 | 6.6% |
| Ticket, bytecode | 44.873 | 42.821 | 4.6% |
| Ticket, native | 69.838 | 68.702 | 1.6% |
| Ticket, copy-patch | 63.454 | 64.596 | -1.8% |

Negative reduction means slower execution. Reused nbody bytecode showed a 6.6% regression, prompting the separate fresh-worker control above. The fresh bytecode median is 0.5% faster, so the reused-worker slowdown did not persist across fresh processes. Fresh native nbody is 0.8% slower and copy-patch 1.2% slower; warmed/cold ticket copy-patch regresses 1.7%/1.8%. These measured limitations remain visible. Nonnative timing effects are not attributed to guarded arithmetic without additional evidence.

The cumulative control also reduces callbacks from 114,782,260 to 38,534,717 (66.4%). Its original root-sync count was 18,689,295, so the root-sync reduction is 23.5%.

Two six-second CPU samples after all formal timings (all worker runs validated) show remaining exclusive shares: Context::step 17.08%, native entry 14.24%, Heap::get 6.30%, root synchronization 5.24%, lir_helper 3.67% and callback 3.58%. Raw profiles/responses/summary are archived; these shares are contextual attribution, not absolute before/after costs.

## Validation and follow-up

`cargo fmt --all`, all-target Clippy with warnings denied, **233 debug tests**, **233 release tests**, release examples and diff whitespace checks pass. Four new all-tier tests cover numeric result types, i64 boundaries, signed zero/NaN/infinity, overflow, bool and inplace-list fallback without replaying prefix effects, live reference survival across GC, OSR and malformed exit rejection. Existing tests for generic polymorphism and detached local reconstruction were updated to select unsupported Bool profiles and the intended scalar guard, rather than assume numeric unions stay generic or choose the first TypeGuard exit.

See [CHECKPOINT.md](CHECKPOINT.md) for current continuation state. Equality and unary-not callbacks, generic attribute reads, name/call overhead and remaining root synchronization are unfinished. Stop optimization at 07:00 UTC / 08:00 Europe/Berlin.
