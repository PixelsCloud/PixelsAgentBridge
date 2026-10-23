"""Machine-readable counts for selected existing unittest suites."""
from pathlib import Path
import json
import sys
import unittest

ROOT = Path(__file__).resolve().parents[1]


def main():
    # Preserve readable Chinese names in redirected logs on Windows as well.
    for stream in (sys.stdout, sys.stderr):
        if hasattr(stream, "reconfigure"):
            stream.reconfigure(encoding="utf-8")
    group, destination = sys.argv[1:]
    selections = {
        "build": ("scripts", ["test_build_version.py", "test_acceptance.py", "test_android_acceptance.py",
                               "test_linux_package.py", "test_linux_desktop_release.py"]),
        "linux": ("scripts", ["test_linux_bootstrap.py"]),
        "packaging": ("packaging/desktop", ["test_packaging.py", "test_desktop_startup.py"]),
        "smoke": ("smoke/tests", ["test_*.py"]),
    }
    folder, patterns = selections[group]
    sys.path.insert(0, str(ROOT))
    sys.path.insert(0, str(ROOT / folder))
    suite = unittest.TestSuite()
    for pattern in patterns:
        discovered = unittest.defaultTestLoader.discover(str(ROOT / folder), pattern=pattern)
        if discovered.countTestCases() == 0:
            raise RuntimeError(f"No unittest cases discovered for {pattern}; do not silently count another file's pass")
        suite.addTests(discovered)
    result = unittest.TextTestRunner(verbosity=2).run(suite)
    failed = len(result.errors) + len(result.failures) + len(result.unexpectedSuccesses)
    skipped = len(result.skipped) + len(result.expectedFailures)
    Path(destination).write_text(json.dumps({"passed": result.testsRun - failed - skipped,
        "failed": failed, "skipped": skipped}), encoding="utf-8")
    return 0 if result.wasSuccessful() else 1


if __name__ == "__main__":
    raise SystemExit(main())
