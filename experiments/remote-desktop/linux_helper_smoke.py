"""Isolated user-session helper startup/SIGTERM check. No service credentials.

Run with a staged Desktop executable. Creates only its own temporary user data,
uses a closed loopback IPC port and server overrides, and reaps its own group.
"""
import json
import os
import pathlib
import signal
import subprocess
import sys
import tempfile
import time


def main():
    binary = pathlib.Path(sys.argv[1]).resolve(strict=True)
    role = sys.argv[2] if len(sys.argv) > 2 else "--session-helper"
    assert role in ("--session-helper", "--media-helper")
    assert binary.parent.name.startswith("pab-linux-media-") and binary.parent.parent == pathlib.Path("/tmp")
    assert os.geteuid() != 0, "Use the existing graphical user context"
    with tempfile.TemporaryDirectory(prefix="pab-linux-helper-data-") as data:
        root = pathlib.Path(data)
        (root/"operator-server.toml").write_text('control_url = "wss://127.0.0.1:9/control"\nrelay_url = "https://127.0.0.1:9"\n')
        env = {k:v for k,v in os.environ.items() if not k.startswith("PAB_")}
        env.update(PAB_DATA_DIR=data, PAB_LOCAL_IPC_PORT="9")
        with (root/"stdout.log").open("wb") as out, (root/"stderr.log").open("wb") as err:
            child = subprocess.Popen([str(binary), role], env=env,
                                     stdout=out, stderr=err, start_new_session=True)
            try:
                time.sleep(3)
                assert child.poll() is None, "Helper exited before SIGTERM"
                child.terminate()
                assert child.wait(timeout=8) == 0, "Helper did not shut down cleanly"
                assert not (root/"local-access.key").exists()
                print(json.dumps({"helper_started":True,"role":role,"alive_before_termination":True,
                    "exit_code":child.returncode,"isolated_data":True,"ipc_port":9,
                    "service_credentials_used":False,"installed_application_modified":False}))
            finally:
                if child.poll() is None:
                    try:
                        os.killpg(child.pid, signal.SIGKILL)
                    except ProcessLookupError:
                        pass
                child.wait(timeout=5)


if __name__ == "__main__":
    main()
