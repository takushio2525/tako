#!/usr/bin/env node
// stage.mjs — 入れ方・使い方動画（#2866）の合成の「舞台」を作業用の置き場に組む。
//
// なぜ要るか:
//   合成の仕組み（1 枚を時刻から決定的に描く comp.js / comp.css・1/30 秒ずつ撮る render.mjs・
//   収録の時刻で切り出しを確定させる timeline.mjs・波形から音を作る bgm.mjs）は、プロモーション動画（#2864）の
//   scripts/promo/remake のものをそのまま使う（この動画用に複製しない）。ただしその 4 本は自分の隣の
//   comp/index.html と comp/timeline.json を固定で読むので、この動画の場面と時刻を渡す口が無い。
//   そこで書き出すたびに、#2864 の 4 本と、この動画の comp/（場面・字幕・時刻）を 1 つの置き場へ並べ、
//   そこで #2864 の render.mjs / bgm.mjs を走らせる。リポジトリに入るのはこの動画の差分だけ。
//
// 音だけは 1 か所を差し替える:
//   bgm.mjs は楽器（音色）と曲の構成（どの小節で何を鳴らすか）が 1 本になっていて、構成は #2864 の 95 秒の
//   場面割りに合わせてある。楽器・残響・書き出しはそのまま使い、「構成」の節だけをこの動画の arrangement.js に
//   差し替えた bgm.mjs を舞台に置く（節の見出しが変わっていたら、黙って続けずに止まる）。
//
// #2864 の側が comp の置き場と曲の構成を引数で受けるようになれば、このファイルは要らなくなる。
//
// 使い方: node scripts/promo/howto/stage.mjs <舞台の置き場>
//   #2864 の一式の置き場は TAKO_HOWTO_REMAKE_DIR（既定は同じ木の scripts/promo/remake）
//   出力: <舞台>/scripts/promo/remake/{render.mjs,timeline.mjs,bgm.mjs,comp/…}（render.mjs が根を ../../.. と見るので同じ深さに置く）

import { copyFileSync, existsSync, mkdirSync, readFileSync, readdirSync, rmSync, symlinkSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const root = resolve(here, '../../..');
const remake = resolve(process.env.TAKO_HOWTO_REMAKE_DIR || join(here, '../remake'));
const stage = process.argv[2] && resolve(process.argv[2]);

const die = (message) => {
	console.error(`ERROR: ${message}`);
	process.exit(1);
};
if (!stage) die('使い方: stage.mjs <舞台の置き場>');
for (const f of ['render.mjs', 'timeline.mjs', 'bgm.mjs', 'comp/comp.js', 'comp/comp.css']) {
	if (!existsSync(join(remake, f))) die(`#2864 の合成の一式が無い: ${join(remake, f)}（TAKO_HOWTO_REMAKE_DIR で置き場を渡す）`);
}

const out = join(stage, 'scripts/promo/remake');
rmSync(stage, { recursive: true, force: true });
mkdirSync(join(out, 'comp'), { recursive: true });

// #2864 の仕組み（そのまま）
for (const f of ['render.mjs', 'timeline.mjs', 'comp/comp.js', 'comp/comp.css']) copyFileSync(join(remake, f), join(out, f));
// この動画の場面・字幕・時刻
for (const f of readdirSync(join(here, 'comp'))) copyFileSync(join(here, 'comp', f), join(out, 'comp', f));

// 音: 「構成」の節だけをこの動画のものへ差し替える
const bgm = readFileSync(join(remake, 'bgm.mjs'), 'utf8');
const from = bgm.indexOf('// ---- 構成');
const to = bgm.indexOf('// ---- 効果音');
if (from < 0 || to < 0 || to < from) die(`${join(remake, 'bgm.mjs')} の節の見出し（「構成」「効果音」）が見つからない。#2864 の側が変わったので、差し替え方を見直す`);
writeFileSync(join(out, 'bgm.mjs'), `${bgm.slice(0, from)}${readFileSync(join(here, 'arrangement.js'), 'utf8')}\n${bgm.slice(to)}`);

// render.mjs は根（../../..）から Playwright（web/tako-remote）とアプリのアイコン（assets/icon）を引く
const web = resolve(process.env.TAKO_HOWTO_WEB_DIR || join(root, 'web'));
if (!existsSync(join(web, 'tako-remote/node_modules/playwright'))) {
	die(`playwright が見つからない: ${join(web, 'tako-remote')}（先に (cd web/tako-remote && npm ci)。入れてある木を TAKO_HOWTO_WEB_DIR で渡してもよい）`);
}
symlinkSync(web, join(stage, 'web'));
symlinkSync(join(root, 'assets'), join(stage, 'assets'));

console.log(out);
