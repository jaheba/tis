# Numeric comparisons with exact integer literals

Validated 29 October 2026. Freshly warmed native Barnes–Hut improves **3.5%**, **2.765 → 2.669 s**, beyond numeric union addition. One-shot improves **4.0%**, **2.840 → 2.727 s**. A separate direct cumulative control against the original shared-frame/shape baseline measures **43.0%** less time, **4.708 → 2.685 s**. Stage percentages are not added.

## Change and safety

Barnes–Hut repeatedly compares total mass with zero and particle count with one. General comparison callbacks handled these numeric sites. Profiled LIR now emits `NumberCompare` when one operand is a known integer literal exactly representable in f64 and the other warmed operand profile contains only Int/Float. All six relations and both operand orders are supported; a literal on the left reverses the relation.

The runtime guard accepts exact Int/Float tags. Int values compare as signed i64 without conversion; Float values compare with the exact f64 literal. This preserves comparisons at 2**53 and i64 boundaries. The literal proof roundtrips through i128: an i64 cast would saturate and incorrectly accept i64::MAX after rounding. Inexact literals, arbitrary mixed-variable comparison, Bool, identity and container semantics retain the general path. A nonnumeric miss exits at the original pre-operation state before effects.

LIR verification checks operand type, exact-literal proof, relation, exit reason, snapshot origin and live boxed input. The operation is pure and returns a raw Bool followed by existing BoxBool. It cannot allocate or call Python after the tag guard, so root maps/safepoints are unnecessary on its fast path. Native code compares integer/float payloads and selects by actual tag; the reference LIR and copied gateway share pure semantics. Copy-and-patch uses its existing callback stencil; BoxBool handles eager observation exactly once. No new unsafe, ABI/stencil changes, copied storage changes or benchmark algorithm changes. Prior work is preserved; no commits/pushes.

## Measurement

Baseline frozen binaries `/private/tmp/tis-profile/numeric-add-*`, candidate `number-compare-*`; original-shapes control `fields-baseline-worker`. At least ten validated samples per binary/backend, serial randomized interleaving. Fresh workers warm twice per sample; one-shot includes startup/parsing/compilation/execution/exit. Nbody and copy-patch Barnes use warmed reused workers. No builds/tests/profiles overlap formal timing. Outliers remain included. All twenty timed runs per fresh native comparison have zero backend compilation/deoptimization. JSON includes hashes, platform, samples, quartiles and warmups; source hashes/logs are archived.

| Fresh native comparison | Before (ms) | After (ms) | Time reduction |
| --- | ---: | ---: | ---: |
| Exact literal comparisons | 2765.488 | 2669.285 | 3.5% |
| Combined overnight work vs shapes | 4708.428 | 2684.650 | 43.0% |

| Warmed native Barnes counter | Before | After | Reduction |
| --- | ---: | ---: | ---: |
| Runtime callbacks | 38,534,717 | 34,472,986 | 10.5% |
| Root synchronizations | 14,297,231 | 14,297,231 | 0.0% |

| Warmed control | Before (ms) | After (ms) | Time reduction |
| --- | ---: | ---: | ---: |
| Ticket, bytecode | 32.853 | 33.498 | -2.0% |
| Ticket, native | 51.687 | 53.568 | -3.6% |
| Ticket, copy-patch | 56.416 | 58.098 | -3.0% |
| Nbody (20k), bytecode | 245.738 | 244.981 | 0.3% |
| Nbody (20k), native | 8.555 | 8.576 | -0.2% |
| Nbody (20k), copy-patch | 166.164 | 165.722 | 0.3% |
| Barnes–Hut, copy-patch | 5201.764 | 5234.322 | -0.6% |

| One-shot control | Before (ms) | After (ms) | Time reduction |
| --- | ---: | ---: | ---: |
| Barnes–Hut, native | 2839.501 | 2726.654 | 4.0% |
| Ticket, bytecode | 43.353 | 42.687 | 1.5% |
| Ticket, native | 68.563 | 69.705 | -1.7% |
| Ticket, copy-patch | 64.724 | 65.835 | -1.7% |

Negative reduction means slower execution. Small timing regressions and process/layout variation remain explicit; no claim is made that every backend improves.

The first fresh ticket control regresses native by 3.6% and copy-patch by 3.0%. A second ten-pair fresh control with seed 20261029 measures native 52.167 → 53.205 ms (+2.0% time) and copy-patch 56.796 → 57.920 ms (+2.0%); bytecode improves 1.2% in that repeat. Cold ticket native/copy-patch regress 1.7% each. This tradeoff is retained explicitly for the native Barnes–Hut gain and callback reduction; the change is not an across-workload improvement. The repeated raw control is archived as `ticket-repeat.json`.

Two six-second CPU profiles after all formal timings (all worker responses validated) show remaining exclusive shares: Context::step 15.68%, native entry 14.94%, root synchronization 6.24%, Heap::get 4.49%, callback 3.70% and lir_helper 2.99%. Raw/parsed samples are archived; these shares are contextual attribution.

## Validation and follow-up

`cargo fmt --all`, all-target Clippy with warnings denied, **236 debug tests**, **236 release tests**, release examples and diff whitespace checks pass. Three new all-tier tests cover six relations/both orders, int64/2**53 precision, signed zero/NaN/infinity, inexact literals, malformed verifier rejection, Bool/heap fallback without replaying prefix effects, live references across GC, OSR and exactly-once eager observation. Existing semantics/GC/deoptimization suites pass.

See [CHECKPOINT.md](CHECKPOINT.md) for continuation state. Unary-not, heap truth, general name/call/attribute callbacks and remaining root synchronization are unfinished. Stop implementation/timing at 07:00 UTC / 08:00 Europe/Berlin.
