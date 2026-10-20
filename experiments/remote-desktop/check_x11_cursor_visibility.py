"""Run the visibility regression only on an X server owned by this driver.

Arguments: native visibility test, optional two-screen cursor regression test.
No installed app, display configuration or user session is modified.
"""
import json
import os
import pathlib
import select
import subprocess
import sys
import tempfile


def main():
    results = []
    for index, executable in enumerate(sys.argv[1:]):
        executable = pathlib.Path(executable).resolve(strict=True)
        read_fd, write_fd = os.pipe()
        with tempfile.TemporaryDirectory(prefix="pab-x11-visibility-") as temporary:
            environment = os.environ.copy()
            environment.pop("WAYLAND_DISPLAY", None)
            environment.update(XDG_SESSION_TYPE="x11", XDG_RUNTIME_DIR=temporary)
            command = ["Xvfb", "-displayfd", str(write_fd), "-nolisten", "tcp",
                       "-screen", "0", "1024x768x24"]
            if index:
                command.extend(["-screen", "1", "800x600x24"])
            with open(pathlib.Path(temporary) / "server.log", "wb") as log:
                server = subprocess.Popen(command, pass_fds=(write_fd,), stdout=log, stderr=log)
                os.close(write_fd)
                try:
                    assert select.select([read_fd], [], [], 10)[0], "Xvfb did not start"
                    display = os.read(read_fd, 32).decode().strip()
                    assert display.isdigit()
                    environment["DISPLAY"] = ":" + display
                    result = subprocess.run([str(executable), "--isolated-display"],
                                            env=environment, capture_output=True, text=True, timeout=30)
                    assert result.returncode == 0, result.stderr
                    results.append(dict(test=executable.name, result=json.loads(result.stdout),
                                        stderr=result.stderr.strip()))
                finally:
                    os.close(read_fd)
                    server.terminate()
                    try:
                        server.wait(timeout=5)
                    except subprocess.TimeoutExpired:
                        server.kill()
                        server.wait(timeout=5)
    assert results
    print(json.dumps(results, indent=2))


if __name__ == "__main__":
    main()
