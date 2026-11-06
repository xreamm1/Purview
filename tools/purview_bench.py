#!/usr/bin/env python3
"""Compare old/new Balanced on Windows using the project's fixed forest flight.

    python tools/purview_bench.py --build OLD_BIN_DIR --label old --profile old-balanced --repeats 2
    python tools/purview_bench.py --build NEW_BIN_DIR --label new --profile new-balanced --repeats 2

Run sequentially with other builds and games stopped. Each repeat creates a fresh seed-1234
world under .local/travel-bench-LABEL; real player options and saves are never opened.
Reports include raw frame times, RSS, process CPU, binary SHA256 and comparison limitations.
Different engine revisions include other changes: differences do not isolate this patch.
"""
import argparse
import ctypes
from ctypes import wintypes
import hashlib
import json
import os
from pathlib import Path
import sys
import time
from types import SimpleNamespace

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'tools'))
import perf_bench as bench

def windows_kernel():
    kernel = ctypes.WinDLL('kernel32', use_last_error=True)
    kernel.OpenProcess.argtypes = [wintypes.DWORD, wintypes.BOOL, wintypes.DWORD]
    kernel.OpenProcess.restype = wintypes.HANDLE
    kernel.GetProcessTimes.argtypes = [wintypes.HANDLE] + [ctypes.POINTER(wintypes.FILETIME)] * 4
    kernel.GetProcessTimes.restype = wintypes.BOOL
    kernel.CloseHandle.argtypes = [wintypes.HANDLE]
    kernel.CloseHandle.restype = wintypes.BOOL
    return kernel


class Sampler(bench.Sampler):
    def run(self):
        self.cpu = []
        kernel = windows_kernel()
        handle = kernel.OpenProcess(0x1000, False, self.pid)
        try:
            while not self.stop.is_set():
                now = time.time()
                rss = bench.rss_bytes(self.pid)
                if rss is not None:
                    self.samples.append((now, rss))
                values = [wintypes.FILETIME() for _ in range(4)]
                if handle and kernel.GetProcessTimes(handle, *(ctypes.byref(v) for v in values)):
                    ticks = sum((v.dwHighDateTime << 32) | v.dwLowDateTime for v in values[2:])
                    self.cpu.append((now, ticks / 10_000_000))
                self.stop.wait(0.25)
        finally:
            if handle:
                kernel.CloseHandle(handle)

    def summary(self, windows):
        result = super().summary(windows)
        seconds = elapsed = 0.0
        for start, end in windows:
            during = [(t, cpu) for t, cpu in self.cpu if start <= t <= end]
            if len(during) > 1:
                elapsed += during[-1][0] - during[0][0]
                seconds += during[-1][1] - during[0][1]
        if elapsed:
            result.update(process_cpu_seconds=seconds, sampled_wall_seconds=elapsed,
                          process_cpu_core_percent=100 * seconds / elapsed,
                          process_cpu_machine_percent=100 * seconds / elapsed / os.cpu_count())
        return result


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('--build', required=True)
    parser.add_argument('--label', required=True)
    parser.add_argument('--profile', choices=['old-balanced', 'new-balanced'], required=True)
    parser.add_argument('--build-profile', default='unknown', help='Cargo profile used for the binary; record unknown if unavailable')
    parser.add_argument('--revision', default='unknown', help='revision of this binary, independently of the checkout')
    parser.add_argument('--repeats', type=int, choices=range(1, 11), default=3)
    parser.add_argument('--resources', type=Path, default=bench.lab.RESOURCES)
    args = parser.parse_args()
    if sys.platform != 'win32':
        parser.error('this process CPU measurement runner requires Windows')
    if not args.label or any(c not in 'abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-_' for c in args.label):
        parser.error('--label must contain only letters, digits, - and _')
    bench.Sampler = Sampler
    bench.WORK = ROOT / '.local' / ('travel-bench-' + args.label)
    bench.WORK.mkdir(parents=True, exist_ok=True)
    bench.WORK.resolve().relative_to((ROOT / '.local').resolve())
    bench.OPTIONS.update(simulationDistance='8', renderClouds='"fast"', particles='1',
                         biomeBlendRadius='1', mipmapLevels='2', entityDistanceScaling='0.75', weatherRadius='5')
    original_write_options = bench.write_options
    workers = min(max((os.cpu_count() or 1) - 2, 1), 8)
    graphics = 'preset:balanced\ncaveCulling:true\nanimatedTextures:true\nframePacing:efficient\n'
    if args.profile == 'new-balanced':
        graphics += f'entityCulling:async\nparticleCulling:async\nblockEntityCache:true\nworkers:{workers}\n'
    else:
        graphics += 'entityCulling:off\nparticleCulling:off\nworkers:0\n'


    def write_options(game_dir, distance, version=None):
        result = original_write_options(game_dir, distance, version)
        (game_dir / 'horizon-graphics.txt').write_text(graphics)
        return result


    bench.write_options = write_options
    settings = SimpleNamespace(build=args.build, resources=str(args.resources.resolve()),
                               game='horizon', width=1280, height=720, offscreen=True,
                               warm=False, port_work=None, server=None, scenes='forest',
                               timeout=400, settle=5, seconds=12)
    binary = Path(args.build).resolve() / 'horizon-client.exe'
    with binary.open('rb') as stream:
        binary_sha256 = hashlib.file_digest(stream, 'sha256').hexdigest()
    result = {'label': args.label, 'binary_sha256': binary_sha256,
              'binary_revision': args.revision, 'build_profile': args.build_profile,
              'hardware': bench.hardware(), 'conditions': {'seed': 1234, 'render_distance': 12,
              'size': [1280, 720], 'seconds': 12, 'settle': 5, 'scene': 'forest', 'offscreen': True}, 'runs': []}
    result['conditions'].update(profile=args.profile, graphics_file=graphics)
    result['limitations'] = ['The binary revisions differ beyond these preset changes; FPS differences cannot be attributed entirely to this patch.',
                             'Process CPU is GetProcessTimes summed across threads, not the render CPU frame time. Machine percent is divided by logical CPU count.',
                             'Two or three short repeated forest flights do not measure fan noise or guarantee improvement in other scenes.']
    for repeat in range(args.repeats):
        run = bench.run_horizon(settings, 12)
        scene = run['scenes']['forest']
        run['frame_summary'] = bench.frames('forest', repeat, scene['frame_ms'], scene['cpu_ms'], scene['gpu_ms'])
        result['runs'].append(run)
        (bench.WORK / 'result.json').write_text(json.dumps(result, indent=2))
        print(json.dumps({'repeat': repeat, 'frames': run['frame_summary'], 'memory_cpu': run['memory']}), flush=True)


if __name__ == '__main__':
    main()
