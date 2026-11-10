/**
 * 内部リンク検査（scripts/verify-links.mjs）の境界テスト（Issue #2898）。
 *
 *   node scripts/test-verify-links.mjs
 *
 * 一時ディレクトリに小さなビルド出力を組み、検査の判定を固定する。見るもの:
 *   - 同じページ内の断片（`href="#..."`）は、そのページ自身の id と突き合わせる
 *     （別ページにだけ在る id では通さない。切れはページと href で名指す）
 *   - 空の `#` と `#_top` は数えない（id が無くても落とさない）
 *   - Starlight の見出しの id・手で置いたアンカー（`tako-anchor` の div）・`name="..."` を id として数える
 *   - `%` で符号化した断片（本文のリンク）も、字面のままの断片（目次）も同じ id に当たる。
 *     壊れた `%` 列は例外で止まらず、切れとして名指す
 *   - 別ページへのリンク（`/b/#x`）の検査は従来どおり
 *   - CLI: 置き場を引数で渡せる・切れがあれば終了コード 1・同じページ内の本数の下限（足場）で止まる
 */
import { spawnSync } from 'node:child_process';
import { mkdirSync, mkdtempSync, realpathSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, resolve, sep } from 'node:path';
import { fileURLToPath } from 'node:url';
import { checkSite } from './verify-links.mjs';

const HERE = dirname(fileURLToPath(import.meta.url));
const SCRIPT = join(HERE, 'verify-links.mjs');
const TMP = realpathSync(tmpdir());
const fails = [];
let cases = 0;
const made = [];

/** `{ 'a/index.html': '<html>…' }` からビルド出力を組む */
function site(files) {
	const dir = mkdtempSync(join(TMP, 'tako-verify-links-'));
	made.push(dir);
	for (const [rel, body] of Object.entries(files)) {
		const full = join(dir, rel);
		mkdirSync(dirname(full), { recursive: true });
		writeFileSync(full, body);
	}
	return dir;
}

const page = (body) => `<!doctype html><html><head><title>t</title></head><body>${body}</body></html>`;

function expect(name, cond, detail) {
	cases++;
	if (!cond) fails.push(`${name}: ${detail}`);
}

// ---- checkSite の判定 ----------------------------------------------------------

{
	const r = checkSite(
		site({
			'a/index.html': page(
				[
					'<h1 id="_top">A</h1>',
					'<h2 id="setup">設定</h2>',
					'<h2 id="起動">起動</h2>',
					'<div id="old-name" class="tako-anchor"></div><h2 id="new-name">新しい名前</h2>',
					'<a name="legacy"></a>',
					// 通るもの
					'<a href="#setup">目次</a>',
					'<a href="#起動">目次（字面のまま）</a>',
					'<a href="#%E8%B5%B7%E5%8B%95">本文（% で符号化）</a>',
					'<a href="#old-name">旧 id（手置きのアンカー）</a>',
					'<a href="#legacy">name の印</a>',
					// 数えないもの
					'<a href="#">空</a>',
					'<a href="#_top">概要</a>',
					// 切れ
					'<a href="#gone">改名で消えた見出し</a>',
					'<a href="#only-in-b">別ページにだけ在る id</a>',
					'<a href="#%E3">壊れた % 列</a>',
				].join(''),
			),
			'b/index.html': page('<h2 id="only-in-b">B</h2><a href="#_top">概要（_top の id 無し）</a>'),
		}),
	);
	const same = r.fails.filter((f) => f.includes('同じページに id が無い'));
	expect('同じページ内: 数えた本数', r.checkedSamePage === 8, `checkedSamePage=${r.checkedSamePage}（期待 8）`);
	expect('同じページ内: 対象外の本数', r.exemptSamePage === 3, `exemptSamePage=${r.exemptSamePage}（期待 3 = # 1 + #_top 2）`);
	expect('同じページ内: 切れの件数', same.length === 3, `切れ ${same.length} 件（期待 3）: ${JSON.stringify(r.fails)}`);
	for (const href of ['#gone', '#only-in-b', '#%E3']) {
		expect(
			`同じページ内: ${href} をページと href で名指す`,
			r.fails.includes(`/a/index.html: 同じページに id が無い ${href}`),
			JSON.stringify(r.fails),
		);
	}
	for (const href of ['#setup', '#起動', '#%E8%B5%B7%E5%8B%95', '#old-name', '#legacy', '#', '#_top']) {
		expect(`同じページ内: ${href} は落とさない`, !r.fails.some((f) => f.endsWith(` ${href}`)), JSON.stringify(r.fails));
	}
	expect('#_top の id が無いページでも落とさない', !r.fails.some((f) => f.startsWith('/b/')), JSON.stringify(r.fails));
}

