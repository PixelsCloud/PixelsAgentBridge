"""Isolated macOS Keychain fixture for native user Git acceptance.

Uses Git v2.44.0's credential-osxkeychain helper, compiled with an explicit test
keychain and system interaction disabled. No product helper/config is replaced.
Supply the upstream C source and its COPYING in --upstream (SHA-256 checked).
The serve command prints readiness, waits for root/stop, and cleans its resources.
"""
import argparse
import base64
import ctypes as C
import hashlib
import http.server
import json
import os
from pathlib import Path
import secrets
import shlex
import shutil
import subprocess
import sys
import threading
import time
from urllib.parse import unquote, urlsplit


class Keychain:
    def __init__(self, root, create=False):
        root = root.resolve(strict=True)
        assert root.parent == Path.home().resolve() and root.name.startswith("pab-keychain-acceptance-")
        assert root.stat().st_uid == os.getuid()
        self.path = root / "fixture.keychain"
        assert self.path.resolve().parent == root and not self.path.is_symlink()
        self.security = C.CDLL("/System/Library/Frameworks/Security.framework/Security")
        self.cf = C.CDLL("/System/Library/Frameworks/CoreFoundation.framework/CoreFoundation")
        self.cf.CFRelease.argtypes = [C.c_void_p]
        self.cf.CFRelease.restype = None
        self.ref = C.c_void_p()
        assert self.bind("SecKeychainSetUserInteractionAllowed", [C.c_ubyte])(0) == 0
        if create:
            password = secrets.token_hex(24).encode()
            with (root / "unlock.bin").open("xb") as file:
                os.chmod(file.name, 0o600)
                file.write(password)
            status = self.bind("SecKeychainCreate", [C.c_char_p, C.c_uint32, C.c_char_p,
                C.c_ubyte, C.c_void_p, C.POINTER(C.c_void_p)])(
                    os.fsencode(self.path), len(password), password, 0, None, C.byref(self.ref))
        else:
            status = self.bind("SecKeychainOpen", [C.c_char_p, C.POINTER(C.c_void_p)])(os.fsencode(self.path), C.byref(self.ref))
        assert status == 0, {"keychain_open_or_create": status}

    def bind(self, name, args):
        fn = getattr(self.security, name)
        fn.argtypes, fn.restype = args, C.c_int32
        return fn

    def control(self, action):
        if action == "unlock":
            password = (self.path.parent / "unlock.bin").read_bytes()
            status = self.bind("SecKeychainUnlock", [C.c_void_p, C.c_uint32, C.c_char_p, C.c_ubyte])(
                self.ref, len(password), password, 1)
        else:
            status = self.bind("SecKeychainLock" if action == "lock" else "SecKeychainDelete", [C.c_void_p])(self.ref)
        assert status == 0, {action: status}
        if action != "delete":
            flags = C.c_uint32()
            status = self.bind("SecKeychainGetStatus", [C.c_void_p, C.POINTER(C.c_uint32)])(self.ref, C.byref(flags))
            assert status == 0 and bool(flags.value & 1) == (action == "unlock")

    def close(self):
        self.cf.CFRelease(self.ref)


def preferences():
    return [subprocess.check_output(["/usr/bin/security", *args], stderr=subprocess.STDOUT, timeout=10)
            for args in (["list-keychains", "-d", "user"], ["default-keychain", "-d", "user"])]


