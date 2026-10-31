# All 120 Flying Circus workloads — 2026-10-31

Coverage increased from 96 to 120 unchanged public workloads. Every default full-size program matches CPython output.
No workload was simplified, filtered out, precomputed or delegated to Python.

| tis configuration | One-shot | Repeated fresh sessions |
| --- | ---: | ---: |
| Bytecode | 120 / 120 | 120 / 120 |
| Reference LIR | 120 / 120 | 120 / 120 |
| Cranelift | 120 / 120 | 120 / 120 |
| Copy-and-patch | 120 / 120 | 120 / 120 |

CPython 3.15.0 also passed all 120. The harness excludes its internal startup workload.

## Changes

- Frozensets, order-independent hashing and set algebra share dictionary key protocols.
- Real generator functions suspend stack/register frames; `yield from` retains generator return values.
  Generator state is traced by GC. Generator bodies execute in bytecode in every configuration and reject scalar/native inlining.
- List subclasses retain ordinary instance identity and overrides alongside builtin list storage.
- Lazy map/filter/count/islice, mutable byte buffers, starred collection literals, function attributes,
  range slicing/indexing and basic percent formatting fill language gaps.
- Rust library services implement MT19937 randomness, six codecs, text/binary regexes, finite Decimal arithmetic,
  basic dataclasses, live instance dictionary updates, graph deepcopy, dates and protocol 4/5 pickle.

These are partial library APIs, not Python feature parity. Remaining boundaries are listed in the project README.
Native configurations can call runtime services or fall back to bytecode; this result does not mean every operation is compiled.

## Validation and evidence

The full Flying Circus `--quick -x1` run used three samples per workload/mode, one repeated warmup, five startup samples
and one separate RSS sample. Both modes include compilation, execution and output delivery. Repeated workers reset to a
fresh VM session; these are not retained-JIT timings. Builds and tests did not overlap samples.

The sampled worker SHA256 was `48718b4e4d17f618ddf4bd81200861efc9dc128c7353975c52558f9a073821ed`.
It was built with `cargo build --release --example circus --bin tis`, default features, rustc 1.99.0,
on aarch64 macOS. The source includes uncommitted changes over `d0f1aab37a2dcd0a3ff2a085977a36d72d6dc997`.
Subsequent review fixed dataclass repr recursion, temporary comparison roots, sparse pickle memo indexing and generator
return-profile accounting. Final binaries and source hashes are recorded separately in the evidence archive and revalidated
against the full suite. Treat the archived quick timings as measurements of the sampled build; this report makes no speed claim.

`cargo fmt --all`, all-target Clippy with warnings denied, and all 485 Rust tests passed.

CPython interoperability checks compare seeded random words/bytes and all six codec outputs directly, and load pickle streams
in both directions. Regression tests cover both bytecode encodings, all tiers, generator frame retention, delegated returns,
GC during attribute callbacks, live dictionaries, shared/cyclic copies and malformed pickle operands.

Raw reports, samples, binaries, logs, source snapshots, the unchanged workload manifest and the worker adapter are outside Git:

`~/Library/Caches/tis-benchmarks/2026-10-31-all120/`

The manifest SHA256 is `1b5831c360d81cf5be02ba0039a6a7bcd69400484bde4938223f46ab8fd9868f`.
`full/report.html` contains the quick results, `all-tiers.json` records final correctness checks,
and `final-state.json` identifies the final source/binaries and validation commands.

## Reproduction

Build the actual Rust worker, then use the archived harness adapter without changing its workloads:

```sh
cargo build --release --example circus --bin tis
python3 benchmarks/results/2026-10-31-all120/reproduce.py \
  --binary target/release/examples/circus \
  --harness "$HOME/Library/Caches/tis-benchmarks/2026-10-31-all120/harness" \
  --harness-python "$HOME/.local/share/uv/tools/flying-circus/bin/python" \
  --cpython /opt/homebrew/bin/python3 \
  --output /tmp/tis-all120-reproduction
```

The output directory must be new. The adapter implements only tis detection and benchmark worker transport.
The Rust worker compiles and executes all Python workload code.
