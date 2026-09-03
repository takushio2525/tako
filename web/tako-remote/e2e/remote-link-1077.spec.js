// Issue #1077（エピック #1059 柱 1-C）: PWA を「一覧 + Claude 公式へ送り出す」形にする。
//
// 検証するのは 3 点（Issue の受け入れ条件）:
//   ① connected のペインだけに「Claude で開く」が出て、daemon が返した URL をそのまま開く
//   ② not_connected / ineligible のペインでは**理由**と「PC 側で有効化する方法」が出る
//   ③ 自前チャットへの回帰が無い（既定 view は chat のまま・送信もできる）
//
// API はすべて `page.route` でモックするので daemon は不要。`remote_link` の中身は
// daemon（`claude_remote_link::RemoteLink::to_json_with_profile`）が実際に返す形・
// 実際の日本語文言に合わせている（PWA 側で文言を組み立てないことの裏取りにもなる）。
//
// 実行:
//   cd web/tako-remote && npx playwright test e2e/remote-link-1077.spec.js
import { test, expect } from '@playwright/test';

const IPHONE_VIEWPORT = { width: 390, height: 844 };
const BASE = `http://localhost:${process.env.TAKO_PWA_PORT || 5174}`;
const EVIDENCE_DIR = process.env.TAKO_EVIDENCE_DIR || `${process.env.HOME}/dev/tako-evidence/1077`;

const FAKE_ME = {
  registered: true,
  device_id: 'test-iphone',
  name: 'iPhone',
  role: 'interact',
  login: 'user@example.com',
  host: 'test-mac',
  version: '0.8.4',
  app_connected: true,
};

// 実 URL は使わない（リポにセッション id を残さない）。形だけ本物に合わせる
const LINK_URL = 'https://claude.ai/code/session_01TESTTESTTESTTESTTEST01';

// daemon が返す remote_link の 4 状態（Rust 側の文言をそのまま写している）
const LINK = {
  connected: {
    url: LINK_URL,
    session_id: 'session_01TESTTESTTESTTESTTEST01',
    account_label: 'univ',
    state: 'connected',
    reason: null,
    next_step: null,
    enable_command: null,
  },
  not_connected: {
    url: null,
    session_id: null,
    account_label: 'personal',
    state: 'not_connected',
    reason: 'この会話は Claude 公式の Remote Control に繋がっていません（tako の opt-in は既定 OFF です）',
    next_step: 'PC 側でプロファイルを opt-in してから master / worker を立て直してください（すでに動いている会話は後から繋げられません）',
    enable_command: 'tako orchestrator profiles set dev --remote-control true',
  },
  ineligible: {
    url: null,
    session_id: null,
    account_label: null,
    state: 'ineligible: disabled_by_policy',
    reason: '組織のポリシー（managed settings の disableRemoteControl）で Remote Control が無効化されている',
    next_step: '組織の管理者に Remote Control の許可を依頼する（tako 側では解除できない）',
    // 環境側の阻害はプロファイルの opt-in では直らないので案内コマンドを出さない
    enable_command: null,
  },
  unsupported: {
    url: null,
    session_id: null,
    account_label: null,
    state: 'ineligible: agent_unsupported',
    reason: 'この系統に Claude 公式の Remote Control に相当する仕組みが無い（claude 専用）',
    next_step: '会話をスマホから見たい場合は master / worker を claude で起動する（`tako agent-support --agent <系統>` で差を確認できる）',
    enable_command: null,
  },
};

