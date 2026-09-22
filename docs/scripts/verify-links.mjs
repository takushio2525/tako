/**
 * ビルド出力の内部リンク検査（Issue #1547）。
 *
 *   node scripts/verify-links.mjs        # dist/ を検査
 *
 * 見るもの:
 *   - `/` 始まりのリンク先がビルド出力に実在する（404 になるリンクを出さない）
 *   - 断片つき（`/agent-support/#...`）はリンク先ページにその id が在る
 *   - `<img src>` など内部アセットの参照先も実在する
 *
 * 断片まで見るのは、docs の一部が生成物で**見出しが条件で変わる**ため。
 * 例: `agent-support.md` の系統別の節は「対応が 1 件でもあるか」で
 * 「を選ぶと落ちるもの」と「でまだ使えないもの」に分かれ、手書きページは
 * その見出しへアンカーで飛んでいる。件数が変われば静かにリンクが切れる。
 *
 * 外部リンク（http/https）は見ない。ネットワークの都合で CI が赤くなるのは
 * docs の正しさとは別の話なので、ここでは内部整合だけを固定する。
 */
import { readFileSync, readdirSync, existsSync, statSync } from 'node:fs';
import { dirname, join, relative, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const HERE = dirname(fileURLToPath(import.meta.url));
const DOCS = resolve(HERE, '..');
const DIST = join(DOCS, 'dist');

function walk(dir, acc = []) {
	for (const name of readdirSync(dir)) {
		const full = join(dir, name);
		if (statSync(full).isDirectory()) walk(full, acc);
		else if (name.endsWith('.html')) acc.push(full);
	}
	return acc;
}

/** `href="..."` / `src="..."` の値を全部拾う */
function refs(html) {
	const out = [];
	const re = /\s(?:href|src)="([^"]*)"/g;
	let m;
	while ((m = re.exec(html)) !== null) out.push(m[1]);
	return out;
}

/** 検査対象は「`/` 始まりで `//` でないもの」だけ */
const isInternal = (u) => u.startsWith('/') && !u.startsWith('//');

/** URL のパス部分 → ビルド出力のファイル。見つからなければ null */
function resolveTarget(pathname) {
	const rel = decodeURIComponent(pathname).replace(/^\//, '');
	const direct = join(DIST, rel);
	if (rel !== '' && existsSync(direct) && statSync(direct).isFile()) return direct;
	const asDir = join(DIST, rel, 'index.html');
	if (existsSync(asDir)) return asDir;
	return null;
}

/** そのページが持つ id の集合 */
const idCache = new Map();
function idsOf(file) {
	if (idCache.has(file)) return idCache.get(file);
	const html = readFileSync(file, 'utf8');
	const set = new Set();
	const re = /\s(?:id|name)="([^"]+)"/g;
	let m;
	while ((m = re.exec(html)) !== null) set.add(m[1]);
	idCache.set(file, set);
	return set;
}

if (!existsSync(DIST)) {
	console.error(`${relative(DOCS, DIST)} が無い。先に npm run build を実行すること`);
	process.exit(1);
}

const pages = walk(DIST).sort();
const fails = [];
let checkedLinks = 0;
let checkedFragments = 0;

for (const file of pages) {
	const from = `/${relative(DIST, file)}`;
	const html = readFileSync(file, 'utf8');
	for (const raw of refs(html)) {
		if (!isInternal(raw)) continue;
		const [pathname, fragment] = raw.split('#');
		checkedLinks++;
		const target = resolveTarget(pathname);
		if (!target) {
			fails.push(`${from}: リンク先が無い ${raw}`);
			continue;
		}
		if (!fragment) continue;
		if (!target.endsWith('.html')) continue;
		checkedFragments++;
		const ids = idsOf(target);
		const decoded = decodeURIComponent(fragment);
		if (!ids.has(fragment) && !ids.has(decoded)) {
			fails.push(`${from}: リンク先に id が無い ${raw}`);
		}
	}
}

// 材料の取り方が壊れたまま「全部 OK」と言わないための足場
if (pages.length < 20) {
	console.error(`ビルド出力の HTML が ${pages.length} 件しかない。dist/ が古いか走査が壊れている`);
	process.exit(1);
}
if (checkedLinks < 100) {
	console.error(`内部リンクが ${checkedLinks} 本しか拾えていない。走査が壊れている`);
	process.exit(1);
}

console.log(`対象: ${relative(DOCS, DIST)}`);
console.log(
	`ページ ${pages.length} 件 / 内部リンク ${checkedLinks} 本（うち断片つき ${checkedFragments} 本）`,
);
if (fails.length) {
	console.error(`\nFAILED ${fails.length} 件:`);
	for (const f of fails) console.error(`  - ${f}`);
	process.exit(1);
}
console.log('OK: 内部リンクはすべて実在し、断片はリンク先の id と一致する');
