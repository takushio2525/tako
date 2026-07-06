import { useState, useEffect } from 'preact/hooks';
import { addMachine, setActiveMachine } from '../store';
import { createClient, resolveHost } from '../api';

// この PWA が動作を保証するデーモン（Mac 側 tako）の最小バージョン。
// Pages 配信の PWA とデーモンはバージョンが独立に進むため（Issue #91 留意点）、
// /api/health が返す version がこれより古い・無い場合は警告を出す（接続は続行する）
const MIN_DAEMON_VERSION = '0.2.0';

function versionOlderThan(version, min) {
  if (!version) return true;
  const va = String(version).split('.').map(n => parseInt(n, 10) || 0);
  const vb = String(min).split('.').map(n => parseInt(n, 10) || 0);
  for (let i = 0; i < 3; i++) {
    if ((va[i] || 0) !== (vb[i] || 0)) return (va[i] || 0) < (vb[i] || 0);
  }
  return false;
}

// Pages（静的ホスティング）から配信されているか。この場合 origin はデーモンではないので、
// 自分の origin への health 試行（失敗まで 5 秒待ち）をスキップして即リレー解決に進む
function isStaticHosting() {
  return /\.pages\.dev$/i.test(window.location.hostname);
}

// URL パラメータから接続情報を読む。host はデーモン候補が既知の場合のみ
// （明示 host パラメータ、または内蔵 PWA 配信 = origin がデーモン自身）
function readParams(params) {
  const host = params.get('host') || (isStaticHosting() ? null : window.location.origin);
  const token = params.get('token');
  const id = params.get('machine') || `m-${Date.now()}`;
  const name = params.get('name') || id;
  return { host, token, id, name };
}

export function ConnectPage({ params }) {
  const [status, setStatus] = useState('connecting');
  const [error, setError] = useState(null);
  const [warning, setWarning] = useState(null);
  const [detail, setDetail] = useState('');

  async function connectTo(host, token, id, name) {
    const client = createClient(host, token);
    const info = await client.health();
    addMachine({ id, name, host, token, version: info && info.version });
    setActiveMachine(id);
    let delay = 600;
    if (versionOlderThan(info && info.version, MIN_DAEMON_VERSION)) {
      setWarning('Mac 側の tako が古い可能性があります。表示や操作が崩れる場合は tako を更新してください。');
      delay = 2500;
    }
    setStatus('connected');
    setTimeout(() => { window.location.hash = '#/panes'; }, delay);
  }

  async function tryConnect(host, token, id, name) {
    if (host) {
      setDetail('接続を確認中...');
      try {
        await connectTo(host, token, id, name);
        return;
      } catch {
        // 直接接続失敗 → KV リレー経由で最新 URL を取得
      }
    }

    if (id) {
      setDetail('最新の接続先を検索中...');
      const resolved = await resolveHost(id);
      if (resolved && resolved !== host) {
        try {
          await connectTo(resolved, token, id, name);
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
    const { host, token, id, name } = readParams(params);

    if (!token) {
      setStatus('error');
      setError('接続情報が不足しています（token）');
      return;
    }

    tryConnect(host, token, id, name);
  }, []);

  function retry() {
    const { host, token, id, name } = readParams(params);

    if (!token) return;
    setStatus('connecting');
    setError(null);
    setWarning(null);
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
        {status === 'connected' && warning && (
          <p style="color: #d97706; font-size: 13px; margin-top: 8px;">{warning}</p>
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
