from pathlib import Path
import subprocess,re,json
r=Path('/Users/jaheba/p/tis/benchmarks/results/overnight-json-bisect-csv/workloads')
rows=[]
for name in ['bisect_search','bisect_insort','perf_json_dumps','perf_json_loads','text_json_transform_small','text_json_transform_medium','text_csv_aggregate_small','text_csv_aggregate_medium','text_csv_cleanup_small','text_csv_cleanup_medium','text_json_parse_small','text_json_parse_medium','text_json_lines_small','text_json_lines_medium']:
 p=subprocess.run(['/private/tmp/tis-overnight-candidate19-cli','--tier','native','--readonly-regions','--mutation-regions','--stats',str(r/(name+'.py'))],capture_output=True,text=True,check=True)
 stats={k:int(v) for k,v in re.findall(r'(\w+): (\d+)',p.stderr)}
 rows.append({'name':name,'stdout':p.stdout,'stats':stats})
Path('/private/tmp/overnight-diagnostics19.json').write_text(json.dumps(rows,indent=2)+'\n')
print([(r['name'],r['stats']['native_compile_ns'],r['stats']['native_runtime_calls'],r['stats']['native_root_syncs']) for r in rows])
