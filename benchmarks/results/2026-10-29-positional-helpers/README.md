# Shared positional helper dispatch

Validated checkpoint, 29 October 2026. Native Barnes–Hut time improves by **5.9%**, from **3.628 s to 3.416 s**, beyond the guarded-fields checkpoint. Fresh workers warm twice; all twenty timed runs have zero backend compilation and zero deoptimizations.

A separate direct comparison against the original shared-frame/shape baseline measures the combined overnight native improvement at **26.8%**, from **4.687 s to 3.430 s**. That is a new paired comparison, not a percentage added to the preceding result.

## Change

[`Vm::lir_helper`](../../../src/tier.rs) now routes calls whose argument names are all None through the existing `call_from_registers` dispatcher. LIR, Cranelift and copy-and-patch reuse the gathered argument slice. Borrowed native/bound builtin names avoid cloning the callable object and take the existing builtin fast route. Keywords, starred arguments and duplicate-keyword checks stay in the general binder; functions, defaults, classes, closures and Python bound methods keep their established invocation logic. Keyed sorting retains its general reentrant path.

Every operand is already rooted by the LIR/native gateway throughout helper execution. Call effects, safepoint publication, frame reconstruction, side exits, GC checkpoints and the 16-byte Value/native ABI are unchanged. There is no new unsafe code, machine-code call convention or inlining. Existing uncommitted shared-frame, shape, guarded-field and block-bookkeeping work is preserved. Benchmark algorithms and fixtures are unchanged.

The legacy `bytecode_positional_calls` and `bytecode_leaf_calls` counters now also include this shared LIR helper route. They describe dispatches and must not be interpreted as native-to-bytecode fallback counts. Zero-argument calls already avoided vector allocation; bound-receiver argument vectors, general binding and callee frame setup remain.

## Measurements

Baseline executables: `/private/tmp/tis-profile/fields-hinted-*`, the completed guarded-fields checkpoint. Candidate: `/private/tmp/tis-profile/positional-final-*`. The cumulative comparison uses `/private/tmp/tis-profile/fields-baseline-worker`, the original shared-frame/shape binary. JSON records hashes, every sample, warmups, quartiles, validation and counters; `source-sha256.json` records Rust/build/fixture inputs. No commits or pushes.

All reported comparisons have at least ten validated samples per binary/backend, serial randomized interleaving, and no overlapping builds, tests or profiles. Reused workers warm twice. Fresh-worker controls start a new worker and warm twice for each sample. One-shot times include startup, parsing, compilation, execution and exit. Outliers remain in the data. Platform: macOS-27.0-arm64-arm-64bit-Mach-O, release builds.

### Freshly warmed native Barnes–Hut

| Comparison | Before | After | Time reduction |
| --- | ---: | ---: | ---: |
| Positional helper change | 3627.919 ms | 3415.610 ms | 5.9% |
| Combined overnight changes vs shapes | 4687.136 ms | 3429.592 ms | 26.8% |

### Reused warmed workers

| Workload/backend | Before | After | Time reduction |
| --- | ---: | ---: | ---: |
| Barnes–Hut, native | 3672.211 ms | 3443.461 ms | 6.2% |
| Barnes–Hut, copy-patch | 5834.990 ms | 5697.054 ms | 2.4% |
| Ticket search, bytecode | 33.312 ms | 33.056 ms | 0.8% |
| Ticket search, lir | 67.380 ms | 66.596 ms | 1.2% |
| Ticket search, native | 53.656 ms | 52.667 ms | 1.8% |
| Ticket search, copy-patch | 57.970 ms | 54.998 ms | 5.1% |
| Nbody (20k), bytecode | 242.674 ms | 242.390 ms | 0.1% |
| Nbody (20k), native | 8.397 ms | 8.388 ms | 0.1% |
| Nbody (20k), copy-patch | 172.808 ms | 161.870 ms | 6.3% |

### Fresh-worker regression controls

| Workload/backend | Before | After | Time reduction |
| --- | ---: | ---: | ---: |
| Ticket search, bytecode | 33.150 ms | 34.153 ms | -3.0% |
| Ticket search, lir | 69.414 ms | 67.706 ms | 2.5% |
| Ticket search, native | 53.841 ms | 51.935 ms | 3.5% |
| Ticket search, copy-patch | 57.072 ms | 54.757 ms | 4.1% |
| Nbody (20k), copy-patch | 161.503 ms | 162.114 ms | -0.4% |

Nbody copy-patch's 6.3% reused-worker improvement does not survive the fresh-worker control (0.4% slower), so it is treated as flat rather than attributed to dispatch. Fresh-worker ticket bytecode is 3.0% slower while the reused worker is 0.8% faster; both results remain visible. Small changes and differences between reused/fresh worker results should be treated cautiously. The no-compilation fresh native control is the primary Barnes–Hut execution result.

### One-shot, no warmup

| Workload/backend | Before | After | Time reduction |
| --- | ---: | ---: | ---: |
| Barnes–Hut, native | 3703.445 ms | 3438.245 ms | 7.2% |
| Ticket search, bytecode | 43.956 ms | 43.316 ms | 1.5% |
| Ticket search, native | 70.487 ms | 68.141 ms | 3.3% |
| Ticket search, copy-patch | 65.950 ms | 63.701 ms | 3.4% |

### Freshly warmed native counters

| Counter | Before | After |
| --- | ---: | ---: |
| Runtime callbacks | 50,713,113 | 50,713,113 |
| Root synchronizations | 18,689,295 | 18,689,295 |
| Positional dispatches | 2,050 | 6,722,672 |
| Borrowed leaf dispatches | 1,147 | 3,944,835 |

This optimization makes helpers cheaper; it does not reduce callback count or weaken required root synchronization. Existing inline field and block fast paths remain active. About 6.72 million positional dispatches and 3.94 million leaf dispatches use the shared route in a run.

## Validation

`cargo fmt --all`, `cargo clippy --all-targets -- -D warnings`, **223 debug tests** and **223 release tests** pass. Logs are archived here. The two new [positional helper tests](../../../tests/positional_helpers.rs) exercise warmed leaf routing in all three LIR backends, defaults, bound methods, closures, recursive collection, keyed-sort callbacks, keyword/star binders and frame cleanup after a non-callable error. Existing guard/GC/deoptimization/language suites also pass. Source hashes confirm the frozen candidate.

## Remaining work

Two six-second CPU samples of the warmed candidate (with validated worker runs and no concurrent builds/tests/timings) identify remaining exclusive costs: Context::step 18.16%, native entry 8.50%, root synchronization 6.94%. Compressed raw profiles and a summary are included.

Native Barnes–Hut remains slower than bytecode and CPython. Around 50.7 million callbacks and 18.7 million root synchronizations remain, together with callee binding/frame setup. Before larger changes, capture an untimed helper-op histogram. Mixed Int/Float arithmetic can prevent monomorphic numeric lowering; guarded arithmetic with checked integer overflow and full fallback is a candidate. A borrowed-leaf gateway could reduce root synchronization only with a proven no-Python/no-GC contract, explicit handling of GC pressure and active region views, and preserved observation/error paths. These ideas are not implemented here.

The next action and cutoff rules are in [CHECKPOINT.md](CHECKPOINT.md). Continue authorized work through the existing heartbeat, stopping implementation and benchmarks by 29 October 2026 at 08:00 Europe/Berlin (07:00 UTC).
