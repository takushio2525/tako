// Issue #1449: 「+」を選択式にして master / ターミナル / SSH ターミナルを起動する。
//
// 受け入れ条件を、モバイル viewport の実 DOM で確かめる:
//   ① 「+」から 3 種を選べ、それぞれ PC 側へ新しいタブ + ペインが立つ
//   ② SSH はホスト一覧から選べ、`ssh_connect` が PC 側と同じ形で読める
//   ③ observe role では選択肢が出ない（サーバーが 403 を返しても理由が出る）
//   ④ #1078 の「+ master」が同じ経路のまま生きている
//
// **叩く API まで固定する**のがこの spec の要点。#1449 は「操作を新設しない」
// （= 既存の `POST /api/tabs` / `POST /api/ssh` を呼ぶだけ）ことが設計の中身なので、
// PWA が独自の経路を生やしたらここで落ちる。
//
// A/B の対照は `?tako_1449_legacy=1`（PWA はブラウザで動くので env が届かない。
// 逃げ道はこの 1 つだけ）。legacy 腕では「+」が #1449 以前の master 1 択に戻る。
//
// 実行:
//   cd web/tako-remote && npx playwright test e2e/launch-menu-1449.spec.js
import { test, expect } from '@playwright/test';

const IPHONE_VIEWPORT = { width: 390, height: 844 };
const BASE = `http://localhost:${process.env.TAKO_PWA_PORT || 5174}`;
const EVIDENCE_DIR = process.env.TAKO_EVIDENCE_DIR || `${process.env.HOME}/dev/tako-evidence/1449`;

function me(role = 'manage') {
  return {
    registered: true, device_id: 'test-iphone', name: 'iPhone', role,
    login: 'user@example.com', host: 'test-mac', version: '0.8.12', app_connected: true,
  };
}

// `~/.ssh/config` 相当。実ホスト名は書かない（#927）
const FAKE_HOSTS = {
  hosts: [
    { name: 'build-box', hostname: 'build-box.example.test', user: 'dev', port: null },
    { name: 'win', hostname: null, user: null, port: null },
  ],
};

const PROFILES = {
  kind: 'master',
  profiles: [
    {
      name: 'dev', kind: 'master', model: 'opus 5', effort: 'high',
      cwd: '/Users/dev/tako', projects: ['tako'],
      remote_control: true, remote_control_effective: true,
    },
  ],
};

const SHELL_PANE = {
  id: 21, title: 'zsh', role: '', agent_type: 'plain',
  cwd: '/Users/dev', state: 'idle', surface: 'foreground',
  position: '1/1', tab_id: 3, tab_title: 'dotfiles', cols: 100, rows: 30,
  focused: false, tmux_target: 'tako-s1:0.0', preview: ['$ '],
  can_ssh: { ok: true }, ssh_connect: null,
};

/// `POST /api/tabs` で立った素のシェル（新しいタブ）
const NEW_TERMINAL_PANE = {
  id: 55, title: 'zsh', role: '', agent_type: 'plain',
  cwd: '/Users/dev', state: 'idle', surface: 'foreground',
  position: '1/1', tab_id: 12, tab_title: '4', cols: 120, rows: 40,
  focused: true, tmux_target: 'tako-s12:0.0', preview: ['$ '],
  can_ssh: { ok: true }, ssh_connect: null,
};

/// `POST /api/ssh { target: "tab" }` で立った SSH ペイン
function sshPane(connect) {
  return {
    id: 66, title: 'ssh:build-box', role: '', agent_type: 'plain',
    cwd: null, state: 'unknown', surface: 'foreground',
    position: '1/1', tab_id: 13, tab_title: 'ssh:build-box', cols: 120, rows: 40,
    focused: true, tmux_target: 'tako-s13:0.0', preview: [],
    can_ssh: { ok: true }, ssh_connect: connect,
  };
}

function json(route, body, status = 200) {
  return route.fulfill({ status, contentType: 'application/json', body: JSON.stringify(body) });
}

/**
 * API をすべてモックする。`calls` に「PWA が何をどの順で叩いたか」が溜まる。
 *
 * `opts.tabsStatus` / `opts.sshStatus` を 403 にするとサーバー側の role 不足、
 * `opts.hosts` で一覧の中身（空の検証用）を差し替えられる
 */
