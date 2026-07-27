// Issue #621: ペイン選択画面が「どれがどれだか分かる」かのモバイル検証。
//
// master + worker 複数（claude / codex / agy）+ 素のシェル + ターミナル無しペインを
// 混在させ、カードが区別できることをスクショと DOM で確かめる。
// API はすべて page.route でモックするので daemon は不要。
//
// 実行:
//   cd web/tako-remote && npx playwright test e2e/panes-621.spec.js
//   TAKO_SHOT_PREFIX=before npx playwright test e2e/panes-621.spec.js  # 改修前の記録用
import { test, expect } from '@playwright/test';

const EVIDENCE_DIR = process.env.TAKO_EVIDENCE_DIR || `${process.env.HOME}/dev/tako-evidence/621`;
const PREFIX = process.env.TAKO_SHOT_PREFIX || 'after';
const IPHONE_VIEWPORT = { width: 390, height: 844 };
const BASE = `http://localhost:${process.env.TAKO_PWA_PORT || 5174}`;

const FAKE_ME = {
  registered: true,
  device_id: 'test-iphone',
  name: 'iPhone',
  role: 'interact',
  login: 'user@example.com',
  host: 'test-mac',
  version: '0.6.0',
  app_connected: true,
};

// daemon が返す生の画面（`GET /api/panes/:id/screen?lines=12` = スクロールバック 12 行 +
// 現画面。エージェント TUI なら末尾は必ず入力欄 + フッター）。
// 改修前の PWA はこれをそのままカードに流し込んでいたので、
// どのカードも「古い履歴の先頭 12 行」で埋まり識別に寄与しなかった
const RAW_SCREEN = {
  'tako-master:0.0': [
    ...Array.from({ length: 12 }, (_, i) => `  ⎿  読み込み中… (${i + 1}/12)`),
    '⏺ worker を 3 体立てました。fix-auth / docs-site / win-port です。',
    '  完了報告が来たら検収して結果をまとめます。',
    '',
    '✻ Cogitated for 1m 2s',
    '──────────────────────────────────────────────',
    '❯ ',
    '──────────────────────────────────────────────',
    '  [Opus 5 · MAX]  🔧 master',
    '  ctx  31% ███░░░░░░░',
    '  5h   20% ██░░░░░░░░ (→2h33m)',
    '  ⏵⏵ auto mode on (shift+tab to cycle) · ← for agents',
  ],
  'tako-w1:0.0': [
    ...Array.from({ length: 12 }, (_, i) => `  ⎿  npm test 出力 ${i + 1} 行目`),
    '⏺ tests/auth.test.ts を読んでいます',
    '',
    '⏺ Searching for 5 patterns, reading 2 files, running 3 shell commands…',
    '',
    '✽ Misting… (2m 14s · ↓ 8.4k tokens)',
    '',
    '──────────────────────────────────────────────',
    '❯ ',
    '──────────────────────────────────────────────',
    '  [Opus 5 · MAX]  🔧 worker: tako:12',
    '  ctx  23% ██░░░░░░░░',
    '  ⏵⏵ auto mode on (shift+tab to cycle)',
  ],
  'tako-w2:0.0': [
    ...Array.from({ length: 12 }, (_, i) => `  ⎿  docs ビルドログ ${i + 1}`),
    '⏺ ビルド成果物を消してから再ビルドします。',
    '',
    '╭────────────────────────────────────╮',
    '│ Bash command                       │',
    '│ rm -rf build                       │',
    '│ Do you want to proceed?            │',
    '│ ❯ 1. Yes                           │',
    '│   2. No                            │',
    '╰────────────────────────────────────╯',
  ],
  'tako-w3:0.0': [
    ...Array.from({ length: 12 }, (_, i) => `  ⎿  cargo check 出力 ${i + 1}`),
    '⏺ Windows のパス正規化を直しています',
    '',
    'Claude usage limit reached. Your limit will reset at 3am.',
    '──────────────────────────────────────────────',
    '❯ ',
    '──────────────────────────────────────────────',
    '  [Opus 5 · MAX]  🔧 worker: tako:14',
  ],
  'tako-s1:0.0': [
    ...Array.from({ length: 12 }, (_, i) => `  依存解決ログ ${i + 1}`),
    '$ npm run dev',
    '',
    '> vite dev',
    '',
    'VITE v6.0.0  ready in 320 ms',
    '➜  Local:   http://localhost:5173/',
  ],
  'tako-s2:0.0': [
    ...Array.from({ length: 12 }, (_, i) => `  過去のコマンド履歴 ${i + 1}`),
    '$ git status -sb',
    '## main...origin/main',
    ' M web/tako-remote/src/pages/panes.jsx',
    '$',
  ],
};

