// SSH の切り替え / 新規接続（#1080。エピック #1059 柱 2-H）
//
// PC 側の部品はすべて揃っている（#1006 の開き先 3 種 / #1010 の接続状態 /
// #1040 の自動再接続）。ここはその**導線と表示だけ**で、接続の判断は一切持たない:
//
// - 開き先の語彙（split / tab / pane）は PC と同じものを送る
// - 「このペインを SSH にできるか」は `/api/v2/panes` の `can_ssh` をそのまま読む
//   （判定材料は PC 側にしか無い。ここで作り直すと必ずずれる）
// - 接続の進み方と失敗の理由は `ssh_connect` をそのまま出す。**失敗しても消えない**
//   （#919 / #1040 の契約。消すと「押したのに何も起きない」に戻る）
import { useState, useEffect } from 'preact/hooks';

// 開き先。ラベルは PC の右クリックメニューと同じ意味にする（#1006）
const TARGETS = [
  { id: 'pane', label: 'このペイン', desc: 'いま見ているペインを SSH にする' },
  { id: 'split', label: '新しいペイン', desc: '同じタブに並べて開く' },
  { id: 'tab', label: '新しいタブ', desc: '別のタブで開く' },
];

// ssh_connect.phase → 表示（色と文言）。未知の phase は握り潰さず素のまま出す
const PHASES = {
  connecting: { label: '接続中', tone: 'wait', spinner: true },
  reconnecting: { label: '再接続中', tone: 'wait', spinner: true },
  failed: { label: '接続できません', tone: 'bad', spinner: false },
  gave_up: { label: '再接続を中止しました', tone: 'bad', spinner: false },
};

function hostLine(h) {
  // name 以外は補助情報。無い項目は出さない（空の「 · 」を並べない）
  const parts = [];
  if (h.hostname && h.hostname !== h.name) parts.push(h.hostname);
  if (h.user) parts.push(h.user);
  if (h.port) parts.push(`:${h.port}`);
  return parts.join(' · ');
}

/**
 * SSH の接続状態バー（#1010 / #1040）。
 * `ssh_connect` が null（＝接続待ちでも失敗でもない）なら何も描かない。
 */
export function SshConnectBar({ state }) {
  if (!state) return null;
  const phase = PHASES[state.phase] || { label: state.phase, tone: 'wait', spinner: false };
  const attempt =
    state.attempt && state.max_attempts
      ? `${state.attempt}/${state.max_attempts} 回目`
      : '';
  const wait = state.retry_in_secs ? `${state.retry_in_secs} 秒後に再試行` : '';
  const elapsed = Number.isFinite(state.elapsed_secs) ? `${state.elapsed_secs}s` : '';
  const meta = [state.host, attempt, wait || elapsed].filter(Boolean).join(' · ');
  return (
    <div class={`ssh-bar ssh-bar-${phase.tone}`} data-testid="ssh-connect-bar">
      <div class="ssh-bar-head">
        {phase.spinner && <span class="ssh-bar-spinner" />}
        <span class="ssh-bar-label">{phase.label}</span>
        <span class="ssh-bar-meta">{meta}</span>
      </div>
      {state.reason && <div class="ssh-bar-reason">{state.reason}</div>}
      {state.next_step && <div class="ssh-bar-next">{state.next_step}</div>}
    </div>
  );
}

/**
 * ホストを選んで接続するシート。
 *
 * `canSsh` = `/api/v2/panes` の `can_ssh`（`{ ok, reason, note }`）。
 * ok が false なら「このペイン」は**選択肢に出さない**（#1080 受け入れ条件 ③）。
 * ただし理由は 1 行だけ添える: 出さないだけだと、スマホには右クリックのような
 * 別の入口が無いので「なぜ出来ないのか」を確かめる手段が消える
 */
export function SshSheet({ client, paneId, canSsh, onClose, onOpened }) {
  const [hosts, setHosts] = useState(null);
  const [error, setError] = useState(null);
  const paneAllowed = !!(canSsh && canSsh.ok);
  const targets = TARGETS.filter(t => t.id !== 'pane' || paneAllowed);
  const [target, setTarget] = useState(paneAllowed ? 'pane' : 'split');
  const [busy, setBusy] = useState(null);

  useEffect(() => {
    let alive = true;
    client
      .sshHosts()
      .then(r => {
        if (alive) setHosts(r.hosts || []);
      })
      .catch(e => {
        if (alive) setError(e.message);
      });
    return () => {
      alive = false;
    };
  }, [client]);

  async function connect(host) {
    if (busy) return;
    setBusy(host.name);
    setError(null);
    if (navigator.vibrate) navigator.vibrate(10);
    try {
      const result =
        target === 'pane'
          ? await client.sshPane(paneId, host.name, { target: 'pane' })
          : await client.sshOpen(host.name, { target, pane: Number(paneId) || undefined });
      onOpened(result, target);
    } catch (e) {
      // 接続そのものの失敗ではなく「開けなかった」失敗（403 / 409 / 503）。
      // 開けたあとの失敗は ssh_connect 側に出るので、ここで消してはいけない
      setError(e.message);
      setBusy(null);
    }
  }

  return (
    <div class="sheet-overlay" onClick={onClose}>
      <div class="sheet-panel" onClick={e => e.stopPropagation()} data-testid="ssh-sheet">
        <div class="sheet-handle" />
        <div class="sheet-section-label">OPEN IN</div>
        <div class="sheet-effort-bar">
          {targets.map(t => (
            <button
              key={t.id}
              class={`sheet-effort-btn${target === t.id ? ' sheet-effort-active' : ''}`}
              data-testid={`ssh-target-${t.id}`}
              onClick={() => setTarget(t.id)}
            >{t.label}</button>
          ))}
        </div>
        <div class="ssh-target-desc">
          {TARGETS.find(t => t.id === target)?.desc}
        </div>
        {!paneAllowed && canSsh && canSsh.note && (
          <div class="ssh-target-blocked" data-testid="ssh-pane-blocked">
            {canSsh.note}
          </div>
        )}

        <div class="sheet-section-label" style="margin-top:16px">SSH HOST</div>
        {error && <div class="ssh-sheet-error" data-testid="ssh-sheet-error">{error}</div>}
        {hosts === null && !error && (
          <div class="ssh-sheet-empty"><div class="spinner" /></div>
        )}
        {hosts !== null && hosts.length === 0 && (
          <div class="ssh-sheet-empty" data-testid="ssh-hosts-empty">
            ~/.ssh/config に Host がありません
          </div>
        )}
        {hosts !== null && hosts.length > 0 && (
          <div class="sheet-model-list ssh-host-list">
            {hosts.map(h => (
              <div
                key={h.name}
                class="sheet-model-item"
                data-testid={`ssh-host-${h.name}`}
                onClick={() => connect(h)}
              >
                <div class="sheet-model-info">
                  <span class="sheet-model-name">{h.name}</span>
                  <span class="sheet-model-desc">{hostLine(h)}</span>
                </div>
                {busy === h.name && <span class="ssh-bar-spinner" style="margin-left:auto" />}
              </div>
            ))}
          </div>
        )}
        <div class="sheet-footer-note">
          接続に失敗してもペインは残ります（理由がその場に出ます）
        </div>
      </div>
    </div>
  );
}
