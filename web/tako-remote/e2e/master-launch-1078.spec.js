// Issue #1078（エピック #1059 柱 1-D）: スマホから「新しいタブ + master 起動」。
//
// 検証するのは 4 点（Issue の受け入れ条件）:
//   ① 1 操作でタブ + master が立つ（`POST /api/tabs` → `POST /api/tabs/:id/master`）
//   ② opt-in していないプロファイルでは公式リンクが出ず理由が出る
//   ③ observe / interact role では 403（押す前に理由・押しても理由）
//   ④ 作られたタブが一覧に出る（Mac 画面側は daemon の `TabNew { focus: true }` が担保）
//
// API はすべて `page.route` でモックする。ボディも検証するので
// 「PWA が daemon の何をどの順で叩くか」まで固定される。
//
// 実行:
//   cd web/tako-remote && npx playwright test e2e/master-launch-1078.spec.js
import { test, expect } from '@playwright/test';

const IPHONE_VIEWPORT = { width: 390, height: 844 };
const BASE = `http://localhost:${process.env.TAKO_PWA_PORT || 5174}`;
const EVIDENCE_DIR = process.env.TAKO_EVIDENCE_DIR || `${process.env.HOME}/dev/tako-evidence/1078`;

const LINK_URL = 'https://claude.ai/code/session_01TESTTESTTESTTESTTEST02';

function me(role = 'manage') {
  return {
    registered: true,
    device_id: 'test-iphone',
    name: 'iPhone',
    role,
    login: 'user@example.com',
    host: 'test-mac',
    version: '0.8.4',
    app_connected: true,
  };
}

// `tako orchestrator profiles list` が返す形（daemon はこれをそのまま流す）
const PROFILES = {
  kind: 'master',
  profiles: [
    {
      name: 'default', kind: 'master', model: null, effort: 'high',
      remote_control: false, remote_control_effective: false,
    },
    {
      name: 'dev', kind: 'master', model: 'opus 5', effort: 'high',
      cwd: '/Users/dev/tako', projects: ['tako'],
      remote_control: true, remote_control_effective: true,
    },
    {
      name: 'org', kind: 'master', model: 'opus 5', effort: 'high',
      remote_control: true, remote_control_effective: false,
      remote_control_blocked: {
        kind: 'disabled_by_policy',
        detail: 'managed-settings.json',
        reason: '組織のポリシー（managed settings の disableRemoteControl）で Remote Control が無効化されている',
        next_step: '組織の管理者に Remote Control の許可を依頼する（tako 側では解除できない）',
      },
    },
  ],
};

// 一覧の初期状態（master を立てる前）
const PANE_SHELL = {
  id: 21, title: 'zsh', role: '', agent_type: 'plain',
  cwd: '/Users/dev', state: 'idle', surface: 'foreground',
  position: '1/1', tab_id: 3, tab_title: 'dotfiles', cols: 100, rows: 30,
  focused: false, tmux_target: 'tako-s1:0.0', preview: ['$ '],
};

// 起動した master ペイン。`remote_link` は待っているあいだ not_connected で、
// bridge が繋がると connected へ変わる（daemon の transcript 読みが実際にやること）
function masterPane(link) {
  return {
    id: 90, title: 'master-dev', role: 'orchestrator-master:dev', agent_type: 'claude',
    cwd: '/Users/dev/tako', state: 'running', surface: 'foreground',
    position: '1/1', tab_id: 9, tab_title: 'master-dev', cols: 120, rows: 40,
    focused: true, session_id: 'm-new', model: 'opus 5',
    tmux_target: 'tako-m-new:0.0', activity: 'busy', preview: ['起動中…'],
    remote_link: link,
  };
}

const LINK_WAITING = {
  url: null, session_id: null, account_label: 'univ', state: 'not_connected',
  reason: 'この会話は Claude 公式の Remote Control に繋がっていません（tako の opt-in は既定 OFF です）',
  next_step: 'PC 側でプロファイルを opt-in してから master / worker を立て直してください（すでに動いている会話は後から繋げられません）',
  enable_command: 'tako orchestrator profiles set dev --remote-control true',
};
const LINK_CONNECTED = {
  url: LINK_URL, session_id: 'session_01TESTTESTTESTTESTTEST02',
  account_label: 'univ', state: 'connected',
  reason: null, next_step: null, enable_command: null,
};

function json(route, body, status = 200) {
  return route.fulfill({
    status,
    contentType: 'application/json',
    body: JSON.stringify(body),
  });
}

