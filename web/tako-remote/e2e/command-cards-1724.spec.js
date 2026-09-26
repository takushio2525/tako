// Issue #1724: スマホ（PWA）からコマンド提案カードを実行する。
//
// 受け入れ条件を、モバイル viewport の実 DOM で確かめる:
//   ① PC のペインに出ているカードが、そのペインの画面に一覧で出る（ラベル・全文・件数）
//   ② interact: 「実行」→ 確認シートに**全文** →「PC で実行する」で 1 回だけ撃つ。
//      送るのは**番号だけ**（本文を送らない）。実行後は PC と同じ実行記録が出る
//   ③ 確認で「やめる」なら撃たない（誤タップ対策）
//   ④ observe: 実行ボタンが出ず、#1452 の権限リクエスト（既存の `/api/pair`）へつながる
//   ⑤ 実行中のコマンドは押せない。古い一覧のまま押しても 409 の理由が出る
//   ⑥ PC 側で閉じられたカードを押すと理由が残る（一覧から消えても黙らない）
//   ⑦ その場で降格された直後に押すと 403 → 権限の導線へ切り替わる
//   ⑧ 確認の二度押しでも 1 回しか撃たない
//
// **実データは一切見ない**（#927）。端末名・ホスト名・パスは偽物だけ。
//
// 実行:
//   cd web/tako-remote && npx playwright test e2e/command-cards-1724.spec.js
import { test, expect } from '@playwright/test';

const IPHONE_VIEWPORT = { width: 390, height: 844 };
const BASE = `http://localhost:${process.env.TAKO_PWA_PORT || 5174}`;
const EVIDENCE_DIR = process.env.TAKO_EVIDENCE_DIR || `${process.env.HOME}/dev/tako-evidence/1724`;

const PANE = 7;

function me(role = 'interact') {
  return {
    registered: true, device_id: 'nPHONE1724', name: 'phone-a', role,
    login: 'tester@example.com', host: 'test-mac', version: '0.8.12', app_connected: true,
    pending: false, denied: false,
  };
}

const PANE_INFO = {
  id: PANE, title: 'master', agent_type: 'plain', tmux_target: 'tako-test:0.0',
  position: 'tab 1', status: 'idle',
};

function card(id, commands, label = null, runs = null) {
  return {
    id, pane: PANE, label, commands, count: commands.length,
    runs: runs || commands.map(() => null),
  };
}

function json(route, body, status = 200) {
  return route.fulfill({ status, contentType: 'application/json', body: JSON.stringify(body) });
}

/**
 * API をすべてモックする。`state.cards` を書き換えると次の一覧から応答が変わる。
 * `calls` に「PWA が何をどの順で叩いたか」が溜まる。
 * `opts.runStatus` で実行の失敗（403 / 404 / 409）を再現できる
 */
async function setupMocks(page, opts = {}) {
  const state = {
    role: opts.role || 'interact',
    cards: opts.cards || [
      card(3, ['cargo test --workspace -j 4'], 'テストを回す'),
      card(4, ['npm run build', 'npm run e2e -- --workers=2']),
    ],
    runStatus: opts.runStatus || 200,
    runDelayMs: opts.runDelayMs || 0,
    nextPane: 40,
  };
  const calls = { list: [], run: [], pair: [], me: 0 };

  await page.route('**/api/me', route => {
    calls.me += 1;
    return json(route, me(state.role));
  });
  await page.route('**/api/pair', route => {
    calls.pair.push(route.request().postDataJSON() || {});
    return json(route, { status: 'pending' });
  });
  await page.route('**/api/v2/panes', route => json(route, { api_version: 2, panes: [PANE_INFO] }));
  await page.route('**/api/agents', route => json(route, { agents: [] }));
  await page.route('**/api/health', route => json(route, { status: 'ok', version: '0.8.12' }));
  await page.route('**/api/tasks**', route => json(route, { tasks: [], count: 0, open_count: 0 }));
  await page.route('**/ws?*', route => route.abort());
  await page.route('**/manifest.json', route => json(route, { name: 'tako remote' }));
  await page.route('**/sw.js', route =>
    route.fulfill({ status: 200, contentType: 'application/javascript', body: '' })
  );

  await page.route('**/api/cards**', async route => {
    const url = new URL(route.request().url());
    const method = route.request().method();
    if (method === 'GET' && url.pathname === '/api/cards') {
      calls.list.push(url.searchParams.get('pane'));
      return json(route, { pane: PANE, cards: state.cards, total: state.cards.length });
    }
    const m = url.pathname.match(/^\/api\/cards\/(\d+)\/run$/);
    if (method === 'POST' && m) {
      const id = Number(m[1]);
      const body = route.request().postDataJSON() || {};
      calls.run.push({ id, body });
      if (state.runDelayMs) await new Promise(r => setTimeout(r, state.runDelayMs));
      if (state.runStatus === 403) {
        state.role = 'observe';
        return json(route, { error: 'この操作には interact 以上の role が必要（現在: observe）' }, 403);
      }
      if (state.runStatus === 404) {
        state.cards = state.cards.filter(c => c.id !== id);
        return json(route, { error: `カードが見つからない（id=${id}）`, kind: 'not_found' }, 404);
      }
      if (state.runStatus === 409) {
        return json(route, {
          error: 'このコマンドはまだ実行中（1 件目・ペイン 31）。終わるか、そのペインを閉じてからもう一度実行する',
          kind: 'still_running',
        }, 409);
      }
      const target = state.cards.find(c => c.id === id);
      const index = body.index || 1;
      const pane = state.nextPane++;
      const run = { pane, state: 'running', exit_code: null, count: 1 };
      target.runs = target.runs.map((r, i) => (i === index - 1 ? run : r));
      return json(route, { pane, from_pane: PANE, card: id, index, run });
    }
    return json(route, { error: 'API エンドポイントが見つからない' }, 404);
  });

  return { state, calls };
}

