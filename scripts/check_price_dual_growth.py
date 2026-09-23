#!/usr/bin/env python3
"""Build a separate capacity diagnostic with the benchmark's existing link libs."""
import csv
import hashlib
import io
import json
import shlex
import subprocess
from pathlib import Path
ROOT = Path(__file__).resolve().parents[1]
BUILD = ROOT / 'cpp/build'
source = ROOT / 'cpp/price_dual_growth.cpp'
obj = BUILD / 'price_dual_growth.o'
binary = BUILD / 'price_dual_growth'
compile_cmd = ['/usr/bin/clang++', '-std=c++20', '-O3', '-DNDEBUG', '-Wall', '-Wextra', '-Werror',
               '-I', str(ROOT / 'cpp/include'), '-isystem', str(ROOT / 'cpp/third_party/abseil-cpp'),
               '-c', str(source), '-o', str(obj)]
subprocess.run(compile_cmd, check=True)
link_cmd = shlex.split((BUILD / 'CMakeFiles/price_dual_bench.dir/link.txt').read_text())
for i, arg in enumerate(link_cmd):
    if arg.endswith('price_dual_bench.cpp.o'):
        link_cmd[i] = str(obj)
    if i and link_cmd[i - 1] == '-o':
        link_cmd[i] = str(binary)
subprocess.run(link_cmd, cwd=BUILD, check=True)
result = {'source_sha256': hashlib.sha256(source.read_bytes()).hexdigest(),
          'binary_sha256': hashlib.sha256(binary.read_bytes()).hexdigest(),
          'compile': compile_cmd, 'link': link_cmd, 'runs': []}
for r in range(3):
    proc = subprocess.run([str(binary)], capture_output=True, text=True, check=True)
    rows = [{k: int(v) for k, v in row.items()} for row in csv.DictReader(io.StringIO(proc.stdout))]
    changed = [x for x in rows if x['capacity_before'] != x['capacity_after']]
    result['runs'].append({'top_5': sorted(rows, key=lambda x: x['ns'], reverse=True)[:5],
                           'capacity_changes': changed})
out = ROOT / 'docs/results/price-dual-growth-diagnostic-2026-09-23.json'
with out.open('x') as stream:
    stream.write(json.dumps(result, indent=2) + '\n')
for r in result['runs']:
    print(r['top_5'])