/// 起動から `opts.connectAfterMs` 経つと `remote_link` が connected になる
/// （bridge 接続の再現。**一覧のポーリング回数ではなく経過時間**で切り替えるので、
/// 「待っている状態」が必ず 1 回は観測される）。
/// `opts.launchStatus` を 403 にすると role 不足のサーバー応答になる
async function setupMocks(page, opts = {}) {
  const {
    role = 'manage',
    connectAfterMs = 2500,
    launchStatus = 200,
    profileState = 'enabled',
    calls = {},
  } = opts;
  calls.panes = 0;
  calls.tabs = [];
  calls.master = [];
  calls.launchedAt = null;

  await page.route('**/api/me', route => json(route, me(role)));
  await page.route('**/api/master/profiles', route => json(route, PROFILES));
  await page.route('**/api/tabs', route => {
    calls.tabs.push(JSON.parse(route.request().postData() || '{}'));
    return json(route, { tab: 9, pane: 90, cwd: '/Users/dev/tako' });
  });
  await page.route('**/api/tabs/*/master', route => {
    calls.master.push({
      url: route.request().url(),
      body: JSON.parse(route.request().postData() || '{}'),
    });
    calls.launchedAt = Date.now();
    if (launchStatus !== 200) {
      return json(route, { error: '権限が足りません（manage 以上が必要）' }, launchStatus);
    }
    return json(route, {
      ok: true, tab: 9, pane: 90, profile: 'dev', tab_title: 'master-dev',
      role: 'orchestrator-master:dev', agent: 'claude', model: 'opus 5',
      effort: 'high', cwd: '/Users/dev/tako',
      remote_control: profileState === 'enabled'
        ? { state: 'enabled', opt_in: true, reason: null, next_step: null, enable_command: null }
        : {
            state: 'off', opt_in: false,
            reason: LINK_WAITING.reason,
            next_step: LINK_WAITING.next_step,
            enable_command: 'tako orchestrator profiles set default --remote-control true',
          },
    });
  });
  await page.route('**/api/v2/panes', route => {
    calls.panes += 1;
    // 起動前は素のシェルだけ。起動後は master ペインが増える（= 一覧に出る）
    const launched = calls.master.length > 0 && launchStatus === 200;
    if (!launched) return json(route, { api_version: 2, panes: [PANE_SHELL] });
    const connected = calls.launchedAt !== null
      && Date.now() - calls.launchedAt >= connectAfterMs;
    const link = connected ? LINK_CONNECTED : LINK_WAITING;
    return json(route, { api_version: 2, panes: [PANE_SHELL, masterPane(link)] });
  });
  await page.route('**/api/panes/*/screen*', route =>
    json(route, { lines: ['$ '], cursor: { x: 2, y: 0 }, size: { cols: 120, rows: 40 } })
  );
  await page.route('**/api/sessions/*/messages*', route =>
    json(route, { session_id: 'x', messages: [] })
  );
  await page.route('**/api/agents', route => json(route, { agents: [] }));
  await page.route('**/api/health', route => json(route, { status: 'ok', version: '0.8.4' }));
  await page.route('**/ws?*', route => route.abort());
  await page.route('**/manifest.json', route => json(route, { name: 'tako remote' }));
  await page.route('**/sw.js', route =>
    route.fulfill({ status: 200, contentType: 'application/javascript', body: '' })
  );
  return calls;
}

async function openLauncher(page) {
  await page.goto(`${BASE}/#/`);
  await page.waitForSelector('.pane-card', { timeout: 10000 });
  await page.locator('.launch-btn').click();
  await page.waitForSelector('.sheet', { timeout: 5000 });
}

