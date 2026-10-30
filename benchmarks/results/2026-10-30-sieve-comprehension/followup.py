from pathlib import Path
import json,time,subprocess,statistics,random
b=Path(__file__).resolve().parent;photo=json.loads((b/'photo.json').read_text());reg=json.loads((b/'regressions.json').read_text());rng=random.Random(71);out={}
for name,data,keys,count in [('photo',photo,['before-native','after-native'],5),('ticket-search',reg['ticket-search'],list(reg['ticket-search']['commands']),7),('text_json_transform_small',reg['text_json_transform_small'],list(reg['text_json_transform_small']['commands']),9),('text_json_parse_small',reg['text_json_parse_small'],list(reg['text_json_parse_small']['commands']),9)]:
 d={'commands':{k:data['commands'][k] for k in keys},'samples_ns':{k:[] for k in keys}}
 for _ in range(count):
  rng.shuffle(keys)
  for key in keys:
   start=time.perf_counter_ns();p=subprocess.run(d['commands'][key],check=True,capture_output=True,text=True);elapsed=time.perf_counter_ns()-start;assert p.stdout==data['expected_stdout'];d['samples_ns'][key].append(elapsed)
 d['median_ms']={k:statistics.median(values)/1e6 for k,values in d['samples_ns'].items()};out[name]=d;(b/'followup.json').write_text(json.dumps(out,indent=2)+'\n');print(name,d['median_ms'],flush=True)
