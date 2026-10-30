# Object protocols and exceptions

Base: `a6ce17c1bf4f1d9e58912800a7339f4db473808e`. Implementation remains uncommitted.
Source fingerprints and frozen release binary hashes are recorded in `source-sha256.json` and `provenance.json`.
Both tiers execute tis; no workload execution is delegated to Python.

## Implemented

Shared runtime gateways handle arithmetic/reflected/in-place slots, rich comparisons, truth/length/index,
iteration, callable instances, string conversion, descriptors, properties, static/class methods,
attribute hooks, C3 inheritance, super, constructors, hashing, mapping/set equality and deletion.
List slicing supports assignment/deletion and user index conversion.

Protected suites lower to shared-environment HIR functions with explicit normal/return/break/continue/exception
outcomes. This supports general try/except/else/finally, handler-name cleanup, subclasses, exception args/identity,
chaining and reraising, and synchronous context managers with suppression and cleanup.
Tracebacks preserve raise/unwind identity and links; Python frame objects and source-line inspection remain unsupported.

Python-capable LIR operations carry conservative effects and complete roots, including inlined truth tests.
Late descriptors/hooks and class mutation invalidate direct field/region shortcuts through a VM-wide guard.
VMs without those protocols retain guarded direct instance reads/writes; exact numeric comparisons do not use recursive
container comparison machinery. Custom-key dictionaries use rooted snapshots; builtin keys retain indexed lookups.
Cold exception metadata is boxed; Value stays 16 bytes, and Object remains bounded to 144 bytes on this platform.

Metaclasses, slots, finalizers, builtin-type subclassing except exceptions, general yield, async, except*,
most stdlib and arbitrary imports remain unsupported. Integers remain signed 64-bit.

## Validation

- `cargo fmt --all --check` and `git diff --check` pass in the integrated checkout.
- `cargo clippy --all-targets --offline --locked -- -D warnings` passes.
- Full `cargo test --offline --locked`: 387 tests pass.
- Eleven extracted Python assertion programs independently pass CPython 3.15 through uv; sources/results in validation/cpython.
- Object protocol tests exercise register/stack bytecode across bytecode, LIR, Cranelift and copy-and-patch tiers, including
  GC during callbacks and descriptor changes after native warmup. Exception tests exercise all four tiers.

## Performance check

Flying Circus runs unchanged full-size programs serially, two timing samples, one repeated warmup and one RSS sample.
One-shot and repeated requests include fresh-session compilation; repeated is not a retained warmed VM.
Startup is measured separately. Native enables readonly and mutation regions.
These short measurements are diagnostic and do not establish statistical significance.

`first-pass/` preserves the initial release measurements, including the substantial Barnes bytecode regression that
prompted the guarded field fast paths. `quick/` contains final measurements. Old custom-equality and fannkuch failures
remain visible; exit code 1 is expected for these baseline failures.

| Mode / workload | Bytecode before → after (ms) | After/before | Native before → after (ms) | After/before |
| --- | ---: | ---: | ---: | ---: |
| one-shot / game_nbody | 248.380 → 285.878 | 1.151 | 32.128 → 33.645 | 1.047 |
| one-shot / perf_barnes_hut | 2454.626 → 2889.178 | 1.177 | 597.232 → 675.518 | 1.131 |
| one-shot / perf_unpack_sequence | 183.889 → 185.614 | 1.009 | 230.161 → 225.206 | 0.978 |
| one-shot / rustpython_micro_list_count_custom_eq_100 | failed → 0.105 | new support | failed → 0.110 | new support |
| one-shot / game_fannkuch | failed → 1858.134 | new support | failed → 2609.126 | new support |
| repeated / game_nbody | 242.813 → 288.073 | 1.186 | 31.402 → 33.534 | 1.068 |
| repeated / perf_barnes_hut | 2400.611 → 4907.464 | 2.044 | 596.738 → 682.881 | 1.144 |
| repeated / perf_unpack_sequence | 184.308 → 177.206 | 0.961 | 223.225 → 222.586 | 0.997 |
| repeated / rustpython_micro_list_count_custom_eq_100 | failed → 0.099 | new support | failed → 0.074 | new support |
| repeated / game_fannkuch | failed → 1878.197 | new support | failed → 2652.806 | new support |

Unpacking is approximately unchanged. Barnes bytecode remains roughly 20% slower and native roughly 15% slower;
n-body also shows overhead. These regressions remain open work, rather than a performance win.
RSS values and individual samples are preserved in quick/one-shot/*.json.

## Barnes bytecode follow-up

A single repeated-request sample took 6.934 s while its pair took 2.881 s, inflating the two-sample median.
A serial follow-up uses three samples per configuration/mode, one repeated warmup and no RSS samples.

| Mode | Before ms | After ms | After/before |
| --- | ---: | ---: | ---: |
| one-shot | 2408.655 | 2881.256 | 1.196 |
| repeated | 2432.872 | 2922.090 | 1.201 |

Use this follow-up for the Barnes bytecode estimate; all original samples remain visible.
