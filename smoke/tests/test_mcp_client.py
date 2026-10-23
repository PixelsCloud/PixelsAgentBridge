from unittest import TestCase
from unittest.mock import Mock

from smoke.mcp_client import McpClient


class OriginalOperationObservationTests(TestCase):
    def test_exited_command_waits_for_output_completion_without_replaying(self):
        client = McpClient.__new__(McpClient)
        original = {'operation_ref': {'device_code': '123456789', 'operation_id': 'original', 'task_id': 'task'},
                    'snapshot': {'state': 'succeeded'}, 'complete': False}
        final = {**original, 'complete': True}
        client.call = Mock(return_value=final)
        self.assertIs(client.observe(original), final)
        self.assertEqual(client.call.call_count, 1)
        self.assertEqual(client.call.call_args.args[0], 'pab_get_operation')
        self.assertEqual(client.call.call_args.args[1]['operation_id'], 'original')

    def test_document_render_observation_retains_kind_and_ref(self):
        client = McpClient.__new__(McpClient)
        original = {'operation_ref': {'device_code': '123456789', 'operation_id': 'render-original', 'kind': 'document_render_page'},
                    'result': {'state': 'running'}}
        final = {**original, 'result': {'state': 'completed'}}
        client.call = Mock(return_value=final)
        self.assertIs(client.observe(original), final)
        self.assertEqual(client.call.call_args.args[1]['kind'], 'document_render_page')
        self.assertEqual(client.call.call_args.args[1]['operation_id'], 'render-original')

    def test_observation_error_is_preserved_without_spinning_or_reissuing_mutation(self):
        client = McpClient.__new__(McpClient)
        original = {'operation_ref': {'device_code': '123456789', 'operation_id': 'original'}, 'result': {'state': 'running'}}
        failure = {'error': {'code': 'original_result_unavailable'}}
        client.call = Mock(return_value=failure)
        self.assertIs(client.observe(original), failure)
        self.assertEqual(client.call.call_count, 1)

    def test_partial_browser_effects_return_original_unconfirmed_result_without_replay(self):
        client = McpClient.__new__(McpClient)
        original = {'operation_ref': {'device_code': '123456789', 'operation_id': 'original', 'kind': 'browser_fill_form'},
                    'result': {'state': 'running'}}
        final = {**original, 'result': {'state': 'unconfirmed', 'error': 'second field timed out'}}
        client.call = Mock(return_value=final)
        self.assertIs(client.observe(original), final)
        self.assertEqual(client.call.call_count, 1)
        self.assertEqual(client.call.call_args.args[0], 'pab_get_operation')
        self.assertEqual(client.call.call_args.args[1]['operation_id'], 'original')


class CompleteOutputTests(TestCase):
    original = {'operation_ref': {'device_code': '123456789', 'task_id': 'original-task'}}

    @staticmethod
    def chunk(text, offset, end, *, complete, available):
        return {'text': text, 'offset': offset, 'next_offset': end,
                'range': {'complete': complete, 'available_to': available}, 'gap': None}

    def test_empty_and_split_chunks_are_read_to_eof_using_the_original_task(self):
        client = McpClient.__new__(McpClient)
        client.call = Mock(side_effect=[
            self.chunk('', 0, 0, complete=False, available=0),
            self.chunk('{"ok":', 0, 6, complete=False, available=11),
            self.chunk('true}', 6, 11, complete=True, available=11)])
        self.assertEqual(client.read_stdout(self.original), '{"ok":true}')
        self.assertEqual([c.args[1]['offset'] for c in client.call.call_args_list], [0, 0, 6])
        self.assertTrue(all(c.args[0] == 'pab_read_output' and c.args[1]['task_id'] == 'original-task'
                            for c in client.call.call_args_list))

    def test_complete_range_with_more_bytes_still_reads_remaining_chunks(self):
        client = McpClient.__new__(McpClient)
        client.call = Mock(side_effect=[self.chunk('ab', 0, 2, complete=True, available=3),
                                       self.chunk('c', 2, 3, complete=True, available=3)])
        self.assertEqual(client.read_stdout(self.original, max_bytes=3), 'abc')

    def test_lost_or_oversized_output_is_rejected_instead_of_parsed(self):
        client = McpClient.__new__(McpClient)
        gap = self.chunk('tail', 4, 8, complete=True, available=8)
        gap['gap'] = {'from': 0, 'to': 4}
        client.call = Mock(return_value=gap)
        with self.assertRaisesRegex(ValueError, 'incomplete or invalid'):
            client.read_stdout(self.original)
        client.call = Mock(return_value=self.chunk('four', 0, 4, complete=True, available=4))
        with self.assertRaisesRegex(ValueError, 'exceeds'):
            client.read_stdout(self.original, max_bytes=3)

    def test_incomplete_empty_output_times_out_without_replaying_the_command(self):
        from unittest.mock import patch
        client = McpClient.__new__(McpClient)
        client.call = Mock(return_value=self.chunk('', 0, 0, complete=False, available=0))
        with patch('smoke.mcp_client.time.monotonic', side_effect=[0, 0, 0, 2]):
            with self.assertRaises(TimeoutError):
                client.read_stdout(self.original, timeout=1)
        self.assertEqual(client.call.call_count, 1)
        self.assertEqual(client.call.call_args.args[0], 'pab_read_output')
