// ペインページ — chat/term デュアルビュー（Issue #284 / カンプ 1a + 1d）
//
// ヘッダ: agent アイコン・タイトル・メタ情報・chat/term トグル
// chat: transcript API からの会話表示 + コンポーザー
// term: 既存リーダービュー資産（WS ストリーム + ANSI レンダ + quick keys）
import { useState, useEffect, useRef, useCallback } from 'preact/hooks';
import { createClient } from '../api';
import { parseAnsiLine, defaultSgrState, colorToCss } from '../ansi';
import { AgentIcon, agentColor } from '../components/agent-icon';
import { ChatView } from '../components/chat-view';

const QUICK_KEYS = [
  { label: 'esc',    seq: 'Escape' },
  { label: 'tab',    seq: 'Tab' },
  { label: 'ctrl',   seq: null, toggle: true },
  { label: '^C',     seq: 'C-c', accent: true },
  { label: '↑', seq: 'Up' },
  { label: '↓', seq: 'Down' },
  { label: '|',      literal: '|' },
];

const MAX_HISTORY_DOM = 6000;
const AT_BOTTOM_PX = 60;
const FONT_MIN = 9;
const FONT_MAX = 22;
const FONT_KEY = 'tako-remote-fs';

// --- 行 DOM 組み立て（Preact 外） ---

function charWidth(cp) {
  return (cp >= 0x1100 && cp <= 0x115f) || cp === 0x2329 || cp === 0x232a ||
    (cp >= 0x2e80 && cp <= 0xa4cf && cp !== 0x303f) ||
    (cp >= 0xac00 && cp <= 0xd7a3) || (cp >= 0xf900 && cp <= 0xfaff) ||
    (cp >= 0xfe30 && cp <= 0xfe6f) || (cp >= 0xff00 && cp <= 0xff60) ||
    (cp >= 0xffe0 && cp <= 0xffe6) || (cp >= 0x1f300 && cp <= 0x1faff) ||
    (cp >= 0x20000 && cp <= 0x3fffd)
    ? 2 : 1;
}

