# Guarded native fields and block bookkeeping

Validated overnight checkpoint, 28–29 October 2026. Native Barnes–Hut improves **22.7%**, from **4.657 s to 3.599 s**, with freshly warmed workers. All twenty timed runs have zero backend compilation and zero deoptimizations. The reused-worker comparison and fresh-process execution are reported separately below.

Implementation: [native field emission](../../../src/native/fields.rs), [heap directory](../../../src/heap.rs), [native block emission](../../../src/native.rs), [LIR receiver feedback](../../../src/lir.rs), and [new field tests](../../../tests/native_fields.rs).

## Changes and safety

Cranelift lowers observed instance field sites to machine-code loads guarded by receiver kind, heap identity, object generation, immutable shape identity and slot bounds. LIR records the instance-receiver feedback hint. Sites without that evidence retain the compact runtime probe; later receiver-type changes remain supported by the fallback. Missing own fields, class members, inherited methods and active feedback windows preserve general attribute behavior.

A weak native field directory initializes only for artifacts that contain these probes. It costs 32 bytes per initialized heap slot while active, plus vector/descriptor overhead, and follows the heap-slot high-water mark. Mutation invalidates a row before exposing mutable access, including when a caller forgets the guard, then refreshes shape/data pointers on guard drop. Collection clears dead rows; generation guards reject reused slots. Generated code reloads the directory after callbacks and consumes every borrowed field pointer before the next callback. The directory does not retain Python objects. Copied Monty storage modules are unchanged.

Value discriminants and ObjectId's C layout make payload decoding explicit. Reads avoid uninitialized padding, preserve float bits, and retain the 16-byte Value/Slot ABI. Native fields remain boxed values; this checkpoint does not propagate numeric field types into scalar LIR.

Stable Cranelift artifacts publish roots at allocating/calling safepoints. Quiet ordinary blocks update predecessor bookkeeping directly. HIR-derived OSR headers retain loop callbacks, including forward-layout backedges; region exits flush deferred state. Reopened feedback windows use the observing path. Existing deoptimization states, root maps and evaluation order remain intact. Attribute writes still use Rust helpers.

## Measurement

Baseline: `4e28b84` plus the pre-existing uncommitted shared-frame and instance-shape work. Candidate preserves that work and adds this checkpoint. No commits or pushes were made. `source-sha256.json` records candidate Rust inputs; each raw comparison records executable hashes and paths. Platform: macOS-27.0-arm64-arm-64bit-Mach-O, release builds.

Each comparison uses ten samples per backend/binary, serial randomized interleaving and validated output. Reused workers warm twice; the additional fresh-worker comparison starts and warms a new process twice for every sample. No build, test or CPU profile overlaps formal timing. Scheduling outliers are retained.

### Freshly warmed native Barnes–Hut

| Protocol | Before | After | Time reduction |
| --- | ---: | ---: | ---: |
| Fresh worker, two warmups | 4.657 s | 3.599 s | 22.7% |

### Reused warmed workers

| Benchmark/backend | Before | After | Time reduction |
| --- | ---: | ---: | ---: |
| Barnes–Hut, bytecode | 2469.056 ms | 2505.557 ms | -1.5% |
| Barnes–Hut, native | 4647.916 ms | 3614.426 ms | 22.2% |
| Ticket search, bytecode | 32.249 ms | 33.654 ms | -4.4% |
| Ticket search, native | 55.929 ms | 54.683 ms | 2.2% |
| Ticket search, copy-patch | 56.898 ms | 57.309 ms | -0.7% |
| Nbody (20k), bytecode | 238.564 ms | 242.462 ms | -1.6% |
| Nbody (20k), native | 8.693 ms | 8.467 ms | 2.6% |
| Nbody (20k), copy-patch | 159.795 ms | 161.171 ms | -0.9% |

Negative reduction denotes slower execution. Small changes require caution; raw ranges and quartiles are retained. Reused Barnes–Hut workers have occasional delayed compilation, up to 7.2 ms in a timed sample. Those samples remain included; the fresh-worker control above has none.

Fresh-worker ticket control (two warmups per sample) checks process-layout sensitivity of the short workload:

| Benchmark/backend | Before | After | Time reduction |
| --- | ---: | ---: | ---: |
| Ticket search, bytecode | 33.423 ms | 33.319 ms | 0.3% |
| Ticket search, native | 57.278 ms | 54.801 ms | 4.3% |
| Ticket search, copy-patch | 56.480 ms | 57.497 ms | -1.8% |

