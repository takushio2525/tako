#!/usr/bin/env node
// render.mjs — comp/index.html を 1/30 秒ずつ進めて撮り、映像（無音の mp4）にする。
//
// 使い方:
//   node scripts/promo/remake/render.mjs --out <mp4>            全編を書き出す
//   node scripts/promo/remake/render.mjs --stills 3,18.5,31 --out <dir>   指定した秒の 1 枚ずつを PNG で出す（確認用）
//   オプション: --scenes <収録の置き場（既定 ~/Movies/tako-promo/remake/scenes）>
//               --from <秒> --to <秒>（一部だけ書き出す）
//
// 描画は Playwright の chromium（headless）。新しい依存は足さず、web/tako-remote の node_modules を使う
// （無ければ `cd web/tako-remote && npm ci`）。動きは壁時計に依らないので、何度書き出しても同じ絵になる。

import { spawn } from 'node:child_process';
import { mkdirSync } from 'node:fs';
import { createRequire } from 'node:module';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { defaultScenesDir, loadTimeline } from './timeline.mjs';

const here = dirname(fileURLToPath(import.meta.url));
const root = resolve(here, '../../..');
const args = process.argv.slice(2);
const opt = (name, fallback) => {
	const i = args.indexOf(`--${name}`);
	return i >= 0 ? args[i + 1] : fallback;
};

const require = createRequire(join(root, 'web/tako-remote/package.json'));
let chromium;
try {
	({ chromium } = require('playwright'));
} catch {
	console.error('playwright が見つからない。先に: (cd web/tako-remote && npm ci)');
	process.exit(1);
}

let timeline;
let footage;
try {
	({ timeline, footage } = loadTimeline(resolve(opt('scenes', defaultScenesDir()))));
} catch (e) {
	console.error(e.message);
	process.exit(1);
}

const browser = await chromium.launch({ headless: true });
const page = await browser.newPage({ viewport: { width: 1920, height: 1080 }, deviceScaleFactor: 1 });
page.on('pageerror', (e) => {
	console.error(`ページのエラー: ${e.message}`);
	process.exitCode = 1;
});
await page.addInitScript(
	({ tl, ft, assets }) => {
		window.TL = tl;
		window.FOOTAGE = ft;
		window.ASSETS = assets;
	},
	{ tl: timeline, ft: footage, assets: { icon: pathToFileURL(join(root, 'assets/icon/icon-a.svg')).href } },
);
await page.goto(pathToFileURL(join(here, 'comp/index.html')).href);
await page.evaluate(() => window.__ready);

const out = resolve(opt('out', 'promo.mp4'));
const stills = opt('stills');
if (stills) {
	mkdirSync(out, { recursive: true });
	for (const t of stills.split(',').map(Number)) {
		await page.evaluate((s) => window.__seek(s), t);
		const file = join(out, `t${t.toFixed(2).padStart(6, '0')}.png`);
		await page.screenshot({ path: file });
		console.log(file);
	}
	await browser.close();
	process.exit(process.exitCode || 0);
}

const fps = timeline.fps;
const from = Number(opt('from', 0));
const to = Number(opt('to', timeline.duration));
const total = Math.round((to - from) * fps);
mkdirSync(dirname(out), { recursive: true });
const ff = spawn(
	'ffmpeg',
	['-nostdin', '-v', 'error', '-y', '-f', 'image2pipe', '-framerate', String(fps), '-c:v', 'png', '-i', '-',
		'-c:v', 'libx264', '-preset', 'medium', '-crf', '16', '-pix_fmt', 'yuv420p', '-r', String(fps), '-movflags', '+faststart', out],
	{ stdio: ['pipe', 'inherit', 'inherit'] },
);
const done = new Promise((res, rej) => ff.on('close', (code) => (code === 0 ? res() : rej(new Error(`ffmpeg が ${code} で終わった`)))));
const started = Date.now();
for (let i = 0; i < total; i += 1) {
	const t = from + i / fps;
	await page.evaluate((s) => window.__seek(s), t);
	const png = await page.screenshot({ type: 'png' });
	if (!ff.stdin.write(png)) await new Promise((r) => ff.stdin.once('drain', r));
	if (i % 150 === 0) console.log(`${i}/${total} 枚（${((Date.now() - started) / 1000).toFixed(0)} 秒）`);
}
ff.stdin.end();
await done;
await browser.close();
console.log(`書き出した: ${out}（${total} 枚 / ${(total / fps).toFixed(2)} 秒）`);
