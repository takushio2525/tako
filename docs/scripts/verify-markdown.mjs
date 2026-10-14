/**
 * ビルド出力の Markdown の描画検査（Issue #2082）。
 *
 *   node scripts/verify-markdown.mjs        # src/content/docs と dist/ を突き合わせる
 *
 * 見るもの:
 *   - 本文（.md / .mdx）に書いた Markdown の表が、ビルド出力で `<table>` として描かれている
 *     （ページごとに「本文の表の数 ≦ 出力の `<table>` の数」）
 *   - 出力の本文に、描かれなかった表の区切り行（`|---|`）が文字のまま残っていない
 *
 * 2026-10 に、Astro 6.4 で `markdown.gfm` の既定が未設定になり、Starlight が足す
 * @astrojs/mdx 5.0.x がそれを「無効」と読んだため、.mdx のページだけ表が `| a | b |` の
 * まま公開されていた（.md は Astro 本体の処理系が既定で GFM を有効にするので無事だった）。
 * ビルドも他の検査も通ってしまう壊れ方なので、ここで表の数を数えて落とす。
 * 直し方は astro.config.mjs の mdx({ gfm: true })。
 */
import { readFileSync, readdirSync, existsSync, statSync } from 'node:fs';
import { dirname, join, relative, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const HERE = dirname(fileURLToPath(import.meta.url));
const DOCS = resolve(HERE, '..');
const CONTENT = join(DOCS, 'src/content/docs');
const DIST = join(DOCS, 'dist');

function walk(dir, exts, acc = []) {
	for (const name of readdirSync(dir)) {
		const full = join(dir, name);
		if (statSync(full).isDirectory()) walk(full, exts, acc);
		else if (exts.some((e) => name.endsWith(e))) acc.push(full);
	}
	return acc;
}

/** 区切り行（`|---|---|` / `| :--- | ---: |` / 先頭末尾の `|` 省略も可） */
const DELIM = /^\s*\|?\s*:?-{3,}:?\s*(\|\s*:?-{3,}:?\s*)+\|?\s*$/;

/** 本文の Markdown の表の数（フェンスの中は数えない。区切り行の直前が `|` を含む行のときだけ表） */
function countTables(src) {
	let n = 0;
	let fence = null;
	const lines = src.split('\n');
	for (let i = 0; i < lines.length; i++) {
		const m = lines[i].match(/^\s*(`{3,}|~{3,})/);
		if (m) {
			if (!fence) fence = m[1][0];
			else if (m[1][0] === fence) fence = null;
			continue;
		}
		if (fence) continue;
		if (DELIM.test(lines[i]) && i > 0 && lines[i - 1].includes('|')) n++;
	}
	return n;
}

/** 本文ファイル → 出力の HTML（`a/b.md` → `dist/a/b/index.html`、`a/index.mdx` → `dist/a/index.html`） */
function outputOf(file) {
	const rel = relative(CONTENT, file).replace(/\.mdx?$/, '');
	const route = rel === 'index' ? '' : rel.replace(/(^|\/)index$/, '');
	return join(DIST, route, 'index.html');
}

/** 本文の領域だけを取り出し、コード（pre / code）を除く */
function proseOf(html) {
	const start = html.indexOf('sl-markdown-content');
	const body = start >= 0 ? html.slice(start) : html;
	return body
		.replace(/<pre[\s\S]*?<\/pre>/g, '')
		.replace(/<code[\s\S]*?<\/code>/g, '')
		.replace(/<script[\s\S]*?<\/script>/g, '')
		.replace(/<style[\s\S]*?<\/style>/g, '');
}

if (!existsSync(DIST)) {
	console.error('dist/ がありません。先に npm run build を実行してください');
	process.exit(1);
}

const problems = [];
let pages = 0;
let tables = 0;
for (const file of walk(CONTENT, ['.md', '.mdx'])) {
	const out = outputOf(file);
	const rel = relative(DOCS, file);
	if (!existsSync(out)) {
		problems.push(`${rel}: 出力 ${relative(DOCS, out)} が見つからない`);
		continue;
	}
	pages++;
	const want = countTables(readFileSync(file, 'utf8'));
	const html = readFileSync(out, 'utf8');
	const got = (html.match(/<table[\s>]/g) ?? []).length;
	tables += want;
	if (got < want) {
		problems.push(`${rel}: 本文の表 ${want} 個に対して出力の <table> が ${got} 個（表が文字のまま描かれている）`);
	}
	const raw = proseOf(html)
		.replace(/<[^>]+>/g, '\n')
		.split('\n')
		.filter((line) => /\|\s*:?-{3,}:?\s*\|/.test(line));
	if (raw.length > 0) {
		problems.push(`${rel}: 表の区切り行が文字のまま残っている（${raw.length} か所。例: ${raw[0].trim().slice(0, 60)}）`);
	}
}

console.log(`対象: dist（本文 ${pages} ページ / Markdown の表 ${tables} 個）`);
if (problems.length > 0) {
	for (const p of problems) console.error(`NG: ${p}`);
	console.error('直し方: .mdx の表なら astro.config.mjs の mdx({ gfm: true }) が効いているかを見る（#2082）');
	process.exit(1);
}
console.log('OK: 本文の Markdown の表はすべて <table> として描かれ、区切り行が文字のまま残っていない');
