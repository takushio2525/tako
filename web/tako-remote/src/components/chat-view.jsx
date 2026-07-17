import { useState, useEffect, useRef, useCallback } from 'preact/hooks';
import { createClient } from '../api';
import { AgentIcon, agentColor, agentDarkColor } from './agent-icon';

const TOOL_ICONS = {
  Bash: 'Bash',
  Edit: 'Edit',
  Read: 'Read',
  Write: 'Write',
  Grep: 'Grep',
  WebSearch: 'WebSearch',
  WebFetch: 'WebFetch',
};

function formatToolSummary(tool) {
  const name = tool.name || 'unknown';
  const summary = tool.summary || '';
  const displayName = TOOL_ICONS[name] || name;
  return { displayName, summary };
}

function parseDiffCounts(summary) {
  const m = summary.match(/\+(\d+)\s*-(\d+)/);
  if (m) return { add: m[1], del: m[2] };
  return null;
}

function ToolCard({ tool, agentType }) {
  const [expanded, setExpanded] = useState(false);
  const { displayName, summary } = formatToolSummary(tool);
  const diff = parseDiffCounts(summary);
  const dotColor = 'success';

  return (
    <div class="tool-card">
      <div class="tool-card-header" onClick={() => setExpanded(!expanded)}>
        <span class={`tool-card-dot ${dotColor}`} />
        <span class="tool-card-label">
          <strong>{displayName}</strong>{summary ? ` ${summary}` : ''}
        </span>
        {diff ? (
          <span class="tool-card-diff">
            <span class="add">+{diff.add}</span>{' '}
            <span class="del">-{diff.del}</span>
          </span>
        ) : (
          <span class="tool-card-chevron">{expanded ? '▴' : '▾'}</span>
        )}
      </div>
      {expanded && !diff && (
        <div class="tool-card-body">
          {summary || '(no output)'}
        </div>
      )}
    </div>
  );
}

function ChatMessage({ msg, agentType }) {
  if (msg.role === 'user') {
    return <div class="chat-user">{msg.text}</div>;
  }

  const agentName = agentType || 'claude';

  return (
    <div class="chat-agent">
      <div class="chat-agent-avatar">
        <AgentIcon type={agentType} small />
        <span class="agent-name">{agentName}</span>
      </div>
      {msg.text && (
        <div class="chat-agent-text">{renderTextWithCode(msg.text)}</div>
      )}
      {msg.tools && msg.tools.map((tool, i) => (
        <ToolCard key={i} tool={tool} agentType={agentType} />
      ))}
    </div>
  );
}

