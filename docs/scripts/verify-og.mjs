/**
 * OG / Twitter Card の検査。ビルド出力（既定）か、デプロイ済みの実 URL を対象にできる。
 *
 *   node scripts/verify-og.mjs                       # dist/ を検査
 *   node scripts/verify-og.mjs https://example.com   # 実 URL を検査（画像の HTTP 200 まで見る）
 *
 * 見るもの:
 *   - og:image が絶対 URL で、実体が 1200x630 で存在する
 *   - og:image がページごとに違う（= 焼き忘れてトップの画像へ落ちていない）
 *   - og:url / canonical がそのページ自身を指す
 *   - twitter:card が summary_large_image
 */
import { readFileSync, readdirSync, statSync } from 'node:fs';
import { dirname, join, relative, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import sharp from 'sharp';

const HERE = dirname(fileURLToPath(import.meta.url));
const DOCS = resolve(HERE, '..');
const DIST = join(DOCS, 'dist');
const base = process.argv[2]?.replace(/\/$/, '');
const EXPECT = { width: 1200, height: 630 };

function walk(dir, acc = []) {
	for (const name of readdirSync(dir)) {
		const full = join(dir, name);
		if (statSync(full).isDirectory()) walk(full, acc);
		else if (name === 'index.html') acc.push(full);
	}
	return acc;
}

const meta = (html, key) =>
	html.match(new RegExp(`<meta (?:property|name)="${key}" content="([^"]*)"`))?.[1];

const pages = walk(DIST)
	.map((file) => ({ file, path: `/${relative(DIST, file).replace(/index\.html$/, '')}` }))
	// 404 は共有されないので対象外（画像はフォールバックで付く）
	.filter((p) => p.path !== '/404.html/' && !p.path.startsWith('/404'))
	.sort((a, b) => a.path.localeCompare(b.path));

const fails = [];
const seen = new Map();
let checkedImages = 0;

for (const page of pages) {
	const html = readFileSync(page.file, 'utf8');
	const at = (msg) => fails.push(`${page.path}: ${msg}`);

	const image = meta(html, 'og:image');
	const ogUrl = meta(html, 'og:url');
	const card = meta(html, 'twitter:card');
	const twImage = meta(html, 'twitter:image');

	if (!image) at('og:image が無い');
	else if (!/^https:\/\//.test(image)) at(`og:image が絶対 URL でない: ${image}`);
	if (card !== 'summary_large_image') at(`twitter:card が ${card}`);
	if (twImage !== image) at(`twitter:image が og:image と違う: ${twImage}`);

	const expectedUrl = base ? `${base}${page.path}` : undefined;
	if (!ogUrl) at('og:url が無い');
	else if (!ogUrl.endsWith(page.path)) at(`og:url が自分を指していない: ${ogUrl}`);
	else if (expectedUrl && ogUrl !== expectedUrl) at(`og:url が ${ogUrl}（期待 ${expectedUrl}）`);

	if (image) {
		const prev = seen.get(image);
		if (prev) at(`og:image が ${prev} と同じ（このページの画像が焼かれていない）: ${image}`);
		else seen.set(image, page.path);

		// 画像の実体を見る。実 URL 指定なら HTTP で、既定ならビルド出力から
		const imgPath = new URL(image).pathname;
		try {
			let buf;
			if (base) {
				const res = await fetch(`${base}${imgPath}`);
				if (!res.ok) throw new Error(`HTTP ${res.status}`);
				const type = res.headers.get('content-type') || '';
				if (!/^image\/(png|jpeg)/.test(type)) throw new Error(`content-type ${type}`);
				buf = Buffer.from(await res.arrayBuffer());
			} else {
				buf = readFileSync(join(DIST, imgPath.replace(/^\//, '')));
			}
			const m = await sharp(buf).metadata();
			if (m.width !== EXPECT.width || m.height !== EXPECT.height)
				at(`画像が ${m.width}x${m.height}（期待 ${EXPECT.width}x${EXPECT.height}）: ${imgPath}`);
			checkedImages++;
		} catch (e) {
			at(`画像が取得できない ${imgPath}: ${e.message}`);
		}
	}
}

console.log(`対象: ${base ?? relative(DOCS, DIST)}`);
console.log(`ページ ${pages.length} 件 / 画像 ${checkedImages} 件 / 固有画像 ${seen.size} 件`);
if (fails.length) {
	console.error(`\nFAILED ${fails.length} 件:`);
	for (const f of fails) console.error(`  - ${f}`);
	process.exit(1);
}
console.log('OK: 全ページに固有の 1200x630 OG 画像 + summary_large_image + 自ページを指す og:url');
