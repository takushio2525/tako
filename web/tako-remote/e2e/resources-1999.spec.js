// Issue #1999（エピック #1992 の S7）: スマホ（PWA）から PC の負荷と順番待ちを見る・優先度を変える。
//
// 受け入れ条件を、モバイル viewport の実 DOM で確かめる:
//   ① `#/` に機の状態のカード（メモリ・メモリ圧・CPU・「実行 N · 待ち M」）。押すと `#/resources`
//   ② `#/resources` に実行中と待ち（誰 / 何 / 進み / 見込み / 優先度）が daemon の答えの並びのまま出る
//   ③ manage: 段のチップ（この処理 / ペイン N の既定）と取り消しの確かめが CLI と同じ要求を撃つ
//   ④ read（observe）/ interact: 変更の口が出ず、#1452 の権限リクエストへ繋がる
//   ⑤ 押す前に終わった処理（daemon の 404 `gone`）は理由が出る / GUI 不在（`gui: false`）は注記が出る
//   ⑥ 絵文字が無い・スマホ幅（390 / 320px）で横スクロールが出ない
//   ⑦ 画面を離れるとポーリングが止まる
//
// **実 PC も実 daemon も一切見ない**（#927）。ペイン名・コマンドの表示名はすべて偽物で、
// 実ユーザー名・実ホームパスはスクショにも入らない。実 daemon と CLI の突き合わせは
// `scripts/test-remote-resources-1999.sh`（と同じ dir の `resources-1999-live.spec.js`）が見る。
//
// 実行:
//   cd web/tako-remote && npx playwright test e2e/resources-1999.spec.js
import { test, expect } from '@playwright/test';
import { evidencePath, TAKO_VERSION } from './support.js';

const IPHONE_VIEWPORT = { width: 390, height: 844 };
const NARROW_VIEWPORT = { width: 320, height: 640 };
const BASE = `http://localhost:${process.env.TAKO_PWA_PORT || 5174}`;

function me(role = 'manage') {
  return {
    registered: true, device_id: 'test-iphone', name: 'iPhone', role,
    login: 'user@example.com', host: 'test-mac', version: TAKO_VERSION, app_connected: true,
  };
}

const MACHINE = {
  memory_mb: { total: 36864, used: 24710, app: 12000, wired: 5000, compressed: 6100, cached: 4000, available: 9800 },
  swap_mb: { used: 2048, total: 8192 },
  pressure: 'warn',
  cpu_percent: 62.4,
  load: [7.12, 6.05, 5.5],
  cpus: 18,
  age_ms: 400,
};

// 折り返しの無い長い表示名（横スクロールの検査用。偽のパスだけ）
const LONG_LABEL = 'cargo test -p tako-control --features very-long-feature-name-for-wrapping-check '
  + '--manifest-path=/Users/testuser/dev/some/really/long/path/without/spaces/Cargo.toml';

function running() {
  return {
    id: 4101, label: 'cargo test -p tako-core', kind: 'build', pane: 7, phase: 'running',
    memory_mb: 2700, cpu_percent: 99.6, stalled: false, cancel: false, ran_s: 121, waited_s: 0,
    progress: { done: 120, total: 340 },
    eta: { state: 'remaining', remaining_s: 180, previous_s: 420, from: 'history', text: 'あと約 3 分（前回 7 分）' },
    priority: 'high', base_priority: 'high', priority_from: 'role', aged: false, job_priority: null,
    tab: 'tako', title: 'master-takodev', role: 'orchestrator-master:takodev', pane_priority: null,
  };
}

function waitingRows() {
  return [
    {
      id: 4102, position: 1, label: 'cargo build --release', kind: 'build', pane: 9, waited_s: 63,
      reason: 'max_jobs', text: '同時に走らせる数の上限（1）に達している', start_s: 180, start_text: '開始まで約 3 分',
      priority: 'normal', base_priority: 'normal', priority_from: 'role', aged: false, job_priority: null,
      tab: 'worker', title: 'fix-1999', role: 'orchestrator-worker-claude:fix-1999', pane_priority: null,
    },
    {
      id: 4103, position: 2, label: LONG_LABEL, kind: 'build', pane: null, waited_s: 3,
      reason: 'ahead', text: '先に並んだ処理の後ろ', start_s: null, start_text: null,
      priority: 'low', base_priority: 'low', priority_from: 'role', aged: false, job_priority: null,
    },
  ];
}

function json(route, body, status = 200) {
  return route.fulfill({ status, contentType: 'application/json', body: JSON.stringify(body) });
}