async function setupMocks(page, opts = {}) {
  const {
    role = 'manage', tabsStatus = 200, sshStatus = 200,
    hosts = FAKE_HOSTS, hostsStatus = 200,
    sshConnect = { host: 'build-box', phase: 'connecting', elapsed_secs: 2, fresh_pane: true },
  } = opts;
  const calls = { tabs: [], ssh: [], master: [], hosts: 0 };
  const extra = [];

  await page.route('**/api/me', route => json(route, me(role)));
  await page.route('**/api/master/profiles', route => json(route, PROFILES));
  await page.route('**/api/tabs', route => {
    calls.tabs.push(route.request().postDataJSON() ?? {});
    if (tabsStatus !== 200) {
      return json(route, { error: `権限が足りません（${role} → manage 以上が必要）` }, tabsStatus);
    }
    extra.push(NEW_TERMINAL_PANE);
    return json(route, { tab: 12, pane: 55, cwd: null });
  });
  await page.route('**/api/tabs/*/master', route => {
    calls.master.push({ url: route.request().url(), body: route.request().postDataJSON() });
    return json(route, {
      ok: true, tab: 12, pane: 55, profile: 'dev', tab_title: 'master-dev',
      role: 'orchestrator-master:dev', agent: 'claude',
      remote_control: { state: 'off', opt_in: false, reason: 'opt-in していません' },
    });
  });
  await page.route('**/api/ssh-hosts', route => {
    calls.hosts += 1;
    if (hostsStatus !== 200) {
      return json(route, { error: 'この端末は manage 権限がありません' }, hostsStatus);
    }
    return json(route, hosts);
  });
  await page.route('**/api/ssh', route => {
    calls.ssh.push({ url: route.request().url(), body: route.request().postDataJSON() });
    if (sshStatus !== 200) {
      return json(route, { error: `権限が足りません（${role} → manage 以上が必要）` }, sshStatus);
    }
    extra.push(sshPane(sshConnect));
    return json(route, {
      tab: 13, pane: 66, host: route.request().postDataJSON().host,
      remote_dir: null, target: 'tab',
      poll: '/api/v2/panes の ssh_connect で接続の進み方と失敗の理由が読める',
    });
  });
  await page.route('**/api/v2/panes', route =>
    json(route, { api_version: 2, panes: [SHELL_PANE, ...extra] })
  );
  await page.route('**/api/panes/*/screen*', route =>
    json(route, { lines: ['$ '], cursor: { x: 2, y: 0 }, size: { cols: 120, rows: 40 } })
  );
  await page.route('**/api/sessions/*/messages*', route =>
    json(route, { session_id: 'x', messages: [] })
  );
  await page.route('**/api/agents', route => json(route, { agents: [] }));
  await page.route('**/api/health', route => json(route, { status: 'ok', version: '0.8.12' }));
  await page.route('**/ws?*', route => route.abort());
  await page.route('**/manifest.json', route => json(route, { name: 'tako remote' }));
  await page.route('**/sw.js', route =>
    route.fulfill({ status: 200, contentType: 'application/javascript', body: '' })
  );
  return calls;
}

async function openMenu(page, query = '') {
  await page.goto(`${BASE}/${query}#/`);
  await page.waitForSelector('.pane-card', { timeout: 10000 });
  await page.locator('.launch-btn').click();
  await page.waitForSelector('.sheet', { timeout: 5000 });
}

