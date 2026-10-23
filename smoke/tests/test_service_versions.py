"""Independent counters and cross-version evidence cannot silently affect client releases."""
import json
from pathlib import Path
import re
import sys
from tempfile import TemporaryDirectory
import tomllib
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / 'scripts'))
from build_version import reserve_product_version, product_for_target
from smoke import run


class ProductVersions(unittest.TestCase):
    def test_counters_migrate_without_changing_client_state_and_advance_independently(self):
        with TemporaryDirectory() as directory:
            root = Path(directory)
            legacy = root / 'build-version.json'
            legacy.write_text('{"version":"1.3.6","build_count":88}')
            original = legacy.read_bytes()
            self.assertEqual(reserve_product_version(root, 'management'), '1.3.6')
            self.assertEqual(reserve_product_version(root, 'management'), '1.3.7')
            self.assertEqual(reserve_product_version(root, 'relay'), '1.3.6')
            self.assertEqual(reserve_product_version(root, 'legacy_server'), '1.3.6')
            self.assertEqual(legacy.read_bytes(), original)
            state = tomllib.loads((root / 'product-versions.toml').read_text())
            self.assertEqual(state['management']['build_count'], 2)
            self.assertEqual(state['relay']['build_count'], 1)
            self.assertEqual(reserve_product_version(root, 'client'), '1.3.7')
            self.assertEqual(tomllib.loads((root / 'product-versions.toml').read_text()), state)

    def test_invalid_product_state_is_preserved(self):
        with TemporaryDirectory() as directory:
            root = Path(directory)
            state = root / 'product-versions.toml'
            state.write_text('schema_version=99\n')
            with self.assertRaisesRegex(ValueError, 'Unsupported'):
                reserve_product_version(root, 'relay')
            self.assertEqual(state.read_text(), 'schema_version=99\n')

    def test_failed_build_reservation_is_not_reused(self):
        with TemporaryDirectory() as directory:
            root = Path(directory)
            (root / 'build-version.json').write_text('{"version":"1.99.99","build_count":0}')
            self.assertEqual(reserve_product_version(root, 'relay'), '1.99.99')
            self.assertEqual(reserve_product_version(root, 'relay'), '2.0.0')

    def test_targets_select_their_product_stream(self):
        for target in ('management', 'web', 'docker-management'):
            self.assertEqual(product_for_target(target), 'management')
        for target in ('relay', 'docker-relay'):
            self.assertEqual(product_for_target(target), 'relay')
        for target in ('server', 'docker'):
            self.assertEqual(product_for_target(target), 'legacy_server')
        for target in ('desktop', 'macos', 'linux', 'desktop-web'):
            self.assertEqual(product_for_target(target), 'client')
        with self.assertRaisesRegex(ValueError, 'Unknown build target'):
            product_for_target('unknown')

    def test_counter_changes_do_not_invalidate_other_products_but_source_changes_do(self):
        with TemporaryDirectory() as directory:
            root = Path(directory)
            for name in ('source.rs', 'build-version.json', 'product-versions.toml'):
                (root / name).write_text('initial')
            def git(*args):
                if args[0] == 'ls-files':
                    return b'source.rs\0build-version.json\0product-versions.toml\0'
                if args[0] == 'status':
                    return b' M build-version.json\0 M product-versions.toml\0'
                return b'abc\n'
            with patch.object(run, 'ROOT', root), patch.object(run, 'git', side_effect=git):
                before = run.source_state()
                self.assertFalse(before['dirty'])
                (root / 'product-versions.toml').write_text('changed counter')
                (root / 'build-version.json').write_text('changed client counter')
                self.assertEqual(run.source_state(), before)
                (root / 'source.rs').write_text('changed implementation')
                self.assertNotEqual(run.source_state()['sha256'], before['sha256'])


class Compatibility(unittest.TestCase):
    def observation(self):
        matrix = tomllib.loads((ROOT / 'smoke/compatibility.toml').read_text())
        artifact = {'version': 'fixture', 'sha256': 'a' * 64}
        return {'combinations': [dict(id=c['id'], outcome='pass', observed='Real isolated connection succeeded',
                                     management=dict(artifact), relay=dict(artifact), client=dict(artifact))
                                 for c in matrix['combination']]}

    def test_all_combinations_require_actual_artifact_identification(self):
        value = self.observation()
        run.validate_compatibility(value)
        value['combinations'][0]['relay'].pop('sha256')
        with self.assertRaisesRegex(ValueError, 'three tested artifacts'):
            run.validate_compatibility(value)

    def test_omitted_or_duplicated_combinations_are_rejected(self):
        value = self.observation()
        for entries in (value['combinations'][:-1], value['combinations'] + value['combinations'][:1]):
            with self.assertRaisesRegex(ValueError, 'every declared combination'):
                run.validate_compatibility({'combinations': entries})

    def test_old_service_rejection_needs_contract_evidence_and_current_services_must_work(self):
        value = self.observation()
        cell = value['combinations'][0]
        cell['outcome'] = 'declared_rejection'
        with self.assertRaisesRegex(ValueError, 'unsupported wire contract'):
            run.validate_compatibility(value)
        cell['unsupported_contract'] = 'baseline node does not support lifecycle acknowledgement'
        run.validate_compatibility(value)
        value['combinations'][-1].update(outcome='declared_rejection', unsupported_contract='not supported')
        with self.assertRaisesRegex(ValueError, 'unsupported wire contract'):
            run.validate_compatibility(value)

    def test_matrix_protocol_numbers_match_implementation(self):
        protocol = tomllib.loads((ROOT / 'smoke/compatibility.toml').read_text())['protocol']
        for key, file, name in (
            ('relay_policy', 'relay_policy.rs', 'RELAY_POLICY_SCHEMA_VERSION'),
            ('relay_directory', 'relay_cluster.rs', 'RELAY_DIRECTORY_SCHEMA_VERSION'),
            ('relay_node_control', 'relay_cluster.rs', 'RELAY_NODE_CONTROL_VERSION')):
            source = (ROOT / 'crates/protocol/src' / file).read_text()
            number = re.search(r'pub const ' + name + r': u16 = (\d+);', source)
            self.assertIsNotNone(number)
            self.assertEqual(protocol[key], int(number[1]))

    def test_relay_smoke_does_not_select_web_suites(self):
        cases = run.selected('relay', 'release')
        self.assertIn('COMPAT-01', {c['id'] for c in cases})
        self.assertIn('AUTO-14', {c['id'] for c in cases})
        self.assertFalse(any(c.get('adapter', '').startswith('web-') for c in cases))
        self.assertIn('AUTO-11', {c['id'] for c in run.selected('management', 'release')})

    def test_modified_matrix_invalidates_existing_report(self):
        with TemporaryDirectory() as directory:
            home = Path(directory)
            (home / 'cases.toml').write_text('case definitions')
            matrix = home / 'compatibility.toml'
            matrix.write_text('original matrix')
            source = {'commit': 'fixture', 'sha256': 'fixture', 'dirty': False}
            report = {'source': source, 'cases': [{'id': 'COMPAT-01'}],
                      'catalog_sha256': run.digest(home / 'cases.toml'), 'compatibility_sha256': run.digest(matrix)}
            with patch.object(run, 'HOME', home), patch.object(run, 'source_state', return_value=source):
                run.verify_binding(report)
                matrix.write_text('changed matrix')
                with self.assertRaisesRegex(ValueError, 'Compatibility matrix changed'):
                    run.verify_binding(report)


if __name__ == '__main__':
    unittest.main()
