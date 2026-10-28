# Ticket search bytecode

Ten serial, interleaved samples on Apple M1 Pro, macOS 27, Rust 1.99 release defaults. Baseline is the runtime from `522bdd3`; later baseline commits only added measurement tools and evidence. Both workers retain adaptive runtime feedback. Outputs were validated on every run.

| Ticket search, 1,600 records | Before | After | Time reduction |
| --- | ---: | ---: | ---: |
| Warmed function, two warmups | 57.952 ms | 45.859 ms | 20.9% |
| Fresh process, no warmup | 62.701 ms | 51.993 ms | 17.1% |

Fresh-process CPython 3.15 measured 32.441 ms, including startup. This is not a steady-state interpreter comparison. Raw samples include large scheduling outliers; the table reports medians and retains every observation.

Changes: visit GC edges without building child vectors, sweep without a garbage-ID vector, borrow canonical builtin names, lower positional calls to a register opcode, and avoid copying already-rooted register operands into transient roots. Builtin calls use direct dispatch; keyed sorting retains the general callback path. JIT emitters are unchanged.

The ticket workload makes 54,855 positional calls per iteration, of which 54,843 use direct builtin dispatch. Profile collection remains enabled. The initial two eight-second CPU samples identified collection, allocation/copying and generic runtime operations as substantial costs; compressed sample reports are included.

Validation: 195 debug and release tests pass; all-target Clippy with warnings denied passes. Added tests cover callee lookup order, nested collection, keyword/star calls, keyed sorting and invalid external GC roots. Copied stable-heap/free-list storage is unchanged.

Commands:

```sh
python3 tools/compare_nbody.py --baseline OLD_BENCH --candidate NEW_BENCH --benchmark ticket-search --tiers bytecode --output warm
python3 tools/bench_oneshot.py --run NEW_RUN --baseline-run OLD_RUN --python /opt/homebrew/bin/python3 --benchmarks ticket-search --tiers bytecode --output cold
```

Raw JSON records binary hashes, versions, counters and individual samples.

Barnes–Hut bytecode regression check: ten paired warmed samples improved from 4.287 s to 3.824 s median (10.8% less time); all outputs validated. See `barnes.json`.
