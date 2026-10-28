# Immediate builtin methods without bound-object allocation

Register bytecode recognizes single-use attribute results consumed as positional callees in the same block. For builtin lists, dicts, sets and strings, lookup resolves the receiver and static method name at the original attribute instruction. The receiver remains rooted in that register while arguments execute; a frame-local pending binding supplies the method at the call. Nested calls and receiver-name rebinding preserve lookup order.

Escaping methods, user-defined methods, keyword/star calls and cross-block callees keep normal method objects. Pending bindings materialize before block boundaries or active feedback collection. Other execution tiers retain ordinary objects; HIR semantics and JIT emitters are unchanged. Keyed sorting retains its existing callback/rooting path.

## Measurements

Baseline `2fbe2ce`; Apple M1 Pro, macOS 27, Rust 1.99 release defaults. Ten serial interleaved samples per comparison, two warmups for warmed calls and none for fresh-process runs. Every output validated. Runtime feedback remains enabled.

| Ticket search, 1,600 records | Before | After | Time reduction |
| --- | ---: | ---: | ---: |
| Warmed function | 45.206 ms | 43.975 ms | 2.7% |
| Fresh process | 54.339 ms | 53.004 ms | 2.5% |

These are small improvements with overlapping sample distributions. Cold samples include a 336 ms candidate outlier, retained in the raw data. Fresh-process CPython was 33.577 ms; this includes startup, not just interpretation.

Each warmed ticket-search iteration completes 42,251 prepared method calls without allocating bound-method objects. The optimization does not remove result objects such as lists returned by string splitting.

Validation: all 198 debug tests pass. The release suite passed 197 tests before the final eager-feedback test was added; that test also passes in debug. All-target Clippy with warnings denied passes. New tests exercise temporary receiver lifetime across collection, nested method calls, receiver rebinding, escaping methods, lookup errors preceding argument side effects and eager-feedback fallback.

Reproduce with `tools/compare_nbody.py --benchmark ticket-search --tiers bytecode` and `tools/bench_oneshot.py --benchmarks ticket-search --tiers bytecode`, supplying frozen baseline/candidate binaries. JSON records binary hashes and all samples.

Barnes–Hut bytecode: 3.870 s baseline versus 3.881 s candidate median over ten paired samples (0.3% slower, effectively flat at this measurement scale). All outputs validated; see `barnes.json`.
