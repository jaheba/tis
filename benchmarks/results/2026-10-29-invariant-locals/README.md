# Invariant local inputs at native entry

Validated checkpoint, 29 October 2026. Freshly warmed native Barnes–Hut time improves **6.1%**, from **3.444 s to 3.232 s**, beyond the positional-helper checkpoint. All twenty timed runs have zero backend compilation and zero deoptimizations.

A separate direct cumulative comparison against the original shared-frame/shape baseline measures **31.5%** less native time, **4.715 s → 3.228 s**. Percentages from separate stages are not added.

## Profile and selection

An untimed diagnostic build counted native helper operations after two warmups: 7,789,970 LoadLocal callbacks, 6,732,216 name loads, 6,720,622 calls, 4,687,245 general attribute reads, 4,374,996 equality comparisons, 4,288,832 inplace additions and 2,308,362 unary-not operations per Barnes–Hut run. StoreLocal and CheckLocal counts were zero. The [raw histogram](../2026-10-29-numeric-calls/helper-detail.json) includes validated output and labels. Diagnostic instrumentation was restored exactly before implementation; no diagnostic API remains.

Indexed local loading was the largest helper category, so this checkpoint optimizes invariant frame inputs rather than adding numeric speculation.

## Change and invariants

[LIR analysis](../../../src/lir.rs) identifies LoadLocal results whose underlying uncaptured slot has no StoreLocal anywhere in the function. [Native entry](../../../src/native_runtime.rs) copies those boxed Values into their result registers only for a fresh block-zero invocation; OSR retains the original helper. Functions whose entry block is an OSR header are conservatively excluded.

[Generated local probes](../../../src/native/locals.rs) reuse the seeded value only when entry inputs are ready, feedback collection is inactive, and the value is not unbound. Missing locals and reopened observation windows execute the original operation at its original point. Unbound reads therefore keep the same exception and preceding effects. No pointers into the movable local buffer are retained. The original active local range remains a GC root; object mutations are observed through the same copied handles. Cached values are cleared with the native register image at return/error.

Uncaptured physical slots cannot be modified by callees. Explicit slot writes disable caching for that slot. SSA reassignment of a Python variable continues through its current SSA values; this optimization only changes the remaining indexed LoadLocal operations. Captured names retain environment handling. GC maps, helper effects, side-exit states and the 16-byte Value/native ABI remain unchanged. Copy-and-patch supplies an empty input descriptor and keeps its helper path. A native-only auxiliary slot holds the fresh-entry flag after HIR and region metadata, and is allocated only when inputs qualify. Context and TierStats retain their prior layouts; the final code has no local-hit counter. No benchmark algorithm, copied Monty storage or stencil relocation/tail-continuation code changes.

The first version cached single-load functions too. It showed small ticket-search regressions (+2.5% fresh warmed native, +1.8% cold native), so the retained version requires at least two invariant load sites to amortize descriptor-walk/guard setup. Single-load functions use compact helpers. An isolated ten-sample check found Barnes–Hut unchanged by this profitability rule (3.255 → 3.249 s); ticket effects were smaller. Preliminary all-load and profitability controls remain archived as separate JSON files.

## Measurement

Baseline: completed positional-helper binaries `/private/tmp/tis-profile/positional-final-*`. Candidate: `/private/tmp/tis-profile/invariant-minimal-*`. The cumulative control uses the original `/private/tmp/tis-profile/fields-baseline-worker`. JSON records executable hashes, every sample, warmups, quartiles and counters. `source-sha256.json` records Rust/build/fixture inputs. All prior uncommitted shared-frame, shape, field and call work is preserved; no commits/pushes.

All comparisons use at least ten samples per backend/binary, validated output and serial randomized interleaving. Fresh workers warm twice before each measured call; one-shot has no warmup and includes startup, parsing, compilation, execution and exit. No builds, tests or profiles overlap formal timing. Outliers remain included. Platform: macOS-27.0-arm64-arm-64bit-Mach-O, release builds.

