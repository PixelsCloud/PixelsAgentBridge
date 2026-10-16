"""Exercise login registration with a fake launchctl; never install or launch apps."""
import configparser
from pathlib import Path
import shutil
import subprocess
import unittest

ROOT = Path(__file__).resolve().parent
GIT_BASH = Path('C:/Program Files/Git/bin/bash.exe')
BASH = str(GIT_BASH) if GIT_BASH.is_file() else shutil.which('bash')


class DesktopStartup(unittest.TestCase):
    def test_linux_login_entry_is_visible_and_enabled(self):
        entry = configparser.ConfigParser(interpolation=None)
        entry.read(ROOT / 'linux-desktop/pixels-agent-bridge-autostart.desktop')
        desktop = entry['Desktop Entry']
        self.assertEqual(desktop['Exec'], '/opt/pixels-agent-bridge/run-desktop.sh')
        self.assertEqual(desktop['Type'], 'Application')
        self.assertFalse(desktop.getboolean('Terminal'))
        self.assertFalse(desktop.getboolean('Hidden', fallback=False))
        self.assertTrue(desktop.getboolean('X-GNOME-Autostart-enabled'))

    @unittest.skipUnless(BASH, 'bash required')
    def test_changed_shell_scripts_parse(self):
        for name in ['macos/install.sh', 'macos/uninstall.sh', 'linux-desktop/run-desktop.sh']:
            with self.subTest(script=name):
                result = subprocess.run([BASH, '-n'], input=(ROOT / name).read_text(encoding='utf-8'),
                                        text=True, capture_output=True)
                self.assertEqual(result.returncode, 0, result.stderr)

    @unittest.skipUnless(BASH, 'bash required')
    def test_mac_upgrade_and_uninstall_unload_desktop_for_each_session(self):
        for name in ['install.sh', 'uninstall.sh']:
            script = (ROOT / 'macos' / name).read_text(encoding='utf-8')
            start = script.index('while read -r uid; do')
            end = script.index("done < <(dscl . -list /Users UniqueID | awk '$2 >= 500 {print $2}')", start)
            block = script[start:script.index('\n', end)]
            result = subprocess.run([BASH, '-s'], input='''set -eu
stopped_domains=(); stopped_plists=()
dscl() { printf 'user1 501\nuser2 502\n'; }
launchctl() { if [[ $1 == bootout ]]; then printf '%s\n' "$2"; fi; }
''' + block, text=True, capture_output=True)
            with self.subTest(script=name):
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(result.stdout.splitlines(), [
                    f'gui/{uid}/com.pixelsagentbridge.{agent}'
                    for uid in [501, 502] for agent in ['desktop', 'session-helper']])

    @unittest.skipUnless(BASH, 'bash required')
    def test_mac_fresh_and_upgrade_startup_registration_is_idempotent(self):
        script = (ROOT / 'macos/install.sh').read_text(encoding='utf-8')
        block = script.split('started_jobs+=(system/com.pixelsagentbridge.executor)\n', 1)[1].split('\ncommitted=1', 1)[0]
        for upgrade in [False, True]:
            setup = 'stopped_domains=(); stopped_plists=()'
            if upgrade:
                setup = '''stopped_domains=(gui/501 gui/501)
stopped_plists=(/Library/LaunchAgents/com.pixelsagentbridge.session-helper.plist /Library/LaunchAgents/com.pixelsagentbridge.desktop.plist)'''
            result = subprocess.run([BASH, '-s'], input='''set -eu
desktop_uid=501; started_jobs=(); loaded='|'
launchctl() {
    if [[ $1 == print ]]; then
        [[ $2 == gui/501 || $loaded == *"|$2|"* ]]
    elif [[ $1 == bootstrap ]]; then
        job=$(basename "$3" .plist)
        [[ $loaded != *"|$2/$job|"* ]] || return 42
        loaded="$loaded$2/$job|"
        printf '%s\n' "$2/$job"
    else return 43; fi
}
''' + setup + '\n' + block + '\nstopped_domains=(); stopped_plists=()\n' + block,
                text=True, capture_output=True)
            with self.subTest(upgrade=upgrade):
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(result.stdout.splitlines(), [
                    'gui/501/com.pixelsagentbridge.session-helper', 'gui/501/com.pixelsagentbridge.desktop'])


if __name__ == '__main__':
    unittest.main()
