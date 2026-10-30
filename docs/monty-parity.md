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

## Integer milestone

Integers that fit in `i64` remain inline. Overflow and larger literals use immutable `BigInt` heap objects;
results return to inline storage when they fit.
JIT code retains its checked small-integer operations and resumes the generic operation on overflow.
Profiles distinguish heap integers from inline integers so large values cannot be mistaken for an unboxed `i64`.

Core arithmetic, shifts, integer bit operations, exact comparisons with floats, floor division and modulo,
integer hashing, JSON numbers, `int()` conversions, `bit_length` and `bit_count` support large values.
`divmod`, `pow` (including modular inverses), `hex`, `oct` and `bin` use the same integer service.
Integer true division rounds the exact quotient instead of converting each input to a float first.
Dictionary keys retain their original heap integer owner across collection.

Range iteration and native integer registers still use `i64`.
Integer byte conversions, Unicode decimal parsing, numeric formatting and some diagnostic messages remain incomplete.
Python’s decimal-string digit limit is not yet enforced.
There is no claim of full Python or Monty integer parity.

## Sorting

`sorted()` and `list.sort()` share a stable merge sort and accept keyword-only `key` and `reverse` arguments.
The key runs once per input value; detached items and callback results remain rooted through comparisons and GC.
`sorted()` consumes its iterable before validating sort keywords, and leaves the original container intact.
Reverse conversion follows the Python 3.14+ truth protocol and can invoke user code before an in-place list is detached.

String slicing uses byte offsets for contiguous ASCII results and streams Unicode code points for other slices.
It copies only the selected result, without building a character array for the full source.

## Remaining work

1. Complex numbers and the remaining integer methods.
2. Remaining bytes/string/container methods and formatting behavior.
3. Pattern matching and other syntax gaps identified by the audit.
4. Bundled modules, including collections, dataclasses, itertools, decimal and codecs.
5. Exact exception messages and traceback/source-location behavior.
6. Host and async APIs, if requested separately.

Keep expected failures visible and add focused cross-tier regression tests as capabilities are implemented.
