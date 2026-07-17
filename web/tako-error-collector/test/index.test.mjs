import { describe, it, beforeEach } from 'node:test';
import assert from 'node:assert/strict';
import worker from '../src/index.js';

// KV モック
function createMockKV() {
  const store = new Map();
  return {
    get: async (key) => store.get(key) || null,
    put: async (key, value, _opts) => store.set(key, value),
    delete: async (key) => store.delete(key),
    _store: store,
  };
}

function makeRequest(method, path, body, headers = {}) {
  const url = `https://test.example.com${path}`;
  const init = { method, headers: { ...headers } };
  if (body) {
    const json = JSON.stringify(body);
    init.body = json;
    init.headers['Content-Type'] = 'application/json';
    init.headers['Content-Length'] = String(new TextEncoder().encode(json).length);
  }
  return new Request(url, init);
}

describe('POST /api/report', () => {
  let env;

  beforeEach(() => {
    env = { REPORTS_KV: createMockKV(), ADMIN_TOKEN: 'test-secret-token' };
  });

  it('有効なレポートを受け入れる', async () => {
    const req = makeRequest('POST', '/api/report', {
      version: '0.5.5',
      os_version: 'Darwin 25.2.0',
      error_kind: 'panic',
      message: 'index out of bounds',
      backtrace: 'at ~/src/main.rs:42',
    });
    const res = await worker.fetch(req, env);
    assert.equal(res.status, 200);
    const data = await res.json();
    assert.equal(data.ok, true);
    assert.ok(data.id);
  });

  it('必須フィールドが欠けるとエラー', async () => {
    const req = makeRequest('POST', '/api/report', {
      version: '0.5.5',
    });
    const res = await worker.fetch(req, env);
    assert.equal(res.status, 400);
  });

  it('不正な error_kind はエラー', async () => {
    const req = makeRequest('POST', '/api/report', {
      version: '0.5.5',
      error_kind: 'warning',
    });
    const res = await worker.fetch(req, env);
    assert.equal(res.status, 400);
  });

  it('索引に追加される', async () => {
    const req = makeRequest('POST', '/api/report', {
      version: '0.5.5',
      error_kind: 'panic',
      message: 'test',
    });
    await worker.fetch(req, env);
    const index = JSON.parse(await env.REPORTS_KV.get('index:reports'));
    assert.equal(index.length, 1);
    assert.equal(index[0].error_kind, 'panic');
  });
});

describe('GET /api/reports', () => {
  let env;

  beforeEach(async () => {
    env = { REPORTS_KV: createMockKV(), ADMIN_TOKEN: 'test-secret-token' };
    // レポート 1 件追加
    await worker.fetch(
      makeRequest('POST', '/api/report', {
        version: '0.5.5',
        error_kind: 'panic',
        message: 'test crash',
      }),
      env,
    );
  });

  it('認証なしは 401', async () => {
    const res = await worker.fetch(makeRequest('GET', '/api/reports'), env);
    assert.equal(res.status, 401);
  });

  it('認証ありで一覧が取れる', async () => {
    const req = makeRequest('GET', '/api/reports', null, {
      Authorization: 'Bearer test-secret-token',
    });
    const res = await worker.fetch(req, env);
    assert.equal(res.status, 200);
    const data = await res.json();
    assert.equal(data.count, 1);
    assert.equal(data.reports[0].error_kind, 'panic');
  });

  it('不正トークンは 401', async () => {
    const req = makeRequest('GET', '/api/reports', null, {
      Authorization: 'Bearer wrong-token',
    });
    const res = await worker.fetch(req, env);
    assert.equal(res.status, 401);
  });
});

describe('GET /api/reports/:id', () => {
  let env, reportId;

  beforeEach(async () => {
    env = { REPORTS_KV: createMockKV(), ADMIN_TOKEN: 'test-secret-token' };
    const res = await worker.fetch(
      makeRequest('POST', '/api/report', {
        version: '0.5.5',
        error_kind: 'critical',
        message: 'daemon failed',
      }),
      env,
    );
    reportId = (await res.json()).id;
  });

  it('単体レポートが取れる', async () => {
    const req = makeRequest('GET', `/api/reports/${reportId}`, null, {
      Authorization: 'Bearer test-secret-token',
    });
    const res = await worker.fetch(req, env);
    assert.equal(res.status, 200);
    const data = await res.json();
    assert.equal(data.error_kind, 'critical');
    assert.equal(data.message, 'daemon failed');
  });
});

describe('DELETE /api/reports/:id', () => {
  let env, reportId;

  beforeEach(async () => {
    env = { REPORTS_KV: createMockKV(), ADMIN_TOKEN: 'test-secret-token' };
    const res = await worker.fetch(
      makeRequest('POST', '/api/report', {
        version: '0.5.5',
        error_kind: 'panic',
        message: 'delete me',
      }),
      env,
    );
    reportId = (await res.json()).id;
  });

  it('削除できる', async () => {
    const req = makeRequest('DELETE', `/api/reports/${reportId}`, null, {
      Authorization: 'Bearer test-secret-token',
    });
    const res = await worker.fetch(req, env);
    assert.equal(res.status, 200);
    // 索引からも消える
    const index = JSON.parse(await env.REPORTS_KV.get('index:reports'));
    assert.equal(index.length, 0);
  });
});

describe('レートリミット', () => {
  it('連続投稿は制限される', async () => {
    const env = { REPORTS_KV: createMockKV(), ADMIN_TOKEN: 'test-secret-token' };
    const body = { version: '0.5.5', error_kind: 'panic', message: 'flood' };
    let blocked = false;
    for (let i = 0; i < 15; i++) {
      const req = makeRequest('POST', '/api/report', body, {
        'CF-Connecting-IP': '1.2.3.4',
      });
      const res = await worker.fetch(req, env);
      if (res.status === 429) {
        blocked = true;
        break;
      }
    }
    assert.ok(blocked, '10 req/min を超えると 429 が返るべき');
  });
});

describe('GET /api/health', () => {
  it('ok を返す', async () => {
    const env = { REPORTS_KV: createMockKV() };
    const res = await worker.fetch(makeRequest('GET', '/api/health'), env);
    assert.equal(res.status, 200);
    const data = await res.json();
    assert.equal(data.status, 'ok');
  });
});
