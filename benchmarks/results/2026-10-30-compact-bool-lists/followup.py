from pathlib import Path
import json,time,subprocess,random,statistics
b=Path(__file__).resolve().parent;photo=json.loads((b/'photo.json').read_text());reg=json.loads((b/'regressions.json').read_text());rng=random.Random(41);out={}
for name,d,keys,count in [('photo',photo,['before-native','after-native','pypy'],5),('unpack',reg['perf_unpack_sequence'],list(reg['perf_unpack_sequence']['commands']),7),('nbody',reg['nbody'],list(reg['nbody']['commands']),7),('json-parse',reg['text_json_parse_small'],list(reg['text_json_parse_small']['commands']),9)]:
 r={'commands':{k:d['commands'][k] for k in keys},'samples_ns':{k:[] for k in keys}}
 for i in range(count):
  rng.shuffle(keys)
  for k in keys:
   start=time.perf_counter_ns();p=subprocess.run(r['commands'][k],check=True,capture_output=True,text=True);ns=time.perf_counter_ns()-start;assert p.stdout==d['expected_stdout'];r['samples_ns'][k].append(ns)
 r['median_ms']={k:statistics.median(v)/1e6 for k,v in r['samples_ns'].items()};out[name]=r;(b/'followup.json').write_text(json.dumps(out,indent=2)+'\n');print(name,r['median_ms'],flush=True)
