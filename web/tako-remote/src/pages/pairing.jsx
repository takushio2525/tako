import { useState, useEffect, useRef } from 'preact/hooks';
import { createClient } from '../api';
import { getDeviceName, setDeviceName } from '../store';

const POLL_MS = 2000;

const ROLES = [
  { value: 'observe', label: 'Observe', desc: '画面を見るだけ（推奨）' },
  { value: 'interact', label: 'Interact', desc: '+ テキスト入力・承認応答' },
  { value: 'manage', label: 'Manage', desc: '+ ペインの管理' },
  { value: 'admin', label: 'Admin', desc: '+ 端末管理' },
];

export function PairingPage({ me, onRegistered }) {
  const [name, setName] = useState(getDeviceName());
  const [role, setRole] = useState('observe');
  const [phase, setPhase] = useState(me.pending ? 'pending' : me.denied ? 'denied' : 'form');
  const [error, setError] = useState(null);
  const pollRef = useRef(null);

  useEffect(() => {
    if (phase !== 'pending') return undefined;
    pollRef.current = setInterval(async () => {
      try {
        const result = await createClient().me();
        if (result.registered) {
          clearInterval(pollRef.current);
          onRegistered();
        } else if (result.denied) {
          clearInterval(pollRef.current);
          setPhase('denied');
        }
      } catch {}
    }, POLL_MS);
    return () => clearInterval(pollRef.current);
  }, [phase]);

  async function requestPairing() {
    setError(null);
    const trimmed = name.trim();
    if (trimmed) setDeviceName(trimmed);
    try {
      const result = await createClient().pair(trimmed, role);
      if (result.status === 'already_registered') {
        onRegistered();
        return;
      }
      setPhase('pending');
    } catch (e) {
      setError(e.message);
    }
  }

  // 承認待ち — カンプのデザイントークンでダーク統一
  if (phase === 'pending') {
    return (
      <div class="connect-page">
        <div class="connect-card">
          <div class="connect-icon"><div class="spinner" /></div>
          <h1>Mac で承認してください</h1>
          <p style="color: var(--fg2); font-size: 14px; margin-top: 8px; max-width: 300px; line-height: 1.6;">
            {me.host ? `${me.host} の` : 'Mac の'}画面にペアリングの承認ダイアログが表示されています。
            「許可」を押すとこの端末が登録されます。
          </p>
          <div class="connect-actions">
            <button class="btn" onClick={() => setPhase('form')}>やり直す</button>
          </div>
        </div>
      </div>
    );
  }

  if (phase === 'denied') {
    return (
      <div class="connect-page">
        <div class="connect-card">
          <div class="connect-icon">
            <span class="status-badge-circle danger">!</span>
          </div>
          <h1>拒否されました</h1>
          <p style="color: var(--fg2); font-size: 14px; margin-top: 8px; max-width: 300px; line-height: 1.6;">
            Mac 側でペアリングが拒否されました。心当たりがなければそのまま閉じてください。
          </p>
          <div class="connect-actions">
            <button class="btn btn-primary" onClick={() => setPhase('form')}>もう一度要求する</button>
          </div>
        </div>
      </div>
    );
  }

  return (
    <div class="connect-page">
      <div class="connect-card" style="max-width: 360px;">
        <h1>この端末をペアリング</h1>
        <p style="color: var(--fg2); font-size: 13px; margin: 8px 0 16px; line-height: 1.6;">
          {me.host ? `${me.host} に` : 'Mac に'}この端末の登録を要求します。
          Mac の画面で承認されるまで画面データは表示されません。
        </p>
        <label style="display: block; text-align: left; font-family: var(--mono); font-size: 10px; color: var(--fg3); letter-spacing: .05em; text-transform: uppercase; margin-bottom: 6px;">
          端末の名前
        </label>
        <input
          type="text"
          value={name}
          placeholder="例: iPhone"
          maxLength={64}
          onInput={e => setName(e.target.value)}
          style="width: 100%; padding: 10px 14px; border-radius: 13px; border: 1px solid var(--line); background: var(--panel); color: var(--fg); font-family: var(--mono); font-size: 13px; margin-bottom: 16px; outline: none;"
        />
        <label style="display: block; text-align: left; font-family: var(--mono); font-size: 10px; color: var(--fg3); letter-spacing: .05em; text-transform: uppercase; margin-bottom: 6px;">
          希望する権限
        </label>
        <div style="display: flex; flex-direction: column; gap: 7px; margin-bottom: 16px;">
          {ROLES.map(r => (
            <button
              key={r.value}
              style={`
                display: flex; align-items: center; gap: 11px;
                border: 1px solid ${role === r.value ? 'var(--claude)' : 'var(--line)'};
                border-radius: 13px; padding: 13px 14px; background: ${role === r.value ? 'var(--panel3)' : 'none'};
                cursor: pointer; text-align: left; color: var(--fg);
              `}
              onClick={() => setRole(r.value)}
            >
              <div style="display: flex; flex-direction: column; gap: 1px; min-width: 0;">
                <span style="font-family: var(--mono); font-size: 13px; font-weight: 600;">{r.label}</span>
                <span style="font-size: 11px; color: var(--fg2);">{r.desc}</span>
              </div>
              {role === r.value && (
                <span style="margin-left: auto; color: var(--claude); font-size: 15px;">{'✓'}</span>
              )}
            </button>
          ))}
        </div>
        {error && <p class="error-text">{error}</p>}
        <button
          class="btn"
          style="width: 100%; background: var(--claude); color: #1B0E09; border-color: var(--claude); font-weight: 600;"
          onClick={requestPairing}
        >
          ペアリングを要求
        </button>
      </div>
    </div>
  );
}
