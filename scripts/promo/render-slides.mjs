// tako:run: node scripts/promo/render-slides.mjs
// 解説動画 v6（#1081）/ X 向けショート（#1284）のスライドを HTML から PNG へ描く。
//
// 使い方: node scripts/promo/render-slides.mjs [deck.html] [出力ディレクトリ]
//   既定: scripts/promo/slides/deck.html → ~/Desktop/tako-promo/slides/
//
// 描画は web/tako-remote が持っている Playwright の Chromium を使う（新規依存を入れない）。
// 1 section.slide = 1 枚 = 1920x1080。ファイル名は section の id。
// フォントの読み込み完了を待ってから撮る（待たないと日本語がフォールバックで描かれる）。
import { createRequire } from 'node:module';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const HERE = path.dirname(fileURLToPath(import.meta.url));
const REPO = path.resolve(HERE, '../..');
// Playwright は web/tako-remote の node_modules にある（record-pwa.cjs と同じ解決）
const require = createRequire(path.join(REPO, 'web/tako-remote/package.json'));
const { chromium } = require('@playwright/test');

const DECK = process.argv[2] || path.join(HERE, 'slides', 'deck.html');
const OUT = process.argv[3]
    || path.join(process.env.TAKO_PROMO_OUT || path.join(os.homedir(), 'Desktop', 'tako-promo'), 'slides');
const W = 1920, H = 1080;

if (!fs.existsSync(DECK)) { console.error(`ERROR: デッキが無い: ${DECK}`); process.exit(1); }
fs.mkdirSync(OUT, { recursive: true });

// Playwright 同梱の Chromium は実機に落ちていないので、**実機の Google Chrome を使う**
// （動画のためだけに 150MB のブラウザを落とさない）。TAKO_SLIDE_CHROME で明示もできる。
const launchOpts = process.env.TAKO_SLIDE_CHROME
    ? { executablePath: process.env.TAKO_SLIDE_CHROME }
    : { channel: 'chrome' };
const browser = await chromium.launch(launchOpts);
console.log(`   描画: ${process.env.TAKO_SLIDE_CHROME || 'channel=chrome（実機の Google Chrome）'}`);
const page = await browser.newPage({ viewport: { width: W, height: H }, deviceScaleFactor: 1 });
await page.goto('file://' + DECK);
await page.evaluate(() => document.fonts.ready);

const ids = await page.$$eval('section.slide', els => els.map(e => e.id));
if (ids.length === 0) { console.error('ERROR: section.slide が 1 つも無い'); await browser.close(); process.exit(1); }

// 実寸が 1920x1080 でない枚があると動画側で拡縮が起きるので、撮る前に必ず検査する
const bad = await page.$$eval('section.slide', (els, [w, h]) => els
    .map(e => ({ id: e.id, w: Math.round(e.getBoundingClientRect().width), h: Math.round(e.getBoundingClientRect().height) }))
    .filter(s => s.w !== w || s.h !== h), [W, H]);
if (bad.length) {
    console.error('ERROR: 実寸が 1920x1080 でないスライド:');
    for (const b of bad) console.error(`  ${b.id}: ${b.w}x${b.h}`);
    await browser.close(); process.exit(1);
}

// 中身がはみ出している枚も落とす（テキストが箱から出たまま気づかず完成品まで進むのを防ぐ）
const overflow = await page.$$eval('section.slide', els => els
    .filter(e => e.scrollWidth > e.clientWidth + 1 || e.scrollHeight > e.clientHeight + 1)
    .map(e => `${e.id}: ${e.scrollWidth}x${e.scrollHeight} > ${e.clientWidth}x${e.clientHeight}`));
if (overflow.length) {
    console.error('ERROR: 中身がはみ出しているスライド:');
    for (const o of overflow) console.error(`  ${o}`);
    await browser.close(); process.exit(1);
}

for (const id of ids) {
    const el = await page.$(`#${id}`);
    await el.screenshot({ path: path.join(OUT, `${id}.png`) });
    console.log(`   ${id}.png`);
}
console.log(`== ${ids.length} 枚 → ${OUT}`);
await browser.close();
