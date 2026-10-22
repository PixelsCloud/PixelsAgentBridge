"""Cross builds must never package or execute a host-architecture Node binary."""
import importlib.util
from pathlib import Path
import struct
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location('browser_runtime_build', Path(__file__).with_name('build.py'))
runtime = importlib.util.module_from_spec(spec)
spec.loader.exec_module(runtime)


class RuntimeArchitecture(unittest.TestCase):
    def test_cross_build_checks_target_without_executing_it(self):
        with tempfile.TemporaryDirectory() as folder:
            node = Path(folder) / 'node'
            node.write_bytes(struct.pack('<II', 0xFEEDFACF, 0x01000007))
            with patch.object(runtime, 'host_architecture', return_value='arm64'), patch.object(runtime.subprocess, 'check_output') as execute:
                self.assertEqual(runtime.checked_node_version(node, 'macos', 'x86_64'), runtime.VERSION)
                execute.assert_not_called()
                with self.assertRaises(ValueError):
                    runtime.checked_node_version(node, 'macos', 'arm64')

    def test_native_node_version_and_malformed_headers_are_rejected(self):
        with tempfile.TemporaryDirectory() as folder:
            node = Path(folder) / 'node'
            with patch.object(runtime, 'host_architecture', return_value='arm64'), patch.object(runtime.subprocess, 'check_output', return_value='v0.0.0\n'):
                for header in (b'', b'not-node', struct.pack('<II', 0xFEEDFACF, 0x0100000C)):
                    node.write_bytes(header)
                    with self.subTest(header=header), self.assertRaises(ValueError):
                        runtime.checked_node_version(node, 'macos', 'arm64')


if __name__ == '__main__':
    unittest.main()
