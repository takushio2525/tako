// ファイルビュー（#1079。リモート刷新 柱 3-E / 編集は #1084 = 柱 3-F /
// SSH 先は #1085 = 柱 3-G / Finder 風の全体閲覧とショートカットは #1451）。
//
// スマホから PC のファイルを「見る → 中身を確かめる → 直す → 保存する」までを 1 画面で。
// 認可は daemon 側の純粋関数（`remote_files::resolve_in_root`）が正。
// ここは見せ方だけを持ち、パスの妥当性を画面側で判断しない
// （画面で弾いたつもりの形が API では通る、という食い違いを作らないため）。
//
// #1451 で見えるものが 3 節になった:
//   ① ショートカット（お気に入り。manage 以上）
//   ② ツリーのルート（#1079 の見え方。**そのまま残す**）
//   ③ このマシン（`/` から全部辿る。manage 以上）
// ②と③の出し分けは **daemon が返す一覧が正**（`kind` が `tree` か `fs` か）。
// 画面側で role を見て節を作らないのは、押せる範囲と見える範囲を
// 2 か所で判断すると必ずズレるため。role を見るのは
// 「ショートカット API を呼ぶか」（呼ぶと 403 になる端末で無駄な赤を出さない）だけ。
//
// API は自前 fetch で叩く: 並行して `api.js` を改修している作業（#1077 / #1078）と
// 衝突させないため、このビューが使う分はこのファイルに閉じてある。
import { useState, useEffect, useCallback } from 'preact/hooks';
import { createClient } from '../api';

const TIMEOUT_MS = 15000;

// #1451 の A/B。PWA はブラウザで動くので env が届かない（逃げ道はこの 1 つだけ）。
// legacy 腕では全体閲覧の節もショートカットも出さず、#1079 の見え方に戻る
function legacy1451() {
  try {
    return new URLSearchParams(window.location.search).get('tako_1451_legacy') === '1';
  } catch {
    return false;
  }
}

// ショートカットを編集できる role（daemon の `FILE_ROUTES` と揃える。
// **認可の正はサーバー側**で、ここは「呼んでも 403 になる端末に赤を出さない」ためだけ）
function canManage(me) {
  return me && (me.role === 'manage' || me.role === 'admin');
}

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

