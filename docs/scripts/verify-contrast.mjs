/**
 * ビルド出力の配色の検査（Issue #2877）。
 *
 *   node scripts/verify-contrast.mjs          # dist/_astro/*.css を読む
 *   node scripts/verify-contrast.mjs <dist>   # 別の出力を読む（検査そのものを確かめるとき）
 *
 * `npm run build` の直後に postbuild として自動で走る（package.json）。CI の docs ジョブと
 * Cloudflare Pages の配備も `npm run build` を通るので、読めない配色はビルドの失敗として止まる。
 *
 * 見るもの（ライト・ダークの両方で）:
 *   1. 灰色の段 `--sl-color-gray-1..7` の向き。Starlight の約束は「gray-1 が文字の側
 *      （--sl-color-white 寄り）、gray-7 が背景の側（--sl-color-black 寄り）」で、Starlight の部品は
 *      この向きを前提に塗る。背景（--sl-color-bg）とのコントラスト比が gray-1 → gray-7 で
 *      単調に下がっていることを見る
 *   2. 背景の上に置く薄い文字（下の TEXT_ON_BG）の文字色が、背景に対して 4.5 以上
 *
 * 2026-11 に、このテーマが灰色の段を逆向き（gray-1 が背景の側）に並べていたため、
 * 図の説明文とフッターの法務リンク（gray-3）がライト 1.45・ダーク 1.66 で背景に溶け、
 * 検索ダイアログの枠の地（gray-6）が配色と逆になり、検索結果の抜粋とテーマ切替のアイコン
 * （gray-2）も 1.22 / 1.28 で読めなかった。ビルドも他の検査も通ってしまう壊れ方なので、ここで落とす。
 *
 * CI の docs ジョブには node しか無いので、ブラウザは使わず CSS を静的に読む。
 * 変数の値は「層（@layer）の外 > 層の中」「:root[data-theme=…] > :root」の順に解決する
 * （@media の中の宣言は見ない。このテーマは配色を data-theme だけで切り替える）。
 */
