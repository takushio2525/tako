// Issue #1472 B: スマホ（PWA `#/tasks`）で添付をその場で見る・鳴らす。
//
// 受け入れ条件を、モバイル viewport の実 DOM とネットワークで確かめる:
//   ① 画像添付が `<img src=".../api/files/download?…">` で**実際に描画される**
//      （`naturalWidth` を見る = 「要素が在る」ではなく「絵が来た」まで）
//   ② タップで拡大 → 閉じられる
//   ③ 動画添付が `<video controls preload="metadata">` で出て、src は同じ download 経路。
//      本物の webm では **metadata が実際に読める**（`readyState` / `duration`）
//   ④ 消えた添付（`exists=false`）・解決できない添付はプレビューを出さず従来の表示
//   ⑤ **一覧を開いただけでは添付の GET が 1 本も飛ばない**（詳細を開いたときだけ）
//   ⑥ observe 端末は取りに行けないのでプレビューも出ない（押せない絵を出さない）
//   ⑦ エッジ: 添付 0 件 / 画像と動画の混在 / 空白・日本語のファイル名 / 巨大画像
//
// **実ファイルシステムも実タスクも一切見ない**（#927）。出てくるパスは
// `/Users/testuser/…` の偽物だけで、実ユーザー名・実ホームパスはスクショにも入らない。
// 添付の中身は `e2e/fixtures/`（320x180 の PNG と 1 秒の webm）を配る。
//
// 実行:
//   cd web/tako-remote && npx playwright test e2e/tasks-preview-1472.spec.js
import { test, expect } from '@playwright/test';
import { evidencePath, TAKO_VERSION } from './support.js';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';

const HERE = dirname(fileURLToPath(import.meta.url));
const IPHONE_VIEWPORT = { width: 390, height: 844 };
const BASE = `http://localhost:${process.env.TAKO_PWA_PORT || 5174}`;

const THUMB_PNG = readFileSync(join(HERE, 'fixtures/thumb.png'));
const CLIP_WEBM = readFileSync(join(HERE, 'fixtures/clip.webm'));

// 偽のホーム（#927）
const HOME = '/Users/testuser';
const NOW = Math.floor(Date.now() / 1000);

function me(role = 'manage') {
  return {
    registered: true, device_id: 'test-iphone', name: 'iPhone', role,
    login: 'user@example.com', host: 'test-mac', version: TAKO_VERSION, app_connected: true,
  };
}

/** daemon が `decorate_attachment` で返す形（`preview` は `open_plan` の 1 実装が決める） */
function attachment(over = {}) {
  return {
    path: `${HOME}/out/thumb.png`, exists: true, name: 'thumb.png', size: 986,
    preview: 'image', root: 'fs', path_rel: 'Users/testuser/out/thumb.png', available: true,
    ...over,
  };
}

function task(over = {}) {
  return {
    id: 'u-1', title: '解説動画 v6 を YouTube へ投稿', body: '試聴して OK なら投稿する。',
    kind: 'post', status: 'open', created_by: 'master:takodev', project: 'tako',
    attachments: [], copy_texts: [], links: [], due: null,
    created_at: NOW - 7200, updated_at: NOW - 600,
    origin: { profile: 'takodev', session_id: 'sess-1', pane: 7, project: 'tako' },
    responses: [], delivery: null,
    ...over,
  };
}

function json(route, body, status = 200) {
  return route.fulfill({ status, contentType: 'application/json', body: JSON.stringify(body) });
}

function listBody(tasks) {
  const open = tasks.filter(t => t.status === 'open');
  const byKind = {};
  for (const t of open) byKind[t.kind] = (byKind[t.kind] || 0) + 1;
  return {
    tasks, count: tasks.length, open_count: open.length,
    open_counts_by_kind: Object.entries(byKind).map(([kind, count]) => ({ kind, count })),
  };
}

/**
 * API をすべてモックする。`calls.download` に「添付を何回・どの綴りで取りに行ったか」が溜まる
 * （受け入れ条件 ⑤ = 一覧では 0 本、を数えるため）。
 * 中身は拡張子で出し分け、**本物の PNG / webm** を配る（描画・metadata を実測する）
 */
