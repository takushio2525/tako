// Issue #1450 B3: スマホ（PWA）からユーザー向けタスクを片付ける。
//
// 受け入れ条件を、モバイル viewport の実 DOM で確かめる:
//   ① 起票されたタスクが一覧と詳細に出る（ナビに未完了件数のバッジ）
//   ② copy_texts の 1 件がワンタップでクリップボードへ入る（**中身を実測**）
//   ③ 添付は既存のファイル API（`/api/files/download?root=…&path=…`）へリンクし、
//      解決できない添付・消えた添付は**理由が出る**（無言で押せないボタンを出さない）
//   ④ observe 端末では返答 / 完了 / 却下が出ず、権限リクエスト（#1452）へ繋がる
//   ⑤ 返答 → やりとりに載り、配送の状態が出る（`sent` を「届いた」と書かない）
//   ⑥ 完了すると一覧から消える
//   ⑦ 画面を離れるとポーリングが止まる
//   ⑧ A/B（`?tako_1450b3_legacy=1`）では画面もバッジも出ない
//
// **実ファイルシステムも実タスクも一切見ない**（#927）。出てくるパスは
// `/Users/testuser/…` の偽物だけで、実ユーザー名・実ホームパスはスクショにも入らない。
//
// 実行:
//   cd web/tako-remote && npx playwright test e2e/tasks-1450b3.spec.js
import { test, expect } from '@playwright/test';
import { evidencePath, TAKO_VERSION } from './support.js';

const IPHONE_VIEWPORT = { width: 390, height: 844 };
const BASE = `http://localhost:${process.env.TAKO_PWA_PORT || 5174}`;

// 偽のホーム（#927）
const HOME = '/Users/testuser';
const NOW = Math.floor(Date.now() / 1000);

function me(role = 'manage') {
  return {
    registered: true, device_id: 'test-iphone', name: 'iPhone', role,
    login: 'user@example.com', host: 'test-mac', version: TAKO_VERSION, app_connected: true,
  };
}

// 投稿タスク（添付 = 動画 / 解決できる・消えた添付 / コピー用テキスト 3 種）
const POST_TASK = {
  id: 'u-12',
  title: '解説動画 v6 を YouTube へ投稿',
  body: '# 投稿の手順\n\n1. 動画を落とす\n2. タイトルと説明を貼る\n\n`tako` の紹介動画です。',
  kind: 'post',
  status: 'open',
  created_by: 'master:takodev',
  project: 'tako',
  attachments: [
    {
      path: `${HOME}/out/v6.mp4`, exists: true, name: 'v6.mp4', size: 28_400_000,
      root: 'fs', path_rel: 'Users/testuser/out/v6.mp4', available: true,
    },
    { path: `${HOME}/out/thumb.png`, exists: false, name: 'thumb.png', available: false },
  ],
  copy_texts: [
    { label: '投稿文', text: 'tako v0.8 を出しました。AI エージェントを 1 画面で見張れます。' },
    { label: 'タイトル', text: 'tako — AI エージェント集約ターミナル' },
    { label: 'タグ', text: '#tako #AI #terminal' },
  ],
  links: ['https://example.com/tako'],
  due: '2026-09-20',
  created_at: NOW - 7200,
  updated_at: NOW - 600,
  origin: { profile: 'takodev', session_id: 'sess-1', pane: 7, project: 'tako' },
  responses: [],
  delivery: null,
};

// レビュー依頼（返答済み = やりとりと配送の表示を確かめる）
const REVIEW_TASK = {
  id: 'u-11',
  title: 'PR #1460 のレビュー',
  body: '権限の導線を見てほしい。',
  kind: 'review',
  status: 'open',
  created_by: 'master:takodev',
  project: 'tako',
  attachments: [],
  copy_texts: [],
  links: [],
  due: null,
  created_at: NOW - 10800,
  updated_at: NOW - 3600,
  origin: { profile: 'takodev', session_id: 'sess-1', pane: 7, project: 'tako' },
  responses: [
    { decision: 'needs_change', comment: 'サムネの文字が小さい', at: NOW - 3600, via: 'pwa' },
  ],
  // **未確定**（「届いた」と書かない側）
  delivery: { state: 'sent', profile: 'takodev', pane: 7, tab: null, reason: null, at: NOW - 3600, response_index: 0 },
};

