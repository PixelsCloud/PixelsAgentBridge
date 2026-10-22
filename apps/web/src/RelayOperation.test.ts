import { describe, expect, it } from 'vitest';
import { remainingSeconds, type RelayOperation } from './relayOperationState';

describe('maintenance countdown', () => {
  const operation = { server_now_unix_ms: 100_000, deadline_unix_ms: 130_000, phase: 'waiting' } as RelayOperation;
  it('uses elapsed time from the server sample, independently of the local date', () => {
    expect(remainingSeconds(operation, 0)).toBe(30);
    expect(remainingSeconds(operation, 1900)).toBe(29);
    expect(remainingSeconds(operation, 40_000)).toBe(0);
    expect(operation.phase).toBe('waiting');
  });
});
