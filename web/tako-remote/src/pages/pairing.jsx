import { useState, useEffect, useRef } from 'preact/hooks';
import { createClient } from '../api';
import { getDeviceName, setDeviceName } from '../store';

// 承認待ちの me() ポーリング間隔
const POLL_MS = 2000;

const ROLES = [
  { value: 'observe', label: 'Observe', desc: '画面を見るだけ（推奨・既定）' },
  { value: 'interact', label: 'Interact', desc: '+ テキスト入力・承認応答' },
  { value: 'manage', label: 'Manage', desc: '+ ペインを閉じる・リサイズ' },
  { value: 'admin', label: 'Admin', desc: '+ 端末管理（一覧・失効）' },
];

// 機器ペアリング画面（#283）。未登録端末はここしか操作できない:
// 名前と希望 role を添えてペアリングを要求 → Mac 画面の承認ダイアログで
// ユーザーが許可すると登録され、onRegistered() 経由で本画面へ進む
export function PairingPage({ me, onRegistered }) {
  const [name, setName] = useState(getDeviceName());
  const [role, setRole] = useState('observe');
  const [phase, setPhase] = useState(me.pending ? 'pending' : me.denied ? 'denied' : 'form');
  const [error, setError] = useState(null);
  const pollRef = useRef(null);

  // 承認待ちの間は me() をポーリングし、登録されたら親へ通知する
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
      } catch {
        // 一時的な失敗はポーリング継続
      }
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

  if (phase === 'pending') {
    return (
      <div class="connect-page">
        <div class="connect-card">
          <div class="connect-icon"><div class="spinner" /></div>
          <h1>Mac で承認してください</h1>
          <p style="color: var(--text-muted); font-size: 14px; margin-top: 8px;">
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
          <div class="connect-icon"><div class="status-badge danger">!</div></div>
          <h1>拒否されました</h1>
          <p style="color: var(--text-muted); font-size: 14px; margin-top: 8px;">
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
        <p style="color: var(--text-muted); font-size: 13px; margin: 8px 0 16px;">
          {me.host ? `${me.host} に` : 'Mac に'}この端末の登録を要求します。
          Mac の画面で承認されるまで画面データは表示されません。
        </p>
        <label style="display: block; text-align: left; font-size: 12px; color: var(--text-muted); margin-bottom: 4px;">
          端末の名前
        </label>
        <input
          type="text"
          value={name}
          placeholder="例: iPhone"
          maxLength={64}
          onInput={e => setName(e.target.value)}
          style="width: 100%; padding: 10px 12px; border-radius: 8px; border: 1px solid var(--border, #2a2e35); background: var(--panel, #121519); color: inherit; font-size: 15px; margin-bottom: 14px;"
        />
        <label style="display: block; text-align: left; font-size: 12px; color: var(--text-muted); margin-bottom: 4px;">
          希望する権限
        </label>
        <div style="display: flex; flex-direction: column; gap: 6px; margin-bottom: 16px;">
          {ROLES.map(r => (
            <button
              key={r.value}
              class="btn"
              style={`display: flex; justify-content: space-between; align-items: baseline; gap: 8px; text-align: left; ${role === r.value ? 'border-color: var(--accent, #D6795C); color: var(--accent, #D6795C);' : ''}`}
              onClick={() => setRole(r.value)}
            >
              <span style="font-weight: 600;">{r.label}</span>
              <span style="font-size: 11px; color: var(--text-muted); flex: 1; text-align: right;">{r.desc}</span>
            </button>
          ))}
        </div>
        {error && <p class="error-text">{error}</p>}
        <button class="btn btn-primary" style="width: 100%;" onClick={requestPairing}>
          ペアリングを要求
        </button>
      </div>
    </div>
  );
}