function json(route, body, status = 200) {
  return route.fulfill({ status, contentType: 'application/json', body: JSON.stringify(body) });
}

function listBody(tasks) {
  const open = tasks.filter(t => t.status === 'open');
  const byKind = {};
  for (const t of open) byKind[t.kind] = (byKind[t.kind] || 0) + 1;
  return {
    tasks,
    count: tasks.length,
    open_count: open.length,
    open_counts_by_kind: Object.entries(byKind).map(([kind, count]) => ({ kind, count })),
  };
}

/**
 * API をすべてモックする。`calls` に「PWA が何をどの順で叩いたか」が溜まる。
 * `opts.role` で端末の役割、`opts.actStatus` で操作系の 403 を再現できる
 */
async function setupMocks(page, opts = {}) {
  const { role = 'manage', actStatus = 200 } = opts;
  const calls = { list: [], respond: [], done: [], dismiss: [], download: [] };
  let tasks = [POST_TASK, REVIEW_TASK].map(t => JSON.parse(JSON.stringify(t)));

  await page.route('**/api/me', route => json(route, me(role)));
  await page.route('**/api/v2/panes', route => json(route, { api_version: 2, panes: [] }));
  await page.route('**/api/agents', route => json(route, { agents: [] }));
  await page.route('**/api/health', route => json(route, { status: 'ok', version: TAKO_VERSION }));
  await page.route('**/ws?*', route => route.abort());
  await page.route('**/manifest.json', route => json(route, { name: 'tako remote' }));
  await page.route('**/sw.js', route =>
    route.fulfill({ status: 200, contentType: 'application/javascript', body: '' })
  );

  // 添付のダウンロードは**既存のファイル API**（新しい経路が生えていないことの裏返し）
  await page.route('**/api/files/download*', route => {
    const params = new URL(route.request().url()).searchParams;
    calls.download.push({ root: params.get('root'), path: params.get('path') });
    return route.fulfill({
      status: 200,
      headers: { 'Content-Disposition': 'attachment; filename="v6.mp4"' },
      contentType: 'application/octet-stream',
      body: 'fake-mp4',
    });
  });

  await page.route('**/api/tasks**', route => {
    const url = new URL(route.request().url());
    const method = route.request().method();
    const path = url.pathname;
    if (method === 'GET') {
      calls.list.push({ all: url.searchParams.get('all'), kind: url.searchParams.get('kind') });
      const all = url.searchParams.get('all') === '1';
      return json(route, listBody(all ? tasks : tasks.filter(t => t.status === 'open')));
    }
    const id = path.split('/')[3];
    const verb = path.split('/')[4];
    if (actStatus !== 200) {
      return json(route, { error: `権限が足りません（${role} → interact 以上が必要）` }, actStatus);
    }
    const target = tasks.find(t => t.id === id);
    if (!target) return json(route, { error: 'タスクが見つからない', kind: 'not_found' }, 404);
    if (verb === 'respond') {
      const body = route.request().postDataJSON() || {};
      calls.respond.push({ id, ...body });
      target.responses = [
        ...target.responses,
        { decision: body.decision, comment: body.comment || '', at: NOW, via: 'pwa' },
      ];
      // daemon は配送を試みた結果を返す（ここでは届いた側）
      target.delivery = {
        state: 'delivered', profile: 'takodev', pane: 7, tab: null,
        reason: null, at: NOW, response_index: target.responses.length - 1,
      };
      return json(route, target);
    }
    if (verb === 'done' || verb === 'dismiss') {
      calls[verb].push(id);
      target.status = verb === 'done' ? 'done' : 'dismissed';
      return json(route, target);
    }
    return json(route, { error: 'API エンドポイントが見つからない' }, 404);
  });

  return calls;
}

async function openTasks(page, query = '') {
  await page.goto(`${BASE}/${query}#/tasks`);
  // **中身が届くまで待つ**（`.task-list` の器は取得前から在るので、
  // それを待つと「まだ 1 回も叩いていない」状態で先へ進んでしまう = 実測）
  await page.waitForSelector('[data-testid="task-row"], .empty-state h2', { timeout: 10000 });
}

