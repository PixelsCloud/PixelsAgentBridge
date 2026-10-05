"""No TCC changes. Native opt-in test signs only temporary test executables."""
from pathlib import Path
import os
import subprocess
import sys
import tempfile
import unittest
import macos_signing as signing


class Signing(unittest.TestCase):
    def test_invalid_identity_cannot_inject_requirements(self):
        for identifier, fingerprint in [('other.app', 'A' * 40),
                                        (signing.IDENTIFIERS['desktop'], 'A' * 39),
                                        ('vip.rgaa.pab.desktop" or true', 'A' * 40)]:
            with self.assertRaises(ValueError):
                signing.requirement(identifier, fingerprint)

    @unittest.skipUnless(sys.platform == 'darwin' and os.environ.get('PAB_TEST_LOCAL_SIGNING'),
                         'requires initialized local signing identity on macOS')
    def test_changed_build_matches_previous_requirement_and_wrong_certificate_fails(self):
        fingerprint = signing.load_identity()
        identifier = signing.IDENTIFIERS['desktop']
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            displays = []
            for version in (1, 2):
                source, binary = root / f'{version}.c', root / f'{version}'
                source.write_text(f'int main(void) {{ return {version}; }}\n')
                subprocess.run(['/usr/bin/cc', str(source), '-o', str(binary)], check=True)
                signing.sign(binary, identifier)
                displays.append(subprocess.check_output(['/usr/bin/codesign', '-d', '-r-', str(binary)], text=True))
                wrong = ('0' if fingerprint[0] != '0' else '1') + fingerprint[1:]
                with self.assertRaises(RuntimeError):
                    signing.verify(binary, identifier, wrong)
            self.assertEqual(displays[0], displays[1])
            # Altering a sealed resource must fail, even with the matching certificate.
            binary.write_bytes(binary.read_bytes() + b'tampered')
            with self.assertRaises(RuntimeError):
                signing.verify(binary, identifier, fingerprint)


if __name__ == '__main__':
    unittest.main()
