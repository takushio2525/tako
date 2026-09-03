// Issue #1080: スマホから SSH ターミナルの切り替え / 新規接続。
//
// 受け入れ条件の 3 つを、モバイル viewport の実 DOM で確かめる:
//   ① ホストを選ぶと Mac 側にペインができて接続まで進む
//   ② 到達不能ホストで理由が出て**ペインが消えない**（#919 / #1040 の契約）
//   ③ `can_ssh` が false のペインは「このペイン」が選択肢に出ない（#1006 の判定を共有）
//
// API はすべて page.route でモックするので daemon も実 SSH も要らない。
// 実 daemon / 実 SSH での通しは PR の検証節に別途記録する。
//
// 実行:
//   cd web/tako-remote && npx playwright test e2e/ssh-1080.spec.js
import { test, expect } from '@playwright/test';

const EVIDENCE_DIR = process.env.TAKO_EVIDENCE_DIR || `${process.env.HOME}/dev/tako-evidence/1080`;
const PREFIX = process.env.TAKO_SHOT_PREFIX || 'after';
const IPHONE_VIEWPORT = { width: 390, height: 844 };
const BASE = `http://localhost:${process.env.TAKO_PWA_PORT || 5174}`;

const FAKE_ME = {
  registered: true,
  device_id: 'test-iphone',
  name: 'iPhone',
  role: 'manage',
  login: 'user@example.com',
  host: 'test-mac',
  version: '0.8.4',
  app_connected: true,
};

// `~/.ssh/config` 相当。実ホスト名は書かない（#927）
const FAKE_HOSTS = {
  hosts: [
    { name: 'build-box', hostname: 'build-box.example.test', user: 'dev', port: null },
    { name: 'win', hostname: null, user: null, port: null },
    { name: 'unreachable', hostname: '198.51.100.9', user: null, port: 2222 },
  ],
};

// 素のシェルのペイン（SSH 化できる） + エージェントのペイン（できない）
function panes({ sshConnect = null, canSsh = { ok: true } } = {}) {
  return {
    api_version: 2,
    panes: [
      {
        id: 7, title: 'zsh', role: '', agent_type: 'plain',
        cwd: '/Users/dev/tako', state: 'idle', surface: 'foreground',
        position: '1/2', tab_id: 1, tab_title: 'tako', cols: 120, rows: 40,
        focused: true, tmux_target: 'tako-7:0.0',
        preview: ['$ '], ssh_connect: sshConnect, can_ssh: canSsh,
      },
      {
        id: 9, title: 'master', role: 'master', agent_type: 'claude',
        cwd: '/Users/dev/tako', state: 'running', surface: 'foreground',
        position: '2/2', tab_id: 1, tab_title: 'tako', cols: 120, rows: 40,
        focused: false, tmux_target: 'tako-9:0.0', session_id: 'm-1',
        preview: ['⏺ 待機中'], ssh_connect: null,
        can_ssh: {
          ok: false,
          reason: 'agent_role',
          note: 'pane 9 は AI エージェントのペインなので SSH 化できない（対話が壊れる）。target=split で新しいペインを作って接続する',
        },
      },
    ],
  };
}

function json(route, body, status = 200) {
  return route.fulfill({ status, contentType: 'application/json', body: JSON.stringify(body) });
}

async function setupMocks(page, opts = {}) {
  const state = { panes: panes(opts), hostsStatus: opts.hostsStatus || 200, opens: [] };
  await page.route('**/api/me', route => json(route, FAKE_ME));
  await page.route('**/api/v2/panes', route => json(route, state.panes));
  await page.route('**/api/ssh-hosts', route => {
    if (state.hostsStatus !== 200) {
      return json(route, { error: 'この端末は manage 権限がありません' }, state.hostsStatus);
    }
    return json(route, FAKE_HOSTS);
  });
  await page.route('**/api/panes/*/screen*', route =>
    json(route, { lines: ['$ '], cursor: { x: 2, y: 0 }, size: { cols: 120, rows: 40 } })
  );
  await page.route('**/api/agents', route => json(route, { agents: [] }));
  await page.route('**/api/health', route => json(route, { status: 'ok', version: '0.8.4' }));
  await page.route('**/ws?*', route => route.abort());
  await page.route('**/manifest.json', route => json(route, { name: 'tako remote' }));
  await page.route('**/sw.js', route =>
    route.fulfill({ status: 200, contentType: 'application/javascript', body: '' })
  );
  return state;
}

