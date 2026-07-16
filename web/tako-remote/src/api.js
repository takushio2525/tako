const TIMEOUT_MS = 10000;

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
    agents() {
      return request('GET', '/api/agents');
    },
    messages(sessionId, tail = 30) {
      return request('GET', `/api/sessions/${encodeURIComponent(sessionId)}/messages?tail=${tail}`);
    },
    // リサイズ要求は存在しない: リモート表示は PC 側のペインサイズに一切影響しない（#63）
    wsUrl(paneId) {
      const proto = base.startsWith('https') ? 'wss' : 'ws';
      const hostPart = base.replace(/^https?:\/\//, '');
      return `${proto}://${hostPart}/ws?pane=${encodeURIComponent(paneId)}`;
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
