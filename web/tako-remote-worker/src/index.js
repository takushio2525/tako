// tako-remote-relay — Cloudflare Workers KV リレー
//
// マシン ID → 最新 tunnel URL のマッピングを KV に保存し、
// スマホ PWA が 2 回目以降も最新 URL を解決できるようにする。
//
// エンドポイント:
//   POST /api/register  — { machineId, tunnelUrl } を KV に保存（TTL 24h）
//   GET  /api/resolve/:machineId — 最新の tunnelUrl を返す
//
// KV バインディング: RELAY_KV（wrangler.toml で設定）

const CORS_HEADERS = {
  'Access-Control-Allow-Origin': '*',
  'Access-Control-Allow-Methods': 'GET, POST, OPTIONS',
  'Access-Control-Allow-Headers': 'Content-Type',
};

const TTL_SECONDS = 24 * 60 * 60; // 24 時間

function json(data, status = 200) {
  return new Response(JSON.stringify(data), {
    status,
    headers: { 'Content-Type': 'application/json', ...CORS_HEADERS },
  });
}

export default {
  async fetch(request, env) {
    if (request.method === 'OPTIONS') {
      return new Response(null, { status: 204, headers: CORS_HEADERS });
    }

    const url = new URL(request.url);
    const path = url.pathname;

    // POST /api/register — tunnel URL の登録
    if (request.method === 'POST' && path === '/api/register') {
      try {
        const body = await request.json();
        const { machineId, tunnelUrl } = body;

        if (!machineId || !tunnelUrl) {
          return json({ error: 'machineId と tunnelUrl は必須' }, 400);
        }

        // machineId の形式チェック（UUID v4 形式のみ許可）
        if (!/^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i.test(machineId)) {
          return json({ error: '無効な machineId 形式' }, 400);
        }

        // tunnelUrl の形式チェック（trycloudflare.com のみ許可）
        if (!tunnelUrl.startsWith('https://') || !tunnelUrl.includes('.trycloudflare.com')) {
          return json({ error: '無効な tunnelUrl 形式' }, 400);
        }

        const entry = {
          tunnelUrl,
          updatedAt: new Date().toISOString(),
        };

        await env.RELAY_KV.put(
          `machine:${machineId}`,
          JSON.stringify(entry),
          { expirationTtl: TTL_SECONDS }
        );

        return json({ ok: true, machineId });
      } catch (e) {
        return json({ error: 'リクエストの処理に失敗' }, 400);
      }
    }

    // GET /api/resolve/:machineId — 最新 tunnel URL の取得
    const resolveMatch = path.match(/^\/api\/resolve\/([0-9a-f-]+)$/i);
    if (request.method === 'GET' && resolveMatch) {
      const machineId = resolveMatch[1];
      const data = await env.RELAY_KV.get(`machine:${machineId}`);

      if (!data) {
        return json({ error: 'マシンが見つからない（オフラインか未登録）' }, 404);
      }

      try {
        const entry = JSON.parse(data);
        return json({
          machineId,
          tunnelUrl: entry.tunnelUrl,
          updatedAt: entry.updatedAt,
        });
      } catch {
        return json({ error: 'データの読み取りに失敗' }, 500);
      }
    }

    // GET /api/health
    if (request.method === 'GET' && path === '/api/health') {
      return json({ status: 'ok', service: 'tako-remote-relay' });
    }

    return json({ error: 'Not Found' }, 404);
  },
};
