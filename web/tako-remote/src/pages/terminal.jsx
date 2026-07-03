import { useState, useEffect, useRef, useCallback } from 'preact/hooks';
import { Terminal } from '@xterm/xterm';
import { FitAddon } from '@xterm/addon-fit';
import { WebLinksAddon } from '@xterm/addon-web-links';
import '@xterm/xterm/css/xterm.css';
import { getActiveMachine } from '../store';
import { createClient } from '../api';

const QUICK_KEYS = [
  { label: 'esc',    seq: 'Escape' },
  { label: 'tab',    seq: 'Tab' },
  { label: 'ctrl',   seq: null, toggle: true },
  { label: '^C',     seq: 'C-c', accent: true },
  { label: '↑',      seq: 'Up' },
  { label: '↓',      seq: 'Down' },
  { label: '←',      seq: 'Left' },
  { label: '→',      seq: 'Right' },
  { label: '|',      literal: '|' },
  { label: '~',      literal: '~' },
];

const TAKO_THEME = {
  background: '#07080A',
  foreground: '#C7CCD1',
  cursor: '#5EE3A0',
  cursorAccent: '#07080A',
  selectionBackground: 'rgba(94, 227, 160, 0.3)',
  selectionForeground: '#ffffff',
  black: '#1D232A',
  red: '#F0655A',
  green: '#5EE3A0',
  yellow: '#E8B23E',
  blue: '#74B6FF',
  magenta: '#a855f7',
  cyan: '#5EE3A0',
  white: '#E9ECEF',
  brightBlack: '#5C636B',
  brightRed: '#f87171',
  brightGreen: '#6FF0B0',
  brightYellow: '#fbbf24',
  brightBlue: '#93C9FF',
  brightMagenta: '#c084fc',
  brightCyan: '#7AEDB8',
  brightWhite: '#f1f5f9',
};