The reused-worker bytecode slowdowns are retained as regressions in the table, even where the fresh-worker control differs. Native metadata/mutation-guard overhead on bytecode remains a follow-up to investigate.

Representative freshly warmed Barnes–Hut counters:

| Counter | Before | After |
| --- | ---: | ---: |
| Runtime callbacks | 114,782,260 | 50,713,113 |
| Root synchronizations | 18,689,295 | 18,689,295 |
| Inline field loads | 0 | 34,447,355 |
| Inline ordinary blocks | 0 | 29,621,792 |

Lazy root publication avoids redundant block-entry copies rather than eliminating required safepoint synchronizations. Native Barnes–Hut remains slower than bytecode.

### Fresh process, no warmup

These wall times include startup, parsing, JIT compilation, execution and exit. CPython controls are retained in JSON as a separate runtime.

| Benchmark/backend | Before | After | Time reduction |
| --- | ---: | ---: | ---: |
| Barnes–Hut, bytecode | 2478.220 ms | 2470.596 ms | 0.3% |
| Barnes–Hut, native | 4679.346 ms | 3678.811 ms | 21.4% |
| Ticket search, bytecode | 41.579 ms | 41.950 ms | -0.9% |
| Ticket search, native | 70.349 ms | 68.450 ms | 2.7% |
| Ticket search, copy-patch | 64.935 ms | 65.311 ms | -0.6% |

## Experiments and profiles

Isolated ten-sample experiments found 14.3% less time from the initial direct-field implementation, 3.0% from extending lazy root publication, and 7.7% from quiet block bookkeeping. They were separate runs and their percentages should not be added. A CheckLocal experiment executed zero warmed fast checks on Barnes–Hut; its code was removed.

The first all-site probe candidate regressed cold ticket search from 71.020 to 74.195 ms (+4.5%). Restricting probe generation to observed instance receivers avoids oversized probes and directory maintenance at builtin-only sites. The isolated cold check against that intermediate candidate is:

| Experiment | Before | After | Time reduction |
| --- | ---: | ---: | ---: |
| Receiver hint vs all-site probes, native | 75.000 ms | 69.824 ms | 6.9% |

Two six-second warmed native profiles before edits attributed 13.39% inclusive time to field probes. Profiles after direct fields, lazy roots and quiet blocks (before the receiver-hint refinement) no longer show field probes among the leading hotspots: Context::step is 16.72% exclusive, root synchronization 6.30%, native entry 7.43%. Percentages refer to their own sampled runs. Compressed call trees, validated worker outputs and summaries are included.

## Validation

`cargo fmt --all`, `cargo clippy --all-targets -- -D warnings`, all **221 debug tests** and all **221 release tests** pass. Archived logs are included. New coverage exercises every field payload type and NaN bits, vector/layout growth, heap-directory growth, collection, non-instance replacement, forgotten mutation guards, builtin-to-instance receiver changes, exact loop-backedge counts and eager observation. Existing tests cover recursive feedback reopening, GC through nested calls, side exits, method replacement and all execution tiers.

## Next work

The subsequent [positional-helper checkpoint](../2026-10-29-positional-helpers/README.md) implements the shared dispatcher lead below and records newer cumulative measurements. Use its checkpoint for the latest next steps.

The implementation at this checkpoint is complete. Continue from `CHECKPOINT.md` under the existing overnight heartbeat until the hard cutoff, 29 October 2026 at 08:00 Europe/Berlin (07:00 UTC).

The main remaining costs are generic helpers, about 18.7 million root synchronizations and native frame setup. First investigate sharing register bytecode's positional/leaf-call path in `Vm::lir_helper`; operands are already rooted there, while the current general Call operation constructs positional/keyword scratch. Then collect an exact untimed helper histogram before considering mixed Int/Float arithmetic, numeric field specialization, method-call fusion or inlining. Fusion must preserve attribute lookup order, intervening effects, deoptimization state and GC roots.

Reproduce with `tools/compare_nbody.py` (`--benchmark barnes-hut`, `--benchmark ticket-search`, or nbody workers; add `--fresh-processes` for the control) and `tools/bench_oneshot.py`. Frozen baseline/candidate executables and full parameters are recorded in the JSON files.
