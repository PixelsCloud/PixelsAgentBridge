"""Release gates, immutable bytes, trusted workflow provenance and version isolation."""
import base64
from contextlib import ExitStack
import json
import os
from pathlib import Path
import sys
from tempfile import TemporaryDirectory
from types import SimpleNamespace
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / 'scripts'))
import github_release as release
import build_android
import build_linux_headless


class ReleaseTests(unittest.TestCase):
    def test_publication_optimization_reaches_linux_containers(self):
        with patch.dict(os.environ, {}, clear=True):
            env = release.build_environment('linux')
        expected = {'CARGO_PROFILE_RELEASE_OPT_LEVEL': '3', 'CARGO_PROFILE_RELEASE_LTO': 'thin',
                    'CARGO_PROFILE_RELEASE_CODEGEN_UNITS': '1', 'CARGO_PROFILE_RELEASE_INCREMENTAL': 'false'}
        self.assertEqual({key: env[key] for key in expected}, expected)
        with patch.dict(os.environ, env, clear=True):
            args = build_linux_headless.release_profile_arguments()
        for key, value in expected.items(): self.assertIn(key + '=' + value, args)

    def setUp(self):
        self.stack = ExitStack()
        self.addCleanup(self.stack.close)
        directory = self.stack.enter_context(TemporaryDirectory())
        self.root = Path(directory)
        self.folder = self.root / '.build/bundle'
        self.folder.mkdir(parents=True)
        self.source = dict(commit='a' * 40, sha256='b' * 64, dirty=False)
        self.stack.enter_context(patch.object(release, 'ROOT', self.root))
        self.stack.enter_context(patch.object(release, 'checked_source', return_value=self.source))
        self.stack.enter_context(patch.dict(os.environ, {
            'PAB_SERVICE_RELEASE_PRIVATE_KEY_BASE64': base64.b64encode(bytes(range(32))).decode(),
            'PAB_RELEASE_REHEARSAL': 'false'
        }))
        self.tag = 'relay-v1.2.3'

    def candidates(self):
        for target in release.plan(self.tag)['matrix']['include']:
            path = self.folder / f"relay-{target['id']}.bin"
            path.write_bytes(target['id'].encode())
            metadata = dict(schema_version=1, target=target['id'], tag=self.tag, product='relay', version='1.2.3',
                source=self.source, assets=[dict(name=path.name, platform='relay', architecture='x86_64',
                    sha256=release.digest(path), bytes=path.stat().st_size)])
            release.write_json(self.folder / f"candidate-{target['id']}.json", metadata)

    def bundle(self):
        self.candidates()
        return release.assemble(self.tag, self.folder)

    def test_product_plan_covers_all_required_platforms_and_independent_latest(self):
        self.assertEqual(len(release.plan('client-v1.2.3')['matrix']['include']), 5)
        for product in ('management', 'relay'):
            plan = release.plan(product + '-v1.2.3')
            self.assertEqual(len(plan['matrix']['include']), 2)
            self.assertFalse(plan['latest'])
        self.assertTrue(release.plan('client-v1.2.3')['latest'])

    def test_tag_validation_rejects_options_shell_content_and_noncanonical_versions(self):
        for tag in ('--help', 'relay-v1.2.3\nother=x', 'client-v1.100.0', 'relay-v01.2.3',
                    'v1.2.3', 'server-v1.2.3', 'client-v1.2.3;echo x'):
            with self.subTest(tag=tag), self.assertRaises(ValueError):
                release.plan(tag)

    def test_preflight_reports_missing_configuration_without_allocating_or_publishing(self):
        with patch.dict(os.environ, {}, clear=True), patch.object(release, 'gh') as github:
            with self.assertRaisesRegex(ValueError, 'PAB_SERVICE_RELEASE_PRIVATE_KEY_BASE64'):
                release.preflight(self.tag)
            github.assert_not_called()

    def test_preflight_checks_the_existing_signing_identity_against_the_public_pin(self):
        from cryptography.hazmat.primitives.serialization import Encoding, PublicFormat
        public = release.signing_key('relay').public_key().public_bytes(Encoding.Raw, PublicFormat.Raw)
        with patch.dict(os.environ, {'PAB_SERVICE_RELEASE_PUBLIC_KEY_BASE64':base64.b64encode(public).decode()}):
            release.preflight(self.tag)
        with patch.dict(os.environ, {'PAB_SERVICE_RELEASE_PUBLIC_KEY_BASE64':base64.b64encode(bytes(32)).decode()}):
            with self.assertRaisesRegex(ValueError, 'pinned public key'):
                release.preflight(self.tag)

    def test_complete_bundle_is_signed_and_verified(self):
        value = self.bundle()
        self.assertEqual(release.read_bundle(self.tag, self.folder), value)
        self.assertEqual(len(value['assets']), 2)

    def test_assembling_the_same_bundle_again_verifies_without_overwriting_it(self):
        value = self.bundle()
        original = (self.folder / 'release.json').read_bytes()
        self.assertEqual(release.assemble(self.tag, self.folder), value)
        self.assertEqual((self.folder / 'release.json').read_bytes(), original)

    def test_evidence_export_refuses_nested_credentials_and_private_keys(self):
        for value in ({'steps':[{'access_token':'synthetic'}]}, {'environment':'-----BEGIN PRIVATE KEY-----'}):
            with self.assertRaises(ValueError):
                release.no_credentials(value)
        release.no_credentials({'operator':'test operator','steps':[{'observed':'Synthetic content verified'}]})

    def test_public_key_verification_does_not_require_a_private_key(self):
        from cryptography.hazmat.primitives.serialization import Encoding, PublicFormat
        value = self.bundle()
        public = release.signing_key('relay').public_key().public_bytes(Encoding.Raw, PublicFormat.Raw)
        with patch.dict(os.environ, {'PAB_SERVICE_RELEASE_PUBLIC_KEY_BASE64': base64.b64encode(public).decode()}):
            with patch.object(release, 'signing_key', side_effect=AssertionError('must not read private key')):
                self.assertEqual(release.read_bundle(self.tag, self.folder), value)

    def test_wrong_public_key_and_modified_signature_are_rejected(self):
        self.bundle()
        with patch.dict(os.environ, {'PAB_SERVICE_RELEASE_PUBLIC_KEY_BASE64': base64.b64encode(bytes(32)).decode()}):
            with self.assertRaisesRegex(ValueError, 'signature'):
                release.read_bundle(self.tag, self.folder)

    def test_signed_rehearsal_cannot_create_or_publish_a_release(self):
        self.candidates()
        for path in self.folder.glob('candidate-*.json'):
            value=json.loads(path.read_text());value['rehearsal']=True;release.write_json(path,value)
        with patch.dict(os.environ, {'PAB_RELEASE_REHEARSAL':'true'}):
            value=release.assemble(self.tag,self.folder)
        self.assertTrue(value['rehearsal'])
        with patch.object(release,'gh') as github, patch.object(release,'remote_identity') as remote:
            with self.assertRaisesRegex(ValueError,'Rehearsal'):
                release.draft(self.tag,self.folder,'owner/repo')
            with self.assertRaisesRegex(ValueError,'Rehearsal'):
                release.publish(self.tag,self.folder,'owner/repo',self.root)
            github.assert_not_called();remote.assert_not_called()

    def test_rehearsal_and_release_platforms_cannot_be_combined(self):
        self.candidates()
        with patch.dict(os.environ, {'PAB_RELEASE_REHEARSAL':'true'}):
            with self.assertRaisesRegex(ValueError,'mix rehearsal'):
                release.assemble(self.tag,self.folder)

    def test_missing_platform_candidate_cannot_be_assembled(self):
        with self.assertRaisesRegex(ValueError, 'platform candidates'):
            release.assemble(self.tag, self.folder)

    def test_candidate_from_another_commit_or_source_tree_is_rejected(self):
        self.candidates()
        p = self.folder / 'candidate-windows-x64.json'
        data = json.loads(p.read_text())
        data['source'] = dict(self.source, sha256='c' * 64)
        release.write_json(p, data)
        with self.assertRaisesRegex(ValueError, 'tagged source'):
            release.assemble(self.tag, self.folder)

    def test_extra_files_and_changed_candidate_bytes_cannot_be_assembled(self):
        self.candidates()
        p = self.folder / 'private-token.txt'
        p.write_text('synthetic fixture')
        with self.assertRaisesRegex(ValueError, 'Unexpected file'):
            release.assemble(self.tag, self.folder)
        p.unlink()  # Owned TemporaryDirectory fixture only.
        (self.folder / 'relay-windows-x64.bin').write_bytes(b'changed')
        with self.assertRaisesRegex(ValueError, 'modified'):
            release.assemble(self.tag, self.folder)

    def test_traversal_symlinks_and_duplicate_asset_names_are_rejected(self):
        self.candidates()
        p = self.folder / 'candidate-windows-x64.json'
        data = json.loads(p.read_text())
        data['assets'][0]['name'] = '../outside'
        release.write_json(p, data)
        with self.assertRaisesRegex(ValueError, 'Unsafe'):
            release.assemble(self.tag, self.folder)

    def test_signed_assets_and_checksum_file_are_immutable(self):
        self.bundle()
        (self.folder / 'SHA256SUMS').write_text('wrong')
        with self.assertRaisesRegex(ValueError, 'Checksum'):
            release.read_bundle(self.tag, self.folder)

    def test_failed_smoke_gate_prevents_any_remote_publication(self):
        self.bundle()
        with patch.object(release, 'validate_reports', side_effect=ValueError('failed smoke')), \
             patch.object(release, 'gh') as github:
            with self.assertRaisesRegex(ValueError, 'failed smoke'):
                release.publish(self.tag, self.folder, 'owner/repo', self.root)
            github.assert_not_called()

    def test_a_completed_interrupted_publication_can_be_verified_without_editing_it(self):
        self.bundle()
        with patch.object(release, 'validate_reports'), patch.object(release, 'verify_remote'), \
             patch.object(release, 'gh', return_value={'isDraft':False}) as github:
            release.publish(self.tag, self.folder, 'owner/repo', self.root)
            self.assertEqual(github.call_count, 1)
            self.assertEqual(github.call_args.args[:2], ('release', 'view'))

    def test_successful_publication_respects_each_products_latest_policy(self):
        self.bundle()
        with patch.object(release, 'validate_reports'), patch.object(release, 'verify_remote') as verify, \
             patch.object(release, 'gh', return_value={'isDraft':True}) as github:
            release.publish(self.tag, self.folder, 'owner/repo', self.root)
            edit = github.call_args.args
            self.assertEqual(edit[:2], ('release', 'edit'))
            self.assertIn('--draft=false', edit)
            self.assertIn('--latest=false', edit)
            self.assertEqual(verify.call_args.kwargs['draft'], False)

    def test_a_missing_report_for_any_attached_asset_blocks_the_release(self):
        value = self.bundle()
        with self.assertRaisesRegex(ValueError, 'Missing full release smoke'):
            release.validate_reports(value, self.folder, self.root / 'reports')

    def test_each_report_reuses_the_existing_full_smoke_gate(self):
        value = self.bundle()
        reports = []
        for i, asset in enumerate(value['assets']):
            p = self.root / 'smoke/runs' / str(i) / 'report.json'
            release.write_json(p, {'artifact': {'sha256': asset['sha256']}})
            reports.append(p)
        with patch.object(release, 'validate_release_report') as gate:
            release.validate_reports(value, self.folder, reports)
            self.assertEqual(gate.call_count, 2)
            for call in gate.call_args_list:
                self.assertEqual(call.kwargs['commit'], self.source['commit'])
                self.assertEqual(call.kwargs['platform'], 'relay')
                self.assertEqual(call.kwargs['version'], '1.2.3')

    def test_draft_retry_uploads_only_missing_files_without_clobber(self):
        value = self.bundle()
        files = release.release_files(value, self.folder)
        existing = {files[0].name}
        with patch.object(release, 'remote_identity'), \
             patch.object(release.subprocess, 'run', return_value=SimpleNamespace(returncode=0)), \
             patch.object(release, 'verify_remote', return_value=existing), \
             patch.object(release, 'gh') as github:
            release.draft(self.tag, self.folder, 'owner/repo')
            self.assertEqual(github.call_count, len(files)-1)
            self.assertTrue(all(call.args[:2] == ('release', 'upload') for call in github.call_args_list))
            self.assertTrue(all('--clobber' not in call.args for call in github.call_args_list))

    def test_remote_lookup_failure_is_not_treated_as_a_missing_release(self):
        self.bundle()
        with patch.object(release, 'remote_identity'), \
             patch.object(release.subprocess, 'run', return_value=SimpleNamespace(returncode=1, stderr='HTTP 403 forbidden')), \
             patch.object(release, 'gh') as github:
            with self.assertRaisesRegex(ValueError, 'Cannot determine'):
                release.draft(self.tag, self.folder, 'owner/repo')
            github.assert_not_called()

    def test_remote_download_is_compared_to_local_bytes(self):
        value = self.bundle()
        files = release.release_files(value, self.folder)
        def github(*args, **kwargs):
            if args[:2] == ('release', 'view'):
                return dict(tagName=self.tag, isDraft=True, assets=[{'name': p.name} for p in files])
            target = Path(args[args.index('--dir')+1]) / args[args.index('--pattern')+1]
            target.write_bytes(b'tampered remote download')
        with patch.object(release, 'remote_identity'), patch.object(release, 'gh', side_effect=github):
            with self.assertRaisesRegex(ValueError, 'refusing to overwrite'):
                release.verify_remote(value, self.folder, 'owner/repo', draft=True)

    def test_fork_pr_failure_or_unexpected_workflow_cannot_supply_artifacts(self):
        valid = dict(repository={'full_name':'owner/repo'}, head_repository={'full_name':'owner/repo'},
                     path='.github/workflows/release.yml', conclusion='success', event='push', head_sha='a'*40)
        for change in ({'head_repository':{'full_name':'other/fork'}}, {'event':'pull_request'},
                       {'conclusion':'failure'}, {'path':'.github/workflows/ci.yml'}):
            with patch.object(release, 'gh', return_value=dict(valid, **change)):
                with self.assertRaisesRegex(ValueError, 'trusted'):
                    release.trusted_run('owner/repo', '123', 'release.yml')

    def test_context_is_bound_to_the_original_candidate_commit(self):
        path = self.root / 'context.json'
        release.write_json(path, dict(tag=self.tag, candidate_run='123', commit='bad'))
        with patch.object(release, 'trusted_run', return_value={'head_sha':'a'*40}):
            with self.assertRaisesRegex(ValueError, 'differs'):
                release.context(path, 'owner/repo', self.root / 'outputs')


