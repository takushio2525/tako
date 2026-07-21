import { useState, useEffect, useRef, useCallback } from 'preact/hooks';
import { createClient } from '../api';
import { AgentIcon, agentColor } from '../components/agent-icon';

function SkeletonCard() {
  return (
    <div class="pane-card skeleton-card" style="opacity: .5;">
      <div class="pane-card-header">
        <div class="pane-card-left">
          <span class="skeleton skeleton-dot" />
          <span class="skeleton skeleton-text" style="width: 120px" />
        </div>
        <span class="skeleton skeleton-text" style="width: 40px" />
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

function stateOf(p) {
  if (p.state === 'error' || p.exit_code) return 'error';
  // permission ダイアログ実在 = ユーザーの承認待ち（#425。サーバーが画面から検知）
  if (p.permission_dialog || p.state === 'busy' || p.state === 'needs_input') return 'busy';
  if (p.state === 'running') return 'running';
  return 'idle';
}

function stateLabel(st) {
  switch (st) {
    case 'error': return 'error';
    case 'busy': return 'needs input';
    case 'running': return 'running';
    default: return 'idle';
  }
}

const PULL_THRESHOLD = 80;

export function PanesPage({ me }) {
  const [panes, setPanes] = useState([]);
  const [previews, setPreviews] = useState({});
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

      for (const p of list) {
        c.screen(p.tmux_target || p.id, 5)
          .then(s => setPreviews(prev => ({ ...prev, [p.id]: s.lines || [] })))
          .catch(() => {});
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
    timerRef.current = setInterval(() => refresh(client), 3000);
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

  const counts = { all: panes.length, busy: 0, running: 0, idle: 0, error: 0 };
  panes.forEach(p => { counts[stateOf(p)]++; });
  const filtered = filter === 'all' ? panes : panes.filter(p => stateOf(p) === filter);

  return (
    <div class="page">
      <div class="panes-header">
        <div class="panes-header-row">
          <div class="machine-chip">
            <span class="dot online" style="width: 7px; height: 7px;" />
            <span class="chip-name">{(me && me.host) || 'tako'}</span>
          </div>
        </div>
        <div style="display: flex; gap: 7px; overflow-x: auto;">
          <button class={`filter-chip${filter === 'all' ? ' active' : ''}`} onClick={() => setFilter('all')}>
            all {counts.all}
          </button>
          {counts.busy > 0 && (
            <button class={`filter-chip${filter === 'busy' ? ' active' : ''}`} onClick={() => setFilter('busy')}>
              <span class="chip-dot" style="background: var(--amber);" />
              needs you {counts.busy}
            </button>
          )}
          {counts.running > 0 && (
            <button class={`filter-chip${filter === 'running' ? ' active' : ''}`} onClick={() => setFilter('running')}>
              <span class="chip-dot" style="background: var(--green);" />
              running {counts.running}
            </button>
          )}
          {counts.error > 0 && (
            <button class={`filter-chip${filter === 'error' ? ' active' : ''}`} onClick={() => setFilter('error')}>
              <span class="chip-dot" style="background: var(--red);" />
              failed {counts.error}
            </button>
          )}
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
          <button class="btn btn-primary" onClick={() => { window.location.hash = '#/'; }}>戻る</button>
        </div>
      ) : panes.length === 0 ? (
        <div class="empty-state">
          <h2>ペインなし</h2>
          <p>アクティブなペインがありません</p>
        </div>
      ) : (
        <div
          class="card-list"
          ref={listRef}
          onTouchStart={onTouchStart}
          onTouchMove={onTouchMove}
          onTouchEnd={onTouchEnd}
          style={`padding-top: 14px;${pullY > 0 ? ` transform: translateY(${pullY}px)` : ''}`}
        >
          {filtered.map(p => {
            const st = stateOf(p);
            const agentType = p.agent_type || 'plain';
            const displayTitle = p.title || `Pane ${p.id}`;
            const cardTitle = agentType !== 'plain'
              ? `${agentType} · ${displayTitle}`
              : displayTitle;

            return (
              <div key={p.id} class={`pane-card state-${st}`} onClick={() => { window.location.hash = `#/panes/${p.id}`; }}>
                <div class="edge-bar" />
                <div class="pane-card-header">
                  <div class="pane-card-left">
                    <AgentIcon type={agentType} />
                    <span class="pane-card-title">{cardTitle}</span>
                  </div>
                  <span class="state-badge">{stateLabel(st)}</span>
                </div>
                <div class="pane-card-meta">
                  {(me && me.host) || 'tako'}
                  {p.position ? ` · ${p.position}` : ''}
                  {p.role ? ` · ${p.role}` : ''}
                </div>
                <div class="pane-card-preview">
                  <div class="pane-card-preview-box">
                    {(previews[p.id] || []).map((line, i) => (
                      <div key={i} class="mono-line">{line || ' '}</div>
                    ))}
                    {!previews[p.id] && <div class="mono-line faded">...</div>}
                  </div>
                </div>
                <div class="pane-card-footer">
                  <span class="footer-meta">#{p.id}</span>
                  <span class="footer-action">{st === 'busy' ? 'respond →' : 'view →'}</span>
                </div>
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
}
