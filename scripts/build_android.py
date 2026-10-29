#!/usr/bin/env python3
"""Build Android components sequentially within a shared ten-core budget."""

from __future__ import annotations

import argparse
import ctypes
import json
import hashlib
import os
from pathlib import Path
import shutil
import re
import subprocess
import sys
import tomllib
import urllib.request

ROOT = Path(__file__).resolve().parents[1]
APP_ROOT = ROOT / "apps" / "android"
TARGETS = {
    "x86_64": ("x86_64-linux-android", "x86_64-linux-android"),
    "arm64-v8a": ("aarch64-linux-android", "aarch64-linux-android"),
}
APK_ACTIONS = ("assemble", "instrument", "verify")
MAX_JOBS = 10


def limit_processors() -> None:
    """Children inherit this affinity, including JVM and compiler worker threads."""
    if sys.platform == "win32":
        kernel = ctypes.WinDLL("kernel32", use_last_error=True)
        kernel.GetCurrentProcess.restype = ctypes.c_void_p
        process = kernel.GetCurrentProcess()
        available = ctypes.c_size_t()
        system = ctypes.c_size_t()
        kernel.GetProcessAffinityMask.argtypes = [
            ctypes.c_void_p, ctypes.POINTER(ctypes.c_size_t),
            ctypes.POINTER(ctypes.c_size_t),
        ]
        kernel.SetProcessAffinityMask.argtypes = [ctypes.c_void_p, ctypes.c_size_t]
        if not kernel.GetProcessAffinityMask(process, ctypes.byref(available), ctypes.byref(system)):
            raise ctypes.WinError(ctypes.get_last_error())
        bits = [1 << index for index in range(available.value.bit_length())
                if available.value & (1 << index)][:MAX_JOBS]
        if not kernel.SetProcessAffinityMask(process, sum(bits)):
            raise ctypes.WinError(ctypes.get_last_error())
    elif hasattr(os, "sched_getaffinity"):
        os.sched_setaffinity(0, sorted(os.sched_getaffinity(0))[:MAX_JOBS])


def environment() -> dict[str, str]:
    env = os.environ.copy()
    sdk = Path(env.get("ANDROID_HOME") or env.get("ANDROID_SDK_ROOT") or "D:/android/sdk")
    if not sdk.is_dir():
        raise RuntimeError("Set ANDROID_HOME to an installed Android SDK")
    ndk = sdk / "ndk" / "29.0.14206865"
    if not ndk.is_dir():
        raise RuntimeError(f"Install the pinned NDK: {ndk}")
    env.update(ANDROID_HOME=str(sdk), ANDROID_SDK_ROOT=str(sdk),
               CARGO_BUILD_JOBS=str(MAX_JOBS), CMAKE_BUILD_PARALLEL_LEVEL="1")
    # A nested CMake/MSVC build occupies one Cargo job; it must not fan out again.
    if os.name == "nt":
        env["_CL_"] = (env.get("_CL_", "") + " /MP1").strip()
        cmake = Path("C:/Program Files/Microsoft Visual Studio/18/Community/Common7/IDE/CommonExtensions/Microsoft/CMake/CMake/bin/cmake.exe")
        if cmake.is_file():
            env["CMAKE"] = str(cmake)
            env["PATH"] = str(cmake.parent) + os.pathsep + env["PATH"]
    java = env.get("PAB_ANDROID_JAVA_HOME")
    if not java:
        candidates = [Path("C:/Program Files/Android/Android Studio/jbr"),
                      Path(env.get("JAVA_HOME", ""))]
        java = next((str(path) for path in candidates if (path / "bin" / "java.exe").is_file()), None)
    if java:
        env["JAVA_HOME"] = java
    env["JAVA_TOOL_OPTIONS"] = f"-XX:ActiveProcessorCount={min(4, MAX_JOBS)}"
    host = {"win32": "windows-x86_64", "linux": "linux-x86_64", "darwin": "darwin-x86_64"}[sys.platform]
    toolchain = ndk / "toolchains" / "llvm" / "prebuilt" / host / "bin"
    for target, clang in TARGETS.values():
        compiler = str(toolchain / f"{clang}28-clang{'.cmd' if os.name == 'nt' else ''}")
        archiver = str(toolchain / f"llvm-ar{'.exe' if os.name == 'nt' else ''}")
        key = target.replace("-", "_")
        env[f"CARGO_TARGET_{key.upper()}_LINKER"] = compiler
        env[f"CC_{key}"] = compiler
        env[f"AR_{key}"] = archiver
    return env