// 別ページへのリンクは従来どおり（同じページ内の検査を足しても壊さない）
{
	const r = checkSite(
		site({
			'a/index.html': page(
				'<a href="/b/#x">ok</a><a href="/b/#%E8%A6%8B">ok（符号化）</a><a href="/b/#nope">切れ</a><a href="/missing/">無いページ</a><img src="/img.png">',
			),
			'b/index.html': page('<h2 id="x">x</h2><h2 id="見">見</h2>'),
			'img.png': 'png',
		}),
	);
	expect('別ページ: 数えた本数', r.checkedLinks === 5 && r.checkedFragments === 3, `links=${r.checkedLinks} fragments=${r.checkedFragments}`);
	expect(
		'別ページ: 切れは 2 件',
		r.fails.length === 2 &&
			r.fails.includes('/a/index.html: リンク先に id が無い /b/#nope') &&
			r.fails.includes('/a/index.html: リンク先が無い /missing/'),
		JSON.stringify(r.fails),
	);
	expect('別ページ: 同じページ内は 0 本', r.checkedSamePage === 0 && r.exemptSamePage === 0, JSON.stringify(r));
}

// ---- CLI（終了コード・名指し・足場） --------------------------------------------

/** 足場の下限（20 ページ・内部リンク 100 本・同じページ内 100 本）を越える大きさの置き場 */
function bigSite({ samePerPage = 5, broken = false } = {}) {
	const files = {};
	for (let i = 0; i < 20; i++) {
		const heads = Array.from({ length: samePerPage }, (_, k) => `<h2 id="s${k}">s${k}</h2>`).join('');
		const toc = Array.from({ length: samePerPage }, (_, k) => `<a href="#s${k}">s${k}</a>`).join('');
		const cross = Array.from({ length: 5 }, (_, k) => `<a href="/p${(i + k + 1) % 20}/">p</a>`).join('');
		files[`p${i}/index.html`] = page(`${heads}${toc}${cross}<a href="#_top">概要</a>`);
	}
	if (broken) files['p3/index.html'] = files['p3/index.html'].replace('<h2 id="s2">', '<h2>');
	return site(files);
}

const run = (dir) => spawnSync(process.execPath, [SCRIPT, dir], { encoding: 'utf8' });

{
	const r = run(bigSite());
	expect('CLI: 切れが無ければ 0', r.status === 0, `status=${r.status} stderr=${r.stderr}`);
	expect(
		'CLI: 同じページ内の本数と対象外の本数を出す',
		r.stdout.includes('同じページ内の断片 100 本（対象外の `#`・`#_top` 20 本）'),
		r.stdout,
	);
}
{
	const r = run(bigSite({ broken: true }));
	expect('CLI: 切れがあれば 1', r.status === 1, `status=${r.status}`);
	expect(
		'CLI: 切れをページと href で名指す',
		r.stderr.includes('FAILED 1 件') && r.stderr.includes('/p3/index.html: 同じページに id が無い #s2'),
		r.stderr,
	);
}
{
	const r = run(bigSite({ samePerPage: 4 }));
	expect(
		'CLI: 同じページ内が下限を割ったら走査の壊れとして止まる',
		r.status === 1 && r.stderr.includes('同じページ内のリンクが 80 本しか拾えていない'),
		`status=${r.status} stderr=${r.stderr}`,
	);
}
{
	const missing = join(TMP, `tako-verify-links-missing-${process.pid}`);
	const r = run(missing);
	expect('CLI: 置き場が無ければ 1', r.status === 1 && r.stderr.includes('が無い'), `status=${r.status} stderr=${r.stderr}`);
}

// ---- 後片付け（自分が作った一時ディレクトリだけ） --------------------------------------

for (const dir of made) {
	if (!resolve(dir).startsWith(TMP + sep) || !dir.includes('tako-verify-links-')) {
		throw new Error(`一時ディレクトリの外は消さない: ${dir}`);
	}
	rmSync(dir, { recursive: true, force: true });
}

if (fails.length) {
	console.error(`FAILED ${fails.length} / ${cases} 件:`);
	for (const f of fails) console.error(`  - ${f}`);
	process.exit(1);
}
console.log(`OK: 内部リンク検査の境界 ${cases} 件（同じページ内の断片・対象外の # と #_top・手置きのアンカー・符号化・CLI の足場）`);
