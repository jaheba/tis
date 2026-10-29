# Audited native name loading

Validated 29 October 2026. Fresh warmed native Barnes–Hut improves **2.8%, 2.251 → 2.187 s** beyond pressure-guarded tuple literals. One-shot improves **3.5%, 2.314 → 2.233 s**. The separate direct cumulative comparison with the original shared-shape baseline measures **51.8%, 4.524 → 2.180 s** less time. Stage percentages are not added.

## Change and safety

Quiet native LoadName uses the existing closed guarded gateway and calls the authoritative Vm::load_name_slot with its original name/cache. This removes general dispatcher bookkeeping without duplicating scope lookup or introducing raw namespace pointers. The audited body/callees read validated environments, update cache/stat metadata and format errors; they allocate no VM objects, trace no GC, enter no Python and alter no frames. Rust error-string allocation does not allocate a VM object. Safety follows from the actual audited bodies, not the &Heap type (the heap supports interior allocation).

Eager/collecting, region and nest execution retain the original helper. A collecting miss precedes lookup; no observation or effect is replayed. Full LIR effects, state/root maps and VM/Slot/stat layouts remain unchanged. Arbitrary reference results remain in the native image and are reloaded into its cache, then rooted at the next explicit safepoint or published before caller collection. Missing-name errors preserve earlier effects and use existing cleanup. Panic containment relies on the same closed no-frame-change contract. Copied storage modules are unchanged.

The existing lookup still controls cache environment identity, namespace binding slots, global and nonlocal resolution, class dynamic lookup and UnboundLocalError/NameError. Rebinding returns the new value through the original slot cache. No new callable/name identity speculation is introduced.

Callbacks **32,165,647**, root synchronizations **7,714,644**, and name-cache hits **6,734,228** are unchanged per warmed native Barnes run. This phase reduces work inside callbacks, not their count. Prior callback/root reductions remain those measured in earlier phases.

## Measurement

Baseline frozen `/private/tmp/tis-profile/tuple-budget-*`, candidate `name-load-*`, direct cumulative original `fields-baseline-worker`. At least ten validated serial randomized interleaved samples per binary/backend. Fresh workers warm twice per sample; one-shot includes startup/parsing/compilation/execution/exit. Nbody and copy-patch Barnes use warmed reused workers. No builds/tests/profiles overlap formal timing; outliers remain included. All fresh native timed samples have zero compilation/deoptimization. Raw JSON records binary hashes, platform, warmups, validation and dispersion. Negative reduction means slower execution.

| Comparison | Before (ms) | After (ms) | Time reduction |
| --- | ---: | ---: | ---: |
| Barnes–Hut fresh native | 2251.115 | 2186.984 | 2.8% |
| Barnes–Hut one-shot native | 2314.341 | 2232.744 | 3.5% |
| Cumulative native vs original shapes | 4523.867 | 2180.140 | 51.8% |
| Ticket fresh bytecode | 31.247 | 31.365 | -0.4% |
| Ticket one-shot bytecode | 41.135 | 40.390 | 1.8% |
| Nbody warmed bytecode | 239.113 | 239.719 | -0.3% |
| Ticket fresh native | 49.658 | 47.186 | 5.0% |
| Ticket one-shot native | 67.031 | 65.918 | 1.7% |
| Nbody warmed native | 8.339 | 8.376 | -0.4% |
| Ticket fresh copy-patch | 52.324 | 52.560 | -0.5% |
| Ticket one-shot copy-patch | 61.096 | 61.063 | 0.1% |
| Nbody warmed copy-patch | 162.866 | 161.960 | 0.6% |
| Barnes–Hut warmed copy-patch | 4884.289 | 4973.925 | -1.8% |

Copy-and-patch Barnes–Hut regresses **1.8%** in the first comparison and **1.3%** in an independent ten-pair repeat (4.861 → 4.926 s); both raw results are retained. Its name-lookup execution path is unchanged. The cause is unresolved; shared code layout is a possibility, not a proven explanation. This tradeoff is retained for the requested native gains, including native ticket-search improving 5.0% warmed. Other measured controls vary from 1.8% faster to 0.5% slower.

## Validation

Formatting, all-target Clippy with warnings denied, **254 debug tests**, **254 release tests**, release examples and whitespace checks pass. Four added tests cover cached global rebinding scalar→reference, eager feedback, missing-name error ordering with exactly one earlier effect, closure/nonlocal/class scope and a 9,000-iteration loop across LIR/native/copy-patch. A strong GC test loads a global reference, clears its binding in a callee, allocates 12,000 garbage objects there, and validates identity/survival followed by eventual release through explicit collection.

Post-timing profiles are archived separately. See CHECKPOINT.md for continuation and remaining bottlenecks. No commits/pushes. Hard stop07:00 UTC /08:00 Europe/Berlin.