def run(command: list[str], env: dict[str, str], cwd: Path = ROOT) -> None:
    print("Running:", " ".join(command), flush=True)
    subprocess.run(command, cwd=cwd, env=env, check=True)


def prepare_verifier() -> None:
    """Use the Android companion from the exact Cargo-locked verifier package."""
    lock = tomllib.loads((ROOT / "Cargo.lock").read_text(encoding="utf-8"))
    package = next(p for p in lock["package"] if p["name"] == "rustls-platform-verifier-android")
    cargo_root = Path(os.environ.get("CARGO_HOME", str(Path.home() / ".cargo")))
    candidates = list((cargo_root / "registry" / "src").glob(
        f"*/rustls-platform-verifier-android-{package['version']}/maven"))
    if len(candidates) != 1:
        raise RuntimeError("Build the Android native target first to fetch its locked TLS companion")
    shutil.copytree(candidates[0], ROOT / ".build" / "android" / "maven", dirs_exist_ok=True)


def prepare_document_runtime() -> None:
    """Verify pinned OCR binaries and model before bundling; no device downloads."""
    destination = ROOT / ".build/android/document-runtime"
    for artifact in tomllib.loads((APP_ROOT / "document-runtime.toml").read_text(encoding="utf-8"))["artifact"]:
        file = (destination / artifact["path"]).resolve()
        if not file.is_relative_to(destination.resolve()):
            raise RuntimeError("OCR artifact path escapes its build directory")
        def valid(path):
            return (path.is_file() and path.stat().st_size == artifact["size"]
                    and hashlib.sha256(path.read_bytes()).hexdigest() == artifact["sha256"])
        if valid(file):
            continue
        file.parent.mkdir(parents=True, exist_ok=True)
        temporary = file.with_suffix(file.suffix + ".download")
        try:
            with urllib.request.urlopen(artifact["url"], timeout=60) as response, temporary.open("wb") as output:
                remaining = artifact["size"] + 1
                while remaining:
                    block = response.read(min(65536, remaining))
                    if not block:
                        break
                    output.write(block)
                    remaining -= len(block)
            if not valid(temporary):
                raise RuntimeError(f"OCR artifact failed size/hash verification: {artifact['path']}")
            temporary.replace(file)
        finally:
            temporary.unlink(missing_ok=True)


def native(abi: str, env: dict[str, str], check: bool, profile: str = 'debug') -> None:
    target = TARGETS[abi][0]
    if check:
        run(["cargo", "check", "--locked", "-j", str(MAX_JOBS), "--target", target,
             "-p", "pab-agent-core", "-p", "pab-transport"], env)
        return
    run(["cargo", "build", "--locked", *(['--release'] if profile == 'release' else []), "-j", str(MAX_JOBS), "--target", target,
         "-p", "pab-android-agent"], env)
    destination = APP_ROOT / "transport" / "src" / "main" / "jniLibs" / abi
    destination.mkdir(parents=True, exist_ok=True)
    shutil.copy2(ROOT / "target" / target / profile / "libpab_android_agent.so", destination)


def release_version_code(version: str) -> int:
    # Explicit numeric product version, monotonic across all supported versions.
    if not re.fullmatch(r'(0|[1-9][0-9]*)\.(0|[1-9][0-9]?)\.(0|[1-9][0-9]?)', version):
        raise ValueError('Use a release version with minor/patch below 100')
    major, minor, patch = map(int, version.split('.'))
    code = major * 10000 + minor * 100 + patch
    if not 1 <= code <= 2_100_000_000:
        raise ValueError('Android release versionCode is out of range')
    return code


