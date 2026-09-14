// ユーザー向けタスクの画面（#1450 B3。リモート刷新の「人がやること」をスマホから片付ける）。
//
// ユーザーの原文は「スマホ版からでは、関連ファイルのダウンロードやテキストのコピーをして、
// スムーズにツイートや動画アップロードがスマホからでもできるようにして欲しい」。
// だからこの画面の主役は **コピーとダウンロード**で、返答 / 完了 / 却下がそれに続く。
//
// ## 何も新設していない
//
// - 操作は B1（#1450）の `Request::UserTask` を daemon が素通しするだけ
//   （経路は `tako_control::remote_tasks::TASK_ROUTES` が正本）
// - **添付のダウンロードは #1079 / #1085 のファイル API そのもの**。daemon が
//   添付の絶対パスを `{root, path_rel}` へ解決して返すので、ここは
//   そのファイル API を組むだけ（新しい配信経路は無い）
// - **その場で見せる（#1472）のも同じ 1 本**。`<img>` / `<video>` の `src` は
//   ダウンロードと**同じ URL に `disposition=inline` を足しただけ**で、
//   認可は `resolve_in_root` の 1 実装のまま。daemon 側は `Range`（206）に
//   応えるようになっただけで、受け口は増えていない。
//   画像 / 動画の判定も自分でやらない（daemon が `attachments[].preview` に載せる。
//   表は `tako_core::open_plan` = cmd+クリックや `tako open` と同じ 1 本）
// - 権限が足りないときの導線は #1452 の `PermissionRequest` をそのまま使う
//
// ## 語彙は PC 版（B2 = 右パネルの tasks ビュー）と同一
//
// 実装は共有しない（GPUI と PWA）が、**同じ言葉で同じ状態を指す**。とくに配送は
// `sent` を「届いた」と書かない（B1 の「分からないものを届いたと騙らない」を守る）。
//
// ## ポーリングの止め方は 1 実装（`usePolling`）
//
// 画面を離れたら `clearInterval`・裏へ回っているあいだは撃たない。この画面と
// 一覧画面のバッジが同じ 1 本を使う（タイマー実装を 2 つ作らない = 番犬が縛る）。
// ポーリングには意味がある: B1 は `list` が走るたびに配送の状態を確定させるので、
// **見ているあいだに `sent` が `delivered` へ変わる**。
import { useState, useEffect, useCallback, useRef } from 'preact/hooks';
import { createClient } from '../api';
import { MarkdownContent } from '../components/chat-view';
import { PermissionRequest, roleAtLeast } from '../components/permission-request';

/** 一覧を取り直す周期。画面とバッジで同じ値を使う */
export const TASKS_POLL_MS = 5000;

/**
 * Web Share API に**ファイル**を載せてよい上限。
 * daemon 側の `remote_tasks::SHARE_MAX_BYTES` と同じ値（番犬が一致を見る）。
 * これを超える動画は「ダウンロード → ファイル / 写真アプリ」経路に倒す
 */
export const SHARE_MAX_BYTES = 64 * 1024 * 1024;

/** 操作（返答 / 完了 / 却下）に要る role。daemon の `TASK_ROUTES` と揃える */
const ACT_ROLE = 'interact';
/** 添付のダウンロードに要る role（`FILE_ROUTES` の download と同じ） */
const DOWNLOAD_ROLE = 'interact';

// #1450 B3 の A/B。PWA はブラウザで動くので env（`TAKO_1450B3_LEGACY=1`）が届かない。
// **逃げ道はこの 1 つだけ**で、legacy 腕では画面もナビのバッジも出ない（B3 以前の見え方）
export function legacy1450b3() {
  try {
    return new URLSearchParams(window.location.search).get('tako_1450b3_legacy') === '1';
  } catch {
    return false;
  }
}

// --- 語彙（PC 版 = `ui_text/panel.rs` の tasks_* と同じ言葉）---

const KIND_LABEL = {
  review: 'レビュー',
  confirm: '確認',
  permission: '許可',
  post: '投稿',
  other: 'その他',
};

const DECISIONS = [
  { key: 'approve', label: '承認' },
  { key: 'reject', label: '却下' },
  { key: 'needs_change', label: '直してほしい' },
  { key: 'answered', label: '回答' },
];

// **`sent` を「届いた」と書かない**（B1 / B2 と同じ物差し）
const DELIVERY_LABEL = {
  sent: '送信済み（確認待ち）',
  launched: 'master を起動しました',
  delivered: 'master に届きました',
  failed: '届きませんでした',
};

