#!/usr/bin/env node
// check-script.mjs — 書き出した動画の画面の文字（OCR）と、台本（comp/index.html と comp/timeline.json）を突き合わせる。
//
// 何を見るか:
//   1. 台本の句（図解の場面の文字・実画面の場面のチップ / 見出し / 字幕 / 吹き出し / 注記）が、
//      その場面が映っている秒のフレームに出ていること
//   2. 実画面でペインへ打つ文字（コマンドと、対話への答え）と、対話の台本が打つコマンドが、その場面のフレームに出ていること
//
// 入力は pii-scan.sh が書く OCR の表（1 fps。`f00001.png<TAB>認識した 1 行`。f00001 = 0〜1 秒の最初の 1 枚）。
//   bash scripts/promo/pii-scan.sh <mp4>          → /private/tmp/tako-promo-pii/<名前>/ocr.tsv
//   node scripts/promo/howto/check-script.mjs /private/tmp/tako-promo-pii/<名前>/ocr.tsv
//
// 突き合わせ方: 空白と、OCR が取り違えやすい記号（全角 / 半角の括弧・中黒・引用符・長音と横棒）を落として比べる。
// 句がそのまま含まれていれば一致。含まれなければ、その場面の文字の中でいちばん近い並び（編集距離）を探し、
// 句の長さの 15% までの食い違いは「ほぼ一致」として、食い違った句と OCR の読みを並べて出す（目で確かめる用）。
// それより遠い句が 1 つでもあれば終了コード 1。

import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const ocrFile = process.argv[2];
if (!ocrFile) {
	console.error('使い方: check-script.mjs <ocr.tsv>');
	process.exit(2);
}

const norm = (s) =>
	s
		.normalize('NFKC')
		.replace(/[\s　]/g, '')
		.replace(/[「」『』“”"'`’‘（）()［］\[\]<>＜＞・･·.。、,，:：;；!！?？~〜ー―—–\-_=＝→/／|│]/g, '')
		.toLowerCase();

// ---- OCR: 秒ごとの文字 ----
const bySecond = new Map();
for (const line of readFileSync(ocrFile, 'utf8').split('\n')) {
	const tab = line.indexOf('\t');
	if (tab < 0) continue;
	const m = /f(\d+)\.png$/.exec(line.slice(0, tab));
	if (!m) continue;
	const sec = Number(m[1]) - 1;
	bySecond.set(sec, (bySecond.get(sec) || '') + norm(line.slice(tab + 1)));
}
const textIn = (start, end) => {
	let out = '';
	for (let s = Math.max(0, Math.floor(start)); s < Math.ceil(end); s += 1) out += bySecond.get(s) || '';
	return out;
};

// ---- 台本の句 ----
const timeline = JSON.parse(readFileSync(join(here, 'comp/timeline.json'), 'utf8'));
const html = readFileSync(join(here, 'comp/index.html'), 'utf8');
const strip = (s) =>
	s
		.replace(/<!--[\s\S]*?-->/g, '')
		.replace(/<[^>]+>/g, '\n')
		.replace(/&gt;/g, '>')
		.replace(/&lt;/g, '<')
		.replace(/&amp;/g, '&');

// 実画面でペインへ打つ文字（record.sh と同じ文。場面の id → 打つ文）
const TYPED = {
	setuprun: ['tako setup'],
	'dlg-orch': ['利用回数を節約したい', 'はい'],
	'dlg-project': ['~/shop-app を登録して'],
	'dlg-share': ['お願いします。名前はそのままで'],
	'dlg-done': ['/exit'],
	update: ['tako update check'],
	master: ['tako master', '決済 API にリトライを入れて、README も更新して'],
};

// 実画面に映るコマンド（対話の台本が実際に打つもの = demo/personas/setup.json の Bash の行と、tako setup / 案内が出す行）
const SHOWN = {
	'dlg-orch': ['tako orchestrator profiles set default --agent claude --agent-effort medium'],
	'dlg-project': ['tako orchestrator projects add --key shop-app --cwd ~/shop-app'],
	'dlg-share': ['gh repo create tako-config-sync --private', 'tako config init --remote https://github.com/demo/tako-config-sync', 'tako config status'],
	noagent: ['curl -fsSL https://claude.ai/install.sh | bash', 'claude auth login'],
};

const phrases = []; // { scene, start, end, text, kind }
for (const scene of timeline.scenes) {
	const add = (text, kind, start = scene.start, end = scene.end) => {
		for (const part of strip(text).split('\n')) {
			const t = part.trim();
			if (norm(t).length >= 2) phrases.push({ scene: scene.id, start, end, text: t, kind });
		}
	};
	if (scene.footage) {
		add(scene.chip || '', 'チップ');
		add(scene.head || '', '見出し');
		add(scene.note || '画面は実際の tako。エージェントの出力はデモ用の台本です', '注記');
		for (const sub of scene.subs || []) add(sub.html, '字幕', scene.start + sub.t0, scene.start + sub.t1);
		for (const c of scene.callouts || []) if (c.text) add(c.text, '吹き出し', scene.start + c.t0, scene.start + c.t1);
		for (const t of TYPED[scene.id] || []) add(t, '打つ文字');
		for (const t of SHOWN[scene.id] || []) add(t, '映るコマンド');
	} else {
		const m = new RegExp(`<section[^>]*id="${scene.id}"[^>]*>([\\s\\S]*?)</section>`).exec(html);
		if (!m) {
			console.error(`場面 ${scene.id} が index.html に無い`);
			process.exit(1);
		}
		add(m[1], '図解');
	}
}

// ---- 突き合わせ ----
// 句 p が文字列 hay のどこかに、食い違い何文字で入るか（部分列の編集距離の最小。Sellers の方法）
function nearest(p, hay) {
	let prev = new Array(p.length + 1).fill(0).map((_, i) => i);
	let best = { dist: p.length, end: 0 };
	for (let j = 1; j <= hay.length; j += 1) {
		const cur = [0];
		for (let i = 1; i <= p.length; i += 1) {
			cur[i] = Math.min(prev[i] + 1, cur[i - 1] + 1, prev[i - 1] + (p[i - 1] === hay[j - 1] ? 0 : 1));
		}
		if (cur[p.length] < best.dist) best = { dist: cur[p.length], end: j };
		prev = cur;
	}
	return best;
}

let exact = 0;
const near = [];
const missing = [];
for (const ph of phrases) {
	const p = norm(ph.text);
	// 出し入れの動きの途中の 1 枚に当たることがあるので、前後 1 秒まで見る
	const hay = textIn(ph.start - 1, ph.end + 1);
	if (hay.includes(p)) {
		exact += 1;
		continue;
	}
	const hit = nearest(p, hay);
	const read = hay.slice(Math.max(0, hit.end - p.length - 2), hit.end);
	(hit.dist <= Math.max(1, Math.floor(p.length * 0.15)) ? near : missing).push({ ...ph, dist: hit.dist, read });
}

console.log(`台本の句: ${phrases.length}（一致 ${exact}・ほぼ一致 ${near.length}・見つからない ${missing.length}）`);
for (const [label, list] of [
	['ほぼ一致（OCR の読み違いか確かめる）', near],
	['見つからない', missing],
]) {
	if (!list.length) continue;
	console.log(`\n== ${label}`);
	for (const m of list) console.log(`  [${m.scene} ${m.start.toFixed(1)}–${m.end.toFixed(1)} 秒 / ${m.kind}] ${m.text}\n      OCR の読み（食い違い ${m.dist} 字）: ${m.read}`);
}
process.exit(missing.length ? 1 : 0);
