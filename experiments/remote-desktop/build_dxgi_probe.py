"""Build one isolated Windows SDK probe with one compiler and <=10 CPU affinity."""
import ctypes
from ctypes import wintypes
import json
import os
from pathlib import Path
import shutil
import subprocess


def build_probe(filename="dxgi_probe.cpp", probe_name="dxgi", include_dir=None):
    if os.name != "nt":
        raise SystemExit("Windows SDK / MSVC required")
    kernel = ctypes.WinDLL("kernel32", use_last_error=True)
    kernel.GetCurrentProcess.restype = wintypes.HANDLE
    kernel.GetProcessAffinityMask.argtypes = [wintypes.HANDLE,
        ctypes.POINTER(ctypes.c_size_t), ctypes.POINTER(ctypes.c_size_t)]
    kernel.SetProcessAffinityMask.argtypes = [wintypes.HANDLE, ctypes.c_size_t]
    process_mask, system_mask = ctypes.c_size_t(), ctypes.c_size_t()
    handle = kernel.GetCurrentProcess()
    if not kernel.GetProcessAffinityMask(handle, ctypes.byref(process_mask), ctypes.byref(system_mask)):
        raise ctypes.WinError(ctypes.get_last_error())
    chosen = sum(1 << bit for bit in [
        bit for bit in range(64) if process_mask.value & (1 << bit)][:10])
    if not chosen or not kernel.SetProcessAffinityMask(handle, chosen):
        raise ctypes.WinError(ctypes.get_last_error())
    active = subprocess.check_output([
        "powershell.exe", "-NoProfile", "-NonInteractive", "-Command",
        "@(Get-Process cargo,rustc,cmake,ninja,cl,msbuild -ErrorAction SilentlyContinue).Count",
    ], text=True).strip()
    if active != "0":
        raise SystemExit("Another build is active; do not exceed the combined build limit")
    vswhere = Path(os.environ["ProgramFiles(x86)"]) / "Microsoft Visual Studio/Installer/vswhere.exe"
    installation = subprocess.check_output([
        str(vswhere), "-latest", "-products", "*", "-requires",
        "Microsoft.VisualStudio.Component.VC.Tools.x86.x64", "-property", "installationPath",
    ], text=True).strip()
    devcmd = Path(installation) / "Common7/Tools/VsDevCmd.bat"
    if not installation or not devcmd.is_file():
        raise SystemExit("MSVC x64 toolchain unavailable")
    # This command only initializes a child environment. Never print its contents.
    if any(c in str(devcmd) for c in '%!"\r\n'):
        raise SystemExit("Unsupported shell characters in toolchain path")
    raw = subprocess.check_output(
        f'cmd.exe /d /s /c ""{devcmd}" -arch=x64 -host_arch=x64 >nul && set"',
        text=True,
    )
    environment = {key.upper(): value for key, value in os.environ.items()}
    for line in raw.splitlines():
        name, separator, value = line.partition("=")
        if separator and name:
            environment[name.upper()] = value
    # No /MP, one translation unit, no nested builds. Neutralize implicit CL flags.
    environment.pop("CL", None)
    environment.pop("_CL_", None)
    environment.update(CARGO_BUILD_JOBS="1", CMAKE_BUILD_PARALLEL_LEVEL="1")
    source = Path(__file__).resolve().parent / filename
    output = source.parents[2] / ".build/remote-desktop" / probe_name
    output.mkdir(parents=True, exist_ok=True)
    compiler = shutil.which("cl.exe", path=environment["PATH"])
    if not compiler:
        raise SystemExit("MSVC environment has no cl.exe")
    subprocess.run([
        compiler, "/nologo", "/std:c++17", "/EHsc", "/W4", "/WX", "/O2", "/MT",
        *([f"/external:I{include_dir}", "/external:W0"] if include_dir else []),
        str(source), f"/Fo{output / (probe_name + '-probe.obj')}", f"/Fe{output / (probe_name + '-probe.exe')}",
        "/link", "d3d11.lib", "dxgi.lib", "user32.lib",
    ], cwd=output, env=environment, check=True)
    print(json.dumps({"executable": str(output / (probe_name + "-probe.exe")),
                      "build_jobs": 1, "affinity_cpu_count": chosen.bit_count()}))


if __name__ == "__main__":
    build_probe()
