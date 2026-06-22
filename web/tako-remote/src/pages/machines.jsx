import { useState, useEffect } from 'preact/hooks';
import { getMachines, removeMachine, setActiveMachine } from '../store';
import { createClient } from '../api';

export function MachinesPage() {
  const [machines, setMachines] = useState(getMachines);
  const [statuses, setStatuses] = useState({});
  const [deleteTarget, setDeleteTarget] = useState(null);

  useEffect(() => {
    machines.forEach(m => {
      setStatuses(prev => ({ ...prev, [m.id]: 'checking' }));
      createClient(m.host, m.token).health()
        .then(() => setStatuses(prev => ({ ...prev, [m.id]: 'online' })))
        .catch(() => setStatuses(prev => ({ ...prev, [m.id]: 'offline' })));
    });
  }, [machines]);

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
    if (min < 1) return 'たった今';
    if (min < 60) return `${min}分前`;
    const h = Math.floor(min / 60);
    if (h < 24) return `${h}時間前`;
    return `${Math.floor(h / 24)}日前`;
  }

  return (
    <div class="page">
      <header class="page-header">
        <div>
          <h1>tako remote</h1>
          <p class="subtitle">登録済みマシン</p>
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
        <div class="card-list">
          {machines.map(m => (
            <div key={m.id} class="card" onClick={() => connect(m)}>
              <div class="card-body">
                <div class="card-title">
                  <span class={`dot ${statuses[m.id] || 'checking'}`} />
                  {m.name}
                </div>
                <div class="card-meta">
                  {statuses[m.id] === 'online' ? 'オンライン' :
                   statuses[m.id] === 'offline' ? 'オフライン' : '確認中...'}
                  {' · '}{timeAgo(m.lastSeen)}
                </div>
              </div>
              <button class="icon-btn" onClick={e => { e.stopPropagation(); setDeleteTarget(m.id); }}>
                <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                  <path d="M3 6h18M8 6V4a2 2 0 012-2h4a2 2 0 012 2v2m3 0v14a2 2 0 01-2 2H7a2 2 0 01-2-2V6h14"/>
                </svg>
              </button>
            </div>
          ))}
        </div>
      )}

      {deleteTarget && (
        <div class="overlay" onClick={() => setDeleteTarget(null)}>
          <div class="dialog" onClick={e => e.stopPropagation()}>
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
