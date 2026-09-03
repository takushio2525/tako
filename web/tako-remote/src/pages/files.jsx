// ファイルビュー（#1079。リモート刷新 柱 3-E / 編集は #1084 = 柱 3-F /
// SSH 先は #1085 = 柱 3-G）。
//
// スマホから PC のファイルを「見る → 中身を確かめる → 直す → 保存する」までを 1 画面で。
// 出せるのは **Mac のファイルツリーに現に出ているルートの配下だけ**で、
// 認可は daemon 側の純粋関数（`remote_files::resolve_in_root`）が正。
// ここは見せ方だけを持ち、パスの妥当性を画面側で判断しない
// （画面で弾いたつもりの形が API では通る、という食い違いを作らないため）。
//
// API は自前 fetch で叩く: 並行して `api.js` を改修している作業（#1077 / #1078）と
// 衝突させないため、このビューが使う分はこのファイルに閉じてある。
import { useState, useEffect, useCallback, useRef } from 'preact/hooks';
import { createClient } from '../api';

const TIMEOUT_MS = 15000;

// 本文プレビューを出す上限（daemon 側 MAX_TEXT_BYTES と揃える）
const PREVIEW_MAX_BYTES = 512 * 1024;

function base() {
  return createClient().base();
}

async function getJson(path) {
  const resp = await fetch(`${base()}${path}`, {
    signal: AbortSignal.timeout(TIMEOUT_MS),
  });
  const body = await resp.json().catch(() => ({}));
  if (!resp.ok) {
    const e = new Error(body.error || `HTTP ${resp.status}`);
    e.status = resp.status;
    e.kind = body.kind;
    throw e;
  }
  return body;
}

// 書き込み（保存 / 送り直し）。読み出しと同じく失敗は status / kind を載せて投げる
async function sendJson(method, path, body) {
  const resp = await fetch(`${base()}${path}`, {
    method,
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body || {}),
    // 保存は SFTP の往復を含むことがあるので読み出しより長く待つ（#966 の実測 1〜2 秒）
    signal: AbortSignal.timeout(TIMEOUT_MS * 2),
  });
  const out = await resp.json().catch(() => ({}));
  if (!resp.ok) {
    const e = new Error(out.error || `HTTP ${resp.status}`);
    e.status = resp.status;
    e.kind = out.kind;
    e.pending = out.pending === true;
    throw e;
  }
  return out;
}

function filesUrl(endpoint, root, path) {
  const params = new URLSearchParams();
  if (root) params.set('root', root);
  if (path) params.set('path', path);
  const qs = params.toString();
  return `${endpoint}${qs ? `?${qs}` : ''}`;
}

// --- 表示のための小道具 ---

function formatSize(bytes) {
  if (bytes === null || bytes === undefined) return '';
  if (bytes < 1024) return `${bytes} B`;
  const units = ['KB', 'MB', 'GB', 'TB'];
  let v = bytes / 1024;
  let i = 0;
  while (v >= 1024 && i < units.length - 1) { v /= 1024; i++; }
  return `${v < 10 ? v.toFixed(1) : Math.round(v)} ${units[i]}`;
}

function formatTime(epochSecs) {
  if (!epochSecs) return '';
  const d = new Date(epochSecs * 1000);
  const now = new Date();
  const sameYear = d.getFullYear() === now.getFullYear();
  const mm = String(d.getMonth() + 1).padStart(2, '0');
  const dd = String(d.getDate()).padStart(2, '0');
  const hh = String(d.getHours()).padStart(2, '0');
  const mi = String(d.getMinutes()).padStart(2, '0');
  return sameYear ? `${mm}/${dd} ${hh}:${mi}` : `${d.getFullYear()}/${mm}/${dd}`;
}

// 親フォルダの相対パス（先頭なら null = ルート一覧へ戻る）
function parentOf(path) {
  if (!path) return null;
  const i = path.lastIndexOf('/');
  return i < 0 ? '' : path.slice(0, i);
}

// `kind` は「フォルダかファイルか」の**ヒント**（一覧の行から分かっているときだけ付く）。
// 当たれば往復が 1 回で済む。外れても・無くても FilesPage が両方試すので、
// 認可も種別の判定も**サーバー側が正**であることは変わらない
function navigate(root, path, kind) {
  const params = new URLSearchParams();
  if (root) params.set('root', root);
  if (path) params.set('path', path);
  if (kind) params.set('kind', kind);
  const qs = params.toString();
  window.location.hash = `#/files${qs ? `?${qs}` : ''}`;
}

const FolderIcon = () => (
  <svg width="17" height="17" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8">
    <path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z" />
  </svg>
);

