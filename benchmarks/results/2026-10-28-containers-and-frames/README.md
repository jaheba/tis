# Containers and frame setup: Monty-inspired runtime improvements

Implementation: `522bdd3`. Baseline: `ef22a35`. Apple M1 Pro, macOS 27, ARM64 release,
Rust 1.99.0. Benchmark sources and deterministic inputs are unchanged.

## Warmed medians, ten serial interleaved samples per pair

| Benchmark / tier | Before | After | Time reduction |
| --- | ---: | ---: | ---: |
| Ticket search bytecode | 90.732 ms | 53.335 ms | 41.2% |
| Ticket search native | 109.337 ms | 73.643 ms | 32.6% |
| Barnes–Hut bytecode | 4.821 s | 4.300 s | 10.8% |
| Barnes–Hut native | 6.405 s | 5.880 s | 8.2% |
| Nbody bytecode, 20k steps | 298.745 ms | 300.311 ms | -0.5% |
| Nbody native, 20k steps | 11.559 ms | 11.512 ms | 0.4% |

Nbody's small differences do not establish a material change. Its fresh-process medians are also
unchanged within normal process variation. Ticket search sees the largest benefit from the new
container paths. These results do not establish a matched performance ratio against Monty; the older
cached Monty numbers discussed in conversation use a different, unverified build and harness protocol.

## One-shot native medians, ten fresh processes per pair

| Benchmark | Before | After | Time reduction |
| --- | ---: | ---: | ---: |
| Ticket search | 127.169 ms | 88.885 ms | 30.1% |
| Barnes–Hut | 6.428 s | 5.896 s | 8.3% |
| Nbody, 20k steps | 45.092 ms | 45.164 ms | -0.2% |

One-shot timing includes startup, source parsing/compilation, the first execution and process exit,
with no warmup calls. OS file caches are not flushed. CPython 3.15.0 without its JIT is also measured
in that run: ticket search 32.558 ms, Barnes–Hut 657.050 ms, nbody 102.648 ms. Those end-to-end numbers
must not be compared directly with warmed function-only measurements. PyPy was not remeasured here.
The nbody candidate has a 348 ms one-shot outlier, retained alongside its 45 ms median.

Warm workers have two preparatory calls. Incremental compilation can continue in timed samples:
maximum native compilation is 0.711 ms baseline / 0.776 ms candidate for ticket search and 6.861 ms /
7.256 ms for Barnes–Hut. These costs remain included. Full raw samples, min/max, quartiles, validation
responses, commands, binary/source hashes and runtime metadata are retained in JSON.
No builds, tests or profiling overlap timed runs. All stdout/result validation succeeds.
No CPU profile was collected for this pass; elapsed-time changes cannot be precisely attributed to
individual changes within this group.

## Adapted runtime patterns

- Dictionary entries remain in insertion order, with a randomized hash-to-entry index and collision
  chains. Reads and updates no longer scan all entries. String reads borrow existing storage instead
  of allocating a temporary owned key. Keys are stored once; the index holds hashes and indices.
  Deletion shifts entries and rebuilds the index, so it remains linear rather than a deletion fast path.
- Hashing agrees with existing key equality across bool/int/float, signed zeros, exact i64 boundaries
  and nested tuples. Integer values above the exact f64 range remain distinct when Python numeric
  comparison says they differ. NaN keys remain unsupported by the language, as before.
- Routine list/dictionary methods operate on the receiver instead of cloning it. Explicit copy
  operations and snapshots required by self-extension/update retain their ownership behavior.
  Compact float-list mutation avoids unnecessary conversion into boxed value vectors.
- Math function objects are cached in the GC-rooted module cache. Repeated `math.sqrt` lookup now
  preserves callable identity and avoids creating a fresh native-function heap object each time.
- Empty frame windows are initialized directly in the VM stack, removing a temporary vector and copy.
  Parameter-to-local-slot indices are computed once in bytecode metadata rather than by comparing
  names on every call. These improvements serve all four execution tiers.

The public Object::Dict payload changes from Vec to Dict; callers can use Dict::default() or .into().
The ordered entry slice is read-only so keys cannot be mutated behind the index. Copied Monty arena
and free-list internals are unchanged, and this pass introduces no unsafe code or new dependencies.
Dedicated method-call dispatch and interned names are still future work; no object shapes were added.

## Validation and reproduction

192 tests pass in debug and release; all-target Clippy with warnings denied is clean. New cross-tier
cases cover numeric/tuple keys, growth, deletion/reinsertion, ordering, dictionary copies, aliases,
self-extension, float-list widening, stable math callable identity and collection. Existing frame,
recursion, closure, default-argument, guard-exit and GC tests exercise the frame-entry changes.

```sh
cargo build --release --examples
python3 tools/compare_nbody.py --benchmark ticket-search --baseline /path/to/old-bench \
  --candidate target/release/examples/bench --tiers native bytecode --output /tmp/ticket-containers
python3 tools/compare_nbody.py --benchmark barnes-hut --baseline /path/to/old-bench \
  --candidate target/release/examples/bench --tiers native bytecode --output /tmp/barnes-containers
python3 tools/bench_oneshot.py --run target/release/examples/run --baseline-run /path/to/old-run \
  --python /path/to/python3 --benchmarks nbody barnes-hut ticket-search --tiers native --output /tmp/container-cold
```