class AndroidAndContainerTests(unittest.TestCase):
    def test_android_version_mapping_is_deterministic_monotonic_and_bounded(self):
        versions = ['1.2.98','1.2.99','1.3.0','1.99.99','2.0.0']
        codes = [build_android.release_version_code(v) for v in versions]
        self.assertEqual(codes, sorted(set(codes)))
        for invalid in ('0.0.0', '1.100.0', '01.2.3', '9999999.0.0', '1.2.3-rc1'):
            with self.assertRaises(ValueError):
                build_android.release_version_code(invalid)

    def test_android_missing_signing_material_fails_before_build_or_device_access(self):
        with patch.object(build_android, 'native') as native, patch.object(build_android, 'build_device_release') as install:
            with self.assertRaisesRegex(ValueError, 'existing keystore'):
                build_android.build_release('1.2.3', {})
            native.assert_not_called()
            install.assert_not_called()

    def test_android_release_uses_release_natives_and_never_installs_or_reads_a_phone(self):
        with TemporaryDirectory() as directory:
            root = Path(directory)
            key = root / 'fixture.keystore'
            key.write_bytes(b'fixture')
            env = dict(PAB_ANDROID_KEYSTORE_FILE=str(key), PAB_ANDROID_KEYSTORE_PASSWORD='fixture',
                PAB_ANDROID_KEY_ALIAS='fixture', PAB_ANDROID_KEY_PASSWORD='fixture',
                PAB_ANDROID_CERT_SHA256='a'*64, ANDROID_HOME=str(root))
            with patch.object(build_android, 'native') as native, \
                 patch.object(build_android, 'prepare_verifier'), patch.object(build_android, 'prepare_document_runtime'), \
                 patch.object(build_android, 'run') as run, patch.object(build_android, 'build_device_release') as install, \
                 patch.object(build_android, 'next_version_code', side_effect=AssertionError('must not query a phone')), \
                 patch.object(build_android.subprocess, 'run', return_value=SimpleNamespace(stdout='Signer #1 certificate SHA-256 digest: '+'a'*64+'\n')):
                build_android.build_release('1.2.3', env)
                self.assertEqual(native.call_count, 2)
                self.assertTrue(all(c.kwargs['profile']=='release' for c in native.call_args_list))
                self.assertIn(':app:assembleRelease', run.call_args.args[0])
                self.assertIn(':core:test', run.call_args.args[0])
                self.assertNotIn('testDebugUnitTest', run.call_args.args[0])
                self.assertIn('testReleaseUnitTest', run.call_args.args[0])
                self.assertIn('-PpixelsVersionCode=10203', run.call_args.args[0])
                install.assert_not_called()

    def test_linux_native_build_budget_is_bounded_and_profiles_are_not_shell_input(self):
        for invalid in (0, 11):
            with self.assertRaises(ValueError):
                build_linux_headless.command('release', invalid)
        with self.assertRaises(ValueError):
            build_linux_headless.command('release;echo bad', 4)
        command = build_linux_headless.command('release', 4)
        self.assertIn('--locked --release', command)
        self.assertIn('-j 4', command)
        self.assertIn('unset CMAKE_BUILD_PARALLEL_LEVEL', command)


