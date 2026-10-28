# Register bytecode, 2026-10-28

Source: `f95d1692c7169e7d0fafb9c2f0efb40edb7c297f`.

The register interpreter reduces nbody bytecode time by 50.4% (2.016× faster).
Compiled tiers are approximately unchanged; the register interpreter now slightly beats the Cranelift tier here.
This measures this one application, not general Python throughput.

| Tier | Stack median (s) | Register median (s) | Speedup |
| --- | ---: | ---: | ---: |
| bytecode | 0.736226 | 0.365251 | 2.016× |
| lir | 0.540545 | 0.542604 | 0.996× |
| native | 0.382163 | 0.382571 | 0.999× |
| copy-patch | 0.435290 | 0.428606 | 1.016× |

Both labels use the same release worker binary, with `--bytecode stack` and `--bytecode register` respectively.
The runner performs two warmups per worker, then ten serial, randomly interleaved samples per label and tier.
Each sample restores identical body coordinates and velocities, times only `advance` with 20,000 iterations,
and checks energy `-0.169089263` outside the timer. Every timed sample had zero backend compilation and deoptimization.
Platform: Apple M1 Pro, macOS 27.0 ARM64, Rust 1.99.0, default Cargo release profile. No tests or builds ran during timing.
Raw samples, worker hashes, readiness and quartiles are in `comparison.json`.

`stack-control.json` separately compares the frozen previous worker with the new worker in stack mode only.
Medians are 0.769751s and 0.736851s respectively; the shared refactoring did not regress the old baseline.
Do not attribute that smaller difference specifically to register encoding. The same-binary comparison is the primary result.

The previously recorded CPython 3.15 control median was 0.075896s, making the new interpreter approximately 4.8× slower.
That is a separate measurement from the sequence experiment, not a paired CPython comparison in this run.

## What changed

HIR lowers to fixed-size register instructions with direct HIR value operands. `advance` contains 130 such instructions,
compared with 664 stack instructions. Instruction payloads occupy 2,600 bytes and 3,232 bytes respectively; these sizes
exclude shared pools, edge transfers, vectors and metadata. Both encodings are currently retained in each code object,
so this is not a claim that total compiler memory decreased. The register interpreter also avoids operand transport,
reuses per-frame argument scratch, and checks collection at complete semantic operation boundaries.
The measured change bundles those execution differences; it does not isolate dispatch count alone.

Constants, runtime services, feedback identity and tier entry/exit state are shared. Parallel block arguments use scratch
storage to preserve swaps. Deoptimization and detached resumption map HIR points to the selected encoding.
The register encoding is now the default; stack encoding remains selectable for differential testing and measurement.

## Lessons from Monty

Monty already pushes exact sequence unpack results without a temporary vector, fuses iterator advance and branch in
`ForIter`, and amortizes periodic dispatch checks. Its stack architecture shows that efficient common operations and
bounded bookkeeping matter independently of register encoding. Tis should next examine fused iteration and remaining
runtime helper costs. The compiled tiers being close to or slower than register bytecode suggests callback/boxing/heap
work still limits these tiers; this comparison alone does not identify which cost dominates.

Validation: full debug and release suites passed (121 tests before one final feedback-equivalence test was added).
The final four-test register suite passes in both debug and release; formatting and all-target clippy pass.
Tests cover both encodings across all four tiers, parallel loop edges, closures and nested calls, mode switching,
core language behavior, eager feedback equality and guard-failure/error recovery.

Reproduce using one built worker:

```sh
python3 tools/compare_nbody.py \
  --baseline /absolute/path/to/nbody_bench \
  --candidate /absolute/path/to/nbody_bench \
  --baseline-bytecode stack --candidate-bytecode register \
  --output /tmp/tis-register-comparison
```
