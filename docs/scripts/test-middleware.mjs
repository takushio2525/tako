/**
 * 旧ドメイン転送（functions/_middleware.js）の境界テスト。
 *
 *   node scripts/test-middleware.mjs
 *
 * 見るもの:
 *   - 旧ドメイン宛は 301 で、転送先のオリジンが常に https://tako.takushio2525.com
 *     （パスに `//evil.example` やエンコード・バックスラッシュを混ぜても外へ出ない）
 *   - パスとクエリはそのまま持ち越し、ポートとスキームは持ち越さない
 *   - FQDN の末尾ドット・大文字でも旧ドメインとして扱う
 *   - それ以外のホスト（本番・プレビュー・紛らわしい名前）は転送せず next() へ渡す
 */
import { onRequest } from '../functions/_middleware.js';

const CANONICAL = 'https://tako.takushio2525.com';
const NEXT = Symbol('next');
const fails = [];
let cases = 0;

function call(url) {
	return onRequest({ request: new Request(url), next: () => NEXT });
}

function redirects(url, expected) {
	cases++;
	const res = call(url);
	if (res === NEXT) return fails.push(`${url}: 転送されなかった`);
	const location = res.headers.get('location');
	if (res.status !== 301) fails.push(`${url}: status ${res.status}（期待 301）`);
	if (new URL(location).origin !== CANONICAL)
		fails.push(`${url}: 転送先のオリジンが ${new URL(location).origin}（${location}）`);
	if (expected !== undefined && location !== expected)
		fails.push(`${url}: Location ${location}（期待 ${expected}）`);
}

function passes(url) {
	cases++;
	if (call(url) !== NEXT) fails.push(`${url}: 転送された（next() へ渡すべき）`);
}

// パスとクエリは持ち越す
redirects('https://tako-docs.pages.dev/', `${CANONICAL}/`);
redirects('https://tako-docs.pages.dev/getting-started/?a=1&b=%2F', `${CANONICAL}/getting-started/?a=1&b=%2F`);

// ポート・スキーム・ホスト名の表記ゆれ
redirects('https://tako-docs.pages.dev:8443/getting-started/', `${CANONICAL}/getting-started/`);
redirects('http://tako-docs.pages.dev/getting-started/', `${CANONICAL}/getting-started/`);
redirects('https://tako-docs.pages.dev./getting-started/', `${CANONICAL}/getting-started/`);
redirects('https://TAKO-DOCS.PAGES.DEV/', `${CANONICAL}/`);

// 外へ出そうとするパス（オリジンが固定のままか）
for (const path of [
	'//evil.example/',
	'///evil.example/',
	'/%2f%2fevil.example',
	'/%5cevil.example',
	'/\\evil.example',
	'/\\\\evil.example',
	'/https://evil.example/',
	'/@evil.example',
	'/..%2f..%2fevil.example',
	'/%00',
	'/no-such-page-xyz/',
]) {
	redirects(`https://tako-docs.pages.dev${path}`);
}

// 転送しないホスト
passes('https://tako.takushio2525.com/');
passes('https://tako.takushio2525.com//evil.example/');
passes('https://abc123.tako-docs.pages.dev/');
passes('https://feature-x.tako-docs.pages.dev/');
passes('https://tako-docs.pages.dev.evil.example/');
passes('https://evil-tako-docs.pages.dev/');
passes('https://evil.example/tako-docs.pages.dev/');
passes('http://localhost:8788/');

console.log(`ケース ${cases} 件`);
if (fails.length) {
	console.error(`\nFAILED ${fails.length} 件:`);
	for (const f of fails) console.error(`  - ${f}`);
	process.exit(1);
}
console.log('OK: 旧ドメインは常に https://tako.takushio2525.com へ 301、それ以外は転送しない');
