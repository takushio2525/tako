// Issue #285: 弾5b カンプ横並びスクショ比較用 Playwright テスト。
// カンプ 1b（承認カード）/ 1c（選択肢ボタン）/ 1e（スラコマ候補）/
// 1f（モデルシート）/ 1g（添付シート）を iPhone viewport で撮影する。
import { test, expect } from '@playwright/test';

const EVIDENCE_DIR = process.env.HOME + '/Desktop/tako-285-evidence';
const IPHONE_VIEWPORT = { width: 390, height: 844 };
const BASE = `http://localhost:${process.env.TAKO_PWA_PORT || 5174}`;

const FAKE_ME = {
  registered: true,
  device_id: 'test-iphone',
  name: 'iPhone',
  role: 'interact',
  login: 'user@example.com',
  host: 'test-mac',
  version: '0.5.5',
  app_connected: true,
};

const FAKE_ME_OBSERVE = {
  ...FAKE_ME,
  role: 'observe',
};

// #425 / #444: 承認待ちの正は「ペイン画面に permission ダイアログが実在すること」。
// daemon は `/api/v2/panes` の各ペインへ `permission_dialog {command, options, highlighted}`
// を付けて返す（`crates/tako-control/src/remote.rs` の `attach_card_summaries`。
// 中身は `claude_tui::detect_permission_dialog` → `PermissionDialog`）。
// 下の値は claude v2.x 実採取相当の画面をその実装へ通して得た出力そのまま:
//   command: String / options: Vec<String> / highlighted: Option<usize>
const FAKE_PERMISSION_DIALOG = {
  command: 'Bash command rm -rf dist/ && npm run build Remove build output and rebuild Do you want to proceed?',
  options: [
    'Yes',
    "Yes, and don't ask again for rm commands",
    'No, and tell Claude what to do differently (esc)',
  ],
  highlighted: 0,
};

const FAKE_PANES = {
  panes: [
    {
      id: 1, title: 'fix-auth', role: 'orchestrator-worker-claude',
      agent_type: 'claude', cwd: '/dev/project', state: 'busy',
      surface: 'foreground', position: '2/4', tab_id: 1, tab_title: 'work',
      cols: 120, rows: 40, focused: false, session_id: 'abc-def-123',
      model: 'opus 4.5', effort: 'high',
    },
    {
      id: 2, title: 'refactor-api', role: 'orchestrator-worker-codex',
      agent_type: 'codex', cwd: '/dev/project', state: 'running',
      surface: 'foreground', position: '3/4', tab_id: 1, tab_title: 'work',
      cols: 120, rows: 40, focused: false, session_id: 'codex-session',
      model: 'gpt-5.6', effort: 'medium',
      // ダイアログが画面に在るペイン。daemon は同時に activity も permission にする
      permission_dialog: FAKE_PERMISSION_DIALOG,
      activity: 'permission',
    },
    {
      id: 3, title: 'docs-site', role: 'orchestrator-worker-agy',
      agent_type: 'agy', cwd: '/dev/docs', state: 'running',
      surface: 'foreground', position: '4/4', tab_id: 1, tab_title: 'work',
      cols: 120, rows: 40, focused: false, session_id: 'agy-session',
      model: 'gemini 3.5', effort: 'fast',
    },
  ],
  api_version: 2,
};

// 1b: codex の承認待ちカード
const FAKE_MESSAGES_1B = {
  session_id: 'codex-session',
  messages: [
    {
      role: 'user',
      text: '/routes 配下のハンドラを async に統一して',
      timestamp: '2026-07-17T21:15:00Z',
    },
    {
      role: 'assistant',
      text: '対象は6ファイルです。順に書き換えます。',
      tools: [
        { name: 'Read', summary: 'src/routes/*.ts 6 files' },
        { name: 'Edit', summary: 'users.ts / orders.ts +4 +61 -58' },
      ],
      // 旧実装は transcript の `approval` から承認カードを推定していたが、
      // #425 で廃止した（auto mode の実行中と承認待ちを区別できず誤表示していた）。
      // いま card を出すのは上の FAKE_PERMISSION_DIALOG だけなので、
      // ここに置き直すと「効いている」と誤解される
      timestamp: '2026-07-17T21:16:00Z',
    },
  ],
};