async function openPane(page) {
  await page.goto(`${BASE}/#/panes/${PANE}`);
  await page.waitForSelector('[data-testid="command-card"]', { timeout: 10000 });
}

test.describe('#1724 スマホからコマンドカードを実行する — モバイル', () => {
  test.use({ viewport: IPHONE_VIEWPORT });

  test('01. そのペインのカードが一覧で出る（ラベル・全文・件数）', async ({ page }) => {
    const { calls } = await setupMocks(page);
    await openPane(page);
    const cards = page.locator('[data-testid="command-card"]');
    await expect(cards).toHaveCount(2);
    await expect(page.locator('[data-testid="command-cards-count"]')).toHaveText('2');
    await expect(cards.first()).toContainText('テストを回す');
    await expect(cards.first()).toContainText('cargo test --workspace -j 4');
    // ラベルの無いカードは PC と同じ既定の見出し
    await expect(cards.nth(1)).toContainText('実行するコマンド');
    await expect(cards.nth(1)).toContainText('1/2 件目');
    await expect(cards.nth(1)).toContainText('npm run e2e -- --workers=2');
    await expect(page.locator('[data-testid="command-run"]')).toHaveCount(3);
    // 一覧は**このペイン**を名指して引く
    expect(calls.list.every(p => p === String(PANE))).toBe(true);
    await page.screenshot({ path: `${EVIDENCE_DIR}/01-cards.png`, fullPage: true });
  });

  test('02. interact: 実行 → 確認（全文）→ 1 回だけ撃ち、記録が出る', async ({ page }) => {
    const { state, calls } = await setupMocks(page);
    await openPane(page);
    await page.locator('[data-testid="command-run"]').first().click();
    const sheet = page.locator('[data-testid="command-confirm"]');
    await expect(sheet).toBeVisible();
    // 全文を見せてから撃つ
    await expect(page.locator('[data-testid="command-confirm-text"]')).toHaveText('cargo test --workspace -j 4');
    await expect(sheet).toContainText('テストを回す');
    await expect(sheet).toContainText('test-mac の同じタブに新しいペインを開いて実行します');
    await page.screenshot({ path: `${EVIDENCE_DIR}/02-confirm.png`, fullPage: true });
    expect(calls.run).toHaveLength(0);

    await page.locator('[data-testid="command-confirm-run"]').click();
    await expect(sheet).toHaveCount(0);
    await expect(page.locator('[data-testid="command-notice"]')).toHaveText('PC のペイン 40 で実行を始めました');
    // **送るのは番号だけ**（本文・ペイン・focus を送らない = 走る文字列は PC のカードのもの）
    expect(calls.run).toEqual([{ id: 3, body: { index: 1 } }]);
    // 実行記録は PC と同じ語彙。走っているあいだは押せない
    const block = page.locator('[data-testid="command-block"]').first();
    await expect(block.locator('[data-testid="command-run-state"]')).toHaveText('実行中');
    await expect(block.locator('[data-testid="command-run"]')).toBeDisabled();
    await page.screenshot({ path: `${EVIDENCE_DIR}/03-running.png`, fullPage: true });

    // PC 側で終わった（一覧の記録が exited へ）→ 次の一覧で揃う・また押せる
    state.cards[0].runs = [{ pane: 40, state: 'exited', exit_code: 0, count: 1 }];
    await expect(block.locator('[data-testid="command-run-state"]'))
      .toHaveText('実行済み（終了コード 0）', { timeout: 12000 });
    await expect(block.locator('[data-testid="command-run"]')).toBeEnabled();
    await page.screenshot({ path: `${EVIDENCE_DIR}/04-exited.png`, fullPage: true });
  });

  test('03. 確認で「やめる」なら撃たない', async ({ page }) => {
    const { calls } = await setupMocks(page);
    await openPane(page);
    await page.locator('[data-testid="command-run"]').nth(2).click();
    await expect(page.locator('[data-testid="command-confirm-text"]')).toHaveText('npm run e2e -- --workers=2');
    await expect(page.locator('[data-testid="command-confirm"]')).toContainText('2/2 件目');
    await page.getByRole('button', { name: 'やめる' }).click();
    await expect(page.locator('[data-testid="command-confirm"]')).toHaveCount(0);
    // 背景のタップでも閉じる（撃たない）
    await page.locator('[data-testid="command-run"]').first().click();
    await page.locator('.sheet-backdrop').click({ position: { x: 10, y: 10 } });
    await expect(page.locator('[data-testid="command-confirm"]')).toHaveCount(0);
    expect(calls.run).toHaveLength(0);
  });

  test('04. observe: 実行が出ず、権限リクエストへつながる', async ({ page }) => {
    const { calls } = await setupMocks(page, { role: 'observe' });
    await openPane(page);
    // カードは見える（Issue の記載）が、実行ボタンは 1 つも無い
    await expect(page.locator('[data-testid="command-card"]')).toHaveCount(2);
    await expect(page.locator('[data-testid="command-run"]')).toHaveCount(0);
    const link = page.locator('[data-testid="command-run-permission"]');
    await expect(link).toContainText('interact 以上');
    await expect(link).toContainText('observe');
    await page.screenshot({ path: `${EVIDENCE_DIR}/05-observe.png`, fullPage: true });

    await link.click();
    await expect(page.locator('[data-testid="permission-request"]')).toBeVisible();
    await page.locator('[data-testid="permission-send"]').click();
    await expect(page.locator('[data-testid="permission-pending"]')).toBeVisible();
    // 既存の `/api/pair` だけが飛ぶ（新しい経路を生やさない = #1452）
    expect(calls.pair).toHaveLength(1);
    expect(calls.pair[0].role).toBe('interact');
    expect(calls.run).toHaveLength(0);
    await page.screenshot({ path: `${EVIDENCE_DIR}/06-permission.png`, fullPage: true });
  });

  test('05. 実行中は押せない・古い一覧から押しても 409 の理由が出る', async ({ page }) => {
    const { calls } = await setupMocks(page, {
      cards: [
        card(5, ['npm run dev'], 'dev サーバー', [{ pane: 31, state: 'running', exit_code: null, count: 1 }]),
        card(6, ['make'], null),
      ],
      runStatus: 409,
    });
    await openPane(page);
    const first = page.locator('[data-testid="command-block"]').first();
    await expect(first.locator('[data-testid="command-run"]')).toBeDisabled();
    await expect(first.locator('[data-testid="command-run"]')).toHaveText('実行中');
    // 2 枚目は一覧では走っていないが、PC 側では走っている（古い一覧）= daemon が 409 で断る
    await page.locator('[data-testid="command-run"]').nth(1).click();
    await page.locator('[data-testid="command-confirm-run"]').click();
    await expect(page.locator('[data-testid="command-notice"]')).toContainText('このコマンドはまだ実行中');
    await expect(page.locator('[data-testid="command-confirm"]')).toHaveCount(0);
    expect(calls.run).toHaveLength(1);
  });

  test('06. PC 側で閉じられたカードを押すと理由が残る', async ({ page }) => {
    await setupMocks(page, { cards: [card(8, ['ls -la'], '一覧')], runStatus: 404 });
    await openPane(page);
    await page.locator('[data-testid="command-run"]').click();
    await page.locator('[data-testid="command-confirm-run"]').click();
    // 一覧からカードが消えても、押した結果は残る（黙って全部消えない）
    await expect(page.locator('[data-testid="command-card"]')).toHaveCount(0);
    await expect(page.locator('[data-testid="command-notice"]')).toHaveText(/このカードは PC 側で閉じられました/);
    await page.screenshot({ path: `${EVIDENCE_DIR}/07-closed.png`, fullPage: true });
    // 閉じれば節ごと消える
    await page.locator('.cmd-notice-close').click();
    await expect(page.locator('[data-testid="command-cards"]')).toHaveCount(0);
  });

  test('07. 降格された直後に押すと 403 → 権限の導線へ切り替わる', async ({ page }) => {
    const { calls } = await setupMocks(page, { runStatus: 403 });
    await openPane(page);
    const meBefore = calls.me;
    await page.locator('[data-testid="command-run"]').first().click();
    await page.locator('[data-testid="command-confirm-run"]').click();
    await expect(page.locator('[data-testid="command-notice"]')).toContainText('この端末の権限では実行できません');
    // me を取り直し、observe になった画面では実行ボタンが消えて導線が出る
    await expect(page.locator('[data-testid="command-run"]')).toHaveCount(0);
    await expect(page.locator('[data-testid="command-run-permission"]')).toBeVisible();
    expect(calls.me).toBeGreaterThan(meBefore);
  });

  test('08. 確認の二度押しでも 1 回しか撃たない', async ({ page }) => {
    const { calls } = await setupMocks(page, { runDelayMs: 800 });
    await openPane(page);
    await page.locator('[data-testid="command-run"]').first().click();
    const go = page.locator('[data-testid="command-confirm-run"]');
    await go.click();
    // 送信中はボタンが止まる（押しても何も起きない）
    await expect(go).toBeDisabled();
    await expect(go).toHaveText('送信中…');
    await go.click({ force: true });
    await expect(page.locator('[data-testid="command-notice"]')).toHaveText('PC のペイン 40 で実行を始めました');
    expect(calls.run).toHaveLength(1);
  });
});
