"""Exercise real process retirement and file rollback only in temporary folders."""
from pathlib import Path
import os
import shutil
import subprocess
import sys
import tempfile
import threading
import time
import unittest

SCRIPTS = Path(__file__).resolve().parent / 'macos'


@unittest.skipUnless(sys.platform == 'darwin', 'macOS process path behavior')
class Upgrade(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.fixture_dir = tempfile.TemporaryDirectory(prefix='pab-upgrade-fixture-')
        cls.fixture = Path(cls.fixture_dir.name) / 'sleeper'
        # Copied Apple platform binaries can be killed by AMFI outside /bin.
        # Use an ordinary locally linked executable, like the installed product.
        subprocess.run(['/usr/bin/cc', '-x', 'c', '-', '-o', str(cls.fixture)],
                       input='#include <unistd.h>\nint main(void) { sleep(30); return 0; }\n',
                       text=True, check=True, capture_output=True)

    @classmethod
    def tearDownClass(cls):
        cls.fixture_dir.cleanup()

    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='pab-upgrade-')
        self.root = Path(self.temp.name).resolve()
        self.app = self.root / 'Installed.app'
        self.support = self.root / 'support'
        self.stage = self.root / 'stage'
        self.children = []
        for folder in (self.app / 'Contents/MacOS', self.support,
                       self.stage / 'new.app/Contents/MacOS', self.stage / 'new-support'):
            folder.mkdir(parents=True)
        for target in (self.app / 'Contents/MacOS/pab-desktop', self.support / 'pab-mcp', self.support / 'pab-executor'):
            shutil.copy(self.fixture, target)
        (self.stage / 'new.app/marker').write_text('new app')
        (self.stage / 'new-support/pab-mcp').write_text('new mcp')
        (self.support / 'settings.env').write_text('old settings')
        (self.root / 'user-data').write_text('preserved')

    def tearDown(self):
        for child in self.children:
            if child.poll() is None:
                child.terminate()
            child.wait(timeout=5)
        self.temp.cleanup()

    def run_lifecycle(self, body):
        return subprocess.run(['/bin/bash', '-c', '''
set -eu
source "$1"
app=$2; install_dir=$3; upgrade=$4
published_app=0; published_support=0
''' + body, 'test', str(SCRIPTS / 'lifecycle.sh'), str(self.app), str(self.support), str(self.stage)],
            capture_output=True, text=True, timeout=30)

    def child(self, executable):
        child = subprocess.Popen([str(executable), '30'])
        self.children.append(child)
        # Reap children while the installer checks whether their PIDs disappeared.
        threading.Thread(target=child.wait, daemon=True).start()
        time.sleep(0.1)
        self.assertIsNone(child.poll(), 'test executable must be alive before upgrading')
        return child

    def test_running_app_and_mcp_are_replaced_without_killing_host(self):
        desktop = self.child(self.app / 'Contents/MacOS/pab-desktop')
        mcp = self.child(self.support / 'pab-mcp')
        unrelated = self.child('/bin/sleep')
        result = self.run_lifecycle('pab_publish_upgrade')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIsNotNone(desktop.poll())
        self.assertIsNotNone(mcp.poll())
        self.assertIsNone(unrelated.poll())
        self.assertEqual((self.app / 'marker').read_text(), 'new app')
        self.assertEqual((self.support / 'pab-mcp').read_text(), 'new mcp')
        self.assertEqual((self.root / 'user-data').read_text(), 'preserved')

    def test_respawning_client_cannot_restart_old_binary(self):
        stop = threading.Event()
        started = threading.Event()
        children = []
        def host():
            while not stop.is_set():
                try:
                    child = subprocess.Popen([str(self.support / 'pab-mcp'), '30'], stderr=subprocess.DEVNULL)
                    children.append(child)
                    started.set()
                    child.wait()
                except OSError:
                    stop.wait(0.01)
        thread = threading.Thread(target=host, daemon=True)
        thread.start()
        try:
            self.assertTrue(started.wait(3))
            time.sleep(0.1)
            self.assertIsNone(children[0].poll())
            result = self.run_lifecycle('pab_publish_upgrade')
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertTrue(all(child.poll() is not None for child in children))
        finally:
            stop.set()
            for child in children:
                if child.poll() is None:
                    child.terminate()
            thread.join(5)

    def test_publication_failure_restores_previous_files(self):
        shutil.rmtree(self.stage / 'new-support')
        result = self.run_lifecycle('''
if pab_publish_upgrade; then exit 99; fi
pab_restore_upgrade
''')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual((self.support / 'settings.env').read_text(), 'old settings')
        self.assertEqual((self.support / 'pab-mcp').read_bytes(), self.fixture.read_bytes())
        self.assertFalse((self.app / 'marker').exists())
        self.assertEqual((self.root / 'user-data').read_text(), 'preserved')

    def test_staging_failure_does_not_stop_existing_app(self):
        desktop = self.child(self.app / 'Contents/MacOS/pab-desktop')
        result = self.run_lifecycle('pab_restore_upgrade')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIsNone(desktop.poll())


if __name__ == '__main__':
    unittest.main()
