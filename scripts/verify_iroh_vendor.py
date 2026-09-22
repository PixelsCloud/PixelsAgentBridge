#!/usr/bin/env python3
"""Verify that the vendored iroh-relay is a pinned upstream plus one known patch."""

from __future__ import annotations

import argparse
import difflib
import hashlib
import json
import os
from pathlib import Path
import sys
import tomllib


REPO_ROOT = Path(__file__).resolve().parents[1]
MANIFEST_PATH = REPO_ROOT / "patches" / "iroh-relay" / "manifest.json"


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def files_under(root: Path) -> set[Path]:
    return {path.relative_to(root) for path in root.rglob("*") if path.is_file()}


def tree_sha256(root: Path) -> str:
    digest = hashlib.sha256()
    for path in sorted(path for path in root.rglob("*") if path.is_file()):
        digest.update(path.relative_to(root).as_posix().encode())
        digest.update(b"\0")
        digest.update(hashlib.sha256(path.read_bytes()).digest())
    return digest.hexdigest()


def find_upstream(package: str, version: str) -> Path | None:
    cargo_home = Path(os.environ.get("CARGO_HOME", Path.home() / ".cargo"))
    candidates = sorted((cargo_home / "registry" / "src").glob(f"*/{package}-{version}"))
    return candidates[-1] if candidates else None


def render_patch(upstream: Path, vendor: Path, modified: list[Path]) -> str:
    chunks: list[str] = []
    for relative in modified:
        original = (upstream / relative).read_text(encoding="utf-8").splitlines(keepends=True)
        patched = (vendor / relative).read_text(encoding="utf-8").splitlines(keepends=True)
        chunks.append(f"diff --git a/{relative.as_posix()} b/{relative.as_posix()}\n")
        chunks.extend(
            difflib.unified_diff(
                original,
                patched,
                fromfile=f"a/{relative.as_posix()}",
                tofile=f"b/{relative.as_posix()}",
            )
        )
    return "".join(chunks)


def verify_cargo_config(version: str, vendor_relative: str, errors: list[str]) -> None:
    workspace = tomllib.loads((REPO_ROOT / "Cargo.toml").read_text(encoding="utf-8"))
    configured = workspace.get("patch", {}).get("crates-io", {}).get("iroh-relay", {})
    if configured.get("path") != vendor_relative:
        errors.append("Cargo.toml does not patch iroh-relay to the recorded vendor path")

    relay = tomllib.loads((REPO_ROOT / "crates" / "relay" / "Cargo.toml").read_text(encoding="utf-8"))
    for section in ("dependencies", "dev-dependencies"):
        requirement = relay.get(section, {}).get("iroh-relay", {}).get("version")
        if requirement != f"={version}":
            errors.append(f"crates/relay {section} must pin iroh-relay to ={version}")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--write-patch",
        action="store_true",
        help="rewrite the recorded patch after hashes and vendor scope validate",
    )
    args = parser.parse_args()

    manifest = json.loads(MANIFEST_PATH.read_text(encoding="utf-8"))
    package = manifest["package"]
    version = manifest["version"]
    vendor_relative = manifest["vendor_path"]
    vendor = REPO_ROOT / vendor_relative
    patch_path = REPO_ROOT / manifest["patch_path"]
    records = manifest["modified_files"]
    modified = sorted(Path(path) for path in records)
    errors: list[str] = []

    verify_cargo_config(version, vendor_relative, errors)
    if tree_sha256(vendor) != manifest["vendor_tree_sha256"]:
        errors.append("vendored source tree differs from the recorded baseline")
    for relative in modified:
        actual = sha256(vendor / relative)
        expected = records[relative.as_posix()]["patched_sha256"]
        if actual != expected:
            errors.append(f"unexpected patched content: {relative.as_posix()}")

    upstream = find_upstream(package, version)
    if upstream is None:
        if args.write_patch:
            errors.append(f"Cargo registry source for {package} {version} is unavailable")
        elif not patch_path.exists():
            errors.append("recorded patch is missing")
        else:
            print(
                f"warning: pristine {package} {version} source unavailable; "
                "verified the complete vendor tree hash only",
                file=sys.stderr,
            )
    else:
        upstream_files = files_under(upstream)
        vendor_files = files_under(vendor)
        if upstream_files != vendor_files:
            errors.append("upstream and vendor file sets differ")
        for relative in sorted(upstream_files & vendor_files):
            if relative in modified:
                expected = records[relative.as_posix()]["upstream_sha256"]
                if sha256(upstream / relative) != expected:
                    errors.append(f"upstream baseline changed: {relative.as_posix()}")
            elif (upstream / relative).read_bytes() != (vendor / relative).read_bytes():
                errors.append(f"unrecorded vendor change: {relative.as_posix()}")

        rendered = render_patch(upstream, vendor, modified)
        if args.write_patch and not errors:
            patch_path.write_text(rendered, encoding="utf-8", newline="\n")
        elif patch_path.exists() and patch_path.read_text(encoding="utf-8") != rendered:
            errors.append("recorded patch does not reproduce the vendored changes")
        elif not patch_path.exists():
            errors.append("recorded patch is missing")

    if errors:
        for error in errors:
            print(f"ERROR: {error}", file=sys.stderr)
        return 1
    print(f"verified {package} {version}: {len(modified)} patched files, no unrecorded drift")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
