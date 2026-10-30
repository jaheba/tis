# Bytecode iteration comparison

Monty's `StringIterator` (`crates/monty/src/types/str.rs`) stores a UTF-8 byte offset.
Its `py_next` decodes one character from the remaining suffix and advances by that character's byte length.
Tis now uses the same cursor approach instead of recounting and rescanning the string on every step.
The whole traversal is linear in the string's byte length, including non-ASCII input.
The existing iterator source reference keeps the string reachable during garbage collection.

Monty's `ForIter` also dispatches directly to iteration operations.
Tis now lowers HIR Iter/Next/IsDone operations to dedicated register bytecode instructions.
These avoid collecting generic runtime argument vectors while preserving HIR sites, feedback and tier handoff.
They are separate instructions; this change does not fuse a whole loop step into one dispatch.
Stack bytecode retains its generic dispatch and benefits from the shared string cursor fix.

## Quick results

Three interleaved fresh-process samples per binary, full unchanged Flying Circus workloads, bytecode tier.
All samples include process startup, compilation and execution, without warmup.
No tests or builds overlapped measurement; all before/after stdout matched exactly.
This is a small standalone comparison, not another complete Flying Circus `--quick` run.

| Workload | Before, ms | After, ms | Speedup |
| --- | ---: | ---: | ---: |
| rustpython_micro_strings | 250.305 | 15.272 | 16.390× |
| game_nbody | 252.010 | 244.893 | 1.029× |
| game_spectral_norm | 306.389 | 305.224 | 1.004× |
| perf_float | 184.533 | 181.096 | 1.019× |
| perf_unpack_sequence | 199.642 | 198.248 | 1.007× |
| rustpython_pystone | 471.300 | 466.627 | 1.010× |
| perf_barnes_hut | 2487.848 | 2483.151 | 1.002× |

The string traversal improvement is clear; 0–3% differences on other workloads are inconclusive with three samples.
This fixes one algorithmic outlier, not the broader gap to CPython.
Direct iteration dispatch alone did not produce a clear general improvement in this sample.

## Reproduction and validation

Before: core commit `e2e931d` with the new CLI, rebuilt with the same checked-in Cargo.lock as the after binary.
After: the same CLI plus the string cursor and dedicated iteration bytecode changes.
`results.json` records binary/source SHA-256 values and individual timing samples.
`measure.py` records the exact commands and retained workload source paths; binary paths are local to this run.

Formatting and all-target Clippy passed; the full test suite passed (330 tests).
New coverage checks Unicode, combining characters, partial iteration, iterator identity, repeated exhaustion,
zip/enumerate and garbage collection across all four execution tiers and both bytecode representations.

Further Monty ideas worth measuring are a fused loop-step instruction (avoiding separate Next/IsDone/Branch dispatch)
and cached ASCII character values (avoiding a fresh one-character string allocation for each yielded ASCII character).
Neither is implemented in this change.