// daemon 側 `remote_preview::summarize` が上の生画面から作るスニペット（#621）。
// TUI のクロムが落ち、末尾 8 行に収まる
const PREVIEW = {
  master: [
    '⏺ worker を 3 体立てました。fix-auth / docs-site / win-port です。',
    '  完了報告が来たら検収して結果をまとめます。',
    '',
    '✻ Cogitated for 1m 2s',
  ],
  worker_busy: [
    '⏺ tests/auth.test.ts を読んでいます',
    '',
    '⏺ Searching for 5 patterns, reading 2 files, running 3 shell commands…',
    '',
    '✽ Misting… (2m 14s · ↓ 8.4k tokens)',
  ],
  worker_permission: [
    '⏺ ビルド成果物を消してから再ビルドします。',
    '',
    '╭────────────────────────────────────╮',
    '│ Bash command                       │',
    '│ rm -rf build                       │',
    '│ Do you want to proceed?            │',
    '│ ❯ 1. Yes                           │',
    '│   2. No                            │',
  ],
  worker_error: [
    '⏺ Windows のパス正規化を直しています',
    '',
    'Claude usage limit reached. Your limit will reset at 3am.',
  ],
  shell_running: [
    '$ npm run dev',
    '',
    '> vite dev',
    '',
    'VITE v6.0.0  ready in 320 ms',
    '➜  Local:   http://localhost:5173/',
  ],
  shell_idle: [
    '$ git status -sb',
    '## main...origin/main',
    ' M web/tako-remote/src/pages/panes.jsx',
    '$',
  ],
};

