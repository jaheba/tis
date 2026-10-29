# Combined native entry frame scan

Validated 29 October 2026. Freshly warmed native Barnes–Hut improves **2.9%**, **2.449 → 2.378 s**, beyond guarded scalar calls. One-shot improves **3.5%**, **2.524 → 2.437 s**. A separate direct cumulative control against the original shared-frame/shape baseline measures **51.1%** less time, **4.876 → 2.386 s**. Stage percentages are not added.

## Change and invariants

Native entry was the largest sampled exclusive cost (16.05%). It copied initialized VM shadow values into the native register image, then scanned the same window again to build rooted indices. It now pushes those same indices during the copy loop, using the pooled rooted vector.

The root set and ascending index order remain identical, including scalar shadow entries. There is no VM allocation or collection between the former steps. Entry-local seeding, guarded scalar/OSR imports, initial collection and all register initialization remain intact. No unsafe, ABI, LIR/effect/state/root-map or layout changes. Copy-and-patch shares this bridge. Early setup errors release the local root-index vector without retaining references; ordinary success/error/panic cleanup remains covered by the existing pooled-scratch tests.

## Measurement

Baseline frozen binaries `/private/tmp/tis-profile/leaf-calls-*`, candidate `entry-scan-*`; original-shapes control `fields-baseline-worker`. At least ten validated samples per binary/backend, serial randomized interleaving. Fresh workers warm twice per sample; one-shot includes startup/parsing/compilation/execution/exit. Nbody and copy-patch Barnes use warmed reused workers. No builds/tests/profiles overlap formal timing. Outliers remain included. All fresh native timed runs have zero backend compilation/deoptimization. JSON contains hashes, platform, raw samples, quartiles and warmups; source hashes/logs are archived.

| Fresh native comparison | Before (ms) | After (ms) | Time reduction |
| --- | ---: | ---: | ---: |
| Combined entry scan | 2449.300 | 2378.375 | 2.9% |
| Combined overnight work vs shapes | 4876.455 | 2385.528 | 51.1% |

Callbacks remain 32,164,624 and root synchronizations 10,388,891 per warmed native run. This change removes redundant frame reads at entry rather than safepoints/callbacks.

| Warmed control | Before (ms) | After (ms) | Time reduction |
| --- | ---: | ---: | ---: |
| Ticket, bytecode | 33.374 | 33.419 | -0.1% |
| Ticket, native | 52.874 | 54.039 | -2.2% |
| Ticket, copy-patch | 56.388 | 54.987 | 2.5% |
| Nbody (20k), bytecode | 245.370 | 245.012 | 0.1% |
| Nbody (20k), native | 8.529 | 8.482 | 0.6% |
| Nbody (20k), copy-patch | 165.271 | 166.042 | -0.5% |
| Barnes–Hut, copy-patch | 5213.344 | 5202.523 | 0.2% |

| One-shot control | Before (ms) | After (ms) | Time reduction |
| --- | ---: | ---: | ---: |
| Barnes–Hut, native | 2524.276 | 2436.926 | 3.5% |
| Ticket, bytecode | 44.279 | 42.303 | 4.5% |
| Ticket, native | 69.807 | 68.365 | 2.1% |
| Ticket, copy-patch | 64.438 | 64.945 | -0.8% |

Negative reduction means slower execution. All measured regressions and previous checkpoint tradeoffs remain explicit.

The first fresh native ticket control is 2.2% slower. A second ten-pair fresh control with seed 20261029 measures 53.086 → 53.415 ms (+0.6% time); cold native ticket improves 2.1%. Both controls are archived without trimming.

Two six-second CPU samples after all formal timing (all worker runs validated) show remaining exclusive shares: native entry 13.20%, Context::step 12.77%, root synchronization 5.82%, Heap::get 5.56% and callback 3.32%. Raw/parsed samples are archived; these shares are contextual attribution.

## Validation and follow-up

`cargo fmt --all`, all-target Clippy with warnings denied, **245 debug tests**, **245 release tests**, release examples and diff whitespace checks pass. Existing GC/deoptimization, eager/OSR, shared native/copy execution and scratch success/error/panic tests exercise the unchanged entry protocol. No implementation-mirroring test was added for this small loop fusion.

See [CHECKPOINT.md](CHECKPOINT.md) for continuation state. Tuple-allocation root synchronization and general name/attribute dispatch remain unfinished. Do not reuse uninitialized/stale slots without explicit entry/OSR/auxiliary metadata proof. No commits/pushes. Stop implementation/timing at 07:00 UTC / 08:00 Europe/Berlin.
