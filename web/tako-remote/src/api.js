const TIMEOUT_MS = 10000;

export function createClient(host, token) {
  const base = host.replace(/\/+$/, '');

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
    screen(id, lines) {
      const qs = lines ? `?lines=${lines}` : '';
      return request('GET', `/api/panes/${id}/screen${qs}`);
    },
    input(id, text, newline = true) {
      return request('POST', `/api/panes/${id}/input`, { text, newline });
    },
    close(id) {
      return request('POST', `/api/panes/${id}/close`);
    },
  };
}
