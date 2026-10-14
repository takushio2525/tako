/**
 * ビルド出力の Markdown の描画検査（Issue #2082）。
 *
 *   node scripts/verify-markdown.mjs        # src/content/docs と dist/ を突き合わせる
 *
 * 見るもの:
 *   - 本文（.md / .mdx）に書いた Markdown の表が、ビルド出力で `<table>` として描かれている
 *     （ページごとに「本文の表の数 ≦ 出力の `<table>` の数」）
 *   - 出力の本文に、描かれなかった表の区切り行（`|---|`）が文字のまま残っていない
 *   - コマンド（<Cmd> = `.tk-cmd-text`）に全角ダッシュ（U+2014 / U+2013）が 1 つも無い（#2088）
 *   - コード（`<code>` / `<pre>`）に、フラグのように英字が続く全角ダッシュ（`—pane` など）が無い（#2088）。
 *     数の範囲（`0.0–1.0`）のように意図して書いたものは英字が続かないので対象外
 *
 * 2026-10 に、Astro 6.4 で `markdown.gfm` の既定が未設定になり、Starlight が足す
 * @astrojs/mdx 5.0.x がそれを「無効」と読んだため、.mdx のページだけ表が `| a | b |` の
 * まま公開されていた（.md は Astro 本体の処理系が既定で GFM を有効にするので無事だった）。
 * 同じ時期に、.mdx の自動整形（smartypants）が <Cmd> の子の `--` を全角ダッシュに置き換え、
 * `brew install --cask …` が `—cask` のまま公開されてコピーしても動かなかった（#2088）。
 * どちらもビルドも他の検査も通ってしまう壊れ方なので、ここで落とす。
 * 直し方は astro.config.mjs の mdx({ gfm: true, smartypants: false })。
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

// コマンドとコードの中の全角ダッシュ（#2088）。本文に対応するページに限らず、出力の全ページを見る
const DASH = /[\u2014\u2013]/;
const FLAG_DASH = /[\u2014\u2013][A-Za-z]/;
const textOf = (fragment) =>
	fragment
		.replace(/<[^>]+>/g, '')
		.replace(/&amp;/g, '&')
		.replace(/&lt;/g, '<')
		.replace(/&gt;/g, '>')
		.replace(/&#x2014;|&mdash;/g, '\u2014')
		.replace(/&#x2013;|&ndash;/g, '\u2013');
let commands = 0;
for (const out of walk(DIST, ['.html'])) {
	const html = readFileSync(out, 'utf8');
	const rel = relative(DOCS, out);
	for (const m of html.matchAll(/<code class="tk-cmd-text[^"]*"[^>]*>([\s\S]*?)<\/code>/g)) {
		commands++;
		const text = textOf(m[1]).trim();
		if (DASH.test(text)) problems.push(`${rel}: コマンドに全角ダッシュが入っている（コピーしても動かない）: ${text.slice(0, 80)}`);
	}
	for (const m of html.matchAll(/<(code|pre)\b(?![^>]*tk-cmd-text)[^>]*>([\s\S]*?)<\/\1>/g)) {
		const text = textOf(m[2]);
		const hit = text.match(FLAG_DASH);
		if (hit) {
			const at = text.indexOf(hit[0]);
			problems.push(`${rel}: コードの中で \`--\` が全角ダッシュに化けている: …${text.slice(Math.max(0, at - 20), at + 20).trim()}…`);
		}
	}
}

console.log(`対象: dist（本文 ${pages} ページ / Markdown の表 ${tables} 個 / コマンド ${commands} 個）`);
if (problems.length > 0) {
	for (const p of problems) console.error(`NG: ${p}`);
	console.error('直し方: .mdx の表なら astro.config.mjs の mdx({ gfm: true })（#2082）、全角ダッシュなら同じ所の smartypants: false（#2088）が効いているかを見る');
	process.exit(1);
}
console.log('OK: 本文の Markdown の表はすべて <table> として描かれ、区切り行が文字のまま残っておらず、コマンドとコードに全角ダッシュが無い');
