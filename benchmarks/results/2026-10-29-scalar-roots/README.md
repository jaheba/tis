# Rejected scalar-result publication experiment

Ten serial interleaved freshly warmed native Barnes–Hut samples per binary measured 3.235 s → 3.469 s (7.2% slower). Every timed sample validated, with zero timed compilation/deoptimization. Callback/root synchronization counts were unchanged. Runtime change reverted exactly to the invariant-input checkpoint; no gain retained.

The candidate skipped quiet native scalar-result shadow-stack publication while retaining reference publication before GC, full eager/copy-patch behavior, explicit LIR maps and register-based reconstruction. 228 debug/release tests and fmt/Clippy passed, including a new mixed-result GC/deoptimization test. The rejected diff and raw samples/logs are archived. Cause of slowdown unresolved; do not repeatedly tune this unchanged approach. The extra actual-value branch may outweigh saved publication work; this is a hypothesis, not a profiled finding.

Next investigate pooling native rooted-index/helper-argument scratch vectors, which are currently allocated for each nested native invocation. No scalar-publication optimization remains.