function makeNode(text, style) {
  if (!style) return document.createTextNode(text);
  const span = document.createElement('span');
  span.textContent = text;
  let fg = colorToCss(style.fg);
  let bg = colorToCss(style.bg);
  if (style.reverse) {
    const f = fg || 'var(--t-fg)';
    const b = bg || 'var(--t-bg)';
    fg = b; bg = f;
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

function appendWithCursor(el, segments, cursorX) {
  let col = 0;
  let placed = false;
  for (const seg of segments) {
    if (placed) { el.appendChild(makeNode(seg.text, seg.style)); continue; }
    let before = '', curCh = '', after = '';
    for (const ch of seg.text) {
      const w = charWidth(ch.codePointAt(0));
      if (!placed && col + w > cursorX) { curCh = ch; placed = true; }
      else if (!placed) before += ch;
      else after += ch;
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
    if (cursorX > col) el.appendChild(document.createTextNode(' '.repeat(cursorX - col)));
    const cur = document.createElement('span');
    cur.className = 'cur';
    cur.textContent = ' ';
    el.appendChild(cur);
  }
}

function buildLine(line, state, cursorX = -1) {
  const { segments, state: endState } = parseAnsiLine(line, state);
  const el = document.createElement('div');
  el.className = 'tl';
  if (cursorX >= 0) appendWithCursor(el, segments, cursorX);
  else for (const seg of segments) el.appendChild(makeNode(seg.text, seg.style));
  return { el, state: endState };
}

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

export function TerminalPage({ paneId, me }) {
  const [view, setView] = useState('chat');
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
  // term ビュー未マウント中に届いた WS init の保留（#426/#428: chat 表示中に init が
  // 届くと DOM が無く捨てられ、update に loading 解除が無いため永久スピナーになる）
  const pendingInitRef = useRef(null);

  if (!clientRef.current) clientRef.current = createClient();

  const agentType = info?.agent_type || 'plain';
  const color = agentColor(agentType);
  const hasChatSupport = !!info?.session_id && (agentType === 'claude' || agentType === 'codex' || agentType === 'agy');

  // ユーザーがトグルで明示選択したか（選択後は自動切替しない。ペイン移動でリセット）
  const userPinnedViewRef = useRef(false);

  // chat が使えなければ term へ、使えるようになったら chat へ自動追従する。
  // 旧実装は chat→term の片方向のみで、最初のポーリングで一時的に session_id が
  // 欠けると以後チャットに戻れなかった（#439 の頑健化）
  useEffect(() => {
    if (!info || userPinnedViewRef.current) return;
    if (view === 'chat' && !hasChatSupport) {
      setView('term');
    } else if (view === 'term' && hasChatSupport) {
      setView('chat');
    }
  }, [info, hasChatSupport, view]);

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

  const replaceScreen = (screenLines, cursor) => {
    const scr = screenRef.current;
    if (!scr) return;
    scr.textContent = '';
    scr.appendChild(buildScreen(screenLines, cursor));
  };

  // init メッセージを DOM に反映する。term ビュー未マウントなら false（保留させる）
  const applyInit = (data) => {
    const reader = readerRef.current;
    if (!reader || !historyRef.current || !screenRef.current) return false;
    historyRef.current.textContent = '';
    sgrStateRef.current = defaultSgrState();
    appendHistory(data.history || []);
    replaceScreen(data.screen || [], data.cursor);
    setLoading(false);
    setHasNew(false);
    atBottomRef.current = true;
    reader.scrollTop = reader.scrollHeight;
    return true;
  };

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
    ws.onopen = () => setConnected(true);
    ws.onmessage = (ev) => {
      let data;
      try { data = JSON.parse(ev.data); } catch { return; }
      if (data.type === 'init') {
        if (applyInit(data)) pendingInitRef.current = null;
        else pendingInitRef.current = data;
      } else if (data.type === 'update') {
        // 保留 init がある間の update は保留側にマージする（term マウント時に最新を適用）
        const pending = pendingInitRef.current;
        if (pending) {
          pending.history = (pending.history || []).concat(data.pushed || []);
          pending.screen = data.screen || [];
          pending.cursor = data.cursor;
          return;
        }
        const reader = readerRef.current;
        if (!reader || !historyRef.current || !screenRef.current) return;
        const follow = atBottomRef.current;
        const pushed = data.pushed || [];
        if (pushed.length) {
          appendHistory(pushed);
          if (follow) {
            const hist = historyRef.current;
            let over = hist.childElementCount - MAX_HISTORY_DOM;
            while (over-- > 0) hist.firstElementChild?.remove();
          }
        }
        replaceScreen(data.screen || [], data.cursor);
        if (follow) reader.scrollTop = reader.scrollHeight;
        else if (pushed.length) setHasNew(true);
      }
    };
    ws.onclose = () => {
      setConnected(false);
      wsRef.current = null;
      reconnectTimerRef.current = setTimeout(connectWs, 3000);
    };
    ws.onerror = () => ws.close();
    wsRef.current = ws;
  }, [paneId]);

  // term ビューがマウントされたら保留中の init を適用する（#426/#428）
  useEffect(() => {
    if (view !== 'term') return;
    const pending = pendingInitRef.current;
    if (pending && applyInit(pending)) pendingInitRef.current = null;
  }, [view]);

  const refreshPanes = useCallback(async () => {
    if (!clientRef.current) return;
    try {
      const result = await clientRef.current.panes();
      const list = result.panes || [];
      setAllPanes(list);
      setInfo(list.find(p => String(p.id) === String(paneId)) || null);
    } catch {}
  }, [paneId]);

  useEffect(() => {
    clientRef.current = createClient();
    setLoading(true);
    setConnected(false);
    setHasNew(false);
    atBottomRef.current = true;
    sgrStateRef.current = defaultSgrState();
    pendingInitRef.current = null;
    userPinnedViewRef.current = false;
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

  function autoResizeTextarea(el) {
    if (!el) return;
    el.style.height = 'auto';
    el.style.height = el.scrollHeight + 'px';
  }

  async function termSend() {
    if (!clientRef.current) return;
    const text = input;
    setInput('');
    if (inputRef.current) inputRef.current.style.height = 'auto';
    if (navigator.vibrate) navigator.vibrate(10);
    try { await clientRef.current.input(paneId, text, true); } catch {}
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

  async function sendStop() {
    if (!clientRef.current) return;
    if (navigator.vibrate) navigator.vibrate(10);
    try { await clientRef.current.sendKeys(paneId, 'Escape'); } catch {}
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

  const idx = allPanes.findIndex(p => String(p.id) === String(paneId));
  function goPane(dir) {
    if (idx < 0) return;
    const next = allPanes[idx + dir];
    if (next) window.location.hash = `#/panes/${next.id}`;
  }

  function onTermKeyDown(e) {
    if (e.isComposing) return;
    if (ctrlMode && e.key.length === 1 && !e.metaKey && !e.altKey) {
      e.preventDefault();
      setCtrlMode(false);
      if (navigator.vibrate) navigator.vibrate(10);
      clientRef.current?.sendKeys(paneId, `C-${e.key}`).catch(() => {});
      return;
    }
    // 送信は cmd/ctrl+Enter か送信ボタンのみ。素の Enter は改行として入力する
    // （#429: モバイルキーボードの改行キーで送信されてしまい改行が打てなかった）
    if (e.key === 'Enter' && (e.metaKey || e.ctrlKey)) {
      e.preventDefault();
      termSend();
    }
  }

  // ヘッダ情報の組み立て
  const title = info
    ? `${agentType !== 'plain' ? agentType + ' · ' : ''}${info.title || `Pane ${paneId}`}`
    : `Pane ${paneId}`;
  const position = info?.position || '';
  const host = (me && me.host) || 'tako';
  const modelName = info?.model || '';
  const subtitle = [host, position, modelName].filter(Boolean).join(' · ');
  const termSubtitle = [host, position, `A− A＋`].filter(Boolean).join(' · ');

  return (
    <div class="page terminal-page">
      {/* ヘッダ — カンプ 1a/1d 共通 */}
      <div class="pane-header">
        <div class="pane-header-left">
          <button class="pane-header-back" onClick={() => { window.location.hash = '#/'; }}>
            {'‹'}
          </button>
          <AgentIcon type={agentType} />
          <div class="pane-header-info">
            <span class="pane-header-title">{title}</span>
            <span class="pane-header-subtitle">
              {view === 'term' ? (
                <>
                  {host}{position ? ` · ${position}` : ''}{' · '}
                  <span
                    style="cursor:pointer"
                    onClick={(e) => { e.stopPropagation(); adjustFont(-1); }}
                  >A−</span>
                  {' '}
                  <span
                    style="cursor:pointer"
                    onClick={(e) => { e.stopPropagation(); adjustFont(+1); }}
                  >A＋</span>
                </>
              ) : subtitle}
            </span>
          </div>
        </div>
        {/* chat/term トグル */}
        {hasChatSupport ? (
          <div class="view-toggle">
            <button
              class={`view-toggle-btn${view === 'chat' ? ' active chat-active' : ''}`}
              style={view === 'chat' ? `color:${color}` : ''}
              onClick={() => { userPinnedViewRef.current = true; setView('chat'); }}
            >chat</button>
            <button
              class={`view-toggle-btn${view === 'term' ? ' active term-active' : ''}`}
              onClick={() => { userPinnedViewRef.current = true; setView('term'); }}
            >term</button>
          </div>
        ) : null}
      </div>

      {/* chat ビュー */}
      {view === 'chat' && hasChatSupport && (
        <ChatView
          paneId={paneId}
          info={info}
          agentType={agentType}
          onStop={sendStop}
          me={me}
        />
      )}

      {/* term ビュー */}
      {view === 'term' && (
        <>
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
              <button class="jump-latest" onClick={jumpToLatest}>{'↓'} 最新へ</button>
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
                  if (k.toggle) setCtrlMode(!ctrlMode);
                  else sendKey(k);
                }}
              >{k.label}</button>
            ))}
          </div>

          <div class="term-input-bar">
            <textarea
              ref={inputRef}
              class="term-input-field"
              value={input}
              onInput={e => { setInput(e.target.value); autoResizeTextarea(e.target); }}
              onKeyDown={onTermKeyDown}
              placeholder="$ command..."
              autocomplete="off"
              autocorrect="off"
              autocapitalize="off"
              spellcheck={false}
              enterkeyhint="enter"
              rows={1}
            />
            <button class="term-send-btn" onClick={termSend}>
              {'↵'}
            </button>
          </div>
        </>
      )}
    </div>
  );
}