// 確定したものだけ色を変える（`sent` は未確定なので緑にしない）
const DELIVERY_TONE = {
  delivered: 'ok',
  failed: 'error',
};

function kindLabel(kind) {
  return KIND_LABEL[kind] || kind || '';
}

function decisionLabel(key) {
  const found = DECISIONS.find(d => d.key === key);
  return found ? found.label : key;
}

/** 相対時刻（PC 版の `tasks_ago` と同じ刻み） */
export function ago(atSecs, nowSecs) {
  const secs = Math.max(0, (nowSecs || Math.floor(Date.now() / 1000)) - (atSecs || 0));
  if (secs < 60) return 'たった今';
  if (secs < 3600) return `${Math.floor(secs / 60)} 分前`;
  if (secs < 86400) return `${Math.floor(secs / 3600)} 時間前`;
  return `${Math.floor(secs / 86400)} 日前`;
}

export function formatSize(bytes) {
  if (bytes === null || bytes === undefined) return '';
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  if (bytes < 1024 * 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
  return `${(bytes / (1024 * 1024 * 1024)).toFixed(2)} GB`;
}

// --- ポーリング（止め方の 1 実装）---

/**
 * `TASKS_POLL_MS` ごとに `fn` を呼ぶ。**画面を離れたら止まり、裏へ回っているあいだは撃たない**。
 * この 1 本を画面とバッジの両方が使う（タイマー実装を 2 つ作らない）
 */
export function usePolling(fn, deps = []) {
  const saved = useRef(fn);
  saved.current = fn;
  useEffect(() => {
    let stopped = false;
    const tick = () => {
      if (stopped) return;
      // スマホをポケットに入れているあいだ叩き続けない
      if (typeof document !== 'undefined' && document.visibilityState === 'hidden') return;
      saved.current();
    };
    tick();
    const timer = setInterval(tick, TASKS_POLL_MS);
    const onVisibility = () => tick();
    document.addEventListener('visibilitychange', onVisibility);
    return () => {
      stopped = true;
      clearInterval(timer);
      document.removeEventListener('visibilitychange', onVisibility);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, deps);
}

/**
 * ナビに出す未完了件数（一覧画面が使う）。
 * 件数の正本は daemon の `open_count`（画面で数え直さない）
 */
export function useOpenTaskCount(enabled = true) {
  const [count, setCount] = useState(null);
  usePolling(() => {
    if (!enabled || legacy1450b3()) return;
    createClient()
      .tasks()
      .then(r => setCount(typeof r.open_count === 'number' ? r.open_count : null))
      // 取れないときはバッジを出さない（0 と嘘をつかない）
      .catch(() => setCount(null));
  }, [enabled]);
  return count;
}

// --- コピー（ワンタップ）---

/**
 * クリップボードへ入れる。`navigator.clipboard` が使えないブラウザ（http 経由・古い WebView）
 * のために `textarea` + `execCommand` のフォールバックを持つ。
 * **成否を返す**ので、押した結果が無言にならない
 */
export async function copyText(text) {
  try {
    if (navigator.clipboard && navigator.clipboard.writeText) {
      await navigator.clipboard.writeText(text);
      return true;
    }
  } catch {
    // フォールバックへ落ちる
  }
  try {
    const area = document.createElement('textarea');
    area.value = text;
    area.setAttribute('readonly', '');
    area.style.position = 'fixed';
    area.style.opacity = '0';
    document.body.appendChild(area);
    area.select();
    const ok = document.execCommand('copy');
    document.body.removeChild(area);
    return ok;
  } catch {
    return false;
  }
}

/** テキストの共有シートを出せるか（出せないときはボタンを出さない） */
function canShareText() {
  return typeof navigator !== 'undefined' && typeof navigator.share === 'function';
}

/** 添付を共有シートへ載せられるか（大きすぎるものはダウンロードへ倒す） */
function canShareFile(att) {
  if (!canShareText() || typeof navigator.canShare !== 'function') return false;
  if (!att.available) return false;
  const size = att.size;
  if (typeof size !== 'number' || size > SHARE_MAX_BYTES) return false;
  try {
    return navigator.canShare({ files: [new File([new Blob()], att.name || 'file')] });
  } catch {
    return false;
  }
}

// --- 添付の URL（**この 1 本だけが配信経路を知っている**）---

/**
 * 添付を取りに行く URL。**保存もその場再生も同じ経路**（#1079 のファイル API）で、
 * 違うのは `disposition` だけ:
 *
 * - 既定（保存）は daemon が `Content-Disposition: attachment` を付ける
 * - `inline: true` は `Content-Type` が実体の型（`video/mp4` 等）になり
 *   `inline` で返る。`<img>` / `<video>` の src はこちら（#1472）
 *
 * **新しい経路をここ以外で組まない**（番犬が配信経路の綴りを 1 か所に縛る）
 */
export function attachmentUrl(base, att, inline = false) {
  if (!att || !att.available) return null;
  const qs = new URLSearchParams({ root: att.root, path: att.path_rel });
  if (inline) qs.set('disposition', 'inline');
  return `${base}/api/files/download?${qs.toString()}`;
}

// --- アイコン（絵文字は使わない。SVG で描く）---

function DownloadIcon() {
  return (
    <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8">
      <path d="M12 3v12m0 0 4-4m-4 4-4-4M4 20h16" />
    </svg>
  );
}

function CopyIcon() {
  return (
    <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8">
      <rect x="9" y="9" width="11" height="11" rx="2" />
      <path d="M5 15V5a2 2 0 0 1 2-2h10" />
    </svg>
  );
}

function ShareIcon() {
  return (
    <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8">
      <path d="M12 3v13m0-13 4 4m-4-4-4 4" />
      <path d="M5 14v5a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2v-5" />
    </svg>
  );
}

function BackIcon() {
  return (
    <svg width="17" height="17" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8">
      <path d="M15 5l-7 7 7 7" />
    </svg>
  );
}

// --- 一覧 ---

function TaskRow({ task, onOpen }) {
  const responses = task.responses || [];
  const delivery = task.delivery;
  return (
    <button class="task-row" data-testid="task-row" data-task={task.id} onClick={onOpen}>
      <div class="task-row-head">
        <span class={`task-kind kind-${task.kind}`}>{kindLabel(task.kind)}</span>
        <span class="task-row-title">{task.title}</span>
      </div>
      <div class="task-row-meta">
        <span class="task-id">{task.id}</span>
        {task.project && <span class="task-project">{task.project}</span>}
        {task.due && <span class="task-due">期限 {task.due}</span>}
        {responses.length > 0 && <span class="task-thread-count">やりとり {responses.length}</span>}
        {delivery && (
          <span class={`task-delivery ${DELIVERY_TONE[delivery.state] || ''}`}>
            {DELIVERY_LABEL[delivery.state] || delivery.state}
          </span>
        )}
        {task.status !== 'open' && (
          <span class="task-status-done">{task.status === 'done' ? '完了' : '却下'}</span>
        )}
      </div>
    </button>
  );
}

// --- 詳細 ---

/**
 * 添付をその場で見せる（#1472）。
 *
 * - 画像は `<img>`（タップで全画面へ）・動画は `<video controls preload="metadata">`
 * - **詳細を開いたときだけ**描く（一覧には出さないので、開くまで 1 バイトも取りに行かない）
 * - `preload="metadata"` なので動画は尺と最初のフレームぶんしか読まない。
 *   本体はシーク（`Range`）で必要なところだけ流れてくる
 *
 * 種別（`att.preview`）は daemon が決める。ここに拡張子の表を持たない
 */
function AttachmentPreview({ att, base, onZoom }) {
  const src = attachmentUrl(base, att, true);
  if (!src) return null;
  if (att.preview === 'image') {
    return (
      <button class="task-attachment-thumb" data-testid="task-preview-image-open" onClick={onZoom}>
        <img
          class="task-attachment-image"
          data-testid="task-preview-image"
          src={src}
          alt={att.name || ''}
          loading="lazy"
          decoding="async"
        />
      </button>
    );
  }
  if (att.preview === 'video') {
    return (
      <video
        class="task-attachment-video"
        data-testid="task-preview-video"
        src={src}
        controls
        preload="metadata"
        playsinline
        webkit-playsinline="true"
      />
    );
  }
  return null;
}

/** 拡大表示（タップで閉じる。スマホには Esc が無いので閉じるボタンも出す） */
function ImageZoom({ att, base, onClose }) {
  const src = attachmentUrl(base, att, true);
  useEffect(() => {
    const onKey = e => {
      if (e.key === 'Escape') onClose();
    };
    document.addEventListener('keydown', onKey);
    return () => document.removeEventListener('keydown', onKey);
  }, [onClose]);
  if (!src) return null;
  return (
    <div class="task-image-zoom" data-testid="task-image-zoom" onClick={onClose}>
      <button class="btn task-image-zoom-close" data-testid="task-image-zoom-close" onClick={onClose}>
        閉じる
      </button>
      <img class="task-image-zoom-img" src={src} alt={att.name || ''} />
    </div>
  );
}

function Attachment({ task, att, canDownload, base }) {
  const [notice, setNotice] = useState(null);
  const [sharing, setSharing] = useState(false);
  const [zoomed, setZoomed] = useState(false);
  const href = attachmentUrl(base, att);
  // 見せられるのは「実体があって・この端末で取りに行けて・種別が分かる」ときだけ
  const showPreview = Boolean(href) && canDownload && (att.preview === 'image' || att.preview === 'video');

  async function share() {
    setSharing(true);
    setNotice(null);
    try {
      const resp = await fetch(href);
      if (!resp.ok) throw new Error(`HTTP ${resp.status}`);
      const blob = await resp.blob();
      const file = new File([blob], att.name, { type: blob.type || 'application/octet-stream' });
      await navigator.share({ files: [file], title: task.title });
    } catch (e) {
      // 共有シートを閉じただけ（AbortError）はエラーにしない
      if (!e || e.name !== 'AbortError') {
        setNotice(`共有できませんでした: ${(e && e.message) || e}`);
      }
    } finally {
      setSharing(false);
    }
  }

  return (
    <div class="task-attachment" data-testid="task-attachment">
      {showPreview && <AttachmentPreview att={att} base={base} onZoom={() => setZoomed(true)} />}
      {zoomed && <ImageZoom att={att} base={base} onClose={() => setZoomed(false)} />}
      <div class="task-attachment-info">
        <span class="task-attachment-name">{att.name || att.path}</span>
        <span class="task-attachment-meta">
          {att.exists === false ? '見つかりません' : formatSize(att.size)}
        </span>
      </div>
      {att.available && canDownload ? (
        <div class="task-attachment-actions">
          {/* daemon が Content-Disposition: attachment を付けるので、素の遷移で
              iOS / Android とも保存シートが開く（#1079 と同じ経路） */}
          <a class="btn" data-testid="task-download" href={href} download={att.name}>
            <DownloadIcon />
            端末に保存
          </a>
          {canShareFile(att) && (
            <button class="btn" disabled={sharing} onClick={share}>
              <ShareIcon />
              {sharing ? '準備中...' : 'アプリへ共有'}
            </button>
          )}
        </div>
      ) : (
        <span class="task-attachment-why">
          {att.exists === false
            ? 'ファイルが消えています'
            : !canDownload
              ? 'この端末では保存できません（権限が足りません）'
              : 'この端末からは開けません（PC のファイルツリーに出ていないフォルダです）'}
        </span>
      )}
      {notice && <div class="file-notice error">{notice}</div>}
    </div>
  );
}

function CopyRow({ item }) {
  const [state, setState] = useState(null);
  async function run() {
    const ok = await copyText(item.text);
    setState(ok ? 'コピーしました' : 'コピーできませんでした');
    setTimeout(() => setState(null), 2500);
  }
  return (
    <div class="task-copy" data-testid="task-copy">
      <div class="task-copy-head">
        <span class="task-copy-label">{item.label || 'テキスト'}</span>
        <div class="task-copy-actions">
          <button class="btn" data-testid="task-copy-btn" onClick={run}>
            <CopyIcon />
            コピー
          </button>
          {canShareText() && (
            <button
              class="btn"
              onClick={() => navigator.share({ text: item.text }).catch(() => {})}
            >
              <ShareIcon />
              共有
            </button>
          )}
        </div>
      </div>
      <pre class="task-copy-text">{item.text}</pre>
      {state && <div class="task-copy-state">{state}</div>}
    </div>
  );
}

function RespondForm({ task, onDone }) {
  const [decision, setDecision] = useState(null);
  const [comment, setComment] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState(null);

  async function send() {
    if (!decision) return;
    setBusy(true);
    setError(null);
    try {
      const updated = await createClient().respondTask(task.id, decision, comment);
      // 返したらフォームは空に戻す（次のタスクへ前の判断を持ち越さない = B2 と同じ）
      setDecision(null);
      setComment('');
      onDone(updated);
    } catch (e) {
      setError(e.message);
    } finally {
      setBusy(false);
    }
  }

  return (
    <div class="task-respond" data-testid="task-respond">
      <div class="task-decisions">
        {DECISIONS.map(d => (
          <button
            key={d.key}
            class={`filter-chip${decision === d.key ? ' active' : ''}`}
            data-testid={`task-decision-${d.key}`}
            onClick={() => setDecision(decision === d.key ? null : d.key)}
          >
            {d.label}
          </button>
        ))}
      </div>
      <textarea
        class="task-comment"
        placeholder="コメント（任意）"
        value={comment}
        onInput={e => setComment(e.currentTarget.value)}
      />
      <div class="task-respond-actions">
        {/* 判断を選ぶまで押せない（誤爆防止）。理由も出す = B2 と同じ */}
        {!decision && <span class="task-hint">判断を選ぶと返せます</span>}
        <button class="btn btn-primary" data-testid="task-send" disabled={!decision || busy} onClick={send}>
          {busy ? '送信中...' : '返す'}
        </button>
      </div>
      {error && <div class="file-notice error">返せませんでした: {error}</div>}
    </div>
  );
}

function TaskDetail({ task, me, onBack, onUpdated, onMeRefresh }) {
  const base = createClient().base();
  const canAct = roleAtLeast((me && me.role) || 'observe', ACT_ROLE);
  const canDownload = roleAtLeast((me && me.role) || 'observe', DOWNLOAD_ROLE);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState(null);
  const attachments = task.attachments || [];
  const copyTexts = task.copy_texts || [];
  const links = task.links || [];
  const responses = [...(task.responses || [])].reverse();
  const delivery = task.delivery;

  async function setStatus(kind) {
    setBusy(true);
    setError(null);
    try {
      const client = createClient();
      await (kind === 'done' ? client.doneTask(task.id) : client.dismissTask(task.id));
      onUpdated();
      onBack();
    } catch (e) {
      setError(e.message);
      setBusy(false);
    }
  }

  return (
    <div class="task-detail" data-testid="task-detail" data-task={task.id}>
      <div class="task-detail-head">
        <button class="btn task-back" onClick={onBack}>
          <BackIcon />
          一覧
        </button>
        <span class={`task-kind kind-${task.kind}`}>{kindLabel(task.kind)}</span>
      </div>
      <h1 class="task-title">{task.title}</h1>
      <div class="task-row-meta">
        <span class="task-id">{task.id}</span>
        {task.project && <span class="task-project">{task.project}</span>}
        {task.due && <span class="task-due">期限 {task.due}</span>}
        {task.created_by && <span class="task-by">{task.created_by}</span>}
      </div>

      {task.body ? (
        <MarkdownContent text={task.body} className="task-body md-content" />
      ) : (
        <p class="task-body-empty">（説明はありません）</p>
      )}

      {attachments.length > 0 && (
        <section class="task-section">
          <h2>添付</h2>
          {attachments.map(att => (
            <Attachment key={att.path} task={task} att={att} canDownload={canDownload} base={base} />
          ))}
        </section>
      )}

      {copyTexts.length > 0 && (
        <section class="task-section">
          <h2>コピー用のテキスト</h2>
          {copyTexts.map((item, i) => (
            <CopyRow key={`${item.label}-${i}`} item={item} />
          ))}
        </section>
      )}

      {links.length > 0 && (
        <section class="task-section">
          <h2>リンク</h2>
          {links.map(url => (
            <a key={url} class="task-link" href={url} target="_blank" rel="noreferrer noopener">
              {url}
            </a>
          ))}
        </section>
      )}

      {task.status === 'open' && (
        <section class="task-section">
          <h2>返答</h2>
          {canAct ? (
            <RespondForm task={task} onDone={onUpdated} />
          ) : (
            <PermissionRequest
              me={me}
              need={ACT_ROLE}
              what="タスクの返答と完了"
              onGranted={onMeRefresh}
            />
          )}
        </section>
      )}

      {delivery && (
        <div class={`task-delivery-line ${DELIVERY_TONE[delivery.state] || ''}`}>
          <span>配送</span>
          <span>{DELIVERY_LABEL[delivery.state] || delivery.state}</span>
          {delivery.reason && <span class="task-delivery-reason">{delivery.reason}</span>}
        </div>
      )}

      {responses.length > 0 && (
        <section class="task-section">
          <h2>やりとり</h2>
          {responses.map((r, i) => (
            <div key={`${r.at}-${i}`} class="task-response">
              <div class="task-response-head">
                <span class="task-response-decision">{decisionLabel(r.decision)}</span>
                <span class="task-response-via">{r.via}</span>
                <span class="task-response-at">{ago(r.at)}</span>
              </div>
              {r.comment && <div class="task-response-comment">{r.comment}</div>}
            </div>
          ))}
        </section>
      )}

      {task.status === 'open' && canAct && (
        <div class="task-footer-actions">
          <button class="btn" data-testid="task-dismiss" disabled={busy} onClick={() => setStatus('dismiss')}>
            却下
          </button>
          <button class="btn btn-primary" data-testid="task-done" disabled={busy} onClick={() => setStatus('done')}>
            完了
          </button>
        </div>
      )}
      {error && <div class="file-notice error">{error}</div>}
    </div>
  );
}

// --- 画面本体 ---

export function TasksPage({ me, id, onMeRefresh }) {
  const [data, setData] = useState(null);
  const [error, setError] = useState(null);
  const [kind, setKind] = useState('');
  const [showDone, setShowDone] = useState(false);
  const legacy = legacy1450b3();

  const refresh = useCallback(() => {
    if (legacy) return;
    createClient()
      .tasks(showDone ? { all: true } : {})
      .then(r => {
        setData(r);
        setError(null);
      })
      .catch(e => setError(e.message));
  }, [legacy, showDone]);

  usePolling(refresh, [refresh]);

  if (legacy) {
    return (
      <div class="page">
        <div class="empty-state">
          <h2>タスクの画面がありません</h2>
          <p>この版ではスマホからタスクを見られません。</p>
        </div>
      </div>
    );
  }

  const tasks = (data && data.tasks) || [];
  const selected = id ? tasks.find(t => t.id === id) : null;
  const counts = (data && data.open_counts_by_kind) || [];

  if (id && data && !selected) {
    return (
      <div class="page">
        <div class="empty-state">
          <h2>見つかりません</h2>
          <p>このタスクは完了・却下されたか、絞り込みの外にあります。</p>
          <button class="btn" onClick={() => { window.location.hash = '#/tasks'; }}>一覧へ</button>
        </div>
      </div>
    );
  }

  if (selected) {
    return (
      <div class="page tasks-page">
        <TaskDetail
          task={selected}
          me={me}
          onBack={() => { window.location.hash = '#/tasks'; }}
          onUpdated={refresh}
          onMeRefresh={onMeRefresh}
        />
      </div>
    );
  }

  const shown = kind ? tasks.filter(t => t.kind === kind) : tasks;

  return (
    <div class="page tasks-page">
      <div class="panes-header">
        <div class="panes-header-row">
          <button class="btn task-back" onClick={() => { window.location.hash = '#/panes'; }}>
            <BackIcon />
            ペイン
          </button>
          <span class="tasks-count">
            {data ? `${data.open_count} 件待ち` : ''}
          </span>
        </div>
        <div class="filter-row">
          <button
            class={`filter-chip${kind === '' ? ' active' : ''}`}
            onClick={() => setKind('')}
          >
            すべて
          </button>
          {counts.filter(c => c.count > 0).map(c => (
            <button
              key={c.kind}
              class={`filter-chip${kind === c.kind ? ' active' : ''}`}
              onClick={() => setKind(kind === c.kind ? '' : c.kind)}
            >
              {kindLabel(c.kind)} {c.count}
            </button>
          ))}
          <button
            class={`filter-chip${showDone ? ' active' : ''}`}
            onClick={() => setShowDone(!showDone)}
          >
            完了も見る
          </button>
        </div>
      </div>

      {error && <div class="file-notice error">読み込めませんでした: {error}</div>}
      {data && data.attachment_error && (
        <div class="file-notice warn">添付の場所を解決できませんでした: {data.attachment_error}</div>
      )}

      {data && shown.length === 0 && (
        <div class="empty-state">
          <h2>{tasks.length === 0 ? '人がやることはありません' : 'この絞り込みに当てはまるものはありません'}</h2>
          <p>master が起票したタスクがここに並びます。</p>
        </div>
      )}

      <div class="task-list" data-testid="task-list">
        {shown.map(task => (
          <TaskRow
            key={task.id}
            task={task}
            onOpen={() => { window.location.hash = `#/tasks?id=${encodeURIComponent(task.id)}`; }}
          />
        ))}
      </div>
    </div>
  );
}
