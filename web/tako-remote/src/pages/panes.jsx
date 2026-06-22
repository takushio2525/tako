import { useState, useEffect, useRef, useCallback } from 'preact/hooks';
import { getActiveMachine } from '../store';
import { createClient } from '../api';

function SkeletonCard() {
  return (
    <div class="pane-card skeleton-card">
      <div class="pane-card-header">
        <span class="skeleton skeleton-dot" />
        <span class="skeleton skeleton-text" style="width: 60%" />
        <span class="skeleton skeleton-text" style="width: 30px" />
      </div>
      <div class="pane-card-preview">
        <div class="skeleton skeleton-line" />
        <div class="skeleton skeleton-line" style="width: 80%" />
        <div class="skeleton skeleton-line" style="width: 50%" />
      </div>
    </div>
  );
}

const PULL_THRESHOLD = 80;

export function PanesPage() {
  const [panes, setPanes] = useState([]);
  const [previews, setPreviews] = useState({});
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState(null);
  const [pulling, setPulling] = useState(false);
  const [pullY, setPullY] = useState(0);
  const machine = getActiveMachine();
  const timerRef = useRef(null);
  const touchStartRef = useRef({ y: 0, scrollTop: 0 });
  const listRef = useRef(null);

  const refresh = useCallback(async (client) => {
    const c = client || (machine && createClient(machine.host, machine.token));
    if (!c) return;
    try {
      const result = await c.panes();
      const list = result.panes || [];
      setPanes(list);
      setLoading(false);
      setError(null);

      for (const p of list) {
        c.screen(p.id, 5)
          .then(s => setPreviews(prev => ({ ...prev, [p.id]: s.lines || [] })))
          .catch(() => {});
      }
    } catch (e) {
      setError(e.message);
      setLoading(false);
    }
  }, [machine]);

  useEffect(() => {
    if (!machine) { window.location.hash = '#/'; return; }
    const client = createClient(machine.host, machine.token);
    refresh(client);
    timerRef.current = setInterval(() => refresh(client), 3000);
    return () => clearInterval(timerRef.current);
  }, []);

  // プルダウンリフレッシュ
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

  if (!machine) return null;

  return (
    <div class="page">
      <header class="page-header">
        <button class="back-btn" onClick={() => { window.location.hash = '#/'; }}>
          <svg width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M15 18l-6-6 6-6"/></svg>
        </button>
        <div>
          <h1>{machine.name}</h1>
          <p class="subtitle">{loading ? '読み込み中...' : `${panes.length} ペイン`}</p>
        </div>
      </header>

      {pulling && (
        <div class="pull-indicator"><div class="spinner" /></div>
      )}

      {loading ? (
        <div class="card-list">
          <SkeletonCard />
          <SkeletonCard />
          <SkeletonCard />
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
          style={pullY > 0 ? `transform: translateY(${pullY}px)` : ''}
        >
          {panes.map(p => (
            <div key={p.id} class="pane-card" onClick={() => { window.location.hash = `#/panes/${p.id}`; }}>
              <div class="pane-card-header">
                <span class={`dot ${p.state || 'idle'}`} />
                <span class="pane-card-title">{p.title || `Pane ${p.id}`}</span>
                <span class="pane-card-id">#{p.id}</span>
              </div>
              <div class="pane-card-preview">
                {(previews[p.id] || []).map((line, i) => (
                  <div key={i} class="mono-line">{line || ' '}</div>
                ))}
                {!previews[p.id] && <div class="mono-line faded">読み込み中...</div>}
              </div>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
