import { useState, useEffect, useRef, useCallback, useMemo } from 'preact/hooks';
import { marked } from 'marked';
import DOMPurify from 'dompurify';
import { createClient } from '../api';
import { AgentIcon, agentColor, agentDarkColor } from './agent-icon';

marked.setOptions({
  breaks: true,
  gfm: true,
});

function renderMarkdownHtml(text) {
  if (!text) return '';
  const raw = marked.parse(text);
  return DOMPurify.sanitize(raw);
}

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

// --- 承認待ちカード（カンプ 1b / #425 再設計）---
// 表示条件は「ペイン画面に permission ダイアログが実在する」こと（サーバーが
// 画面キャプチャから検知してペイン情報に付与する）。transcript からの推定は廃止した
// （auto mode のツール実行中と承認待ちが区別できず誤表示していた）。
// ボタンは実ダイアログの選択肢そのもの。押すと respond API がダイアログ実在を
// 再検証したうえで番号キーを送る
function ApprovalCard({ dialog, onChoose, canInteract, sending }) {
  const options = dialog.options || [];
  const disabled = !canInteract || sending;
  return (
    <div class="approval-card">
      <div class="approval-card-header">
        <span class="approval-card-dot" />
        <span class="approval-card-title">承認が必要</span>
      </div>
      {dialog.command && <div class="approval-card-body">{dialog.command}</div>}
      <div class="approval-card-actions" style="flex-direction:column;align-items:stretch">
        {options.map((opt, i) => (
          <button
            key={i}
            class={i === options.length - 1 && options.length > 1 ? 'approval-btn-deny' : 'approval-btn-allow'}
            onClick={() => onChoose(i + 1)}
            disabled={disabled}
            style={`padding:9px 12px${disabled ? ';opacity:.4;cursor:not-allowed' : ''}`}
          >{i + 1}. {opt}</button>
        ))}
      </div>
    </div>
  );
}

// --- 選択肢ボタン（カンプ 1c）---
function ChoiceButtons({ choices, onSelect }) {
  return (
    <div class="choice-buttons">
      {choices.map((choice, i) => (
        <button key={i} class={`choice-btn${i === 0 ? ' choice-btn-primary' : ''}`} onClick={() => onSelect(choice, i)}>
          {choice}
        </button>
      ))}
    </div>
  );
}

function MarkdownContent({ text, className }) {
  const html = useMemo(() => renderMarkdownHtml(text), [text]);
  return <div class={className} dangerouslySetInnerHTML={{ __html: html }} />;
}

