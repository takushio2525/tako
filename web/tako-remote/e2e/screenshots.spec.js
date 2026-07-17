// Issue #284: カンプとの横並びスクショ比較用 Playwright テスト。
// Vite dev サーバーに接続し、API をモック（page.route）して各画面を iPhone viewport で撮影する。
// 実行: cd web/tako-remote && npx playwright test e2e/screenshots.spec.js
import { test, expect } from '@playwright/test';

const EVIDENCE_DIR = process.env.HOME + '/Desktop/tako-284-evidence';
const IPHONE_VIEWPORT = { width: 390, height: 844 };
const BASE = 'http://localhost:5174';

// モックデータ
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

const FAKE_ME_PENDING = {
  registered: false,
  pending: true,
  host: 'test-mac',
  version: '0.5.5',
};

const FAKE_ME_DENIED = {
  registered: false,
  denied: true,
  host: 'test-mac',
  version: '0.5.5',
};

const FAKE_PANES = {
  panes: [
    {
      id: 1, title: 'fix-auth', role: 'orchestrator-worker-claude',
      agent_type: 'claude', cwd: '/dev/project', state: 'busy',
      surface: 'foreground', position: '2/4', tab_id: 1, tab_title: 'work',
      cols: 120, rows: 40, focused: false, session_id: 'abc-def-123',
      model: 'opus 4.5',
    },
    {
      id: 2, title: 'refactor-api', role: 'orchestrator-worker-codex',
      agent_type: 'codex', cwd: '/dev/project', state: 'running',
      surface: 'foreground', position: '3/4', tab_id: 1, tab_title: 'work',
      cols: 120, rows: 40, focused: false, model: 'gpt-5.6',
    },
    {
      id: 3, title: 'docs-site', role: 'orchestrator-worker-agy',
      agent_type: 'agy', cwd: '/dev/docs', state: 'running',
      surface: 'foreground', position: '4/4', tab_id: 1, tab_title: 'work',
      cols: 120, rows: 40, focused: false, model: 'gemini 3.5',
    },
    {
      id: 4, title: 'master', role: 'master', agent_type: 'claude',
      cwd: '/dev/project', state: 'idle', surface: 'foreground',
      position: '1/4', tab_id: 1, tab_title: 'work', cols: 120, rows: 40,
      focused: true, session_id: 'master-session',
    },
  ],
  api_version: 2,
};

const FAKE_MESSAGES = {
  session_id: 'abc-def-123',
  messages: [
    {
      role: 'user',
      text: '認証のバグを直して。tests/auth.test.ts が2件落ちてる',
      timestamp: '2026-07-17T22:41:00Z',
    },
    {
      role: 'assistant',
      text: 'テストを確認します。トークンの有効期限判定が原因のようです。',
      tools: [
        { name: 'Bash', summary: 'npm test -- auth' },
        { name: 'Edit', summary: 'src/auth/token.ts +8 -3' },
      ],
      timestamp: '2026-07-17T22:42:00Z',
    },
    {
      role: 'assistant',
      text: '期限比較を `<=` に修正しました。再テストします。',
      timestamp: '2026-07-17T22:43:00Z',
    },
  ],
};

const FAKE_SCREEN = {
  lines: [
    '$ npm test -- auth',
    'PASS tests/auth.test.ts',
    '  OK token refresh (42ms)',
    '  OK expiry check (8ms)',
    '  16 passed',
  ],
  cursor: { x: 0, y: 5 },
  size: { cols: 120, rows: 40 },
};

async function setupMocks(page) {
  await page.route('**/api/me', route =>
    route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify(FAKE_ME) })
  );
  await page.route('**/api/v2/panes', route =>
    route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify(FAKE_PANES) })
  );
  await page.route('**/api/panes/*/screen*', route =>
    route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify(FAKE_SCREEN) })
  );
  await page.route('**/api/sessions/*/messages*', route =>
    route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify(FAKE_MESSAGES) })
  );
  await page.route('**/api/agents', route =>
    route.fulfill({ status: 200, contentType: 'application/json', body: '{"agents":[]}' })
  );
  await page.route('**/api/health', route =>
    route.fulfill({ status: 200, contentType: 'application/json', body: '{"status":"ok","version":"0.5.5"}' })
  );
  // WS 接続はモックページでは使わない
  await page.route('**/ws?*', route => route.abort());
  await page.route('**/manifest.json', route =>
    route.fulfill({ status: 200, contentType: 'application/json', body: '{"name":"tako remote"}' })
  );
  await page.route('**/sw.js', route =>
    route.fulfill({ status: 200, contentType: 'application/javascript', body: '' })
  );
}

async function setupPendingMocks(page) {
  await page.route('**/api/me', route =>
    route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify(FAKE_ME_PENDING) })
  );
  await page.route('**/manifest.json', route =>
    route.fulfill({ status: 200, contentType: 'application/json', body: '{"name":"tako remote"}' })
  );
  await page.route('**/sw.js', route =>
    route.fulfill({ status: 200, contentType: 'application/javascript', body: '' })
  );
}

