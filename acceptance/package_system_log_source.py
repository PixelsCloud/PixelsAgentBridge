"""Create a small working-tree source snapshot for isolated cross-OS acceptance."""

from pathlib import Path
import subprocess
import sys
import tarfile


root = Path(__file__).resolve().parents[1]
destination = Path(sys.argv[1]).resolve()
tracked = subprocess.check_output(["git", "ls-files", "-z"], cwd=root).decode().split("\0")
new_roots = [
    Path("crates/os-system-log"),
    Path("crates/system-log-store"),
    Path("crates/protocol/src/system_log.rs"),
    Path("crates/executor/src/task_service/system_log.rs"),
]
files = {root / relative for relative in tracked if relative}
for relative in new_roots:
    path = root / relative
    files.update(path.rglob("*") if path.is_dir() else [path])
destination.parent.mkdir(parents=True, exist_ok=True)
with tarfile.open(destination, "w:gz") as archive:
    for path in sorted(files):
        if path.is_file():
            archive.add(path, arcname=path.relative_to(root).as_posix(), recursive=False)
print(f"{destination}: {destination.stat().st_size} bytes")