async function setupMocks(page, tasks, opts = {}) {
  const { role = 'manage' } = opts;
  const calls = { download: [] };

  await page.route('**/api/me', route => json(route, me(role)));
  await page.route('**/api/v2/panes', route => json(route, { api_version: 2, panes: [] }));
  await page.route('**/api/agents', route => json(route, { agents: [] }));
  await page.route('**/api/health', route => json(route, { status: 'ok', version: TAKO_VERSION }));
  await page.route('**/ws?*', route => route.abort());
  await page.route('**/manifest.json', route => json(route, { name: 'tako remote' }));
  await page.route('**/sw.js', route =>
    route.fulfill({ status: 200, contentType: 'application/javascript', body: '' })
  );

  await page.route('**/api/files/download*', route => {
    const url = new URL(route.request().url());
    const rel = url.searchParams.get('path') || '';
    calls.download.push({
      root: url.searchParams.get('root'),
      path: rel,
      disposition: url.searchParams.get('disposition'),
      range: route.request().headers()['range'] || null,
    });
    // daemon の `?disposition=inline` と同じ出し分け（型を名乗り、inline で返す）
    const inline = url.searchParams.get('disposition') === 'inline';
    const webm = rel.endsWith('.webm');
    const body = webm ? CLIP_WEBM : THUMB_PNG;
    const type = webm ? 'video/webm' : 'image/png';
    return route.fulfill({
      status: 200,
      headers: {
        'Content-Type': inline ? type : 'application/octet-stream',
        'Content-Disposition': `${inline ? 'inline' : 'attachment'}; filename="x"`,
        'Accept-Ranges': 'bytes',
      },
      body,
    });
  });

  await page.route('**/api/tasks**', route => {
    const url = new URL(route.request().url());
    if (route.request().method() === 'GET') return json(route, listBody(tasks));
    return json(route, { error: 'このテストは読み出しだけを見る' }, 405);
  });

  return calls;
}

async function openDetail(page, id = 'u-1') {
  await page.goto(`${BASE}/#/tasks?id=${id}`);
  await page.waitForSelector('[data-testid="task-detail"]', { timeout: 10000 });
}

