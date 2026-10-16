"""Build the isolated NVENC probe against the pinned upstream header only."""
from hashlib import sha256
from pathlib import Path
import subprocess
import tomllib

from build_dxgi_probe import build_probe


if __name__ == "__main__":
    root = Path(__file__).resolve().parents[2]
    pins = tomllib.loads(Path(__file__).with_name("sources.toml").read_text(encoding="utf-8"))
    external = root / ".build/remote-desktop/upstream/hwcodec/externals"
    revision = subprocess.check_output(
        ["git", "-C", str(external), "rev-parse", "HEAD"], text=True).strip()
    if revision != pins["hwcodec"]["externals_commit"]:
        raise SystemExit("NVENC headers must come from the pinned externals revision")
    include = external / "nv-codec-headers_n12.1.14.0/include/ffnvcodec"
    header = include / "nvEncodeAPI.h"
    original = subprocess.check_output([
        "git", "-C", str(external), "show",
        "HEAD:nv-codec-headers_n12.1.14.0/include/ffnvcodec/nvEncodeAPI.h"])
    if sha256(original).hexdigest() != pins["nvenc_header"]["git_blob_sha256"]:
        raise SystemExit("NVENC header content does not match its recorded hash")
    if header.read_bytes().replace(b"\r\n", b"\n") != original.replace(b"\r\n", b"\n"):
        raise SystemExit("Pinned NVENC header has local modifications")
    print("NVENC header SHA256: " + sha256(original).hexdigest(), flush=True)
    build_probe("nvenc_probe.cpp", "nvenc", include)
