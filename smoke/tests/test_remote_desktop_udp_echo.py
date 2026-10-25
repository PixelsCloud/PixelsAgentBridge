import time
import unittest
from unittest.mock import Mock
from smoke.remote_desktop_udp_echo import private_ipv4, reply, send_reply


class EchoTests(unittest.TestCase):
    def packet(self):
        return b'PABRDNET' + (42).to_bytes(8, 'little') + bytes(range(16)) + bytes(32)

    def test_echo_preserves_token_and_sequence_without_amplifying(self):
        data = self.packet()
        result = reply(data, '10.23.0.10', '10.23.0.10', time.perf_counter_ns())
        self.assertEqual(len(result), 64)
        self.assertEqual(result[:32], data[:32])
        self.assertLess(int.from_bytes(result[32:40], 'little'), 100000)
        self.assertEqual(result[40:], bytes(24))

    def test_unexpected_peer_size_magic_and_payload_are_silent(self):
        for data, peer in [(self.packet(), '10.23.0.11'), (self.packet()+b'x', '10.23.0.10'),
                           (b'bad', '10.23.0.10'), (bytes(64), '10.23.0.10'),
                           (self.packet()[:-1]+b'x', '10.23.0.10')]:
            self.assertIsNone(reply(data, peer, '10.23.0.10', time.perf_counter_ns()))

    def test_public_unspecified_and_multicast_addresses_are_rejected(self):
        for value in ['0.0.0.0', '8.8.8.8', '224.0.0.1', '::1', 'localhost', '169.254.1.1']:
            with self.assertRaises(ValueError):
                private_ipv4(value)
        self.assertEqual(private_ipv4('10.23.0.20'), '10.23.0.20')

    def test_transient_local_send_failure_is_recorded_without_retry_or_service_exit(self):
        import errno
        sock, record = Mock(), Mock()
        error = OSError(errno.ENOBUFS, 'synthetic buffer exhaustion')
        sock.sendto.side_effect = error
        self.assertFalse(send_reply(sock, self.packet(), ('10.23.0.10', 42), record))
        sock.sendto.assert_called_once()
        record.assert_called_once_with('send', error, 42)
        sock.sendto.side_effect = None
        self.assertTrue(send_reply(sock, self.packet(), ('10.23.0.10', 42), record))

    def test_unexpected_socket_error_is_recorded_and_remains_fatal(self):
        import errno
        sock, record = Mock(), Mock()
        error = OSError(errno.EBADF, 'closed socket')
        sock.sendto.side_effect = error
        with self.assertRaises(OSError):
            send_reply(sock, self.packet(), ('10.23.0.10', 42), record)
        record.assert_called_once_with('send', error, 42)
