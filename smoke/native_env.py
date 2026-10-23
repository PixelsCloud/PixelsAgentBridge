"""Portable native-test environment; discover Visual Studio rather than hardcode it."""
import os
from pathlib import Path
import subprocess


def prepare(environment):
    env = dict(environment)
    if os.name != "nt":
        return env
    # Some upstream CMake projects add /MP themselves. The last option bounds it.
    env["_CL_"] = (env.get("_CL_", "") + " /MP1").strip()
    if env.get("CMAKE"):
        return env
    vswhere = Path(env.get("ProgramFiles(x86)", "C:/Program Files (x86)")) / "Microsoft Visual Studio/Installer/vswhere.exe"
    if vswhere.is_file():
        installation = subprocess.check_output([str(vswhere), "-latest", "-products", "*",
            "-requires", "Microsoft.VisualStudio.Component.VC.Tools.x86.x64", "-property", "installationPath"],
            text=True, timeout=15).strip()
        if installation:
            cmake = Path(installation) / "Common7/IDE/CommonExtensions/Microsoft/CMake/CMake/bin/cmake.exe"
            if cmake.is_file():
                # Bundled CMake understands that installed Visual Studio's generator.
                env["CMAKE"] = str(cmake)
                env["PATH"] = str(cmake.parent) + os.pathsep + env.get("PATH", "")
    return env
