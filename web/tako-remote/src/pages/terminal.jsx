import { useState, useEffect, useRef, useCallback } from 'preact/hooks';
import { getActiveMachine } from '../store';
import { createClient } from '../api';

const QUICK_KEYS = [
  { label: 'Tab',    seq: '\t',    newline: false },
  { label: 'Ctrl+C', seq: '\x03', newline: false },
  { label: 'Ctrl+D', seq: '\x04', newline: false },
  { label: 'Ctrl+Z', seq: '\x1a', newline: false },
  { label: '↑',      seq: '\x1b[A', newline: false },
  { label: '↓',      seq: '\x1b[B', newline: false },
  { label: 'Esc',    seq: '\x1b',   newline: false },
];

export function TerminalPage({ paneId }) {
  const [lines, setLines] = useState([]);
  const [input, setInput] = useState('');
  const [loading, setLoading] = useState(true);
  const [info, setInfo] = useState(null);
  const [allPanes, setAllPanes] = useState([]);
  const machine = getActiveMachine();
  const outputRef = useRef(null);
  const inputRef = useRef(null);
  const timerRef = useRef(null);
  const touchRef = useRef({ x: 0, y: 0 });

  const clientRef = useRef(null);
  if (machine && !clientRef.current) {
    clientRef.current = createClient(machine.host, machine.token);
  }

  const refresh = useCallback(async () => {
    if (!clientRef.current) return;
    try {
      const [screen, panesList] = await Promise.all([
        clientRef.current.screen(paneId, 200),
        clientRef.current.panes(),
      ]);
      setLines(screen.lines || []);
      const list = panesList.panes || [];
      setAllPanes(list);
      setInfo(list.find(p => p.id === paneId) || null);
      setLoading(false);
    } catch {
      setLoading(false);
    }
  }, [paneId]);

  useEffect(() => {
    if (!machine) { window.location.hash = '#/'; return; }
    clientRef.current = createClient(machine.host, machine.token);
    setLoading(true);
    refresh();
    timerRef.current = setInterval(refresh, 2000);
    return () => clearInterval(timerRef.current);
  }, [paneId, refresh]);

  useEffect(() => {
    const el = outputRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [lines]);

  async function send() {
    const text = input.trim();
    if (!text || !clientRef.current) return;
    setInput('');
    try {
      await clientRef.current.input(paneId, text, true);
      setTimeout(refresh, 300);
    } catch { /* ignore */ }
    inputRef.current?.focus();
  }

  async function sendKey(seq, newline) {
    if (!clientRef.current) return;
    try {
      await clientRef.current.input(paneId, seq, newline);
      setTimeout(refresh, 300);
    } catch { /* ignore */ }
  }

  function onKeyDown(e) {
    if (e.key === 'Enter') { e.preventDefault(); send(); }
  }

  // スワイプでペイン間移動
  function onTouchStart(e) {
    touchRef.current = { x: e.touches[0].clientX, y: e.touches[0].clientY };
  }

  function onTouchEnd(e) {
    const dx = e.changedTouches[0].clientX - touchRef.current.x;
    const dy = e.changedTouches[0].clientY - touchRef.current.y;
    if (Math.abs(dx) < 80 || Math.abs(dx) < Math.abs(dy) * 1.5) return;
    const idx = allPanes.findIndex(p => p.id === paneId);
    if (idx < 0) return;
    if (dx > 0 && idx > 0) {
      window.location.hash = `#/panes/${allPanes[idx - 1].id}`;
    } else if (dx < 0 && idx < allPanes.length - 1) {
      window.location.hash = `#/panes/${allPanes[idx + 1].id}`;
    }
  }

  if (!machine) return null;

  const idx = allPanes.findIndex(p => p.id === paneId);
  const pos = idx >= 0 ? `${idx + 1}/${allPanes.length}` : '';

  return (
    <div class="page terminal-page" onTouchStart={onTouchStart} onTouchEnd={onTouchEnd}>
      <header class="terminal-header">
        <button class="back-btn" onClick={() => { window.location.hash = '#/panes'; }}>
          <svg width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M15 18l-6-6 6-6"/></svg>
        </button>
        <div class="terminal-title-bar">
          <span class="terminal-name">{info?.title || `Pane ${paneId}`}</span>
          {pos && <span class="badge">{pos}</span>}
        </div>
      </header>

      <div class="terminal-output" ref={outputRef}>
        {loading ? (
          <div class="center-fill"><div class="spinner" /></div>
        ) : (
          lines.map((line, i) => <div key={i} class="mono-line">{line || ' '}</div>)
        )}
      </div>

      <div class="quick-keys">
        {QUICK_KEYS.map(k => (
          <button key={k.label} class="quick-key" onClick={() => sendKey(k.seq, k.newline)}>
            {k.label}
          </button>
        ))}
      </div>

      <div class="input-bar">
        <input
          ref={inputRef}
          type="text"
          class="input-field"
          value={input}
          onInput={e => setInput(e.target.value)}
          onKeyDown={onKeyDown}
          placeholder="コマンドを入力..."
          autocomplete="off"
          autocorrect="off"
          autocapitalize="off"
          spellcheck={false}
        />
        <button class="send-btn" onClick={send} disabled={!input.trim()}>
          <svg width="20" height="20" viewBox="0 0 24 24" fill="currentColor"><path d="M2.01 21L23 12 2.01 3 2 10l15 2-15 2z"/></svg>
        </button>
      </div>
    </div>
  );
}
