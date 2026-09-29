/**
 * 検索向けの head と sitemap の検査（Issue #1843）。ビルド出力（既定）か、配信中の URL を対象にできる。
 *
 *   node scripts/verify-seo.mjs                          # dist/ を検査
 *   node scripts/verify-seo.mjs https://<配信先のオリジン>   # robots.txt → sitemap を辿って実 URL を検査
 *
 * 見るもの:
 *   - robots.txt が sitemap-index.xml を指し、sitemap に**索引させる全ページ**が載っている
 *     （dist のページと過不足なし。404 は載せない）
 *   - 各ページ: `<title>` と description があり、ページ間で重複しない / canonical がそのページ自身 /
 *     noindex が付いていない
 *   - 構造化データ（JSON-LD）が 1 本あり JSON として読める。トップは WebSite、それ以外は
 *     BreadcrumbList（1 から連番・最後がそのページ・途中の段がすべて実在するページ）
 *   - FAQPage があれば、質問と回答がそのページの本文に実際に出ている
 *     （構造化データだけにある Q&A は Google のガイドライン違反になる）
 *
 * 型と項目が schema.org の語彙に合っているかまでは見ない（語彙の全量が要るため）。それは
 * PR の検証で schema.org の語彙を使って別に確かめる（#1843 の PR 本文を参照）。
 */
import { existsSync, readFileSync, readdirSync, statSync } from 'node:fs';
import { dirname, join, relative, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const HERE = dirname(fileURLToPath(import.meta.url));
const DOCS = resolve(HERE, '..');
const DIST = join(DOCS, 'dist');

// canonical・sitemap の基点。正本は astro.config.mjs の `site`（ここに URL を二重に書かない）
const SITE = readFileSync(join(DOCS, 'astro.config.mjs'), 'utf8')
	.match(/^\s*site:\s*'([^']+)'/m)?.[1]
	?.replace(/\/$/, '');
if (!SITE) throw new Error('astro.config.mjs から site が読めない');

const base = process.argv[2]?.replace(/\/$/, '');
const fails = [];
const fail = (where, msg) => fails.push(`${where}: ${msg}`);

