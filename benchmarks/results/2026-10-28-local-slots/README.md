# Indexed function locals, 2026-10-28

Ordinary function locals now use indexed frame slots instead of string-keyed environment access.
This improves the n-body baseline and benefits all execution tiers.

## Representation and semantics

The frontend resolves local slots in HIR before bytecode generation.
HIR owns the slot-name layout and explicit `LoadLocal(index)`/`StoreLocal(index)` operations.
Bytecode emits dedicated local-access instructions.
LIR retains the same identities and currently uses helpers to access the slots.

Each active frame owns a separate vector of optional boxed values.
An empty slot raises `UnboundLocalError`; it does not fall back to a global binding.
The GC traces local vectors independently of temporary-register liveness.
LIR frame states record the local count, and deoptimization copies the current local values alongside live temporaries.
Detached side exits validate and restore both layouts.
Return, error and native panic recovery release abandoned local frames.

Any local name referenced by a nested function or class conservatively remains in its shared environment.
This analysis deliberately overcaptures shadowed names; it does not require precise free-variable resolution yet.
Class/module namespaces, global/nonlocal assignment and closure late binding retain their existing named semantics.
Argument binding still constructs initial named bindings before moving uncaptured parameters into local slots.
Local values remain boxed: this change is not SSA promotion or unboxed local storage.

Six new regression tests cover slot lowering, closure/class/comprehension scope, argument binding, unbound locals,
GC during nested execution, internal and detached deoptimization, and invalid local layouts.
The complete 92-test suite passes in debug and release builds.
After the final IR-display edit, affected debug tests and the full release suite were repeated successfully.
Formatting and Clippy with warnings denied pass.

## Warm n-body comparison

Apple M1 Pro, macOS 27.0, Rust 1.99.0, default Cargo release profile.
Baseline core revision: `87980ab4ade219f2d933bdca59c1411168f183d2`.
Candidate revision: `c8889abdd922186dfd627b64fb56e3c4383fe19f`.
The baseline executable is the exact candidate from the preceding helper-overhead experiment; the hashes match.

Each worker runs two untimed warmups followed by ten timed `advance(0.01, 20000)` calls.
Samples run serially with shuffled runtime order, seed 20261028.
Initial scene restoration and energy validation are outside the timer.
The default profiling and 1,000-entry/backedge thresholds remain enabled.
Every optimized sample enters its requested tier once with no compilation or deoptimization during timing.
Builds, tests and other workloads finish before timed measurements.

| Tier | Before slots | Indexed slots | Time reduction | Indexed-slot interquartile range |
| --- | ---: | ---: | ---: | ---: |
| Bytecode | 2.241 s | 1.566 s | 30.12% | 1.565–1.568 s |
| Reference LIR | 2.515 s | 2.046 s | 18.65% | 2.044–2.051 s |
| Cranelift | 2.168 s | 1.722 s | 20.58% | 1.717–1.723 s |
| Copy-and-patch | 2.316 s | 1.829 s | 21.05% | 1.825–1.833 s |

`locals-comparison.json` contains all observations, validation data, warmups and executable hashes.
These timings exclude source compilation and process startup.

The bytecode interpreter is faster than the current native tiers again.
Local bytecodes avoid generic helper dispatch, while native local access still pays for callbacks, root synchronization
and profiling around its helpers.
The experiment does not separately attribute the improvement to string lookup, allocation or dispatch savings.

`advance.lir.txt` is a diagnostic dump from the existing `hotspots` example, which uses 16-entry/128-backedge thresholds.
Its 47 local loads and 22 local stores now use slot indices; only the global `range` load remains named.
These are static operation counts, not weighted execution frequencies.

## Site-feedback cost diagnostic

| Bytecode build | Median | Interquartile range |
| --- | ---: | ---: |
| Indexed locals, full feedback | 1.622 s | 1.606–1.634 s |
| Indexed locals, site feedback removed | 1.084 s | 1.079–1.093 s |

The diagnostic takes 33.19% less time within this separate paired run.
Its full-feedback baseline is about 3.6% slower than in the preceding tier comparison, so comparisons use this run's
own baseline rather than mixing samples across experiments.
This result supports investigating bounded or sampled feedback while retaining immediate observations on guard failure.
Simply disabling feedback would discard information required by the JIT.

The diagnostic patch removes bytecode `Observe` emission and per-result type observations in bytecode stores.
It leaves control-flow counts, function argument/return profiles, GC and adaptive addition feedback enabled.
It does not remove all profiling, and it cannot provide the site metadata needed for normal JIT specialization.
It is an experimental build, not the default runtime or a proposed replacement for feedback collection.
The production candidate retains complete feedback.
Together with remaining boxed local helpers, this profiling cost helps explain why the current native tiers still lag
the bytecode baseline; it is not a measured decomposition of their entire runtime.

`no-site-feedback.patch` applies to candidate revision `c8889ab`.
`site-feedback-comparison.json` retains ten interleaved warmed bytecode samples per build with energy checks.
These diagnostic measurements run separately after the tier comparison.
They measure the cost of the removed site-feedback machinery, including its bytecode dispatch.

## Reproduce

Save the baseline before building the candidate, then run:

```sh
cargo build --release --example nbody_bench
python3 tools/compare_nbody.py \
  --baseline /path/to/baseline-nbody-bench \
  --candidate target/release/examples/nbody_bench \
  --output /tmp/tis-local-slots-results
```

For the diagnostic, save the indexed-local worker, apply the retained patch in a separate checkout, build it and use
the same command with `--tiers bytecode` and a new output directory.
Do not run builds or tests concurrently with timing.
The fixture and its license are preserved in `../2026-10-28-nbody/`.
