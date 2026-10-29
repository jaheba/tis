# Numeric/helper profiling checkpoint

Started 2026-10-28 23:48 UTC / 29 October 00:48 Berlin. Hard cutoff 07:00 UTC / 08:00 Berlin. All existing user changes and prior optimizations must remain; no commits/pushes. Baseline is positional-final-{worker,run,nbody} in /private/tmp/tis-profile, validated 223 debug/release tests.

TEMPORARY diagnostic overlay is active in src/native_runtime.rs, src/tier.rs, examples/bench.rs. Exact before snapshots are /private/tmp/tis-profile/numeric-diagnostic-source/{native_runtime.rs,tier.rs,bench.rs}; restore these three exact files after the diagnostic worker finishes, preserving all earlier work. diagnostic.patch records instrumentation. No permanent diagnostic API is intended.

Diagnostic worker built successfully and frozen /private/tmp/tis-profile/numeric-diagnostic-worker. Untimed two-warmup/two-run histogram is running in session 50110. Do not run builds/tests/other profiles/timings while it runs. Raw result target helper-histogram.json here. Next inspect histogram, restore overlay, assert restored source hashes match positional-helpers manifest, then choose one measured optimization.

Diagnostic overlay restored exactly and source hashes verified against positional-helpers checkpoint. No diagnostic code remains. helper-detail.json confirms LoadLocal=7,789,970, StoreLocal=0, CheckLocal=0, CompareEq=4,374,996, Inplace+=4,288,832, UnaryNot=2,308,362 per warmed run. Highest-count helper is indexed local loading, so next optimization will cache proven invariant frame-local values at fresh native entry, retaining LoadLocal operations/fallbacks and OSR/feedback behavior. Next active checkpoint: ../2026-10-29-invariant-locals/CHECKPOINT.md.
