// Issue #1452: 権限が足りないときに「権限の更新をリクエスト」できる。
//
// 受け入れ条件を、モバイル viewport の実 DOM で確かめる:
//   ① `#/files` の権限不足画面にリクエストの導線が出る（observe のとき）
//   ② 押すと **`POST /api/pair`（既存経路）だけ**が飛ぶ。要求 role・理由・端末名が載る
//   ③ 送信後は「PC で承認待ち」になり、`/api/me` を **2 秒ポーリング**する
//   ④ 承認されたら **再読込なしで**ファイル一覧が出る（ポーリングも止まる）
//   ⑤ 拒否されたら理由が出て、もう一度要求できる（ポーリングは止まる）
//   ⑥ 画面を離れたらポーリングは止まる
//
// **叩く API まで固定する**のがこの spec の要点。#1452 は「新しい API を作らない」
// （= #283 からある `/api/pair` を押せる場所に出すだけ）ことが設計の中身なので、
// PWA が独自の経路を生やしたらここで落ちる。
//
// A/B の対照は `?tako_1452_legacy=1`（PWA はブラウザで動くので env が届かない。
// 逃げ道はこの 1 つだけ）。legacy 腕では #1452 以前の「足りません」だけに戻る。
//
// 実行:
//   cd web/tako-remote && npx playwright test e2e/permission-request-1452.spec.js
import { test, expect } from '@playwright/test';

const IPHONE_VIEWPORT = { width: 390, height: 844 };
const BASE = `http://localhost:${process.env.TAKO_PWA_PORT || 5174}`;
const EVIDENCE_DIR = process.env.TAKO_EVIDENCE_DIR || `${process.env.HOME}/dev/tako-evidence/1452`;

// 実端末名・実ホスト名は書かない（#927）
function me(over = {}) {
  return {
    registered: true, device_id: 'nPHONEA', name: 'phone-a', role: 'observe',
    login: 'tester@example.com', host: 'test-mac', version: '0.8.12', app_connected: true,
    pending: false, denied: false,
    ...over,
  };
}

const ROOTS = {
  roots: [{ id: 'r1', name: 'tako', path: '/w/tako', kind: 'local', placement: 'local' }],
};

function json(route, body, status = 200) {
  return route.fulfill({ status, contentType: 'application/json', body: JSON.stringify(body) });
}

/**
 * API をモックする。`state` を書き換えると次のポーリングから応答が変わる。
 * `calls` に「PWA が何をどの順で叩いたか」が溜まる
 */
async function setupMocks(page, opts = {}) {
  const { role = 'observe' } = opts;
  const state = { role, pending: false, denied: false, requested_role: null };
  const calls = { pair: [], me: 0, files: 0, other: [] };

  await page.route('**/api/me', route => {
    calls.me += 1;
    return json(route, me({
      role: state.role,
      pending: state.pending,
      denied: state.denied,
      ...(state.requested_role ? { requested_role: state.requested_role } : {}),
    }));
  });
  await page.route('**/api/pair', route => {
    const body = route.request().postDataJSON() ?? {};
    calls.pair.push(body);
    state.pending = true;
    state.denied = false;
    state.requested_role = body.role;
    return json(route, { status: 'pending' });
  });
  await page.route('**/api/files**', route => {
    calls.files += 1;
    if (state.role === 'observe') {
      return json(route, { error: 'この操作には interact 以上の権限が必要' }, 403);
    }
    return json(route, ROOTS);
  });
  // 権限まわりで**呼ばれてはいけない**経路（呼ばれたら calls.other に残る）
  for (const path of ['**/api/admin/**', '**/api/devices**']) {
    await page.route(path, route => {
      calls.other.push(route.request().url());
      return json(route, { error: 'not reachable' }, 401);
    });
  }
  await page.route('**/api/v2/panes', route => json(route, { api_version: 2, panes: [] }));
  await page.route('**/api/agents', route => json(route, { agents: [] }));
  await page.route('**/api/health', route => json(route, { status: 'ok', version: '0.8.12' }));
  await page.route('**/ws?*', route => route.abort());
  await page.route('**/manifest.json', route => json(route, { name: 'tako remote' }));
  await page.route('**/sw.js', route =>
    route.fulfill({ status: 200, contentType: 'application/javascript', body: '' })
  );
  return { state, calls };
}

