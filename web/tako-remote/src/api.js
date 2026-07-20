const TIMEOUT_MS = 10000;

// PWA は daemon 自身（Tailscale Serve 経由の固定 ts.net URL）から配信されるため、
// API は常に同一 origin。認証は機器ペアリング二層認証がサーバー側で行う（#283）:
// - 層①: tailscale serve が付与する identity ヘッダ（クライアント側の作業なし）
// - 層②: Mac 画面で承認された端末か（未登録なら /api/me が registered=false を返す）
// 旧方式の bearer token・localStorage 保存は全廃した。
export function createClient() {
  const base = window.location.origin;

  async function request(method, path, body) {
    const headers = {};
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
      const e = new Error(err.error || `HTTP ${resp.status}`);
      e.status = resp.status;
      throw e;
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
    // この端末の登録状態（registered / pending / denied / role）
    me() {
      return request('GET', '/api/me');
    },
    // ペアリング / role 変更を要求する（Mac 画面に承認ダイアログが出る）
    pair(name, role = 'observe') {
      return request('POST', '/api/pair', { name, role });
    },
    panes() {
      return request('GET', '/api/v2/panes');
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
    // ファイルアップロード（#285: POST /api/upload。Interact role 必須）
    async upload(paneId, file) {
      const form = new FormData();
      form.append('file', file);
      form.append('pane', paneId);
      const resp = await fetch(`${base}/api/upload`, {
        method: 'POST',
        body: form,
        signal: AbortSignal.timeout(60000),
      });
      if (!resp.ok) {
        const err = await resp.json().catch(() => ({}));
        const e = new Error(err.error || `HTTP ${resp.status}`);
        e.status = resp.status;
        throw e;
      }
      return resp.json();
    },
    // WS の認証もサーバー側の identity + ペアリング照合（サブプロトコルに secret を載せない）
    wsProtocols() {
      return ['tako-remote'];
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
