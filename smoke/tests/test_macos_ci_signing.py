"""Check imported signing identities without touching a developer keychain."""
import base64
from datetime import datetime, timedelta, timezone
import hashlib
import os
from pathlib import Path
import sys
from tempfile import TemporaryDirectory
import unittest
from unittest.mock import patch

from cryptography import x509
from cryptography.hazmat.primitives import hashes, serialization
from cryptography.hazmat.primitives.asymmetric import rsa
from cryptography.hazmat.primitives.serialization import pkcs12
from cryptography.x509.oid import NameOID

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / 'scripts'))
import macos_signing as signing


class CiSigningTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        key = rsa.generate_private_key(public_exponent=65537, key_size=2048)
        name = x509.Name([x509.NameAttribute(NameOID.COMMON_NAME, 'Disposable signing test')])
        now = datetime.now(timezone.utc)
        cert = (x509.CertificateBuilder().subject_name(name).issuer_name(name)
                .public_key(key.public_key()).serial_number(x509.random_serial_number())
                .not_valid_before(now - timedelta(minutes=1)).not_valid_after(now + timedelta(days=1))
                .sign(key, hashes.SHA256()))
        cls.pin = hashlib.sha1(cert.public_bytes(serialization.Encoding.DER)).hexdigest().upper()
        data = pkcs12.serialize_key_and_certificates(b'fixture', key, cert, None,
            serialization.BestAvailableEncryption(b'fixture-password'))
        cls.env = dict(GITHUB_ACTIONS='true', RUNNER_ENVIRONMENT='github-hosted',
            PAB_MACOS_SIGNING_P12_BASE64=base64.b64encode(data).decode(),
            PAB_MACOS_SIGNING_P12_PASSWORD='fixture-password', PAB_MACOS_CERT_SHA1=cls.pin)

    def test_import_refuses_developer_and_self_hosted_keychains(self):
        with TemporaryDirectory() as temporary, patch.object(signing.sys, 'platform', 'darwin'):
            state = Path(temporary) / 'state'
            for host in ('', 'self-hosted'):
                with patch.dict(os.environ, dict(self.env, RUNNER_ENVIRONMENT=host), clear=True), \
                     patch.object(signing, 'run') as run, self.assertRaisesRegex(RuntimeError, 'Actions runner'):
                    signing.import_ci_identity(state)
                run.assert_not_called()
                self.assertFalse(state.exists())

    def test_wrong_certificate_is_rejected_before_creating_a_keychain(self):
        with TemporaryDirectory() as temporary, patch.object(signing.sys, 'platform', 'darwin'), \
             patch.dict(os.environ, dict(self.env, PAB_MACOS_CERT_SHA1='0' * 40), clear=True), \
             patch.object(signing, 'run') as run:
            state = Path(temporary) / 'state'
            with self.assertRaisesRegex(RuntimeError, 'pinned certificate'):
                signing.import_ci_identity(state)
            run.assert_not_called()
            self.assertFalse(state.exists())

    def test_import_preserves_search_list_and_proves_private_key_is_usable(self):
        with TemporaryDirectory() as temporary:
            state = Path(temporary) / 'state'
            old = '/Users/runner/Library/Keychains/login.keychain-db'
            commands = []

            def run(args):
                commands.append(args)
                if args[1] == 'create-keychain':
                    Path(args[-1]).write_bytes(b'fixture keychain')
                if args[1:] == ['list-keychains', '-d', 'user']:
                    return ('    "' + old + '"\n').encode()
                return b''

            with patch.object(signing.sys, 'platform', 'darwin'), \
                 patch.dict(os.environ, self.env, clear=True), patch.object(signing, 'run', side_effect=run), \
                 patch.object(signing, 'load_identity', return_value=self.pin), \
                 patch.object(signing.shutil, 'copyfile', side_effect=lambda _, target: target.write_bytes(b'probe')), \
                 patch.object(signing, 'sign') as sign:
                signing.import_ci_identity(state)
            self.assertIn(['/usr/bin/security', 'list-keychains', '-d', 'user', '-s', old,
                           state / 'signing.keychain-db'], commands)
            sign.assert_called_once()
            self.assertEqual(sign.call_args.args[1:], (signing.IDENTIFIERS['executor'], state))


if __name__ == '__main__':
    unittest.main()
