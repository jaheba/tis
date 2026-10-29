# JSON implementation experiment

Two codecs expose the same minimal operations:

- `import json`: `json.loads(text)` and `json.dumps(value)` call Rust helpers.
- `codec.py`: `json_loads(text)` and `json_dumps(value)` execute Python through tis HIR, bytecode, and the selected JIT tier. The harness prepends this source; no Python module loader is required.

Run the comparison with:

```sh
cargo run --release --example json_bench -- 1000
```

The harness runs both codecs in bytecode, LIR, native, and copy-and-patch modes. It warms each workload before timing, then reports elapsed wall time, a checksum, native entries, and compilation errors. Each iteration decodes and encodes one small object. These are exploratory end-to-end timings, including allocation and GC, not isolated parser throughput. Rust helpers remain opaque to the JIT; Python codec functions can be compiled and profiled normally. Copy-and-patch has separate entry counters, so the harness reports both native backend counters.

The shared subset supports null, booleans, signed 64-bit integers, finite floats, Unicode scalar strings, arrays, and objects with string keys. Encoders also accept tuples as arrays. Output is compact, preserves dictionary order, and leaves Unicode unescaped. Duplicate input keys keep the last value. Both implementations bound nesting and reject cycles, nonfinite numbers, nonstring keys, malformed syntax, and integer overflow. Float formatting can differ; compare decoded values rather than encoded bytes.

This is intentionally smaller than Python's standard `json` API: no keyword options, streams, hooks, custom encoders, arbitrary precision integers, or lone surrogates. The Python codec uses assertions for validation because tis does not support Python exception handling. Error types/messages and exact depth-limit accounting are not a compatibility target. `ord` and `chr` were added as ordinary VM builtins for the Python Unicode paths; `chr` accepts Unicode scalars only.

`tests/json.rs` checks cross-codec roundtrips in all four tiers, malformed inputs, cycles, shared containers, numeric limits, and Unicode escapes.

## Initial local result

One release run of 1,000 decode/encode iterations, after three warmup runs of the same size:

| Tier | Rust helper | Python codec |
| --- | ---: | ---: |
| Bytecode | 2.065 ms | 114.122 ms |
| LIR | 2.446 ms | 231.958 ms |
| Native | 2.240 ms | 151.556 ms |
| Copy-and-patch | 2.310 ms | 191.707 ms |

All checksums were 51,000 and all compilation-error counters were zero. The Python native and copy-and-patch runs each recorded 227,473 function entries across warmup and measurement. On this small workload, Python compilation does execute successfully but does not yet beat bytecode; Rust is substantially faster. These are single samples, not a statistical benchmark or a claim about larger documents.
