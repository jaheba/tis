# Matched Monty / tis comparison

Monty 1.4.0, clean committed source `0f243e6ae6df760f09040a17604e23d5264b65ba`.
Tis runtime `522bdd3`, measurement adapter `3b4758b`. CPython 3.15.0, JIT unavailable.
Apple M1 Pro, macOS 27, ARM64. Monty was built from `git archive HEAD` into an isolated temporary
source/target directory. Neither Monty checkout's local changes were used or modified.

## Repeated fresh application requests

Ten serial interleaved samples after two untimed requests per runtime/workload. Processes are reused,
but application sessions, script definitions and tis JIT artifacts are fresh for each request.
Timing includes full source parsing/compilation, execution and output delivery. It excludes worker startup,
fixture file loading, session configuration/reset and shutdown. These are not warmed function-call timings.

| Fixture | Monty | tis bytecode | tis native | CPython |
| --- | ---: | ---: | ---: | ---: |
| Nbody, 20k steps | 289.776 ms | 302.488 ms | 38.975 ms | 77.207 ms |
| Barnes–Hut, 200 particles / 100 steps | 4.145 s | 4.296 s | 5.866 s | 625.940 ms |
| Ticket search, 1,600 tickets | 22.672 ms | 57.727 ms | 83.662 ms | 6.635 ms |

Relative to Monty:

- Nbody: tis bytecode takes 1.04x the time; tis native is 7.43x faster.
- Barnes–Hut: tis bytecode takes 1.04x the time; tis native takes 1.42x.
- Ticket search: tis bytecode takes 2.55x the time; tis native takes 3.69x.

Thus the remaining Monty gap is concentrated in ticket search rather than a universally slower
bytecode VM. Native execution is beneficial for nbody, but currently adds overhead on the two
object-heavy workloads. This comparison alone does not identify the remaining ticket-search costs;
method/builtin dispatch and runtime-library paths are the next profiling targets.

## One-shot, fresh process with no warmup

Ten serial interleaved subprocess runs, including startup, file reading, source compilation,
execution, output capture and process exit. OS file caches are not flushed.

| Fixture | Monty | tis bytecode | tis native | CPython |
| --- | ---: | ---: | ---: | ---: |
| Nbody, 20k steps | 297.033 ms | 309.408 ms | 44.877 ms | 101.744 ms |
| Barnes–Hut | 4.187 s | 4.290 s | 5.871 s | 667.343 ms |
| Ticket search | 29.702 ms | 62.074 ms | 89.693 ms | 32.168 ms |

For one-shot ticket search, tis bytecode takes 2.09x Monty's time and native takes 3.02x.
CPython's process startup changes its relative ranking on this short workload.

## Method and caveats

All 240 measured executions validate identical fixture output: both nbody energies, the Barnes–Hut
energy and all ticket rankings/scores. Sources are byte-identical across runtimes, including the
previously documented Barnes output-format and ticket generator-to-list adaptations. Source/binary
hashes, every output, raw timing, warmup and quartile/min/max summary are retained in `comparison.json`.
No builds, tests or profiling overlap measurements. No samples or outliers are discarded.

The repeated harness uses Flying Circus's Monty and CPython worker classes. Tis has a small adapter
with the same fresh-session boundary. Tis preloads the fixture and sends a short run command;
Monty and CPython transport source per request. Monty uses its protobuf transport and streamed print
events; tis aggregates output into one JSON response. Request timing includes each engine's output
transport, so this is a practical application-request comparison, not a transport-free engine microbenchmark.

Monty's default release profile uses fat LTO and one codegen unit; tis uses its default Cargo release
profile. Both use Rust 1.99.0 on the same host. These product-default build configurations differ and
must not be interpreted as a controlled experiment isolating interpreter architecture. The generic
Flying Circus resolver's revision/build strings are version-only; the explicit committed-source provenance
and actual build command are recorded in `builds.json` and the build log.

The Monty binary is preserved in the Flying Circus cache as `tis-comparison-0f243e6a-20261028`.
This replaces the earlier unverified older-binary estimates with a verified reference.
The adapter passes Rust formatting, all-target Clippy, and the existing 192-test suite; reset behavior
was also smoke-tested against CPython output before timing. No interpreter implementation changed.

## Reproduction

Use Python from an environment with Flying Circus installed:

```sh
cargo build --release --examples
/path/to/flying-circus/python tools/compare_monty.py --monty /path/to/verified/monty \
  --monty-revision 0f243e6ae6df760f09040a17604e23d5264b65ba --tis-revision 522bdd3 \
  --tis-run target/release/examples/run --tis-worker target/release/examples/request_bench \
  --python /path/to/python3 --output /tmp/tis-monty-matched
```