// 1c: agy の選択肢ボタン
const FAKE_MESSAGES_1C = {
  session_id: 'agy-session',
  messages: [
    {
      role: 'user',
      text: 'docsサイトのビルドが遅い。原因調べて',
      timestamp: '2026-07-17T20:02:00Z',
    },
    {
      role: 'assistant',
      text: 'ビルドをプロファイルしました。画像最適化が全体の82%を占めています。\nキャッシュを有効化すれば2回目以降は〜6秒になります。設定を変更しますか？',
      tools: [
        { name: 'Bash', summary: 'npm run build --profile' },
      ],
      choices: ['変更する', '詳細を見る'],
      timestamp: '2026-07-17T20:03:00Z',
    },
  ],
};

// 1e: claude のスラコマ候補（会話済み + /c 入力中）
const FAKE_MESSAGES_1E = {
  session_id: 'abc-def-123',
  messages: [
    {
      role: 'user',
      text: '認証のバグを直して',
      timestamp: '2026-07-17T22:41:00Z',
    },
    {
      role: 'assistant',
      text: '修正が完了しました。全テストがパスしています。',
      timestamp: '2026-07-17T22:43:00Z',
    },
  ],
};

async function setupMocks(page, meData = FAKE_ME) {
  await page.route('**/api/me', route =>
    route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify(meData) })
  );
  await page.route('**/api/v2/panes', route =>
    route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify(FAKE_PANES) })
  );
  await page.route('**/api/health', route =>
    route.fulfill({ status: 200, contentType: 'application/json', body: '{"status":"ok","version":"0.5.5"}' })
  );
  await page.route('**/api/agents', route =>
    route.fulfill({ status: 200, contentType: 'application/json', body: '{"agents":[]}' })
  );
  await page.route('**/ws?*', route => route.abort());
  await page.route('**/manifest.json', route =>
    route.fulfill({ status: 200, contentType: 'application/json', body: '{"name":"tako remote"}' })
  );
  await page.route('**/sw.js', route =>
    route.fulfill({ status: 200, contentType: 'application/javascript', body: '' })
  );
  await page.route('**/api/panes/*/screen*', route =>
    route.fulfill({ status: 200, contentType: 'application/json', body: '{"lines":[],"cursor":{"x":0,"y":0},"size":{"cols":120,"rows":40}}' })
  );
}

function trackExternalRequests(page) {
  const external = [];
  page.on('request', req => {
    const url = req.url();
    if (
      url.startsWith('http://localhost') || url.startsWith('https://localhost') ||
      url.startsWith('ws://localhost') || url.startsWith('wss://localhost') ||
      url.startsWith('data:') || url.startsWith('blob:')
    ) return;
    external.push(url);
  });
  return external;
}