const PANES = {
  api_version: 2,
  panes: [
    {
      id: 4, title: 'master', role: 'orchestrator-master:dev', agent_type: 'claude',
      cwd: '/Users/dev/tako', state: 'running', surface: 'foreground',
      position: '1/3', tab_id: 1, tab_title: 'tako', cols: 120, rows: 40,
      focused: true, session_id: 'master-session', model: 'opus 5',
      tmux_target: 'tako-master:0.0', activity: 'idle',
      preview: ['⏺ worker を立てました。'],
      remote_link: LINK.connected,
    },
    {
      id: 12, title: 'fix-auth', role: 'orchestrator-worker-claude:fix-auth',
      agent_type: 'claude', cwd: '/Users/dev/tako', state: 'running',
      surface: 'foreground', position: '2/3', tab_id: 1, tab_title: 'tako',
      cols: 120, rows: 40, focused: false, session_id: 'w-1', model: 'opus 5',
      tmux_target: 'tako-w1:0.0', activity: 'busy',
      preview: ['⏺ tests/auth.test.ts を読んでいます'],
      remote_link: LINK.not_connected,
    },
    {
      id: 13, title: 'docs-site', role: 'orchestrator-worker-codex',
      agent_type: 'codex', cwd: '/Users/dev/tako/docs', state: 'running',
      surface: 'foreground', position: '3/3', tab_id: 1, tab_title: 'tako',
      cols: 120, rows: 40, focused: false, session_id: 'w-2', model: 'gpt-5.6',
      tmux_target: 'tako-w2:0.0', activity: 'idle',
      preview: ['⏺ docs をビルドしました'],
      remote_link: LINK.unsupported,
    },
    {
      id: 14, title: 'policy-master', role: 'orchestrator-master', agent_type: 'claude',
      cwd: '/Users/dev/other', state: 'running', surface: 'foreground',
      position: '1/1', tab_id: 2, tab_title: 'other', cols: 120, rows: 40,
      focused: false, session_id: 'm-2', model: 'opus 5',
      tmux_target: 'tako-m2:0.0', activity: 'idle',
      preview: ['⏺ 待機中'],
      remote_link: LINK.ineligible,
    },
    {
      // 素のシェル: daemon は remote_link を付けない（会話が無いので理由も出さない）
      id: 21, title: 'zsh', role: '', agent_type: 'plain',
      cwd: '/Users/dev/dotfiles', state: 'idle', surface: 'foreground',
      position: '1/1', tab_id: 3, tab_title: 'dotfiles', cols: 100, rows: 30,
      focused: false, tmux_target: 'tako-s1:0.0', preview: ['$ '],
    },
  ],
};

function json(route, body) {
  return route.fulfill({
    status: 200,
    contentType: 'application/json',
    body: JSON.stringify(body),
  });
}

async function setupMocks(page, { panes = PANES } = {}) {
  await page.route('**/api/me', route => json(route, FAKE_ME));
  await page.route('**/api/v2/panes', route => json(route, panes));
  await page.route('**/api/panes/*/screen*', route =>
    json(route, { lines: ['$ '], cursor: { x: 2, y: 0 }, size: { cols: 120, rows: 40 } })
  );
  await page.route('**/api/sessions/*/messages*', route =>
    json(route, {
      session_id: 'x',
      messages: [
        { role: 'user', text: 'テストを流して', timestamp: '2026-09-03T01:00:00Z' },
        { role: 'assistant', text: 'テストを実行します。', timestamp: '2026-09-03T01:00:05Z' },
      ],
    })
  );
  await page.route('**/api/agents', route => json(route, { agents: [] }));
  await page.route('**/api/health', route => json(route, { status: 'ok', version: '0.8.4' }));
  await page.route('**/ws?*', route => route.abort());
  await page.route('**/manifest.json', route => json(route, { name: 'tako remote' }));
  await page.route('**/sw.js', route =>
    route.fulfill({ status: 200, contentType: 'application/javascript', body: '' })
  );
}

async function gotoPanes(page) {
  await page.goto(`${BASE}/#/`);
  await page.waitForSelector('.pane-card', { timeout: 10000 });
  await page.waitForTimeout(400);
}

// カードは pane id で特定する（`master` はタイトルが `policy-master` にも部分一致する）
const CARD_ID = {
  master: 4,
  'fix-auth': 12,
  'docs-site': 13,
  'policy-master': 14,
  zsh: 21,
};
function cardByTitle(page, title) {
  return page.locator(`.pane-card[data-pane-id="${CARD_ID[title]}"]`);
}

