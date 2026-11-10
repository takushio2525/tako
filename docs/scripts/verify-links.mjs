/**
 * ビルド出力の内部リンク検査（Issue #1547 / #2898）。
 *
 *   node scripts/verify-links.mjs              # dist/ を検査
 *   node scripts/verify-links.mjs <ディレクトリ>  # ビルド出力の写しなど、別の置き場を検査
 *
 * 見るもの:
 *   - `/` 始まりのリンク先がビルド出力に実在する（404 になるリンクを出さない）
 *   - 断片つき（`/agent-support/#...`）はリンク先ページにその id が在る
 *   - 同じページ内の断片（`href="#..."`）はそのページにその id が在る（#2898）。
 *     空の `#` と `#_top`（Starlight の目次の「概要」。飛び先の `<h1 id="_top">` は
 *     Starlight 自身が振る）は数えない
 *   - `<img src>` など内部アセットの参照先も実在する
 *
 * id として数えるのは `id="..."` と `name="..."` のすべて。Starlight が見出しと目次に
 * 振る id も、改名した見出しの旧 id を残すために手で置いたアンカー
 * （`<div id="..." class="tako-anchor">`）も同じ扱いになる。
 *
 * 断片まで見るのは、docs の一部が生成物で**見出しが条件で変わる**ため。
 * 例: `agent-support.md` の系統別の節は「対応が 1 件でもあるか」で
 * 「を選ぶと落ちるもの」と「でまだ使えないもの」に分かれ、手書きページは
 * その見出しへアンカーで飛んでいる。件数が変われば静かにリンクが切れる。
 * 同じページ内も同じで、見出しを改名すると本文から「3. 節」へのリンクが静かに切れる
 * （#2889 で当たった。以前は `/` 始まりだけを見ていたので CI が緑のままだった）。
 *
 * 外部リンク（http/https）は見ない。ネットワークの都合で CI が赤くなるのは
 * docs の正しさとは別の話なので、ここでは内部整合だけを固定する。
 *
 * 判定の境界は `scripts/test-verify-links.mjs` が固定している。
 */
import { readFileSync, readdirSync, existsSync, statSync } from 'node:fs';
import { dirname, join, relative, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const HERE = dirname(fileURLToPath(import.meta.url));
const DOCS = resolve(HERE, '..');

/** 飛び先の id を docs の側で持たない約束の同ページ内リンク（数えない） */
export const SAME_PAGE_EXEMPT = new Set(['#', '#_top']);

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

/** 別ページ（と内部アセット）への参照は「`/` 始まりで `//` でないもの」 */
const isInternal = (u) => u.startsWith('/') && !u.startsWith('//');

/** 同じページ内の断片 */
const isSamePage = (u) => u.startsWith('#');

/** 壊れた `%` 列は字面のまま返す（飛び先の id と一致しないので、切れとして名指される） */
function safeDecode(s) {
	try {
		return decodeURIComponent(s);
	} catch {
		return s;
	}
}

/** その id 集合に断片が在るか（字面のままでも、`%` を解いた形でもよい） */
const hasFragment = (ids, fragment) => ids.has(fragment) || ids.has(safeDecode(fragment));

/**
 * ビルド出力 `dist` を検査する。終了コードと足場の下限は呼び手（CLI）が決める。
 *
 * @returns {{ pages: string[], fails: string[], checkedLinks: number,
 *   checkedFragments: number, checkedSamePage: number, exemptSamePage: number }}
 */
export function checkSite(dist) {
	/** URL のパス部分 → ビルド出力のファイル。見つからなければ null */
	function resolveTarget(pathname) {
		const rel = safeDecode(pathname).replace(/^\//, '');
		const direct = join(dist, rel);
		if (rel !== '' && existsSync(direct) && statSync(direct).isFile()) return direct;
		const asDir = join(dist, rel, 'index.html');
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

	const pages = walk(dist).sort();
	const fails = [];
	let checkedLinks = 0;
	let checkedFragments = 0;
	let checkedSamePage = 0;
	let exemptSamePage = 0;

	for (const file of pages) {
		const from = `/${relative(dist, file)}`;
		const html = readFileSync(file, 'utf8');
		for (const raw of refs(html)) {
			if (isSamePage(raw)) {
				if (SAME_PAGE_EXEMPT.has(raw)) {
					exemptSamePage++;
					continue;
				}
				checkedSamePage++;
				if (!hasFragment(idsOf(file), raw.slice(1))) {
					fails.push(`${from}: 同じページに id が無い ${raw}`);
				}
				continue;
			}
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
			if (!hasFragment(idsOf(target), fragment)) {
				fails.push(`${from}: リンク先に id が無い ${raw}`);
			}
		}
	}

	return { pages, fails, checkedLinks, checkedFragments, checkedSamePage, exemptSamePage };
}

function main() {
	const dist = resolve(process.argv[2] ?? join(DOCS, 'dist'));
	// docs の外（ビルド出力の写しなど）は `../..` を重ねず絶対パスで見せる
	const rel = relative(DOCS, dist);
	const shown = rel && !rel.startsWith('..') ? rel : dist;
	if (!existsSync(dist)) {
		console.error(`${shown} が無い。先に npm run build を実行すること`);
		process.exit(1);
	}

	const r = checkSite(dist);

	// 材料の取り方が壊れたまま「全部 OK」と言わないための足場
	if (r.pages.length < 20) {
		console.error(`ビルド出力の HTML が ${r.pages.length} 件しかない。dist/ が古いか走査が壊れている`);
		process.exit(1);
	}
	if (r.checkedLinks < 100) {
		console.error(`内部リンクが ${r.checkedLinks} 本しか拾えていない。走査が壊れている`);
		process.exit(1);
	}
	// #2898 の時点の dist で 1703 本（見出しのある各ページに Starlight の目次が付く）。
	// 下限を割るなら拾い方が壊れている
	if (r.checkedSamePage < 100) {
		console.error(`同じページ内のリンクが ${r.checkedSamePage} 本しか拾えていない。走査が壊れている`);
		process.exit(1);
	}

	console.log(`対象: ${shown}`);
	console.log(
		`ページ ${r.pages.length} 件 / 内部リンク ${r.checkedLinks} 本（うち断片つき ${r.checkedFragments} 本）` +
			` / 同じページ内の断片 ${r.checkedSamePage} 本（対象外の \`#\`・\`#_top\` ${r.exemptSamePage} 本）`,
	);
	if (r.fails.length) {
		console.error(`\nFAILED ${r.fails.length} 件:`);
		for (const f of r.fails) console.error(`  - ${f}`);
		process.exit(1);
	}
	console.log('OK: 内部リンクはすべて実在し、断片はリンク先（同じページ内はそのページ）の id と一致する');
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) main();
