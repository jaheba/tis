# CPU profiles, 2026-10-28

Runtime source: `9ca3888c958a5d9b0379b8dd1dcde9a2be225be7` (HEAD `9de910b` adds benchmark evidence only).
Optimized release build with `CARGO_PROFILE_RELEASE_DEBUG=2`, Rust 1.99.0, macOS 27.0 ARM64 / Apple M1 Pro.
No runtime code was changed for these profiles.

## Findings

The native tier spends most sampled execution in the Rust callback boundary.
The following exclusive percentages are disjoint; inclusive percentages contain callees and must not be added together.

| Native component | Exclusive samples | Range across passes |
| --- | ---: | ---: |
| Callback body, including inlined context handling | 38.7% | 38.7–38.7% |
| Root synchronization | 19.8% | 19.4–20.2% |
| Inactive detailed-feedback gates | 9.9% | 9.8–9.9% |
| Boxed slot conversions | 5.5% | 5.4–5.5% |
| Heap handle reads | 5.9% | 5.7–6.1% |
| Anonymous generated native frames | 5.5% | 5.4–5.6% |

Callback execution **including its callees** accounts for 89.6% (89.4–89.7%) of native samples.
Root synchronization publishes references before helpers; this is distinct from actual GC traversal.
The profile does not support blaming GC collection for this overhead.
Anonymous frames are consistent with generated code, but lack symbols and include any native work inlined there.
These are sampled stack locations, not a count of floating-point instructions.

| Register interpreter component | Exclusive samples | Inclusive samples |
| --- | ---: | ---: |
| Main execution function | 52.6% | 97.0% |
| Generic runtime operation dispatcher | 13.3% | 27.2% |
| Heap handle reads | 8.6% | 8.6% |
| Indexed heap reads | 4.1% | 8.5% |
| Block bookkeeping | 2.6% | 4.4% |
| GC collector | 1.6% | 3.8% |

The main interpreter function includes inlined numeric guards/arithmetic and register loads/stores.
Its 52.6% exclusive share is **not** a measurement of dispatch overhead alone.
Its two passes agree within one percentage point; generic operations occupy approximately 27% including callees.
The profile justifies further work on direct/fused common operations but does not yet isolate the interpreter's largest
individual opcode cost. Generic operation percentages include nested iterator and heap work.

## Next changes

1. Cache the native detailed-feedback gate at execution boundaries so frozen profiles avoid `observe_site_with` calls.
   Preserve eager mode and update the gate when observation windows reopen or guards fail.
2. Give LIR helpers precise effects and separate GC root maps from complete deoptimization state.
   Helpers currently use conservative effects, and root synchronization scans `FrameState.values` on every helper.
   Non-allocating/non-calling operations should avoid that scan when no collection checkpoint runs there.
   Allocating helpers and collection checkpoints must still publish all live references.
3. Reduce callback frequency with direct implementations of non-allocating truth, iterator-state and sequence operations.
   Use the existing guarded sequence reads and preserve explicit side exits and heap handle validation.

The first two changes target measured native boundary overhead.
Their potential gains are not additive predictions: compiled layout and relative costs will change after each step.

## Method and evidence

`tools/profile_nbody.py` starts a fresh worker per pass, completes two warmups, then attaches `/usr/bin/sample` at 1 ms
for ten seconds while repeatedly running the fixed 20,000-step nbody advance.
Capture order was bytecode, native, bytecode, native. There were no overlapping workers, tests or builds.
Every completed advance restores the same scene first and validates energy `-0.169089263` afterwards.
The small scene restore and energy check remain in the sampling window.
Every recorded response reported zero backend compilation and deoptimization.
Bytecode remained Ready and native remained Compiled, each after one bounded feedback window.
Each bytecode response executed 5,400,000 numeric fast paths.

The reports contain 15,828 bytecode samples and 16,170 native samples across the two passes.
The runner validated 63 bytecode advances and 49 native advances around the capture windows.
Sampling overhead raised median worker advance times to roughly 0.325s and 0.42s respectively;
use the preceding numeric-specialization benchmark for unprofiled timings, not these diagnostic runs.

Raw call trees are retained as `*.sample.txt.gz`.
`metadata.json` contains the worker hash, sampling commands, warmup readiness and validated responses.
The two `*-summary.json` files contain demangled exclusive and inclusive counts for each pass and their weighted aggregate.
The parser subtracts immediate child counts for exclusive samples, counts each ancestor once per weighted path for inclusive
samples, and verifies nonnegative exclusive counts and exact conservation of the total sample count.
A synthetic recursive call tree also verifies inclusive deduplication.
Rust names were demangled with the locally cached `rustc-demangle` 0.1.28 crate; no runtime code was instrumented.

Reproduce with the commands under "CPU sampling on macOS" in the project README.
The raw reports can be summarized without a demangler; function names will remain mangled.
