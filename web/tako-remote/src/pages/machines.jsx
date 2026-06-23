import { useState, useEffect } from 'preact/hooks';
import { getMachines, removeMachine, setActiveMachine, updateMachineHost } from '../store';
import { createClient, resolveHost } from '../api';

export function MachinesPage() {
  const [machines, setMachines] = useState(getMachines);
  const [statuses, setStatuses] = useState({});
  const [deleteTarget, setDeleteTarget] = useState(null);

  useEffect(() => {
    machines.forEach(async (m) => {
      setStatuses(prev => ({ ...prev, [m.id]: 'checking' }));
      try {
        await createClient(m.host, m.token).health();
        setStatuses(prev => ({ ...prev, [m.id]: 'online' }));
        return;
      } catch {}

      const resolved = await resolveHost(m.id);
      if (resolved && resolved !== m.host) {
        try {
          await createClient(resolved, m.token).health();
          updateMachineHost(m.id, resolved);
          setMachines(getMachines());
          setStatuses(prev => ({ ...prev, [m.id]: 'online' }));
          return;
        } catch {}
      }
      setStatuses(prev => ({ ...prev, [m.id]: 'offline' }));
    });
  }, [machines.length]);

  function connect(m) {
    setActiveMachine(m.id);
    window.location.hash = '#/panes';
  }

  function doDelete(id) {
    removeMachine(id);
    setMachines(getMachines());
    setDeleteTarget(null);
  }

  function timeAgo(ts) {
    const d = Date.now() - ts;
    const min = Math.floor(d / 60000);
    if (min < 1) return 'just now';
    if (min < 60) return `${min}m ago`;
    const h = Math.floor(min / 60);
    if (h < 24) return `${h}h ago`;
    return `${Math.floor(h / 24)}d ago`;
  }

  const firstOnline = machines.find(m => statuses[m.id] === 'online');

  return (
    <div class="page">
      <header class="page-header" style="padding: 8px 18px 20px; justify-content: space-between;">
        <div style="display: flex; align-items: baseline; gap: 10px;">
          <h1>tako</h1>
          <span class="host-count">{machines.length} host{machines.length !== 1 ? 's' : ''}</span>
        </div>
      </header>

      {machines.length === 0 ? (
        <div class="empty-state">
          <div class="empty-icon">📱</div>
          <h2>マシン未登録</h2>
          <p>
            Mac で <code>tako remote start</code> を実行し、
            表示される QR コードをスキャンしてください
          </p>
        </div>
      ) : (
        <>
          <div class="card-list">
            {machines.map(m => {
              const st = statuses[m.id] || 'checking';
              const isOffline = st === 'offline';
              return (
                <div
                  key={m.id}
                  class={`machine-card ${st}${isOffline ? ' machine-card-offline' : ''}`}
                  onClick={() => connect(m)}
                >
                  <div class="edge-bar" />
                  <div class="machine-card-head">
                    <div class="machine-card-name">
                      <span class={`dot ${st}`} />
                      <span class="name">{m.name}</span>
                    </div>
                    <div style="display: flex; align-items: center; gap: 8px;">
                      <span class="status-label">
                        {st === 'online' ? 'connected' :
                         st === 'offline' ? 'offline' : 'checking...'}
                      </span>
                      <button
                        class="delete-btn"
                        onClick={e => { e.stopPropagation(); setDeleteTarget(m.id); }}
                      >
                        <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                          <path d="M3 6h18M8 6V4a2 2 0 012-2h4a2 2 0 012 2v2m3 0v14a2 2 0 01-2 2H7a2 2 0 01-2-2V6h14"/>
                        </svg>
                      </button>
                    </div>
                  </div>
                  <div class="host-info">
                    {m.host ? m.host.replace(/^https?:\/\//, '').replace(/\/$/, '').substring(0, 32) : '—'}
                    {' · '}{timeAgo(m.lastSeen)}
                  </div>
                </div>
              );
            })}
          </div>

          {firstOnline && (
            <div style="flex: none; padding: 14px 18px 30px; background: linear-gradient(0deg, var(--bg) 60%, transparent);">
              <button class="cta-button" onClick={() => connect(firstOnline)}>
                <span class="cta-label">Open {firstOnline.name}</span>
                <span class="cta-arrow">→</span>
              </button>
            </div>
          )}
        </>
      )}

      {deleteTarget && (
        <div class="overlay" onClick={() => setDeleteTarget(null)}>
          <div class="dialog" onClick={e => e.stopPropagation()}>
            <div class="dialog-handle" />
            <p>このマシンを削除しますか？</p>
            <div class="dialog-actions">
              <button class="btn" onClick={() => setDeleteTarget(null)}>キャンセル</button>
              <button class="btn btn-danger" onClick={() => doDelete(deleteTarget)}>削除</button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
