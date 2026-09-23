#!/usr/bin/env python3
"""Compare C++ full OMS and isolated locator costs, with parity gates."""
import argparse
import csv
import datetime
import hashlib
import io
import itertools
import json
import platform
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
BACKENDS = ('standard', 'pages', 'hash_ordered')

def call(args):
    return subprocess.run([str(x) for x in args], cwd=ROOT, text=True,
                          capture_output=True, check=True)

def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()

def main():
    p = argparse.ArgumentParser()
    p.add_argument('--samples', type=int, default=20000)
    p.add_argument('--rounds', type=int, default=6)
    p.add_argument('--build', default='cpp/build')
    p.add_argument('--check-only', action='store_true')
    p.add_argument('--tag', default=datetime.date.today().isoformat())
    a = p.parse_args()
    if not 0 < a.samples <= 200000 or a.rounds < 1:
        p.error('invalid sample count')
    if any(c not in 'abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-_' for c in a.tag):
        p.error('invalid tag')
    build = ROOT / a.build
    trace = ROOT / 'tests/data/oms-parity.trace'
    steps = len([x for x in trace.read_text().splitlines() if x and not x.startswith('#')])
    baseline = call([ROOT / 'target/parity/examples/oms_parity', 'trace', 'standard', trace]).stdout
    validation = {'steps': steps, 'trace_sha256': sha(trace), 'backends': {}}
    for backend in BACKENDS:
        output = call([build / 'oms_parity', 'trace', backend, trace]).stdout
        if len(output.splitlines()) != steps or output != baseline:
            raise RuntimeError(f'{backend}: stepwise Rust baseline mismatch')
        validation['backends'][backend] = hashlib.sha256(output.encode()).hexdigest()
    validation['index_tests'] = call([build / 'index_tests']).stdout.strip()
    print(json.dumps(validation, indent=2), flush=True)
    if a.check_only:
        return
    cache = (build / 'CMakeCache.txt').read_text()
    if 'CMAKE_BUILD_TYPE:STRING=Release' not in cache or 'OMS_SANITIZE:BOOL=ON' in cache:
        raise RuntimeError('timing requires Release without sanitizers')
    base = ROOT / f'docs/results/price-dual-{a.tag}'
    paths = {s: Path(str(base) + s) for s in ('-time.csv', '-alloc.csv', '-metadata.json')}
    if any(x.exists() for x in paths.values()):
        raise RuntimeError('use a fresh tag')
    source_paths = sorted((ROOT / 'cpp/include').glob('*.hpp')) + sorted((ROOT / 'cpp/parity').glob('*'))
    source_paths += [ROOT / 'cpp/price_dual_bench.cpp', ROOT / 'cpp/tests.cpp', ROOT / 'cpp/CMakeLists.txt', Path(__file__).resolve()]
    binaries = [build / x for x in ('oms_parity', 'price_dual_bench', 'price_dual_alloc', 'index_tests')]
    metadata = {'started_at': datetime.datetime.now().astimezone().isoformat(),
                'platform': platform.platform(), 'samples': a.samples, 'rounds': a.rounds,
                'validation': validation, 'compiler': call(['/usr/bin/clang++', '--version']).stdout,
                'flags': (build / 'CMakeFiles/oms_parity.dir/flags.make').read_text(),
                'source_sha256': {str(x.relative_to(ROOT)): sha(x) for x in source_paths},
                'binary_sha256': {str(x.relative_to(ROOT)): sha(x) for x in binaries},
                'digests': []}
    orders = list(itertools.permutations(BACKENDS))
    with paths['-time.csv'].open('x', newline='') as stream:
        writer = None
        for r in range(a.rounds):
            expected = None
            for backend in orders[r % len(orders)]:
                print(f'round {r + 1}/{a.rounds}: {backend}', flush=True)
                full = call([build / 'oms_parity', 'bench', backend, a.samples])
                digest = [x for x in full.stderr.splitlines() if x.startswith('DIGEST ')]
                if len(digest) != 5 or (expected is not None and digest != expected):
                    raise RuntimeError('full OMS workload digest mismatch')
                expected = digest
                metadata['digests'].append({'round': r, 'backend': backend, 'values': digest})
                local = call([build / 'price_dual_bench', backend, a.samples])
                for workload, proc, count in [('oms', full, 36), ('locator', local, 22)]:
                    rows = list(csv.DictReader(io.StringIO(proc.stdout)))
                    if len(rows) != count:
                        raise RuntimeError('unexpected metric count')
                    for row in rows:
                        row['round'] = r
                        row['workload'] = workload
                        row['backend'] = backend
                        if workload == 'oms' and row['scenario'].startswith(backend + '_'):
                            row['scenario'] = row['scenario'][len(backend) + 1:]
                        if writer is None:
                            writer = csv.DictWriter(stream, fieldnames=list(row))
                            writer.writeheader()
                        writer.writerow(row)
                stream.flush()
    with paths['-alloc.csv'].open('x', newline='') as stream:
        writer = None
        for backend in BACKENDS:
            rows = list(csv.DictReader(io.StringIO(call([build / 'price_dual_alloc', backend, a.samples]).stdout)))
            if len(rows) != 22:
                raise RuntimeError('allocation metric count')
            for row in rows:
                if writer is None:
                    writer = csv.DictWriter(stream, fieldnames=list(row))
                    writer.writeheader()
                writer.writerow(row)
    for path in source_paths + binaries:
        kind = 'source_sha256' if path in source_paths else 'binary_sha256'
        if sha(path) != metadata[kind][str(path.relative_to(ROOT))]:
            raise RuntimeError('source or binary changed during run')
    metadata['completed_at'] = datetime.datetime.now().astimezone().isoformat()
    paths['-metadata.json'].write_text(json.dumps(metadata, indent=2) + '\n')
    print(paths['-metadata.json'])

if __name__ == '__main__':
    main()
