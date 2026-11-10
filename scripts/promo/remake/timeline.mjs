// timeline.mjs — comp/timeline.json を読み、収録の出来事（beats.tsv）を使って時刻を確定させる。
//
// timeline.json では、実画面の切り出しを「出来事の名前 ± 秒」で書ける（例: "master:report+1.6"）。
// 撮り直すたびに秒数は少し動くので、名前で書いておけば映像と音の両方が同じ規則で追従する。
// render.mjs（映像）と bgm.mjs（音）が同じこの 1 本を通す。

import { existsSync, readFileSync, readdirSync } from 'node:fs';
import { homedir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));

export function defaultScenesDir() {
	return process.env.TAKO_PROMO_OUT ? join(process.env.TAKO_PROMO_OUT, 'scenes') : join(homedir(), 'Movies/tako-promo/remake/scenes');
}

// 収録した場面を読む: { 名前: { dir（file:// の URL）, frames: [[番号, ms]…], beats: { 名前: 秒 } } }
export function loadFootage(scenesDir) {
	const footage = {};
	for (const name of existsSync(scenesDir) ? readdirSync(scenesDir) : []) {
		const file = join(scenesDir, name, 'frames.tsv');
		if (!existsSync(file)) continue;
		const rows = (f) => (existsSync(f) ? readFileSync(f, 'utf8').split('\n').filter(Boolean).map((l) => l.split('\t')) : []);
		footage[name] = {
			dir: pathToFileURL(join(scenesDir, name)).href,
			frames: rows(file).map((r) => r.map(Number)),
			beats: Object.fromEntries(rows(join(scenesDir, name, 'beats.tsv')).map(([k, ms]) => [k, Number(ms) / 1000])),
		};
	}
	return footage;
}

// "名前+秒" / "名前-秒" / 数値 → 収録の中の秒
function footageTime(value, footage, where) {
	if (typeof value === 'number') return value;
	const m = /^(.+?)([+-]\d+(?:\.\d+)?)?$/.exec(value);
	const at = footage?.beats[m[1]];
	if (at === undefined) throw new Error(`${where}: 出来事 ${m[1]} が収録に無い`);
	return at + Number(m[2] || 0);
}

// 収録の中の秒 → 映像の秒（その場面の切り出しをたどる）
function videoTime(scene, sec) {
	for (const c of scene.clips) {
		if (sec >= c.src[0] && sec <= c.src[1]) return scene.start + c.at[0] + ((sec - c.src[0]) / (c.src[1] - c.src[0])) * (c.at[1] - c.at[0]);
	}
	return null;
}

export function loadTimeline(scenesDir = defaultScenesDir()) {
	const timeline = JSON.parse(readFileSync(join(here, 'comp/timeline.json'), 'utf8'));
	const footage = loadFootage(resolve(scenesDir));
	const sfx = [...(timeline.sfx || [])];
	for (const scene of timeline.scenes) {
		if (!scene.clips) continue;
		for (const clip of scene.clips) {
			const name = clip.footage || scene.footage;
			if (!footage[name]) throw new Error(`収録が無い: ${join(scenesDir, name)}（先に record.sh を回す）`);
			clip.src = clip.src.map((v) => footageTime(v, footage[name], `${scene.id}`));
		}
		// 場面の効果音: 出来事が映像のどの秒に映るかを切り出しから求める
		for (const s of scene.sfx || []) {
			const sec = footageTime(s.beat, footage[scene.footage], scene.id);
			const t = videoTime(scene, sec);
			if (t !== null) sfx.push({ ...s, t: Number(t.toFixed(3)) });
		}
	}
	timeline.sfx = sfx.sort((a, b) => a.t - b.t);
	return { timeline, footage };
}
