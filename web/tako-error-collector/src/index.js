// tako-error-collector — エラーレポート収集 Cloudflare Worker
//
// tako アプリが検知したエラー（panic / 重大エラー）を PII なしで収集する。
// 蓄積データからの Issue 化はオーナーが手動で行う。
//
// エンドポイント:
//   POST /api/report        — レポート送信（認証不要・レートリミットあり）
//   GET  /api/reports       — レポート一覧（要 ADMIN_TOKEN）
//   GET  /api/reports/:id   — 単体取得（要 ADMIN_TOKEN）
//   DELETE /api/reports/:id — 削除（要 ADMIN_TOKEN）
//   GET  /api/health        — ヘルスチェック
//
// KV バインディング: REPORTS_KV
// Secret 環境変数: ADMIN_TOKEN（読み取り認証用。wrangler secret put で設定）

const CORS_HEADERS = {
  'Access-Control-Allow-Origin': '*',
  'Access-Control-Allow-Methods': 'GET, POST, DELETE, OPTIONS',
  'Access-Control-Allow-Headers': 'Content-Type, Authorization',
};

const REPORT_TTL_SECONDS = 90 * 24 * 60 * 60; // 90 日
const RATE_WINDOW_SECONDS = 60;
const RATE_LIMIT_REPORT = 10; // 10 req/min/IP
const MAX_REPORT_SIZE = 32 * 1024; // 32KB

function json(data, status = 200) {
  return new Response(JSON.stringify(data), {
    status,
    headers: { 'Content-Type': 'application/json', ...CORS_HEADERS },
  });
}

function clientIp(request) {
  return request.headers.get('CF-Connecting-IP') || '';
}

async function isRateLimited(env, ip) {
  const window = Math.floor(Date.now() / 1000 / RATE_WINDOW_SECONDS);
  const key = `rl:report:${ip || 'unknown'}:${window}`;
  const current = parseInt((await env.REPORTS_KV.get(key)) || '0', 10) || 0;
  if (current >= RATE_LIMIT_REPORT) {
    return true;
  }
  await env.REPORTS_KV.put(key, String(current + 1), {
    expirationTtl: RATE_WINDOW_SECONDS * 2,
  });
  return false;
}

function checkAdmin(request, env) {
  const token = env.ADMIN_TOKEN;
  if (!token) return false;
  const auth = request.headers.get('Authorization') || '';
  const provided = auth.startsWith('Bearer ') ? auth.slice(7) : '';
  if (!provided || provided.length !== token.length) return false;
  // 定数時間比較
  let mismatch = 0;
  for (let i = 0; i < token.length; i++) {
    mismatch |= token.charCodeAt(i) ^ provided.charCodeAt(i);
  }
  return mismatch === 0;
}

// レポートのバリデーション: 必須フィールドと型の最小検証
function validateReport(body) {
  if (!body || typeof body !== 'object') return 'リクエスト本文が不正';
  if (typeof body.version !== 'string' || !body.version) return 'version は必須';
  if (typeof body.error_kind !== 'string' || !body.error_kind) return 'error_kind は必須';
  const validKinds = ['panic', 'critical', 'invariant_violation'];
  if (!validKinds.includes(body.error_kind)) return `error_kind は ${validKinds.join('/')} のいずれか`;
  if (body.message !== undefined && typeof body.message !== 'string') return 'message は文字列';
  if (body.backtrace !== undefined && typeof body.backtrace !== 'string') return 'backtrace は文字列';
  if (body.os_version !== undefined && typeof body.os_version !== 'string') return 'os_version は文字列';
  // 文字列長の上限
  if (body.message && body.message.length > 2048) return 'message が長すぎる（上限 2048 文字）';
  if (body.backtrace && body.backtrace.length > 16384) return 'backtrace が長すぎる（上限 16384 文字）';
  return null;
}

export default {
  async fetch(request, env) {
    if (request.method === 'OPTIONS') {
      return new Response(null, { status: 204, headers: CORS_HEADERS });
    }

    const url = new URL(request.url);
    const path = url.pathname;

    // POST /api/report
    if (request.method === 'POST' && path === '/api/report') {
      if (await isRateLimited(env, clientIp(request))) {
        return json({ error: 'レート制限を超過。しばらく待ってください' }, 429);
      }
      // サイズチェック
      const contentLength = parseInt(request.headers.get('Content-Length') || '0', 10);
      if (contentLength > MAX_REPORT_SIZE) {
        return json({ error: 'レポートが大きすぎる' }, 413);
      }
      try {
        const body = await request.json();
        const err = validateReport(body);
        if (err) return json({ error: err }, 400);

        const id = crypto.randomUUID();
        const report = {
          id,
          version: body.version,
          os_version: body.os_version || null,
          error_kind: body.error_kind,
          message: body.message || null,
          backtrace: body.backtrace || null,
          received_at: new Date().toISOString(),
        };

        await env.REPORTS_KV.put(`report:${id}`, JSON.stringify(report), {
          expirationTtl: REPORT_TTL_SECONDS,
        });

        // 索引更新（最新 1000 件）
        const indexRaw = await env.REPORTS_KV.get('index:reports');
        const index = indexRaw ? JSON.parse(indexRaw) : [];
        index.unshift({ id, error_kind: body.error_kind, version: body.version, received_at: report.received_at });
        if (index.length > 1000) index.length = 1000;
        await env.REPORTS_KV.put('index:reports', JSON.stringify(index), {
          expirationTtl: REPORT_TTL_SECONDS,
        });

        return json({ ok: true, id });
      } catch {
        return json({ error: 'リクエストの処理に失敗' }, 400);
      }
    }

    // GET /api/reports
    if (request.method === 'GET' && path === '/api/reports') {
      if (!checkAdmin(request, env)) {
        return json({ error: '認証が必要' }, 401);
      }
      const indexRaw = await env.REPORTS_KV.get('index:reports');
      const index = indexRaw ? JSON.parse(indexRaw) : [];
      return json({ reports: index, count: index.length });
    }

    // GET /api/reports/:id
    const getMatch = path.match(/^\/api\/reports\/([0-9a-f-]+)$/i);
    if (request.method === 'GET' && getMatch) {
      if (!checkAdmin(request, env)) {
        return json({ error: '認証が必要' }, 401);
      }
      const id = getMatch[1];
      const data = await env.REPORTS_KV.get(`report:${id}`);
      if (!data) return json({ error: 'レポートが見つからない' }, 404);
      return json(JSON.parse(data));
    }

    // DELETE /api/reports/:id
    const delMatch = path.match(/^\/api\/reports\/([0-9a-f-]+)$/i);
    if (request.method === 'DELETE' && delMatch) {
      if (!checkAdmin(request, env)) {
        return json({ error: '認証が必要' }, 401);
      }
      const id = delMatch[1];
      await env.REPORTS_KV.delete(`report:${id}`);
      // 索引から除去
      const indexRaw = await env.REPORTS_KV.get('index:reports');
      if (indexRaw) {
        const index = JSON.parse(indexRaw).filter((r) => r.id !== id);
        await env.REPORTS_KV.put('index:reports', JSON.stringify(index), {
          expirationTtl: REPORT_TTL_SECONDS,
        });
      }
      return json({ ok: true, deleted: id });
    }

    // GET /api/health
    if (request.method === 'GET' && path === '/api/health') {
      return json({ status: 'ok', service: 'tako-error-collector' });
    }

    return json({ error: 'Not Found' }, 404);
  },
};
