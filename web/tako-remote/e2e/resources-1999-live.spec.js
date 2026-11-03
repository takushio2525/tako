// Issue #1999: **実 daemon** に繋いだ PWA の `#/resources` を読み、PC の `tako queue --json` と突き合わせる材料を書き出す。
//
// `scripts/test-remote-resources-1999.sh` だけが env を渡して走らせる（普段の `npm run e2e` では skip）。
// あのスクリプトは隔離 GUI（明示の調停役）・本物の wrapper（`tako queue sleep 600`）・隔離 daemon を立て、
// この spec が描いた行（受付番号・並び・段）を CLI の答えと比べる。PWA から変えた段が CLI で見えるかも
// スクリプトの側が CLI で確かめる。
//
// 経路: PWA は vite の dev サーバーから配り、`/api/*` だけを Playwright が実 daemon へ中継する
// （tailscale serve が足すのと同じ identity ヘッダを付ける。daemon の層①は接続元プロセスの名前を
// `TAKO_REMOTE_TRUSTED_PEER_NAMES` で信頼している = 隔離の偽 tailscale と同じ代理）。
//
// 行の JSON は `TAKO_1999_ROWS <名前> <JSON>` の 1 行で標準出力へ出す（spec からファイルを書かない = #1749。
// スクリプトが Playwright の出力から拾う）。画像は `evidencePath`（`TAKO_EVIDENCE_DIR` を渡せばその下）。
//
// env:
//   TAKO_1999_LIVE_BASE   実 daemon の待ち受け（`http://127.0.0.1:<port>`）
//   TAKO_1999_LIVE_HOST   serve の名前（X-Forwarded-Host）
//   TAKO_1999_LIVE_RAISE  段を top へ上げる受付番号（PWA の UI で押す）
import { test, expect } from '@playwright/test';
import { evidencePath } from './support.js';

const LIVE = process.env.TAKO_1999_LIVE_BASE || '';
const HOST = process.env.TAKO_1999_LIVE_HOST || '';
const RAISE = process.env.TAKO_1999_LIVE_RAISE || '';
const BASE = `http://localhost:${process.env.TAKO_PWA_PORT || 5174}`;
const IPHONE_VIEWPORT = { width: 390, height: 844 };

test.skip(!LIVE, '実 daemon との突き合わせは scripts/test-remote-resources-1999.sh から走らせる');

async function relay(page) {
  await page.route('**/api/**', async route => {
    const req = route.request();
    const url = new URL(req.url());
    const headers = { ...req.headers(), 'x-forwarded-for': '100.64.0.7', 'x-forwarded-host': HOST };
    delete headers.host;
    delete headers.origin;
    const resp = await route.fetch({ url: `${LIVE}${url.pathname}${url.search}`, headers });
    await route.fulfill({ response: resp });
  });
  await page.route('**/ws?*', route => route.abort());
  await page.route('**/sw.js', route =>
    route.fulfill({ status: 200, contentType: 'application/javascript', body: '' })
  );
}

function emit(name, value) {
  console.log(`TAKO_1999_ROWS ${name} ${JSON.stringify(value)}`);
}

// 画面に出ている行（実行中 → 待ちの順）の受付番号・番目・段
async function rows(page) {
  return page.evaluate(() => [...document.querySelectorAll('[data-testid="running-row"], [data-testid="waiting-row"]')]
    .map(r => ({
      phase: r.dataset.testid === 'running-row' ? 'running' : 'waiting',
      id: Number(r.dataset.jobId),
      position: (() => {
        const p = r.querySelector('[data-testid="job-position"]');
        return p ? Number(p.textContent.replace(/[^0-9]/g, '')) : null;
      })(),
      priority: r.querySelector('[data-testid="prio-chip"]').textContent,
      label: r.querySelector('[data-testid="job-label"]').textContent,
      owner: r.querySelector('[data-testid="job-owner"]').textContent.replace(/開く ›$/, ''),
    })));
}

test('実 daemon の #/resources を読み、PWA の UI から段を上げる', async ({ page }) => {
  await page.setViewportSize(IPHONE_VIEWPORT);
  await relay(page);

  // `#/` の機のカード（実 daemon の答え）
  await page.goto(`${BASE}/#/`);
  await expect(page.getByTestId('machine-card')).toBeVisible({ timeout: 15000 });
  await expect(page.getByTestId('machine-card-counts')).toBeVisible();
  emit('home', await page.getByTestId('machine-card-counts').textContent());
  // 左上の機の名前は実機のホスト名（`/api/me` の host）。画像に写さない（#927）
  await page.evaluate(() => {
    for (const el of document.querySelectorAll('.machine-chip .chip-name')) el.textContent = '<host>';
  });
  await page.screenshot({ path: evidencePath('live-01-home.png') });

  await page.getByTestId('machine-card').click();
  await expect(page.getByTestId('running-row').first()).toBeVisible({ timeout: 15000 });
  await expect(page.getByTestId('waiting-row').first()).toBeVisible();
  emit('before', await rows(page));
  await page.screenshot({ path: evidencePath('live-02-resources.png') });

  if (RAISE) {
    const row = page.locator(`[data-testid="waiting-row"][data-job-id="${RAISE}"]`);
    await row.getByTestId('prio-toggle').click();
    await row.getByTestId('prio-job').getByRole('button', { name: 'top' }).click();
    await expect(page.getByTestId('res-notice')).toHaveText('優先度を top にした（待ち 1 番目）');
    // 次の読みで上げた行が待ちの先頭へ来る（daemon → GUI の調停役の並び）
    await expect.poll(async () =>
      page.getByTestId('waiting-row').first().evaluate(r => r.dataset.jobId), { timeout: 15000 }
    ).toBe(String(RAISE));
    emit('after', await rows(page));
    await page.getByTestId('res-notice').scrollIntoViewIfNeeded();
    await page.screenshot({ path: evidencePath('live-03-raised.png') });
  }
});
