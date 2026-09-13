// Issue #1451: ファイル閲覧を Finder 風の全体閲覧へ広げ、ショートカットを足せるようにする。
//
// 受け入れ条件を、モバイル viewport の実 DOM で確かめる:
//   ① `/` からディレクトリを辿ってファイルをプレビューできる（manage 端末）
//   ② interact 端末では**全体閲覧の節が出ない**（#1079 のツリー節はそのまま）
//   ③ 読めないディレクトリ / 読めない 1 件は**理由が出る**（無言禁止）
//   ④ ショートカットの追加 / 削除が画面からでき、既定は消せない
//   ⑤ 叩く API が `remote_files::FILE_ROUTES` の宣言どおり（表外を呼ばない）
//
// **実ファイルシステムを一切見ない**のがこの spec の要点（#927）。
// 一覧はすべてモックで、出てくるパスは `/Users/testuser/…` の偽の木だけ。
// 実 `/` の一覧・実ホームパス・実ユーザー名はスクショにも入らない。
//
// A/B の対照は `?tako_1451_legacy=1`（PWA はブラウザで動くので env が届かない）。
// legacy 腕では全体閲覧の節もショートカットも出ず、#1079 の見え方に戻る。
//
// 実行:
//   cd web/tako-remote && npx playwright test e2e/files-1451.spec.js
import { test, expect } from '@playwright/test';

const IPHONE_VIEWPORT = { width: 390, height: 844 };
const BASE = `http://localhost:${process.env.TAKO_PWA_PORT || 5174}`;
const EVIDENCE_DIR = process.env.TAKO_EVIDENCE_DIR || `${process.env.HOME}/dev/tako-evidence/1451`;

// 偽のホーム（#927: 実ユーザー名・実パスを書かない）
const HOME = '/Users/testuser';

function me(role = 'manage') {
  return {
    registered: true, device_id: 'test-iphone', name: 'iPhone', role,
    login: 'user@example.com', host: 'test-mac', version: '0.8.12', app_connected: true,
  };
}

// ツリーのルート（#1079 の見え方。**全 role で出る**）
const TREE_ROOT = {
  id: 'aaaaaaaaaaaa', name: 'tako', tab: 3, tab_title: 'tako', ssh: false, kind: 'tree',
};
// 全体閲覧の入口（manage 以上でしか daemon が返さない）
const FS_ROOT = { id: 'fs', name: '/', tab: 0, tab_title: '', ssh: false, kind: 'fs' };

const SHORTCUTS = [
  {
    id: 'b0b0b0b0b0b0', path: HOME, display_path: '~', name: 'ホーム',
    added_at: 0, builtin: true, root: 'fs', path_rel: 'Users/testuser', available: true,
  },
  {
    id: 'c1c1c1c1c1c1', path: `${HOME}/dev/tako`, display_path: '~/dev/tako', name: 'tako',
    added_at: 1757000000, builtin: false, root: 'fs', available: true,
  },
];

// `/` 直下（偽）
const ROOT_DIR = {
  root: 'fs', root_name: '/', root_kind: 'fs', path: '', abs: '/', truncated: false,
  entries: [
    { name: 'Users', dir: true, size: null, modified: 1757000000, symlink: false, escapes_root: false, hidden: false, unreadable: false },
    { name: 'private', dir: true, size: null, modified: 1757000000, symlink: false, escapes_root: false, hidden: false, unreadable: false },
    { name: 'etc', dir: true, size: null, modified: 1756000000, symlink: true, escapes_root: false, hidden: false, unreadable: false },
    { name: '.VolumeIcon.icns', dir: false, size: 12345, modified: 1750000000, symlink: false, escapes_root: false, hidden: true, unreadable: false },
  ],
};