// ショートカットの削除（宛先は query。DELETE のボディは中継で落ちうる）
async function deleteJson(path) {
  const resp = await fetch(`${base()}${path}`, {
    method: 'DELETE',
    signal: AbortSignal.timeout(TIMEOUT_MS),
  });
  const out = await resp.json().catch(() => ({}));
  if (!resp.ok) {
    const e = new Error(out.error || `HTTP ${resp.status}`);
    e.status = resp.status;
    e.kind = out.kind;
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

const StarIcon = ({ filled }) => (
  <svg width="16" height="16" viewBox="0 0 24 24"
    fill={filled ? 'currentColor' : 'none'} stroke="currentColor" stroke-width="1.8">
    <path d="M12 3.6l2.6 5.3 5.8.8-4.2 4.1 1 5.8-5.2-2.7-5.2 2.7 1-5.8-4.2-4.1 5.8-.8z" />
  </svg>
);

const MachineIcon = () => (
  <svg width="17" height="17" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8">
    <rect x="3" y="4" width="18" height="12" rx="2" /><path d="M8 20h8" /><path d="M12 16v4" />
  </svg>
);

const TrashIcon = () => (
  <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8">
    <path d="M4 7h16" /><path d="M9 7V5h6v2" /><path d="M6 7l1 13h10l1-13" />
  </svg>
);

// --- 画面 ---

export function FilesPage({ me, root, path, hint }) {
  const [state, setState] = useState({ loading: true });
  // ショートカットを扱えるか。**派生した真偽値**を依存に使う:
  // `me` そのものを依存にすると、role が同じでも `refreshMe` のたびに
  // オブジェクトの同一性が変わって開いているフォルダを読み直してしまう。
  // 逆に依存から外すと、role が上がっても `load` が作り直されず
  // ショートカットが出ないままになる（どちらの取りこぼしも避ける）
  const canShortcuts = canManage(me) && !legacy1451();

  const load = useCallback(async () => {
    setState({ loading: true });
    try {
      if (!root) {
        const body = await getJson('/api/files');
        // ショートカットは **manage 以上**（daemon の `FILE_ROUTES`）。
        // 権限が無い端末では**呼ばない**（押してもいない操作で赤を出さない）。
        // 呼んで失敗したときも一覧そのものは出す = ショートカットの不調で
        // ファイルビューごと開けなくならない
        let shortcuts = [];
        let shortcutsError = null;
        if (canShortcuts) {
          try {
            shortcuts = (await getJson('/api/files/shortcuts')).shortcuts || [];
          } catch (e) {
            shortcutsError = e;
          }
        }
        setState({
          loading: false,
          kind: 'roots',
          roots: body.roots || [],
          shortcuts,
          shortcutsError,
        });
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
  }, [root, path, hint, canShortcuts]);

  useEffect(() => { load(); }, [load]);

  // ショートカットの追加 / 削除。**結果は必ず読み直す**（画面側で一覧を
  // 組み直すと、daemon が冪等に畳んだ結果とズレる）
  const addShortcut = useCallback(async (abs, name) => {
    await sendJson('POST', '/api/files/shortcuts', { path: abs, name });
  }, []);
  const removeShortcut = useCallback(async (id) => {
    await deleteJson(`/api/files/shortcuts?id=${encodeURIComponent(id)}`);
    load();
  }, [load]);

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
      ) : state.error && state.error.kind === 'unreadable' ? (
        /* #1451: 全体閲覧では `/private/var/db` のような読めないフォルダへ
           普通に入れてしまう。「エラー」の 1 行で終わらせず、**理由と戻り道**を出す */
        <div class="empty-state" data-testid="dir-unreadable">
          <h2>このフォルダは開けません</h2>
          <p>{state.error.message}</p>
          <p>
            macOS が守っているフォルダ（システム領域・他のユーザーの領域）は、
            tako からも読めません。
          </p>
          <button class="btn" onClick={() => navigate(root, parentOf(path || '') ?? '')}>
            上のフォルダへ戻る
          </button>
        </div>
      ) : state.error ? (
        <div class="center-fill">
          <p class="error-text">{state.error.message}</p>
          <button class="btn btn-primary" onClick={load}>再試行</button>
        </div>
      ) : state.kind === 'roots' ? (
        <RootList
          roots={state.roots}
          shortcuts={state.shortcuts || []}
          shortcutsError={state.shortcutsError}
          onRemoveShortcut={removeShortcut}
        />
      ) : state.kind === 'dir' ? (
        <DirList
          dir={state.dir}
          root={root}
          path={path}
          canShortcut={canShortcuts}
          onAddShortcut={addShortcut}
        />
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
          {/* #1451: 全体閲覧のルートは名前が区切り（`/` や `C:\`）そのものなので、
              ツリー由来と同じ規則で継ぐと `//Users/...` になる。
              **区切りで終わるルート名は継ぎ足さない** */}
          {rootName.endsWith('/') || rootName.endsWith('\\')
            ? `${rootName}${path}`
            : [rootName, path].filter(Boolean).join('/')}
        </div>
      ) : (
        <div class="file-crumb">{(me && me.host) || 'tako'} のファイルツリー</div>
      )}
    </div>
  );
}

// ルート一覧は 3 節（#1451）。**節の作り方は daemon の答えが正**:
// `kind === 'fs'` なら「このマシン」、それ以外は従来どおり「ツリーのルート」。
// ショートカットだけは別 API（manage 以上）なので、取れたときだけ先頭へ出る
function RootList({ roots, shortcuts, shortcutsError, onRemoveShortcut }) {
  const treeRoots = roots.filter(r => r.kind !== 'fs');
  const fsRoots = roots.filter(r => r.kind === 'fs');

  if (!roots.length && !shortcuts.length) {
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
      {shortcutsError && (
        <div class="file-notice warn">
          ショートカットを読めませんでした（{shortcutsError.message}）
        </div>
      )}

      {shortcuts.length > 0 && (
        <>
          <div class="file-section" data-testid="files-section-shortcuts">ショートカット</div>
          {shortcuts.map(s => (
            <ShortcutRow key={s.id} shortcut={s} onRemove={onRemoveShortcut} />
          ))}
        </>
      )}

      {treeRoots.length > 0 && (
        <>
          <div class="file-section" data-testid="files-section-tree">tako のツリー</div>
          {treeRoots.map(r => (
            <button
              key={r.id}
              class="file-row"
              data-testid={`tree-root-${r.id}`}
              onClick={() => navigate(r.id, '', 'dir')}
            >
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
        </>
      )}

      {fsRoots.length > 0 && (
        <>
          <div class="file-section" data-testid="files-section-fs">このマシン</div>
          {fsRoots.map(r => (
            <button
              key={r.id}
              class="file-row"
              data-testid={`fs-root-${r.id}`}
              onClick={() => navigate(r.id, '', 'dir')}
            >
              <span class="file-row-icon dir"><MachineIcon /></span>
              <span class="file-row-main">
                <span class="file-row-name">{r.name}</span>
                <span class="file-row-meta">ルートから全部たどる</span>
              </span>
              <span class="file-row-chevron">{'›'}</span>
            </button>
          ))}
        </>
      )}
    </div>
  );
}

// ショートカット 1 行。既定（`builtin`）は削除できない = ボタンを出さない。
// `available: false` は「今このマシンでは開けない」（daemon が飛び先を解決できなかった）
function ShortcutRow({ shortcut, onRemove }) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState(null);

  async function remove() {
    setBusy(true);
    setError(null);
    try {
      await onRemove(shortcut.id);
    } catch (err) {
      setError(err.message);
      setBusy(false);
    }
  }

  return (
    <div class="file-row-wrap">
      {/* 削除は**行の中に入れない**（`<button>` の入れ子はタップが親に吸われて
          「削除のつもりが遷移」になる端末がある）。兄弟として並べ、
          行と削除で押した先が別物であることを DOM の構造で保証する */}
      <div class="file-row-pair">
        <button
          data-testid={`shortcut-${shortcut.id}`}
          class={`file-row${shortcut.available === false ? ' disabled' : ''}`}
          disabled={shortcut.available === false}
          onClick={() => navigate(shortcut.root, shortcut.path || '', 'dir')}
        >
          <span class="file-row-icon dir"><StarIcon filled /></span>
          <span class="file-row-main">
            <span class="file-row-name">{shortcut.name}</span>
            <span class="file-row-meta">
              {shortcut.available === false
                ? 'このマシンでは開けません'
                : shortcut.display_path}
            </span>
          </span>
          {shortcut.available !== false && <span class="file-row-chevron">{'›'}</span>}
        </button>
        {!shortcut.builtin && (
          <button
            class="file-row-remove"
            data-testid={`shortcut-remove-${shortcut.id}`}
            aria-label={`${shortcut.name} をショートカットから外す`}
            disabled={busy}
            onClick={remove}
          >
            <TrashIcon />
          </button>
        )}
      </div>
      {error && <div class="file-notice error">{error}</div>}
    </div>
  );
}

// 「このフォルダをショートカットに追加」。**冪等**（daemon が同じパスを畳む）なので
// 二重に押しても増えない。押した結果は言葉で返す（無言で終わらせない）
function AddShortcutRow({ abs, name, onAdd }) {
  const [state, setState] = useState(null); // null | 'busy' | 'done' | { error }

  async function add() {
    setState('busy');
    try {
      await onAdd(abs, name);
      setState('done');
    } catch (e) {
      setState({ error: e.message });
    }
  }

  if (state === 'done') {
    return <div class="file-notice ok">ショートカットに追加しました</div>;
  }
  return (
    <>
      <button
        class="btn file-shortcut-add"
        data-testid="shortcut-add"
        disabled={state === 'busy'}
        onClick={add}
      >
        <StarIcon />
        {state === 'busy' ? '追加中...' : 'ショートカットに追加'}
      </button>
      {state && state.error && <div class="file-notice error">{state.error}</div>}
    </>
  );
}

function DirList({ dir, root, path, canShortcut, onAddShortcut }) {
  const [showHidden, setShowHidden] = useState(false);
  const all = dir.entries || [];
  const entries = showHidden ? all : all.filter(e => !e.hidden);
  // 表示状態と無関係に数える（`all.length - entries.length` だと表示 ON のとき 0 になり、
  // トグルのボタンごと消えて**元に戻せなくなる**）
  const hiddenCount = all.filter(e => e.hidden).length;

  return (
    <div class="card-list" style="padding-top: 12px;">
      {/* #1451: ショートカットに足せるのは**絶対パスが分かるとき**だけ。
          daemon は全体閲覧（`kind: fs`）のときにしか `abs` を返さないので、
          ツリー由来のルートでは出ない（#1079 の「絶対パスを配らない」を保つ） */}
      {canShortcut && dir.abs && (
        <AddShortcutRow abs={dir.abs} name={path ? path.split('/').pop() : dir.root_name} onAdd={onAddShortcut} />
      )}
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
          data-testid={`entry-${e.name}`}
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
                : e.unreadable
                  /* #1451: metadata が引けない = 権限が無い / リンクが切れている。
                     0 バイトの行として黙って混ぜない（無言禁止。#1399 系） */
                  ? (e.symlink ? 'リンク先を読めません' : '読み取り権限がありません')
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
      // 送れたのは**退避してあった内容**なので、画面は真実へ揃え直す。
      // ただし手元に未保存の編集が残っているときは捨てない（保存で送り直せる）
      if (!dirty) onReload();
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
              ? 'リモートへ書き戻しました'
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
