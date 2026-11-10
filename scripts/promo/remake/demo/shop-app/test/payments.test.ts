import { test } from 'node:test';
import assert from 'node:assert/strict';
import { withRetry } from '../src/lib/retry';

test('3 回目で成功すれば結果を返す', async () => {
  let calls = 0;
  const result = await withRetry(async () => {
    calls += 1;
    if (calls < 3) throw new Error('network');
    return 'ok';
  }, { retries: 3, baseMs: 1 });
  assert.equal(result, 'ok');
  assert.equal(calls, 3);
});
