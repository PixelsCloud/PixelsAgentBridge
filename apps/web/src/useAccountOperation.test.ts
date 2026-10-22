import { expect, it } from 'vitest';
import { readAccountIntent } from './useAccountOperation';

it('restores only public operation metadata and discards sensitive or unexpected fields', () => {
  const operation_id = '20545b3a-e6f6-4b91-a122-9072055487ea';
  expect(readAccountIntent(JSON.stringify({ operation_id, kind: 'password_reset', revoke_sessions: true, new_password: 'must never persist', token_hash: 'not an ID' })))
    .toEqual({ operation_id, kind: 'password_reset', revoke_sessions: true });
  expect(readAccountIntent(JSON.stringify({ operation_id, kind: 'session_revoke', scope: 'all', session_id: 'unexpected' }))).toBeUndefined();
  expect(readAccountIntent(JSON.stringify({ operation_id: 'not-a-uuid', kind: 'password_reset', revoke_sessions: true }))).toBeUndefined();
});