def serve(args):
    root = Path(args.root)
    assert root.parent.resolve() == Path.home().resolve() and root.name.startswith("pab-keychain-acceptance-")
    root.mkdir(mode=0o700)
    before = preferences()
    keychain = server = thread = None
    authenticated = 0
    try:
        upstream = Path(args.upstream)
        source = (upstream / "git-credential-osxkeychain.c").read_bytes()
        license_text = (upstream / "COPYING").read_bytes()
        assert hashlib.sha256(source).hexdigest() == "60be5c75ad00aa0f554cd302c8b83127ffdcd59426074080c3843d6ecd781029"
        assert hashlib.sha256(license_text).hexdigest() == "5b2198d1645f767585e8a88ac0499b04472164c0d2da22e75ecf97ef443ab32e"
        text = source.decode()
        marker = "NULL, /* default keychain */"
        assert text.count(marker) == 1
        (root / "upstream.c").write_text(text.replace(marker, "fixture_keychain,"), encoding="utf-8")
        (root / "COPYING").write_bytes(license_text)
        (root / "wrapper.c").write_text('''#include <Security/Security.h>
#include <stdio.h>
static SecKeychainRef fixture_keychain;
#define main upstream_main
#include "upstream.c"
#undef main
int main(int argc, const char **argv) {
    if (argc != 3) return 2;
    OSStatus status = SecKeychainSetUserInteractionAllowed(false);
    if (!status) status = SecKeychainOpen(argv[1], &fixture_keychain);
    if (status) { fprintf(stderr, "fixture Keychain status %d\\n", (int)status); return 1; }
    const char *args[] = {argv[0], argv[2], NULL};
    int result = upstream_main(2, args);
    CFRelease(fixture_keychain);
    return result;
}
''', encoding="utf-8")
        helper = root / "credential-helper"
        subprocess.run(["/usr/bin/clang", "-Wno-deprecated-declarations", str(root / "wrapper.c"),
                        "-framework", "Security", "-framework", "CoreFoundation", "-o", str(helper)], check=True, timeout=60)
        keychain = Keychain(root, create=True)
        assert preferences() == before, "private keychain changed real preferences"
        seed, remote = root / "seed", root / "remote.git"
        def git(*argv):
            return subprocess.check_output(["/usr/bin/git", *map(str, argv)], stderr=subprocess.PIPE, text=True, timeout=30).strip()
        git("init", "-b", "main", seed)
        git("-C", seed, "config", "user.name", "PAB Keychain Fixture")
        git("-C", seed, "config", "user.email", "fixture@example.invalid")
        git("-C", seed, "config", "commit.gpgsign", "false")
        (seed / "fixture.txt").write_text("Keychain 中文 fixture\n", encoding="utf-8")
        git("-C", seed, "add", "fixture.txt")
        git("-C", seed, "commit", "-m", "fixture")
        oid = git("-C", seed, "rev-parse", "HEAD")
        git("clone", "--bare", seed, remote)
        git("--git-dir", remote, "update-server-info")
        secret = "PAB_FAKE_KEYCHAIN_SECRET_" + secrets.token_hex(24)
        authorization = "Basic " + base64.b64encode(("fixture:" + secret).encode()).decode()
        class Handler(http.server.SimpleHTTPRequestHandler):
            def __init__(self, *a, **kw):
                super().__init__(*a, directory=str(remote), **kw)
            def log_message(self, *a):
                pass
            def authorize(self):
                nonlocal authenticated
                if self.headers.get("Authorization") != authorization:
                    self.send_response(401)
                    self.send_header("WWW-Authenticate", 'Basic realm="PAB isolated Keychain"')
                    self.send_header("Content-Length", "0")
                    self.end_headers()
                    return False
                # Never serve the private keychain, password file or helper sources.
                decoded = unquote(urlsplit(self.path).path)
                candidate = (remote / decoded.removeprefix("/remote.git/")).resolve()
                if not decoded.startswith("/remote.git/") or remote.resolve() not in candidate.parents:
                    self.send_error(404)
                    return False
                authenticated += 1
                self.path = self.path.removeprefix("/remote.git")
                return True
            def do_GET(self):
                if self.authorize():
                    super().do_GET()
            def do_HEAD(self):
                if self.authorize():
                    super().do_HEAD()
        server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        port = server.server_port
        credential = f"protocol=http\nhost=127.0.0.1:{port}\nusername=fixture\npassword={secret}\n\n"
        subprocess.run([str(helper), str(keychain.path), "store"], input=credential, text=True, check=True, timeout=10)
        request = f"protocol=http\nhost=127.0.0.1:{port}\nusername=fixture\n\n"
        observed = subprocess.check_output([str(helper), str(keychain.path), "get"], input=request, text=True, timeout=10)
        assert "password=" + secret in observed, "independent helper could not read the fixture credential"
        config = {"root": str(root), "repo": str(root / "client"), "url": f"http://127.0.0.1:{port}/remote.git",
                  "helper": "!" + shlex.join([str(helper), str(keychain.path)]), "expected_oid": oid,
                  "python": sys.executable, "controller": str(Path(__file__).resolve())}
        (root / "config.json").write_text(json.dumps(config), encoding="utf-8")
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        print(json.dumps({"ready": True, "config": str(root / "config.json"), "uid": os.getuid()}), flush=True)
        until = time.monotonic() + 300
        while not (root / "stop").exists():
            if time.monotonic() >= until:
                raise TimeoutError("fixture controller deadline reached")
            time.sleep(0.1)
        print(json.dumps({"authenticated_http_requests": authenticated}), flush=True)
    finally:
        if thread:
            server.shutdown()
        if server:
            server.server_close()
        if thread:
            thread.join(timeout=5)
        if keychain:
            keychain.control("delete")
            keychain.close()
        assert preferences() == before, "real keychain preferences changed"
        if root.exists():
            assert root.resolve().parent == Path.home().resolve() and not root.is_symlink()
            shutil.rmtree(root)
        print(json.dumps({"fixture_removed": not root.exists(), "keychain_preferences_unchanged": True}), flush=True)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=["serve", "lock", "unlock"])
    parser.add_argument("--root", required=True)
    parser.add_argument("--upstream")
    args = parser.parse_args()
    if args.action == "serve":
        serve(args)
    else:
        keychain = Keychain(Path(args.root))
        try:
            keychain.control(args.action)
        finally:
            keychain.close()
        print(json.dumps({"action": args.action, "confirmed": True, "uid": os.getuid()}))
