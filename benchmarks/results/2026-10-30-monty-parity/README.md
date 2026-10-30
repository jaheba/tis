# Monty parity: fixture audit and bytes milestone

Full Monty feature parity is not complete. This milestone adds a reusable fixture audit and immutable bytes support. See `docs/monty-parity.md` for scope and remaining work.

The baseline includes the existing uncommitted compact-boolean work; this comparison isolates the parity changes. Binary hashes and source hashes are in provenance.json and changed-files.json. Release binaries are locally retained in the ignored binaries directory.

## Fixture audit

Reference: Monty 0f243e6a. All 711 fixtures remain visible; 206 host/async/refcount fixtures are outside this milestone, eight intentional CPython divergences and one Monty expected failure are separate. These are declared fixture expectations and assertions, not a host-CPython stdout comparison.

| Result | Before | After |
|---|---:|---:|
| engine_error | 4 | 4 |
| exception | 54 | 67 |
| exception_message_difference | 89 | 89 |
| intentional_divergence | 8 | 8 |
| missing_exception | 1 | 1 |
| outside_scope | 206 | 206 |
| outside_target | 1 | 1 |
| pass | 62 | 64 |
| traceback_type_difference | 43 | 42 |
| traceback_unverified | 90 | 96 |
| unsupported | 153 | 133 |

Bytecode and native have identical status counts before and after. Twenty formerly unsupported cases now reach later checks; most still fail on missing features, integer overflow or diagnostic differences. Exact new passes are bytes__decode_invalid_utf8.py and type__bytes_negative.py. Traceback-unverified cases are not exact passes. Fixture counts are not feature coverage percentages.

## Regression check

Two fresh-process samples per binary and tier, including compilation. Small workloads; timings are diagnostic and not stable performance claims. Outputs match across binaries and tiers.

| Workload | Bytecode before/after (ms) | Native before/after (ms) |
|---|---:|---:|
| sieve | 170.42 / 168.10 | 47.65 / 47.92 |
| game_nbody | 266.04 / 267.32 | 35.56 / 35.82 |
| barnes_hut | 2780.04 / 2814.68 | 2804.80 / 2802.58 |
| perf_unpack_sequence | 170.05 / 169.23 | 216.00 / 215.80 |

## Validation

cargo fmt --all; all-target Clippy with -D warnings; cargo test: 408 passed. Audit-runner unit tests: six passed. The focused bytes fixture also passes CPython 3.15. Bytes tests cover every tier and both interpreter encodings, collection, key identity and VM isolation.

## Reproduce

```sh
python3 tools/check_monty.py --monty /path/to/monty --tis /path/to/tis --tier bytecode --output /tmp/new-bytecode-audit
python3 tools/check_monty.py --monty /path/to/monty --tis /path/to/tis --tier native --output /tmp/new-native-audit
```

For timing reproduction, place matching release binaries at binaries/before and binaries/after, then run regressions.py. Original commands used staging paths recorded in JSON; archived copies of the workloads are provided.
