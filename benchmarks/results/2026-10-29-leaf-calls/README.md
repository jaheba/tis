# Guarded scalar builtin calls

Validated 29 October 2026. Freshly warmed native Barnes–Hut improves **4.8%**, **2.580 → 2.456 s**, beyond typed unary-not. One-shot improves **5.6%**, **2.656 → 2.508 s**. A separate direct cumulative control against the original shared-frame/shape baseline measures **49.6%** less time, **4.890 → 2.465 s**. Stage percentages are not added.

## Selection and contract

An untimed diagnostic counted 1,989,043 len calls and 1,919,699 math.sqrt calls per warmed native run, alongside 2,144,216 calculate_force methods and other calls. The [histogram](callee-histogram.json) includes validated output; it is not timing evidence. Diagnostic edits were restored exactly and every preceding source hash verified before implementation. No diagnostic API/counter remains.

Len and sqrt now share a scalar evaluator with ordinary builtin dispatch. Its closed implementation calls only audited length/primitive math/argument checks; these do not allocate VM objects, collect, enter Python or alter VM frames. A shared `&Heap` alone does not prove no allocation because the heap uses interior mutability. Arity/type/domain errors and scalar values remain shared with the original path. Rust error-string allocation cannot trigger VM GC. Results are only Int/Float.

Typed LIR records a `ScalarCall` probe contract only for warmed monomorphic direct Native targets with one positional argument. Profiles retain stable HIR identity/revision and native names, never heap addresses. Full conservative Call effects, pre-call state and root maps remain explicit because fallback can call arbitrary Python and return references.

## Native gateway and safety

Quiet native code materializes just probe operands. The small synchronous C gateway validates the actual callable's current Native descriptor name, not a binding name or cached object identity. Thus rebinding, host descriptor mutation, different heaps and method wrappers cannot bypass fallback. The gateway returns hit/miss/error separately; a miss has no Python effects and runs the original fully materialized rooted helper exactly once. Reopened/eager feedback uses that general path.

A successful probe cannot collect or reenter, so it stores its scalar result in the native image and reloads cached values without root synchronization. Errors/panics stay inside the C boundary; the read-only evaluator cannot abandon VM frames. Existing region bookkeeping is flushed, and probes are disabled in region/nest blocks. New private event9 uses the existing fixed Context/Slot lifetime contract; the 16-byte ABI and 26 copied-stencil relocation/tail checks remain unchanged. Every probe is honestly counted as a native callback.

## Measurement

Baseline `/private/tmp/tis-profile/bool-not-*`, candidate `leaf-calls-*`; original-shapes control `fields-baseline-worker`. At least ten validated samples per binary/backend, serial randomized interleaving. Fresh workers warm twice per sample; one-shot includes startup/parsing/compilation/execution/exit. Nbody and copy-patch Barnes use warmed reused workers. No builds/tests/profiles overlap formal timing. Outliers remain included. Every fresh native timed run has zero backend compilation/deoptimization. JSON contains hashes, raw samples, quartiles and warmups; source hashes/logs are archived.

| Fresh native comparison | Before (ms) | After (ms) | Time reduction |
| --- | ---: | ---: | ---: |
| Guarded scalar calls | 2579.860 | 2455.544 | 4.8% |
| Combined overnight work vs shapes | 4889.952 | 2464.642 | 49.6% |

| Warmed native Barnes counter | Before | After | Reduction |
| --- | ---: | ---: | ---: |
| Runtime callbacks | 32,164,624 | 32,164,624 | 0.0% |
| Root synchronizations | 14,297,231 | 10,388,891 | 27.3% |

The eligible guarded subset removes 3,908,340 root synchronizations. Callbacks and shared positional/leaf-dispatch counts remain unchanged. Compared with the original shape baseline, root synchronizations fall from 18,689,295 to 10,388,891 (44.4% fewer), and callbacks from 114,782,260 to 32,164,624 (72.0% fewer).

| Warmed control | Before (ms) | After (ms) | Time reduction |
| --- | ---: | ---: | ---: |
| Ticket, bytecode | 34.582 | 33.935 | 1.9% |
| Ticket, native | 54.292 | 52.722 | 2.9% |
| Ticket, copy-patch | 58.124 | 56.294 | 3.1% |
| Nbody (20k), bytecode | 244.518 | 244.384 | 0.1% |
| Nbody (20k), native | 8.453 | 8.412 | 0.5% |
| Nbody (20k), copy-patch | 164.544 | 164.902 | -0.2% |
| Barnes–Hut, copy-patch | 5149.880 | 5340.102 | -3.7% |

| One-shot control | Before (ms) | After (ms) | Time reduction |
| --- | ---: | ---: | ---: |
| Barnes–Hut, native | 2656.161 | 2508.089 | 5.6% |
| Ticket, bytecode | 43.066 | 42.613 | 1.1% |
| Ticket, native | 70.500 | 70.002 | 0.7% |
| Ticket, copy-patch | 64.955 | 64.784 | 0.3% |

Negative reduction means slower execution. Regressions/process-layout effects remain explicit; no claim is made that every backend improves. Earlier comparison ticket tradeoffs remain archived.

The reused-worker copy-and-patch Barnes control regresses 3.7%. A ten-pair fresh-worker repeat measures 5.131 → 5.219 s (+1.7% time), archived as `barnes-copy-fresh.json`. Separate untimed copy profiles show ScalarBuiltin::evaluate below 0.3% inclusive samples; that does not explain the whole regression. No speculative inline/layout tuning was applied. This measured tradeoff is retained for the native target gain and root reduction.

Two six-second native CPU profiles after all formal timing (all worker responses validated) show remaining exclusive shares: native entry 16.05%, Context::step 12.28%, root synchronization 5.19%, Heap::get 5.32%, callback 3.45% and scalar-call gateway 1.42%. Raw/parsed samples are archived; these percentages are contextual attribution.

## Validation and follow-up

Formatting, all-target Clippy with warnings denied, **245 debug tests**, **245 release tests**, release examples and diff whitespace checks pass. Five new tests cover quiet zero-root synchronization while preserving LIR effects, eager fallback, Native descriptor mutation with unchanged identity, error/prefix effects, keyword/star binding, malformed metadata, all-tier values/errors and a strong reentrant-GC miss: a direct native field value must stay alive after Python clears that field and collects, while returning a fresh heap result. Mixed numeric argument warmup avoids retiring an unrelated monomorphic float guard before the eager-path check.

See [CHECKPOINT.md](CHECKPOINT.md) for continuation state. General names/attributes, Python calls, tuple allocation and remaining root synchronization are unfinished. No commits/pushes. Stop implementation/timing at 07:00 UTC / 08:00 Europe/Berlin.
