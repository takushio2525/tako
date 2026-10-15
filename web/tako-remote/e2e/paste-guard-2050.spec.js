// Issue #2050: スマホの入力に鍵らしき文字があれば送る前に確かめる（判定は PC 側）。
//
// 受け入れ条件を、モバイル viewport の実 DOM で確かめる（daemon はモック。PC の判定と daemon の
// 409 は Rust の単体と scripts/test-paste-guard-2050.sh の実経路が見る）:
//   ① 409 + 伏せ字 → 確認シート（規則・伏せ字。本文は出ない）→「やめる」なら送り直さず、
//      入力欄に文字が戻る（直せる）
//   ② 「送る」なら `confirm_secret: true` を付けて 1 回だけ送り直す
//   ③ 警告だけの段（200 + paste_guard）: 送れて、画面の上に通知（押すと消える）
//   ④ 当たらない入力はシートも通知も出ない（1 回で送る）
//
// **実データは一切見ない**（#927）。鍵は前置詞と本体を分けて書いた偽物。
//
// 実行:
//   cd web/tako-remote && npx playwright test e2e/paste-guard-2050.spec.js
import { test, expect } from '@playwright/test';
import { evidencePath, TAKO_VERSION } from './support.js';

const IPHONE_VIEWPORT = { width: 390, height: 844 };
const BASE = `http://localhost:${process.env.TAKO_PWA_PORT || 5174}`;
const PANE = 7;
const SECRET = 'ghp_' + 'EXAMPLE0123456789abcdefghijklmnopqrst';
const GUARD = {
  verdict: 'confirm',
  rules: ['github_token'],
  findings: [{ rule: 'github_token', line: 1, masked: 'ghp_…qrst' }],
  truncated: false,
};

function json(route, body, status = 200) {
  return route.fulfill({ status, contentType: 'application/json', body: JSON.stringify(body) });
}

/**
 * API をすべてモックする。`mode` = confirm（当たれば 409）/ warn（送って 200 + 警告）。
 * `inputs` に PWA が `/input` へ送った本文が順に溜まる
 */
async function setupMocks(page, mode = 'confirm') {
  const inputs = [];
  await page.route('**/api/me', route => json(route, {
    registered: true, device_id: 'nPHONE2050', name: 'phone-a', role: 'interact',
    login: 'tester@example.com', host: 'test-mac', version: TAKO_VERSION, app_connected: true,
    pending: false, denied: false,
  }));
  await page.route('**/api/v2/panes', route => json(route, {
    api_version: 2,
    panes: [{ id: PANE, title: 'zsh', agent_type: 'plain', tmux_target: 'tako-test:0.0', position: 'tab 1', status: 'idle' }],
  }));
  await page.route('**/api/agents', route => json(route, { agents: [] }));
  await page.route('**/api/health', route => json(route, { status: 'ok', version: TAKO_VERSION }));
  await page.route('**/api/tasks**', route => json(route, { tasks: [], count: 0, open_count: 0 }));
  await page.route('**/api/cards**', route => json(route, { pane: PANE, cards: [], total: 0 }));
  await page.route('**/api/panes/*/screen*', route =>
    json(route, { lines: ['tako ~ %'], cursor: { x: 9, y: 0 }, size: { cols: 80, rows: 24 } })
  );
  await page.route('**/ws?*', route => route.abort());
  await page.route('**/manifest.json', route => json(route, { name: 'tako remote' }));
  await page.route('**/sw.js', route =>
    route.fulfill({ status: 200, contentType: 'application/javascript', body: '' })
  );
  await page.route(`**/api/panes/${PANE}/input`, route => {
    const body = route.request().postDataJSON() || {};
    inputs.push(body);
    const hit = typeof body.text === 'string' && body.text.includes('ghp_');
    if (!hit) return json(route, { ok: true });
    if (mode === 'warn') return json(route, { ok: true, paste_guard: { ...GUARD, verdict: 'warn' } });
    if (body.confirm_secret === true) return json(route, { ok: true });
    return json(route, { error: '鍵らしき文字が入っている（送っていない）', paste_guard: GUARD }, 409);
  });
  return { inputs };
}

