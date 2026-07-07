// tako-remote-relay のスモークテスト（Node built-in test runner）
// 実行: npm test（wrangler 不要。KV をモックして fetch ハンドラを直接呼ぶ）
import { test } from 'node:test';
import assert from 'node:assert/strict';
import worker from '../src/index.js';

const MID = '01234567-89ab-4cde-8f01-23456789abcd'; // UUID v4 形式
const URL1 = 'https://foo-bar-baz.trycloudflare.com';
const URL2 = 'https://evil-attacker.trycloudflare.com';
const SECRET_A = 'a'.repeat(64);
const SECRET_B = 'b'.repeat(64);

function mockKV() {
  const store = new Map();
  return {
    async get(k) {
      return store.has(k) ? store.get(k) : null;
    },
    async put(k, v) {
      store.set(k, v);
    },
  };
}

function register(env, body) {
  return worker.fetch(
    new Request('https://relay.example/api/register', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(body),
    }),
    env
  );
}

function resolve(env, machineId) {
  return worker.fetch(new Request(`https://relay.example/api/resolve/${machineId}`), env);
}

test('secret 付き登録は protected になり、同じ secret で更新できる', async () => {
  const env = { RELAY_KV: mockKV() };
  const r1 = await register(env, { machineId: MID, tunnelUrl: URL1, secret: SECRET_A });
  assert.equal(r1.status, 200);
  assert.equal((await r1.json()).protected, true);

  const r2 = await register(env, { machineId: MID, tunnelUrl: URL1, secret: SECRET_A });
  assert.equal(r2.status, 200);
});

test('保護された machineId は別 secret / 無 secret で上書きできない', async () => {
  const env = { RELAY_KV: mockKV() };
  await register(env, { machineId: MID, tunnelUrl: URL1, secret: SECRET_A });

  const wrong = await register(env, { machineId: MID, tunnelUrl: URL2, secret: SECRET_B });
  assert.equal(wrong.status, 403);
  const none = await register(env, { machineId: MID, tunnelUrl: URL2 });
  assert.equal(none.status, 403);

  // tunnelUrl が乗っ取られていないこと
  const res = await resolve(env, MID);
  assert.equal((await res.json()).tunnelUrl, URL1);
});

test('レガシー（無 secret）登録は許可され、後から secret を紐付けたら以後保護される', async () => {
  const env = { RELAY_KV: mockKV() };
  const legacy = await register(env, { machineId: MID, tunnelUrl: URL1 });
  assert.equal(legacy.status, 200);
  assert.equal((await legacy.json()).protected, false);

  const claim = await register(env, { machineId: MID, tunnelUrl: URL1, secret: SECRET_A });
  assert.equal(claim.status, 200);
  const wrong = await register(env, { machineId: MID, tunnelUrl: URL2, secret: SECRET_B });
  assert.equal(wrong.status, 403);
});

test('入力バリデーション（machineId / tunnelUrl / secret）', async () => {
  const env = { RELAY_KV: mockKV() };
  assert.equal((await register(env, { machineId: 'not-uuid', tunnelUrl: URL1 })).status, 400);
  assert.equal(
    (await register(env, { machineId: MID, tunnelUrl: 'https://evil.example.com' })).status,
    400
  );
  assert.equal(
    (await register(env, { machineId: MID, tunnelUrl: URL1, secret: 'short' })).status,
    400
  );
});

test('resolve は登録済み tunnelUrl を返し、未登録は 404', async () => {
  const env = { RELAY_KV: mockKV() };
  assert.equal((await resolve(env, MID)).status, 404);
  await register(env, { machineId: MID, tunnelUrl: URL1, secret: SECRET_A });
  const res = await resolve(env, MID);
  assert.equal(res.status, 200);
  assert.equal((await res.json()).tunnelUrl, URL1);
});

test('register は同一 IP からの過剰リクエストを 429 で弾く（#104）', async () => {
  const env = { RELAY_KV: mockKV() };
  const ip = '203.0.113.7';
  function reg() {
    return worker.fetch(
      new Request('https://relay.example/api/register', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json', 'CF-Connecting-IP': ip },
        body: JSON.stringify({ machineId: MID, tunnelUrl: URL1 }),
      }),
      env
    );
  }
  // 上限（60/分）までは通り、超えたら 429
  let got429 = false;
  for (let i = 0; i < 62; i++) {
    const r = await reg();
    if (r.status === 429) {
      got429 = true;
      break;
    }
  }
  assert.equal(got429, true, '上限超過で 429 が返るべき');
});

test('別 IP はレート制限バケットを共有しない（#104）', async () => {
  const env = { RELAY_KV: mockKV() };
  function regFrom(ip) {
    return worker.fetch(
      new Request('https://relay.example/api/register', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json', 'CF-Connecting-IP': ip },
        body: JSON.stringify({ machineId: MID, tunnelUrl: URL1 }),
      }),
      env
    );
  }
  // IP-A を上限まで消費
  for (let i = 0; i < 61; i++) await regFrom('198.51.100.1');
  // IP-B は影響を受けず通る
  const other = await regFrom('198.51.100.2');
  assert.equal(other.status, 200);
});
