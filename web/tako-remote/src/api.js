const TIMEOUT_MS = 10000;
const DEFAULT_RELAY_URL = 'https://tako-remote-relay.takushio2525.workers.dev';

export function createClient(host, token) {
  const raw = host.replace(/\/+$/, '');
  const base = /^https?:\/\//.test(raw) ? raw : `http://${raw}`;

  async function request(method, path, body) {
    const headers = { 'Authorization': `Bearer ${token}` };
    if (body !== undefined) {
      headers['Content-Type'] = 'application/json';
    }
    const resp = await fetch(`${base}${path}`, {
      method,
      headers,
      body: body !== undefined ? JSON.stringify(body) : undefined,
      signal: AbortSignal.timeout(TIMEOUT_MS),
    });
    if (!resp.ok) {
      const err = await resp.json().catch(() => ({}));
      throw new Error(err.error || `HTTP ${resp.status}`);
    }
    return resp.json();
  }

  return {
    health() {
      return fetch(`${base}/api/health`, {
        signal: AbortSignal.timeout(5000),
      }).then(r => {
        if (!r.ok) throw new Error(`HTTP ${r.status}`);
        return r.json();
      });
    },
    panes() {
      return request('GET', '/api/panes');
    },
    screen(id, lines, ansi = false) {
      const params = [];
      if (lines) params.push(`lines=${lines}`);
      if (ansi) params.push('ansi=1');
      const qs = params.length ? `?${params.join('&')}` : '';
      return request('GET', `/api/panes/${encodeURIComponent(id)}/screen${qs}`);
    },
    scrollback(id, lines = 1000) {
      return request('GET', `/api/panes/${encodeURIComponent(id)}/scrollback?lines=${lines}`);
    },
    input(id, text, newline = true) {
      return request('POST', `/api/panes/${encodeURIComponent(id)}/input`, { text, newline });
    },
    sendKeys(id, keys) {
      return request('POST', `/api/panes/${encodeURIComponent(id)}/input`, { keys });
    },
    close(id) {
      return request('POST', `/api/panes/${encodeURIComponent(id)}/close`);
    },
    resize(id, cols, rows) {
      return request('POST', `/api/panes/${encodeURIComponent(id)}/resize`, { cols, rows });
    },
    resizeReset(id) {
      return request('POST', `/api/panes/${encodeURIComponent(id)}/resize`, { reset: true });
    },
    agents() {
      return request('GET', '/api/agents');
    },
    messages(sessionId, tail = 30) {
      return request('GET', `/api/sessions/${encodeURIComponent(sessionId)}/messages?tail=${tail}`);
    },
    wsUrl(paneId, cols, rows) {
      const proto = base.startsWith('https') ? 'wss' : 'ws';
      const hostPart = base.replace(/^https?:\/\//, '');
      let url = `${proto}://${hostPart}/ws?pane=${encodeURIComponent(paneId)}`;
      if (cols && rows) url += `&cols=${cols}&rows=${rows}`;
      return url;
    },
    wsProtocols() {
      return ['tako-remote', `token.${token}`];
    },
    base() {
      return base;
    },
  };
}

/**
 * Workers KV リレーから最新の tunnel URL を解決する。
 * 失敗時は null を返す（フォールバックは呼び出し側で行う）
 */
export async function resolveHost(machineId) {
  if (!machineId) return null;
  try {
    const resp = await fetch(`${DEFAULT_RELAY_URL}/api/resolve/${machineId}`, {
      signal: AbortSignal.timeout(5000),
    });
    if (!resp.ok) return null;
    const data = await resp.json();
    return data.tunnelUrl || null;
  } catch {
    return null;
  }
}

/**
 * 指数バックオフ付きリトライ。接続復旧を待つ用途。
 * callback が成功するまで最大 maxRetries 回リトライし、成功した結果を返す。
 * 全リトライ失敗時は最後のエラーを throw する。
 */
export async function withRetry(callback, { maxRetries = 5, baseDelay = 1000 } = {}) {
  let lastError;
  for (let i = 0; i <= maxRetries; i++) {
    try {
      return await callback();
    } catch (e) {
      lastError = e;
      if (i < maxRetries) {
        const delay = baseDelay * Math.pow(2, i) + Math.random() * 500;
        await new Promise(r => setTimeout(r, delay));
      }
    }
  }
  throw lastError;
}
