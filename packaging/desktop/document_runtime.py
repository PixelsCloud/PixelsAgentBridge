"""Build the pinned, offline PDFium and Tesseract data payload.

The PDFium archive is verified before extraction. No runtime download is needed
on an installed machine. Tesseract itself is statically linked into the worker.
"""

import argparse
import sys
from hashlib import sha256
from io import BytesIO
from pathlib import Path
import tarfile
from urllib.request import Request, urlopen
import zipfile

ROOT = Path(__file__).resolve().parents[2]
VERSION = "chromium/8086"
ASSETS = {
    "windows": ("pdfium-win-x64.tgz", "1fd8af952832dbb0eb16d9249f68fe09e5f5ebf7c3dd9f6066ea2720cc28487d", "bin/pdfium.dll", "pdfium.dll"),
    "linux": ("pdfium-linux-x64.tgz", "588577cf52dabc1a444988bac841920df54cc2f141801424de97ab04f4fbb935", "lib/libpdfium.so", "libpdfium.so"),
    "macos-arm64": ("pdfium-mac-arm64.tgz", "e98679e052c07edbb5a627980902abb823d4b3f35744d877bd21668bd9fc13ab", "lib/libpdfium.dylib", "libpdfium.dylib"),
    "macos-x64": ("pdfium-mac-x64.tgz", "933a85a138f6027243c56bff8676375c33ceeb767401389415ffc44d689ca85d", "lib/libpdfium.dylib", "libpdfium.dylib"),
}


def runtime_files(platform: str) -> dict[str, bytes]:
    archive_name, digest, library_member, library_name = ASSETS[platform]
    cache = ROOT / ".build" / "document-runtime" / archive_name
    if not cache.is_file() or sha256(cache.read_bytes()).hexdigest() != digest:
        url = f"https://github.com/bblanchon/pdfium-binaries/releases/download/{VERSION}/{archive_name}"
        request = Request(url, headers={"User-Agent": "PixelsAgentBridge-release"})
        with urlopen(request, timeout=120) as response:
            content = response.read(80 * 1024 * 1024 + 1)
        if len(content) > 80 * 1024 * 1024 or sha256(content).hexdigest() != digest:
            raise ValueError(f"PDFium archive checksum failed: {archive_name}")
        cache.parent.mkdir(parents=True, exist_ok=True)
        cache.write_bytes(content)
    with tarfile.open(cache, "r:gz") as package:
        member = package.getmember(library_member)
        if not member.isfile() or member.size > 80 * 1024 * 1024:
            raise ValueError("Unexpected PDFium library member")
        library = package.extractfile(member).read()
        license_file = package.extractfile("LICENSE").read()
    files = {library_name: library, "PDFIUM-LICENSE": license_file}
    if platform.startswith("macos-") and sys.platform == "darwin":
        sys.path.insert(0, str(ROOT / "scripts"))
        from macos_signing import IDENTIFIERS, sign
        signed = cache.parent / f"signed-pdfium-{platform}.dylib"
        signed.write_bytes(library)
        sign(signed, IDENTIFIERS["pdfium"])
        files[library_name] = signed.read_bytes()
    for name in ("eng.traineddata", "chi_sim.traineddata", "chi_sim_vert.traineddata", "LICENSE", "README.md"):
        files[f"tessdata/{name}"] = (ROOT / "assets" / "tessdata" / name).read_bytes()
    return files


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--platform", choices=ASSETS, required=True)
    args = parser.parse_args()
    files = runtime_files(args.platform)
    out = ROOT / ".build" / "document-runtime"
    out.mkdir(parents=True, exist_ok=True)
    if args.platform == "windows":
        destination = out / "document-runtime-windows.zip"
        with zipfile.ZipFile(destination, "w", compression=zipfile.ZIP_DEFLATED) as archive:
            for name, data in files.items():
                archive.writestr(name, data)
    else:
        destination = out / ("document-runtime-macos.tar.gz" if args.platform.startswith("macos-") else "document-runtime-linux.tar.gz")
        with tarfile.open(destination, "w:gz") as archive:
            for name, data in files.items():
                path = f"document-runtime/{name}"
                member = tarfile.TarInfo(path)
                member.size = len(data)
                member.mode = 0o644
                archive.addfile(member, BytesIO(data))
    print(destination)


if __name__ == "__main__":
    main()