export function TerminalPage({ paneId }) {
  const [loading, setLoading] = useState(true);
  const [info, setInfo] = useState(null);
  const [allPanes, setAllPanes] = useState([]);
  const [connected, setConnected] = useState(false);
  const [input, setInput] = useState('');
  const [layer, setLayer] = useState('live');
  const [scrollbackLines, setScrollbackLines] = useState([]);
  const [scrollbackLoading, setScrollbackLoading] = useState(false);
  const [ctrlMode, setCtrlMode] = useState(false);
  const machine = getActiveMachine();

  const termRef = useRef(null);
  const fitRef = useRef(null);
  const containerRef = useRef(null);
  const inputRef = useRef(null);
  const clientRef = useRef(null);
  const wsRef = useRef(null);
  const touchRef = useRef({ x: 0, y: 0 });
  const prevContentRef = useRef('');
  const scrollbackRef = useRef(null);
  const reconnectTimerRef = useRef(null);
  const paneListTimerRef = useRef(null);

  if (machine && !clientRef.current) {
    clientRef.current = createClient(machine.host, machine.token);
  }

  // xterm.js 初期化
  useEffect(() => {
    if (!containerRef.current) return;
    const term = new Terminal({
      theme: TAKO_THEME,
      fontFamily: "'Geist Mono', 'SF Mono', 'JetBrains Mono', 'Fira Code', ui-monospace, monospace",
      fontSize: 14,
      lineHeight: 1.2,
      cursorBlink: true,
      cursorStyle: 'block',
      scrollback: 0,
      disableStdin: true,
      convertEol: true,
    });
    const fitAddon = new FitAddon();
    term.loadAddon(fitAddon);
    term.loadAddon(new WebLinksAddon());
    term.open(containerRef.current);
    requestAnimationFrame(() => { try { fitAddon.fit(); } catch {} });
    termRef.current = term;
    fitRef.current = fitAddon;

    const observer = new ResizeObserver(() => {
      requestAnimationFrame(() => { try { fitAddon.fit(); } catch {} });
    });
    observer.observe(containerRef.current);
    const onViewportResize = () => {
      requestAnimationFrame(() => { try { fitAddon.fit(); } catch {} });
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

  // WS 接続（ライブレイヤー）
  const connectWs = useCallback(() => {
    if (!clientRef.current || !paneId) return;
    if (wsRef.current) {
      wsRef.current.onclose = null;
      wsRef.current.onerror = null;
      wsRef.current.onmessage = null;
      wsRef.current.close();
      wsRef.current = null;
    }

    const term = termRef.current;
    const fit = fitRef.current;
    let cols, rows;
    if (term) {
      try { fit?.fit(); } catch {}
      cols = term.cols;
      rows = term.rows;
    }

    const url = clientRef.current.wsUrl(paneId, cols, rows);
    const protocols = clientRef.current.wsProtocols();
    const ws = new WebSocket(url, protocols);

    ws.onopen = () => {
      setConnected(true);
      setLoading(false);
      prevContentRef.current = '';
    };

    ws.onmessage = (ev) => {
      try {
        const data = JSON.parse(ev.data);
        if (data.type === 'screen' && term) {
          const lines = data.lines || [];
          const content = lines.join('\n');
          if (content !== prevContentRef.current) {
            prevContentRef.current = content;
            let buf = '\x1b[H';
            for (const line of lines) {
              buf += '\x1b[2K' + line + '\x1b[0m\r\n';
            }
            buf += '\x1b[J';
            if (data.cursor) {
              buf += `\x1b[${data.cursor.y + 1};${data.cursor.x + 1}H`;
            }
            term.write(buf);
          }
        }
      } catch {}
    };

    ws.onclose = () => {
      setConnected(false);
      wsRef.current = null;
      reconnectTimerRef.current = setTimeout(connectWs, 3000);
    };

    ws.onerror = () => {
      ws.close();
    };

    wsRef.current = ws;
  }, [paneId]);

  // ペイン一覧の定期取得
  const refreshPanes = useCallback(async () => {
    if (!clientRef.current) return;
    try {
      const result = await clientRef.current.panes();
      const list = result.panes || [];
      setAllPanes(list);
      setInfo(list.find(p => p.id === paneId) || null);
    } catch {}
  }, [paneId]);

  useEffect(() => {
    if (!machine) { window.location.hash = '#/'; return; }
    clientRef.current = createClient(machine.host, machine.token);
    prevContentRef.current = '';
    if (termRef.current) termRef.current.clear();
    setLoading(true);
    setConnected(false);

    connectWs();
    refreshPanes();
    paneListTimerRef.current = setInterval(refreshPanes, 5000);

    return () => {
      clearInterval(paneListTimerRef.current);
      clearTimeout(reconnectTimerRef.current);
      if (wsRef.current) {
        wsRef.current.onclose = null;
        wsRef.current.onerror = null;
        wsRef.current.onmessage = null;
        wsRef.current.close();
        wsRef.current = null;
      }
    };
  }, [paneId, connectWs, refreshPanes]);

  // 履歴レイヤーのデータ取得
  const loadScrollback = useCallback(async () => {
    if (!clientRef.current) return;
    setScrollbackLoading(true);
    try {
      const result = await clientRef.current.scrollback(paneId, 2000);
      setScrollbackLines(result.lines || []);
    } catch {
      setScrollbackLines(['(スクロールバックの取得に失敗しました)']);
    }
    setScrollbackLoading(false);
  }, [paneId]);

  useEffect(() => {
    if (layer === 'history') loadScrollback();
  }, [layer, loadScrollback]);

  useEffect(() => {
    if (layer === 'history' && scrollbackRef.current && scrollbackLines.length > 0) {
      scrollbackRef.current.scrollTop = scrollbackRef.current.scrollHeight;
    }
  }, [layer, scrollbackLines]);

  async function send() {
    if (!clientRef.current) return;
    const text = input;
    setInput('');
    if (navigator.vibrate) navigator.vibrate(10);
    try {
      await clientRef.current.input(paneId, text, true);
    } catch {}
    inputRef.current?.focus();
  }

  async function sendKey(k) {
    if (!clientRef.current) return;
    if (navigator.vibrate) navigator.vibrate(10);
    if (k.literal) {
      try { await clientRef.current.input(paneId, k.literal, false); } catch {}
    } else if (k.seq) {
      let seq = k.seq;
      if (ctrlMode && seq.length === 1) {
        seq = `C-${seq}`;
        setCtrlMode(false);
      }
      try { await clientRef.current.sendKeys(paneId, seq); } catch {}
    }
  }

  function onTouchStart(e) {
    touchRef.current = { x: e.touches[0].clientX, y: e.touches[0].clientY };
  }
  function onTouchEnd(e) {
    const dx = e.changedTouches[0].clientX - touchRef.current.x;
    const dy = e.changedTouches[0].clientY - touchRef.current.y;
    if (Math.abs(dx) < 80 || Math.abs(dx) < Math.abs(dy) * 1.5) return;
    const idx = allPanes.findIndex(p => p.id === paneId);
    if (idx < 0) return;
    if (dx > 0 && idx > 0) window.location.hash = `#/panes/${allPanes[idx - 1].id}`;
    else if (dx < 0 && idx < allPanes.length - 1) window.location.hash = `#/panes/${allPanes[idx + 1].id}`;
  }

  // #41 の isComposing ガード + Shift+Enter で改行（#26）
  function onInputKeyDown(e) {
    if (e.isComposing) return;
    if (ctrlMode && e.key.length === 1 && !e.metaKey && !e.altKey) {
      e.preventDefault();
      setCtrlMode(false);
      if (navigator.vibrate) navigator.vibrate(10);
      clientRef.current?.sendKeys(paneId, `C-${e.key}`).catch(() => {});
      return;
    }
    if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault();
      send();
    }
  }

  function onSubmit(e) {
    e.preventDefault();
    send();
  }

  if (!machine) return null;
  const idx = allPanes.findIndex(p => p.id === paneId);
  const pos = idx >= 0 ? `${idx + 1}/${allPanes.length}` : '';

  return (
    <div class="page terminal-page">
      <header class="terminal-header">
        <div class="terminal-header-left">
          <button class="back-btn" onClick={() => { window.location.hash = '#/panes'; }}>‹</button>
          <div class="terminal-header-info">
            <div class="terminal-header-top">
              <span class={`conn-dot ${connected ? 'on' : 'off'}`} />
              <span class="terminal-name">{info?.title || `Pane ${paneId}`}</span>
            </div>
            <span class="terminal-meta">{machine.name}{pos ? ` · ${pos}` : ''}</span>
          </div>
        </div>
        <div class="terminal-header-right">
          <button
            class={`layer-toggle${layer === 'history' ? ' active' : ''}`}
            onClick={() => setLayer(layer === 'live' ? 'history' : 'live')}
            title={layer === 'live' ? '履歴を表示' : 'ライブに戻る'}
          >
            {layer === 'live' ? '↑履歴' : '●ライブ'}
          </button>
        </div>
      </header>

      {layer === 'live' ? (
        <div
          class="xterm-container"
          ref={containerRef}
          onTouchStart={onTouchStart}
          onTouchEnd={onTouchEnd}
          onClick={() => inputRef.current?.focus()}
        >
          {loading && <div class="xterm-loading"><div class="spinner" /></div>}
        </div>
      ) : (
        <div class="scrollback-container" ref={scrollbackRef}>
          {scrollbackLoading ? (
            <div class="xterm-loading"><div class="spinner" /></div>
          ) : (
            <pre class="scrollback-content">{scrollbackLines.join('\n')}</pre>
          )}
        </div>
      )}

      {!connected && layer === 'live' && (
        <div class="reconnect-bar">接続が切れています — 再接続中...</div>
      )}

      <div class="quick-keys">
        {QUICK_KEYS.map(k => (
          <button
            key={k.label}
            class={`quick-key${k.accent ? ' key-accent' : ''}${k.toggle && ctrlMode ? ' key-active' : ''}`}
            onClick={() => {
              if (k.toggle) {
                setCtrlMode(!ctrlMode);
              } else {
                sendKey(k);
              }
            }}
          >
            {k.label}
          </button>
        ))}
      </div>

      <form class="input-bar" onSubmit={onSubmit}>
        <textarea
          ref={inputRef}
          class="input-field input-textarea"
          value={input}
          onInput={e => setInput(e.target.value)}
          onKeyDown={onInputKeyDown}
          placeholder="$ command..."
          autocomplete="off"
          autocorrect="off"
          autocapitalize="off"
          spellcheck={false}
          enterkeyhint="send"
          rows={1}
        />
        <button type="submit" class="send-btn">{input.trim() ? '↑' : '↵'}</button>
      </form>
    </div>
  );
}
