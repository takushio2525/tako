import { useState, useEffect } from 'preact/hooks';
import { addMachine, setActiveMachine } from '../store';
import { createClient, resolveHost } from '../api';

export function ConnectPage({ params }) {
  const [status, setStatus] = useState('connecting');
  const [error, setError] = useState(null);
  const [detail, setDetail] = useState('');

  async function tryConnect(host, token, id, name) {
    // まず直接接続を試みる
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

    // machine ID がある場合、KV リレーから最新 URL を resolve
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

    // 両方失敗 → エラー表示（マシン情報は保存しておく）
    addMachine({ id, name, host, token });
    setStatus('error');
    setError('接続に失敗しました。Mac で tako remote start が実行中か確認してください。');
  }

  useEffect(() => {
    const host = params.get('host');
    const token = params.get('token');
    const id = params.get('machine') || `m-${Date.now()}`;
    const name = params.get('name') || id;

    if (!host || !token) {
      setStatus('error');
      setError('接続情報が不足しています（host / token）');
      return;
    }

    tryConnect(host, token, id, name);
  }, []);

  function retry() {
    const host = params.get('host');
    const token = params.get('token');
    const id = params.get('machine') || `m-${Date.now()}`;
    const name = params.get('name') || id;

    if (!host || !token) return;
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
          <p class="detail-text">{detail}</p>
        )}
        {error && <p class="error-text">{error}</p>}
        {status === 'error' && (
          <div class="connect-actions">
            <button class="btn" onClick={retry}>再試行</button>
            <button class="btn" onClick={() => { window.location.hash = '#/'; }}>
              マシン一覧
            </button>
            <p class="hint-text">
              接続先が変わった場合は、Mac で QR コードを再表示してスキャンしてください
            </p>
          </div>
        )}
      </div>
    </div>
  );
}
