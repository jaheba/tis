# N-body measurements, 2026-10-28

Copy-and-patch reduces backend compilation time, but currently makes n-body execution slower than bytecode. These results establish a baseline for improving the generated code and runtime interface.

Apple M1 Pro, macOS 27.0, AC power, low-power mode disabled. Rust 1.99.0, default Cargo release profile, CPython 3.15.0 with its JIT unavailable. Source revision `12e5234a97a0619797f03a807c3bc1a1f5d5af3e` adds benchmark tooling to core revision `a0a862a`.

## Full workload

Unchanged `game_nbody.py`, 20,000 steps. Times below are medians of 10 serial samples per runtime after two warmups. Runtime order is shuffled each round with seed 20261028. Every result is checked against the expected energy output.

| Runtime | Fresh process, seconds | Warm advance(), seconds | Warm interquartile range, seconds | Warm / bytecode |
| --- | ---: | ---: | ---: | ---: |
| tis bytecode | 2.270 | 2.271 | 2.261–2.274 | 1.00× |
| tis reference LIR | 2.550 | 2.553 | 2.551–2.556 | 1.12× |
| tis Cranelift | 2.618 | 2.642 | 2.636–2.647 | 1.16× |
| tis copy-and-patch | 2.760 | 2.785 | 2.784–2.787 | 1.23× |
| CPython | 0.102 | 0.076 | 0.075–0.076 | 0.033× |

Fresh-process timing includes startup, file loading, compilation and execution. Warm timing measures `advance(0.01, 20000)` in a reused VM after restoring initial positions and velocities. State restoration, energy validation and IPC are outside that timer. Profiling remains enabled with the default 1,000-entry/backedge thresholds. Optimized samples each enter the requested tier once, with no backend compilation or deoptimization during measurement.

CPython is about 22× faster than tis bytecode end-to-end and 30× faster in the warmed call. Copy-and-patch is about 23% slower than bytecode in the warmed call. These measurements cover one workload on one machine; they do not measure where execution time is spent.

## Backend compilation

The first n-body warmup recorded 4.103 ms of Cranelift backend compilation and 0.172 ms for copy-and-patch, about 24× less. These are single observations, excluding shared HIR-to-LIR lowering, not latency distributions. Saving about 4 ms cannot materially improve a roughly 2.7-second workload.

A separate floating arithmetic kernel, `(x + y) * y - x`, measures backend generation/linking without common HIR-to-LIR lowering. Each process discards three compilation warmups and reports a median of 20 compilations; five fresh processes are run serially. The table summarizes those five reported medians.

| Backend | Median compilation | Range of process medians | Code bytes | Patch-data bytes |
| --- | ---: | ---: | ---: | ---: |
| Cranelift | 221.084 µs | 212.875–444.291 µs | 948 | 0 |
| Copy-and-patch | 7.541 µs | 7.417–9.375 µs | 2,092 | 3,720 |

Copy-and-patch compiles this kernel about 29× faster, with a larger artifact. Median call time across the five processes is 1.467 µs for Cranelift and 1.470 µs for copy-and-patch. Each process times 10,000 calls; those timings include the VM call path and should not be interpreted as isolated arithmetic throughput.

The next useful experiment is profiling n-body to locate runtime/helper costs before changing either backend. Both native paths still use memory-backed registers and shared runtime helpers, so native code generation alone does not remove those costs.

## Reproduce

From the repository root, with a supported native target and Python installed:

```sh
cargo build --release --examples
python3 tools/bench_nbody.py \
  --run target/release/examples/run \
  --worker target/release/examples/nbody_bench \
  --kernel target/release/examples/backend_compare \
  --source tests/fixtures/game_nbody.py \
  --samples 10 --warmups 2 \
  --output /tmp/tis-nbody-results
```

Use a new output directory. The runner uses the Python interpreter invoking it. `raw.json` retains all samples, tier validation, versions, power state, hashes and the source revision; `summary.json` retains timing summaries. `game_nbody.py` and its license are saved alongside the measurements. Fresh-process warmups warm OS caches; these are not cold-cache measurements. Host scheduling and frequency changes are uncontrolled.
