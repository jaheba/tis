from pathlib import Path
import json,time,subprocess,statistics,random
root=Path(__file__).resolve().parent
original=json.loads((root/'regressions.json').read_text());results={};rng=random.Random(23)
for name in ['sieve','game_nbody','barnes_hut','perf_unpack_sequence']:
    result={};commands=original[name]['samples'];keys=list(commands)
    for repeat in range(3):
        rng.shuffle(keys)
        for key in keys:
            command=commands[key]['command'];start=time.perf_counter_ns()
            run=subprocess.run(command,check=True,capture_output=True,text=True)
            elapsed=time.perf_counter_ns()-start
            assert run.stdout==original[name]['stdout']
            result.setdefault(key,[]).append(elapsed)
    results[name]={'samples_ns':result,'median_ns':{k:statistics.median(v) for k,v in result.items()}}
    (root/'followup.json').write_text(json.dumps(results,indent=2)+'\n')
    print(name,{k:round(v/1e6,2) for k,v in results[name]['median_ns'].items()},flush=True)
