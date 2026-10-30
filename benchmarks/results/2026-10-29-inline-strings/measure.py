import json, subprocess, time, statistics, hashlib, platform
from pathlib import Path
root=Path('/Users/jaheba/p/tis')
sources=root/'benchmarks/results/2026-10-29-flying-circus-quick/quick/workloads'
binaries={'before':'/Users/jaheba/.cargo/bin/tis','after':'/private/tmp/tis-leaf-isolated-target/release/tis'}
names=['rustpython_micro_strings','game_nbody','game_spectral_norm','perf_float','perf_unpack_sequence','rustpython_pystone','perf_barnes_hut','text_line_logs_medium','perf_json_loads','api_report','memory_string_index_short','memory_string_index_long']
result={'method':'Three interleaved fresh-process samples; bytecode tier; full workload sizes; includes process startup and source compilation; no warmup. No builds or tests overlap measurement.', 'machine':platform.platform(), 'binaries':{k:{'path':v,'sha256':hashlib.sha256(Path(v).read_bytes()).hexdigest()} for k,v in binaries.items()},'workloads':{}}
for name in names:
    path=sources/(name+'.py')
    samples={k:[] for k in binaries}
    expected=None
    for sample in range(3):
        for label in (list(binaries) if sample%2==0 else list(reversed(binaries))):
            start=time.perf_counter_ns()
            run=subprocess.run([binaries[label],'--tier','bytecode',str(path)],capture_output=True,check=True)
            elapsed=(time.perf_counter_ns()-start)/1e6
            if expected is None: expected=run.stdout
            assert run.stdout==expected, (name,label)
            assert not run.stderr, (name,label,run.stderr)
            samples[label].append(elapsed)
    medians={k:statistics.median(v) for k,v in samples.items()}
    row={'samples_ms':samples,'median_ms':medians,'speedup':medians['before']/medians['after'],'source_sha256':hashlib.sha256(path.read_bytes()).hexdigest(),'stdout':expected.decode()}
    result['workloads'][name]=row
    print(name,medians,'speedup',round(row['speedup'],3),flush=True)
rss = {}
for label, binary in binaries.items():
    code = "chars = list('a' * 50000)\nprint(len(chars))"
    run = subprocess.run(['/usr/bin/time', '-l', binary, '--tier', 'bytecode', '-c', code], capture_output=True, text=True, check=True)
    assert run.stdout == '50000\n'
    import re
    peak = int(re.search(r'(\d+)\s+maximum resident set size', run.stderr).group(1))
    rss[label] = peak
result['retained_characters_peak_rss_bytes'] = rss
print('50k retained characters peak RSS bytes', rss, flush=True)
Path('/private/tmp/tis-inline-results.json').write_text(json.dumps(result,indent=2)+'\n')