const decode = (s) =>
	s
		.replace(/&lt;/g, '<')
		.replace(/&gt;/g, '>')
		.replace(/&quot;/g, '"')
		.replace(/&#39;/g, "'")
		.replace(/&#x27;/g, "'")
		.replace(/&amp;/g, '&');

/** 本文（main）の可視テキストを、空白を詰めた 1 本の文字列にする */
function bodyText(html) {
	const main = html.match(/<main[\s\S]*<\/main>/)?.[0] ?? html;
	return decode(
		main
			.replace(/<script[\s\S]*?<\/script>/g, '')
			.replace(/<style[\s\S]*?<\/style>/g, '')
			.replace(/<[^>]+>/g, '')
	).replace(/\s+/g, '');
}

/** URL パス → 読み出し（dist のファイルか、配信先への fetch） */
async function load(path) {
	if (base) {
		const res = await fetch(`${base}${path}`, { redirect: 'manual' });
		if (res.status !== 200) throw new Error(`HTTP ${res.status}`);
		return res.text();
	}
	const file = path.endsWith('/') ? join(DIST, path, 'index.html') : join(DIST, path);
	return readFileSync(file, 'utf8');
}

function distPages() {
	const out = [];
	const walk = (dir) => {
		for (const name of readdirSync(dir)) {
			const full = join(dir, name);
			if (statSync(full).isDirectory()) walk(full);
			else if (name === 'index.html') out.push(`/${relative(DIST, full).replace(/index\.html$/, '')}`);
		}
	};
	walk(DIST);
	return out.filter((p) => !p.startsWith('/404')).sort();
}

const locs = (xml) => [...xml.matchAll(/<loc>([^<]+)<\/loc>/g)].map((m) => decode(m[1]));
const toPath = (url) => new URL(url).pathname;

// ── robots.txt → sitemap ──────────────────────────────────────────────
const robots = await load('/robots.txt');
const sitemapUrl = robots.match(/^Sitemap:\s*(\S+)/m)?.[1];
if (sitemapUrl !== `${SITE}/sitemap-index.xml`) fail('/robots.txt', `Sitemap 行が ${sitemapUrl}`);
if (/^Disallow:\s*\/\s*$/m.test(robots)) fail('/robots.txt', 'サイト全体を Disallow している');

const children = locs(await load('/sitemap-index.xml'));
if (children.length === 0) fail('/sitemap-index.xml', '子 sitemap が無い');
const sitemapPaths = [];
for (const child of children) {
	if (!child.startsWith(`${SITE}/`)) fail('/sitemap-index.xml', `別オリジンを指す: ${child}`);
	for (const loc of locs(await load(toPath(child)))) {
		if (!loc.startsWith(`${SITE}/`)) fail(toPath(child), `別オリジンの URL: ${loc}`);
		sitemapPaths.push(toPath(loc));
	}
}
const pages = [...new Set(sitemapPaths)].sort();
if (pages.length !== sitemapPaths.length) fail('sitemap', 'URL が重複している');
if (pages.some((p) => p.startsWith('/404'))) fail('sitemap', '404 が載っている');

if (!base) {
	const built = distPages();
	for (const p of built) if (!pages.includes(p)) fail('sitemap', `dist にあるのに載っていない: ${p}`);
	for (const p of pages) if (!built.includes(p)) fail('sitemap', `dist に無いのに載っている: ${p}`);
}

// ── 各ページの head ────────────────────────────────────────────────────
const titles = new Map();
const descriptions = new Map();
const counts = { WebSite: 0, BreadcrumbList: 0, FAQPage: 0, Question: 0 };

for (const path of pages) {
	let html;
	try {
		html = await load(path);
	} catch (e) {
		fail(path, `読めない（${e.message}）`);
		continue;
	}
	const head = html.split('</head>')[0];
	const at = (msg) => fail(path, msg);

	const title = head.match(/<title>([^<]*)<\/title>/)?.[1];
	if (!title?.trim()) at('<title> が無い');
	else (titles.get(title) ?? titles.set(title, []).get(title)).push(path);

	const description = head.match(/<meta name="description" content="([^"]*)"/)?.[1];
	if (!description?.trim()) at('description が無い');
	else (descriptions.get(description) ?? descriptions.set(description, []).get(description)).push(path);

	const canonical = head.match(/<link rel="canonical" href="([^"]*)"/)?.[1];
	if (canonical !== `${SITE}${path}`) at(`canonical が ${canonical}`);
	if (/<meta name="robots" content="[^"]*noindex/.test(head)) at('noindex が付いている');

	const blocks = [...head.matchAll(/<script type="application\/ld\+json">([\s\S]*?)<\/script>/g)];
	if (blocks.length !== 1) {
		at(`JSON-LD が ${blocks.length} 本`);
		continue;
	}
	let data;
	try {
		data = JSON.parse(blocks[0][1]);
	} catch (e) {
		at(`JSON-LD が JSON として読めない: ${e.message}`);
		continue;
	}
	if (data['@context'] !== 'https://schema.org') at(`@context が ${data['@context']}`);
	const graph = data['@graph'] ?? [];
	const ofType = (t) => graph.filter((n) => n['@type'] === t);

	if (path === '/') {
		const site = ofType('WebSite');
		counts.WebSite += site.length;
		if (site.length !== 1) at('トップに WebSite が 1 つ無い');
		else if (site[0].url !== `${SITE}/` || !site[0].name) at('WebSite の url / name が違う');
		// @id で参照している実体が同じグラフに在る
		const ids = new Set(graph.map((n) => n['@id']).filter(Boolean));
		for (const node of graph)
			for (const value of Object.values(node))
				if (value && typeof value === 'object' && value['@id'] && !ids.has(value['@id']))
					at(`@id ${value['@id']} の実体がグラフに無い`);
	} else {
		const crumbs = ofType('BreadcrumbList');
		counts.BreadcrumbList += crumbs.length;
		if (crumbs.length !== 1) at('BreadcrumbList が 1 つ無い');
		else {
			const items = crumbs[0].itemListElement ?? [];
			items.forEach((item, i) => {
				if (item.position !== i + 1) at(`パンくずの position が連番でない（${i + 1} 番目が ${item.position}）`);
				if (!item.name) at(`パンくずの ${i + 1} 番目に name が無い`);
				const itemPath = item.item?.startsWith(SITE) ? item.item.slice(SITE.length) : undefined;
				if (!itemPath || !pages.includes(itemPath)) at(`パンくずの ${i + 1} 番目が実在しないページ: ${item.item}`);
			});
			if (items.length < 2) at('パンくずが 2 段未満');
			if (items[0]?.item !== `${SITE}/`) at('パンくずの先頭がトップでない');
			if (items.at(-1)?.item !== `${SITE}${path}`) at('パンくずの最後がこのページでない');
		}
	}

	for (const faq of ofType('FAQPage')) {
		counts.FAQPage++;
		const text = bodyText(html);
		const questions = faq.mainEntity ?? [];
		if (questions.length === 0) at('FAQPage に質問が無い');
		for (const q of questions) {
			counts.Question++;
			const name = (q.name ?? '').replace(/\s+/g, '');
			const answer = (q.acceptedAnswer?.text ?? '').replace(/\s+/g, '');
			if (!name || !text.includes(name)) at(`FAQ の質問が本文に無い: ${q.name}`);
			if (!answer || !text.includes(answer)) at(`FAQ の回答が本文と一致しない: ${q.name}`);
		}
	}
}

for (const [title, where] of titles) if (where.length > 1) fail('title', `「${title}」が重複: ${where.join(', ')}`);
for (const [d, where] of descriptions)
	if (where.length > 1) fail('description', `「${d.slice(0, 30)}…」が重複: ${where.join(', ')}`);

const target = base ?? 'dist';
if (fails.length) {
	console.error(`✗ ${target}: ${fails.length} 件\n  ${fails.join('\n  ')}`);
	process.exit(1);
}
console.log(
	`✓ ${target}: ${pages.length} ページ（sitemap と一致・robots から到達）、` +
		`title / description / canonical / JSON-LD 問題なし ` +
		`(WebSite ${counts.WebSite} / BreadcrumbList ${counts.BreadcrumbList} / ` +
		`FAQPage ${counts.FAQPage}・質問 ${counts.Question})`
);