const FileIcon = () => (
  <svg width="17" height="17" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8">
    <path d="M14 3H7a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8z" />
    <path d="M14 3v5h5" />
  </svg>
);

const DownloadIcon = () => (
  <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9">
    <path d="M12 4v11" /><path d="M7 11l5 5 5-5" /><path d="M5 20h14" />
  </svg>
);

// --- 画面 ---

export function FilesPage({ me, root, path, hint }) {
  const [state, setState] = useState({ loading: true });

  const load = useCallback(async () => {
    setState({ loading: true });
    try {
      if (!root) {
        const body = await getJson('/api/files');
        setState({ loading: false, kind: 'roots', roots: body.roots || [] });
        return;
      }
      const asDir = async () => ({
        kind: 'dir',
        dir: await getJson(filesUrl('/api/files', root, path)),
      });
      const asFile = async () => ({
        kind: 'file',
        file: await getJson(filesUrl('/api/files/content', root, path)),
      });
      // ヒントがあればそちらを先に試す（外れたら種別違いのときだけもう一方へ）
      const [first, second] =
        hint === 'file' ? [asFile, asDir] : [asDir, asFile];
      const mismatch = ['not_a_directory', 'not_a_file'];
      try {
        setState({ loading: false, ...(await first()) });
      } catch (e) {
        if (!mismatch.includes(e.kind)) throw e;
        setState({ loading: false, ...(await second()) });
      }
    } catch (e) {
      setState({ loading: false, error: e });
    }
  }, [root, path, hint]);

  useEffect(() => { load(); }, [load]);

  const roleTooLow = state.error && state.error.status === 403 && !state.error.kind;

  return (
    <div class="page">
      <FilesHeader
        me={me}
        root={root}
        path={path}
        rootName={
          (state.dir && state.dir.root_name) || (state.file && state.file.root_name) || ''
        }
        sshHost={(state.dir && state.dir.host) || (state.file && state.file.host) || ''}
        onRefresh={load}
      />

      {state.loading ? (
        <div class="center-fill"><div class="spinner" /></div>
      ) : roleTooLow ? (
        <div class="empty-state">
          <h2>権限が足りません</h2>
          <p>
            ファイルの参照には interact 以上の権限が要ります。
            Mac の tako で、この端末の権限を上げてください。
          </p>
        </div>
      ) : state.error ? (
        <div class="center-fill">
          <p class="error-text">{state.error.message}</p>
          <button class="btn btn-primary" onClick={load}>再試行</button>
        </div>
      ) : state.kind === 'roots' ? (
        <RootList roots={state.roots} />
      ) : state.kind === 'dir' ? (
        <DirList dir={state.dir} root={root} path={path} />
      ) : (
        <FileView file={state.file} root={root} path={path} onReload={load} />
      )}
    </div>
  );
}

function FilesHeader({ me, root, path, rootName, sshHost, onRefresh }) {
  const parent = parentOf(path || '');
  const label = !root
    ? 'ファイル'
    : (path ? path.split('/').pop() : rootName || 'フォルダ');

  function goBack() {
    if (!root) { window.location.hash = '#/'; return; }
    if (parent === null) { navigate(null, null); return; }
    navigate(root, parent);
  }

  return (
    <div class="panes-header">
      <div class="panes-header-row">
        <button class="pane-header-back" onClick={goBack} aria-label="戻る">{'‹'}</button>
        <div class="machine-chip" style="flex: 1; min-width: 0; overflow: hidden;">
          <span class="chip-name" style="overflow: hidden; text-overflow: ellipsis; white-space: nowrap;">
            {label}
          </span>
        </div>
        <button class="refresh-btn" aria-label="更新" onClick={onRefresh}>
          <svg width="17" height="17" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
            <path d="M20 11a8 8 0 1 0-2.3 5.7" /><path d="M20 5v6h-6" />
          </svg>
        </button>
      </div>
      {root ? (
        <div class="file-crumb">
          {sshHost && <span class="file-badge">SSH {sshHost}</span>}
          {[rootName, path].filter(Boolean).join('/')}
        </div>
      ) : (
        <div class="file-crumb">{(me && me.host) || 'tako'} のファイルツリー</div>
      )}
    </div>
  );
}

