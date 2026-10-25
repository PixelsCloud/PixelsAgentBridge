import importlib.util
from pathlib import Path
import struct
import tempfile
import unittest

spec=importlib.util.spec_from_file_location('replay',Path(__file__).resolve().parents[1]/'remote_desktop_replay.py')
replay=importlib.util.module_from_spec(spec);spec.loader.exec_module(replay)


class ArchiveTests(unittest.TestCase):
    def parse(self,data):
        with tempfile.TemporaryDirectory() as root:
            path=Path(root)/'synthetic.pab';path.write_bytes(data)
            return replay.packets(path)

    def sample(self):
        return b'PABPROOF1'+struct.pack('<III',1920,1080,30)+struct.pack('<IQ',5,0)+b'\0\0\0\1e'

    def test_archive_preserves_identical_packet_bytes(self):
        geometry,frames=self.parse(self.sample())
        self.assertEqual(geometry,(1920,1080,30))
        self.assertEqual(frames,[(0,b'\0\0\0\1e')])

    def test_partial_packet_is_not_replayed_as_complete(self):
        for cut in (1,7,15,25,34,37):
            with self.assertRaises(ValueError):self.parse(self.sample()[:cut])

    def test_invalid_geometry_and_sequence_are_rejected(self):
        with self.assertRaises(ValueError):self.parse(b'PABPROOF1'+struct.pack('<III',8193,1080,30))
        with self.assertRaises(ValueError):self.parse(self.sample()+struct.pack('<IQ',1,2)+b'x')


if __name__=='__main__':unittest.main()
