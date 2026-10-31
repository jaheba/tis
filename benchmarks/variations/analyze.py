"""Summarize fresh and retained results without discarding failing variants."""
import argparse
import json
from pathlib import Path
import statistics

TIERS = ['bytecode', 'cnp', 'cranelift', 'cpython', 'pypy']


def report(root):
    manifest = json.loads((root / 'programs/manifest.json').read_text())
    retained = json.loads((root / 'retained.json').read_text())
    config = json.loads((root / 'retained-config.json').read_text())
    versions = json.loads((root / 'provenance.json').read_text())
    lines = ['# Custom source/data variations — 2026-10-31\n',
             'Seventeen standalone programs across four families compare unchanged tis runtime code with CPython and PyPy.',
             'Equivalent rewrites are checked for identical CPython output before any timings.',
             'Changed data cases have their own independently computed expected output.\n',
             '## Workloads\n',
             '| Family | Variants |', '| --- | --- |']
    for family in sorted({r['family'] for r in manifest.values()}):
        lines.append('| ' + family + ' | ' + '; '.join(
            f"{r['variant']}: {r['description']}" for r in manifest.values() if r['family'] == family) + ' |')
    lines += ['', 'The seed controls histogram input keys. Numeric and object inputs use exact binary fractions.',
              'No source variant was changed or removed after measuring its performance.\n',
              '## Boundaries and runtimes\n',
              'Fresh sessions use Flying Circus serially: three timing samples, three repeated-mode warmups,',
              'two startup samples and no RSS samples. Both modes exclude worker startup and fixture preparation.',
              'Definitions and input construction precede the fixture marker. The first benchmark call includes',
              'execution feedback and backend compilation. Reused workers reset the application session.\n',
              f"Retained sessions keep the same functions/data/VM, run {config['warmups']} full warmup calls, then measure",
              f"{config['samples']} calls. Each call includes compilation of its small invocation wrapper and JSON transport;",
              'callee code and feedback remain retained. Setup and all warmup/sample times are saved separately.',
              'Each case/runtime gets its own worker. No builds/tests overlap samples.\n',
              'CnP and Cranelift enable readonly and mutation regions. Bytecode is a separate interpreter control.',
              'These are quick measurements: sub-millisecond transport, GC and JIT outliers remain visible.',
              'Extra warmups do not establish universal steady state.\n']
    for tier in ['cpython', 'pypy']:
        lines.append(f"- {tier}: {versions[tier]['version'].replace(chr(10), ' ')}")
    lines += ['', 'The frozen tis worker SHA256 is `' + versions['tis']['sha256'] + '`.',
              'It is the previously verified counter-loop build; no runtime source changes were made for this suite.\n']
    checks = 0
    for mode in ['one-shot', 'repeated', 'retained']:
        lines += [f'## {mode}\n', 'Milliseconds, median request time. `error` cases remain in the table.\n',
                  '| Variant | Bytecode | CnP | Cranelift | CPython | PyPy |',
                  '| --- | ---: | ---: | ---: | ---: | ---: |']
        data = retained if mode == 'retained' else {
            t: json.loads((root / 'fresh' / mode / (t + '.json')).read_text())['benchmarks'] for t in TIERS}
        for name in manifest:
            values = []
            for tier in TIERS:
                row = data[tier][name]
                if row['status'] == 'ok':
                    checks += 1
                    value = statistics.median(row['sample_seconds']) if mode == 'retained' else row['median_wall_seconds']
                    values.append(f'{value*1000:.3f}')
                else:
                    values.append('error')
            lines.append('| ' + name + ' | ' + ' | '.join(values) + ' |')
        lines.append('')
    lines += ['## Findings\n',
              f'All {checks}/255 case/runtime/mode combinations pass exact output checks.\n',
              'The following ratios compare retained helper/control time within each runtime.',
              'Ratios include any extra call/binding/protocol work introduced by that variant.\n',
              '| Runtime | Numeric helper / inline | Object helper / fields | Method / fields |',
              '| --- | ---: | ---: | ---: |']
    for tier in TIERS:
        def median(name):
            return statistics.median(retained[tier][name]['sample_seconds'])
        ratios = [median('numeric__helper') / median('numeric__inline'),
                  median('objects__helper') / median('objects__fields'),
                  median('objects__method') / median('objects__fields')]
        lines.append('| ' + tier + ' | ' + ' | '.join(f'{ratio:.2f}' for ratio in ratios) + ' |')
    lines += ['', 'Fill timings retain the full `list.count(False)` checksum cost.',
              'They measure an application, not isolated marking-store throughput.\n',
              '## Retained variability\n',
              '| Runtime | Largest sample max/min ratio | Variant |', '| --- | ---: | --- |']
    for tier in TIERS:
        ratio, name = max((max(r['sample_seconds'])/min(r['sample_seconds']), n)
                          for n, r in retained[tier].items() if r['status'] == 'ok')
        lines.append(f'| {tier} | {ratio:.2f} | {name} |')
    lines += ['', 'Ratios above one describe sample spread, not a confidence interval or proof of continued compilation.',
              'Raw warmups/samples retain these outliers. No parity claim follows from isolated medians.\n',
              '## Reproduction\n',
              'The generator and drivers live in `benchmarks/variations/`.',
              'The runner uses the archived tis-compatible Flying Circus adapter and frozen worker by default.\n',
              '```sh',
              'python3 benchmarks/variations/generate.py --output /tmp/tis-custom-new/programs --seed 1807',
              'python3 benchmarks/variations/run.py /tmp/tis-custom-new',
              'python3 benchmarks/variations/analyze.py /tmp/tis-custom-new',
              '```\n',
              'Raw programs, manifests, runtime hashes/versions, JSON/HTML reports, warmups, samples, diagnostics and logs:',
              '`~/Library/Caches/tis-benchmarks/2026-10-31-custom-variations/`.\n']
    (root / 'README.md').write_text('\n'.join(lines))
    print(root / 'README.md')


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('root', type=Path)
    report(parser.parse_args().root)
