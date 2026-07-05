// ターミナルページ — リーダービュー方式（Issue #63）
//
// サーバーはペインの「中身」（履歴 + 現画面、ANSI 付き）だけを WS でプッシュし、
// このページがモバイル向けに再描画する。ターミナルのグリッドを忠実再現せず、
// 読みやすさを最優先する設計:
//
// - 履歴 + ライブ画面を 1 本の縦スクロールに連結（切替 UI なし）。
//   下端 = ライブ追従、上スクロールで過去、下端復帰で追従再開
// - 行はビューポート幅で折り返し（pre-wrap）、フォントサイズ調整可
// - テキスト選択・コピーはブラウザネイティブ
// - ペインサイズには一切影響しない（PC 非破壊 — リサイズ要求を送らない）
//
// 履歴 DOM は Preact を通さず直接 append する（数千行の vdom diff を避ける）。
// ライブ画面 DOM は update ごとに丸ごと置き換える（高々数十行なので軽い）。
import { useState, useEffect, useRef, useCallback } from 'preact/hooks';
import { getActiveMachine } from '../store';
import { createClient } from '../api';
import { parseAnsiLine, defaultSgrState, colorToCss } from '../ansi';

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

/** 履歴 DOM に保持する最大行数。超過分は先頭（最古）から削る */
const MAX_HISTORY_DOM = 6000;
/** 下端からこの px 以内なら「下端にいる」= ライブ追従中とみなす */
const AT_BOTTOM_PX = 60;
const FONT_MIN = 9;
const FONT_MAX = 22;
const FONT_KEY = 'tako-remote-fs';

// --- 行 DOM の組み立て（Preact 外の純粋ヘルパー） ---

/** East Asian Width の全角近似（カーソル位置合わせ用） */
function charWidth(cp) {
  return (cp >= 0x1100 && cp <= 0x115f) || cp === 0x2329 || cp === 0x232a ||
    (cp >= 0x2e80 && cp <= 0xa4cf && cp !== 0x303f) ||
    (cp >= 0xac00 && cp <= 0xd7a3) || (cp >= 0xf900 && cp <= 0xfaff) ||
    (cp >= 0xfe30 && cp <= 0xfe6f) || (cp >= 0xff00 && cp <= 0xff60) ||
    (cp >= 0xffe0 && cp <= 0xffe6) || (cp >= 0x1f300 && cp <= 0x1faff) ||
    (cp >= 0x20000 && cp <= 0x3fffd)
    ? 2 : 1;
}

/** 装飾付きテキストノードを作る。style が null なら素の TextNode */
function makeNode(text, style) {
  if (!style) return document.createTextNode(text);
  const span = document.createElement('span');
  span.textContent = text;
  let fg = colorToCss(style.fg);
  let bg = colorToCss(style.bg);
  if (style.reverse) {
    const f = fg || 'var(--t-fg)';
    const b = bg || 'var(--t-bg)';
    fg = b;
    bg = f;
  }
  if (fg) span.style.color = fg;
  if (bg) span.style.backgroundColor = bg;
  let cls = '';
  if (style.bold) cls += ' b';
  if (style.dim) cls += ' d';
  if (style.italic) cls += ' i';
  if (style.underline) cls += ' u';
  if (style.strike) cls += ' x';
  if (cls) span.className = cls.trim();
  return span;
}

/** セグメント列を el に追記しつつ、表示幅 cursorX の位置の文字をカーソル表示にする */
function appendWithCursor(el, segments, cursorX) {
  let col = 0;
  let placed = false;
  for (const seg of segments) {
    if (placed) {
      el.appendChild(makeNode(seg.text, seg.style));
      continue;
    }
    let before = '';
    let curCh = '';
    let after = '';
    for (const ch of seg.text) {
      const w = charWidth(ch.codePointAt(0));
      if (!placed && col + w > cursorX) {
        curCh = ch;
        placed = true;
      } else if (!placed) {
        before += ch;
      } else {
        after += ch;
      }
      col += w;
    }
    if (before) el.appendChild(makeNode(before, seg.style));
    if (curCh) {
      const cur = document.createElement('span');
      cur.className = 'cur';
      cur.textContent = curCh;
      el.appendChild(cur);
    }
    if (after) el.appendChild(makeNode(after, seg.style));
  }
  if (!placed) {
    // カーソルが行末より右: 空白でパディングして置く
    if (cursorX > col) el.appendChild(document.createTextNode(' '.repeat(cursorX - col)));
    const cur = document.createElement('span');
    cur.className = 'cur';
    cur.textContent = ' ';
    el.appendChild(cur);
  }
}

