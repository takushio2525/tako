/**
 * 配信ヘッダの検査。ビルド出力の dist/_headers（既定）か、デプロイ済みの実 URL を対象にできる。
 *
 *   node scripts/verify-headers.mjs                       # dist/_headers を検査
 *   node scripts/verify-headers.mjs https://example.com   # 実 URL のレスポンスヘッダを検査
 *
 * 見るもの（全ページ = `/*` に付いていること）:
 *   - HSTS が 1 年以上
 *   - 埋め込みの禁止（CSP の frame-ancestors 'none' と X-Frame-Options: DENY）
 *   - CSP の base-uri / object-src / form-action
 *   - nosniff・Referrer-Policy・Permissions-Policy
 */
import { existsSync, readFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const HERE = dirname(fileURLToPath(import.meta.url));
const DIST = join(resolve(HERE, '..'), 'dist');
const base = process.argv[2]?.replace(/\/$/, '');

/** `_headers` の `/*` ブロックを { 小文字の名前: 値 } にする（ファイルが無ければ空） */
function fromHeadersFile() {
	const file = join(DIST, '_headers');
	if (!existsSync(file)) return {};
	const lines = readFileSync(file, 'utf8').split('\n');
	const headers = {};
	let inAll = false;
	for (const line of lines) {
		if (/^\s*#/.test(line) || !line.trim()) continue;
		if (!/^\s/.test(line)) {
			inAll = line.trim() === '/*';
			continue;
		}
		if (!inAll) continue;
		const i = line.indexOf(':');
		headers[line.slice(0, i).trim().toLowerCase()] = line.slice(i + 1).trim();
	}
	return headers;
}

const fails = [];

function check(where, h) {
	const at = (msg) => fails.push(`${where}: ${msg}`);
	const hsts = h['strict-transport-security'] ?? '';
	const maxAge = Number(hsts.match(/max-age=(\d+)/)?.[1] ?? 0);
	if (maxAge < 31536000) at(`HSTS の max-age が 1 年未満（${hsts || 'なし'}）`);
	const csp = h['content-security-policy'] ?? '';
	for (const d of ["frame-ancestors 'none'", "base-uri 'self'", "object-src 'none'", "form-action 'self'"]) {
		if (!csp.includes(d)) at(`CSP に ${d} が無い（${csp || 'なし'}）`);
	}
	// script-src / default-src を足すなら、検索（Pagefind）の WebAssembly に 'wasm-unsafe-eval' が要る
	if (/(^|;)\s*(script-src|default-src)\s/.test(csp) && !csp.includes("'wasm-unsafe-eval'"))
		at("CSP がスクリプトを縛っているのに 'wasm-unsafe-eval' が無い（検索が止まる）");
	if ((h['x-frame-options'] ?? '').toUpperCase() !== 'DENY') at(`X-Frame-Options が DENY でない（${h['x-frame-options'] ?? 'なし'}）`);
	if (!/^nosniff$/i.test((h['x-content-type-options'] ?? '').split(',')[0].trim()))
		at(`X-Content-Type-Options が nosniff でない（${h['x-content-type-options'] ?? 'なし'}）`);
	if (!h['referrer-policy']) at('Referrer-Policy が無い');
	const pp = h['permissions-policy'] ?? '';
	for (const f of ['camera=()', 'microphone=()', 'geolocation=()']) {
		if (!pp.includes(f)) at(`Permissions-Policy に ${f} が無い（${pp || 'なし'}）`);
	}
}

if (base) {
	// 静的ページ・生成ページ・存在しないページ（404）のどれにも付くこと
	for (const path of ['/', '/getting-started/', '/agent-support/', '/no-such-page-xyz/']) {
		const url = base + path;
		try {
			const res = await fetch(url, { redirect: 'manual' });
			const h = Object.fromEntries([...res.headers].map(([k, v]) => [k.toLowerCase(), v]));
			check(`${url} (HTTP ${res.status})`, h);
		} catch (e) {
			fails.push(`${url}: ${e.message}`);
		}
	}
} else {
	check('dist/_headers', fromHeadersFile());
}

console.log(`対象: ${base ?? 'dist/_headers'}`);
if (fails.length) {
	console.error(`\nFAILED ${fails.length} 件:`);
	for (const f of fails) console.error(`  - ${f}`);
	process.exit(1);
}
console.log('OK: HSTS・埋め込み禁止・CSP の基本指令・nosniff・Referrer-Policy・Permissions-Policy');
