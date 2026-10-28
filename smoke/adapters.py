"""Explicit allowlist of reusable tests. No shell or arbitrary commands from TOML."""
from pathlib import Path
import json
import os
import re
import shutil
import sys
from urllib.parse import urlsplit

ROOT = Path(__file__).resolve().parents[1]


def command_for(adapter, directory, lab):
    env = os.environ.copy()
    jobs = lab.get("execution", {}).get("jobs", 2)
    if type(jobs) is not int or not 1 <= jobs <= 8:
        raise ValueError("execution.jobs must be 1..8 (reserve two jobs for nested builds)")
    env.update(CARGO_BUILD_JOBS=str(jobs), CMAKE_BUILD_PARALLEL_LEVEL="1",
               CMAKE_BUILD_PARALLEL="1", RAYON_NUM_THREADS="1", UV_THREADPOOL_SIZE="1")
    env["PAB_TEST_CATALOG_PATH"] = str(directory / "mcp-catalog.json")
    if adapter == "linux-media-compat":
        if not sys.platform.startswith("linux"):
            raise ValueError("Linux media compatibility requires a Linux host")
        if not shutil.which("g++") or not shutil.which("pkg-config"):
            raise ValueError("Linux media compatibility requires g++ and pkg-config/libdrm headers")
        return [sys.executable, str(ROOT / "smoke/linux_media_compat.py"),
                "--output", str(directory)], ROOT, env, "counts"
    if adapter == "android-relay":
        android = lab.get("android", {})
        if android.get("enabled") is not True or not android.get("serial"):
            raise ValueError("Explicitly enable a prepared Android test device in lab.toml")
        command = [sys.executable, str(ROOT / "smoke/android.py"), "--serial", android["serial"], "--output", str(directory)]
        for name in ("apk", "mcp", "fixture"):
            path = (ROOT / android.get(name, "missing-artifact")).resolve()
            if not path.is_file():
                raise ValueError(f"Missing Android test artifact: {name}")
            command += ["--" + name, str(path)]
        return command, ROOT, env, "counts"
    if adapter in ("python", "selftest", "packaging", "python-linux"):
        if adapter == "python-linux" and not sys.platform.startswith("linux"):
            raise ValueError("Linux bootstrap requires a Linux host/container")
        groups = {"python": "build", "selftest": "smoke", "packaging": "packaging", "python-linux": "linux"}
        return [sys.executable, str(ROOT / "smoke/unittest_adapter.py"), groups[adapter],
                str(directory / "counts.json")], ROOT, env, "counts"
    if adapter.startswith("cargo-"):
        if adapter == "cargo-media-timing" and os.name != "nt":
            raise ValueError("Windows media timing requires a Windows host")
        from smoke.native_env import prepare
        env = prepare(env)
        env["PAB_CLIENT_DEFAULTS_FILE"] = str(ROOT / "smoke/fixtures/client-defaults.toml")
        selections = {
            "cargo-core": ["-p", "pab-protocol", "-p", "pab-terminal", "--lib"],
            "cargo-mcp": ["-p", "pab-bridge", "--test", "mcp_operations"],
            "cargo-executor": ["-p", "pab-executor", "--lib"],
            "cargo-relay": ["-p", "pab-relay", "--lib"],
            "cargo-service": ["-p", "pab-server", "-p", "pab-relay", "--tests", "--no-fail-fast"],
            "cargo-remote-desktop": ["-p", "pab-remote-desktop", "--lib", "--test", "reliable_sessions"],
            "cargo-media-timing": ["-p", "pab-desktop-media", "--test", "windows_media_timing"],
            "cargo-media-conversion": ["-p", "pab-desktop-media", "--lib", "software::conversion_tests"],
            "cargo-file-manager": ["-p", "pab-file-manager", "-p", "pab-protocol", "--lib"],
            "cargo-file-manager-executor": ["-p", "pab-executor", "--lib", "filesystem_manager_tests"],
            "cargo-upload-pipeline": ["-p", "pab-bridge", "--lib", "upload_pipeline::tests"],
            "cargo-file-transfer-wire": ["-p", "pab-bridge", "--lib", "file_transfer::tests"],
            "cargo-native-file-transfer": ["-p", "pab-executor", "--lib", "transfer_native_tests"],
            "cargo-file-batches": ["-p", "pab-bridge", "--lib", "runtime::file_batch_tests::"],
        }
        if adapter not in selections:
            raise ValueError("Unknown Cargo adapter")
        if not shutil.which("cargo"):
            raise ValueError("Cargo is unavailable")
        if adapter == "cargo-executor":
            native = lab.get("native_executor", {})
            command = [sys.executable, str(ROOT / "smoke/executor.py"), "--output", str(directory)]
            if native.get("enabled") is True:
                command.append("--native")
                if os.name == "nt" and native.get("user") is not None:
                    command += ["--unit-user", str(int(native["user"]))]
                for key, variable in (("worker", "PAB_EXECUTION_TEST_WORKER"),
                                      ("worker", "PAB_DESKTOP_TERMINAL_TEST_WORKER"),
                                      ("process_fixture", "PAB_PROCESS_TEST_FIXTURE")):
                    if native.get(key):
                        path = (ROOT / native[key]).resolve(strict=True)
                        if not path.is_file():
                            raise ValueError(f"Native test executable is not a file: {key}")
                        env[variable] = str(path)
                if native.get("user") is not None:
                    env["PAB_EXECUTION_TEST_USER"] = str(native["user"])
            return command, ROOT, env, "counts"
        if adapter == "cargo-service":
            import tomllib
            from smoke.web_fixture import fixture_info
            fixture = (ROOT / lab.get("web", {}).get("fixture_config", "missing-fixture")).resolve()
            fixture_info(fixture)
            env["DATABASE_URL"] = tomllib.loads(fixture.read_text(encoding="utf-8"))["database"]["url"]
        # A single Cargo job also bounds native build scripts that inherit NUM_JOBS.
        return ["cargo", "test", "--locked", "-j", "1", *selections[adapter], "--", "--test-threads=1"], ROOT, env, "libtest"
    if adapter == "file-transfer-lab":
        settings = lab.get("file_transfer_lab", {})
        if settings.get("enabled") is not True:
            raise ValueError("The isolated two-host file transfer lab must be explicitly enabled")
        paths = {key: (ROOT / settings[key]).resolve(strict=True) for key in ("lab", "mcp")}
        command = [sys.executable, str(ROOT / "smoke/file_transfer.py"), "--lab", str(paths["lab"]),
                   "--mcp", str(paths["mcp"]), "--output", str(directory)]
        if settings.get("config"):
            command += ["--config", str((ROOT / settings["config"]).resolve(strict=True))]
        return command, ROOT, env, "counts"
    node = shutil.which("node")
    if not node:
        raise ValueError("Node.js is unavailable")
    if adapter == "web-model":
        cwd = ROOT / "apps/web"
        executable = cwd / "node_modules/vitest/vitest.mjs"
        command = [node, str(executable), "run", "--exclude", "e2e/**", "--maxWorkers=1",
                   "--reporter=json", f"--outputFile={directory / 'vitest.json'}"]
        parser = "vitest"
    elif adapter in ("web-ui", "web-live", "desktop-ui"):
        cwd = ROOT / ("apps/desktop" if adapter == "desktop-ui" else "apps/web")
        executable = cwd / "node_modules/@playwright/test/cli.js"
        command = [node, str(executable), "test", "--workers=1", "--retries=0", "--reporter=json",
                   f"--output={directory / 'browser'}"]
        if adapter != "desktop-ui":
            web = lab.get("web", {})
            parsed = urlsplit(web.get("url", ""))
            if (web.get("isolated") is not True or parsed.scheme != "https"
                    or parsed.hostname not in ("127.0.0.1", "localhost", "::1")
                    or parsed.username or parsed.password or parsed.query or parsed.fragment):
                raise ValueError("Web tests require an explicit isolated loopback HTTPS URL")
            env["PAB_WEB_TEST_URL"] = web["url"]
            env["PAB_WEB_BROWSER"] = web.get("browser", "chrome")
            if adapter == "web-live":
                fixture = (ROOT / web.get("fixture_config", "missing-fixture")).resolve()
                if not fixture.is_file():
                    raise ValueError("Isolated Web fixture configuration is missing")
                from smoke.web_fixture import fixture_info
                fixture_info(fixture)
                # Existing live tests require this exact fixture origin; do not silently skip them.
                if web["url"].rstrip("/") != "https://127.0.0.1:38623":
                    raise ValueError("Existing live Web suites require https://127.0.0.1:38623")
                env["PAB_ADMIN_FIXTURE_CONFIG"] = str(fixture)
                command += ["usage-live.spec.ts", "audit-live.spec.ts", "account-support-live.spec.ts",
                            "service-diagnostics-live.spec.ts"]
            else:
                command += ["auth-loading.spec.ts", "header-hover.spec.ts", "live-refresh.spec.ts",
                            "resource-refresh.spec.ts", "usage-refresh.spec.ts", "usage-management.spec.ts",
                            "device-navigation-bandwidth.spec.ts", "service-diagnostics.spec.ts",
                            "relay-management.spec.ts", "relay-diagnostics.spec.ts", "overview-dashboard.spec.ts"]
        env["PLAYWRIGHT_JSON_OUTPUT_NAME"] = str(directory / "playwright.json")
        env["PAB_WEB_ARTIFACT_DIR"] = str(directory)
        parser = "playwright"
    else:
        raise ValueError("Unknown adapter")
    if not executable.is_file():
        raise ValueError(f"Dependencies missing: run npm ci in {cwd.relative_to(ROOT).as_posix()}")
    return command, cwd, env, parser


