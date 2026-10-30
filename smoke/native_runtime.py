"""Stage the same verified offline document dependencies used by installers for Cargo tests."""
from pathlib import Path
import argparse
import importlib.util
import platform
import os

ROOT = Path(__file__).resolve().parents[1]


def stage(destination):
    destination = Path(destination).resolve()
    if not any(destination.is_relative_to((ROOT / folder).resolve()) for folder in ("target", ".build")):
        raise ValueError("Test dependencies must be staged under this repository's target directory")
    system = platform.system()
    if system in ("Windows", "Linux") and platform.machine().lower() not in ("amd64", "x86_64"):
        raise ValueError("This repository currently packages PDFium for Windows/Linux x86_64 only")
    key = {"Windows": "windows", "Linux": "linux", "Darwin":
           "macos-arm64" if platform.machine() == "arm64" else "macos-x64"}[system]
    spec = importlib.util.spec_from_file_location("smoke_document_runtime", ROOT / "packaging/desktop/document_runtime.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    files = module.runtime_files(key)
    for name, content in files.items():
        path = (destination / "document-runtime" / name).resolve()
        if not path.is_relative_to(destination / "document-runtime"):
            raise ValueError("Unexpected runtime member")
        path.parent.mkdir(parents=True, exist_ok=True)
        if not path.is_file() or path.read_bytes() != content:
            path.write_bytes(content)
    return destination / "document-runtime"


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    target = Path(os.environ.get("CARGO_TARGET_DIR", ROOT / "target"))
    if not target.is_absolute():
        target = ROOT / target
    parser.add_argument("--destination", type=Path, default=target / "release/deps")
    print(stage(parser.parse_args().destination))
