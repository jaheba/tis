# Monty compatibility

The compatibility target is Python behavior supported by Monty, while retaining tis’s HIR, bytecode and JIT architecture. Full parity is not implemented.

Run the fixture audit on a POSIX host with a release tis binary:

```sh
python3 tools/check_monty.py --monty /path/to/monty --tis /path/to/tis --tier bytecode --output /tmp/monty-bytecode
```

Repeat with `--tier native` and a fresh output directory. The report pins the Monty revision, tis binary hash and each fixture’s source hash. All fixtures remain visible, including unsupported and excluded cases.

The runner uses Monty’s declared expectations and fixture assertions. It never executes reference fixtures in host CPython: they include filesystem and resource-limit tests. It does not verify arbitrary stdout, and execution-only passes establish less than assertion or return-value passes. Exception messages, traceback formatting gaps and unsupported features are separate outcomes. A fixture count is not a feature-coverage percentage; a single missing operation can block a large fixture.

Children have time and output limits. On macOS the memory budget uses RSS polling every 10 ms, which is approximate and is not a sandbox. Other POSIX hosts use an address-space limit. Host callbacks, filesystem mounts, async execution and refcount expectations are excluded from this core-language milestone. Intentional Monty divergences from CPython are reported separately.

## Bytes milestone

Bytes literals and immutable heap objects work through HIR runtime operations in all execution tiers. Core support includes construction, indexing, slicing, iteration, concatenation, repetition, comparison, membership and dictionary/set keys. Key storage retains the original bytes object across collection. The value representation remains 16 bytes.

Common search, split, replacement, case conversion, hex, join and strip methods are implemented. String encoding and bytes decoding support UTF-8 and ASCII with strict, ignore and replace error handling. This is not the complete bytes API: other codecs, some method options, formatting and exact diagnostic compatibility remain open.

## Remaining work

1. Arbitrary-precision integers and complex numbers.
2. Remaining bytes/string/container methods and formatting behavior.
3. Pattern matching and other syntax gaps identified by the audit.
4. Bundled modules, including collections, dataclasses, itertools, decimal and codecs.
5. Exact exception messages and traceback/source-location behavior.
6. Host and async APIs, if requested separately.

Keep expected failures visible and add focused cross-tier regression tests as capabilities are implemented.
