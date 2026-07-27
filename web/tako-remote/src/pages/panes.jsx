// ペイン選択画面（#621）。
//
// 設計のゴールはただ一つ「どれがどれだか分かる」こと。そのために出すのは
//   1. 中身のスニペット — daemon が TUI のクロムを落として返す `preview`
//   2. 誰か           — role（master / solo / worker / user）+ agent 種別 + cwd
//   3. 今どうなのか   — activity（承認待ち / 実行中 / 待機 / 停止）
// さらにタブでグループ化して「どのタブに何がいるか」を俯瞰できるようにしている。
//
// スニペットは daemon の `/api/v2/panes` に同梱される（#621 で N+1 リクエストを解消）。
// 古い daemon やキャプチャ失敗でフィールドが無いときだけ screen API へフォールバックする。
import { useState, useEffect, useRef, useCallback } from 'preact/hooks';
import { createClient } from '../api';
import { AgentIcon } from '../components/agent-icon';

const PREVIEW_LINES = 8;
const PULL_THRESHOLD = 80;
const POLL_MS = 3000;

// --- ペインの分類 ---

// 状態は daemon が画面から導いた activity を最優先で使う。
// activity が無いペイン（素のシェル）は OSC 133 の state が正（#621）
function stateOf(p) {
  switch (p.activity) {
    case 'permission': return 'permission';
    case 'busy': return 'busy';
    case 'error': return 'error';
    case 'idle': return 'idle';
    default: break;
  }
  // 旧 daemon 互換: permission_dialog だけが来ることがある
  if (p.permission_dialog) return 'permission';
  if (p.state === 'failed' || p.exit_code) return 'error';
  if (p.state === 'running') return 'busy';
  return 'idle';
}

const STATE_LABEL = {
  permission: '承認待ち',
  busy: '実行中',
  error: '停止',
  idle: '待機',
};

// 要対応 = 人が見ないと進まないもの
function needsYou(st) {
  return st === 'permission' || st === 'error';
}

function roleCategory(p) {
  const role = (p.role || '').toLowerCase();
  // worker の role は `orchestrator-worker-<agent>` で master / solo を含まないため
  // 先に判定してよい。master / solo は `master:<suffix>` 形式も取る（#210）
  if (role.includes('worker')) return 'worker';
  if (role.includes('master')) return 'master';
  if (role.includes('solo')) return 'solo';
  return 'user';
}

const ROLE_LABEL = { master: 'master', solo: 'solo', worker: 'worker', user: 'shell' };

// worker の担当を示すラベル（`orchestrator-worker-claude:fix-auth` の末尾など）
function workerLabel(p) {
  const role = p.role || '';
  const idx = role.indexOf(':');
  return idx >= 0 ? role.slice(idx + 1) : '';
}

// cwd はフルパスだと横に溢れるうえ識別に効かないので末尾 2 階層だけ出す
function shortCwd(cwd) {
  if (!cwd) return '';
  const parts = cwd.replace(/\/+$/, '').split('/').filter(Boolean);
  if (parts.length === 0) return '/';
  return parts.slice(-2).join('/');
}

// 画面末尾から意味のある行だけを PREVIEW_LINES 行取る（screen API
// フォールバック用）。daemon の `remote_preview` と同じ「末尾を見せる」方針。
// 旧実装は配列の先頭から描画していたため、カードには**最も古い履歴**が出ていた
function tailLines(lines) {
  const trimmed = [...lines];
  while (trimmed.length && !trimmed[trimmed.length - 1].trim()) trimmed.pop();
  return trimmed.slice(-PREVIEW_LINES);
}

function groupByTab(panes) {
  const groups = [];
  const index = new Map();
  for (const p of panes) {
    const key = p.tab_id ?? p.tab_title ?? '-';
    if (!index.has(key)) {
      index.set(key, groups.length);
      groups.push({ key, title: p.tab_title || 'tab', panes: [] });
    }
    groups[index.get(key)].panes.push(p);
  }
  return groups;
}

// --- 部品 ---