/** 1 行分の div.tl を作る。state は行を跨ぐ SGR 継続、cursorX >= 0 でカーソル埋め込み */
function buildLine(line, state, cursorX = -1) {
  const { segments, state: endState } = parseAnsiLine(line, state);
  const el = document.createElement('div');
  el.className = 'tl';
  if (cursorX >= 0) {
    appendWithCursor(el, segments, cursorX);
  } else {
    for (const seg of segments) el.appendChild(makeNode(seg.text, seg.style));
  }
  return { el, state: endState };
}

/** ライブ画面全体の DocumentFragment を作る（カーソル行に cursor を埋め込む） */
function buildScreen(screenLines, cursor) {
  const frag = document.createDocumentFragment();
  let state = defaultSgrState();
  screenLines.forEach((line, y) => {
    const cx = cursor && cursor.y === y ? cursor.x : -1;
    const { el, state: st } = buildLine(line, state, cx);
    state = st;
    frag.appendChild(el);
  });
  return frag;
}

export function TerminalPage({ paneId }) {
  const [loading, setLoading] = useState(true);
  const [info, setInfo] = useState(null);
  const [allPanes, setAllPanes] = useState([]);
  const [connected, setConnected] = useState(false);
  const [input, setInput] = useState('');
  const [ctrlMode, setCtrlMode] = useState(false);
  const [hasNew, setHasNew] = useState(false);
  const [fontSize, setFontSize] = useState(() => {
    const saved = parseInt(localStorage.getItem(FONT_KEY) || '', 10);
    return Number.isFinite(saved) && saved >= FONT_MIN && saved <= FONT_MAX ? saved : 13;
  });
  const machine = getActiveMachine();

  const readerRef = useRef(null);
  const historyRef = useRef(null);
  const screenRef = useRef(null);
  const inputRef = useRef(null);
  const clientRef = useRef(null);
  const wsRef = useRef(null);
  const touchRef = useRef({ x: 0, y: 0 });
  const reconnectTimerRef = useRef(null);
  const paneListTimerRef = useRef(null);
  const atBottomRef = useRef(true);
  const sgrStateRef = useRef(defaultSgrState());

  if (machine && !clientRef.current) {
    clientRef.current = createClient(machine.host, machine.token);
  }

  /** 履歴 DOM へ行を追記する（SGR 状態は行を跨いで継続） */
  const appendHistory = (lines) => {
    const hist = historyRef.current;
    if (!hist || !lines.length) return;
    const frag = document.createDocumentFragment();
    let state = sgrStateRef.current;
    for (const line of lines) {
      const { el, state: st } = buildLine(line, state);
      state = st;
      frag.appendChild(el);
    }
    sgrStateRef.current = state;
    hist.appendChild(frag);
  };

  /** ライブ画面 DOM を丸ごと置き換える */
  const replaceScreen = (screenLines, cursor) => {
    const scr = screenRef.current;
    if (!scr) return;
    scr.textContent = '';
    scr.appendChild(buildScreen(screenLines, cursor));
  };

  // WS 接続。#52 対策: 張り替え・クリーンアップ時は必ずハンドラを外してから close する
  const connectWs = useCallback(() => {
    if (!clientRef.current || !paneId) return;
    if (wsRef.current) {
      wsRef.current.onclose = null;
      wsRef.current.onerror = null;
      wsRef.current.onmessage = null;
      wsRef.current.close();
      wsRef.current = null;
    }

    const ws = new WebSocket(clientRef.current.wsUrl(paneId), clientRef.current.wsProtocols());

    ws.onopen = () => {
      setConnected(true);
    };

    ws.onmessage = (ev) => {
      let data;
      try {
        data = JSON.parse(ev.data);
      } catch {
        return;
      }
      const reader = readerRef.current;
      if (!reader || !historyRef.current || !screenRef.current) return;

      if (data.type === 'init') {
        historyRef.current.textContent = '';
        sgrStateRef.current = defaultSgrState();
        appendHistory(data.history || []);
        replaceScreen(data.screen || [], data.cursor);
        setLoading(false);
        setHasNew(false);
        // init はビューの作り直し。下端（ライブ追従）から始める
        atBottomRef.current = true;
        reader.scrollTop = reader.scrollHeight;
      } else if (data.type === 'update') {
        const follow = atBottomRef.current;
        const pushed = data.pushed || [];
        if (pushed.length) {
          appendHistory(pushed);
          if (follow) {
            // 追従中のみ古い行を間引く（過去閲覧中に消すと視点が飛ぶ）
            const hist = historyRef.current;
            let over = hist.childElementCount - MAX_HISTORY_DOM;
            while (over-- > 0) hist.firstElementChild?.remove();
          }
        }
        replaceScreen(data.screen || [], data.cursor);
        if (follow) {
          reader.scrollTop = reader.scrollHeight;
        } else if (pushed.length) {
          setHasNew(true);
        }
      }
      // keepalive は接続維持のみ。error は onclose → 再接続に任せる
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
    setLoading(true);
    setConnected(false);
    setHasNew(false);
    atBottomRef.current = true;
    sgrStateRef.current = defaultSgrState();
    if (historyRef.current) historyRef.current.textContent = '';
    if (screenRef.current) screenRef.current.textContent = '';

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

  function onReaderScroll() {
    const el = readerRef.current;
    if (!el) return;
    const atBottom = el.scrollHeight - el.scrollTop - el.clientHeight < AT_BOTTOM_PX;
    atBottomRef.current = atBottom;
    if (atBottom) setHasNew(false);
  }

  function jumpToLatest() {
    const el = readerRef.current;
    if (!el) return;
    atBottomRef.current = true;
    setHasNew(false);
    el.scrollTop = el.scrollHeight;
  }

  function adjustFont(d) {
    setFontSize(f => {
      const next = Math.max(FONT_MIN, Math.min(FONT_MAX, f + d));
      localStorage.setItem(FONT_KEY, String(next));
      return next;
    });
  }

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

  const idx = allPanes.findIndex(p => p.id === paneId);

  function goPane(dir) {
    if (idx < 0) return;
    const next = allPanes[idx + dir];
    if (next) window.location.hash = `#/panes/${next.id}`;
  }

  function onTouchStart(e) {
    touchRef.current = { x: e.touches[0].clientX, y: e.touches[0].clientY };
  }
  function onTouchEnd(e) {
    const dx = e.changedTouches[0].clientX - touchRef.current.x;
    const dy = e.changedTouches[0].clientY - touchRef.current.y;
    if (Math.abs(dx) < 80 || Math.abs(dx) < Math.abs(dy) * 1.5) return;
    goPane(dx > 0 ? -1 : +1);
  }

  // #41 の isComposing ガード + Shift+Enter で改行（#26）+ ctrl トグル（#51）
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
          <button class="hbtn" onClick={() => adjustFont(-1)} title="文字を小さく">A−</button>
          <button class="hbtn" onClick={() => adjustFont(+1)} title="文字を大きく">A＋</button>
          <button class="hbtn" disabled={idx <= 0} onClick={() => goPane(-1)} title="前のペイン">‹</button>
          <button class="hbtn" disabled={idx < 0 || idx >= allPanes.length - 1} onClick={() => goPane(+1)} title="次のペイン">›</button>
        </div>
      </header>

      <div class="reader-wrap">
        <div
          class="reader"
          ref={readerRef}
          style={`--term-fs:${fontSize}px`}
          onScroll={onReaderScroll}
          onTouchStart={onTouchStart}
          onTouchEnd={onTouchEnd}
        >
          <div class="reader-history" ref={historyRef} />
          <div class="reader-screen" ref={screenRef} />
        </div>
        {loading && <div class="reader-loading"><div class="spinner" /></div>}
        {hasNew && !loading && (
          <button class="jump-latest" onClick={jumpToLatest}>↓ 最新へ</button>
        )}
        {!connected && !loading && (
          <div class="reconnect-bar">接続が切れています — 再接続中...</div>
        )}
      </div>

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
