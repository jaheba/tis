# Benchmark reports

[Cranelift Boolean loop regions](2026-10-31-cranelift-boolean-regions/README.md) compares both backends with matching
fill/reduction coverage.

[Lessons learned](../../docs/lessons-learned.md) collects conclusions, failed approaches and open questions across
experiments.

Keep written reports and top-level reproduction code in Git.
Store raw samples, JSON/HTML reports, profiles, logs, binaries, archives and copied workload trees outside the
repository.
The root .gitignore enforces this layout.

Historical artifact files are preserved locally under:

`~/Library/Caches/tis-benchmarks/2026-10-30-history-cleanup/artifacts/`

Old reports may reference paths that now resolve inside that archive rather than this checkout.
The same cache directory contains the pre-rewrite Git bundle, original Git directory and commit map.
The current benchmark-game report uses its own external evidence archive and reproduction script.

- [2026-10-31: full Flying Circus, six runtimes](2026-10-31-flying-circus-full/README.md): 120 public workloads, including Unicode; fresh-session timing and RSS.

- [2026-10-31: bytecode toward Monty parity](2026-10-31-monty-parity/README.md): guarded unpack batches, ASCII indexing and shared HIR bodies; full-suite controls and open regressions.

- [2026-10-31: borrowed string runtime and adaptive Unicode offsets](2026-10-31-string-runtime/README.md): search support, eight newly passing agent workloads, selective caches and full-suite/confirmation measurements.

- [2026-10-31: all 120 Flying Circus workloads](2026-10-31-all120/README.md): all four tiers, both fresh-session modes, runtime/library support and remaining boundaries.

- [2026-10-31: performance generalization probe](2026-10-31-variations/README.md): seeded source/data/layout variations expose loop-form and helper-boundary cliffs with a frozen runtime.

- [Counter-loop Boolean fills](2026-10-31-counter-loops/README.md): paired source-form controls and fresh holdouts.

- [Custom variation comparison](2026-10-31-custom-variations/README.md): 17 new workloads, five runtimes, cold/fresh/retained boundaries.

- [Custom variation fixes](2026-10-31-custom-fixes/README.md): guarded loops, count/dictionary runtime work, CnP body inlining, native float views and descriptor safety.

- [Loop-entry guards for inlined helpers](2026-10-31-loop-inlining/README.md): shared float-helper proof, both backends, controls and full-suite validation.