def build_release(version: str, env: dict[str, str]) -> Path:
    code = release_version_code(version)
    env = dict(env, PAB_INSTALLER_VERSION=version)
    required = ('PAB_ANDROID_KEYSTORE_FILE', 'PAB_ANDROID_KEYSTORE_PASSWORD',
                'PAB_ANDROID_KEY_ALIAS', 'PAB_ANDROID_KEY_PASSWORD', 'PAB_ANDROID_CERT_SHA256')
    if any(not env.get(name) for name in required):
        raise ValueError('Android release requires the existing keystore, credentials and pinned certificate SHA-256')
    if not Path(env['PAB_ANDROID_KEYSTORE_FILE']).is_file():
        raise ValueError('Android release keystore is missing')
    for abi in TARGETS:
        native(abi, env, False, profile='release')
    prepare_verifier()
    prepare_document_runtime()
    wrapper = str(APP_ROOT / ('gradlew.bat' if os.name == 'nt' else 'gradlew'))
    run([wrapper, '--no-daemon', f'--max-workers={min(2, MAX_JOBS)}',
         f'-PpixelsVersionCode={code}', f'-PpixelsVersionName={version}',
         'lintRelease', ':core:test', 'testDebugUnitTest', ':app:assembleRelease'], env, APP_ROOT)
    apk = APP_ROOT / 'app/build/outputs/apk/release/app-release.apk'
    apksigner = Path(env['ANDROID_HOME']) / 'build-tools/36.0.0' / ('apksigner.bat' if os.name == 'nt' else 'apksigner')
    result = subprocess.run([str(apksigner), 'verify', '--verbose', '--print-certs', str(apk)],
                            env=env, capture_output=True, text=True, check=True)
    actual = re.findall(r'^Signer #\d+ certificate SHA-256 digest: ([0-9a-fA-F]+)$', result.stdout, re.M)
    expected = env['PAB_ANDROID_CERT_SHA256'].replace(':', '').lower()
    if not re.fullmatch('[0-9a-f]{64}', expected) or [a.lower() for a in actual] != [expected]:
        raise ValueError('Android signing certificate does not match the pinned release identity')
    return apk


def build_device_release(abi: str, env: dict[str, str]) -> None:
    """Release code, existing USB package/certificate/data; never create a new key."""
    if not (Path.home() / '.android/debug.keystore').is_file():
        raise RuntimeError('The existing device signing key is required; do not generate a replacement')
    serial = tomllib.loads((ROOT / '.build/android/device.toml').read_text(encoding='utf-8'))['serial']
    adb = str(Path(env['ANDROID_HOME']) / 'platform-tools' / ('adb.exe' if os.name == 'nt' else 'adb'))
    package = 'com.pixels.agent.debug'
    installed = subprocess.check_output([adb, '-s', serial, 'shell', 'pm', 'path', package], env=env, text=True).strip().splitlines()
    base = next((line.removeprefix('package:') for line in installed if line.endswith('/base.apk')), None)
    if not base:
        raise RuntimeError('The existing development app must be installed; this action only updates it')
    original = ROOT / '.build/android/installed-device.apk'
    run([adb, '-s', serial, 'pull', base, str(original)], env)
    signer = str(Path(env['ANDROID_HOME']) / 'build-tools/36.0.0' / ('apksigner.bat' if os.name == 'nt' else 'apksigner'))
    def certificate(apk: Path) -> list[str]:
        value = subprocess.check_output([signer, 'verify', '--print-certs', str(apk)], env=env, text=True)
        return re.findall(r'^Signer #\d+ certificate SHA-256 digest: ([0-9a-fA-F]+)$', value, re.M)
    expected = certificate(original)
    if len(expected) != 1:
        raise RuntimeError('Cannot verify the installed device signing identity')
    env = dict(env, CARGO_PROFILE_RELEASE_OPT_LEVEL='2', CARGO_PROFILE_RELEASE_LTO='false',
               CARGO_PROFILE_RELEASE_CODEGEN_UNITS='16', CARGO_PROFILE_RELEASE_INCREMENTAL='true')
    native(abi, env, False, profile='release')
    prepare_verifier()
    prepare_document_runtime()
    wrapper = str(APP_ROOT / ('gradlew.bat' if os.name == 'nt' else 'gradlew'))
    run([wrapper, '--no-daemon', f'--max-workers={min(2, MAX_JOBS)}', '-PpixelsDeviceIdentity=true',
         f'-PpixelsDeviceAbi={abi}', f'-PpixelsVersionCode={next_version_code(env)}',
         ':app:assembleRelease'], env, APP_ROOT)
    apk = APP_ROOT / 'app/build/outputs/apk/release/app-release.apk'
    if certificate(apk) != expected:
        raise RuntimeError('Device certificate differs; update refused, installed app preserved')
    run([adb, '-s', serial, 'install', '-r', str(apk)], env)