function SkeletonCard() {
  return (
    <div class="pane-card skeleton-card">
      <div class="pane-card-header">
        <div class="pane-card-left">
          <span class="skeleton skeleton-icon" />
          <span class="skeleton skeleton-text" style="width: 120px" />
        </div>
        <span class="skeleton skeleton-text" style="width: 46px" />
      </div>
      <div class="pane-card-preview">
        <div class="pane-card-preview-box">
          <div class="skeleton skeleton-line" />
          <div class="skeleton skeleton-line" style="width: 80%" />
          <div class="skeleton skeleton-line" style="width: 50%" />
        </div>
      </div>
    </div>
  );
}

function StatusPill({ state }) {
  return (
    <span class={`status-pill state-${state}`}>
      <span class="status-pill-dot" />
      {STATE_LABEL[state] || state}
    </span>
  );
}

function PreviewBox({ pane, fallback }) {
  if (!pane.tmux_target) {
    return (
      <div class="pane-card-no-terminal">
        <NoTerminalIcon />
        <span>ターミナルなし（プレビュー等）</span>
      </div>
    );
  }
  // daemon 同梱の preview が正。無ければ screen API のフォールバック結果
  const lines = Array.isArray(pane.preview) ? pane.preview : fallback;
  if (lines === undefined) {
    return <div class="pane-card-preview-box"><div class="mono-line faded">読み込み中...</div></div>;
  }
  if (lines === null) {
    return (
      <div class="pane-card-preview-box preview-unavailable">
        <div class="mono-line faded">画面を取得できませんでした</div>
      </div>
    );
  }
  if (lines.length === 0) {
    return (
      <div class="pane-card-preview-box preview-empty">
        <div class="mono-line faded">出力はまだありません</div>
      </div>
    );
  }
  // 溢れるときだけ下端をフェードさせる（溢れていないのに掛けると
  // 最後の 1 行がただ薄く見えて「読めない行がある」と誤解させる）
  const cls = estimatedRows(lines) > VISIBLE_ROWS ? ' has-more' : '';
  return (
    <div class={`pane-card-preview-box${cls}`}>
      {lines.map((line, i) => (
        <div key={i} class="mono-line">{line || ' '}</div>
      ))}
    </div>
  );
}

// スニペット枠に収まる行数と、折り返しを含めた実表示行数の見積もり。
// 端末幅 390px・10.5px の等幅で 1 行あたりおよそ 44 桁
const VISIBLE_ROWS = 8;
const WRAP_COLS = 44;
function estimatedRows(lines) {
  return lines.reduce((sum, l) => sum + Math.max(1, Math.ceil(l.length / WRAP_COLS)), 0);
}

function PaneCard({ pane, fallback, onOpen }) {
  const st = stateOf(pane);
  const cat = roleCategory(pane);
  const agentType = pane.agent_type || 'plain';
  const label = workerLabel(pane);
  const cwd = shortCwd(pane.cwd);

  return (
    <div
      class={`pane-card state-${st} role-${cat}`}
      data-pane-id={pane.id}
      onClick={onOpen}
    >
      <div class="edge-bar" />
      <div class="pane-card-header">
        <div class="pane-card-left">
          <AgentIcon type={agentType} />
          <div class="pane-card-titles">
            <span class="pane-card-title">{pane.title || `Pane ${pane.id}`}</span>
            <span class="pane-card-sub">
              #{pane.id}{cwd ? ` · ${cwd}` : ''}
            </span>
          </div>
        </div>
        <StatusPill state={st} />
      </div>

      <div class="pane-card-chips">
        {/* ターミナルを持たないペイン（プレビュー等）に shell と出すと嘘になる */}
        {(pane.tmux_target || cat !== 'user') && (
          <span class={`card-chip card-chip-role role-${cat}`}>{ROLE_LABEL[cat]}</span>
        )}
        {agentType !== 'plain' && (
          <span class="card-chip card-chip-agent">{agentType}</span>
        )}
        {pane.model && <span class="card-chip">{pane.model}</span>}
        {label && <span class="card-chip card-chip-task">{label}</span>}
      </div>

      {pane.permission_dialog && (
        <div class="card-permission">
          <span class="card-permission-label">承認待ち</span>
          <code class="card-permission-cmd">{pane.permission_dialog.command || '確認'}</code>
        </div>
      )}
      {pane.error && (
        <div class="card-error">
          <span class="card-error-kind">{pane.error.kind}</span>
          <span class="card-error-detail">{pane.error.detail}</span>
        </div>
      )}

      <div class="pane-card-preview">
        <PreviewBox pane={pane} fallback={fallback} />
      </div>

      <div class="pane-card-footer">
        <span class="footer-meta">{pane.position ? `pane ${pane.position}` : ''}</span>
        <span class="footer-action">
          {st === 'permission' ? '応答する' : st === 'error' ? '確認する' : '開く'}
          <span class="footer-arrow">›</span>
        </span>
      </div>
    </div>
  );
}