function RootList({ roots }) {
  if (!roots.length) {
    return (
      <div class="empty-state">
        <h2>フォルダがありません</h2>
        <p>
          Mac の tako でフォルダを開くと、ここに並びます。
          ツリーに出ているフォルダの中だけが参照できます。
          SSH 先は Mac で「リモートからフォルダを開く」と並びます。
        </p>
      </div>
    );
  }
  return (
    <div class="card-list" style="padding-top: 12px;">
      {roots.map(r => (
        <button key={r.id} class="file-row" onClick={() => navigate(r.id, '', 'dir')}>
          <span class="file-row-icon dir"><FolderIcon /></span>
          <span class="file-row-main">
            <span class="file-row-name">{r.name}</span>
            <span class="file-row-meta">
              {r.tab_title || `タブ ${r.tab}`}
              {/* SSH 先は行末バッジで示す（Mac のツリーと同じ言い方。#976） */}
              {r.ssh && <SshBadge host={r.host} connected={r.connected} />}
            </span>
          </span>
          <span class="file-row-chevron">{'›'}</span>
        </button>
      ))}
    </div>
  );
}

function DirList({ dir, root, path }) {
  const [showHidden, setShowHidden] = useState(false);
  const all = dir.entries || [];
  const entries = showHidden ? all : all.filter(e => !e.hidden);
  // 表示状態と無関係に数える（`all.length - entries.length` だと表示 ON のとき 0 になり、
  // トグルのボタンごと消えて**元に戻せなくなる**）
  const hiddenCount = all.filter(e => e.hidden).length;

  return (
    <div class="card-list" style="padding-top: 12px;">
      {dir.ssh && dir.connected === false && (
        <div class="file-notice warn">
          このホストとの接続が切れています。Mac 側でつながると読み直せます。
        </div>
      )}
      {dir.truncated && (
        <div class="file-notice">
          エントリが多いため一部だけ表示しています
        </div>
      )}
      {entries.length === 0 ? (
        <div class="empty-state">
          <h2>空のフォルダ</h2>
          {hiddenCount > 0 && <p>隠しファイルが {hiddenCount} 件あります。</p>}
        </div>
      ) : entries.map(e => (
        <button
          key={e.name}
          class={`file-row${e.escapes_root ? ' disabled' : ''}`}
          disabled={e.escapes_root}
          onClick={() =>
            navigate(root, path ? `${path}/${e.name}` : e.name, e.dir ? 'dir' : 'file')
          }
        >
          <span class={`file-row-icon${e.dir ? ' dir' : ''}`}>
            {e.dir ? <FolderIcon /> : <FileIcon />}
          </span>
          <span class="file-row-main">
            <span class="file-row-name">{e.name}</span>
            <span class="file-row-meta">
              {e.escapes_root
                ? 'ツリーの外を指すリンク'
                : [
                    e.dir ? '' : formatSize(e.size),
                    formatTime(e.modified),
                    e.symlink ? 'リンク' : '',
                  ].filter(Boolean).join(' · ')}
            </span>
          </span>
          {!e.escapes_root && <span class="file-row-chevron">{'›'}</span>}
        </button>
      ))}
      {hiddenCount > 0 && (
        <button class="btn" style="width: 100%; margin-top: 10px;" onClick={() => setShowHidden(v => !v)}>
          {showHidden ? '隠しファイルを隠す' : `隠しファイルを表示（${hiddenCount}）`}
        </button>
      )}
    </div>
  );
}

const EditIcon = () => (
  <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9">
    <path d="M4 20h4l10-10-4-4L4 16z" /><path d="M14 6l4 4" />
  </svg>
);

// SSH 先ルート / 切断のバッジ（ツリーの行末バッジ #976 と同じ言い方）
function SshBadge({ host, connected }) {
  return (
    <span class={`file-badge${connected === false ? ' off' : ''}`}>
      {connected === false ? `切断 ${host}` : `SSH ${host}`}
    </span>
  );
}

