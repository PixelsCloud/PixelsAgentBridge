"""Read-only GPU memory samples for one isolated product Desktop process tree."""
import argparse
import json
import pathlib
import subprocess
import time

import psutil


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--profile', type=pathlib.Path, required=True)
    parser.add_argument('--seconds', type=int, required=True)
    args = parser.parse_args()
    assert 60 <= args.seconds <= 32400
    root = args.profile.resolve(strict=True)
    owner = psutil.Process(json.loads((root / 'process.json').read_text())['pid'])
    assert pathlib.Path(owner.exe()).resolve() == root / 'pab-desktop.exe'
    # Exclusive creation preserves evidence from any previous observation.
    with (root / 'gpu-resources.jsonl').open('x', encoding='utf-8') as output:
        deadline = time.monotonic() + args.seconds
        while time.monotonic() < deadline and owner.is_running() and not (root / 'stop').exists():
            try:
                pids = [owner.pid] + [p.pid for p in owner.children(recursive=True)]
            except psutil.NoSuchProcess:
                break
            # Only numeric PIDs are inserted; no path, credential or environment text.
            identifiers = ','.join(str(pid) for pid in pids)
            command = (
                f"$observedIds=@({identifiers}); "
                "@(Get-CimInstance Win32_PerfFormattedData_GPUPerformanceCounters_GPUProcessMemory "
                "-ErrorAction Stop | Where-Object {$_.Name -match '^pid_(\\d+)_' "
                "-and [int]$Matches[1] -in $observedIds} | "
                "Select-Object Name,DedicatedUsage,SharedUsage,TotalCommitted) | ConvertTo-Json -Compress"
            )
            row = {'at_unix_ms': round(time.time() * 1000), 'observed_pids': pids}
            try:
                result = subprocess.run(
                    ['powershell.exe', '-NoProfile', '-NonInteractive', '-Command', command],
                    capture_output=True, text=True, timeout=20,
                    creationflags=subprocess.CREATE_NO_WINDOW,
                )
                row['exit_code'] = result.returncode
                if result.returncode == 0:
                    records = json.loads(result.stdout) if result.stdout.strip() else []
                    row['gpu_process_memory'] = [records] if isinstance(records, dict) else records
                    row['available'] = bool(row['gpu_process_memory'])
                else:
                    row['error'] = result.stderr[-1200:]
            except (subprocess.TimeoutExpired, json.JSONDecodeError) as error:
                row['error'] = type(error).__name__
            output.write(json.dumps(row) + '\n')
            output.flush()
            print(json.dumps({'gpu_sample_saved': True, 'available': row.get('available', False)}), flush=True)
            until = min(deadline, time.monotonic() + 30)
            while time.monotonic() < until and owner.is_running() and not (root / 'stop').exists():
                time.sleep(.5)
    print(json.dumps({'gpu_observer_stopped': True}), flush=True)


if __name__ == '__main__':
    main()