/**
 * API をすべてモックする。`calls` に「PWA が何をどの順で叩いたか」が溜まる。
 * 変更の要求は mock の中の行へ当てる（次の読みで並びが変わる = 実 daemon と同じ見え方）
 */
async function setupMocks(page, opts = {}) {
  const { role = 'manage', gui = true, gone = false, eta = null } = opts;
  const calls = { status: 0, jobPriority: [], panePriority: [], cancel: [] };
  let run = [{ ...running(), ...(eta ? { eta, progress: null } : {}) }];
  let wait = waitingRows();

  await page.route('**/api/me', route => json(route, me(role)));
  await page.route('**/api/v2/panes', route => json(route, {
    api_version: 2,
    panes: [{ id: 7, title: 'master-takodev', tab_id: 1, tab_title: 'tako', role: 'orchestrator-master:takodev', agent_type: 'claude', activity: 'busy', preview: ['working...'], tmux_target: 'tako-x:0.0' }],
  }));
  await page.route('**/api/agents', route => json(route, { agents: [] }));
  await page.route('**/api/tasks**', route => json(route, { tasks: [], count: 0, open_count: 0, open_counts_by_kind: [] }));
  await page.route('**/api/health', route => json(route, { status: 'ok', version: TAKO_VERSION }));
  await page.route('**/ws?*', route => route.abort());
  await page.route('**/manifest.json', route => json(route, { name: 'tako remote' }));
  await page.route('**/sw.js', route =>
    route.fulfill({ status: 200, contentType: 'application/javascript', body: '' })
  );

  await page.route('**/api/resources**', async route => {
    const url = new URL(route.request().url());
    const method = route.request().method();
    const path = url.pathname;
    if (method === 'GET' && path === '/api/resources') {
      calls.status++;
      if (!gui) {
        return json(route, { gui: false, machine: MACHINE, queue: null, note: '順番待ちは無効（tako が起動していない）', reason: 'tako app が稼働していない' });
      }
      return json(route, {
        gui: true,
        machine: MACHINE,
        queue: {
          arbiter: true, role: 'production', limits: { max_jobs: 1 },
          running: run, waiting: wait, blocked: null, unmanaged: 0,
        },
      });
    }
    // 権限は daemon が正（ここでは manage 未満の変更を 403 で返す = 実 daemon と同じ）
    if (method === 'POST' && !['manage', 'admin'].includes(role)) {
      return json(route, { error: 'この操作には manage 以上の権限が必要（現在: observe）' }, 403);
    }
    let m = path.match(/^\/api\/resources\/jobs\/(\d+)\/priority$/);
    if (method === 'POST' && m) {
      const body = route.request().postDataJSON();
      calls.jobPriority.push({ id: Number(m[1]), priority: body.priority });
      if (gone) return json(route, { error: `受付番号 ${m[1]} は順番待ちに居ない（\`tako queue\` で一覧）`, kind: 'gone' }, 404);
      // 上げた行を待ちの先頭へ（実 daemon の並び直しの見え方）
      const id = Number(m[1]);
      const row = wait.find(j => j.id === id);
      if (row) {
        row.priority = body.priority; row.job_priority = body.priority; row.priority_from = 'job';
        wait = [row, ...wait.filter(j => j.id !== id)].map((j, i) => ({ ...j, position: i + 1 }));
      }
      return json(route, { id, label: row ? row.label : '', phase: 'waiting', changed: true, priority: body.priority, position: 1 });
    }
    m = path.match(/^\/api\/resources\/panes\/(\d+)\/priority$/);
    if (method === 'POST' && m) {
      const body = route.request().postDataJSON();
      calls.panePriority.push({ pane: Number(m[1]), priority: body.priority });
      for (const j of [...run, ...wait]) if (j.pane === Number(m[1])) j.pane_priority = body.priority;
      return json(route, { pane: Number(m[1]), priority: body.priority, jobs: [] });
    }
    m = path.match(/^\/api\/resources\/jobs\/(\d+)\/cancel$/);
    if (method === 'POST' && m) {
      const id = Number(m[1]);
      calls.cancel.push(id);
      run = run.filter(j => j.id !== id);
      wait = wait.filter(j => j.id !== id).map((j, i) => ({ ...j, position: i + 1 }));
      return json(route, { id, label: 'x', phase: 'waiting', cancelled: true });
    }
    return json(route, { error: 'not found' }, 404);
  });
  return calls;
}