def install_updates(env: dict[str, str], include_tests: bool) -> None:
    """Update the configured development phone in place; never uninstall or clear data."""
    device_file = ROOT / ".build" / "android" / "device.toml"
    if not device_file.exists():
        return
    serial = tomllib.loads(device_file.read_text(encoding="utf-8")).get("serial")
    if not isinstance(serial, str) or not serial.strip():
        raise RuntimeError(f"Set serial in {device_file}")
    adb = shutil.which("adb", path=env["PATH"])
    if not adb:
        raise RuntimeError("ADB is required to update the configured Android phone")
    state = subprocess.run([adb, "-s", serial, "get-state"], env=env,
                           capture_output=True, text=True, check=False)
    if state.returncode or state.stdout.strip() != "device":
        raise RuntimeError(f"APK built, but Android device {serial} is offline or unauthorized")
    outputs = APP_ROOT / "app" / "build" / "outputs" / "apk"
    run([adb, "-s", serial, "install", "-r", str(outputs / "debug/app-debug.apk")], env)
    if include_tests:
        run([adb, "-s", serial, "install", "-r", "-t",
             str(outputs / "androidTest/debug/app-debug-androidTest.apk")], env)


def next_version_code(env: dict[str, str]) -> int:
    """Reserve a monotonic development version only when an APK is being produced."""
    file = ROOT / ".build/android/version.toml"
    previous = 1
    if file.exists():
        previous = tomllib.loads(file.read_text(encoding="utf-8"))["version_code"]
        if not isinstance(previous, int) or isinstance(previous, bool) or previous < 1:
            raise RuntimeError("Invalid Android version.toml; preserve it for recovery")
    metadata = APP_ROOT / "app/build/outputs/apk/debug/output-metadata.json"
    if metadata.exists():
        for element in json.loads(metadata.read_text(encoding="utf-8"))["elements"]:
            previous = max(previous, element["versionCode"])
    device = ROOT / ".build/android/device.toml"
    if device.exists():
        serial = tomllib.loads(device.read_text(encoding="utf-8"))["serial"]
        result = subprocess.run(["adb", "-s", serial, "shell", "dumpsys", "package", "com.pixels.agent.debug"],
                                env=env, capture_output=True, text=True, timeout=20, check=False)
        match = re.search(r"\bversionCode=(\d+)", result.stdout)
        if match:
            previous = max(previous, int(match[1]))
    if previous >= 2_100_000_000:
        raise RuntimeError("Android versionCode exhausted")
    version = previous + 1
    temporary = file.with_suffix(".tmp")
    temporary.write_text(f"version_code = {version}\n", encoding="utf-8")
    temporary.replace(file)
    return version