### Native Barnes–Hut, fresh workers

| Comparison | Before | After | Time reduction |
| --- | ---: | ---: | ---: |
| Invariant local inputs | 3443.890 ms | 3232.263 ms | 6.1% |
| Combined overnight changes vs shapes | 4714.841 ms | 3228.394 ms | 31.5% |

### Regression checks

Ticket uses fresh workers; nbody and copy-patch Barnes use reused workers with two warmups.

| Workload/backend | Before | After | Time reduction |
| --- | ---: | ---: | ---: |
| Ticket search, bytecode | 33.293 ms | 34.363 ms | -3.2% |
| Ticket search, native | 54.012 ms | 54.792 ms | -1.4% |
| Ticket search, copy-patch | 57.414 ms | 57.419 ms | -0.0% |
| Nbody (20k), bytecode | 244.683 ms | 243.883 ms | 0.3% |
| Nbody (20k), native | 8.533 ms | 8.579 ms | -0.5% |
| Nbody (20k), copy-patch | 163.425 ms | 163.096 ms | 0.2% |
| Barnes–Hut, copy-patch | 5665.035 ms | 5753.703 ms | -1.6% |

Negative reduction denotes slower execution. Observed regressions and process/layout sensitivity are retained explicitly; the optimization targets native Barnes–Hut, and native execution remains slower than bytecode and CPython there.

### One-shot, no warmup

| Workload/backend | Before | After | Time reduction |
| --- | ---: | ---: | ---: |
| Barnes–Hut, native | 3472.665 ms | 3265.748 ms | 6.0% |
| Ticket search, bytecode | 44.729 ms | 43.800 ms | 2.1% |
| Ticket search, native | 70.159 ms | 71.106 ms | -1.4% |
| Ticket search, copy-patch | 65.929 ms | 65.441 ms | 0.7% |

### Freshly warmed native counters

| Counter | Before | After |
| --- | ---: | ---: |
| Runtime callbacks | 50,713,113 | 42,926,781 |
| Root synchronizations | 18,689,295 | 18,689,295 |

The callback reduction is 15.4%, or 7,786,332 fewer callbacks. Earlier instrumented controls directly counted the cached loads; the final version removes that shared statistics field. Required safepoint synchronization stays intact; only redundant local helpers/result publication are bypassed.

The shared Context/statistics fields were removed after preliminary copy-patch ticket regression controls. Final copy-patch ticket is flat (57.414 → 57.419 ms); copy-patch Barnes is 1.6% slower and bytecode ticket is 3.2% slower. These controls do not exercise cached native loads, so their cause remains unresolved; no claim of improvement on those backends is made. Earlier variants and raw controls remain archived. The final fresh native distribution is broad: baseline quartiles 3.432–3.823 s, candidate 3.224–3.468 s; medians and outliers are reported without trimming.

CPU profiles archived as `after-profile*` describe the earlier context-field variant, not the final binary. They identify remaining Context::step, native entry and root-publication costs; they are contextual evidence only.

## Validation and next work

`cargo fmt --all`, all-target Clippy with warnings denied, **227 debug tests** and **227 release tests** pass. Logs are included. [New integration tests](../../../tests/invariant_locals.rs) cover all Value payloads/NaN bits and reference release, explicit mutable slots and prior effects, recursive GC, SSA parameter reassignment, OSR and eager feedback. A native unit test proves an unbound invariant load still raises after a preceding print. Existing language/GC/deoptimization and all-tier suites pass.

The next work is in [CHECKPOINT.md](CHECKPOINT.md). Remaining helper counts point to mixed numeric inplace addition, equality and unary-not lowering. Also inspect root-publication overhead: primitive results need no tracing, but any reduction must preserve reference-result rooting before collection, observation and complete deoptimization state. Native scratch pooling is another candidate. These follow-ups are not implemented here.

Continue under the existing overnight heartbeat; stop implementation/benchmarking by 29 October 2026 at 08:00 Europe/Berlin (07:00 UTC).
