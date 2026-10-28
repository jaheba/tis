# One-shot benchmark baseline

Runtime source: `a5bf5d0`. Apple M1 Pro, macOS 27, ARM64 release build, Rust 1.99.0.
CPython 3.15.0 (JIT unavailable), PyPy 7.3.16 / Python 3.10.14 (JIT enabled).

Each fixture executes exactly once in each fresh process, with **zero warmup calls**.
Wall time includes process startup, reading/parsing source, compilation, JIT work triggered during
that execution, output capture and process exit. This is a different measurement from warmed
function-call timing. OS file caches are not flushed; this is process-cold, not a cold boot.

## Ten-sample medians

| Fixture | tis bytecode | tis native | CPython | PyPy |
| --- | ---: | ---: | ---: | ---: |
| Nbody, 20,000 steps | 334.014 ms | 46.108 ms | 104.232 ms | 42.865 ms |
| Barnes–Hut, 200 particles / 100 steps | 8.904 s | 12.529 s | 0.690 s | 0.406 s |
| Ticket search, 1,600 tickets | 109.415 ms | 139.606 ms | 37.635 ms | 42.934 ms |

The one-shot nbody gap between native tis and PyPy is 1.08x. Native tis remains slower than
bytecode on the two object-heavy targets. CPython is faster than PyPy for one-shot ticket search,
where startup and tier warmup are substantial relative to the workload itself.
These observations should not be substituted for steady-state comparisons.

Variability is retained. Nbody native ranges from 44.722 to 242.149 ms; Barnes–Hut native ranges
from 11.519 to 17.721 s. The median is reported without discarding outliers. The JSON includes
all min/max values, quartiles, raw samples, stdout/stderr, return codes, commands, binary/source
hashes and runtime metadata.

All 120 processes passed exact output validation. Runs were serial and interleaved in a seeded
random order within each benchmark. No builds, tests or profiling ran concurrently.
Only measurement tooling/documentation changed; the frozen runtime binary matches the preceding
object-suite baseline source. The new runner passes Python bytecode compilation and the complete
live measurement exercised every runtime/workload pair ten times. Existing Rust validation remains
185 passing debug/release tests with clean Clippy from that runtime source.

## Reproduction

```sh
cargo build --release --examples
python3 tools/bench_oneshot.py --run target/release/examples/run \
  --python /path/to/python3 --pypy /path/to/pypy3 --output /tmp/tis-oneshot
```

Use `--benchmarks nbody`, `--benchmarks barnes-hut` or `--benchmarks ticket-search` to select one.
Use `--tiers lir copy-patch` to select the other execution tiers. PyPy is optional.
Warmed function-call results remain in the preceding native/object-suite reports.