import { readFileSync, readdirSync, existsSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const HERE = dirname(fileURLToPath(import.meta.url));
const DIST = resolve(process.argv[2] ?? join(HERE, '..', 'dist'));
const CSS_DIR = join(DIST, '_astro');

/** 本文並みに読めることの下限（WCAG 2.x の本文の目安） */
const MIN_TEXT = 4.5;

/** 背景（--sl-color-bg）の上に置く薄い文字。selector は dist の CSS に現れる形 */
const TEXT_ON_BG = [
	{
		name: '図の説明文',
		source: 'src/styles/tako-theme.css',
		selector: '.tako-shot figcaption',
		match: (s) => s === '.tako-shot figcaption',
	},
	{
		name: 'フッターの法務リンク',
		source: 'src/components/FooterLegal.astro',
		selector: '.tako-legal',
		// Astro の scoped style は `.tako-legal:where(.astro-xxxx)` になる
		match: (s) => /^\.tako-legal(:where\(\.astro-[\w-]+\))?$/.test(s),
	},
];

const THEMES = ['light', 'dark'];

// ---- CSS の読み取り（minify 済みの出力を前提にした小さな解析） ----

/** 文字列と括弧の外にある区切り文字で分ける */
function splitTop(text, sep) {
	const out = [];
	let depth = 0;
	let quote = null;
	let cur = '';
	for (let i = 0; i < text.length; i++) {
		const c = text[i];
		if (quote) {
			cur += c;
			if (c === '\\') cur += text[++i] ?? '';
			else if (c === quote) quote = null;
			continue;
		}
		if (c === '"' || c === "'") quote = c;
		else if (c === '(' || c === '[') depth++;
		else if (c === ')' || c === ']') depth--;
		if (c === sep && depth === 0) {
			out.push(cur);
			cur = '';
			continue;
		}
		cur += c;
	}
	out.push(cur);
	return out;
}

/** `{` の位置から対応する `}` の位置を返す（文字列の中の括弧は数えない） */
function matchBrace(css, open) {
	let depth = 0;
	let quote = null;
	for (let i = open; i < css.length; i++) {
		const c = css[i];
		if (quote) {
			if (c === '\\') i++;
			else if (c === quote) quote = null;
			continue;
		}
		if (c === '"' || c === "'") quote = c;
		else if (c === '{') depth++;
		else if (c === '}' && --depth === 0) return i;
	}
	throw new Error(`CSS の括弧が閉じていない（位置 ${open}）`);
}

/**
 * 規則を平らに並べる。@layer の中は layered = true、@media / @supports などの中は
 * conditional = true（配色の解決には使わない）。@keyframes / @font-face は読み飛ばす
 */
function parseRules(css, ctx = { layered: false, conditional: false }, acc = []) {
	css = css.replace(/\/\*[\s\S]*?\*\//g, '');
	let i = 0;
	while (i < css.length) {
		const open = css.indexOf('{', i);
		const semi = css.indexOf(';', i);
		if (open === -1) break;
		// `@layer a, b;` や `@import …;` のような文だけの at 規則
		if (semi !== -1 && semi < open && css.slice(i, semi).trim().startsWith('@')) {
			i = semi + 1;
			continue;
		}
		const head = css.slice(i, open).trim();
		const close = matchBrace(css, open);
		const body = css.slice(open + 1, close);
		if (head.startsWith('@')) {
			const name = head.slice(1).split(/[\s({]/)[0].toLowerCase();
			if (name === 'layer') parseRules(body, { ...ctx, layered: true }, acc);
			else if (name === 'media' || name === 'supports' || name === 'container' || name === 'scope')
				parseRules(body, { ...ctx, conditional: true }, acc);
			// @keyframes / @font-face / @property などは配色の解決に関係しない
		} else {
			const decls = new Map();
			for (const d of splitTop(body, ';')) {
				const k = d.indexOf(':');
				if (k === -1) continue;
				const prop = d.slice(0, k).trim();
				const value = d.slice(k + 1).replace(/!important\s*$/i, '').trim();
				if (prop) decls.set(prop, value);
			}
			const selectors = splitTop(head, ',').map((s) => s.trim().replace(/\s+/g, ' ').replace(/=(['"])([^'"]*)\1\]/g, '=$2]'));
			acc.push({ selectors, decls, ...ctx });
		}
		i = close + 1;
	}
	return acc;
}

// ---- 色 ----

function parseColor(text) {
	const s = text.trim().toLowerCase();
	let m = s.match(/^#([0-9a-f]{3,8})$/);
	if (m) {
		let h = m[1];
		if (h.length === 3 || h.length === 4) h = [...h].map((c) => c + c).join('');
		if (h.length !== 6 && h.length !== 8) return null;
		const n = (j) => parseInt(h.slice(j, j + 2), 16);
		return { r: n(0), g: n(2), b: n(4), a: h.length === 8 ? n(6) / 255 : 1 };
	}
	m = s.match(/^rgba?\(([^)]*)\)$/);
	if (m) {
		const v = m[1].split(/[\s,/]+/).filter(Boolean);
		if (v.length < 3) return null;
		const ch = (x) => (x.endsWith('%') ? (parseFloat(x) * 255) / 100 : parseFloat(x));
		const a = v[3] === undefined ? 1 : v[3].endsWith('%') ? parseFloat(v[3]) / 100 : parseFloat(v[3]);
		return { r: ch(v[0]), g: ch(v[1]), b: ch(v[2]), a };
	}
	m = s.match(/^hsla?\(([^)]*)\)$/);
	if (m) {
		const v = m[1].split(/[\s,/]+/).filter(Boolean);
		if (v.length < 3) return null;
		const h = (((parseFloat(v[0]) % 360) + 360) % 360) / 360;
		const sat = parseFloat(v[1]) / 100;
		const l = parseFloat(v[2]) / 100;
		const a = v[3] === undefined ? 1 : v[3].endsWith('%') ? parseFloat(v[3]) / 100 : parseFloat(v[3]);
		const q = l < 0.5 ? l * (1 + sat) : l + sat - l * sat;
		const p = 2 * l - q;
		const f = (t) => {
			t = (t + 1) % 1;
			if (t < 1 / 6) return p + (q - p) * 6 * t;
			if (t < 1 / 2) return q;
			if (t < 2 / 3) return p + (q - p) * (2 / 3 - t) * 6;
			return p;
		};
		return { r: f(h + 1 / 3) * 255, g: f(h) * 255, b: f(h - 1 / 3) * 255, a };
	}
	if (s === 'white') return { r: 255, g: 255, b: 255, a: 1 };
	if (s === 'black') return { r: 0, g: 0, b: 0, a: 1 };
	return null;
}

const hex = (c) => '#' + [c.r, c.g, c.b].map((v) => Math.round(v).toString(16).padStart(2, '0')).join('');

function luminance(c) {
	const f = (v) => {
		v /= 255;
		return v <= 0.04045 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4;
	};
	return 0.2126 * f(c.r) + 0.7152 * f(c.g) + 0.0722 * f(c.b);
}

/** 文字色（半透明なら背景に重ねる）と背景のコントラスト比 */
function contrast(fg, bg) {
	const c = { r: fg.r * fg.a + bg.r * (1 - fg.a), g: fg.g * fg.a + bg.g * (1 - fg.a), b: fg.b * fg.a + bg.b * (1 - fg.a) };
	const a = luminance(c);
	const b = luminance(bg);
	return (Math.max(a, b) + 0.05) / (Math.min(a, b) + 0.05);
}

// ---- 変数の解決 ----

function tokensFor(rules, theme) {
	const map = new Map();
	const sels = [':root', `:root[data-theme=${theme}]`];
	// 層の中 → 層の外、それぞれ詳細度の低い :root → :root[data-theme=…] の順に上書きする
	for (const layered of [true, false]) {
		for (const sel of sels) {
			for (const r of rules) {
				if (r.conditional || r.layered !== layered || !r.selectors.includes(sel)) continue;
				for (const [k, v] of r.decls) if (k.startsWith('--')) map.set(k, v);
			}
		}
	}
	return map;
}

function resolveValue(value, tokens, depth = 0) {
	if (depth > 20) throw new Error(`変数の参照が循環している: ${value}`);
	const at = value.indexOf('var(');
	if (at === -1) return value;
	let d = 0;
	let end = at + 3;
	for (; end < value.length; end++) {
		if (value[end] === '(') d++;
		else if (value[end] === ')' && --d === 0) break;
	}
	const inner = value.slice(at + 4, end);
	const [name, ...rest] = splitTop(inner, ',');
	const fallback = rest.length ? rest.join(',').trim() : null;
	const got = tokens.get(name.trim());
	const sub = got !== undefined ? resolveValue(got, tokens, depth + 1) : fallback !== null ? resolveValue(fallback, tokens, depth + 1) : null;
	if (sub === null) return null;
	return resolveValue(value.slice(0, at) + sub + value.slice(end + 1), tokens, depth + 1);
}

function colorOf(name, value, tokens, errors, theme) {
	const resolved = value === null ? null : resolveValue(value, tokens);
	const c = resolved === null ? null : parseColor(resolved);
	if (!c) errors.push(`[${theme}] ${name}: 色を解釈できない（${value ?? '未定義'} → ${resolved ?? '未定義'}）`);
	return c;
}

// ---- 検査 ----

if (!existsSync(CSS_DIR)) {
	console.error(`NG: ${CSS_DIR} が無い（先に npm run build）`);
	process.exit(1);
}
const files = readdirSync(CSS_DIR)
	.filter((n) => n.endsWith('.css'))
	.sort();
const rules = files.flatMap((n) => parseRules(readFileSync(join(CSS_DIR, n), 'utf8')));

const errors = [];
const lines = [];
for (const theme of THEMES) {
	const tokens = tokensFor(rules, theme);
	const bg = colorOf('--sl-color-bg', 'var(--sl-color-bg)', tokens, errors, theme);
	if (!bg) continue;

	// 1. 灰色の段の向き
	const grays = [1, 2, 3, 4, 5, 6, 7].map((k) => {
		const c = colorOf(`--sl-color-gray-${k}`, `var(--sl-color-gray-${k})`, tokens, errors, theme);
		return c && { k, c, ratio: contrast(c, bg) };
	});
	if (grays.every(Boolean)) {
		const desc = grays.map((g) => `gray-${g.k} ${hex(g.c)}=${g.ratio.toFixed(2)}`).join(' / ');
		lines.push(`[${theme}] 灰色の段（背景 ${hex(bg)} との比）: ${desc}`);
		for (let i = 1; i < grays.length; i++) {
			const a = grays[i - 1];
			const b = grays[i];
			if (!(a.ratio > b.ratio)) {
				errors.push(
					`[${theme}] 灰色の段が逆向き: --sl-color-gray-${a.k}（${hex(a.c)}・背景との比 ${a.ratio.toFixed(2)}）より ` +
						`--sl-color-gray-${b.k}（${hex(b.c)}・${b.ratio.toFixed(2)}）の方が背景から遠い。` +
						'Starlight の約束は gray-1 = 文字の側・gray-7 = 背景の側（src/styles/tako-theme.css）',
				);
				break;
			}
		}
	}

	// 2. 背景の上の薄い文字
	for (const t of TEXT_ON_BG) {
		const hits = rules.filter((r) => !r.conditional && r.decls.has('color') && r.selectors.some(t.match));
		if (!hits.length) {
			errors.push(`[${theme}] ${t.name}（${t.selector}）の color の規則が出力に無い（${t.source} の名前を変えたら TEXT_ON_BG も直す）`);
			continue;
		}
		const value = hits[hits.length - 1].decls.get('color');
		const fg = colorOf(`${t.name}（${t.selector}）`, value, tokens, errors, theme);
		if (!fg) continue;
		const r = contrast(fg, bg);
		lines.push(`[${theme}] ${t.name}（${t.selector}）: 文字 ${hex(fg)}（${value}）/ 背景 ${hex(bg)} → ${r.toFixed(2)}`);
		if (r < MIN_TEXT) {
			errors.push(
				`[${theme}] ${t.name}（${t.selector}）が背景に溶ける: 文字 ${hex(fg)}（${value}）/ 背景 ${hex(bg)} → ` +
					`コントラスト比 ${r.toFixed(2)}（${MIN_TEXT} 以上が要る。${t.source}）`,
			);
		}
	}
}

console.log(`対象: ${CSS_DIR}（CSS ${files.length} 本 / 規則 ${rules.length} 個）`);
for (const l of lines) console.log('  ' + l);
if (errors.length) {
	for (const e of errors) console.error('NG: ' + e);
	process.exit(1);
}
console.log(`OK: 両方の配色で灰色の段が gray-1（文字の側）→ gray-7（背景の側）の向きに並び、図の説明文とフッターの法務リンクが背景に対して ${MIN_TEXT} 以上`);
