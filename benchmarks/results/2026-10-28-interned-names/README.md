# Source literals and stable namespace slots

Source-string constants are interned for one VM lifetime. Bytecode and LIR sites cache non-owning identities and validate them against the current heap before reuse; a VM-owned pool roots canonical strings. Dynamic strings are not interned. Reusing code across VMs does not share heap objects.

Namespaces have a name index plus a stable vector of value slots. Replacing/removing bindings does not renumber slots. Bytecode and LIR name helpers cache the source environment, target namespace and slot, and read the current value on each access. Fixed lexical parent/local-name metadata belongs to the compiler. Paths through class scopes retain dynamic lookup. Cached identities do not root environments; active lexical ancestry and VM globals do.

The HIR's semantic names and profile identities are unchanged. No process-global symbol table, string reference counting or object shapes were introduced. The name index remains ordered for cold/class lookups; warmed slot accesses skip name comparison entirely.

## Ticket search

Baseline `c0697a0`; Apple M1 Pro, macOS 27, Rust 1.99 release defaults. Ten serial interleaved samples with two warmups for each worker. All outputs validated; feedback remains enabled. Binary hashes and raw samples are recorded in JSON. Builds/tests did not overlap timings.

| Benchmark/backend | Before | After | Time reduction |
| --- | ---: | ---: | ---: |
| Ticket search, bytecode | 37.753 ms | 34.047 ms | 9.8% |
| Ticket search, native | 60.543 ms | 57.621 ms | 4.8% |
| Ticket search, copy-patch | 61.871 ms | 57.835 ms | 6.5% |

A warmed bytecode iteration performs 211,717 cached name accesses and 8,032 cached source-literal loads, with zero newly interned literal objects. The pool retains source literals until the VM is dropped.

## Fresh process

Ten samples without warmup, including startup and compilation:

| Ticket-search backend | Before | After | Time reduction |
| --- | ---: | ---: | ---: |
| bytecode | 47.228 ms | 43.359 ms | 8.2% |
| native | 75.325 ms | 72.961 ms | 3.1% |
| copy-patch | 69.851 ms | 67.459 ms | 3.4% |

The raw samples retain scheduling outliers. Cold CPython measured 34.538 ms, including startup; it is not a warmed comparison.

## Other workloads

Ten paired warmed samples per backend, with validated outputs:

| Benchmark/backend | Before | After | Time reduction |
| --- | ---: | ---: | ---: |
| Barnes–Hut, bytecode | 3.395 s | 3.290 s | 3.1% |
| Barnes–Hut, native | 5.182 s | 5.184 s | -0.0% |
| Nbody (20k), bytecode | 242.254 ms | 243.353 ms | -0.5% |
| Nbody (20k), native | 8.821 ms | 8.729 ms | 1.0% |
| Nbody (20k), copy-patch | 161.063 ms | 161.090 ms | -0.0% |

Barnes–Hut bytecode improves about 3%; native Barnes–Hut and nbody are effectively flat. Small changes around 1% are treated as noise.

## Validation

All 206 debug and release tests passed; all-target Clippy with warnings denied passed. Tests cover shared code alternating between heaps and namespace layouts, GC retention, literal sharing across sites/backends, global rebinding, late definitions after lookup errors, nonlocal writes, class shadowing, dynamic strings and stable slots after removal/replacement.

Reproduce with `tools/compare_nbody.py` (generic `bench` workers for ticket/Barnes; `nbody_bench` for nbody) and `tools/bench_oneshot.py`, supplying the baseline/candidate executables recorded in JSON.