test.describe('#1078 スマホから master を起動 — モバイル', () => {
  test.use({ viewport: IPHONE_VIEWPORT });

  test('01. プロファイルが Remote Control の状態つきで並ぶ', async ({ page }) => {
    await setupMocks(page);
    await openLauncher(page);

    const rows = page.locator('.launch-profile');
    await expect(rows).toHaveCount(3);
    // opt-in していない / している / している が環境で不可 の 3 状態が見分けられる
    await expect(rows.nth(0)).toContainText('default');
    await expect(rows.nth(0).locator('.launch-rc-badge')).toHaveText('Remote Control OFF');
    await expect(rows.nth(1)).toContainText('dev');
    await expect(rows.nth(1).locator('.launch-rc-badge')).toHaveText('Remote Control ON');
    await expect(rows.nth(2).locator('.launch-rc-badge')).toHaveText('Remote Control 不可');
    // 起動フォルダと担当プロジェクトも手がかりとして出る
    await expect(rows.nth(1)).toContainText('/Users/dev/tako');
    await expect(rows.nth(1)).toContainText('tako');
    await page.screenshot({ path: `${EVIDENCE_DIR}/01-picker.png` });
  });

  test('02. 1 操作でタブ + master が立ち、繋がったら Claude へ送り出す', async ({ page }) => {
    const calls = await setupMocks(page, { connectAfterMs: 2500 });
    await openLauncher(page);

    await page.locator('.launch-profile', { hasText: 'dev' }).click();
    // 待ち画面（bridge_status が出るまでポーリング）
    await expect(page.locator('.launch-waiting')).toBeVisible();
    await page.screenshot({ path: `${EVIDENCE_DIR}/02-waiting.png` });

    // 繋がったら「Claude で開く」（URL は daemon が返したもの）
    const link = page.locator('.launch-result .claude-open');
    await expect(link).toBeVisible({ timeout: 15000 });
    await expect(link).toHaveAttribute('href', LINK_URL);
    await expect(link).toHaveAttribute('target', '_blank');
    await page.screenshot({ path: `${EVIDENCE_DIR}/02-connected.png` });

    // 叩いた経路と本体（タブ → そのタブで master）
    expect(calls.tabs).toEqual([{ cwd: '/Users/dev/tako' }]);
    expect(calls.master).toHaveLength(1);
    expect(calls.master[0].url).toContain('/api/tabs/9/master');
    expect(calls.master[0].body).toEqual({ profile: 'dev' });

    // 起動したタブ・ペインが分かる（Mac 画面で探せる）
    await expect(page.locator('.launch-result-sub')).toContainText('タブ 9');
    await expect(page.locator('.launch-result-sub')).toContainText('ペイン 90');
  });

  test('03. 起動したタブが一覧に出る', async ({ page }) => {
    await setupMocks(page, { connectAfterMs: 0 });
    await openLauncher(page);
    await page.locator('.launch-profile', { hasText: 'dev' }).click();
    await expect(page.locator('.launch-result')).toBeVisible();
    // シートを閉じると一覧が更新されて新しいタブグループが出る
    await page.locator('.launch-result-actions .btn', { hasText: '閉じる' }).click();
    await expect(page.locator('.tab-group-name', { hasText: 'master-dev' })).toBeVisible({ timeout: 10000 });
    await expect(page.locator('.pane-card[data-pane-id="90"]')).toBeVisible();
    await page.screenshot({ path: `${EVIDENCE_DIR}/03-list-after.png` });
  });

  test('04. opt-in していないプロファイルでは公式リンクが出ず理由が出る', async ({ page }) => {
    await setupMocks(page, { profileState: 'off' });
    await openLauncher(page);
    await page.locator('.launch-profile', { hasText: 'default' }).click();

    const result = page.locator('.launch-result');
    await expect(result).toBeVisible();
    // 繋がらないことが起動時点で確定しているので待たない
    expect(await result.locator('.launch-waiting').count()).toBe(0);
    expect(await result.locator('.claude-open').count()).toBe(0);
    // 理由と有効化コマンドが出る（文言は daemon が返したもの）
    await expect(result.locator('.remote-link-reason')).toContainText('opt-in は既定 OFF');
    await expect(result.locator('.remote-link-reason code')).toHaveText(
      'tako orchestrator profiles set default --remote-control true'
    );
    await page.screenshot({ path: `${EVIDENCE_DIR}/04-opt-in-off.png` });
  });

  test('05. observe role では押す前に理由が出る', async ({ page }) => {
    await setupMocks(page, { role: 'observe' });
    await openLauncher(page);
    await expect(page.locator('.sheet')).toContainText('権限');
    await expect(page.locator('.sheet')).toContainText('Manage 以上');
    // 選択肢そのものを出さない（押しても 403 になるものを押させない）
    expect(await page.locator('.launch-profile').count()).toBe(0);
    await page.screenshot({ path: `${EVIDENCE_DIR}/05-observe.png` });
  });

  test('06. interact role でもサーバーが 403 なら理由を出す', async ({ page }) => {
    // 端末が manage を持っていても、サーバー側で権限が変わっていれば 403 になる。
    // その 403 を握りつぶさずに理由へ変える（黙って何も起きないのが最悪）
    await setupMocks(page, { launchStatus: 403 });
    await openLauncher(page);
    await page.locator('.launch-profile', { hasText: 'dev' }).click();
    await expect(page.locator('.sheet .error-text')).toContainText('Manage 以上');
    expect(await page.locator('.claude-open').count()).toBe(0);
    await page.screenshot({ path: `${EVIDENCE_DIR}/06-forbidden.png` });
  });

  test('07. 繋がらないまま上限に達したら理由へ切り替える', async ({ page }) => {
    // 待ち上限を待たずに検証するため、ページ内の上限を短くしてから起動する
    await setupMocks(page, { connectAfterMs: 3600000 });
    await page.goto(`${BASE}/#/`);
    await page.waitForSelector('.pane-card', { timeout: 10000 });
    // 実時間 90 秒は待てないので、Date.now を進めて上限到達を再現する
    await page.evaluate(() => {
      const real = Date.now;
      let shifted = false;
      Date.now = () => (shifted ? real() + 120000 : real());
      setTimeout(() => { shifted = true; }, 1500);
    });
    await page.locator('.launch-btn').click();
    await page.locator('.launch-profile', { hasText: 'dev' }).click();
    await expect(page.locator('.launch-note', { hasText: '時間内に Claude 公式へ繋がりません' }))
      .toBeVisible({ timeout: 20000 });
    // 理由（このペインの remote_link）が読める
    await expect(page.locator('.remote-link-reason')).toBeVisible();
    expect(await page.locator('.claude-open').count()).toBe(0);
    await page.screenshot({ path: `${EVIDENCE_DIR}/07-timeout.png` });
  });
});
