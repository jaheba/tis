# Benchmark reports

Keep written reports and top-level reproduction code in Git.
Store raw samples, JSON/HTML reports, profiles, logs, binaries, archives and copied workload trees outside the repository.
The root .gitignore enforces this layout.

Historical artifact files are preserved locally under:

`~/Library/Caches/tis-benchmarks/2026-10-30-history-cleanup/artifacts/`

Old reports may reference paths that now resolve inside that archive rather than this checkout.
The same cache directory contains the pre-rewrite Git bundle, original Git directory and commit map.
The current benchmark-game report uses its own external evidence archive and reproduction script.