function ChatMessage({ msg, agentType, paneId, onSend, canInteract }) {
  if (msg.role === 'user') {
    return <MarkdownContent text={msg.text} className="chat-user md-content" />;
  }

  const agentName = agentType || 'claude';

  return (
    <div class="chat-agent">
      <div class="chat-agent-avatar">
        <AgentIcon type={agentType} small />
        <span class="agent-name">{agentName}</span>
      </div>
      {msg.text && (
        <MarkdownContent text={msg.text} className="chat-agent-text md-content" />
      )}
      {msg.tools && msg.tools.map((tool, i) => (
        <ToolCard key={i} tool={tool} agentType={agentType} />
      ))}
      {msg.choices && msg.choices.length > 0 && (
        <ChoiceButtons
          choices={msg.choices}
          onSelect={(choice, idx) => onSend(String(idx + 1))}
        />
      )}
    </div>
  );
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

// --- スラッシュコマンド静的候補（カンプ 1e）---
const SLASH_COMMANDS = [
  { cmd: '/compact', desc: 'コンテキストを圧縮' },
  { cmd: '/cost', desc: '今セッションの使用量' },
  { cmd: '/clear', desc: '会話履歴をクリア' },
  { cmd: '/help', desc: 'ヘルプを表示' },
  { cmd: '/model', desc: 'モデルを切り替え' },
  { cmd: '/status', desc: '現在の状態を確認' },
];

function SlashCandidates({ filter, onSelect, agentColor: color }) {
  const filtered = filter
    ? SLASH_COMMANDS.filter(c => c.cmd.startsWith(filter))
    : SLASH_COMMANDS;
  if (filtered.length === 0) return null;
  return (
    <div class="slash-candidates">
      <div class="slash-candidates-header">
        <span class="slash-candidates-label">COMMANDS</span>
        <span class="slash-candidates-hint">長押しで一覧 ⌃</span>
      </div>
      {filtered.map((c, i) => (
        <div
          key={c.cmd}
          class={`slash-candidate${i === 0 ? ' slash-candidate-active' : ''}`}
          onClick={() => onSelect(c.cmd)}
        >
          <span class="slash-candidate-cmd" style={i === 0 ? `color:${color}` : ''}>{c.cmd}</span>
          <span class="slash-candidate-desc">{c.desc}</span>
        </div>
      ))}
    </div>
  );
}

// --- モデル / エフォートシート（カンプ 1f）---
const CLAUDE_MODELS = [
  { id: 'opus', name: 'Opus 4.5', desc: '複雑なタスク向け・最高性能' },
  { id: 'sonnet', name: 'Sonnet 4.6', desc: '日常タスク・高速' },
  { id: 'haiku', name: 'Haiku 4.5', desc: '軽量・最速' },
];
const EFFORT_LEVELS = [
  { id: 'off', label: 'off' },
  { id: 'low', label: '低' },
  { id: 'medium', label: '中' },
  { id: 'high', label: '高' },
];

function ModelEffortSheet({ currentModel, currentEffort, onSelectModel, onSelectEffort, onClose, agentType }) {
  const color = agentColor(agentType);
  const [selectedEffort, setSelectedEffort] = useState(() => {
    if (!currentEffort) return null;
    const lower = currentEffort.toLowerCase();
    if (lower.includes('high') || lower === '高') return 'high';
    if (lower.includes('medium') || lower === '中') return 'medium';
    if (lower.includes('low') || lower === '低') return 'low';
    return null;
  });

  return (
    <div class="sheet-overlay" onClick={onClose}>
      <div class="sheet-panel" onClick={e => e.stopPropagation()}>
        <div class="sheet-handle" />
        <div class="sheet-section-label">MODEL</div>
        <div class="sheet-model-list">
          {CLAUDE_MODELS.map(m => {
            const isActive = currentModel && currentModel.toLowerCase().includes(m.id);
            return (
              <div
                key={m.id}
                class={`sheet-model-item${isActive ? ' sheet-model-active' : ''}`}
                style={isActive ? `border-color:${color}` : ''}
                onClick={() => onSelectModel(m.name)}
              >
                <AgentIcon type={agentType} small />
                <div class="sheet-model-info">
                  <span class="sheet-model-name">{m.name}</span>
                  <span class="sheet-model-desc">{m.desc}</span>
                </div>
                {isActive && <span class="sheet-model-check" style={`color:${color}`}>✓</span>}
              </div>
            );
          })}
        </div>
        <div class="sheet-section-label">THINKING EFFORT</div>
        <div class="sheet-effort-bar">
          {EFFORT_LEVELS.map(e => (
            <button
              key={e.id}
              class={`sheet-effort-btn${selectedEffort === e.id ? ' sheet-effort-active' : ''}`}
              style={selectedEffort === e.id ? `background:${color};color:${agentDarkColor(agentType)}` : ''}
              onClick={() => {
                setSelectedEffort(e.id);
                onSelectEffort(e.id);
              }}
            >{e.label}</button>
          ))}
        </div>
        <div class="sheet-footer-note">
          選択は /model・/effort としてエージェントに送信されます
        </div>
      </div>
    </div>
  );
}

// --- ファイル添付シート（カンプ 1g）---
function AttachSheet({ onClose, onFileSelected, pendingFile, agentType }) {
  const fileInputRef = useRef(null);

  function handleSource(source) {
    if (source === 'file' && fileInputRef.current) {
      fileInputRef.current.click();
    } else if (source === 'camera') {
      const input = document.createElement('input');
      input.type = 'file';
      input.accept = 'image/*';
      input.capture = 'environment';
      input.onchange = (e) => {
        const file = e.target.files?.[0];
        if (file) onFileSelected(file);
      };
      input.click();
    } else if (source === 'photo') {
      const input = document.createElement('input');
      input.type = 'file';
      input.accept = 'image/*,video/*';
      input.onchange = (e) => {
        const file = e.target.files?.[0];
        if (file) onFileSelected(file);
      };
      input.click();
    }
  }

  return (
    <div class="sheet-overlay" onClick={onClose}>
      <div class="sheet-panel" onClick={e => e.stopPropagation()}>
        <div class="sheet-handle" />
        <div class="attach-sources">
          <div class="attach-source" onClick={() => handleSource('camera')}>
            <span class="attach-source-icon">
              <svg width="22" height="22" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5"><rect x="2" y="6" width="20" height="14" rx="3"/><circle cx="12" cy="13" r="4"/><path d="M7 6V5a2 2 0 012-2h6a2 2 0 012 2v1"/></svg>
            </span>
            <span class="attach-source-label">カメラ</span>
          </div>
          <div class="attach-source" onClick={() => handleSource('photo')}>
            <span class="attach-source-icon">
              <svg width="22" height="22" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5"><rect x="2" y="3" width="20" height="18" rx="3"/><circle cx="8.5" cy="8.5" r="2"/><path d="M22 15l-5-5L5 21"/></svg>
            </span>
            <span class="attach-source-label">写真</span>
          </div>
          <div class="attach-source" onClick={() => handleSource('file')}>
            <span class="attach-source-icon">
              <svg width="22" height="22" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5"><path d="M14 2H6a2 2 0 00-2 2v16a2 2 0 002 2h12a2 2 0 002-2V8z"/><polyline points="14 2 14 8 20 8"/></svg>
            </span>
            <span class="attach-source-label">ファイル</span>
          </div>
        </div>
        <div class="attach-note">
          リモートの作業ディレクトリにアップロードし、パスをエージェントに渡します
        </div>
        <input
          ref={fileInputRef}
          type="file"
          style="display:none"
          onChange={e => {
            const file = e.target.files?.[0];
            if (file) onFileSelected(file);
          }}
        />
      </div>
    </div>
  );
}

function PendingAttachment({ file, uploadState, onRemove }) {
  const icon = file.type?.startsWith('image/') ? 'img' : 'file';
  const sizeStr = file.size < 1024 * 1024
    ? `${(file.size / 1024).toFixed(0)}KB`
    : `${(file.size / (1024 * 1024)).toFixed(1)}MB`;
  return (
    <div class="pending-attach">
      <span class="pending-attach-icon">
        {icon === 'img' ? (
          <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5"><rect x="2" y="3" width="20" height="18" rx="3"/><circle cx="8.5" cy="8.5" r="2"/><path d="M22 15l-5-5L5 21"/></svg>
        ) : (
          <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5"><path d="M14 2H6a2 2 0 00-2 2v16a2 2 0 002 2h12a2 2 0 002-2V8z"/><polyline points="14 2 14 8 20 8"/></svg>
        )}
      </span>
      <div class="pending-attach-info">
        <span class="pending-attach-name">{file.name}</span>
        <span class={`pending-attach-status ${uploadState?.status || 'pending'}`}>
          {uploadState?.status === 'uploading' && `↑ ${sizeStr} ・ アップロード中...`}
          {uploadState?.status === 'done' && `↑ ${sizeStr} ・ 完了`}
          {uploadState?.status === 'error' && `エラー: ${uploadState.error}`}
          {(!uploadState || uploadState.status === 'pending') && `${sizeStr}`}
        </span>
      </div>
      <button class="pending-attach-remove" onClick={onRemove}>✕</button>
    </div>
  );
}


export function ChatView({ paneId, info, agentType, onStop, me }) {
  const [messages, setMessages] = useState([]);
  const [loading, setLoading] = useState(true);
  const [input, setInput] = useState('');
  const [showSlash, setShowSlash] = useState(false);
  const [showModelSheet, setShowModelSheet] = useState(false);
  const [showAttachSheet, setShowAttachSheet] = useState(false);
  const [pendingFile, setPendingFile] = useState(null);
  const [uploadState, setUploadState] = useState(null);
  const [respondSending, setRespondSending] = useState(false);
  const scrollRef = useRef(null);
  const inputRef = useRef(null);
  const timerRef = useRef(null);
  const atBottomRef = useRef(true);

  // 承認待ち = ペイン画面に permission ダイアログが実在する（#425。サーバー検知）
  const permissionDialog = info?.permission_dialog || null;
  const isRunning = !permissionDialog && info && (info.state === 'busy' || info.state === 'running');
  const canInteract = me && (me.role === 'interact' || me.role === 'manage' || me.role === 'admin');

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
  }, [messages, permissionDialog]);

  function onScroll() {
    const el = scrollRef.current;
    if (!el) return;
    atBottomRef.current = el.scrollHeight - el.scrollTop - el.clientHeight < 60;
  }

  async function send(text) {
    const t = (text || '').trim();
    if (!t) return;
    if (navigator.vibrate) navigator.vibrate(10);
    try {
      await createClient().input(paneId, t, true);
    } catch {}
  }

  // permission ダイアログへの応答（#425）。サーバーがダイアログ実在を再検証する。
  // 409（既に解消済み）は次の panes ポーリングでカードが消えるため黙って無視する
  async function respondToDialog(choice) {
    if (respondSending) return;
    setRespondSending(true);
    if (navigator.vibrate) navigator.vibrate(10);
    try {
      await createClient().respond(paneId, choice);
    } catch {}
    setRespondSending(false);
  }

  async function sendFromComposer() {
    const text = input.trim();
    if (!text) return;
    setInput('');
    setShowSlash(false);

    // 添付ファイルがある場合はパスをプレフィックスに
    if (pendingFile && uploadState?.status === 'done' && uploadState.path) {
      await send(`${uploadState.path} ${text}`);
    } else {
      await send(text);
    }
    setPendingFile(null);
    setUploadState(null);
    inputRef.current?.focus();
  }

  function onInputChange(e) {
    const val = e.target.value;
    setInput(val);
    // `/` で始まるとスラコマ候補を表示
    if (val.startsWith('/') && !val.includes(' ')) {
      setShowSlash(true);
    } else {
      setShowSlash(false);
    }
  }

  function onKeyDown(e) {
    if (e.isComposing) return;
    // 送信は cmd/ctrl+Enter か送信ボタンのみ。素の Enter は改行として入力する
    // （#429: モバイルキーボードの改行キーで送信されてしまい改行が打てなかった）
    if (e.key === 'Enter' && (e.metaKey || e.ctrlKey)) {
      e.preventDefault();
      sendFromComposer();
    }
  }

  function onSlashSelect(cmd) {
    if (cmd === '/model') {
      setShowModelSheet(true);
      setInput('');
      setShowSlash(false);
      return;
    }
    setInput('');
    setShowSlash(false);
    send(cmd);
  }

  function onModelSelect(modelName) {
    setShowModelSheet(false);
    send(`/model ${modelName}`);
  }

  function onEffortSelect(level) {
    setShowModelSheet(false);
    send(`/effort ${level}`);
  }

  async function onFileSelected(file) {
    setShowAttachSheet(false);
    setPendingFile(file);
    setUploadState({ status: 'uploading' });
    try {
      const client = createClient();
      const result = await client.upload(paneId, file);
      setUploadState({ status: 'done', path: result.path });
    } catch (e) {
      setUploadState({ status: 'error', error: e.message });
    }
  }

  const color = agentColor(agentType);
  const darkColor = agentDarkColor(agentType);
  const placeholder = `${agentType || 'agent'} に返信...`;

  let lastDate = null;

  return (
    <>
      <div class={`chat-scroll${showSlash ? ' chat-scroll-dimmed' : ''}`} ref={scrollRef} onScroll={onScroll}>
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
              <ChatMessage
                msg={msg}
                agentType={agentType}
                paneId={paneId}
                onSend={send}
                canInteract={canInteract}
              />
            </div>
          );
        })}
        {permissionDialog && (
          <ApprovalCard
            dialog={permissionDialog}
            canInteract={canInteract}
            sending={respondSending}
            onChoose={respondToDialog}
          />
        )}
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

      {/* スラコマ候補（カンプ 1e）*/}
      {showSlash && (
        <SlashCandidates
          filter={input}
          onSelect={onSlashSelect}
          agentColor={color}
        />
      )}

      <div class="composer">
        <div class={`composer-box${showSlash ? ' composer-box-active' : ''}`} style={showSlash ? `border-color:${color}` : ''}>
          {/* 添付プレビュー（カンプ 1g） */}
          {pendingFile && (
            <PendingAttachment
              file={pendingFile}
              uploadState={uploadState}
              onRemove={() => { setPendingFile(null); setUploadState(null); }}
            />
          )}
          <textarea
            ref={inputRef}
            class="composer-input"
            value={input}
            onInput={onInputChange}
            onKeyDown={onKeyDown}
            placeholder={placeholder}
            autocomplete="off"
            autocorrect="off"
            autocapitalize="off"
            spellcheck={false}
            rows={1}
          />
          <div class="composer-toolbar">
            <button
              class={`composer-btn-attach${pendingFile ? ' composer-btn-attach-active' : ''}`}
              style={pendingFile ? `border-color:${color};color:${color}` : ''}
              onClick={() => setShowAttachSheet(true)}
            >+</button>
            {info?.model && (
              <button class="composer-chip" onClick={() => setShowModelSheet(true)}>
                {info.model} ▾
              </button>
            )}
            {info?.effort && (
              <button class="composer-chip" onClick={() => setShowModelSheet(true)}>
                {info.effort} ▾
              </button>
            )}
            <button
              class="composer-send"
              style={`background:${color};color:${darkColor}`}
              onClick={sendFromComposer}
              disabled={!input.trim() && !(pendingFile && uploadState?.status === 'done')}
            >
              {'↑'}
            </button>
          </div>
        </div>
      </div>

      {/* モデル / エフォートシート（カンプ 1f）*/}
      {showModelSheet && (
        <ModelEffortSheet
          currentModel={info?.model}
          currentEffort={info?.effort}
          onSelectModel={onModelSelect}
          onSelectEffort={onEffortSelect}
          onClose={() => setShowModelSheet(false)}
          agentType={agentType}
        />
      )}

      {/* ファイル添付シート（カンプ 1g）*/}
      {showAttachSheet && (
        <AttachSheet
          onClose={() => setShowAttachSheet(false)}
          onFileSelected={onFileSelected}
          pendingFile={pendingFile}
          agentType={agentType}
        />
      )}
    </>
  );
}