// 接続要求を受けたら daemon が返す応答 + 以後のペイン一覧の変化をモックする
async function mockOpen(page, state, { pane, target, after }) {
  const handler = async route => {
    state.opens.push({ url: route.request().url(), body: route.request().postDataJSON() });
    if (after) state.panes = after;
    await json(route, {
      tab: 1, pane, host: route.request().postDataJSON().host,
      remote_dir: null, target,
      poll: '/api/v2/panes の ssh_connect で接続の進み方と失敗の理由が読める',
    });
  };
  await page.route('**/api/panes/*/ssh', handler);
  await page.route('**/api/ssh', handler);
}

async function openSshSheet(page, paneId = 7) {
  await page.goto(`${BASE}/#/panes/${paneId}`);
  // 素のシェルのペインは chat 非対応なので term ビューが既定で出る
  await page.waitForSelector('[data-testid="ssh-open-btn"]', { timeout: 10000 });
  await page.click('[data-testid="ssh-open-btn"]');
  await page.waitForSelector('[data-testid="ssh-sheet"]', { timeout: 5000 });
}

test.use({ viewport: IPHONE_VIEWPORT });

test('① ホストを選ぶと接続が始まり、新しいペインへ移る（target=split）', async ({ page }) => {
  const state = await setupMocks(page);
  // 開いたあとの一覧: 新ペイン 11 ができて接続中
  const after = panes();
  after.panes.push({
    id: 11, title: 'ssh:build-box', role: '', agent_type: 'plain',
    cwd: null, state: 'unknown', surface: 'foreground',
    position: '3/3', tab_id: 1, tab_title: 'tako', cols: 120, rows: 40,
    focused: true, tmux_target: 'tako-11:0.0', preview: [],
    can_ssh: { ok: true },
    ssh_connect: { host: 'build-box', phase: 'connecting', elapsed_secs: 2, reason: null, fresh_pane: true },
  });
  await mockOpen(page, state, { pane: 11, target: 'split', after });

  await openSshSheet(page);
  await page.screenshot({ path: `${EVIDENCE_DIR}/${PREFIX}-01-sheet.png` });
  await page.click('[data-testid="ssh-target-split"]');
  await page.click('[data-testid="ssh-host-build-box"]');

  // 送った中身が #1006 の語彙どおりであること
  await expect.poll(() => state.opens.length).toBe(1);
  expect(state.opens[0].body).toMatchObject({ host: 'build-box', target: 'split' });
  expect(state.opens[0].url).toContain('/api/ssh');

  // 新しいペインへ移り、接続中が見えている
  await page.waitForFunction(() => window.location.hash === '#/panes/11', { timeout: 5000 });
  await page.waitForSelector('[data-testid="ssh-connect-bar"]', { timeout: 10000 });
  await expect(page.locator('[data-testid="ssh-connect-bar"]')).toContainText('接続中');
  await expect(page.locator('[data-testid="ssh-connect-bar"]')).toContainText('build-box');
  await page.screenshot({ path: `${EVIDENCE_DIR}/${PREFIX}-02-connecting.png` });
});

test('① このペインを SSH にする（target=pane）とペインは増えず移動もしない', async ({ page }) => {
  const state = await setupMocks(page);
  await mockOpen(page, state, { pane: 7, target: 'pane' });
  await openSshSheet(page);
  // 素のシェルなので「このペイン」が既定で選ばれている
  await expect(page.locator('[data-testid="ssh-target-pane"]')).toHaveClass(/sheet-effort-active/);
  await page.click('[data-testid="ssh-host-win"]');
  await expect.poll(() => state.opens.length).toBe(1);
  expect(state.opens[0].url).toContain('/api/panes/7/ssh');
  expect(state.opens[0].body).toMatchObject({ host: 'win', target: 'pane' });
  // ペイン ID は変わらない（#1006 の本題）
  await page.waitForTimeout(400);
  expect(await page.evaluate(() => window.location.hash)).toBe('#/panes/7');
});

