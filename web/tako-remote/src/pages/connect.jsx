import { useState, useEffect } from 'preact/hooks';
import { addMachine, setActiveMachine } from '../store';
import { createClient } from '../api';

export function ConnectPage({ params }) {
  const [status, setStatus] = useState('connecting');
  const [error, setError] = useState(null);

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

    (async () => {
      try {
        const client = createClient(host, token);
        await client.health();
        addMachine({ id, name, host, token });
        setActiveMachine(id);
        setStatus('connected');
        setTimeout(() => { window.location.hash = '#/panes'; }, 600);
      } catch (e) {
        addMachine({ id, name, host, token });
        setStatus('error');
        setError(`接続失敗: ${e.message}`);
      }
    })();
  }, []);

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
        {error && <p class="error-text">{error}</p>}
        {status === 'error' && (
          <div class="connect-actions">
            <button class="btn" onClick={() => {
              setStatus('connecting');
              setError(null);
              const host = params.get('host');
              const token = params.get('token');
              if (host && token) {
                const client = createClient(host, token);
                client.health()
                  .then(() => {
                    setStatus('connected');
                    setActiveMachine(params.get('machine') || `m-${Date.now()}`);
                    setTimeout(() => { window.location.hash = '#/panes'; }, 600);
                  })
                  .catch(e => {
                    setStatus('error');
                    setError(`接続失敗: ${e.message}`);
                  });
              }
            }}>再試行</button>
            <button class="btn btn-primary" onClick={() => { window.location.hash = '#/'; }}>
              マシン一覧
            </button>
          </div>
        )}
      </div>
    </div>
  );
}