// --- 画面本体 ---

export function PanesPage({ me }) {
  const [panes, setPanes] = useState([]);
  const [fallbacks, setFallbacks] = useState({});
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState(null);
  const [pulling, setPulling] = useState(false);
  const [pullY, setPullY] = useState(0);
  const [filter, setFilter] = useState('all');
  const timerRef = useRef(null);
  const touchStartRef = useRef({ y: 0, scrollTop: 0 });
  const listRef = useRef(null);

  const refresh = useCallback(async (client) => {
    const c = client || createClient();
    try {
      const result = await c.panes();
      const list = result.panes || [];
      setPanes(list);
      setLoading(false);
      setError(null);

      // daemon が preview を返せなかったペインだけ screen API で補う
      for (const p of list) {
        if (Array.isArray(p.preview) || !p.tmux_target) continue;
        c.screen(p.tmux_target || p.id)
          .then(s => setFallbacks(prev => ({ ...prev, [p.id]: tailLines(s.lines || []) })))
          .catch(() => setFallbacks(prev => ({ ...prev, [p.id]: null })));
      }
    } catch (e) {
      if (e.status === 403) { window.location.reload(); return; }
      setError(e.message);
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    const client = createClient();
    refresh(client);
    timerRef.current = setInterval(() => refresh(client), POLL_MS);
    return () => clearInterval(timerRef.current);
  }, []);

  function onTouchStart(e) {
    const el = listRef.current;
    touchStartRef.current = { y: e.touches[0].clientY, scrollTop: el?.scrollTop || 0 };
  }
  function onTouchMove(e) {
    const el = listRef.current;
    if (!el || touchStartRef.current.scrollTop > 0) return;
    const dy = e.touches[0].clientY - touchStartRef.current.y;
    if (dy > 0 && el.scrollTop <= 0) {
      setPullY(Math.min(dy * 0.4, 100));
      if (dy > 10) e.preventDefault();
    }
  }
  function onTouchEnd() {
    if (pullY >= PULL_THRESHOLD) {
      setPulling(true);
      setPullY(0);
      refresh().then(() => setPulling(false));
    } else {
      setPullY(0);
    }
  }

  const counts = { all: panes.length, needs: 0, busy: 0, idle: 0 };
  panes.forEach(p => {
    const st = stateOf(p);
    if (needsYou(st)) counts.needs++;
    else if (st === 'busy') counts.busy++;
    else counts.idle++;
  });

  const matches = (p) => {
    const st = stateOf(p);
    if (filter === 'all') return true;
    if (filter === 'needs') return needsYou(st);
    if (filter === 'busy') return st === 'busy';
    return st === 'idle';
  };
  const groups = groupByTab(panes.filter(matches));

  const FILTERS = [
    { key: 'all', label: 'すべて', count: counts.all, color: null },
    { key: 'needs', label: '要対応', count: counts.needs, color: 'var(--amber)' },
    { key: 'busy', label: '実行中', count: counts.busy, color: 'var(--green)' },
    { key: 'idle', label: '待機', count: counts.idle, color: 'var(--fg3)' },
  ];

  return (
    <div class="page">
      <div class="panes-header">
        <div class="panes-header-row">
          <div class="machine-chip">
            <span class="dot online" style="width: 7px; height: 7px;" />
            <span class="chip-name">{(me && me.host) || 'tako'}</span>
          </div>
          <button
            class={`refresh-btn${pulling ? ' spinning' : ''}`}
            aria-label="更新"
            onClick={() => { setPulling(true); refresh().then(() => setPulling(false)); }}
          >
            <RefreshIcon />
          </button>
        </div>
        <div class="filter-row">
          {FILTERS.map(f => (
            (f.key === 'all' || f.count > 0) && (
              <button
                key={f.key}
                class={`filter-chip${filter === f.key ? ' active' : ''}`}
                onClick={() => setFilter(f.key)}
              >
                {f.color && <span class="chip-dot" style={`background: ${f.color};`} />}
                {f.label} {f.count}
              </button>
            )
          ))}
        </div>
      </div>

      {pulling && <div class="pull-indicator"><div class="spinner" /></div>}

      {loading ? (
        <div class="card-list" style="padding-top: 14px;">
          <SkeletonCard /><SkeletonCard /><SkeletonCard />
        </div>
      ) : error ? (
        <div class="center-fill">
          <p class="error-text">{error}</p>
          <button class="btn btn-primary" onClick={() => refresh()}>再試行</button>
        </div>
      ) : panes.length === 0 ? (
        <div class="empty-state">
          <h2>ペインがありません</h2>
          <p>Mac で tako を起動すると、タブとペインがここに並びます。</p>
        </div>
      ) : groups.length === 0 ? (
        <div class="empty-state">
          <h2>該当なし</h2>
          <p>この絞り込みに合うペインはありません。</p>
          <button class="btn" style="margin-top: 16px;" onClick={() => setFilter('all')}>
            すべて表示
          </button>
        </div>
      ) : (
        <div
          class="card-list"
          ref={listRef}
          onTouchStart={onTouchStart}
          onTouchMove={onTouchMove}
          onTouchEnd={onTouchEnd}
          style={pullY > 0 ? `transform: translateY(${pullY}px)` : ''}
        >
          {groups.map(group => (
            <div class="tab-group" key={group.key}>
              <div class="tab-group-header">
                <span class="tab-group-name">{group.title}</span>
                <span class="tab-group-count">{group.panes.length}</span>
                <span class="tab-group-states">
                  {group.panes.map(p => (
                    <span key={p.id} class={`group-dot state-${stateOf(p)}`} />
                  ))}
                </span>
              </div>
              {group.panes.map(p => (
                <PaneCard
                  key={p.id}
                  pane={p}
                  fallback={fallbacks[p.id]}
                  onOpen={() => { window.location.hash = `#/panes/${p.id}`; }}
                />
              ))}
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

function RefreshIcon() {
  return (
    <svg width="16" height="16" viewBox="0 0 16 16" fill="none">
      <path
        d="M13.5 8a5.5 5.5 0 1 1-1.61-3.89"
        stroke="currentColor"
        stroke-width="1.4"
        stroke-linecap="round"
      />
      <path d="M13.6 2.2v3.1h-3.1z" fill="currentColor" />
    </svg>
  );
}

function NoTerminalIcon() {
  return (
    <svg width="16" height="16" viewBox="0 0 16 16" fill="none" style="flex-shrink: 0;">
      <rect x="1.5" y="3.5" width="13" height="9" rx="1.5" stroke="currentColor" stroke-width="1.2" />
      <path d="M4.5 6.5L7 9L4.5 11.5" stroke="currentColor" stroke-width="1.2" stroke-linecap="round" stroke-linejoin="round" />
      <line x1="8.5" y1="11.5" x2="11.5" y2="11.5" stroke="currentColor" stroke-width="1.2" stroke-linecap="round" />
      <line x1="1" y1="15" x2="15" y2="1" stroke="currentColor" stroke-width="1.2" stroke-linecap="round" />
    </svg>
  );
}
