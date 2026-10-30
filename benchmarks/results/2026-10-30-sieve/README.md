# Sieve of Eratosthenes

Original deterministic scalar-loop sieve through 1,000,000 inclusive.
Each invocation allocates a fresh boolean list, marks multiples starting at each prime's square,
and scans the result for count, sum and largest prime.
Expected stdout: `78498 37550402023 999983`.
Identical Python source runs on every runtime; no library sieve or slice-assignment shortcut is used.

All 389 Rust tests and all-target Clippy pass. The reduced cross-tier test checks 0, 1, 2, 49 and 100.
Full-size CPython output matches the fixture. Both measured tis tiers validate complete output on every sample.

Three serial, interleaved timing samples per runtime. These are quick diagnostics, not stable speed rankings.
Warmed measurements retain a VM, compile definitions before timing, and run one preparatory call.
One-shot measurements launch a fresh process and include parsing, compilation, execution and exit.
OS caches are not flushed. Raw times, commands, runtime versions, hashes and stderr are retained in the JSON files.
PyPy has only one warmed preparatory call, so its warmup may be incomplete.

| Runtime | Warmed median ms | One-shot median ms |
| --- | ---: | ---: |
| bytecode | 382.90 | 380.39 |
| native | 497.70 | 505.13 |
| cpython | 49.85 | 77.66 |
| pypy | 16.77 | 45.05 |

Native is slower than bytecode on this workload; no optimization is claimed.
The runtime source is unchanged. Benchmark additions remain uncommitted.

Reproduce after building the examples:

```sh
python3 tools/bench_suite.py --worker target/release/examples/bench --benchmarks sieve \
  --samples 3 --warmups 1 --output /tmp/sieve-warmed
python3 tools/bench_oneshot.py --run target/release/examples/run --benchmarks sieve \
  --samples 3 --output /tmp/sieve-oneshot
```

Pass `--python /path/to/python3` and optional `--pypy /path/to/pypy3` to select reference executables.
