"""Publication gates, signature separation and reconciliation without remote mutations."""
import base64
import hashlib
import io
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tarfile
from tempfile import TemporaryDirectory
import unittest
from unittest.mock import patch
import zipfile

from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / 'scripts'))
import release_service as release
from package_service_image import package_image


class ServiceRelease(unittest.TestCase):
    def setUp(self):
        temporary = TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.source = {'commit': 'a' * 40, 'sha256': 'b' * 64, 'dirty': False}
        for target, value in (('ROOT', self.root), ('source_state', lambda: dict(self.source))):
            mock = patch.object(release, target, value)
            mock.start(); self.addCleanup(mock.stop)
        self.artifact = self.root / 'pixels-agent-bridge-relay-windows-x86_64-release-1.2.3.zip'
        contents = {'pab-relay-server.exe': b'fixture relay', 'pab-relay-healthcheck.exe': b'fixture probe', 'LICENSE': b'fixture license'}
        with zipfile.ZipFile(self.artifact, 'w') as archive:
            for name, data in contents.items():
                archive.writestr(name, data)
        self.metadata = {'archive': self.artifact.name, 'kind': 'native', 'product': 'relay', 'platform': 'windows',
                         'architecture': 'x86_64', 'version': '1.2.3', 'source': self.source,
                         'sha256': release.digest(self.artifact),
                         'files': {name: hashlib.sha256(data).hexdigest() for name, data in contents.items()}}
        self.artifact.with_suffix('.zip.manifest.json').write_text(json.dumps(self.metadata))
        self.notes = self.root / 'notes.md'
        self.notes.write_text('Public fixture release notes')
        self.state = release.prepare('relay', self.artifact, self.notes)
        self.report = self.root / 'report.json'
        self.report.write_text('{"profile":"release"}')
        # Only an ephemeral deterministic fixture key; no deployed keys are loaded.
        self.key = self.root / 'fixture.key'
        self.key.write_bytes(bytes(range(32)))
        mock = patch.dict(os.environ, {'PAB_SERVICE_RELEASE_PRIVATE_KEY_FILE': str(self.key)})
        mock.start(); self.addCleanup(mock.stop)

    def test_wrong_product_and_modified_archive_are_rejected(self):
        with self.assertRaisesRegex(ValueError, 'identity or checksum'):
            release.read_candidate(self.artifact, 'management')
        self.artifact.write_bytes(b'other bytes')
        with self.assertRaisesRegex(ValueError, 'identity or checksum'):
            release.read_candidate(self.artifact, 'relay')

    def test_rejected_smoke_gate_makes_no_remote_or_signing_changes(self):
        with patch.object(release, 'validate_release_report', side_effect=ValueError('blocked smoke')), patch.object(release.subprocess, 'run') as remote:
            with self.assertRaisesRegex(ValueError, 'blocked smoke'):
                release.publish(self.state, self.report, 'example/project')
        remote.assert_not_called()
        self.assertFalse((self.state.parent / 'release.json').exists())
        self.assertEqual(json.loads(self.state.read_text())['phase'], 'prepared')

    def test_release_uses_product_target_and_exact_candidate_for_gate(self):
        with patch.object(release, 'validate_release_report', side_effect=ValueError('stop')) as gate:
            with self.assertRaises(ValueError):
                release.publish(self.state, self.report, 'example/project')
        self.assertEqual(gate.call_args.kwargs['platform'], 'relay')
        self.assertEqual(gate.call_args.kwargs['artifact'].parent, self.state.parent)
        self.assertEqual(gate.call_args.kwargs['commit'], self.source['commit'])

    def test_notes_change_blocks_publication(self):
        (self.state.parent / 'notes.md').write_text('changed after preparation')
        with patch.object(release, 'validate_release_report'), patch.object(release.subprocess, 'run') as remote:
            with self.assertRaisesRegex(ValueError, 'notes changed'):
                release.publish(self.state, self.report, 'example/project')
        remote.assert_not_called()

    def test_signature_is_bound_to_service_domain_and_does_not_publish_private_evidence(self):
        _, state, artifact = release.read_state(self.state)
        envelope = release.public_manifest(state, artifact, self.report)
        encoded = json.dumps(envelope['release'], sort_keys=True, separators=(',', ':'), ensure_ascii=False).encode()
        public = Ed25519PrivateKey.from_private_bytes(self.key.read_bytes()).public_key()
        public.verify(base64.b64decode(envelope['signature']), release.DOMAIN + encoded)
        with self.assertRaises(Exception):
            public.verify(base64.b64decode(envelope['signature']), b'PAB-RELEASE-V1\n' + encoded)
        self.assertEqual(set(envelope['release']['smoke']), {'sha256', 'profile'})
        self.assertNotIn('example/project', json.dumps(envelope))

    def test_uncertain_publication_is_not_replayed_and_never_becomes_client_latest(self):
        with patch.object(release, 'validate_release_report'), patch.object(release.subprocess, 'run', side_effect=subprocess.TimeoutExpired('gh', 1)) as remote:
            with self.assertRaisesRegex(RuntimeError, 'unconfirmed'):
                release.publish(self.state, self.report, 'example/project')
            with self.assertRaisesRegex(ValueError, 'do not replay'):
                release.publish(self.state, self.report, 'example/project')
        self.assertEqual(remote.call_count, 1)
        command = remote.call_args.args[0]
        self.assertIn('--latest=false', command)
        self.assertEqual(command[3], 'relay/v1.2.3-windows-x86_64')
        self.assertEqual(json.loads(self.state.read_text())['phase'], 'publishing')

    def test_download_verification_requires_trusted_key_and_exact_artifact(self):
        _, state, artifact = release.read_state(self.state)
        envelope = self.state.parent / 'release.json'
        envelope.write_text(json.dumps(release.public_manifest(state, artifact, self.report)))
        public = self.root / 'trusted-public.key'
        public.write_bytes(Ed25519PrivateKey.from_private_bytes(self.key.read_bytes()).public_key().public_bytes_raw())
        self.assertEqual(release.verify_file(artifact, envelope, public)['product'], 'relay')
        public.write_bytes(Ed25519PrivateKey.from_private_bytes(b'X' * 32).public_key().public_bytes_raw())
        with self.assertRaisesRegex(ValueError, 'signature verification failed'):
            release.verify_file(artifact, envelope, public)
        public.write_bytes(Ed25519PrivateKey.from_private_bytes(self.key.read_bytes()).public_key().public_bytes_raw())
        artifact.write_bytes(b'corrupt download')
        with self.assertRaisesRegex(ValueError, 'Downloaded artifact differs'):
            release.verify_file(artifact, envelope, public)

    def test_native_and_image_tags_do_not_collide(self):
        self.assertNotEqual(release.release_tag(self.metadata), release.release_tag(dict(self.metadata, kind='docker-image')))

    def published_attempt(self):
        _, state, artifact = release.read_state(self.state)
        envelope = self.state.parent / 'release.json'
        envelope.write_text(json.dumps(release.public_manifest(state, artifact, self.report)))
        state.update(phase='publishing', repo='example/project', release_sha256=release.digest(envelope))
        self.state.write_text(json.dumps(state))
        return state, artifact

    def test_verify_downloads_original_assets_before_confirming_success(self):
        state, artifact = self.published_attempt()
        data = {'tagName': state['tag'], 'targetCommitish': self.source['commit'], 'isDraft': False,
                'assets': [{'name': artifact.name}, {'name': 'release.json'}]}
        def download(command, **kwargs):
            destination = Path(command[command.index('--dir') + 1])
            shutil.copyfile(artifact, destination / artifact.name)
            shutil.copyfile(self.state.parent / 'release.json', destination / 'release.json')
        def output(command, **kwargs):
            return self.source['commit'].encode() if command[1] == 'api' else json.dumps(data).encode()
        with patch.object(release.subprocess, 'check_output', side_effect=output), patch.object(release.subprocess, 'run', side_effect=download):
            release.verify(self.state, 'example/project')
        self.assertEqual(json.loads(self.state.read_text())['phase'], 'published')

    def test_wrong_commit_or_draft_release_remains_unconfirmed(self):
        state, artifact = self.published_attempt()
        data = {'tagName': state['tag'], 'targetCommitish': 'wrong commit', 'isDraft': True,
                'assets': [{'name': artifact.name}, {'name': 'release.json'}]}
        with patch.object(release.subprocess, 'check_output', return_value=json.dumps(data).encode()), patch.object(release.subprocess, 'run') as download:
            with self.assertRaisesRegex(ValueError, 'Remote release identity'):
                release.verify(self.state, 'example/project')
        download.assert_not_called()
        self.assertEqual(json.loads(self.state.read_text())['phase'], 'publishing')

    def test_actual_git_tag_must_match_even_if_release_metadata_claims_the_right_commit(self):
        state, artifact = self.published_attempt()
        data = {'tagName': state['tag'], 'targetCommitish': self.source['commit'], 'isDraft': False,
                'assets': [{'name': artifact.name}, {'name': 'release.json'}]}
        with patch.object(release.subprocess, 'check_output', side_effect=[json.dumps(data).encode(), b'wrong tag commit']), patch.object(release.subprocess, 'run') as download:
            with self.assertRaisesRegex(ValueError, 'Remote Git tag'):
                release.verify(self.state, 'example/project')
        download.assert_not_called()