def stage_server_tools() -> None:
    """Keep Android test binaries independent of other sessions rebuilding target/debug."""
    destination = ROOT / ".build/android/tools"
    destination.mkdir(parents=True, exist_ok=True)
    extension = ".exe" if os.name == "nt" else ""
    fingerprints = {}
    for name in ("pab-server", "pab-relay-server", "pab-mcp"):
        file = destination / (name + extension)
        shutil.copy2(ROOT / "target/debug" / file.name, file)
        with file.open("rb") as stream:
            fingerprints[file.name] = hashlib.file_digest(stream, "sha256").hexdigest()
    (destination / "manifest.json").write_text(json.dumps(fingerprints, indent=2) + "\n", encoding="utf-8")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=[
        "check-shared", "native", "assemble", "release", "device-release", "check", "format", "instrument",
        "verify", "regression", "clippy", "clippy-android", "test-native", "test-execution", "test-execution-linux", "test-port-forward", "test-port-forward-linux", "test-file-enhancements", "test-file-enhancements-linux", "test-files", "test-system", "test-documents", "test-documents-linux", "control-fixture", "remote-tools", "mcp-tools", "shared-test", "check-integration", "check-web", "server-tools", "public-compat", "linux-tools",
    ])
    parser.add_argument("--abi", choices=list(TARGETS), default="x86_64")
    parser.add_argument("--jobs", type=int, choices=range(1, 11), default=10,
                        help="Share the ten-job total budget with other active builds")
    parser.add_argument("--install-tests", action="store_true",
                        help="Also update the optional instrumentation APK on the configured phone")
    parser.add_argument('--version', help='Explicit version for the signed release action; no device access')
    args = parser.parse_args()
    if (args.action == 'release') != bool(args.version):
        parser.error('--version is required exclusively for release')
    global MAX_JOBS
    MAX_JOBS = args.jobs
    limit_processors()
    env = environment()
    lock_path = ROOT / ".build" / "android" / "build.lock"
    lock_path.parent.mkdir(parents=True, exist_ok=True)
    with lock_path.open("a+b") as lock:
        if os.name == "nt":
            import msvcrt
            lock.seek(0, os.SEEK_END)
            if lock.tell() == 0:
                lock.write(b"0")
                lock.flush()
            lock.seek(0)
            try:
                msvcrt.locking(lock.fileno(), msvcrt.LK_NBLCK, 1)
            except OSError:
                raise RuntimeError("Another Android build owns the build lock; wait for it to finish") from None
        else:
            import fcntl
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        if args.action == 'release':
            build_release(args.version, env)
            return
        if args.action == 'device-release':
            build_device_release(args.abi, env)
            return
        if args.action in APK_ACTIONS:
            # Kotlin/JNI must match every ABI shipped in the APK, including an
            # emulator ABI left by an earlier build. Never package stale natives.
            for abi in TARGETS:
                native(abi, env, False)
        elif args.action in ("check-shared", "native"):
            native(args.abi, env, args.action == "check-shared")
        if args.action == "regression":
            run(["cargo", "test", "--locked", "-j", str(MAX_JOBS),
                 "-p", "pab-protocol", "-p", "pab-task-runtime",
                 "-p", "pab-agent-core", "-p", "pab-transport"], env)
        if args.action == "clippy":
            run(["cargo", "clippy", "--locked", "-j", str(MAX_JOBS),
                 "-p", "pab-android-agent", "-p", "pab-device-session", "--all-targets", "--no-deps",
                 "--", "-D", "warnings"], env)
            # Shared core has existing lint warnings; retain and review them rather
            # than changing unrelated runtime code during final APK acceptance.
            run(["cargo", "clippy", "--locked", "-j", str(MAX_JOBS),
                 "-p", "pab-agent-core", "--all-targets", "--no-deps"], env)
        if args.action == "clippy-android":
            run(["cargo", "clippy", "--locked", "-j", str(MAX_JOBS), "--target", TARGETS[args.abi][0],
                 "-p", "pab-android-agent", "--lib", "--no-deps", "--", "-D", "warnings"], env)
        if args.action == "test-native":
            run(["cargo", "test", "--locked", "-j", str(MAX_JOBS), "-p", "pab-android-agent"], env)
        if args.action == "test-execution":
            run(["cargo", "test", "--locked", "-j", str(MAX_JOBS), "-p", "pab-terminal", "-p", "pab-task-runtime"], env)
            run(["cargo", "test", "--locked", "-j", str(MAX_JOBS), "-p", "pab-executor", "--lib", "command"], env)
        if args.action == "test-port-forward":
            run(["cargo", "test", "--locked", "-j", str(MAX_JOBS), "-p", "pab-device-session",
                 "-p", "pab-executor", "-p", "pab-bridge", "--lib", "port_forward"], env)
        if args.action == "test-port-forward-linux":
            cache = ROOT / ".build/linux-cargo-home"
            run(["docker", "--context", "desktop-linux", "run", "--rm", "--pull", "never",
                 "--cpus", str(MAX_JOBS), "--memory", "6g",
                 "--mount", f"type=bind,source={ROOT},target=/src,readonly",
                 "--mount", f"type=bind,source={cache},target=/cargo-home",
                 "--mount", "type=volume,source=pab-linux-bullseye-target-20261015,target=/cargo-target",
                 "--workdir", "/src", "--env", "CARGO_HOME=/cargo-home",
                 "--env", "CARGO_TARGET_DIR=/cargo-target", "--env", f"CARGO_BUILD_JOBS={MAX_JOBS}",
                 "--env", "CMAKE_BUILD_PARALLEL_LEVEL=1", "--env", "RAYON_NUM_THREADS=2",
                 "pab-linux-headless-build-deps:bullseye", "sh", "-c",
                 'cargo test --locked -j "$CARGO_BUILD_JOBS" -p pab-device-session -p pab-executor -p pab-bridge --lib port_forward'], env)
        if args.action == "test-file-enhancements":
            run(["cargo", "test", "--locked", "-j", str(MAX_JOBS), "-p", "pab-protocol", "--lib", "filesystem"], env)
            run(["cargo", "test", "--locked", "-j", str(MAX_JOBS), "-p", "pab-file-codec"], env)
            run(["cargo", "test", "--locked", "-j", str(MAX_JOBS), "-p", "pab-executor", "-p", "pab-bridge", "--lib", "filesystem"], env)
            run(["cargo", "test", "--locked", "-j", str(MAX_JOBS), "-p", "pab-bridge", "--bin", "pab-mcp", "filesystem"], env)
        if args.action == "test-file-enhancements-linux":
            cache = ROOT / ".build/linux-cargo-home"
            run(["docker", "--context", "desktop-linux", "run", "--rm", "--pull", "never", "--cpus", str(MAX_JOBS), "--memory", "6g",
                 "--mount", f"type=bind,source={ROOT},target=/src,readonly",
                 "--mount", f"type=bind,source={cache},target=/cargo-home",
                 "--mount", "type=volume,source=pab-linux-bullseye-target-20261015,target=/cargo-target",
                 "--workdir", "/src", "--env", "CARGO_HOME=/cargo-home", "--env", "CARGO_TARGET_DIR=/cargo-target", "--env", f"CARGO_BUILD_JOBS={MAX_JOBS}",
                 "--env", "CMAKE_BUILD_PARALLEL_LEVEL=1", "--env", "RAYON_NUM_THREADS=2", "pab-linux-headless-build-deps:bullseye", "sh", "-c",
                 'cargo test --locked -j "$CARGO_BUILD_JOBS" -p pab-protocol --lib filesystem && cargo test --locked -j "$CARGO_BUILD_JOBS" -p pab-file-codec && cargo test --locked -j "$CARGO_BUILD_JOBS" -p pab-executor -p pab-bridge --lib filesystem && cargo test --locked -j "$CARGO_BUILD_JOBS" -p pab-bridge --bin pab-mcp filesystem'], env)
        if args.action == "test-execution-linux":
            cache = ROOT / ".build/linux-cargo-home"
            run(["docker", "--context", "desktop-linux", "run", "--rm", "--pull", "never",
                 "--cpus", str(MAX_JOBS), "--memory", "6g",
                 "--mount", f"type=bind,source={ROOT},target=/src,readonly",
                 "--mount", f"type=bind,source={cache},target=/cargo-home",
                 "--mount", "type=volume,source=pab-linux-bullseye-target-20261015,target=/cargo-target",
                 "--workdir", "/src", "--env", "CARGO_HOME=/cargo-home",
                 "--env", "CARGO_TARGET_DIR=/cargo-target", "--env", f"CARGO_BUILD_JOBS={MAX_JOBS}",
                 "--env", "CMAKE_BUILD_PARALLEL_LEVEL=1", "--env", "RAYON_NUM_THREADS=2",
                 "pab-linux-headless-build-deps:bullseye", "sh", "-c",
                 'cargo test --locked -j "$CARGO_BUILD_JOBS" -p pab-terminal -p pab-task-runtime && cargo test --locked -j "$CARGO_BUILD_JOBS" -p pab-executor --lib command'], env)
        if args.action == "test-documents":
            run(["cargo", "test", "--locked", "-j", str(MAX_JOBS), "-p", "pab-document",
                 "--no-default-features", "--lib"], env)
            run(["cargo", "test", "--locked", "-j", str(MAX_JOBS), "-p", "pab-executor",
                 "--lib", "document"], env)
        if args.action == "test-documents-linux":
            run(["docker", "--context", "desktop-linux", "run", "--rm", "--pull", "never",
                 "--cpus", str(MAX_JOBS), "--memory", "6g",
                 "--mount", f"type=bind,source={ROOT},target=/src,readonly",
                 "--mount", f"type=bind,source={ROOT / '.build/linux-cargo-home'},target=/cargo-home",
                 "--mount", "type=volume,source=pab-linux-bullseye-target-20261015,target=/cargo-target", "--workdir", "/src",
                 "--env", "CARGO_HOME=/cargo-home", "--env", "CARGO_TARGET_DIR=/cargo-target",
                 "--env", f"CARGO_BUILD_JOBS={MAX_JOBS}", "--env", "CMAKE_BUILD_PARALLEL_LEVEL=1",
                 "--env", "RAYON_NUM_THREADS=2", "pab-linux-headless-build-deps:bullseye", "sh", "-c",
                 'cargo test --locked -j "$CARGO_BUILD_JOBS" -p pab-document --no-default-features --lib && cargo test --locked -j "$CARGO_BUILD_JOBS" -p pab-executor --lib document'], env)
        if args.action == "test-system":
            packages = ["-p", "pab-android-agent", "-p", "pab-protocol", "-p", "pab-platform",
                        "-p", "pab-desktop-control", "-p", "pab-bridge"]
            for pattern in ("system", "application", "package", "every_tool_has_exactly_one_group"):
                run(["cargo", "test", "--locked", "-j", str(MAX_JOBS), *packages,
                     "--lib", "--bins", pattern], env)
        if args.action == "test-files":
            run(["cargo", "test", "--locked", "-j", str(MAX_JOBS),
                 "-p", "pab-file-codec", "-p", "pab-android-agent", "-p", "pab-executor",
                 "--lib", "filesystem"], env)
        if args.action == "control-fixture":
            run(["cargo", "build", "--locked", "-j", str(MAX_JOBS), "-p", "pab-android-agent",
                 "--example", "control-fixture"], env)
        if args.action == "public-compat":
            run(["cargo", "build", "--locked", "-j", str(MAX_JOBS), "-p", "pab-android-agent",
                 "--example", "public-compat"], env)
            run([str(ROOT / "target/debug/examples" / ("public-compat.exe" if os.name == "nt" else "public-compat"))], env)
        if args.action == "linux-tools":
            output = ROOT / ".build/android/tools-linux"
            output.mkdir(parents=True, exist_ok=True)
            cache = ROOT / ".build/linux-cargo-home"
            cache.mkdir(parents=True, exist_ok=True)
            run(["docker", "--context", "desktop-linux", "run", "--rm", "--pull", "never",
                 "--cpus", str(MAX_JOBS), "--memory", "4g",
                 "--mount", f"type=bind,source={ROOT},target=/src,readonly",
                 "--mount", f"type=bind,source={cache},target=/cargo-home",
                 "--mount", "type=volume,source=pab-linux-bullseye-target-20261015,target=/cargo-target",
                 "--mount", f"type=bind,source={output},target=/out", "--workdir", "/src",
                 "--env", "CARGO_HOME=/cargo-home", "--env", "CARGO_TARGET_DIR=/cargo-target",
                 "--env", f"CARGO_BUILD_JOBS={MAX_JOBS}", "--env", "CMAKE_BUILD_PARALLEL_LEVEL=1",
                 "pab-linux-headless-build-deps:bullseye", "sh", "-c",
                 'cargo build --locked -j "$CARGO_BUILD_JOBS" -p pab-bridge --bin pab-mcp && cp /cargo-target/debug/pab-mcp /out/pab-mcp'], env)
            (output / "manifest.json").write_text(json.dumps({"platform":"linux-x86_64",
                "cargo_jobs":MAX_JOBS,"cpu_quota":MAX_JOBS,"installed_application_updated":False,
                "sha256":hashlib.sha256((output / "pab-mcp").read_bytes()).hexdigest()}, indent=2), encoding="utf-8")
        if args.action == "remote-tools":
            run(["cargo", "build", "--locked", "-j", str(MAX_JOBS), "-p", "pab-android-agent", "--example", "remote-fixture"], env)
            run(["cargo", "build", "--locked", "-j", str(MAX_JOBS), "-p", "pab-bridge", "--bin", "pab-mcp"], env)
        if args.action == "mcp-tools":
            run(["cargo", "build", "--locked", "-j", str(MAX_JOBS), "-p", "pab-bridge", "--bin", "pab-mcp"], env)
        if args.action == "shared-test":
            run(["cargo", "test", "--locked", "-j", str(MAX_JOBS), "-p", "pab-device-session", "-p", "pab-executor", "-p", "pab-bridge", "--lib", "--bins", "--", "--test-threads=1"], env)
        if args.action == "server-tools":
            run(["cargo", "build", "--locked", "-j", str(MAX_JOBS), "-p", "pab-server", "-p", "pab-relay", "-p", "pab-bridge", "--bin", "pab-server", "--bin", "pab-relay-server", "--bin", "pab-mcp"], env)
            stage_server_tools()
        if args.action == "check-integration":
            run(["cargo", "check", "--locked", "-j", str(MAX_JOBS), "-p", "pab-server", "-p", "pab-relay", "--all-targets"], env)
        if args.action == "check-web":
            runner = "npm.cmd" if os.name == "nt" else "npm"
            run([runner, "exec", "--", "tsc", "--noEmit"], env, ROOT / "apps/web")
        if args.action in ("assemble", "check", "format", "instrument", "verify"):
            prepare_verifier()
            wrapper = str(APP_ROOT / ("gradlew.bat" if os.name == "nt" else "gradlew"))
            tasks = {
                "assemble": [":app:assembleDebug"],
                "check": ["spotlessCheck", "lintDebug", ":core:test", "testDebugUnitTest"],
                "format": ["spotlessApply"],
                "instrument": [":app:assembleDebug", ":app:assembleDebugAndroidTest"],
                "verify": ["spotlessApply", "spotlessCheck", "lintDebug", ":core:test",
                           ":app:assembleDebug", ":app:assembleDebugAndroidTest"],
            }[args.action]
            version = []
            if args.action != "format":
                prepare_document_runtime()
            if args.action in APK_ACTIONS:
                version = [f"-PpixelsVersionCode={next_version_code(env)}"]
            run([wrapper, "--no-daemon", f"--max-workers={min(2, MAX_JOBS)}", *version, *tasks], env, APP_ROOT)
            if args.action in APK_ACTIONS:
                install_updates(env, include_tests=args.install_tests)


if __name__ == "__main__":
    main()
