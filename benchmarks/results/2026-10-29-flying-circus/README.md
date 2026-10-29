# Flying Circus compatibility

The unchanged local Flying Circus manifest contains 103 workloads. The initial
bytecode audit passed 47. With named imports from math/json, string splitlines,
split maxsplit, rjust and scientific f-string formatting, 54 pass in each of
bytecode, reference LIR, Cranelift and copy-and-patch. Full support remains unfinished.

`compatibility.json` records source, manifest and binary hashes, exact commands,
statuses, errors and output from failing requests. Successful stdout was checked
exactly against the manifest. All sources include their fixture setup; nothing is
rewritten, reduced or delegated to CPython. These are correctness checks, not
timing comparisons. The binary includes the checkout's existing uncommitted work.
The initial per-request timeout was 10 seconds; Barnes-Hut LIR passed when retried
with 60 seconds. All other workloads completed within the initial limit.

Reproduce with a fresh output filename:

```sh
cargo build --release --example run
python3 tools/check_flying_circus.py \
  --suite /Users/jaheba/p/flying-circus/src/flying_circus/workloads \
  --run target/release/examples/run \
  --tiers bytecode lir native copy-patch --timeout 60 \
  --output /tmp/tis-flying-circus.json
```

The checker exits unsuccessfully for any error, output mismatch, unexpected stderr
or timeout. It saves each completed result, accepts repeated `--benchmark NAME`
filters, and refuses to overwrite an existing report.

## Remaining work

First failures do not identify every missing feature in a workload. The next
groups require independent language/runtime changes with tests across all tiers:

- Lazy generator expressions and resumable `yield`/`yield from`: capacity planning,
  portfolio risk, unchanged ticket search, JSON parsing, retained-string workloads,
  nqueens and generator benchmarks. Eager list substitution would change semantics.
- Exception handling and raising: CSV/JSON-lines workloads and Richards.
- List slice assignment: fannkuch.
- Class protocols and decorators, including custom equality and properties:
  list-count custom-equality cases, complex-class and DeltaBlue workloads.
- Immutable frozenset semantics and hashing: five frozenset workloads.
- Python-compatible JSON encoding defaults/options: dumps output lengths and
  transform keyword arguments.
- Additional modules: random, re, bisect, itertools, gc, base64, decimal, copy,
  dataclasses, collections.abc, datetime and pickle. Some workloads additionally
  need bytes/bytearray, arbitrary-precision integers and context managers.

The six output mismatches are JSON dumps and the five custom-equality list-count
cases. The other 43 failures are compile/runtime errors; none are counted as support.

Validation: `cargo fmt --all`, `cargo clippy --all-targets -- -D warnings`, and
`cargo test`. Regression coverage for the added features is in
`tests/flying_circus.rs`.