test.describe('#1472 B スマホで添付をその場で見る — モバイル', () => {
  test.use({ viewport: IPHONE_VIEWPORT });

  test('01. 画像添付がインラインで描画され、src が既存の download 経路を指す', async ({ page }) => {
    const calls = await setupMocks(page, [task({ attachments: [attachment()] })]);
    await openDetail(page);

    const img = page.locator('[data-testid="task-preview-image"]');
    await expect(img).toHaveCount(1);
    const src = await img.getAttribute('src');
    // **既存の 1 本**（新しい経路が生えていない）+ その場表示の指定
    expect(src).toContain('/api/files/download?');
    expect(src).toContain('root=fs');
    expect(src).toContain('path=Users%2Ftestuser%2Fout%2Fthumb.png');
    expect(src).toContain('disposition=inline');

    // 「要素が在る」ではなく「絵が来た」まで見る
    await expect
      .poll(() => img.evaluate(el => el.naturalWidth), { timeout: 5000 })
      .toBe(320);
    expect(await img.evaluate(el => el.naturalHeight)).toBe(180);

    // 保存ボタンは残っている（ダウンロードは disposition を付けない = 保存シートが開く）
    const href = await page.locator('[data-testid="task-download"]').first().getAttribute('href');
    expect(href).toContain('/api/files/download?');
    expect(href).not.toContain('disposition=inline');

    expect(calls.download.filter(c => c.disposition === 'inline')).toHaveLength(1);
    await page.screenshot({ path: evidencePath('01-image-inline.png'), fullPage: true });
  });

  test('02. 画像をタップで拡大 → 閉じられる', async ({ page }) => {
    await setupMocks(page, [task({ attachments: [attachment()] })]);
    await openDetail(page);

    await expect(page.locator('[data-testid="task-image-zoom"]')).toHaveCount(0);
    await page.locator('[data-testid="task-preview-image-open"]').click();
    const zoom = page.locator('[data-testid="task-image-zoom"]');
    await expect(zoom).toBeVisible();
    // 拡大側も同じ経路（別の URL を組んでいない）
    const zoomSrc = await zoom.locator('img').getAttribute('src');
    expect(zoomSrc).toContain('/api/files/download?');
    expect(zoomSrc).toContain('disposition=inline');
    await page.screenshot({ path: evidencePath('02-image-zoom.png') });

    await page.locator('[data-testid="task-image-zoom-close"]').click();
    await expect(zoom).toHaveCount(0);

    // 背景タップでも閉じる（スマホの自然な操作）
    await page.locator('[data-testid="task-preview-image-open"]').click();
    await expect(page.locator('[data-testid="task-image-zoom"]')).toBeVisible();
    await page.locator('[data-testid="task-image-zoom"]').click({ position: { x: 5, y: 400 } });
    await expect(page.locator('[data-testid="task-image-zoom"]')).toHaveCount(0);
  });

  test('03. 動画添付が controls + preload=metadata で出て、metadata が実際に読める', async ({ page }) => {
    const att = attachment({
      path: `${HOME}/out/clip.webm`, name: 'clip.webm', size: 8511,
      preview: 'video', path_rel: 'Users/testuser/out/clip.webm',
    });
    const calls = await setupMocks(page, [task({ attachments: [att] })]);
    await openDetail(page);

    const video = page.locator('[data-testid="task-preview-video"]');
    await expect(video).toHaveCount(1);
    // 一覧を重くしない指定（本体は Range で必要なところだけ流れてくる）
    await expect(video).toHaveAttribute('preload', 'metadata');
    expect(await video.evaluate(el => el.controls)).toBe(true);
    const src = await video.getAttribute('src');
    expect(src).toContain('/api/files/download?');
    expect(src).toContain('path=Users%2Ftestuser%2Fout%2Fclip.webm');
    expect(src).toContain('disposition=inline');

    // **鳴らせる状態まで来ている**（HAVE_METADATA 以上 + 尺が読めている）
    await expect
      .poll(() => video.evaluate(el => el.readyState), { timeout: 10000 })
      .toBeGreaterThanOrEqual(1);
    const duration = await video.evaluate(el => el.duration);
    expect(duration).toBeGreaterThan(0.5);
    expect(await video.evaluate(el => el.videoWidth)).toBe(160);

    expect(calls.download.some(c => c.disposition === 'inline')).toBe(true);
    await page.screenshot({ path: evidencePath('03-video-inline.png'), fullPage: true });
  });

  test('04. 消えた添付・解決できない添付はプレビューを出さない', async ({ page }) => {
    const calls = await setupMocks(page, [
      task({
        attachments: [
          // 消えた（`exists=false`）: 種別は分かるが実体が無い
          attachment({ path: `${HOME}/out/gone.png`, name: 'gone.png', exists: false, available: false, root: null, path_rel: null }),
          // この端末のツリー配下に無い（role では落とせない）
          attachment({ path: '/opt/elsewhere/far.mp4', name: 'far.mp4', preview: 'video', available: false, root: null, path_rel: null }),
        ],
      }),
    ]);
    await openDetail(page);

    await expect(page.locator('[data-testid="task-attachment"]')).toHaveCount(2);
    await expect(page.locator('[data-testid="task-preview-image"]')).toHaveCount(0);
    await expect(page.locator('[data-testid="task-preview-video"]')).toHaveCount(0);
    // 従来どおり理由が出る（無言で押せない行にしない）
    await expect(page.locator('[data-testid="task-attachment"]').first()).toContainText('ファイルが消えています');
    await expect(page.locator('[data-testid="task-attachment"]').nth(1)).toContainText('PC のファイルツリーに出ていないフォルダです');
    expect(calls.download).toHaveLength(0);
    await page.screenshot({ path: evidencePath('04-unavailable.png'), fullPage: true });
  });

  test('05. 一覧を開いただけでは添付を 1 本も取りに行かない', async ({ page }) => {
    const heavy = [
      task({ id: 'u-1', attachments: [attachment(), attachment({ path: `${HOME}/out/v6.mp4`, name: 'v6.mp4', preview: 'video', path_rel: 'Users/testuser/out/v6.mp4', size: 298_000_000 })] }),
      task({ id: 'u-2', title: 'X にショート動画を投稿', attachments: [attachment({ path: `${HOME}/out/short.webm`, name: 'short.webm', preview: 'video', path_rel: 'Users/testuser/out/short.webm' })] }),
    ];
    const calls = await setupMocks(page, heavy);
    await page.goto(`${BASE}/#/tasks`);
    await page.waitForSelector('[data-testid="task-row"]');
    await expect(page.locator('[data-testid="task-row"]')).toHaveCount(2);
    // ポーリング 1 周期ぶん待っても飛ばない
    await page.waitForTimeout(1200);
    expect(calls.download, '一覧で添付を取りに行っている').toHaveLength(0);
    // ナビのバッジ経由（`#/panes`）でも同じ
    await page.goto(`${BASE}/#/panes`);
    await expect(page.locator('[data-testid="tasks-badge"]')).toHaveText('2');
    expect(calls.download).toHaveLength(0);

    // 詳細を開いて初めて飛ぶ
    await page.goto(`${BASE}/#/tasks?id=u-2`);
    await page.waitForSelector('[data-testid="task-preview-video"]');
    await expect.poll(() => calls.download.length, { timeout: 5000 }).toBeGreaterThan(0);
  });

  test('06. observe 端末はプレビューを出さない（取りに行けないので絵も出さない）', async ({ page }) => {
    const calls = await setupMocks(page, [task({ attachments: [attachment()] })], { role: 'observe' });
    await openDetail(page);
    await expect(page.locator('[data-testid="task-attachment"]')).toHaveCount(1);
    await expect(page.locator('[data-testid="task-preview-image"]')).toHaveCount(0);
    await expect(page.locator('[data-testid="task-attachment"]')).toContainText('この端末では保存できません');
    expect(calls.download).toHaveLength(0);
  });

  test('07. エッジ: 添付 0 件・画像と動画の混在・空白と日本語のファイル名', async ({ page }) => {
    const calls = await setupMocks(page, [
      task({ id: 'u-1', attachments: [] }),
      task({
        id: 'u-2', title: '混在',
        attachments: [
          attachment({ path: `${HOME}/out/サムネ 01.png`, name: 'サムネ 01.png', path_rel: 'Users/testuser/out/サムネ 01.png' }),
          attachment({ path: `${HOME}/out/clip.webm`, name: 'clip.webm', preview: 'video', path_rel: 'Users/testuser/out/clip.webm' }),
          attachment({ path: `${HOME}/out/notes.md`, name: 'notes.md', preview: null, path_rel: 'Users/testuser/out/notes.md' }),
        ],
      }),
    ]);

    // 添付 0 件 = 「添付」の節ごと出ない
    await openDetail(page, 'u-1');
    await expect(page.locator('[data-testid="task-attachment"]')).toHaveCount(0);
    expect(calls.download).toHaveLength(0);

    await openDetail(page, 'u-2');
    await expect(page.locator('[data-testid="task-attachment"]')).toHaveCount(3);
    await expect(page.locator('[data-testid="task-preview-image"]')).toHaveCount(1);
    await expect(page.locator('[data-testid="task-preview-video"]')).toHaveCount(1);
    // ビューアが無い種別（md）は従来どおり保存だけ
    await expect(page.locator('[data-testid="task-attachment"]').nth(2).locator('[data-testid="task-download"]')).toHaveCount(1);

    // 空白と日本語は URL エンコードされて往復する（綴りが壊れない）
    const src = await page.locator('[data-testid="task-preview-image"]').getAttribute('src');
    expect(src).toContain('%E3%82%B5%E3%83%A0%E3%83%8D');
    expect(src).not.toMatch(/path=[^&]*\s/);
    await expect
      .poll(() => page.locator('[data-testid="task-preview-image"]').evaluate(el => el.naturalWidth), { timeout: 5000 })
      .toBe(320);
    const seen = calls.download.find(c => c.disposition === 'inline' && c.path.includes('サムネ'));
    expect(seen, '日本語のファイル名がそのまま daemon へ渡る').toBeTruthy();
    expect(seen.path).toBe('Users/testuser/out/サムネ 01.png');
    await page.screenshot({ path: evidencePath('07-mixed.png'), fullPage: true });
  });

  test('08. 巨大画像でも一覧は軽いまま（詳細でだけ読み、頭が止まる）', async ({ page }) => {
    const big = attachment({ path: `${HOME}/out/huge.png`, name: 'huge.png', size: 42_000_000, path_rel: 'Users/testuser/out/huge.png' });
    const calls = await setupMocks(page, [task({ attachments: [big] })]);
    await page.goto(`${BASE}/#/tasks`);
    await page.waitForSelector('[data-testid="task-row"]');
    await expect(page.locator('[data-testid="task-row"]')).toContainText('解説動画');
    expect(calls.download).toHaveLength(0);

    await openDetail(page);
    const img = page.locator('[data-testid="task-preview-image"]');
    await expect
      .poll(() => img.evaluate(el => el.naturalWidth), { timeout: 5000 })
      .toBe(320);
    // CSS が頭を止めているので、詳細が画面外まで伸びない
    const height = await img.evaluate(el => el.getBoundingClientRect().height);
    expect(height).toBeGreaterThan(0);
    expect(height).toBeLessThanOrEqual(IPHONE_VIEWPORT.height * 0.5);
    // 取りに行くのは 1 回だけ（描き直しのたびに撃たない）
    expect(calls.download.filter(c => c.disposition === 'inline')).toHaveLength(1);
  });
});
