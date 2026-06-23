import { useState, useEffect } from 'preact/hooks';
import { addMachine, setActiveMachine } from '../store';
import { createClient, resolveHost } from '../api';

export function ConnectPage({ params }) {
  const [status, setStatus] = useState('connecting');
  const [error, setError] = useState(null);
  const [detail, setDetail] = useState('');

  async function tryConnect(host, token, id, name) {
    setDetail('接続を確認中...');
    try {
      const client = createClient(host, token);
      await client.health();
      addMachine({ id, name, host, token });
      setActiveMachine(id);
      setStatus('connected');
      setTimeout(() => { window.location.hash = '#/panes'; }, 600);
      return;
    } catch {
      // 直接接続失敗 → KV リレー経由で最新 URL を取得
    }

    if (id) {
      setDetail('最新の接続先を検索中...');
      const resolved = await resolveHost(id);
      if (resolved && resolved !== host) {
        try {
          const client = createClient(resolved, token);
          await client.health();
          addMachine({ id, name, host: resolved, token });
          setActiveMachine(id);
          setStatus('connected');
          setTimeout(() => { window.location.hash = '#/panes'; }, 600);
          return;
        } catch {
          // resolve した URL でも接続失敗
        }
      }
    }

    addMachine({ id, name, host, token });
    setStatus('error');
    setError('接続に失敗しました。Mac で tako remote start が実行中か確認してください。');
  }

  useEffect(() => {
    const host = params.get('host') || window.location.origin;
    const token = params.get('token');
    const id = params.get('machine') || `m-${Date.now()}`;
    const name = params.get('name') || id;

    if (!token) {
      setStatus('error');
      setError('接続情報が不足しています（token）');
      return;
    }

    tryConnect(host, token, id, name);
  }, []);

  function retry() {
    const host = params.get('host') || window.location.origin;
    const token = params.get('token');
    const id = params.get('machine') || `m-${Date.now()}`;
    const name = params.get('name') || id;

    if (!token) return;
    setStatus('connecting');
    setError(null);
    tryConnect(host, token, id, name);
  }

  return (
    <div class="connect-page">
      <div class="connect-card">
        <div class="connect-icon">
          {status === 'connecting' && <div class="spinner" />}
          {status === 'connected' && <div class="status-badge success">✓</div>}
          {status === 'error' && <div class="status-badge danger">!</div>}
        </div>
        <h1>
          {status === 'connecting' ? '接続中...' :
           status === 'connected' ? '接続完了' : '接続エラー'}
        </h1>
        {status === 'connecting' && detail && (
          <p style="color: var(--text-muted); font-size: 13px; font-family: var(--mono); margin-top: 8px;">
            {detail}
          </p>
        )}
        {error && <p class="error-text">{error}</p>}
        {status === 'error' && (
          <div class="connect-actions">
            <button class="btn btn-primary" onClick={retry}>再試行</button>
            <button class="btn" onClick={() => { window.location.hash = '#/'; }}>
              マシン一覧
            </button>
          </div>
        )}
      </div>
    </div>
  );
}