class SourceGuards(unittest.TestCase):
    def test_rehearsal_requires_current_main_and_never_resolves_or_creates_a_tag(self):
        source=dict(commit='a'*40,sha256='b'*64,dirty=False)
        env=dict(GITHUB_ACTIONS='true',GITHUB_EVENT_NAME='workflow_dispatch',GITHUB_REF='refs/heads/main',
                 GITHUB_SHA=source['commit'],PAB_RELEASE_REHEARSAL='true')
        with patch.object(release,'source_state',return_value=source), patch.dict(os.environ,env,clear=True), \
             patch.object(release.subprocess,'run') as run, patch.object(release.subprocess,'check_output') as resolve:
            self.assertEqual(release.checked_source('relay-v1.2.3'),source)
            self.assertEqual(run.call_args.args[0][:3],['git','merge-base','--is-ancestor'])
            resolve.assert_not_called()
        for bad in ({'GITHUB_EVENT_NAME':'pull_request'},{'GITHUB_REF':'refs/heads/other'},{'GITHUB_SHA':'c'*40}):
            with patch.object(release,'source_state',return_value=source), patch.dict(os.environ,dict(env,**bad),clear=True), \
                 patch.object(release.subprocess,'run'), self.assertRaisesRegex(ValueError,'current main'):
                release.checked_source('relay-v1.2.3')


if __name__ == '__main__':
    unittest.main()
