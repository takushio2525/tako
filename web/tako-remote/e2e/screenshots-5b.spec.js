// Issue #285: 弾5b カンプ横並びスクショ比較用 Playwright テスト。
// カンプ 1b（承認カード）/ 1c（選択肢ボタン）/ 1e（スラコマ候補）/
// 1f（モデルシート）/ 1g（添付シート）を iPhone viewport で撮影する。
import { test, expect } from '@playwright/test';

const EVIDENCE_DIR = process.env.HOME + '/Desktop/tako-285-evidence';
const IPHONE_VIEWPORT = { width: 390, height: 844 };
const BASE = 'http://localhost:5174';

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
      approval: {
        tool: 'Bash',
        command: 'rm -rf dist/ && npm run build',
      },
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
    await page.waitForTimeout(1000);
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

  // --- 受け入れ条件①: 承認カード通し実測 ---
  // 承認カード表示 → 許可(y)タップ → dispatch Send (POST /api/panes/:id/input) へ
  // text="y" newline=true が送信されることを Playwright でキャプチャして検証する
  test('承認カード通し: 許可(y)タップ → input API に y が送信される', async ({ page }) => {
    const inputRequests = [];
    await setupMocks(page);
    await page.route('**/api/sessions/codex-session/messages*', route =>
      route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify(FAKE_MESSAGES_1B) })
    );
    // input API をキャプチャ（dispatch Send への入口）
    await page.route('**/api/panes/*/input', async route => {
      const body = route.request().postDataJSON();
      inputRequests.push(body);
      await route.fulfill({ status: 200, contentType: 'application/json', body: '{"ok":true}' });
    });
    await page.goto(`${BASE}/#/panes/2`);
    await page.waitForSelector('.approval-card', { timeout: 10000 });
    await page.screenshot({ path: `${EVIDENCE_DIR}/e2e-approval-before.png`, fullPage: false });

    // 許可(y)ボタンをタップ
    await page.click('.approval-btn-allow');
    await page.waitForTimeout(500);
    await page.screenshot({ path: `${EVIDENCE_DIR}/e2e-approval-after.png`, fullPage: false });

    // input API に "y" が送信されたことを検証
    expect(inputRequests.length).toBeGreaterThan(0);
    const lastInput = inputRequests[inputRequests.length - 1];
    expect(lastInput.text).toBe('y');
    expect(lastInput.newline).toBe(true);
  });

  test('承認カード通し: 拒否(n)タップ → input API に n が送信される', async ({ page }) => {
    const inputRequests = [];
    await setupMocks(page);
    await page.route('**/api/sessions/codex-session/messages*', route =>
      route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify(FAKE_MESSAGES_1B) })
    );
    await page.route('**/api/panes/*/input', async route => {
      const body = route.request().postDataJSON();
      inputRequests.push(body);
      await route.fulfill({ status: 200, contentType: 'application/json', body: '{"ok":true}' });
    });
    await page.goto(`${BASE}/#/panes/2`);
    await page.waitForSelector('.approval-card', { timeout: 10000 });

    // 拒否(n)ボタンをタップ
    await page.click('.approval-btn-deny');
    await page.waitForTimeout(500);

    expect(inputRequests.length).toBeGreaterThan(0);
    const lastInput = inputRequests[inputRequests.length - 1];
    expect(lastInput.text).toBe('n');
    expect(lastInput.newline).toBe(true);
  });

  // 承認カードで Observe role の場合ボタンが disabled で送信されないことの検証
  test('承認カード: Observe role ではボタンが disabled', async ({ page }) => {
    const inputRequests = [];
    await setupMocks(page, FAKE_ME_OBSERVE);
    await page.route('**/api/sessions/codex-session/messages*', route =>
      route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify(FAKE_MESSAGES_1B) })
    );
    await page.route('**/api/panes/*/input', async route => {
      const body = route.request().postDataJSON();
      inputRequests.push(body);
      await route.fulfill({ status: 200, contentType: 'application/json', body: '{"ok":true}' });
    });
    await page.goto(`${BASE}/#/panes/2`);
    await page.waitForSelector('.approval-card', { timeout: 10000 });

    // ボタンが disabled であること
    const allowBtn = page.locator('.approval-btn-allow');
    await expect(allowBtn).toBeDisabled();

    // クリックしても input は呼ばれない
    await allowBtn.click({ force: true });
    await page.waitForTimeout(500);
    expect(inputRequests.length).toBe(0);
  });
});
