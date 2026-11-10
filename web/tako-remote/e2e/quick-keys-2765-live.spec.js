// Issue #2765: **実 daemon** に繋いだ PWA の term ビューのクイックキー（esc / tab / ^C / ↑ / ↓）を押し、
// daemon の応答（状態コード）と PWA が送った本文を書き出す。
//
// `scripts/test-remote-keys-2765.sh` だけが env を渡して走らせる（普段の `npm run e2e` では skip）。
// あのスクリプトは隔離 GUI（器のペイン）・隔離 daemon・ペインの記録係（raw の端末が受けたバイトを
// 16 進でファイルへ書く）を立て、この spec が押した順のバイトがペインへ届いたかを記録で確かめる。
//
// 経路: PWA は vite の dev サーバーから配り、`/api/*` だけを Playwright が実 daemon へ中継する
// （tailscale serve が足すのと同じ identity ヘッダを付ける。#1999 の live spec と同じ形）。
// 押した結果は `TAKO_2765_KEY <JSON>` の 1 行で標準出力へ出す（spec からファイルを書かない = #1749）。
//
// env:
//   TAKO_2765_LIVE_BASE  実 daemon の待ち受け（`http://127.0.0.1:<port>`）
//   TAKO_2765_LIVE_HOST  serve の名前（X-Forwarded-Host）
//   TAKO_2765_LIVE_PANE  押す先のペイン ID（記録係のペイン）
import { test, expect } from '@playwright/test';
import { evidencePath } from './support.js';

const LIVE = process.env.TAKO_2765_LIVE_BASE || '';
const HOST = process.env.TAKO_2765_LIVE_HOST || '';
const PANE = process.env.TAKO_2765_LIVE_PANE || '';
const BASE = `http://localhost:${process.env.TAKO_PWA_PORT || 5174}`;
const IPHONE_VIEWPORT = { width: 390, height: 844 };

test.skip(!LIVE, '実 daemon との通しは scripts/test-remote-keys-2765.sh から走らせる');

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

test('実 daemon の term ビューでクイックキーを押す', async ({ page }) => {
  await page.setViewportSize(IPHONE_VIEWPORT);
  await relay(page);
  await page.goto(`${BASE}/#/panes/${PANE}`);
  // シェルのペイン（会話が無い）は term ビューが既定。クイックキーは器のペインにだけ出る
  await expect(page.locator('.quick-keys')).toBeVisible({ timeout: 15000 });

  // 1 つ押すごとに daemon の応答まで待つ（押した順にペインへ届くことを記録で見るため）
  for (const label of ['esc', 'tab', '^C', '↑', '↓']) {
    const [resp] = await Promise.all([
      page.waitForResponse(r =>
        r.url().endsWith(`/api/panes/${PANE}/input`) && r.request().method() === 'POST'),
      page.getByRole('button', { name: label, exact: true }).click(),
    ]);
    console.log(`TAKO_2765_KEY ${JSON.stringify({
      label,
      status: resp.status(),
      sent: resp.request().postDataJSON(),
    })}`);
  }
  // 左上の機の名前は実機のホスト名（`/api/me` の host）。画像に写さない（#927）
  await page.evaluate(() => {
    for (const el of document.querySelectorAll('.machine-chip .chip-name')) el.textContent = '<host>';
  });
  await page.screenshot({ path: evidencePath('live-quick-keys.png') });
});