def counts_for(parser, directory, output):
    if parser == "counts":
        return json.loads((directory / "counts.json").read_text(encoding="utf-8"))
    if parser == "vitest":
        result = json.loads((directory / "vitest.json").read_text(encoding="utf-8"))
        return {"passed": result["numPassedTests"], "failed": result["numFailedTests"],
                "skipped": result["numPendingTests"] + result.get("numTodoTests", 0)}
    if parser == "playwright":
        result = json.loads((directory / "playwright.json").read_text(encoding="utf-8"))
        stats = result["stats"]
        return {"passed": stats["expected"],
                "failed": stats["unexpected"] + stats["flaky"] + len(result.get("errors", [])),
                "skipped": stats["skipped"]}
    if parser == "libtest":
        rows = re.findall(r"test result: \w+\. (\d+) passed; (\d+) failed; (\d+) ignored", output)
        return dict(zip(("passed", "failed", "skipped"), (sum(int(row[i]) for row in rows) for i in range(3))))
    raise ValueError("Unknown result parser")


def result_status(returncode, counts):
    if set(counts) != {"passed", "failed", "skipped"} or any(type(v) is not int or v < 0 for v in counts.values()):
        return "unconfirmed"
    if returncode != 0 or counts["failed"]:
        return "fail"
    if counts["skipped"] or counts["passed"] == 0:
        return "blocked"
    return "pass"