// `/Users/testuser`（読めない 1 件を混ぜる）
const HOME_DIR = {
  root: 'fs', root_name: '/', root_kind: 'fs', path: 'Users/testuser', abs: HOME, truncated: false,
  entries: [
    { name: 'dev', dir: true, size: null, modified: 1757000000, symlink: false, escapes_root: false, hidden: false, unreadable: false },
    { name: 'Library', dir: true, size: null, modified: 1757000000, symlink: false, escapes_root: false, hidden: false, unreadable: true },
    { name: 'notes.md', dir: false, size: 2048, modified: 1757100000, symlink: false, escapes_root: false, hidden: false, unreadable: false },
    { name: 'broken-link', dir: false, size: null, modified: null, symlink: true, escapes_root: false, hidden: false, unreadable: true },
  ],
};

const NOTES = {
  root: 'fs', root_name: '/', root_kind: 'fs', path: 'Users/testuser/notes.md',
  size: 2048, binary: false, truncated: false,
  text: '# メモ\n\n全体閲覧から開いたファイル。\n', etag: '2048-0123456789abcdef', ssh: false,
};

// 読めないフォルダ（daemon は 500 + kind: unreadable を返す）
const UNREADABLE = {
  error: '読み取れませんでした（権限を確認してください）',
  error_en: 'Could not read it (check permissions)',
  kind: 'unreadable',
};

function json(route, body, status = 200) {
  return route.fulfill({ status, contentType: 'application/json', body: JSON.stringify(body) });
}

function qs(url) {
  return new URL(url).searchParams;
}

/**
 * API をすべてモックする。`calls` に「PWA が何をどの順で叩いたか」が溜まる。
 *
 * `opts.role` で端末の役割、`opts.shortcutsStatus` でショートカット API の
 * 権限不足（403）を再現できる
 */
async function setupMocks(page, opts = {}) {
  const { role = 'manage', shortcutsStatus = 200 } = opts;
  const calls = { files: [], content: [], shortcuts: [], added: [], removed: [], other: [] };
  // 画面から足したものを覚えておく（追加 → 一覧で出る、までを 1 本で確かめる）
  let shortcuts = SHORTCUTS.map(s => ({ ...s }));

  await page.route('**/api/me', route => json(route, me(role)));
  await page.route('**/api/v2/panes', route => json(route, { api_version: 2, panes: [] }));
  await page.route('**/api/agents', route => json(route, { agents: [] }));
  await page.route('**/api/health', route => json(route, { status: 'ok', version: '0.8.12' }));
  await page.route('**/ws?*', route => route.abort());
  await page.route('**/manifest.json', route => json(route, { name: 'tako remote' }));
  await page.route('**/sw.js', route =>
    route.fulfill({ status: 200, contentType: 'application/javascript', body: '' })
  );

  await page.route('**/api/files/shortcuts*', route => {
    const method = route.request().method();
    if (shortcutsStatus !== 200) {
      calls.shortcuts.push({ method, status: shortcutsStatus });
      return json(route, { error: `権限が足りません（${role} → manage 以上が必要）` }, shortcutsStatus);
    }
    if (method === 'POST') {
      const body = route.request().postDataJSON() || {};
      calls.added.push(body);
      const entry = {
        id: 'd2d2d2d2d2d2', path: body.path, display_path: body.path.replace(HOME, '~'),
        name: body.name || body.path.split('/').pop(), added_at: 1757200000,
        builtin: false, root: 'fs', available: true,
      };
      // **冪等**: 同じパスなら増やさない（daemon 側の正本と同じ振る舞いを模す）
      if (!shortcuts.some(s => s.path === body.path)) shortcuts.push(entry);
      return json(route, { shortcut: entry });
    }
    if (method === 'DELETE') {
      const id = qs(route.request().url()).get('id');
      calls.removed.push(id);
      shortcuts = shortcuts.filter(s => s.id !== id);
      return json(route, { removed: id });
    }
    calls.shortcuts.push({ method, status: 200 });
    return json(route, { shortcuts });
  });

  await page.route('**/api/files/content*', route => {
    const p = qs(route.request().url()).get('path');
    calls.content.push(p);
    if (p === 'Users/testuser/notes.md') return json(route, NOTES);
    return json(route, { error: 'フォルダではありません', kind: 'not_a_file' }, 400);
  });

  await page.route('**/api/files*', route => {
    const url = route.request().url();
    // `/api/files/...` の別経路はここへ来ない（上で先に捕まえている）
    const root = qs(url).get('root');
    const p = qs(url).get('path') || '';
    calls.files.push({ root, path: p });
    if (!root) {
      const roots = role === 'manage' || role === 'admin' ? [TREE_ROOT, FS_ROOT] : [TREE_ROOT];
      return json(route, { roots });
    }
    if (root === 'fs' && p === '') return json(route, ROOT_DIR);
    if (root === 'fs' && p === 'Users') {
      return json(route, {
        root: 'fs', root_name: '/', root_kind: 'fs', path: 'Users', abs: '/Users', truncated: false,
        entries: [{ name: 'testuser', dir: true, size: null, modified: 1757000000, symlink: false, escapes_root: false, hidden: false, unreadable: false }],
      });
    }
    if (root === 'fs' && p === 'Users/testuser') return json(route, HOME_DIR);
    if (root === 'fs' && p === 'private') return json(route, UNREADABLE, 500);
    if (root === 'fs' && p === 'Users/testuser/notes.md') {
      return json(route, { error: 'フォルダではありません', kind: 'not_a_directory' }, 400);
    }
    if (root === TREE_ROOT.id) {
      return json(route, {
        root: TREE_ROOT.id, root_name: 'tako', root_kind: 'tree', path: p, abs: null, truncated: false,
        entries: [{ name: 'README.md', dir: false, size: 100, modified: 1757000000, symlink: false, escapes_root: false, hidden: false, unreadable: false }],
      });
    }
    return json(route, { error: 'このフォルダは tako のファイルツリーに出ていません', kind: 'unknown_root' }, 403);
  });

  return calls;
}