test('② 到達不能ホストは理由が出てペインが消えない', async ({ page }) => {
  const state = await setupMocks(page);
  const after = panes();
  after.panes.push({
    id: 11, title: 'ssh:unreachable', role: '', agent_type: 'plain',
    cwd: null, state: 'unknown', surface: 'foreground',
    position: '3/3', tab_id: 1, tab_title: 'tako', cols: 120, rows: 40,
    focused: true, tmux_target: 'tako-11:0.0', preview: [],
    can_ssh: { ok: true },
    ssh_connect: {
      host: 'unreachable', phase: 'failed', elapsed_secs: 10, fresh_pane: true,
      reason: 'ssh: connect to host 198.51.100.9 port 2222: Operation timed out',
    },
  });
  await mockOpen(page, state, { pane: 11, target: 'split', after });

  await openSshSheet(page);
  await page.click('[data-testid="ssh-target-split"]');
  await page.click('[data-testid="ssh-host-unreachable"]');
  await page.waitForFunction(() => window.location.hash === '#/panes/11', { timeout: 5000 });

  const bar = page.locator('[data-testid="ssh-connect-bar"]');
  await expect(bar).toBeVisible({ timeout: 10000 });
  await expect(bar).toContainText('接続できません');
  await expect(bar).toContainText('Operation timed out');
  await page.screenshot({ path: `${EVIDENCE_DIR}/${PREFIX}-03-failed.png` });

  // ペインは残り続ける。一覧を何度ポーリングしても消えないし、理由も消えない
  await page.waitForTimeout(6000);
  await expect(bar).toBeVisible();
  await expect(bar).toContainText('Operation timed out');
  const count = await page.evaluate(async () => (await (await fetch('/api/v2/panes')).json()).panes.length);
  expect(count).toBe(3);
});

test('② 再接続中は試行回数と次の再試行までの秒数が出る（#1040）', async ({ page }) => {
  await setupMocks(page, {
    sshConnect: {
      host: 'build-box', phase: 'reconnecting', elapsed_secs: 40, attempt: 2,
      max_attempts: 6, retry_in_secs: 5, disconnected_secs: 12,
      reason: 'Connection closed by remote host',
      next_step: '繋がらないままなら ssh build-box を手で実行する',
    },
  });
  await page.goto(`${BASE}/#/panes/7`);
  const bar = page.locator('[data-testid="ssh-connect-bar"]');
  await expect(bar).toBeVisible({ timeout: 10000 });
  await expect(bar).toContainText('再接続中');
  await expect(bar).toContainText('2/6 回目');
  await expect(bar).toContainText('5 秒後に再試行');
  await expect(bar).toContainText('ssh build-box を手で実行');
  await page.screenshot({ path: `${EVIDENCE_DIR}/${PREFIX}-04-reconnecting.png` });
});

test('③ can_ssh が false のペインは「このペイン」が選択肢に出ない', async ({ page }) => {
  await setupMocks(page);
  // pane 9 = master（agent_role で拒否）。chat が既定なので term へ切り替える
  await page.goto(`${BASE}/#/panes/9`);
  await page.waitForSelector('.view-toggle-btn', { timeout: 10000 });
  await page.click('.view-toggle-btn:has-text("term")');
  await page.click('[data-testid="ssh-open-btn"]');
  await page.waitForSelector('[data-testid="ssh-sheet"]');

  await expect(page.locator('[data-testid="ssh-target-pane"]')).toHaveCount(0);
  await expect(page.locator('[data-testid="ssh-target-split"]')).toBeVisible();
  await expect(page.locator('[data-testid="ssh-target-tab"]')).toBeVisible();
  // 出さないだけでなく、理由は読める（スマホには右クリックのような別入口が無い）
  await expect(page.locator('[data-testid="ssh-pane-blocked"]')).toContainText('AI エージェントのペイン');
  await expect(page.locator('[data-testid="ssh-pane-blocked"]')).toContainText('target=split');
  await page.screenshot({ path: `${EVIDENCE_DIR}/${PREFIX}-05-blocked.png` });
  // 既定は「新しいペイン」に倒れている（押せない選択肢が選ばれた状態にしない）
  await expect(page.locator('[data-testid="ssh-target-split"]')).toHaveClass(/sheet-effort-active/);
});

test('権限やホスト一覧の失敗は理由が読める（黙って空にしない）', async ({ page }) => {
  await setupMocks(page, { hostsStatus: 403 });
  await openSshSheet(page);
  await expect(page.locator('[data-testid="ssh-sheet-error"]')).toContainText('manage 権限');
  await page.screenshot({ path: `${EVIDENCE_DIR}/${PREFIX}-06-forbidden.png` });
});