function renderTextWithCode(text) {
  const parts = text.split(/(`[^`]+`)/g);
  return parts.map((part, i) => {
    if (part.startsWith('`') && part.endsWith('`')) {
      return <code key={i}>{part.slice(1, -1)}</code>;
    }
    return part;
  });
}

function formatTime(ts) {
  if (!ts) return null;
  try {
    const d = new Date(ts);
    const h = d.getHours().toString().padStart(2, '0');
    const m = d.getMinutes().toString().padStart(2, '0');
    return `${h}:${m}`;
  } catch {
    return null;
  }
}

function formatDateLabel(ts) {
  if (!ts) return null;
  try {
    const d = new Date(ts);
    const today = new Date();
    if (
      d.getFullYear() === today.getFullYear() &&
      d.getMonth() === today.getMonth() &&
      d.getDate() === today.getDate()
    ) {
      return `今日 ${formatTime(ts)}`;
    }
    return `${d.getMonth() + 1}/${d.getDate()} ${formatTime(ts)}`;
  } catch {
    return null;
  }
}

export function ChatView({ paneId, info, agentType, onStop }) {
  const [messages, setMessages] = useState([]);
  const [loading, setLoading] = useState(true);
  const [input, setInput] = useState('');
  const scrollRef = useRef(null);
  const inputRef = useRef(null);
  const timerRef = useRef(null);
  const atBottomRef = useRef(true);

  const isRunning = info && (info.state === 'busy' || info.state === 'running');

  const fetchMessages = useCallback(async () => {
    if (!info || !info.session_id) return;
    try {
      const client = createClient();
      const result = await client.messages(info.session_id, 50);
      setMessages(result.messages || []);
      setLoading(false);
    } catch {
      setLoading(false);
    }
  }, [info?.session_id]);

  useEffect(() => {
    if (!info?.session_id) {
      setMessages([]);
      setLoading(false);
      return;
    }
    setLoading(true);
    fetchMessages();
    timerRef.current = setInterval(fetchMessages, 3000);
    return () => clearInterval(timerRef.current);
  }, [info?.session_id, fetchMessages]);

  useEffect(() => {
    if (atBottomRef.current && scrollRef.current) {
      scrollRef.current.scrollTop = scrollRef.current.scrollHeight;
    }
  }, [messages]);

  function onScroll() {
    const el = scrollRef.current;
    if (!el) return;
    atBottomRef.current = el.scrollHeight - el.scrollTop - el.clientHeight < 60;
  }

  async function send() {
    const text = input.trim();
    if (!text) return;
    setInput('');
    if (navigator.vibrate) navigator.vibrate(10);
    try {
      await createClient().input(paneId, text, true);
    } catch {}
    inputRef.current?.focus();
  }

  function onKeyDown(e) {
    if (e.isComposing) return;
    if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault();
      send();
    }
  }

  const color = agentColor(agentType);
  const darkColor = agentDarkColor(agentType);
  const placeholder = `${agentType || 'agent'} に返信...`;

  let lastDate = null;

  return (
    <>
      <div class="chat-scroll" ref={scrollRef} onScroll={onScroll}>
        {loading && (
          <div style="display:flex;justify-content:center;padding:40px">
            <div class="spinner" />
          </div>
        )}
        {!loading && messages.length === 0 && info?.session_id && (
          <div style="text-align:center;color:var(--fg3);font-size:13px;padding:40px 0">
            会話が見つかりません
          </div>
        )}
        {!loading && !info?.session_id && (
          <div style="text-align:center;color:var(--fg3);font-size:13px;padding:40px 0">
            このペインにはチャット履歴がありません。<br />term ビューに切り替えてください。
          </div>
        )}
        {messages.map((msg, i) => {
          const dateLabel = formatDateLabel(msg.timestamp);
          let showDate = false;
          if (dateLabel && dateLabel !== lastDate) {
            showDate = true;
            lastDate = dateLabel;
          }
          return (
            <div key={i}>
              {showDate && <div class="chat-date">{dateLabel}</div>}
              <ChatMessage msg={msg} agentType={agentType} />
            </div>
          );
        })}
        {isRunning && (
          <div class="chat-running">
            <span
              class="chat-running-dot"
              style={`background:${color};width:7px;height:7px;border-radius:50%`}
            />
            <span class="chat-running-text">
              {'実行中…'}
            </span>
            <button class="chat-stop-btn" onClick={() => onStop?.()}>
              <svg width="8" height="8" viewBox="0 0 8 8" style="margin-right:4px;vertical-align:middle"><rect width="8" height="8" rx="1" fill="currentColor"/></svg>停止
            </button>
          </div>
        )}
      </div>

      <div class="composer">
        <div class="composer-box">
          <textarea
            ref={inputRef}
            class="composer-input"
            value={input}
            onInput={e => setInput(e.target.value)}
            onKeyDown={onKeyDown}
            placeholder={placeholder}
            autocomplete="off"
            autocorrect="off"
            autocapitalize="off"
            spellcheck={false}
            rows={1}
          />
          <div class="composer-toolbar">
            <button class="composer-btn-attach" disabled>+</button>
            {info?.model && (
              <span class="composer-chip">{info.model}</span>
            )}
            <button
              class="composer-send"
              style={`background:${color};color:${darkColor}`}
              onClick={send}
              disabled={!input.trim()}
            >
              {'↑'}
            </button>
          </div>
        </div>
      </div>
    </>
  );
}