async function openTerminal(page) {
  await page.goto(`${BASE}/#/panes/${PANE}`);
  await page.waitForSelector('.term-input-field', { timeout: 10000 });
}

async function typeAndSend(page, text) {
  await page.locator('.term-input-field').fill(text);
  await page.locator('.term-send-btn').click();
}

test.describe('#2050 スマホの入力の鍵らしき文字の確認 — モバイル', () => {
  test.use({ viewport: IPHONE_VIEWPORT });

  test('01. 確認シートに規則と伏せ字（本文は出ない）→ やめると送り直さず、入力欄に戻る', async ({ page }) => {
    const { inputs } = await setupMocks(page);
    await openTerminal(page);
    const text = `export TOKEN=${SECRET}`;
    await typeAndSend(page, text);
    const sheet = page.locator('[data-testid="paste-guard-sheet"]');
    await expect(sheet).toBeVisible();
    await expect(sheet).toContainText('鍵らしき文字が入っています');
    await expect(sheet).toContainText('GitHub のトークン・1 行目');
    await expect(sheet).toContainText('ghp_…qrst');
    await expect(sheet).not.toContainText('EXAMPLE0123');
    await page.screenshot({ path: evidencePath('01-sheet.png'), fullPage: true });
    // 既定（目立つ方）は「やめる」
    await expect(page.locator('[data-testid="paste-guard-cancel"]')).toHaveClass(/btn-primary/);
    await page.locator('[data-testid="paste-guard-cancel"]').click();
    await expect(sheet).toHaveCount(0);
    expect(inputs).toHaveLength(1);
    expect(inputs[0].confirm_secret).toBeUndefined();
    // 直せるように入力欄へ戻る
    await expect(page.locator('.term-input-field')).toHaveValue(text);
    // 背景のタップでもやめる
    await page.locator('.term-send-btn').click();
    await expect(sheet).toBeVisible();
    await page.locator('.sheet-backdrop').click({ position: { x: 10, y: 10 } });
    await expect(sheet).toHaveCount(0);
    expect(inputs).toHaveLength(2);
    expect(inputs.every(b => b.confirm_secret === undefined)).toBe(true);
  });

  test('02. 「送る」なら confirm_secret を付けて 1 回だけ送り直す', async ({ page }) => {
    const { inputs } = await setupMocks(page);
    await openTerminal(page);
    const text = `export TOKEN=${SECRET}`;
    await typeAndSend(page, text);
    await page.locator('[data-testid="paste-guard-send"]').click();
    await expect(page.locator('[data-testid="paste-guard-sheet"]')).toHaveCount(0);
    await expect.poll(() => inputs.length).toBe(2);
    expect(inputs[1]).toEqual({ text, newline: true, confirm_secret: true });
    await expect(page.locator('.term-input-field')).toHaveValue('');
    await page.screenshot({ path: evidencePath('02-sent.png'), fullPage: true });
  });

  test('03. 警告だけの段: 送れて、通知が出る（押すと消える）', async ({ page }) => {
    const { inputs } = await setupMocks(page, 'warn');
    await openTerminal(page);
    await typeAndSend(page, `export TOKEN=${SECRET}`);
    const notice = page.locator('[data-testid="paste-guard-notice"]');
    await expect(notice).toBeVisible();
    await expect(notice).toContainText('そのまま送りました');
    await expect(notice).toContainText('ghp_…qrst');
    await expect(page.locator('[data-testid="paste-guard-sheet"]')).toHaveCount(0);
    expect(inputs).toHaveLength(1);
    await page.screenshot({ path: evidencePath('03-warned.png'), fullPage: true });
    await notice.click();
    await expect(notice).toHaveCount(0);
  });

  test('04. 当たらない入力はシートも通知も出ない', async ({ page }) => {
    const { inputs } = await setupMocks(page);
    await openTerminal(page);
    await typeAndSend(page, 'cargo test --workspace');
    await expect.poll(() => inputs.length).toBe(1);
    await expect(page.locator('[data-testid="paste-guard-sheet"]')).toHaveCount(0);
    await expect(page.locator('[data-testid="paste-guard-notice"]')).toHaveCount(0);
    await expect(page.locator('.term-input-field')).toHaveValue('');
  });
});
