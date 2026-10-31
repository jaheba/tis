# General augmented item updates

Baseline: 60e48bc. Candidate introduces `CoreOp::InplaceItem(operator)` with
receiver, key, old value and RHS inputs. All augmented item assignments use it,
for every operator and arbitrary RHS expressions. Names, attributes and items
share the same in-place value service.

HIR still loads the old value before evaluating the RHS. The new operation then
performs the in-place operation and stores its result without loading again.
This preserves reentrant RHS mutations, custom indexing, __iadd__ and setter
behavior. The receiver, key, old value, RHS and computed result remain rooted
across callbacks. LIR effects stay conservative.

Inside that general operation, exact builtin dictionaries without protocol keys
have a fast path for string keys and checked scalar arithmetic. Exact list updates
also use the shared sequence setter. Both use the original loaded value, even if
RHS evaluation removed or replaced the entry. Builtin scalar paths cannot run
Python or collect, so they skip callback-root setup. Other values/operators,
custom keys and overflow use the shared runtime operations. There is no literal-RHS rewrite and no cache held across RHS calls.

This combines arithmetic and storage into one runtime operation, leaving the
initial lookup separate to preserve evaluation order. Native lowering already
inlined much of the arithmetic, so this does not inherently reduce native callbacks.
Profiled float-list updates retain typed float arithmetic and stores in LIR,
preserving the existing native regions. Complete single-probe get/add/store
fusion remains future work requiring proof that intervening RHS evaluation is safe.

## Measurement

Unchanged Game k-nucleotide with FASTA n=20,000. Release/default features on
macOS arm64. Native readonly/mutation region flags enabled. Flying Circus
`--quick -x1`, three samples per mode, one repeated warmup and one RSS sample.
Compilation/execution/output included; startup and fixture setup excluded.
Repeated requests use fresh VMs. Exact output checked against CPython.

The register backend has a direct item-update instruction and uses adaptive
numeric plans over the old-value/RHS inputs. Type misses update those operand
positions and return to the shared protocol operation. Callback-free builtin
stores skip the GC checkpoint, matching the existing scalar-store policy.

| Runtime | One-shot ms | Repeated ms | Peak RSS MiB |
| --- | ---: | ---: | ---: |
| tis bytecode | 231 | 226 | 23.3 |
| tis native | 322 | 320 | 29.3 |
| CPython 3.15 | 79.1 | 77.3 | 33.7 |
| PyPy 7.3.16 | 62.3 | 54.8 | 83.7 |

Five-sample alternating CLI comparison includes startup and fixture setup:
bytecode 237 → 239 ms (roughly unchanged), native 346 → 333 ms (about 4% less).
A separate three-sample comparison gives 229 → 229 ms and 347 → 332 ms.
Sampling is too short for statistical significance claims. Native remains slower
than bytecode and the Python baselines.

Native runtime calls remain 2,662,469 and root syncs 2,621,447. The prior lowering
already specialized numeric addition, so combining arithmetic with storage does
not remove a native callback here. Neither run records compilation errors or
deoptimizations. Larger gains require reusing the initial lookup or lowering the
mapping update directly to native operations.

## Controls and validation

Three-sample CLI controls, before → after in milliseconds:

| Workload | Bytecode | Native |
| --- | ---: | ---: |
| sieve | 170.9 → 174.2 | 50.4 → 49.1 |
| game_nbody | 283.5 → 285.4 | 38.7 → 39.1 |
| barnes_hut | 2915.6 → 2988.0 | 678.3 → 677.6 |
| perf_unpack_sequence | 175.6 → 177.3 | 220.5 → 224.2 |

A five-sample alternating bytecode n-body follow-up gives 287 → 289 ms, within 1%.
All control outputs match before/after. Variations in the short control run are
below 3%; this cannot establish absence of small regressions.

Formatting, all-target Clippy with `-D warnings`, and all 420 tests pass. The
12-test object protocol suite was rechecked on the final sources. Both new Python
regressions also pass in CPython. Coverage includes every augmented operator,
arbitrary RHS calls, key/receiver evaluation order, RHS entry replacement/removal,
custom get/iadd/set hooks, list aliases, overflow, hot int/custom/float transitions,
and a float-store guard failure that must not repeat RHS effects. All four tiers
and both bytecode encodings are exercised; all 22 native region tests pass.

Raw samples, frozen release binaries, hashes, standalone workload, adapter,
source snapshots and validation logs are archived outside Git at
`~/Library/Caches/tis-benchmarks/2026-10-30-general-augmented.tar.gz`.
`followup.py` reproduces the alternating comparison, `compare.py` the controls,
and `run_quick.py` the Flying Circus run. Build with
`cargo build --release --locked --bin tis --example circus`, default features.