class ProductImage(unittest.TestCase):
    def test_image_export_is_by_immutable_id_and_release_checks_the_saved_config(self):
        with TemporaryDirectory() as directory:
            root = Path(directory)
            configuration = b'{"os":"linux","architecture":"amd64"}'
            image_id = 'sha256:' + hashlib.sha256(configuration).hexdigest()
            source = {'commit': 'a' * 40, 'sha256': 'b' * 64, 'dirty': False}
            info = {'Os': 'linux', 'Architecture': 'amd64', 'Id': image_id, 'Config': {'Labels': {
                'vip.rgaa.pab.product': 'relay', 'org.opencontainers.image.version': '1.2.3',
                'org.opencontainers.image.revision': source['commit'], 'vip.rgaa.pab.source-sha256': source['sha256']}}}
            def save(command, **kwargs):
                self.assertEqual(command[-1], image_id)
                path = command[command.index('--output') + 1]
                entries = {'manifest.json': json.dumps([{'Config': 'config.json', 'Layers': []}]).encode(), 'config.json': configuration}
                with tarfile.open(path, 'w') as archive:
                    for name, data in entries.items():
                        member = tarfile.TarInfo(name); member.size = len(data)
                        archive.addfile(member, io.BytesIO(data))
            for provenance in (source, None):
                with self.subTest(source_archive=provenance is None):
                    with patch('package_service_image.subprocess.check_output', return_value=json.dumps([info])), patch('package_service_image.subprocess.run', side_effect=save):
                        path = package_image(root, 'relay', '1.2.3', 'fixture:1.2.3', provenance)
                    metadata = release.read_candidate(path, 'relay')
                    self.assertEqual(metadata['image_id'], image_id)
                    self.assertEqual(metadata['kind'], 'docker-image')
                    self.assertEqual(metadata['source'], provenance)

    def test_wrong_product_image_cannot_be_exported(self):
        info = {'Os': 'linux', 'Architecture': 'amd64', 'Id': 'fixture', 'Config': {'Labels': {}}}
        with TemporaryDirectory() as directory, patch('package_service_image.subprocess.check_output', return_value=json.dumps([info])), patch('package_service_image.subprocess.run') as save:
            with self.assertRaisesRegex(ValueError, 'Image identity'):
                package_image(Path(directory), 'relay', '1.2.3', 'other-image', {'commit': 'a', 'sha256': 'b'})
            save.assert_not_called()


if __name__ == '__main__':
    unittest.main()