async function openFiles(page, query = '') {
  await page.goto(`${BASE}/${query}#/files`);
}

test.describe('#1452 権限の更新をリクエストする — モバイル', () => {
  test.use({ viewport: IPHONE_VIEWPORT });

  test('01. 権限不足の画面にリクエストの導線が出る', async ({ page }) => {
    await setupMocks(page);
    await openFiles(page);

    const card = page.locator('[data-testid="permission-request"]');
    await expect(card).toBeVisible({ timeout: 10000 });
    await expect(card).toContainText('権限が足りません');
    await expect(card).toContainText('interact');
    // 今の role より強いものだけが選べる（押しても何も起きない選択肢を並べない）
    await expect(page.locator('[data-testid="permission-role-interact"]')).toBeVisible();
    await expect(page.locator('[data-testid="permission-role-admin"]')).toBeVisible();
    await expect(page.locator('[data-testid="permission-role-observe"]')).toHaveCount(0);
    await expect(page.locator('[data-testid="permission-send"]')).toBeVisible();
    await page.screenshot({ path: `${EVIDENCE_DIR}/01-request.png` });
  });

  test('02. 押すと POST /api/pair だけが飛ぶ（要求 role・理由・端末名つき）', async ({ page }) => {
    const { calls } = await setupMocks(page);
    await openFiles(page);
    await page.locator('[data-testid="permission-request"]').waitFor({ timeout: 10000 });

    await page.locator('[data-testid="permission-role-manage"]').click();
    await page.locator('#permission-reason').fill('出先でログを見たい');
    await page.locator('[data-testid="permission-send"]').click();

    await expect(page.locator('[data-testid="permission-pending"]')).toBeVisible({ timeout: 5000 });
    expect(calls.pair).toHaveLength(1);
    expect(calls.pair[0].role).toBe('manage');
    expect(calls.pair[0].reason).toBe('出先でログを見たい');
    expect(typeof calls.pair[0].name).toBe('string');
    // 管理 API・端末管理 API には一切触らない
    expect(calls.other).toEqual([]);
    await page.screenshot({ path: `${EVIDENCE_DIR}/02-pending.png` });
  });

  test('03. 承認待ちの間だけ /api/me を見に行く', async ({ page }) => {
    const { calls } = await setupMocks(page);
    await openFiles(page);
    await page.locator('[data-testid="permission-request"]').waitFor({ timeout: 10000 });

    const before = calls.me;
    await page.locator('[data-testid="permission-send"]').click();
    await page.locator('[data-testid="permission-pending"]').waitFor({ timeout: 5000 });
    // 2 秒周期なので 5 秒で 2 回以上増える
    await expect.poll(() => calls.me - before, { timeout: 8000 }).toBeGreaterThanOrEqual(2);
  });

  test('04. 承認されたら再読込なしでファイル一覧が出て、ポーリングが止まる', async ({ page }) => {
    const { state, calls } = await setupMocks(page);
    await openFiles(page);
    await page.locator('[data-testid="permission-request"]').waitFor({ timeout: 10000 });
    await page.locator('[data-testid="permission-send"]').click();
    await page.locator('[data-testid="permission-pending"]').waitFor({ timeout: 5000 });

    // PC 側で承認された、に相当する状態遷移（PWA は何も再読込しない）
    const navigations = [];
    page.on('framenavigated', f => navigations.push(f.url()));
    state.role = 'manage';
    state.pending = false;

    await expect(page.locator('.file-row').first()).toBeVisible({
      timeout: 10000,
    });
    await expect(page.locator('[data-testid="permission-pending"]')).toHaveCount(0);
    // ページの読み込み直しが起きていない（= 会話も状態も保たれる）
    expect(navigations).toEqual([]);

    // ポーリングが止まっている（承認後に増え続けない）
    const settled = calls.me;
    await page.waitForTimeout(5000);
    expect(calls.me - settled).toBeLessThanOrEqual(1);
    await page.screenshot({ path: `${EVIDENCE_DIR}/04-granted.png` });
  });

  test('05. 拒否されたら理由が出て、もう一度要求できる（ポーリングは止まる）', async ({ page }) => {
    const { state, calls } = await setupMocks(page);
    await openFiles(page);
    await page.locator('[data-testid="permission-request"]').waitFor({ timeout: 10000 });
    await page.locator('[data-testid="permission-send"]').click();
    await page.locator('[data-testid="permission-pending"]').waitFor({ timeout: 5000 });

    state.pending = false;
    state.denied = true;

    const denied = page.locator('[data-testid="permission-denied"]');
    await expect(denied).toBeVisible({ timeout: 10000 });
    await expect(denied).toContainText('許可されませんでした');

    const settled = calls.me;
    await page.waitForTimeout(5000);
    expect(calls.me - settled).toBeLessThanOrEqual(1);

    // もう一度要求できる（行き止まりにしない）
    await denied.locator('button').click();
    await expect(page.locator('[data-testid="permission-request"]')).toBeVisible();
    await page.screenshot({ path: `${EVIDENCE_DIR}/05-denied.png` });
  });

  test('06. 画面を離れるとポーリングは止まる', async ({ page }) => {
    const { calls } = await setupMocks(page);
    await openFiles(page);
    await page.locator('[data-testid="permission-request"]').waitFor({ timeout: 10000 });
    await page.locator('[data-testid="permission-send"]').click();
    await page.locator('[data-testid="permission-pending"]').waitFor({ timeout: 5000 });

    // ホームへ戻る（権限不足の画面を降ろす）
    await page.evaluate(() => { window.location.hash = '#/'; });
    await page.waitForTimeout(500);
    const settled = calls.me;
    await page.waitForTimeout(5000);
    expect(calls.me - settled).toBeLessThanOrEqual(1);
  });

  test('07. 既に権限があったら待たずに使える（already_registered）', async ({ page }) => {
    const { state } = await setupMocks(page);
    await page.route('**/api/pair', route => {
      state.role = 'manage';
      return json(route, { status: 'already_registered', role: 'manage' });
    });
    await openFiles(page);
    await page.locator('[data-testid="permission-request"]').waitFor({ timeout: 10000 });
    await page.locator('[data-testid="permission-send"]').click();

    await expect(page.locator('.file-row').first()).toBeVisible({
      timeout: 10000,
    });
    await expect(page.locator('[data-testid="permission-pending"]')).toHaveCount(0);
  });

  test('08. 送信に失敗したら理由が出る（無言で落ちない）', async ({ page }) => {
    const { calls } = await setupMocks(page);
    await page.route('**/api/pair', route => {
      calls.pair.push(route.request().postDataJSON() ?? {});
      return json(route, { error: 'ペアリングを要求できない（tailnet の照合に失敗）' }, 403);
    });
    await openFiles(page);
    await page.locator('[data-testid="permission-request"]').waitFor({ timeout: 10000 });
    await page.locator('[data-testid="permission-send"]').click();

    await expect(page.locator('.error-text')).toContainText('tailnet', { timeout: 5000 });
    // 送信欄はそのまま残る（やり直せる）
    await expect(page.locator('[data-testid="permission-send"]')).toBeVisible();
  });

  test('09. 画面に絵文字が無い', async ({ page }) => {
    await setupMocks(page);
    await openFiles(page);
    await page.locator('[data-testid="permission-request"]').waitFor({ timeout: 10000 });
    const text = await page.locator('[data-testid="permission-request"]').innerText();
    // 絵文字（記号 + 絵文字ブロック・補助面）が 1 つも無いこと
    expect(text).not.toMatch(/[\u{1F300}-\u{1FAFF}\u{2600}-\u{27BF}\u{FE0F}]/u);
  });

  test('10. legacy 腕は #1452 以前（導線なし）に戻る', async ({ page }) => {
    const { calls } = await setupMocks(page);
    await openFiles(page, '?tako_1452_legacy=1');

    const legacy = page.locator('[data-testid="permission-legacy"]');
    await expect(legacy).toBeVisible({ timeout: 10000 });
    await expect(legacy).toContainText('権限が足りません');
    // 押せる場所が無い = #1452 が直した症状そのもの
    await expect(page.locator('[data-testid="permission-send"]')).toHaveCount(0);
    await expect(page.locator('[data-testid="permission-request"]')).toHaveCount(0);
    expect(calls.pair).toEqual([]);
    await page.screenshot({ path: `${EVIDENCE_DIR}/10-legacy.png` });
  });
});