async function noHorizontalScroll(page) {
  return page.evaluate(() => {
    const doc = document.documentElement;
    const scroller = document.querySelector('.res-scroll, .card-list');
    const over = [doc, document.body, scroller].filter(Boolean)
      .filter(el => el.scrollWidth > el.clientWidth + 1)
      .map(el => `${el.className || el.tagName}: ${el.scrollWidth} > ${el.clientWidth}`);
    // 子のどれかが画面の右へはみ出していないか（overflow: hidden で隠れていても見る）
    const vw = window.innerWidth;
    const sticking = [...document.querySelectorAll('.machine-card, .job-row, .prio-panel, .cancel-confirm, .res-section')]
      .filter(el => el.getBoundingClientRect().right > vw + 0.5)
      .map(el => `${el.className}: right ${el.getBoundingClientRect().right} > ${vw}`);
    return [...over, ...sticking];
  });
}

async function noEmoji(page) {
  return page.evaluate(() => {
    const text = document.body.innerText;
    const hits = text.match(/\p{Extended_Pictographic}/gu);
    return hits ? [...new Set(hits)] : [];
  });
}

test.describe('#1999 PC の負荷と順番待ち', () => {
  test('① #/ の機のカードから #/resources へ', async ({ page }) => {
    await page.setViewportSize(IPHONE_VIEWPORT);
    await setupMocks(page);
    await page.goto(`${BASE}/#/`);
    const card = page.getByTestId('machine-card');
    await expect(card).toBeVisible();
    await expect(card.getByTestId('machine-memory')).toHaveText('24.1 GB / 36.0 GB');
    await expect(card.getByTestId('machine-pressure')).toHaveText('メモリ圧 warn');
    await expect(card.getByTestId('machine-card-counts')).toHaveText('実行 1 · 待ち 2');
    // メモリ圧 warn のバーは amber（PC のステータスバーの区画と同じ割り当て）
    await expect(card.getByTestId('res-memory-bar')).toHaveClass(/tone-warn/);
    expect(await noEmoji(page)).toEqual([]);
    expect(await noHorizontalScroll(page)).toEqual([]);
    await page.screenshot({ path: evidencePath('01-home-card.png'), fullPage: true });
    await card.click();
    await expect(page).toHaveURL(/#\/resources$/);
    await expect(page.getByTestId('resources-page')).toBeVisible();
  });

  test('② 実行中と待ちが daemon の答えの並びで出る（誰 / 何 / 進み / 見込み / 優先度）', async ({ page }) => {
    await page.setViewportSize(IPHONE_VIEWPORT);
    await setupMocks(page);
    await page.goto(`${BASE}/#/resources`);
    await expect(page.getByTestId('running-title')).toHaveText('実行中 1 / 上限 1');
    await expect(page.getByTestId('waiting-title')).toHaveText('待ち 2');
    const run = page.getByTestId('running-row');
    await expect(run).toHaveCount(1);
    await expect(run.getByTestId('job-label')).toHaveText('cargo test -p tako-core');
    await expect(run.getByTestId('job-owner')).toContainText('ペイン 7 · tako · master-takodev · orchestrator-master:takodev');
    await expect(run.getByTestId('job-progress')).toContainText('120/340');
    await expect(run.getByTestId('job-eta')).toHaveText('あと約 3 分（前回 7 分）');
    await expect(run.getByTestId('prio-chip')).toHaveText('high');
    await expect(run).toContainText('役割の既定');
    // 待ちは受付の並び（position）のまま
    const ids = await page.getByTestId('waiting-row').evaluateAll(rows => rows.map(r => r.dataset.jobId));
    expect(ids).toEqual(['4102', '4103']);
    const first = page.getByTestId('waiting-row').nth(0);
    await expect(first.getByTestId('job-position')).toHaveText('1 番目');
    await expect(first.getByTestId('job-reason')).toHaveText('同時に走らせる数の上限（1）に達している');
    await expect(first).toContainText('開始まで約 3 分');
    await expect(page.getByTestId('waiting-row').nth(1)).toContainText('ペインの外');
    expect(await noEmoji(page)).toEqual([]);
    expect(await noHorizontalScroll(page)).toEqual([]);
    await page.screenshot({ path: evidencePath('02-resources.png'), fullPage: true });
  });

  test('③ manage: 段のチップと取り消しが CLI と同じ要求を撃つ', async ({ page }) => {
    await page.setViewportSize(IPHONE_VIEWPORT);
    const calls = await setupMocks(page);
    await page.goto(`${BASE}/#/resources`);
    const second = page.locator('[data-testid="waiting-row"][data-job-id="4103"]');
    await second.getByTestId('prio-toggle').click();
    const panel = second.getByTestId('prio-panel');
    await expect(panel).toBeVisible();
    // ペインの外の処理には「ペイン N の既定」の列が無い
    await expect(panel.getByTestId('prio-pane')).toHaveCount(0);
    await expect(panel.getByTestId('prio-job').locator('button.selected')).toHaveText('default');
    expect(await noHorizontalScroll(page)).toEqual([]);
    await panel.scrollIntoViewIfNeeded();
    await page.screenshot({ path: evidencePath('03-priority-panel.png'), fullPage: true });
    await panel.getByTestId('prio-job').getByRole('button', { name: 'top' }).click();
    await expect(page.getByTestId('res-notice')).toHaveText('優先度を top にした（待ち 1 番目）');
    expect(calls.jobPriority).toEqual([{ id: 4103, priority: 'top' }]);
    // 次の読みで並びが変わる（上げた処理が先頭）
    await expect.poll(async () =>
      page.getByTestId('waiting-row').evaluateAll(rows => rows.map(r => r.dataset.jobId))
    ).toEqual(['4103', '4102']);
    await expect(page.locator('[data-job-id="4103"]').getByTestId('prio-chip')).toHaveText('top');
    await expect(page.locator('[data-job-id="4103"]')).toContainText('処理に付けた段');

    // ペインの既定（実行中の行 = ペイン 7）。走っている処理は止めない旨を添える
    const run = page.getByTestId('running-row');
    await run.getByTestId('prio-toggle').click();
    await expect(run.getByTestId('prio-panel')).toContainText('走っている処理は止めない');
    await run.getByTestId('prio-pane').getByRole('button', { name: 'low' }).click();
    await expect(page.getByTestId('res-notice')).toHaveText('ペイン 7 の優先度の既定を low にした');
    expect(calls.panePriority).toEqual([{ pane: 7, priority: 'low' }]);

    // 取り消し: 「やめる」では撃たない・「取り消す」で撃つ
    const w = page.locator('[data-job-id="4102"]');
    await w.getByTestId('cancel-toggle').click();
    await expect(w.getByTestId('cancel-confirm')).toContainText('「cargo build --release」を待ちから外しますか？');
    await w.getByTestId('cancel-confirm').scrollIntoViewIfNeeded();
    await page.screenshot({ path: evidencePath('04-cancel-confirm.png'), fullPage: true });
    await w.getByTestId('cancel-no').click();
    await expect(w.getByTestId('cancel-confirm')).toHaveCount(0);
    expect(calls.cancel).toEqual([]);
    await w.getByTestId('cancel-toggle').click();
    await w.getByTestId('cancel-yes').click();
    await expect(page.getByTestId('res-notice')).toHaveText('「cargo build --release」を取り消した');
    expect(calls.cancel).toEqual([4102]);
    await expect(page.locator('[data-job-id="4102"]')).toHaveCount(0);
    await expect(page.getByTestId('waiting-title')).toHaveText('待ち 1');
    // 実行中の取り消しは「止めて」と言う
    await run.getByTestId('cancel-toggle').click();
    await expect(run.getByTestId('cancel-confirm')).toContainText('「cargo test -p tako-core」を止めて取り消しますか？');
    expect(await noEmoji(page)).toEqual([]);
  });

  for (const role of ['observe', 'interact']) {
    test(`④ ${role}: 変更の口が出ず、権限リクエストへ繋がる`, async ({ page }) => {
      await page.setViewportSize(IPHONE_VIEWPORT);
      const calls = await setupMocks(page, { role });
      await page.goto(`${BASE}/#/resources`);
      await expect(page.getByTestId('running-row')).toHaveCount(1);
      await expect(page.getByTestId('waiting-row')).toHaveCount(2);
      // 読めるが、段のチップ・取り消しは 1 つも無い
      await expect(page.getByTestId('prio-toggle')).toHaveCount(0);
      await expect(page.getByTestId('cancel-toggle')).toHaveCount(0);
      await expect(page.getByTestId('resources-permission')).toContainText('優先度の変更と取り消し');
      expect(calls.jobPriority).toEqual([]);
      expect(calls.cancel).toEqual([]);
      if (role === 'observe') {
        expect(await noHorizontalScroll(page)).toEqual([]);
        await page.getByTestId('resources-permission').scrollIntoViewIfNeeded();
        await page.screenshot({ path: evidencePath('05-observe.png'), fullPage: true });
      }
    });
  }

  test('⑤ 押す前に終わった処理と GUI 不在は理由が出る', async ({ page }) => {
    await page.setViewportSize(IPHONE_VIEWPORT);
    await setupMocks(page, { gone: true });
    await page.goto(`${BASE}/#/resources`);
    const row = page.locator('[data-job-id="4102"]');
    await row.getByTestId('prio-toggle').click();
    await row.getByTestId('prio-job').getByRole('button', { name: 'high' }).click();
    await expect(page.getByTestId('res-notice')).toHaveText('「cargo build --release」はもう順番待ちに居ない');
    await expect(page.getByTestId('res-notice')).toHaveClass(/error/);

    await page.unrouteAll({ behavior: 'ignoreErrors' });
    await setupMocks(page, { gui: false });
    await page.goto(`${BASE}/#/resources`);
    await page.reload();
    await expect(page.getByTestId('machine-offline')).toHaveText('順番待ちは無効（tako が起動していない）');
    await expect(page.getByTestId('machine-card-full').getByTestId('machine-memory')).toHaveText('24.1 GB / 36.0 GB');
    await expect(page.getByTestId('running-row')).toHaveCount(0);
    await expect(page.getByTestId('running-title')).toHaveCount(0);
  });

  test('⑤b 見込みの文は経過を繰り返さない（調停役の eta.text ではなく欄から組む）', async ({ page }) => {
    await page.setViewportSize(IPHONE_VIEWPORT);
    for (const [eta, want] of [
      [{ state: 'unknown', remaining_s: null, previous_s: null, from: null, text: '経過 2 分 1 秒（見込みなし）' }, '見込みなし'],
      [{ state: 'overrun', remaining_s: null, previous_s: 90, from: 'history', text: '前回より長くかかっている（経過 2 分 1 秒・前回 2 分）' }, '前回より長くかかっている（前回 2 分）'],
      [{ state: 'remaining', remaining_s: 45, previous_s: null, from: 'history', text: 'あと約 45 秒' }, 'あと約 45 秒'],
    ]) {
      await page.unrouteAll({ behavior: 'ignoreErrors' });
      await setupMocks(page, { eta });
      await page.goto(`${BASE}/#/resources`);
      await page.reload();
      const run = page.getByTestId('running-row');
      await expect(run.getByTestId('job-eta')).toHaveText(want);
      // 経過は数値の行の 1 か所だけ
      expect((await run.textContent()).match(/経過/g)).toHaveLength(1);
    }
  });

  test('⑥ 320px でも横スクロールが出ない（段の列と確かめを開いたまま）', async ({ page }) => {
    await page.setViewportSize(NARROW_VIEWPORT);
    await setupMocks(page);
    await page.goto(`${BASE}/#/`);
    await expect(page.getByTestId('machine-card')).toBeVisible();
    expect(await noHorizontalScroll(page)).toEqual([]);
    await page.goto(`${BASE}/#/resources`);
    const run = page.getByTestId('running-row');
    await run.getByTestId('prio-toggle').click();
    await page.locator('[data-job-id="4103"]').getByTestId('cancel-toggle').click();
    await expect(page.getByTestId('prio-panel')).toBeVisible();
    await expect(page.getByTestId('cancel-confirm')).toBeVisible();
    expect(await noHorizontalScroll(page)).toEqual([]);
    expect(await noEmoji(page)).toEqual([]);
    await page.getByTestId('cancel-confirm').scrollIntoViewIfNeeded();
    await page.screenshot({ path: evidencePath('06-narrow-320.png'), fullPage: true });
  });

  test('⑦ 画面を離れるとポーリングが止まる', async ({ page }) => {
    await page.setViewportSize(IPHONE_VIEWPORT);
    const calls = await setupMocks(page);
    await page.goto(`${BASE}/#/resources`);
    await expect.poll(() => calls.status, { timeout: 10000 }).toBeGreaterThanOrEqual(2);
    // ファイル画面へ移る（`#/` には機のカードの読みがあるので、それを持たない画面へ）
    await page.route('**/api/files**', route => json(route, { roots: [], entries: [] }));
    await page.evaluate(() => { window.location.hash = '#/files'; });
    await page.waitForTimeout(300);
    const after = calls.status;
    await page.waitForTimeout(3500);
    expect(calls.status).toBe(after);
  });
});