test.describe('#1449 「+」から 3 種を起動する — モバイル', () => {
  test.use({ viewport: IPHONE_VIEWPORT });

  test('01. 「+」は 3 択（master / ターミナル / SSH ターミナル）', async ({ page }) => {
    await setupMocks(page);
    await openMenu(page);

    await expect(page.locator('[data-testid="launch-kind-master"]')).toContainText('master を起動');
    await expect(page.locator('[data-testid="launch-kind-terminal"]')).toContainText('ターミナル');
    await expect(page.locator('[data-testid="launch-kind-ssh"]')).toContainText('SSH ターミナル');
    await expect(page.locator('.launch-kind')).toHaveCount(3);
    // 入口のラベルも master 固定ではなくなっている
    await expect(page.locator('.launch-btn')).toContainText('新規');
    await page.screenshot({ path: `${EVIDENCE_DIR}/01-menu.png` });
  });

  test('02. ターミナル: POST /api/tabs だけでタブが立ち、そのペインへ移る', async ({ page }) => {
    const calls = await setupMocks(page);
    await openMenu(page);
    await page.locator('[data-testid="launch-kind-terminal"]').click();

    // 叩くのは既存の 1 本だけ（master 用の組み立て経路は通らない）
    await expect.poll(() => calls.tabs.length).toBe(1);
    expect(calls.master).toHaveLength(0);
    // 立ったペインへ移る = PWA がそのタブへ切り替わる
    await page.waitForFunction(() => window.location.hash === '#/panes/55', { timeout: 5000 });
    // 立ったのは素のシェルなので term ビュー（入力バー + SSH の入口）が出る
    await expect(page.locator('.term-input-field')).toBeVisible({ timeout: 10000 });
    await expect(page.locator('[data-testid="ssh-open-btn"]')).toBeVisible();
    await page.screenshot({ path: `${EVIDENCE_DIR}/02-terminal.png` });
  });

  test('03. SSH: ホスト一覧から選ぶと target=tab で開き、接続状態が読める', async ({ page }) => {
    const calls = await setupMocks(page);
    await openMenu(page);
    await page.locator('[data-testid="launch-kind-ssh"]').click();

    // 一覧は PC 側の答えそのまま（`GET /api/ssh-hosts` = `Request::SshHosts`）
    await expect(page.locator('[data-testid="ssh-host-build-box"]')).toBeVisible();
    await expect(page.locator('[data-testid="ssh-host-build-box"]')).toContainText('build-box.example.test');
    await expect(page.locator('[data-testid="ssh-host-win"]')).toBeVisible();
    await page.screenshot({ path: `${EVIDENCE_DIR}/03-ssh-hosts.png` });

    await page.locator('[data-testid="ssh-host-build-box"]').click();
    await expect.poll(() => calls.ssh.length).toBe(1);
    // 開き先の語彙は #1006 のもの。「+」からは常に新しいタブ
    expect(calls.ssh[0].body).toMatchObject({ host: 'build-box', target: 'tab' });
    expect(calls.ssh[0].url).toContain('/api/ssh');
    // タブ作成 API は通らない（`OpenRemote` がタブごと作る = 二重に作らない）
    expect(calls.tabs).toHaveLength(0);

    await page.waitForFunction(() => window.location.hash === '#/panes/66', { timeout: 5000 });
    const bar = page.locator('[data-testid="ssh-connect-bar"]');
    await expect(bar).toBeVisible({ timeout: 10000 });
    await expect(bar).toContainText('接続中');
    await expect(bar).toContainText('build-box');
    await page.screenshot({ path: `${EVIDENCE_DIR}/03-ssh-connecting.png` });
  });

  test('04. master: #1078 の経路（tabs → tabs/:id/master）がそのまま生きている', async ({ page }) => {
    const calls = await setupMocks(page);
    await openMenu(page);
    await page.locator('[data-testid="launch-kind-master"]').click();

    await page.locator('.launch-profile', { hasText: 'dev' }).click();
    await expect.poll(() => calls.master.length).toBe(1);
    expect(calls.tabs).toEqual([{ cwd: '/Users/dev/tako' }]);
    expect(calls.master[0].url).toContain('/api/tabs/12/master');
    expect(calls.master[0].body).toEqual({ profile: 'dev' });
    await page.screenshot({ path: `${EVIDENCE_DIR}/04-master.png` });
  });

  test('05. observe role では 3 択そのものが出ない', async ({ page }) => {
    const calls = await setupMocks(page, { role: 'observe' });
    await openMenu(page);

    expect(await page.locator('.launch-kind').count()).toBe(0);
    await expect(page.locator('.sheet')).toContainText('権限');
    await expect(page.locator('.sheet')).toContainText('Manage 以上');
    // 見せないだけでなく、押せる経路も無い（一覧すら引かない）
    expect(calls.hosts).toBe(0);
    expect(calls.tabs).toHaveLength(0);
    await page.screenshot({ path: `${EVIDENCE_DIR}/05-observe.png` });
  });

  test('06. サーバーが 403 を返したら理由を出す（黙って閉じない）', async ({ page }) => {
    // 端末が manage を名乗っていても、サーバー側で権限が変わっていれば 403 になる
    await setupMocks(page, { tabsStatus: 403, sshStatus: 403 });
    await openMenu(page);

    await page.locator('[data-testid="launch-kind-terminal"]').click();
    await expect(page.locator('[data-testid="launch-error"]')).toContainText('Manage 以上');
    // 失敗したので移動しない（一覧のままで、押し直せる）
    expect(await page.evaluate(() => window.location.hash)).toBe('#/');

    await page.locator('[data-testid="launch-kind-ssh"]').click();
    await page.locator('[data-testid="ssh-host-build-box"]').click();
    await expect(page.locator('[data-testid="ssh-sheet-error"]')).toContainText('Manage 以上');
    await page.screenshot({ path: `${EVIDENCE_DIR}/06-forbidden.png` });
  });

  test('07-edge. ホスト一覧が空でも理由が読める', async ({ page }) => {
    await setupMocks(page, { hosts: { hosts: [] } });
    await openMenu(page);
    await page.locator('[data-testid="launch-kind-ssh"]').click();
    await expect(page.locator('[data-testid="ssh-hosts-empty"]')).toContainText('~/.ssh/config');
    await page.screenshot({ path: `${EVIDENCE_DIR}/07-hosts-empty.png` });
  });

  test('08-edge. ホスト一覧が 403 でも黙って空にしない', async ({ page }) => {
    await setupMocks(page, { hostsStatus: 403 });
    await openMenu(page);
    await page.locator('[data-testid="launch-kind-ssh"]').click();
    await expect(page.locator('[data-testid="ssh-sheet-error"]')).toContainText('manage 権限');
  });

  test('09-edge. 接続に失敗したホストは理由がその場に残る（ペインは消えない）', async ({ page }) => {
    await setupMocks(page, {
      sshConnect: {
        host: 'build-box', phase: 'failed', elapsed_secs: 10, fresh_pane: true,
        reason: 'ssh: connect to host build-box.example.test port 22: Operation timed out',
        next_step: '繋がらないままなら ssh build-box を手で実行する',
      },
    });
    await openMenu(page);
    await page.locator('[data-testid="launch-kind-ssh"]').click();
    await page.locator('[data-testid="ssh-host-build-box"]').click();
    await page.waitForFunction(() => window.location.hash === '#/panes/66', { timeout: 5000 });

    const bar = page.locator('[data-testid="ssh-connect-bar"]');
    await expect(bar).toBeVisible({ timeout: 10000 });
    await expect(bar).toContainText('接続できません');
    await expect(bar).toContainText('Operation timed out');
    // 一覧を何度ポーリングしてもペインは残る（#919 / #1040 の契約）
    await page.waitForTimeout(2500);
    await expect(bar).toBeVisible();
    await page.screenshot({ path: `${EVIDENCE_DIR}/09-ssh-failed.png` });
  });

  test('10-edge. ターミナルを連打しても 1 枚しか立たない', async ({ page }) => {
    const calls = await setupMocks(page);
    await openMenu(page);
    const btn = page.locator('[data-testid="launch-kind-terminal"]');
    await btn.click();
    // 2 度目・3 度目は disabled（または既に遷移済み）なので force で押し込む
    await btn.click({ force: true, timeout: 1000 }).catch(() => {});
    await btn.click({ force: true, timeout: 1000 }).catch(() => {});
    await page.waitForFunction(() => window.location.hash === '#/panes/55', { timeout: 5000 });
    await page.waitForTimeout(500);
    expect(calls.tabs).toHaveLength(1);
  });

  test('11-AB. legacy 腕（?tako_1449_legacy=1）は #1449 以前の master 1 択に戻る', async ({ page }) => {
    await setupMocks(page);
    await openMenu(page, '?tako_1449_legacy=1');

    expect(await page.locator('.launch-kind').count()).toBe(0);
    await expect(page.locator('.launch-btn')).toContainText('master');
    // 開いた時点で #1078 のプロファイル一覧（= 旧挙動そのもの）
    await expect(page.locator('.launch-profile', { hasText: 'dev' })).toBeVisible();
    await page.screenshot({ path: `${EVIDENCE_DIR}/11-legacy.png` });
  });
});
