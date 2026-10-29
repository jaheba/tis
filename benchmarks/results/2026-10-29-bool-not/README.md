# Typed unary-not lowering

Validated 29 October 2026. Freshly warmed native Barnes–Hut improves **3.7%**, **2.681 → 2.581 s**, beyond literal comparisons. One-shot improves **3.6%**, **2.737 → 2.638 s**. A separate direct cumulative control against the original shared-frame/shape baseline measures **45.2%** less time, **4.730 → 2.593 s**. Stage percentages are not added.

## Change and safety

HIR Unary(not) now lowers to existing Truth(raw Bool), typed BoolNot(raw Bool), and BoxBool. Native Truth already inlines None/Bool/Int/Float tags, including both floating zeros and NaN truth; heap values retain the existing read-only truth helper. Inversion is pure and its verifier requires a Bool input. No speculative Python type assumption is needed.

Truth retains the original pre-operation frame state, read-only effects, empty root map and error point. It does not collect or call Python under the VM's existing truth semantics. Cached float views end at that callback boundary. Scalars remain in the native image for reconstruction; BoxBool produces the original HIR result and records eager observation exactly once. Heap/error/OSR paths stay available.

The copied backend adds one offline bool_not stencil using the existing fixed slot/patch pointer, bounds and synchronous lifetime contract. Its output is canonical raw Bool. Stencil symbol count changes from 25 to 26; strict relocation and tail-continuation checks remain enabled. Dedicated Rust 2024 stencil formatting and normal formatting were run. The 16-byte ABI, HIR behavior, copied storage and benchmark algorithms are unchanged. No commit/push; prior uncommitted work preserved.

## Measurement

Baseline frozen binaries `/private/tmp/tis-profile/number-compare-*`, candidate `bool-not-*`; original-shapes control `fields-baseline-worker`. At least ten validated samples per binary/backend, serial randomized interleaving. Fresh workers warm twice per sample; one-shot includes startup/parsing/compilation/execution/exit. Nbody and copy-patch Barnes use warmed reused workers. No builds/tests/profiles overlap formal timings. Outliers remain included. All twenty timed runs per fresh native comparison have zero backend compilation/deoptimization. JSON retains hashes, platform, raw samples, quartiles and warmups; source hashes/logs are archived.

| Fresh native comparison | Before (ms) | After (ms) | Time reduction |
| --- | ---: | ---: | ---: |
| Typed unary-not | 2680.567 | 2580.891 | 3.7% |
| Combined overnight work vs shapes | 4729.648 | 2593.110 | 45.2% |

| Warmed native Barnes counter | Before | After | Reduction |
| --- | ---: | ---: | ---: |
| Runtime callbacks | 34,472,986 | 32,164,624 | 6.7% |
| Root synchronizations | 14,297,231 | 14,297,231 | 0.0% |

All 2,308,362 former unary-not callbacks are removed. Heap truth fallback remains available. Root synchronizations are unchanged by this read-only optimization.

| Warmed control | Before (ms) | After (ms) | Time reduction |
| --- | ---: | ---: | ---: |
| Ticket, bytecode | 33.989 | 34.372 | -1.1% |
| Ticket, native | 53.735 | 54.003 | -0.5% |
| Ticket, copy-patch | 58.965 | 59.373 | -0.7% |
| Nbody (20k), bytecode | 243.619 | 244.218 | -0.2% |
| Nbody (20k), native | 8.470 | 8.542 | -0.9% |
| Nbody (20k), copy-patch | 164.270 | 164.739 | -0.3% |
| Barnes–Hut, copy-patch | 5355.916 | 5120.454 | 4.4% |

| One-shot control | Before (ms) | After (ms) | Time reduction |
| --- | ---: | ---: | ---: |
| Barnes–Hut, native | 2736.593 | 2638.380 | 3.6% |
| Ticket, bytecode | 44.032 | 42.810 | 2.8% |
| Ticket, native | 71.133 | 71.614 | -0.7% |
| Ticket, copy-patch | 66.753 | 65.879 | 1.3% |

Negative reduction means slower execution. Small timing regressions/process-layout variation remain explicit; preceding literal-comparison ticket regressions remain documented in that checkpoint.

Two six-second CPU samples after all formal timing (all worker runs validated) show remaining exclusive shares: Context::step 15.65%, native entry 14.74%, root synchronization 6.48%, Heap::get 4.63%, callback 3.65% and lir_helper 2.99%. Raw/parsed samples and worker responses are archived; these percentages are contextual attribution.

## Validation and follow-up

Dedicated stencil rustfmt, `cargo fmt --all`, all-target Clippy with warnings denied, **240 debug tests**, **240 release tests**, release examples and diff whitespace checks pass. Four new all-tier tests cover scalar/heap truth categories, signed zero/NaN/infinity/subnormal values, exact eager observation, GC/OSR state, malformed BoolNot operand rejection and a Truth error from an internal Unbound sentinel after a preceding print. That error test warms actual LIR/native/copied entries and verifies no repeated/missing effects and empty frames. The initial invalid-handle test assumed entry succeeded; it was replaced by this explicit HIR truth-error case rather than relying on stale host handles.

See [CHECKPOINT.md](CHECKPOINT.md) for continuation state. General name/call/attribute callbacks, heap truth and remaining root synchronization are unfinished. Stop implementation/timing at 07:00 UTC / 08:00 Europe/Berlin.