function FileView({ file, root, path, onReload }) {
  const name = (path || '').split('/').pop();
  const downloadUrl = `${base()}${filesUrl('/api/files/download', root, path)}`;
  // 編集できるのは「本文が返っていて・書けて・検証子がある」ものだけ。
  // 判断の材料はすべてサーバー側が付けたもので、画面側で拡張子を見たりしない
  const canEdit = typeof file.text === 'string' && !file.read_only && !!file.etag;

  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState(file.text || '');
  // 画面に出している本文。保存できたらここを差し替える（読み直しの往復を省く）。
  // **`file` を書き換えない**のが要点: prop を触ると下の useEffect の依存が動き、
  // 保存できた直後に状態がリセットされて「保存しました」が一瞬で消える
  const [shown, setShown] = useState(file.text || '');
  const [etag, setEtag] = useState(file.etag || '');
  const [busy, setBusy] = useState(false);
  // { ok: true, remote } / { ok: false, message, kind, pending }
  const [result, setResult] = useState(null);
  const [pendingWrite, setPendingWrite] = useState(file.pending_write === true);
  const areaRef = useRef(null);

  // 別のファイルへ移った / 読み直したら下書きと結果を捨てる
  // （依存は**読み込んだ応答そのもの**。値ではなく識別で見るので、
  //   自分の保存で状態が巻き戻らない）
  useEffect(() => {
    setEditing(false);
    setDraft(file.text || '');
    setShown(file.text || '');
    setEtag(file.etag || '');
    setResult(null);
    setPendingWrite(file.pending_write === true);
  }, [root, path, file]);

  const dirty = editing && draft !== shown;

  async function save() {
    setBusy(true);
    setResult(null);
    try {
      const out = await sendJson('PUT', filesUrl('/api/files/content', root, path), {
        text: draft,
        etag,
      });
      // 応答の検証子で続けて保存できる（読み直さなくても 2 回目が通る）
      setEtag(out.etag || '');
      setPendingWrite(false);
      setResult({ ok: true, remote: out.remote });
      setEditing(false);
      setShown(draft);
    } catch (e) {
      setResult({ ok: false, message: e.message, kind: e.kind, pending: e.pending });
      if (e.pending) setPendingWrite(true);
    } finally {
      setBusy(false);
    }
  }

  async function push() {
    setBusy(true);
    setResult(null);
    try {
      // `force` は送らない（競合は読み直して直す。#1085）
      await sendJson('POST', filesUrl('/api/files/push', root, path), {});
      setPendingWrite(false);
      setResult({ ok: true, pushed: true });
    } catch (e) {
      setResult({ ok: false, message: e.message, kind: e.kind, pending: e.pending });
    } finally {
      setBusy(false);
    }
  }

  return (
    <>
      <div class="file-actions">
        <div class="file-actions-info">
          <span class="file-actions-name">{name}</span>
          <span class="file-actions-meta">
            {formatSize(file.size)}
            {file.ssh && <SshBadge host={file.host} connected={file.connected} />}
            {file.read_only && <span class="file-badge off">読み取り専用</span>}
          </span>
        </div>
        {canEdit && !editing && (
          <button class="btn file-edit-btn" onClick={() => setEditing(true)}>
            <EditIcon />
            編集
          </button>
        )}
        {/* daemon が Content-Disposition: attachment を付けるので、
            素の遷移で iOS / Android とも保存シートが開く。
            **編集モードの「保存」（PC へ書き戻す）と言い分ける**ため、
            こちらは「端末に保存」= 手元へ落とす操作だと分かる語にしてある */}
        <a class="btn file-download" href={downloadUrl}>
          <DownloadIcon />
          端末に保存
        </a>
      </div>

      {/* 前のセッションで押し出せていない保存が残っている（#966 / #1085） */}
      {pendingWrite && (
        <div class="file-notice warn">
          リモートへ送れていない保存が残っています。
          <button class="btn file-inline-btn" disabled={busy} onClick={push}>
            送り直す
          </button>
        </div>
      )}

      {result && !result.ok && (
        <div class="file-notice error">
          <span>{result.message}</span>
          {result.kind === 'conflict' && (
            <button class="btn file-inline-btn" onClick={onReload}>読み直す</button>
          )}
          {result.pending && (
            <button class="btn file-inline-btn" disabled={busy} onClick={push}>送り直す</button>
          )}
        </div>
      )}
      {result && result.ok && (
        <div class="file-notice ok">
          {result.pushed
            ? 'リモートへ送りました'
            : result.remote
              ? `保存してリモートへ書き戻しました（${result.remote.state || 'saved'}）`
              : '保存しました'}
        </div>
      )}

      {file.binary ? (
        <div class="empty-state">
          <h2>プレビューできません</h2>
          <p>テキストではないファイルです。保存してから開いてください。</p>
        </div>
      ) : file.truncated ? (
        <div class="empty-state">
          <h2>大きすぎます</h2>
          <p>
            {formatSize(PREVIEW_MAX_BYTES)} を超えるファイルはここに表示しません。
            保存してから開いてください。
          </p>
        </div>
      ) : editing ? (
        <>
          <textarea
            ref={areaRef}
            class="file-editor"
            value={draft}
            spellcheck={false}
            autocapitalize="off"
            autocorrect="off"
            autocomplete="off"
            onInput={e => setDraft(e.currentTarget.value)}
          />
          <div class="file-edit-actions">
            <button
              class="btn"
              disabled={busy}
              onClick={() => { setDraft(shown); setEditing(false); setResult(null); }}
            >
              取り消す
            </button>
            <button class="btn btn-primary" disabled={busy || !dirty} onClick={save}>
              {busy ? '保存中...' : '保存'}
            </button>
          </div>
        </>
      ) : (
        <pre class="file-preview">{shown}</pre>
      )}
    </>
  );
}