async function setupDeniedMocks(page) {
  await page.route('**/api/me', route =>
    route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify(FAKE_ME_DENIED) })
  );
  await page.route('**/manifest.json', route =>
    route.fulfill({ status: 200, contentType: 'application/json', body: '{"name":"tako remote"}' })
  );
  await page.route('**/sw.js', route =>
    route.fulfill({ status: 200, contentType: 'application/javascript', body: '' })
  );
}

// 外部リクエスト（localhost / Vite HMR 以外）の追跡
function trackExternalRequests(page) {
  const external = [];
  page.on('request', req => {
    const url = req.url();
    if (
      url.startsWith('http://localhost') ||
      url.startsWith('https://localhost') ||
      url.startsWith('ws://localhost') ||
      url.startsWith('wss://localhost') ||
      url.startsWith('data:') ||
      url.startsWith('blob:')
    ) return;
    external.push(url);
  });
  return external;
}

test.describe('PWA screenshots — iPhone viewport', () => {
  test.use({ viewport: IPHONE_VIEWPORT });

  test('01. ペイン一覧（ホーム画面）', async ({ page }) => {
    const external = trackExternalRequests(page);
    await setupMocks(page);
    await page.goto(`${BASE}/#/`);
    await page.waitForSelector('.pane-card', { timeout: 10000 });
    await page.waitForTimeout(500);
    await page.screenshot({ path: `${EVIDENCE_DIR}/01-pane-list.png`, fullPage: false });
    expect(external).toEqual([]);
  });

  test('02. チャットビュー（claude）', async ({ page }) => {
    const external = trackExternalRequests(page);
    await setupMocks(page);
    await page.goto(`${BASE}/#/panes/1`);
    await page.waitForSelector('.chat-scroll', { timeout: 10000 });
    await page.waitForTimeout(800);
    await page.screenshot({ path: `${EVIDENCE_DIR}/02-chat-claude.png`, fullPage: false });
    expect(external).toEqual([]);
  });

  test('03. term ビュー（chat/term 切替後）', async ({ page }) => {
    const external = trackExternalRequests(page);
    await setupMocks(page);
    await page.goto(`${BASE}/#/panes/1`);
    await page.waitForSelector('.view-toggle', { timeout: 10000 });
    const termBtn = page.locator('.view-toggle-btn', { hasText: 'term' });
    await termBtn.click();
    await page.waitForSelector('.quick-keys', { timeout: 10000 });
    await page.waitForTimeout(500);
    await page.screenshot({ path: `${EVIDENCE_DIR}/03-term-view.png`, fullPage: false });
    expect(external).toEqual([]);
  });

  test('04. ペアリング承認待ち', async ({ page }) => {
    await setupPendingMocks(page);
    await page.goto(`${BASE}/`);
    await page.waitForSelector('.connect-card', { timeout: 10000 });
    await page.waitForTimeout(500);
    await page.screenshot({ path: `${EVIDENCE_DIR}/04-pairing-pending.png`, fullPage: false });
  });

  test('05. ペアリング拒否', async ({ page }) => {
    await setupDeniedMocks(page);
    await page.goto(`${BASE}/`);
    await page.waitForSelector('.connect-card', { timeout: 10000 });
    await page.waitForTimeout(500);
    await page.screenshot({ path: `${EVIDENCE_DIR}/05-pairing-denied.png`, fullPage: false });
  });

  test('06. codex ペイン', async ({ page }) => {
    await setupMocks(page);
    await page.goto(`${BASE}/#/panes/2`);
    await page.waitForTimeout(2000);
    await page.screenshot({ path: `${EVIDENCE_DIR}/06-codex-pane.png`, fullPage: false });
  });

  test('07. agy ペイン', async ({ page }) => {
    await setupMocks(page);
    await page.goto(`${BASE}/#/panes/3`);
    await page.waitForTimeout(2000);
    await page.screenshot({ path: `${EVIDENCE_DIR}/07-agy-pane.png`, fullPage: false });
  });

  test('08. 外部リクエスト 0 件（ネットワーク証拠）', async ({ page }) => {
    const external = trackExternalRequests(page);
    await setupMocks(page);
    await page.goto(`${BASE}/#/`);
    await page.waitForSelector('.pane-card', { timeout: 10000 });
    // ペイン一覧 → チャット → term を遷移
    await page.goto(`${BASE}/#/panes/1`);
    await page.waitForTimeout(2000);
    expect(external).toEqual([]);
  });

  test('09. chat/term 切替 + テキスト入力', async ({ page }) => {
    await setupMocks(page);
    await page.goto(`${BASE}/#/panes/1`);
    await page.waitForSelector('.composer-input', { timeout: 10000 });
    await page.fill('.composer-input', 'テスト入力');
    await page.waitForTimeout(300);
    await page.screenshot({ path: `${EVIDENCE_DIR}/09-chat-input.png`, fullPage: false });

    const termBtn = page.locator('.view-toggle-btn', { hasText: 'term' });
    await termBtn.click();
    await page.waitForSelector('.term-input-field', { timeout: 10000 });
    await page.fill('.term-input-field', 'ls -la');
    await page.waitForTimeout(300);
    await page.screenshot({ path: `${EVIDENCE_DIR}/09-term-input.png`, fullPage: false });
  });
});
