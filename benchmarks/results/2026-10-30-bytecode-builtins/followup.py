from pathlib import Path
import json,subprocess,time,random,statistics
b=Path(__file__).resolve().parent;data=json.loads((b/'oneshot.json').read_text());out={};rng=random.Random(23)
for name in ['sieve','barnes-hut','perf_unpack_sequence','ticket-search','bisect-insort','text_json_parse_small','text_json_lines_small','text_csv_cleanup_small']:
 d=data[name];keys=['before-bytecode','after-bytecode']+(['monty'] if name=='sieve' else []);r={'commands':{k:d['commands'][k] for k in keys},'samples':{k:[] for k in keys}}
 for i in range(5 if name in ['sieve','barnes-hut','perf_unpack_sequence','ticket-search'] else 9):
  rng.shuffle(keys)
  for k in keys:
   t=time.perf_counter_ns();p=subprocess.run(d['commands'][k],capture_output=True,text=True,check=True);ns=time.perf_counter_ns()-t;assert p.stdout==d['expected_stdout'];r['samples'][k].append(ns)
 r['median_ms']={k:statistics.median(v)/1e6 for k,v in r['samples'].items()};out[name]=r;(b/'followup.json').write_text(json.dumps(out,indent=2)+'\n');print(name,r['median_ms'],flush=True)
