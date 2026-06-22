import { useState, useEffect, useRef } from 'preact/hooks';
import { getActiveMachine } from '../store';
import { createClient } from '../api';

export function PanesPage() {
  const [panes, setPanes] = useState([]);
  const [previews, setPreviews] = useState({});
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState(null);
  const machine = getActiveMachine();
  const timerRef = useRef(null);

  useEffect(() => {
    if (!machine) { window.location.hash = '#/'; return; }

    const client = createClient(machine.host, machine.token);

    async function refresh() {
      try {
        const result = await client.panes();
        const list = result.panes || [];
        setPanes(list);
        setLoading(false);
        setError(null);

        for (const p of list) {
          client.screen(p.id, 5)
            .then(s => setPreviews(prev => ({ ...prev, [p.id]: s.lines || [] })))
            .catch(() => {});
        }
      } catch (e) {
        setError(e.message);
        setLoading(false);
      }
    }

    refresh();
    timerRef.current = setInterval(refresh, 3000);
    return () => clearInterval(timerRef.current);
  }, []);

  if (!machine) return null;

  return (
    <div class="page">
      <header class="page-header">
        <button class="back-btn" onClick={() => { window.location.hash = '#/'; }}>
          <svg width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M15 18l-6-6 6-6"/></svg>
        </button>
        <div>
          <h1>{machine.name}</h1>
          <p class="subtitle">{panes.length} ペイン</p>
        </div>
      </header>

      {loading ? (
        <div class="center-fill"><div class="spinner" /></div>
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
        <div class="card-list">
          {panes.map(p => (
            <div key={p.id} class="pane-card" onClick={() => { window.location.hash = `#/panes/${p.id}`; }}>
              <div class="pane-card-header">
                <span class={`dot ${p.state || 'idle'}`} />
                <span class="pane-card-title">{p.title || `Pane ${p.id}`}</span>
                <span class="pane-card-id">#{p.id}</span>
              </div>
              <div class="pane-card-preview">
                {(previews[p.id] || []).map((line, i) => (
                  <div key={i} class="mono-line">{line || ' '}</div>
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
