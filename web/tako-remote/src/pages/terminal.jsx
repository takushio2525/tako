import { useState, useEffect, useRef, useCallback } from 'preact/hooks';
import { Terminal } from '@xterm/xterm';
import { FitAddon } from '@xterm/addon-fit';
import { WebLinksAddon } from '@xterm/addon-web-links';
import '@xterm/xterm/css/xterm.css';
import { getActiveMachine } from '../store';
import { createClient } from '../api';

const QUICK_KEYS = [
  { label: 'Tab',    seq: '\t' },
  { label: 'Ctrl+C', seq: '\x03' },
  { label: 'Ctrl+D', seq: '\x04' },
  { label: 'Ctrl+Z', seq: '\x1a' },
  { label: '↑',      seq: '\x1b[A' },
  { label: '↓',      seq: '\x1b[B' },
  { label: '←',      seq: '\x1b[D' },
  { label: '→',      seq: '\x1b[C' },
  { label: 'Esc',    seq: '\x1b' },
  { label: 'Enter',  seq: '\r' },
];

const TAKO_THEME = {
  background: '#0b0e14',
  foreground: '#e8ecf1',
  cursor: '#22d3a7',
  cursorAccent: '#0b0e14',
  selectionBackground: 'rgba(34, 211, 167, 0.3)',
  selectionForeground: '#ffffff',
  black: '#1a2234',
  red: '#ef4444',
  green: '#22c55e',
  yellow: '#f59e0b',
  blue: '#3b82f6',
  magenta: '#a855f7',
  cyan: '#22d3a7',
  white: '#e8ecf1',
  brightBlack: '#4a5568',
  brightRed: '#f87171',
  brightGreen: '#4ade80',
  brightYellow: '#fbbf24',
  brightBlue: '#60a5fa',
  brightMagenta: '#c084fc',
  brightCyan: '#5eead4',
  brightWhite: '#f1f5f9',
};

const POLL_INTERVAL = 1000;

export function TerminalPage({ paneId }) {
  const [loading, setLoading] = useState(true);
  const [info, setInfo] = useState(null);
  const [allPanes, setAllPanes] = useState([]);
  const [connected, setConnected] = useState(true);
  const [input, setInput] = useState('');
  const machine = getActiveMachine();

  const termRef = useRef(null);
  const fitRef = useRef(null);
  const containerRef = useRef(null);
  const inputRef = useRef(null);
  const clientRef = useRef(null);
  const timerRef = useRef(null);
  const prevContentRef = useRef('');
  const touchRef = useRef({ x: 0, y: 0 });
  const failCountRef = useRef(0);

  if (machine && !clientRef.current) {
    clientRef.current = createClient(machine.host, machine.token);
  }

  // xterm.js 初期化
  useEffect(() => {
    if (!containerRef.current) return;

    const term = new Terminal({
      theme: TAKO_THEME,
      fontFamily: "'SF Mono', 'JetBrains Mono', 'Fira Code', 'Cascadia Code', ui-monospace, monospace",
      fontSize: 14,
      lineHeight: 1.2,
      cursorBlink: false,
      cursorStyle: 'block',
      scrollback: 500,
      disableStdin: true,
      convertEol: true,
    });

    const fitAddon = new FitAddon();
    term.loadAddon(fitAddon);
    term.loadAddon(new WebLinksAddon());

    term.open(containerRef.current);
    requestAnimationFrame(() => {
      try { fitAddon.fit(); } catch {}
    });

    termRef.current = term;
    fitRef.current = fitAddon;

    const observer = new ResizeObserver(() => {
      requestAnimationFrame(() => {
        try { fitAddon.fit(); } catch {}
      });
    });
    observer.observe(containerRef.current);

    const onViewportResize = () => {
      requestAnimationFrame(() => {
        try { fitAddon.fit(); } catch {}
      });
    };
    window.visualViewport?.addEventListener('resize', onViewportResize);

    return () => {
      window.visualViewport?.removeEventListener('resize', onViewportResize);
      observer.disconnect();
      term.dispose();
      termRef.current = null;
      fitRef.current = null;
    };
  }, []);

  // 画面ポーリング
  const refresh = useCallback(async () => {
    if (!clientRef.current) return;
    try {
      const [screen, panesList] = await Promise.all([
        clientRef.current.screen(paneId, 200),
        clientRef.current.panes(),
      ]);

      const lines = screen.lines || [];
      const content = lines.join('\n');

      if (content !== prevContentRef.current && termRef.current) {
        prevContentRef.current = content;
        let buf = '\x1b[H';
        for (const line of lines) {
          buf += '\x1b[2K' + line + '\r\n';
        }
        buf += '\x1b[J';
        termRef.current.write(buf);
      }

      const list = panesList.panes || [];
      setAllPanes(list);
      setInfo(list.find(p => p.id === paneId) || null);
      setLoading(false);
      setConnected(true);
      failCountRef.current = 0;
    } catch {
      failCountRef.current++;
      if (failCountRef.current >= 3) setConnected(false);
      setLoading(false);
    }
  }, [paneId]);

  useEffect(() => {
    if (!machine) { window.location.hash = '#/'; return; }
    clientRef.current = createClient(machine.host, machine.token);
    prevContentRef.current = '';
    if (termRef.current) termRef.current.clear();
    setLoading(true);
    setConnected(true);
    failCountRef.current = 0;
    refresh();
    timerRef.current = setInterval(refresh, POLL_INTERVAL);
    return () => clearInterval(timerRef.current);
  }, [paneId, refresh]);

  async function send() {
    const text = input.trim();
    if (!text || !clientRef.current) return;
    setInput('');
    if (navigator.vibrate) navigator.vibrate(10);
    try {
      await clientRef.current.input(paneId, text, true);
      setTimeout(refresh, 200);
    } catch {}
    inputRef.current?.focus();
  }

  async function sendKey(seq) {
    if (!clientRef.current) return;
    if (navigator.vibrate) navigator.vibrate(10);
    try {
      await clientRef.current.input(paneId, seq, false);
      setTimeout(refresh, 200);
    } catch {}
  }

  // スワイプでペイン切替
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

  function onKeyDown(e) {
    if (e.key === 'Enter') { e.preventDefault(); send(); }
  }

  if (!machine) return null;

  const idx = allPanes.findIndex(p => p.id === paneId);
  const pos = idx >= 0 ? `${idx + 1}/${allPanes.length}` : '';

  return (
    <div class="page terminal-page">
      <header class="terminal-header">
        <button class="back-btn" onClick={() => { window.location.hash = '#/panes'; }}>
          <svg width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M15 18l-6-6 6-6"/></svg>
        </button>
        <div class="terminal-title-bar">
          <span class={`conn-dot ${connected ? 'on' : 'off'}`} />
          <span class="terminal-name">{info?.title || `Pane ${paneId}`}</span>
          {pos && <span class="badge">{pos}</span>}
        </div>
      </header>

      <div
        class="xterm-container"
        ref={containerRef}
        onTouchStart={onTouchStart}
        onTouchEnd={onTouchEnd}
        onClick={() => inputRef.current?.focus()}
      >
        {loading && (
          <div class="xterm-loading"><div class="spinner" /></div>
        )}
      </div>

      {!connected && (
        <div class="reconnect-bar">
          <span>接続が切れています — 再接続中...</span>
        </div>
      )}

      <div class="quick-keys">
        {QUICK_KEYS.map(k => (
          <button key={k.label} class="quick-key" onClick={() => sendKey(k.seq)}>
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