async function openFiles(page, query = '') {
  await page.goto(`${BASE}/${query}#/files`);
  await page.waitForSelector('.card-list, .empty-state', { timeout: 10000 });
}

test.describe('#1451 Finder 風の全体閲覧とショートカット — モバイル', () => {
  test.use({ viewport: IPHONE_VIEWPORT });

  test('01. manage は 3 節（ショートカット / ツリー / このマシン）', async ({ page }) => {
    await setupMocks(page);
    await openFiles(page);

    await expect(page.locator('[data-testid="files-section-shortcuts"]')).toBeVisible();
    await expect(page.locator('[data-testid="files-section-tree"]')).toBeVisible();
    await expect(page.locator('[data-testid="files-section-fs"]')).toBeVisible();
    // #1079 のツリーのルートはそのまま並ぶ
    await expect(page.locator(`[data-testid="tree-root-${TREE_ROOT.id}"]`)).toContainText('tako');
    // 既定ショートカット（ホーム）は `~` で出る = 実ホームパスを画面に出さない
    await expect(page.locator('[data-testid="shortcut-b0b0b0b0b0b0"]')).toContainText('~');
    await page.screenshot({ path: `${EVIDENCE_DIR}/01-roots.png` });
  });

  test('02. `/` から辿ってファイルをプレビューできる', async ({ page }) => {
    const calls = await setupMocks(page);
    await openFiles(page);

    await page.locator('[data-testid="fs-root-fs"]').click();
    await expect(page.locator('[data-testid="entry-Users"]')).toBeVisible();
    // パンくずは絶対パス（ルート名が区切りなので二重の `/` にならない）
    await expect(page.locator('.file-crumb')).toHaveText('/');
    await page.screenshot({ path: `${EVIDENCE_DIR}/02-root-dir.png` });

    await page.locator('[data-testid="entry-Users"]').click();
    await expect(page.locator('[data-testid="entry-testuser"]')).toBeVisible();
    await expect(page.locator('.file-crumb')).toHaveText('/Users');

    await page.locator('[data-testid="entry-testuser"]').click();
    await expect(page.locator('[data-testid="entry-notes.md"]')).toBeVisible();
    await expect(page.locator('.file-crumb')).toHaveText('/Users/testuser');

    await page.locator('[data-testid="entry-notes.md"]').click();
    await expect(page.locator('.file-preview')).toContainText('全体閲覧から開いたファイル');
    await expect.poll(() => calls.content).toContain('Users/testuser/notes.md');
    await page.screenshot({ path: `${EVIDENCE_DIR}/03-preview.png` });
  });

  test('03. 読めないフォルダは理由と戻り道が出る', async ({ page }) => {
    await setupMocks(page);
    await openFiles(page);
    await page.locator('[data-testid="fs-root-fs"]').click();
    await page.locator('[data-testid="entry-private"]').click();

    const notice = page.locator('[data-testid="dir-unreadable"]');
    await expect(notice).toBeVisible();
    await expect(notice).toContainText('読み取れませんでした');
    await expect(notice.locator('button')).toContainText('上のフォルダへ戻る');
    await page.screenshot({ path: `${EVIDENCE_DIR}/04-unreadable-dir.png` });

    // 戻り道が実際に効く
    await notice.locator('button').click();
    await expect(page.locator('[data-testid="entry-Users"]')).toBeVisible();
  });

  test('04. 読めない 1 件は行に理由が出る（0 バイトで無言にしない）', async ({ page }) => {
    await setupMocks(page);
    await openFiles(page);
    await page.locator('[data-testid="fs-root-fs"]').click();
    await page.locator('[data-testid="entry-Users"]').click();
    await page.locator('[data-testid="entry-testuser"]').click();

    await expect(page.locator('[data-testid="entry-Library"]')).toContainText('読み取り権限がありません');
    await expect(page.locator('[data-testid="entry-broken-link"]')).toContainText('リンク先を読めません');
    // 読める行は従来どおりサイズ / 更新日時
    await expect(page.locator('[data-testid="entry-notes.md"]')).toContainText('2.0 KB');
    await page.screenshot({ path: `${EVIDENCE_DIR}/05-unreadable-entry.png` });
  });

  test('05. ショートカットを足して消せる（既定は消せない）', async ({ page }) => {
    const calls = await setupMocks(page);
    await openFiles(page);
    await page.locator('[data-testid="fs-root-fs"]').click();
    await page.locator('[data-testid="entry-Users"]').click();
    await expect(page.locator('[data-testid="entry-testuser"]')).toBeVisible();

    // 全体閲覧の配下なので「ショートカットに追加」が出る
    const add = page.locator('[data-testid="shortcut-add"]');
    await expect(add).toBeVisible();
    await add.click();
    await expect(page.locator('.file-notice.ok')).toContainText('ショートカットに追加しました');
    await expect.poll(() => calls.added.map(a => a.path)).toContain('/Users');
    await page.screenshot({ path: `${EVIDENCE_DIR}/06-shortcut-added.png` });

    // 一覧へ戻ると増えている
    await openFiles(page);
    await expect(page.locator('[data-testid="shortcut-d2d2d2d2d2d2"]')).toBeVisible();

    // 既定（ホーム）には削除ボタンが出ない
    await expect(page.locator('[data-testid="shortcut-remove-b0b0b0b0b0b0"]')).toHaveCount(0);
    // 登録分は消せる
    await expect(page.locator('[data-testid="shortcut-remove-c1c1c1c1c1c1"]')).toBeVisible();
    await page.locator('[data-testid="shortcut-remove-c1c1c1c1c1c1"]').click();
    await expect(page.locator('[data-testid="shortcut-c1c1c1c1c1c1"]')).toHaveCount(0);
    expect(calls.removed).toContain('c1c1c1c1c1c1');
    // 削除は行の遷移を起こさない（`#/files` のまま）
    expect(new URL(page.url()).hash).toBe('#/files');
  });

  test('06. ツリーのルート配下では「ショートカットに追加」を出さない', async ({ page }) => {
    await setupMocks(page);
    await openFiles(page);
    await page.locator(`[data-testid="tree-root-${TREE_ROOT.id}"]`).click();
    await expect(page.locator('[data-testid="entry-README.md"]')).toBeVisible();
    // #1079 は絶対パスを返さない = 足す宛先が無いのでボタンも出さない
    await expect(page.locator('[data-testid="shortcut-add"]')).toHaveCount(0);
  });

  test('07. interact は #1079 のまま（全体閲覧もショートカットも出ない）', async ({ page }) => {
    const calls = await setupMocks(page, { role: 'interact' });
    await openFiles(page);

    await expect(page.locator('[data-testid="files-section-fs"]')).toHaveCount(0);
    await expect(page.locator('[data-testid="files-section-shortcuts"]')).toHaveCount(0);
    // ツリーのルートはこれまでどおり見える
    await expect(page.locator(`[data-testid="tree-root-${TREE_ROOT.id}"]`)).toBeVisible();
    // **呼びもしない**（押してもいない操作で 403 の赤を出さない）
    expect(calls.shortcuts).toHaveLength(0);
    expect(calls.added).toHaveLength(0);
    await page.screenshot({ path: `${EVIDENCE_DIR}/07-interact.png` });
  });

  test('08. observe は従来どおり権限不足の案内', async ({ page }) => {
    await setupMocks(page, { role: 'observe' });
    // role が足りないときの 403（kind なし）を再現する
    await page.route('**/api/files*', route =>
      json(route, { error: 'この端末には権限がありません' }, 403)
    );
    await openFiles(page);
    await expect(page.locator('.empty-state h2')).toContainText('権限が足りません');
  });

  test('09. legacy 腕（?tako_1451_legacy=1）は #1079 の見え方に戻る', async ({ page }) => {
    const calls = await setupMocks(page);
    await openFiles(page, '?tako_1451_legacy=1');

    // daemon は manage なので `fs` を返すが、PWA 側の A/B でも節は組まれる。
    // **ショートカットは呼ばない**のが legacy 腕の観測点
    expect(calls.shortcuts).toHaveLength(0);
    await expect(page.locator('[data-testid="files-section-shortcuts"]')).toHaveCount(0);
    await page.screenshot({ path: `${EVIDENCE_DIR}/08-legacy.png` });
  });

  test('10. ショートカット API が落ちてもファイルビューは開く', async ({ page }) => {
    await setupMocks(page, { shortcutsStatus: 403 });
    await openFiles(page);

    await expect(page.locator('.file-notice.warn')).toContainText('ショートカットを読めませんでした');
    // 一覧そのものは出る（ショートカットの不調でビューごと開けなくならない）
    await expect(page.locator('[data-testid="files-section-tree"]')).toBeVisible();
    await expect(page.locator('[data-testid="files-section-fs"]')).toBeVisible();
  });

  test('11. 叩く API は宣言どおり（表外を呼ばない）', async ({ page }) => {
    const calls = await setupMocks(page);
    await openFiles(page);
    await page.locator('[data-testid="fs-root-fs"]').click();
    await page.locator('[data-testid="entry-Users"]').click();

    // `/api/files`（一覧）と `/api/files/shortcuts` 以外は呼んでいない
    expect(calls.other).toHaveLength(0);
    // 一覧は root 省略 → fs → Users の順（余計な往復をしない）
    await expect
      .poll(() => calls.files.map(c => `${c.root || '-'}:${c.path}`))
      .toEqual(['-:', 'fs:', 'fs:Users']);
  });
});