const FAKE_PANES = {
  api_version: 2,
  panes: [
    {
      id: 4, title: 'master', role: 'master', agent_type: 'claude',
      cwd: '/Users/dev/tako', state: 'running', surface: 'foreground',
      position: '1/4', tab_id: 1, tab_title: 'tako', cols: 120, rows: 40,
      focused: true, session_id: 'master-session', model: 'opus 5',
      tmux_target: 'tako-master:0.0',
      activity: 'idle', preview: PREVIEW.master,
    },
    {
      id: 12, title: 'fix-auth', role: 'orchestrator-worker-claude',
      agent_type: 'claude', cwd: '/Users/dev/tako', state: 'running',
      surface: 'foreground', position: '2/4', tab_id: 1, tab_title: 'tako',
      cols: 120, rows: 40, focused: false, session_id: 'w-1', model: 'opus 5',
      tmux_target: 'tako-w1:0.0',
      activity: 'busy', preview: PREVIEW.worker_busy,
    },
    {
      id: 13, title: 'docs-site', role: 'orchestrator-worker-codex',
      agent_type: 'codex', cwd: '/Users/dev/tako/docs', state: 'running',
      surface: 'foreground', position: '3/4', tab_id: 1, tab_title: 'tako',
      cols: 120, rows: 40, focused: false, model: 'gpt-5.6', session_id: 'w-2',
      tmux_target: 'tako-w2:0.0',
      activity: 'permission', preview: PREVIEW.worker_permission,
      permission_dialog: {
        command: 'rm -rf build',
        options: ['1. Yes', "2. Yes, and don't ask again", '3. No'],
        highlighted: 0,
      },
    },
    {
      id: 14, title: 'win-port', role: 'orchestrator-worker-agy',
      agent_type: 'agy', cwd: '/Users/dev/tako', state: 'running',
      surface: 'background', position: '4/4', tab_id: 1, tab_title: 'tako',
      cols: 120, rows: 40, focused: false,
      tmux_target: 'tako-w3:0.0',
      activity: 'error', preview: PREVIEW.worker_error,
      error: { kind: 'usage_limit', detail: 'Claude usage limit reached. Your limit will reset at 3am.', recommended_action: 'wait_reset' },
    },
    {
      id: 21, title: 'zsh', role: '', agent_type: 'plain',
      cwd: '/Users/dev/dotfiles', state: 'running', surface: 'foreground',
      position: '1/2', tab_id: 2, tab_title: 'dotfiles', cols: 100, rows: 30,
      focused: false, tmux_target: 'tako-s1:0.0', preview: PREVIEW.shell_running,
    },
    {
      id: 22, title: 'zsh', role: '', agent_type: 'plain',
      cwd: '/Users/dev/dotfiles', state: 'idle', surface: 'foreground',
      position: '2/2', tab_id: 2, tab_title: 'dotfiles', cols: 100, rows: 30,
      focused: false, tmux_target: 'tako-s2:0.0', preview: PREVIEW.shell_idle,
    },
    {
      id: 31, title: 'README.md', role: '', agent_type: 'plain',
      cwd: '/Users/dev/tako', state: 'unknown', surface: 'foreground',
      position: '1/1', tab_id: 3, tab_title: 'docs', cols: 0, rows: 0,
      focused: false, tmux_target: null,
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

async function setupMocks(page, { panes = FAKE_PANES, screenFails = false } = {}) {
  await page.route('**/api/me', route => json(route, FAKE_ME));
  await page.route('**/api/v2/panes', route => json(route, panes));
  await page.route('**/api/panes/*/screen*', route => {
    if (screenFails) {
      return route.fulfill({ status: 404, contentType: 'application/json', body: '{"error":"can\'t find pane"}' });
    }
    const target = decodeURIComponent(route.request().url().match(/\/api\/panes\/([^/]+)\/screen/)[1]);
    const lines = RAW_SCREEN[target] || [];
    return json(route, { lines, cursor: { x: 0, y: lines.length }, size: { cols: 120, rows: 40 } });
  });
  await page.route('**/api/sessions/*/messages*', route =>
    json(route, {
      session_id: 'x',
      messages: [
        { role: 'user', text: 'docs をビルドし直して', timestamp: '2026-07-27T22:41:00Z' },
        { role: 'assistant', text: 'ビルド成果物を消してから再ビルドします。', timestamp: '2026-07-27T22:42:00Z' },
      ],
    })
  );
  await page.route('**/api/agents', route => json(route, { agents: [] }));
  await page.route('**/api/health', route => json(route, { status: 'ok', version: '0.6.0' }));
  await page.route('**/ws?*', route => route.abort());
  await page.route('**/manifest.json', route => json(route, { name: 'tako remote' }));
  await page.route('**/sw.js', route =>
    route.fulfill({ status: 200, contentType: 'application/javascript', body: '' })
  );
}

async function gotoPanes(page) {
  await page.goto(`${BASE}/#/`);
  await page.waitForSelector('.pane-card', { timeout: 10000 });
  await page.waitForTimeout(600);
}

// カードはタイトルで特定する（スニペット本文に別ペイン名が出ることがあるので
// カード全体の hasText では一意にならない）
function cardByTitle(page, title) {
  return page.locator('.pane-card', {
    has: page.locator('.pane-card-title', { hasText: title }),
  });
}

test.describe('#621 ペイン選択画面 — モバイル', () => {
  test.use({ viewport: IPHONE_VIEWPORT });

  test('01. 混在構成の一覧（全体）', async ({ page }) => {
    await setupMocks(page);
    await gotoPanes(page);
    await page.screenshot({ path: `${EVIDENCE_DIR}/${PREFIX}-01-list-viewport.png` });
    // 一覧は `.card-list` 内スクロールなので fullPage が効かない。
    // 縦長ビューポートにして全カードを 1 枚に収める
    await page.setViewportSize({ width: 390, height: 2000 });
    await page.waitForTimeout(400);
    await page.screenshot({ path: `${EVIDENCE_DIR}/${PREFIX}-01-list-full.png` });
    await page.setViewportSize(IPHONE_VIEWPORT);
    await page.waitForTimeout(200);

    // 7 ペインすべてがカードとして出る
    await expect(page.locator('.pane-card')).toHaveCount(7);
  });

  test('02. タブごとにグループ化されている', async ({ page }) => {
    await setupMocks(page);
    await gotoPanes(page);
    const headers = page.locator('.tab-group-header');
    await expect(headers).toHaveCount(3);
    await expect(headers.nth(0)).toContainText('tako');
    await expect(headers.nth(1)).toContainText('dotfiles');
    await expect(headers.nth(2)).toContainText('docs');
  });

  test('03. 種別・状態がカードで区別できる', async ({ page }) => {
    await setupMocks(page);
    await gotoPanes(page);

    // 役割の区別
    await expect(page.locator('.pane-card.role-master')).toHaveCount(1);
    await expect(page.locator('.pane-card.role-worker')).toHaveCount(3);
    await expect(page.locator('.pane-card.role-user')).toHaveCount(3);

    // 状態の区別（activity 由来。素のシェルは OSC 133 の state 由来）
    await expect(cardByTitle(page, 'master')).toHaveClass(/state-idle/);
    await expect(cardByTitle(page, 'fix-auth')).toHaveClass(/state-busy/);
    await expect(cardByTitle(page, 'docs-site')).toHaveClass(/state-permission/);
    await expect(cardByTitle(page, 'win-port')).toHaveClass(/state-error/);
    await expect(cardByTitle(page, 'README.md')).toHaveClass(/state-idle/);
    // 状態ラベルが日本語で読める
    await expect(cardByTitle(page, 'docs-site').locator('.status-pill')).toContainText('承認待ち');
    await expect(cardByTitle(page, 'fix-auth').locator('.status-pill')).toContainText('実行中');
    await expect(cardByTitle(page, 'win-port').locator('.status-pill')).toContainText('停止');

    // エージェント種別のラベル
    const codexCard = cardByTitle(page, 'docs-site');
    await expect(codexCard.locator('.card-chip-agent')).toContainText('codex');
  });

  test('04. 中身のスニペットがカードに出る', async ({ page }) => {
    await setupMocks(page);
    await gotoPanes(page);

    const busy = cardByTitle(page, 'fix-auth');
    await expect(busy.locator('.pane-card-preview-box')).toContainText('Misting');
    await expect(busy.locator('.pane-card-preview-box')).toContainText('tests/auth.test.ts');

    const master = cardByTitle(page, 'master');
    await expect(master.locator('.pane-card-preview-box')).toContainText('worker を 3 体立てました');

    const shell = cardByTitle(page, 'zsh').first();
    await expect(shell.locator('.pane-card-preview-box')).toContainText('VITE v6.0.0');
  });

  test('05. 承認待ちは何を聞かれているかまで見える', async ({ page }) => {
    await setupMocks(page);
    await gotoPanes(page);
    const card = cardByTitle(page, 'docs-site');
    await expect(card.locator('.card-permission')).toContainText('rm -rf build');
    await card.scrollIntoViewIfNeeded();
    await page.screenshot({ path: `${EVIDENCE_DIR}/${PREFIX}-05-permission-card.png` });
  });

  test('06. エラーは種別と対処が見える', async ({ page }) => {
    await setupMocks(page);
    await gotoPanes(page);
    const card = cardByTitle(page, 'win-port');
    await expect(card.locator('.card-error')).toContainText('usage limit');
  });

  test('07. 要対応フィルタで絞り込める', async ({ page }) => {
    await setupMocks(page);
    await gotoPanes(page);
    await page.locator('.filter-chip', { hasText: '要対応' }).click();
    await page.waitForTimeout(300);
    // permission + error の 2 件だけが残る
    await expect(page.locator('.pane-card')).toHaveCount(2);
    await page.screenshot({ path: `${EVIDENCE_DIR}/${PREFIX}-07-filter-needs-you.png` });
  });

  test('08. カードから会話画面へ遷移できる（チャット非回帰）', async ({ page }) => {
    await setupMocks(page);
    await gotoPanes(page);
    await cardByTitle(page, 'fix-auth').click();
    await page.waitForSelector('.pane-header', { timeout: 10000 });
    expect(page.url()).toContain('#/panes/12');
  });

  test('09-edge. ペイン 0 件', async ({ page }) => {
    await setupMocks(page, { panes: { api_version: 2, panes: [] } });
    await page.goto(`${BASE}/#/`);
    await page.waitForSelector('.empty-state', { timeout: 10000 });
    await page.waitForTimeout(400);
    await page.screenshot({ path: `${EVIDENCE_DIR}/${PREFIX}-09-empty.png` });
    await expect(page.locator('.empty-state')).toBeVisible();
  });

  test('10-edge. スニペットが取れないペイン', async ({ page }) => {
    // daemon がキャプチャに失敗すると preview フィールドごと落ちてくる。
    // PWA は screen API へフォールバックし、そこも失敗したら明示表示にする
    const noPreview = {
      api_version: 2,
      panes: FAKE_PANES.panes.map(p => {
        const { preview, ...rest } = p;
        return rest;
      }),
    };
    await setupMocks(page, { panes: noPreview, screenFails: true });
    await gotoPanes(page);
    await page.waitForTimeout(1200);
    await page.screenshot({ path: `${EVIDENCE_DIR}/${PREFIX}-10-preview-unavailable.png`, fullPage: true });
    await expect(page.locator('.preview-unavailable').first()).toBeVisible();
  });

  test('11. 承認待ちペインのチャットで承認カードが出て respond に届く', async ({ page }) => {
    // #425 の現契約: カードの選択肢は実ダイアログそのもの。押すと
    // POST /api/panes/:id/respond に 1 始まりの選択肢番号が飛ぶ
    const responds = [];
    await setupMocks(page);
    await page.route('**/api/panes/*/respond', async route => {
      responds.push(route.request().postDataJSON());
      await route.fulfill({ status: 200, contentType: 'application/json', body: '{"ok":true}' });
    });
    await page.goto(`${BASE}/#/panes/13`);
    await page.waitForSelector('.approval-card', { timeout: 10000 });
    await expect(page.locator('.approval-card-body')).toContainText('rm -rf build');
    await page.screenshot({ path: `${EVIDENCE_DIR}/${PREFIX}-11-chat-approval.png` });

    await page.locator('.approval-btn-allow').first().click();
    await page.waitForTimeout(400);
    expect(responds.length).toBeGreaterThan(0);
    expect(responds[responds.length - 1].choice).toBe('1');
  });
});