test.describe('#1077 Claude 公式へ送り出す — モバイル', () => {
  test.use({ viewport: IPHONE_VIEWPORT });

  test('01. connected のペインにだけ「Claude で開く」が出る', async ({ page }) => {
    await setupMocks(page);
    await gotoPanes(page);

    // 一覧全体で 1 個だけ（connected は master だけ）
    expect(await page.locator('.claude-open').count()).toBe(1);

    const link = cardByTitle(page, 'master').locator('.claude-open');
    await expect(link).toBeVisible();
    // daemon の URL をそのまま開く（PWA が id から組み立てない）
    await expect(link).toHaveAttribute('href', LINK_URL);
    // 新しいタブ / Claude アプリで開く
    await expect(link).toHaveAttribute('target', '_blank');
    await expect(link).toHaveAttribute('rel', /noopener/);

    // 未接続・不適格・素のシェルには出ない
    for (const title of ['fix-auth', 'docs-site', 'policy-master', 'zsh']) {
      expect(await cardByTitle(page, title).locator('.claude-open').count()).toBe(0);
    }
    await page.screenshot({ path: `${EVIDENCE_DIR}/01-list.png` });
  });

  test('02. 「Claude で開く」はカードを開かず外部リンクだけを踏む', async ({ page }) => {
    await setupMocks(page);
    await gotoPanes(page);

    // 実際に別タブへ遷移させると claude.ai を叩くので、遷移だけ止めて
    // 「カードの onClick へ伝播していない」= ハッシュが変わらないことを見る
    await page.locator('.claude-open').evaluate(el => el.removeAttribute('href'));
    await page.locator('.claude-open').click();
    await page.waitForTimeout(300);
    expect(await page.evaluate(() => window.location.hash)).toBe('#/');
  });

  test('03. 未接続のペインは理由と有効化コマンドを出す', async ({ page }) => {
    await setupMocks(page);
    await gotoPanes(page);

    const card = cardByTitle(page, 'fix-auth');
    const toggle = card.locator('.remote-link-toggle');
    await expect(toggle).toBeVisible();
    await expect(toggle).toContainText('未接続');
    // 一覧は既定でたたむ（カードが縦に伸びて識別性を落とさない）
    expect(await card.locator('.remote-link-reason').count()).toBe(0);

    await toggle.click();
    const reason = card.locator('.remote-link-reason');
    await expect(reason).toBeVisible();
    await expect(reason).toContainText('Remote Control に繋がっていません');
    await expect(reason).toContainText('PC 側でプロファイルを opt-in');
    // PC 側で有効化する方法（押せるボタンではなくコマンドの提示）
    await expect(reason.locator('code')).toHaveText(
      'tako orchestrator profiles set dev --remote-control true'
    );
    await page.screenshot({ path: `${EVIDENCE_DIR}/03-not-connected.png` });
  });

  test('04. 環境側の阻害では opt-in コマンドを出さない（押しても直らないものを勧めない）', async ({ page }) => {
    await setupMocks(page);
    await gotoPanes(page);

    const card = cardByTitle(page, 'policy-master');
    await card.locator('.remote-link-toggle').click();
    const reason = card.locator('.remote-link-reason');
    await expect(reason).toContainText('組織のポリシー');
    await expect(reason).toContainText('管理者に Remote Control の許可を依頼');
    expect(await reason.locator('code').count()).toBe(0);

    // codex は上流に手段が無い = 系統の差として理由が出る
    const codex = cardByTitle(page, 'docs-site');
    await codex.locator('.remote-link-toggle').click();
    await expect(codex.locator('.remote-link-reason')).toContainText('claude 専用');
    expect(await codex.locator('.remote-link-reason code').count()).toBe(0);
    await page.screenshot({ path: `${EVIDENCE_DIR}/04-ineligible.png` });
  });

  test('05. アカウント表示が一覧に出る（別アカウントで出ない問題を切り分けられる）', async ({ page }) => {
    await setupMocks(page);
    await gotoPanes(page);

    await expect(cardByTitle(page, 'master').locator('.card-chip-account')).toHaveText('univ');
    await expect(cardByTitle(page, 'fix-auth').locator('.card-chip-account')).toHaveText('personal');
    // アカウントが分からないペインには出さない（嘘のラベルを出さない）
    expect(await cardByTitle(page, 'policy-master').locator('.card-chip-account').count()).toBe(0);
  });

  test('06. 素のシェルには Remote Control の話を出さない', async ({ page }) => {
    await setupMocks(page);
    await gotoPanes(page);
    const card = cardByTitle(page, 'zsh');
    expect(await card.locator('.remote-link-row').count()).toBe(0);
    expect(await card.locator('.claude-open').count()).toBe(0);
  });

  test('07. 自前チャットへの回帰が無い（既定 view は chat・公式リンクは併記）', async ({ page }) => {
    await setupMocks(page);
    await gotoPanes(page);

    await cardByTitle(page, 'master').click();
    await page.waitForSelector('.pane-header', { timeout: 10000 });
    await page.waitForTimeout(500);

    // 既定は自前チャット（フォールバックを残す = 設計の確定判断）
    await expect(page.locator('.view-toggle-btn.chat-active')).toBeVisible();
    await expect(page.locator('.chat-scroll')).toBeVisible();
    // 会話本文が読める（transcript API 経路が生きている）
    await expect(page.locator('.page.terminal-page')).toContainText('テストを実行します');
    // 同じ画面から公式へも行ける
    await expect(page.locator('.claude-open')).toHaveAttribute('href', LINK_URL);
    await page.screenshot({ path: `${EVIDENCE_DIR}/07-pane-chat.png` });

    // term ビューへの切替も従来どおり
    await page.locator('.view-toggle-btn', { hasText: 'term' }).click();
    await page.waitForTimeout(300);
    await expect(page.locator('.reader')).toBeVisible();
  });

  test('08. 未接続ペインを開いても理由が読める', async ({ page }) => {
    await setupMocks(page);
    await gotoPanes(page);
    await cardByTitle(page, 'fix-auth').click();
    await page.waitForSelector('.pane-header', { timeout: 10000 });
    await page.locator('.remote-link-toggle').click();
    await expect(page.locator('.remote-link-reason')).toContainText('opt-in は既定 OFF');
  });
});
