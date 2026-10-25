from pathlib import Path
import struct
import tempfile
import unittest
from smoke.remote_desktop_packets import analyze


class PacketTests(unittest.TestCase):
    def packet(self, source, receiver, sequence=7, token=b'A'*16):
        data = bytearray(42)
        data[12:14] = b'\x08\x00'
        data[14], data[23] = 0x45, 17
        data[26:30], data[30:34] = bytes(map(int, source.split('.'))), bytes(map(int, receiver.split('.')))
        return bytes(data)+b'PABRDNET'+struct.pack('<Q', sequence)+token+bytes(32)

    def text(self, day, events, lost=0):
        rows = [f'Event: Header, EventsLost: {lost}, BuffersLost: 0']
        for second, fraction, direction, component, data in events:
            rows.append(f'[00]::2026-10-{day} 12:00:{second:02}.{fraction:09} [Microsoft-Windows-PktMon] PktGroupId 1, Direction {direction} , Component {component}, Edge 1')
            rows.append('  0x0000:  '+data.hex(' '))
        return '\n'.join(rows)

    def result(self, source, receiver, **kwargs):
        with tempfile.TemporaryDirectory() as root:
            s, r = Path(root)/'source.txt', Path(root)/'receiver.txt'
            s.write_text(source, encoding='utf-16')
            r.write_text(receiver, encoding='utf-8-sig')
            return analyze(s, r, '10.0.0.1', '10.0.0.2', 1, 2, **kwargs)

    def captures(self, token=b'A'*16, lost=0):
        request = self.packet('10.0.0.2', '10.0.0.1')
        response = self.packet('10.0.0.1', '10.0.0.2', token=token)
        source = self.text(20, [(30, 100000000, 'Rx', 1, request), (30, 102000000, 'Tx', 1, response)], lost)
        receiver = self.text(25, [(1, 0, 'Tx', 2, request), (1, 5000000, 'Rx', 2, response)])
        return source, receiver

    def test_compares_local_intervals_with_different_peer_dates_and_clocks(self):
        result = self.result(*self.captures())
        self.assertTrue(result['etl_events_complete'])
        self.assertTrue(result['paired_observations_complete'])
        row = result['probes'][0]
        self.assertEqual(row['source_nic_turnaround_ms'], 2)
        self.assertEqual(row['receiver_nic_roundtrip_ms'], 5)
        self.assertEqual(row['unattributed_duration_ms'], 3)

    def test_same_sequence_from_different_sessions_is_not_paired(self):
        result = self.result(*self.captures(token=b'B'*16))
        self.assertEqual(result['paired_probes'], 0)
        self.assertFalse(result['paired_observations_complete'])

    def test_lost_capture_events_prevent_complete_evidence(self):
        result = self.result(*self.captures(lost=1))
        self.assertEqual(result['paired_probes'], 1)
        self.assertFalse(result['etl_events_complete'])

    def test_wrong_nic_direction_does_not_create_a_roundtrip(self):
        source, receiver = self.captures()
        receiver = receiver.replace('Direction Tx', 'Direction Rx')
        result = self.result(source, receiver)
        self.assertFalse(result['paired_observations_complete'])
        self.assertEqual(result['missing_observations']['receiver_request'], 1)

    def test_no_etw_loss_does_not_imply_all_probe_observations_are_present(self):
        source, receiver = self.captures()
        extra = self.text(25, [(2, 0, 'Tx', 2, self.packet('10.0.0.2', '10.0.0.1', sequence=8))])
        result = self.result(source, receiver+'\n'+extra)
        self.assertTrue(result['etl_events_complete'])
        self.assertFalse(result['paired_observations_complete'])
        self.assertEqual(result['paired_probes'], 1)
