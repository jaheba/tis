# Inline strings

Adapted the representation idea from [Monty PR #994](https://github.com/pydantic/monty/pull/994).
Strings up to 11 UTF-8 bytes fit inside the existing 16-byte Value, with no interpreter heap object.
The explicit four-byte tag and native field ABI remain unchanged; Monty's niche-based layout fits 15 bytes instead.
Option<Value> and native Slot also remain 16 bytes.

Heap::string and Heap::string_owned select inline or heap storage; Heap::as_str reads either representation.
Raw Heap::allocate(Object::String(...)) still permits explicit heap strings, including short ones.
Literal loading and character extraction avoid temporary String allocations for inline results.
Other operations may still allocate temporary Rust strings while constructing their result.
Inline values need no GC roots, and native boxing/unboxing preserves all twelve payload bytes.
Native field decoders recognize the new variant; invalid native payload lengths/UTF-8 return errors.

Equal inline strings share identity under `is`, including independently created strings, as in Monty #994.
Long strings retain heap identity. Capacity counts UTF-8 bytes, not Python characters.

## Quick bytecode timings

Three interleaved fresh-process samples per binary, full unchanged Flying Circus workloads.
Timings include process startup, source compilation and execution, without warmup.
No builds or tests overlapped this final timing run; all before/after output matched.
Before is the installed CLI with the previous linear string iterator and dedicated iteration bytecodes.
After adds inline strings, using the same Cargo.lock.
These are standalone quick samples, not a complete new Flying Circus --quick run.

| Workload | Before, ms | After, ms | Speedup |
| --- | ---: | ---: | ---: |
| rustpython_micro_strings | 10.199 | 9.127 | 1.117× |
| game_nbody | 250.322 | 246.069 | 1.017× |
| game_spectral_norm | 291.731 | 302.524 | 0.964× |
| perf_float | 180.038 | 182.192 | 0.988× |
| perf_unpack_sequence | 197.938 | 201.179 | 0.984× |
| rustpython_pystone | 465.883 | 465.958 | 1.000× |
| perf_barnes_hut | 2466.699 | 2511.919 | 0.982× |
| text_line_logs_medium | 28.401 | 28.456 | 0.998× |
| perf_json_loads | 5.970 | 6.312 | 0.946× |
| api_report | 4.684 | 4.897 | 0.957× |
| memory_string_index_short | 142.584 | 128.102 | 1.113× |
| memory_string_index_long | 157.685 | 155.360 | 1.015× |

String iteration and short-string indexing took about 10% less time.
Other timings varied within approximately 6%; three samples do not establish stable small regressions or gains.
This is primarily an allocation improvement, not a general interpreter speedup.
An initial implementation's duplicate heap lookups in len/indexing were removed before this final run.

A separate fresh-process sample retained 50,000 one-character strings with:

```python
chars = list('a' * 50000)
print(len(chars))
```

macOS `/usr/bin/time -l` measured peak whole-process RSS:

- Before: 16318464 bytes (15.56 MiB).
- After: 4358144 bytes (4.16 MiB).

RSS is one sample per binary and includes the CLI and runtime, not just Python payloads.

## Validation

Formatting, all-target Clippy and all 332 tests passed.
All 54 previously supported Flying Circus programs matched their manifest stdout in bytecode and native modes:
108 successful checks, with both native region flags enabled.
Tests cover byte/Unicode boundaries, empty/NUL strings, zero short-string heap allocations, heap/inline equality,
keys, indexing, slicing, iteration, builtins, methods, JSON, native fields, GC and native payload validation.
Existing result assertions now inspect string contents independently of storage representation.

Raw samples, binary hashes, source hashes, compatibility results and reproduction scripts are retained here.
The measured binary paths in results.json refer to the binaries used during this run; installation happens afterward.