test.describe('#1450 B3 スマホからタスクを片付ける — モバイル', () => {
  test.use({ viewport: IPHONE_VIEWPORT });

  test('01. 一覧に出て、ナビに未完了件数のバッジが出る', async ({ page }) => {
    await setupMocks(page);
    await page.goto(`${BASE}/#/panes`);
    await expect(page.locator('[data-testid="tasks-badge"]')).toHaveText('2');

    await page.locator('[data-testid="tasks-entry"]').click();
    await page.waitForSelector('[data-testid="task-list"]');
    const rows = page.locator('[data-testid="task-row"]');
    await expect(rows).toHaveCount(2);
    await expect(rows.first()).toContainText('解説動画 v6 を YouTube へ投稿');
    await expect(rows.first()).toContainText('投稿');
    // 未確定の配送を「届いた」と書かない
    await expect(rows.nth(1)).toContainText('送信済み（確認待ち）');
    await page.screenshot({ path: evidencePath('01-list.png'), fullPage: true });
  });

  test('02. 詳細に本文 / 添付 / コピー用テキスト / リンクが出る', async ({ page }) => {
    await setupMocks(page);
    await openTasks(page);
    await page.locator('[data-testid="task-row"]').first().click();
    await page.waitForSelector('[data-testid="task-detail"]');

    // markdown が描かれている（見出しが h1 として出る）
    await expect(page.locator('.task-body h1')).toHaveText('投稿の手順');
    // 添付は 2 件（落とせるものと、消えたもの）
    const atts = page.locator('[data-testid="task-attachment"]');
    await expect(atts).toHaveCount(2);
    await expect(atts.first()).toContainText('v6.mp4');
    await expect(atts.first()).toContainText('27.1 MB');
    await expect(atts.nth(1)).toContainText('見つかりません');
    await expect(atts.nth(1)).toContainText('ファイルが消えています');
    await expect(atts.nth(1).locator('[data-testid="task-download"]')).toHaveCount(0);
    // コピー用テキストは 3 件
    await expect(page.locator('[data-testid="task-copy"]')).toHaveCount(3);
    await expect(page.locator('.task-link')).toHaveText('https://example.com/tako');
    await page.screenshot({ path: evidencePath('02-detail.png'), fullPage: true });
  });

  test('03. 添付のリンクが既存のファイル API（root + 相対パス）を指す', async ({ page }) => {
    await setupMocks(page);
    await openTasks(page);
    await page.locator('[data-testid="task-row"]').first().click();
    const link = page.locator('[data-testid="task-download"]').first();
    const href = await link.getAttribute('href');
    expect(href).toContain('/api/files/download?root=fs&path=');
    expect(href).toContain(encodeURIComponent('Users/testuser/out/v6.mp4'));
    // **絶対パスを URL に載せない**（#1079 の約束）
    expect(href).not.toContain('/Users/testuser/out/v6.mp4');
  });

  test('04. コピーがワンタップでクリップボードへ入る', async ({ page, context }) => {
    await context.grantPermissions(['clipboard-read', 'clipboard-write']);
    await setupMocks(page);
    await openTasks(page);
    await page.locator('[data-testid="task-row"]').first().click();

    await page.locator('[data-testid="task-copy-btn"]').first().click();
    await expect(page.locator('.task-copy-state').first()).toHaveText('コピーしました');
    const clip = await page.evaluate(() => navigator.clipboard.readText());
    expect(clip).toBe('tako v0.8 を出しました。AI エージェントを 1 画面で見張れます。');

    // **その 1 件だけ**が入る（3 つ目 = タグを押すとタグに変わる）
    await page.locator('[data-testid="task-copy-btn"]').nth(2).click();
    const clip2 = await page.evaluate(() => navigator.clipboard.readText());
    expect(clip2).toBe('#tako #AI #terminal');
  });

  test('05. 返答するとやりとりに載り、配送の状態が出る', async ({ page }) => {
    const calls = await setupMocks(page);
    await openTasks(page);
    await page.locator('[data-testid="task-row"]').first().click();

    // 判断を選ぶまで送れない（誤爆防止）
    await expect(page.locator('[data-testid="task-send"]')).toBeDisabled();
    await expect(page.locator('.task-hint')).toHaveText('判断を選ぶと返せます');
    await page.locator('[data-testid="task-decision-needs_change"]').click();
    await page.locator('.task-comment').fill('サムネを差し替えてほしい');
    await page.locator('[data-testid="task-send"]').click();

    await expect(page.locator('.task-response')).toHaveCount(1);
    await expect(page.locator('.task-response').first()).toContainText('直してほしい');
    await expect(page.locator('.task-response').first()).toContainText('サムネを差し替えてほしい');
    await expect(page.locator('.task-delivery-line')).toContainText('master に届きました');
    expect(calls.respond).toEqual([
      { id: 'u-12', decision: 'needs_change', comment: 'サムネを差し替えてほしい' },
    ]);
    await page.screenshot({ path: evidencePath('05-responded.png'), fullPage: true });
  });

  test('06. 完了すると一覧から消える', async ({ page }) => {
    const calls = await setupMocks(page);
    await openTasks(page);
    await page.locator('[data-testid="task-row"]').first().click();
    await page.locator('[data-testid="task-done"]').click();

    await page.waitForSelector('[data-testid="task-list"]');
    await expect(page.locator('[data-testid="task-row"]')).toHaveCount(1);
    await expect(page.locator('[data-testid="task-row"]').first()).toContainText('PR #1460');
    expect(calls.done).toEqual(['u-12']);
  });

  test('07. observe は閲覧だけで、権限リクエストへ繋がる', async ({ page }) => {
    await setupMocks(page, { role: 'observe' });
    await openTasks(page);
    await expect(page.locator('[data-testid="task-row"]')).toHaveCount(2);
    await page.locator('[data-testid="task-row"]').first().click();

    // 返答フォームも完了 / 却下も出ない
    await expect(page.locator('[data-testid="task-respond"]')).toHaveCount(0);
    await expect(page.locator('[data-testid="task-done"]')).toHaveCount(0);
    await expect(page.locator('[data-testid="task-dismiss"]')).toHaveCount(0);
    // 行き止まりにしない（#1452 の導線）
    await expect(page.locator('.permission-request, .permission-reason')).toHaveCount(1);
    // 添付も落とせない（理由つき）
    await expect(page.locator('[data-testid="task-download"]')).toHaveCount(0);
    await expect(page.locator('.task-attachment-why').first()).toContainText('権限が足りません');
    await page.screenshot({ path: evidencePath('07-observe.png'), fullPage: true });
  });

  test('08. タスクが 0 件なら「人がやることはありません」', async ({ page }) => {
    await setupMocks(page);
    await page.route('**/api/tasks**', route => json(route, listBody([])));
    await openTasks(page);
    await expect(page.locator('.empty-state h2')).toHaveText('人がやることはありません');
    // バッジも出ない（0 を出して「待ち 0」と主張しない）
    await page.goto(`${BASE}/#/panes`);
    await expect(page.locator('[data-testid="tasks-badge"]')).toHaveCount(0);
  });

  test('09. 画面を離れるとポーリングが止まる', async ({ page }) => {
    const calls = await setupMocks(page);
    await openTasks(page);
    const before = calls.list.length;
    expect(before).toBeGreaterThan(0);

    // ペイン一覧へ戻る（タスク画面の interval は cleanup で止まる）。
    // バッジ側も止まることを確かめたいので `?tako_1450b3_legacy=1` で無効化して離れる
    await page.goto(`${BASE}/?tako_1450b3_legacy=1#/panes`);
    await page.waitForTimeout(200);
    const afterLeave = calls.list.length;
    // 6 秒（= 周期 5 秒 + 余裕）待っても増えない
    await page.waitForTimeout(6000);
    expect(calls.list.length).toBe(afterLeave);
  });

  test('10. A/B: legacy 腕では画面もバッジも出ない', async ({ page }) => {
    await setupMocks(page);
    await page.goto(`${BASE}/?tako_1450b3_legacy=1#/panes`);
    await expect(page.locator('[data-testid="tasks-entry"]')).toHaveCount(0);
    await expect(page.locator('[data-testid="tasks-badge"]')).toHaveCount(0);

    await page.goto(`${BASE}/?tako_1450b3_legacy=1#/tasks`);
    await expect(page.locator('.empty-state h2')).toHaveText('タスクの画面がありません');
    await expect(page.locator('[data-testid="task-row"]')).toHaveCount(0);
  });

  test('11. 完了も見るで、片付いたものも読める', async ({ page }) => {
    const calls = await setupMocks(page);
    await openTasks(page);
    await page.getByText('完了も見る').click();
    await expect.poll(() => calls.list.some(c => c.all === '1')).toBe(true);
  });
});
