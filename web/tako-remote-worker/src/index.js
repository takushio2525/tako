// tako-remote-relay — Cloudflare Workers KV リレー
//
// マシン ID → 最新 tunnel URL のマッピングを KV に保存し、
// スマホ PWA が 2 回目以降も最新 URL を解決できるようにする。
//
// エンドポイント:
//   POST /api/register  — { machineId, tunnelUrl, secret? } を KV に保存（TTL 24h）
//   GET  /api/resolve/:machineId — 最新の tunnelUrl を返す
//
// 登録の保護（first-write-wins。#78）:
//   secret 付きで登録された machineId は、以後同じ secret（の SHA-256 一致）でしか
//   上書きできない。secret ハッシュは `secret:<machineId>` キーに TTL 30 日で保存し、
//   登録のたびに延長する。secret なしのレガシー登録は「secret 未登録の ID」に限り許可
//   （旧クライアント互換。新クライアントが一度 secret 登録すれば以後は保護される）。
//   resolve は従来どおり無認証: machineId 自体が能力トークンで、tunnel 先の tako は
//   別途トークン認証を持つ。
//
// KV バインディング: RELAY_KV（wrangler.toml で設定）

const CORS_HEADERS = {
  'Access-Control-Allow-Origin': '*',
  'Access-Control-Allow-Methods': 'GET, POST, OPTIONS',
  'Access-Control-Allow-Headers': 'Content-Type',
};

const TTL_SECONDS = 24 * 60 * 60; // 24 時間
const SECRET_TTL_SECONDS = 30 * 24 * 60 * 60; // 30 日（register ごとに延長）

// レートリミット（#104）: 送信元 IP ごとの固定ウィンドウ。無認証エンドポイントへの
// 無差別スパム（KV 書き込み・読み取りによる課金 / クォータ消費）を抑える。
// KV は結果整合なので厳密ではないが、桁違いの濫用を弾く用途には十分
const RATE_WINDOW_SECONDS = 60;
const RATE_LIMIT_REGISTER = 60; // register: 60 req/min/IP
const RATE_LIMIT_RESOLVE = 240; // resolve: 240 req/min/IP

function json(data, status = 200) {
  return new Response(JSON.stringify(data), {
    status,
    headers: { 'Content-Type': 'application/json', ...CORS_HEADERS },
  });
}

/**
 * 固定ウィンドウのレートリミット判定。上限超過なら true（= ブロック）。
 * KV キー `rl:<scope>:<ip>:<window>` にカウンタを持ち、ウィンドウ境界で自然に切り替わる。
 * IP が取れない場合は 'unknown' でまとめる（全体で 1 バケット）
 */
async function isRateLimited(env, scope, ip, limit) {
  const window = Math.floor(Date.now() / 1000 / RATE_WINDOW_SECONDS);
  const key = `rl:${scope}:${ip || 'unknown'}:${window}`;
  const current = parseInt((await env.RELAY_KV.get(key)) || '0', 10) || 0;
  if (current >= limit) {
    return true;
  }
  // TTL はウィンドウ 2 個分（境界での取りこぼしを避ける）
  await env.RELAY_KV.put(key, String(current + 1), {
    expirationTtl: RATE_WINDOW_SECONDS * 2,
  });
  return false;
}

function clientIp(request) {
  return request.headers.get('CF-Connecting-IP') || '';
}

async function sha256Hex(text) {
  const digest = await crypto.subtle.digest('SHA-256', new TextEncoder().encode(text));
  return [...new Uint8Array(digest)].map((b) => b.toString(16).padStart(2, '0')).join('');
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
      if (await isRateLimited(env, 'reg', clientIp(request), RATE_LIMIT_REGISTER)) {
        return json({ error: 'レート制限を超えました。しばらく待ってください' }, 429);
      }
      try {
        const body = await request.json();
        const { machineId, tunnelUrl, secret } = body;

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

        // secret の形式チェック（指定時は hex 64 文字のみ許可）
        if (secret !== undefined && !/^[0-9a-f]{64}$/i.test(String(secret))) {
          return json({ error: '無効な secret 形式' }, 400);
        }

        // first-write-wins の登録保護（#78）
        const secretKey = `secret:${machineId}`;
        const storedHash = await env.RELAY_KV.get(secretKey);
        const providedHash = secret ? await sha256Hex(String(secret).toLowerCase()) : null;
        if (storedHash) {
          if (providedHash !== storedHash) {
            return json({ error: 'この machineId は登録済みで、secret が一致しない' }, 403);
          }
          // 一致 → TTL を延長
          await env.RELAY_KV.put(secretKey, storedHash, { expirationTtl: SECRET_TTL_SECONDS });
        } else if (providedHash) {
          // 初回 secret 登録（以後この machineId は secret 必須になる）
          await env.RELAY_KV.put(secretKey, providedHash, { expirationTtl: SECRET_TTL_SECONDS });
        }
        // storedHash も providedHash も無い → レガシー登録（保護なし）として許可

        const entry = {
          tunnelUrl,
          updatedAt: new Date().toISOString(),
        };

        await env.RELAY_KV.put(
          `machine:${machineId}`,
          JSON.stringify(entry),
          { expirationTtl: TTL_SECONDS }
        );

        return json({ ok: true, machineId, protected: Boolean(storedHash || providedHash) });
      } catch (e) {
        return json({ error: 'リクエストの処理に失敗' }, 400);
      }
    }

    // GET /api/resolve/:machineId — 最新 tunnel URL の取得
    const resolveMatch = path.match(/^\/api\/resolve\/([0-9a-f-]+)$/i);
    if (request.method === 'GET' && resolveMatch) {
      if (await isRateLimited(env, 'res', clientIp(request), RATE_LIMIT_RESOLVE)) {
        return json({ error: 'レート制限を超えました。しばらく待ってください' }, 429);
      }
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
