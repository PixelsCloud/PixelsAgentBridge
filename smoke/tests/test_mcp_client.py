from unittest import TestCase
from unittest.mock import Mock

from smoke.mcp_client import McpClient


class OriginalOperationObservationTests(TestCase):
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
