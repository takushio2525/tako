/**
 * releases.md が現行の版から取り残されていないかの番犬（Issue #1546）。
 *
 *   node docs/scripts/check-releases-page.mjs
 *   node docs/scripts/check-releases-page.mjs --version=0.9.0        # 将来の版を偽装して試す
 *   node docs/scripts/check-releases-page.mjs --releases=<path>      # 別ファイルを検査する
 *
 * 経緯: v0.8.0 が出てから 25 日間、ページの見出しは「v0.7.0 — 最新の安定版」のままだった。
 * ページ自身のメンテナ向けコメントが「マイナー以上が出たら節を足す」と書いていても、
 * 人が読む規約は破られる。**機械が落とす**形にしておく。
 *
 * 見るもの:
 *   A. 現行の版の minor 系列（例 v0.8）の節がある
 *   B. その節がページの先頭の版節である（新しい順に並んでいる）
 *   C. 「最新の安定版」のラベルがその節の中にしか出てこない（古い節に置き忘れていない）
 *
 * 「現行の版」の採り方: Cargo.toml の workspace version と CHANGELOG の最新節、
 * それに git tag（取れたときだけ）の**最大値**。CI の checkout はタグを持たない
 * （fetch-depth: 1）ので、タグだけに頼ると番犬が常に素通りしてしまう。逆に
 * Cargo.toml は release.sh がタグより先に bump するので、タグより早く気づける。
 */
import { execFileSync } from 'node:child_process';
import { readFileSync } from 'node:fs';
import { dirname, join, relative, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const HERE = dirname(fileURLToPath(import.meta.url));
const ROOT = resolve(HERE, '..', '..');

const arg = (name) =>
	process.argv.slice(2).find((a) => a.startsWith(`--${name}=`))?.slice(name.length + 3);

const RELEASES = resolve(arg('releases') ?? join(ROOT, 'docs/src/content/docs/releases.md'));
// 表示はリポジトリ相対に寄せる（CI のログにも手元のホームパスにも同じ文字列が出る）
const SHOWN = relative(ROOT, RELEASES) || RELEASES;
const LABEL = '最新の安定版';

/** "1.2.3" → [1,2,3]。semver 以外は null */
const parse = (v) => {
	const m = /^(\d+)\.(\d+)\.(\d+)$/.exec(v?.trim() ?? '');
	return m ? [Number(m[1]), Number(m[2]), Number(m[3])] : null;
};
const cmp = (a, b) => a[0] - b[0] || a[1] - b[1] || a[2] - b[2];
const series = (v) => `v${v[0]}.${v[1]}`;

/** 現行の版を、取れた供給源すべての最大値として決める */
function currentVersion() {
	const forced = arg('version');
	if (forced) {
		const v = parse(forced);
		if (!v) fail([`--version=${forced} は x.y.z の形ではありません`]);
		return { version: v, from: [`--version=${forced}`] };
	}

	const found = [];
	const cargo = /^\[workspace\.package\][\s\S]*?^version\s*=\s*"([^"]+)"/m.exec(
		readFileSync(join(ROOT, 'Cargo.toml'), 'utf8'),
	)?.[1];
	if (parse(cargo)) found.push({ v: parse(cargo), from: `Cargo.toml (${cargo})` });

	const changelog = /^## \[(\d+\.\d+\.\d+)\]/m.exec(
		readFileSync(join(ROOT, 'CHANGELOG.md'), 'utf8'),
	)?.[1];
	if (parse(changelog)) found.push({ v: parse(changelog), from: `CHANGELOG.md (${changelog})` });

	// タグは「あれば見る」。浅い checkout では 1 本も無いのが正常なので、無くても落とさない
	try {
		const tags = execFileSync('git', ['tag', '-l', 'v[0-9]*'], { cwd: ROOT, encoding: 'utf8' })
			.split('\n')
			.map((t) => parse(t.replace(/^v/, '')))
			.filter(Boolean)
			.sort(cmp);
		const latest = tags.at(-1);
		if (latest) found.push({ v: latest, from: `git tag (v${latest.join('.')})` });
	} catch {
		/* git が無い / リポジトリでない: タグ以外の供給源で判定する */
	}

	if (found.length === 0) fail(['現行の版を 1 つも決められませんでした（Cargo.toml / CHANGELOG.md / git tag）']);
	found.sort((a, b) => cmp(a.v, b.v));
	const top = found.at(-1);
	return { version: top.v, from: found.map((f) => f.from) };
}

function fail(lines) {
	console.error('releases.md の番犬が落ちました（Issue #1546）:\n');
	for (const l of lines) console.error(`  - ${l}`);
	console.error(`\n直し方: ${SHOWN} に現行の系列の節を足し、「${LABEL}」のラベルをそこへ移す。`);
	console.error('  書き方はファイル冒頭のメンテナ向けコメントにあります。');
	process.exit(1);
}

const { version, from } = currentVersion();
const want = series(version);

const text = readFileSync(RELEASES, 'utf8');
const lines = text.split('\n');

// `## ` 見出しを拾い、`v<major>.<minor>` を名乗るものだけを版節とみなす
const sections = [];
lines.forEach((line, i) => {
	if (!line.startsWith('## ')) return;
	const m = /v(\d+)\.(\d+)/.exec(line);
	sections.push({ line, index: i, series: m ? `v${m[1]}.${m[2]}` : null });
});
const versionSections = sections.filter((s) => s.series);

const errors = [];

// A: 現行の系列の節がある
const cur = versionSections.find((s) => s.series === want);
if (!cur) {
	errors.push(
		`現行は ${want} 系（${from.join(' / ')}）ですが、${want} の節がありません。` +
			`ページにあるのは ${versionSections.map((s) => s.series).join(' / ') || '（版節なし）'} です`,
	);
} else if (versionSections[0] !== cur) {
	// B: 新しい順。現行の節が先頭に無い = 足したが置き場所が違う
	errors.push(
		`${want} の節が ${SHOWN}:${cur.index + 1} にありますが、` +
			`先頭の版節は ${versionSections[0].series}（${versionSections[0].index + 1} 行目）です。版節は新しい順に並べます`,
	);
}

// C: ラベルは現行の節の中にだけ
if (cur) {
	const next = sections.find((s) => s.index > cur.index);
	const start = cur.index;
	const end = next ? next.index : lines.length;
	lines.forEach((line, i) => {
		if (!line.includes(LABEL)) return;
		if (i >= start && i < end) return;
		if (i < sections[0]?.index) return; // 冒頭のメンテナ向けコメント・導入は対象外
		errors.push(
			`「${LABEL}」が ${want} の節の外に残っています（${SHOWN}:${i + 1}）: ${line.trim()}`,
		);
	});
}

if (errors.length) fail(errors);

console.log(
	`releases.md OK: 現行 ${want} 系（${from.join(' / ')}）の節が先頭にあり、「${LABEL}」もその中にあります`,
);