test.describe('弾5b: UI 高度機能スクショ — iPhone viewport', () => {
  test.use({ viewport: IPHONE_VIEWPORT });

  test('1b. 承認待ちカード（codex）', async ({ page }) => {
    const external = trackExternalRequests(page);
    await setupMocks(page);
    await page.route('**/api/sessions/codex-session/messages*', route =>
      route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify(FAKE_MESSAGES_1B) })
    );
    await page.goto(`${BASE}/#/panes/2`);
    await page.waitForSelector('.chat-scroll', { timeout: 10000 });
    // 承認カードが実際に出るまで待つ。固定時間で撮ると #1089 のように
    // 「カードの無い承認カードのスクショ」が無言で撮れ続ける（#796 の作法）
    await page.waitForSelector('.approval-card', { timeout: 10000 });
    await page.screenshot({ path: `${EVIDENCE_DIR}/1b-approval-card.png`, fullPage: false });
    expect(external).toEqual([]);
  });

  test('1c. 選択肢ボタン（agy）', async ({ page }) => {
    const external = trackExternalRequests(page);
    await setupMocks(page);
    await page.route('**/api/sessions/agy-session/messages*', route =>
      route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify(FAKE_MESSAGES_1C) })
    );
    await page.goto(`${BASE}/#/panes/3`);
    await page.waitForSelector('.chat-scroll', { timeout: 10000 });
    await page.waitForTimeout(1000);
    await page.screenshot({ path: `${EVIDENCE_DIR}/1c-choice-buttons.png`, fullPage: false });
    expect(external).toEqual([]);
  });

  test('1e. スラコマ候補', async ({ page }) => {
    const external = trackExternalRequests(page);
    await setupMocks(page);
    await page.route('**/api/sessions/abc-def-123/messages*', route =>
      route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify(FAKE_MESSAGES_1E) })
    );
    await page.goto(`${BASE}/#/panes/1`);
    await page.waitForSelector('.composer-input', { timeout: 10000 });
    await page.waitForTimeout(500);
    await page.fill('.composer-input', '/c');
    await page.waitForTimeout(300);
    await page.screenshot({ path: `${EVIDENCE_DIR}/1e-slash-commands.png`, fullPage: false });
    expect(external).toEqual([]);
  });

  test('1f. モデル/エフォートシート', async ({ page }) => {
    const external = trackExternalRequests(page);
    await setupMocks(page);
    await page.route('**/api/sessions/abc-def-123/messages*', route =>
      route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify(FAKE_MESSAGES_1E) })
    );
    await page.goto(`${BASE}/#/panes/1`);
    await page.waitForSelector('.composer-chip', { timeout: 10000 });
    await page.waitForTimeout(500);
    await page.click('.composer-chip');
    await page.waitForSelector('.sheet-panel', { timeout: 5000 });
    await page.waitForTimeout(300);
    await page.screenshot({ path: `${EVIDENCE_DIR}/1f-model-effort-sheet.png`, fullPage: false });
    expect(external).toEqual([]);
  });

  test('1g. ファイル添付シート', async ({ page }) => {
    const external = trackExternalRequests(page);
    await setupMocks(page);
    await page.route('**/api/sessions/abc-def-123/messages*', route =>
      route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify(FAKE_MESSAGES_1E) })
    );
    await page.goto(`${BASE}/#/panes/1`);
    await page.waitForSelector('.composer-btn-attach', { timeout: 10000 });
    await page.waitForTimeout(500);
    await page.click('.composer-btn-attach');
    await page.waitForSelector('.attach-sources', { timeout: 5000 });
    await page.waitForTimeout(300);
    await page.screenshot({ path: `${EVIDENCE_DIR}/1g-attach-sheet.png`, fullPage: false });
    expect(external).toEqual([]);
  });

  test('外部リクエスト 0 件', async ({ page }) => {
    const external = trackExternalRequests(page);
    await setupMocks(page);
    await page.route('**/api/sessions/*/messages*', route =>
      route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify(FAKE_MESSAGES_1B) })
    );
    await page.goto(`${BASE}/#/panes/2`);
    await page.waitForTimeout(2000);
    expect(external).toEqual([]);
  });

  // --- 承認カード通し実測（#425 で経路が変わった。#1089 で追従）---
  // 表示: `/api/v2/panes` の `permission_dialog` 実在（transcript の推定は廃止）
  // 応答: `POST /api/panes/:id/respond` `{choice: "<1 始まりの番号>"}`
  //       （`src/api.js` の `respond()`。旧経路の input へ "y"/"n" 直送ではない）

  // 承認カード 3 件の共通モック。respond（現行）と input（旧経路）の両方を
  // キャプチャして、押した先が respond であることまで固定する
  async function setupApprovalMocks(page, meData = FAKE_ME) {
    const respondRequests = [];
    const inputRequests = [];
    await setupMocks(page, meData);
    await page.route('**/api/sessions/codex-session/messages*', route =>
      route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify(FAKE_MESSAGES_1B) })
    );
    await page.route('**/api/panes/*/respond', async route => {
      respondRequests.push({ url: route.request().url(), body: route.request().postDataJSON() });
      // PWA は応答本文を読まない（respondToDialog は結果を捨てる）ので最小形で足りる
      await route.fulfill({ status: 200, contentType: 'application/json', body: '{"ok":true}' });
    });
    await page.route('**/api/panes/*/input', async route => {
      inputRequests.push(route.request().postDataJSON());
      await route.fulfill({ status: 200, contentType: 'application/json', body: '{"ok":true}' });
    });
    return { respondRequests, inputRequests };
  }

  test('承認カード通し: 許可（1 番目の選択肢）タップ → respond API へ choice=1 が送信される', async ({ page }) => {
    const { respondRequests, inputRequests } = await setupApprovalMocks(page);
    await page.goto(`${BASE}/#/panes/2`);
    await page.waitForSelector('.approval-card', { timeout: 10000 });

    // ボタンは dialog.options そのもの（= どこから来た選択肢かを縛る）
    const buttons = page.locator('.approval-card button');
    await expect(buttons).toHaveCount(FAKE_PERMISSION_DIALOG.options.length);
    await expect(buttons.nth(0)).toHaveText(`1. ${FAKE_PERMISSION_DIALOG.options[0]}`);
    await page.screenshot({ path: `${EVIDENCE_DIR}/e2e-approval-before.png`, fullPage: false });

    // 許可 = 最後以外の選択肢（approval-btn-allow）。3 択なので 1 番目を押す
    await page.locator('.approval-btn-allow').first().click();

    // 固定時間ではなくリクエスト到達を待つ（#796）
    await expect.poll(() => respondRequests.length, { timeout: 5000 }).toBeGreaterThan(0);
    await page.screenshot({ path: `${EVIDENCE_DIR}/e2e-approval-after.png`, fullPage: false });

    const last = respondRequests[respondRequests.length - 1];
    expect(last.url).toContain('/api/panes/2/respond');
    expect(last.body).toEqual({ choice: '1' });
    // 旧経路（input へ "y" 直送）は通らない = サーバーがダイアログ実在を再検証する契約
    expect(inputRequests).toEqual([]);
  });

  test('承認カード通し: 拒否（最後の選択肢）タップ → respond API へ choice=3 が送信される', async ({ page }) => {
    const { respondRequests, inputRequests } = await setupApprovalMocks(page);
    await page.goto(`${BASE}/#/panes/2`);
    await page.waitForSelector('.approval-card', { timeout: 10000 });

    // 拒否 = 最後の選択肢（approval-btn-deny）。1 枚だけ
    const denyBtn = page.locator('.approval-btn-deny');
    await expect(denyBtn).toHaveCount(1);
    const denyIndex = FAKE_PERMISSION_DIALOG.options.length - 1;
    await expect(denyBtn).toHaveText(`${denyIndex + 1}. ${FAKE_PERMISSION_DIALOG.options[denyIndex]}`);
    await denyBtn.click();

    await expect.poll(() => respondRequests.length, { timeout: 5000 }).toBeGreaterThan(0);
    const last = respondRequests[respondRequests.length - 1];
    expect(last.url).toContain('/api/panes/2/respond');
    expect(last.body).toEqual({ choice: String(denyIndex + 1) });
    expect(inputRequests).toEqual([]);
  });

  test('承認カード: Observe role ではボタンが disabled', async ({ page }) => {
    const { respondRequests, inputRequests } = await setupApprovalMocks(page, FAKE_ME_OBSERVE);
    await page.goto(`${BASE}/#/panes/2`);
    await page.waitForSelector('.approval-card', { timeout: 10000 });

    // 全ボタンが disabled（respond は Interact 以上。サーバー側も 403 で断る）
    await expect(page.locator('.approval-btn-allow').first()).toBeDisabled();
    await expect(page.locator('.approval-btn-deny')).toBeDisabled();

    // クリックしても送信されない。固定時間待ちの代わりに「次の messages ポーリング」を
    // 関門にする（respond は onClick から同期で飛ぶので、ポーリングが 1 周してなお
    // 0 件なら飛んでいない）。#796
    await page.locator('.approval-btn-allow').first().click({ force: true });
    await page.waitForRequest(
      req => req.url().includes('/api/sessions/codex-session/messages'),
      { timeout: 10000 }
    );
    expect(respondRequests).toEqual([]);
    expect(inputRequests).toEqual([]);
  });
});
