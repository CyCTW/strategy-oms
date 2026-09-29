#!/usr/bin/env python3
"""Run prebuilt Release C++ experiments serially and record source/build metadata."""
import argparse
import datetime
import hashlib
import json
import os
import platform
import subprocess
from pathlib import Path

root = Path(__file__).resolve().parents[1]
parser = argparse.ArgumentParser()
parser.add_argument('--samples', type=int, default=20_000)
parser.add_argument('--rounds', type=int, default=14)  # 7 backends: balanced order needs 14
parser.add_argument('--tag', default=datetime.date.today().isoformat())
args = parser.parse_args()
if not (0 < args.samples <= 1_000_000 and args.rounds > 0):
    parser.error('samples must be 1..1,000,000; rounds must be positive (benchmark guard only)')
if any(c not in 'abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-_' for c in args.tag):
    parser.error('tag must contain only letters, digits, dash or underscore')

def output(command):
    return subprocess.check_output(command, cwd=root, text=True).strip()

build = root / 'cpp/build'
cache = (build / 'CMakeCache.txt').read_text()
compiler = next(line.split('=',1)[1] for line in cache.splitlines() if line.startswith('CMAKE_CXX_COMPILER:'))
if 'CMAKE_BUILD_TYPE:STRING=Release' not in cache or 'OMS_SANITIZE:BOOL=ON' in cache:
    raise SystemExit('Configure cpp/build as Release with sanitizers OFF before timing.')
commit = output(['git', '-C', 'cpp/third_party/abseil-cpp', 'rev-parse', 'HEAD'])
if commit != 'd9e4955c65cd4367dd6bf46f4ccb8cd3d100540b':
    raise SystemExit('Abseil commit does not match cpp/DEPENDENCIES.md')
paths = {p: root / f'docs/results/cpp-linear-{p}-{args.tag}.csv' for p in ['time','alloc']}
meta_path = root / f'docs/results/cpp-linear-metadata-{args.tag}.json'
for p in [*paths.values(), meta_path]:
    if p.exists():
        raise SystemExit(f'Result already exists: {p}; use a new --tag')
env = os.environ.copy()
env.update(OMS_BENCH_SAMPLES=str(args.samples), OMS_BENCH_ROUNDS=str(args.rounds))
metadata = {
    'started_at': datetime.datetime.now().astimezone().isoformat(),
    'suite': 'linear-search', 'large_book_samples_cap': 4000, 'range_samples_cap': 2000, 'samples': args.samples, 'rounds': args.rounds, 'platform': platform.platform(),
    'machine': platform.machine(), 'abseil_commit': commit,
    'compiler': output([compiler, '--version']),
    'cmake': output(['cmake', '--version']),
    'timing_flags': (build/'CMakeFiles/linear_bench.dir/flags.make').read_text(),
    'allocation_flags': (build/'CMakeFiles/linear_alloc.dir/flags.make').read_text(),
    'source_sha256': {
        str(p.relative_to(root)): hashlib.sha256(p.read_bytes()).hexdigest()
        for p in sorted((root/'cpp/include').glob('*.hpp')) + [root/'cpp/linear_bench.cpp',root/'cpp/tests.cpp',root/'cpp/CMakeLists.txt']
    },
}
for pass_name, executable in [('time','linear_bench'),('alloc','linear_alloc')]:
    print(f'Running {pass_name} pass', flush=True)
    with paths[pass_name].open('x') as f:
        subprocess.run([str(build/executable)], cwd=root, env=env, stdout=f, check=True)
    metadata[executable+'_sha256'] = hashlib.sha256((build/executable).read_bytes()).hexdigest()
metadata['completed_at'] = datetime.datetime.now().astimezone().isoformat()
meta_path.write_text(json.dumps(metadata, indent=2)+'\n')
print(meta_path)
