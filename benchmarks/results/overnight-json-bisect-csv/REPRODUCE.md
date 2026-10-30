# Reproducing the final comparison

Frozen release executables are in binaries/. candidate20-source.tar.gz contains the exact Rust sources, Cargo files, build script, examples and regression fixtures. candidate20-all-source-sha256.json covers the build script, Rust sources, examples, tests and Cargo files. candidate20-build-provenance.json records hashes, compiler, flags and runtime configuration. Monty compiler flags remain unverified; its frozen binary hash is recorded. The archived CPython executable requires its Homebrew framework and standard library; use the pinned Cellar path in provenance.

The harness snapshot is ../2026-10-29-flying-circus-quick/harness-snapshot.tar.gz. Extract it into /private/tmp/flying-circus-tis (the archive contains src/flying_circus). The existing Flying Circus Python environment supplies its dependencies. The original harness checkout was not edited.

Run overnight_fc_final.py with a fresh label beginning with final and the absolute path to binaries/tis-candidate20-worker. It runs --quick serially, overridden to12 timing samples and3 startup samples, with the14 unchanged source snapshots in workloads/. Both modes include compilation, execution and output delivery. Change the root and pinned interpreter paths in the script only if relocating this evidence. Final raw commands are in overnight-final20-command.json.

For retained-VM execution, build examples/retained_benchmark.rs and run the archived overnight_retained20.py after adjusting its binary path to binaries/tis-candidate20-retained. This is a separate measurement boundary. The28 retained results and CPython reference stdout are in candidate20-retained/. Sampling-profiler runs are separate and their perturbed timings are excluded from comparisons.
