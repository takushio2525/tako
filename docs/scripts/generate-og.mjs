/**
 * OG 画像ジェネレータ（1200x630・ページ単位）
 *
 * `src/content/docs/**` の frontmatter からページ一覧を作り、サイトと同じ配色・
 * タイポグラフィのカードを headless Chrome で描いて `public/og/*.png` に書き出す。
 * 併せて `src/data/og-manifest.json`（route middleware が読む索引）を更新する。
 *
 * 生成物はリポジトリにコミットする（CI / Cloudflare Pages のビルド環境に
 * 日本語フォントがある保証が無いため、ビルド時生成にはしない）。ページを
 * 追加・改題したら `npm run og` を実行して差分をコミットすること。
 * 未生成のページは manifest に載らず、middleware がトップの画像へ落とす。
 *
 * 実行要件: macOS + Google Chrome + ネットワーク（Google Fonts の Inter）。
 *
 * ロゴの不変条件（最重要）:
 *   マスコットは幅だけを指定し `height: auto` で描く。SVG 側の
 *   preserveAspectRatio も既定（xMidYMid meet）のままなので、縦横比が崩れる
 *   経路が構造的に存在しない。加えて描画後に実測比と viewBox 比を突き合わせ、
 *   ずれていれば生成を失敗させる（LOGO_RATIO_TOLERANCE）。
 */
import { execFileSync } from 'node:child_process';
import { mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, statSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, relative, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import sharp from 'sharp';

const HERE = dirname(fileURLToPath(import.meta.url));
const DOCS = resolve(HERE, '..');
const CONTENT = join(DOCS, 'src/content/docs');
const OUT_DIR = join(DOCS, 'public/og');
const MANIFEST = join(DOCS, 'src/data/og-manifest.json');
const MASCOT = join(DOCS, 'public/tako-mascot.svg');

const CHROME = '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome';
const WIDTH = 1200;
const HEIGHT = 630;
/** 実測比と viewBox 比のずれの許容（サブピクセル丸めぶんだけ） */
const LOGO_RATIO_TOLERANCE = 0.002;

/* ── サイトの配色（src/styles/tako-theme.css のダークモードと同じ値） ── */
const C = {
  bg: '#191520',
  card: '#221d2b',
  surface: '#251f2e',
  ink: '#ede7f0',
  inkMuted: '#a79fac',
  inkSubtle: '#7b7388',
  accent: '#f5809f',
  accentStrong: '#ffa8c2',
  green: '#63d493',
  glowPink: 'rgba(245, 128, 159, 0.18)',
  glowGreen: 'rgba(99, 212, 147, 0.13)',
  line: 'rgba(245, 128, 159, 0.26)',
};

/**
 * サイドバー（astro.config.mjs）のグループと同じ肩書きを付ける。
 * 未分類のページがあれば落とす = 新設ページの取りこぼしに気づける。
 */
const SECTIONS = [
  [/^getting-started(\/|$)|^releases$|^windows-support$/, 'はじめに'],
  [/^features\/(orchestration|orchestrator|mcp-server)$/, 'AI と使う'],
  [/^features\//, '機能紹介'],
  [/^guides\//, '使い方ガイド'],
  [/^development\//, '開発者向け'],
];

function sectionOf(slug) {
  for (const [re, label] of SECTIONS) if (re.test(slug)) return label;
  throw new Error(`セクション未分類のページ: ${slug}（generate-og.mjs の SECTIONS に追加すること）`);
}

/** frontmatter から title / description を読む（このサイトは 1 行文字列しか使っていない） */
function frontmatter(file) {
  const text = readFileSync(file, 'utf8');
  const m = text.match(/^---\r?\n([\s\S]*?)\r?\n---/);
  if (!m) throw new Error(`frontmatter が無い: ${file}`);
  const out = {};
  for (const line of m[1].split(/\r?\n/)) {
    const kv = line.match(/^(title|description):\s*(.+)$/);
    if (kv) out[kv[1]] = kv[2].trim().replace(/^["']|["']$/g, '');
  }
  if (!out.title) throw new Error(`title が読めない: ${file}`);
  return out;
}

/** 内容ファイルのパス → サイト上の slug（'' はトップ）と URL パス */
function routeOf(rel) {
  const noExt = rel.replace(/\.(md|mdx)$/, '');
  const slug = noExt.replace(/(^|\/)index$/, '');
  return { slug, pathname: slug === '' ? '/' : `/${slug}/` };
}

function walk(dir, acc = []) {
  for (const name of readdirSync(dir).sort()) {
    const full = join(dir, name);
    if (statSync(full).isDirectory()) walk(full, acc);
    else if (/\.(md|mdx)$/.test(name)) acc.push(full);
  }
  return acc;
}

/** URL パス → 画像のファイル名（middleware 側と同じ規則） */
function imageKey(slug) {
  return slug === '' ? 'index' : slug.replace(/\//g, '-');
}

const esc = (s) => String(s).replace(/[&<>"]/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' })[c]);

/* ── マスコット SVG の viewBox から「正しい縦横比」を取る ── */
const mascotSvg = readFileSync(MASCOT, 'utf8');
const viewBox = mascotSvg.match(/viewBox="([\d.\-\s]+)"/);
if (!viewBox) throw new Error('マスコット SVG の viewBox が読めない');
const [, , vbW, vbH] = viewBox[1].trim().split(/\s+/).map(Number);
const LOGO_RATIO = vbW / vbH;
const mascotDataUri = `data:image/svg+xml;base64,${Buffer.from(mascotSvg).toString('base64')}`;

const SHARED_CSS = `
  @import url('https://fonts.googleapis.com/css2?family=Inter:wght@400;600;700;800&family=JetBrains+Mono:wght@500&display=swap');
  * { margin: 0; padding: 0; box-sizing: border-box; }
  html, body { width: ${WIDTH}px; height: ${HEIGHT}px; }
  body {
    background: ${C.bg};
    color: ${C.ink};
    font-family: 'Inter', 'Hiragino Sans', 'Noto Sans JP', system-ui, sans-serif;
    -webkit-font-smoothing: antialiased;
    overflow: hidden;
    position: relative;
  }
  /* サイトのヒーローと同じ二色のグロー */
  body::before, body::after { content: ''; position: absolute; border-radius: 50%; }
  body::before { width: 900px; height: 900px; left: -280px; top: -420px; background: radial-gradient(circle, ${C.glowPink} 0%, transparent 65%); }
  body::after  { width: 820px; height: 820px; right: -300px; bottom: -420px; background: radial-gradient(circle, ${C.glowGreen} 0%, transparent 65%); }
  .frame { position: relative; width: 100%; height: 100%; padding: 64px 72px; display: flex; flex-direction: column; }
  /* ロゴは幅だけ指定 + height:auto = 縦横比が崩れる余地が無い */
  .mascot { height: auto; display: block; }
  .wordmark { font-weight: 800; letter-spacing: -0.02em; }
  .mono { font-family: 'JetBrains Mono', ui-monospace, Menlo, monospace; }
  .site { color: ${C.inkSubtle}; font-size: 22px; letter-spacing: 0.01em; }
`;

/**
 * トップページ: マスコット + サイト既存のキャッチコピー。
 * ページタイトル（「tako とは」）は大きなワードマークと重複するので使わない。
 */
const HERO_BADGE = 'AI エージェント時代のターミナル';
const HERO_CATCH = 'AI エージェントのための次世代ターミナル';
const HERO_SUB = '設定ゼロで AI が画面を組み立てる macOS 向け GUI ターミナル';

function heroHtml() {
  return `<!doctype html><html lang="ja"><meta charset="utf-8"><style>${SHARED_CSS}
    .frame { flex-direction: row; align-items: center; gap: 48px; }
    .left { flex: 1 1 auto; min-width: 0; }
    .badge { display: inline-flex; align-items: center; gap: 10px; padding: 10px 20px; border-radius: 999px;
      background: rgba(245,128,159,0.13); border: 1px solid ${C.line}; color: ${C.accentStrong};
      font-size: 22px; font-weight: 600; }
    .badge i { width: 9px; height: 9px; border-radius: 50%; background: ${C.accent}; display: block; }
    .name { font-size: 128px; line-height: 1; margin: 24px 0 0; }
    .catch { font-size: 35px; font-weight: 700; line-height: 1.3; margin-top: 16px; letter-spacing: -0.01em; white-space: nowrap; }
    .sub { font-size: 24px; line-height: 1.6; color: ${C.inkMuted}; margin-top: 14px; text-wrap: balance; }
    .right { flex: 0 0 auto; }
    .foot { position: absolute; left: 72px; bottom: 44px; }
  </style><body><div class="frame">
    <div class="left">
      <div class="badge"><i></i>${esc(HERO_BADGE)}</div>
      <div class="name wordmark">tako</div>
      <div class="catch">${esc(HERO_CATCH)}</div>
      <div class="sub">${esc(HERO_SUB)}</div>
    </div>
    <div class="right"><img class="mascot" id="logo" src="${mascotDataUri}" width="300" alt=""></div>
    <div class="foot site mono">tako-docs.pages.dev</div>
  </div>${FIT_SCRIPT}</body></html>`;
}

/** 下層ページ: 共通の土台 + そのページのタイトル / 説明 */
function pageHtml({ title, description, section }) {
  return `<!doctype html><html lang="ja"><meta charset="utf-8"><style>${SHARED_CSS}
    .head { display: flex; align-items: center; gap: 16px; }
    .head .wordmark { font-size: 40px; }
    .sep { width: 1px; height: 34px; background: rgba(237,231,240,0.18); }
    .section { font-size: 26px; font-weight: 600; color: ${C.accentStrong}; letter-spacing: 0.01em; }
    .section span { color: ${C.accent}; margin-right: 10px; }
    .body { flex: 1 1 auto; display: flex; flex-direction: column; justify-content: center; min-height: 0; padding-right: 40px; }
    /* text-wrap: balance = 2 行になるタイトル / 説明で最終行に 1 文字だけ残さない */
    #title { font-size: 84px; font-weight: 800; line-height: 1.22; letter-spacing: -0.015em;
      max-height: 210px; overflow: hidden; text-wrap: balance; }
    .desc { margin-top: 26px; font-size: 27px; line-height: 1.6; color: ${C.inkMuted}; text-wrap: balance;
      display: -webkit-box; -webkit-line-clamp: 2; -webkit-box-orient: vertical; overflow: hidden; }
    .foot { display: flex; align-items: center; justify-content: space-between; padding-top: 26px;
      border-top: 1px solid rgba(237,231,240,0.12); }
    .accent-rule { position: absolute; left: 0; top: 0; width: 100%; height: 6px;
      background: linear-gradient(90deg, ${C.accent} 0%, ${C.accentStrong} 45%, ${C.green} 100%); }
  </style><body><div class="accent-rule"></div><div class="frame">
    <div class="head">
      <img class="mascot" id="logo" src="${mascotDataUri}" width="62" alt="">
      <div class="wordmark">tako</div>
      <div class="sep"></div>
      <div class="section"><span>—</span>${esc(section)}</div>
    </div>
    <div class="body">
      <div id="title">${esc(title)}</div>
      ${description ? `<div class="desc">${esc(description)}</div>` : ''}
    </div>
    <div class="foot"><div class="site mono">tako-docs.pages.dev</div><div class="site">ドキュメント</div></div>
  </div>${FIT_SCRIPT}</body></html>`;
}

/**
 * 描画後にやること:
 *  1. 長いタイトルを枠に収まるまで縮める
 *  2. ロゴの実測サイズを DOM へ書き出す（生成側が縦横比を検算する）
 */
const FIT_SCRIPT = `<script>
  (async () => {
    try { await document.fonts.ready; } catch (e) {}
    const t = document.getElementById('title');
    if (t) {
      let size = parseFloat(getComputedStyle(t).fontSize);
      while (t.scrollHeight > t.clientHeight && size > 40) {
        size -= 2;
        t.style.fontSize = size + 'px';
      }
    }
    const logo = document.getElementById('logo');
    const r = logo.getBoundingClientRect();
    document.documentElement.dataset.logoW = r.width.toFixed(4);
    document.documentElement.dataset.logoH = r.height.toFixed(4);
    document.documentElement.dataset.ready = '1';
  })();
</script>`;

/* ── 生成 ── */
const pages = walk(CONTENT).map((file) => {
  const rel = relative(CONTENT, file);
  const { slug, pathname } = routeOf(rel);
  const fm = frontmatter(file);
  return {
    file: rel,
    slug,
    pathname,
    key: imageKey(slug),
    title: fm.title,
    description: fm.description || '',
    section: slug === '' ? null : sectionOf(slug),
  };
});

mkdirSync(OUT_DIR, { recursive: true });
mkdirSync(dirname(MANIFEST), { recursive: true });
const work = mkdtempSync(join(tmpdir(), 'tako-og-'));

const only = process.argv.slice(2).find((a) => a.startsWith('--only='))?.slice(7);
const targets = only ? pages.filter((p) => p.key.includes(only)) : pages;
if (only && targets.length === 0) throw new Error(`--only=${only} に一致するページが無い`);

const report = [];
for (const page of targets) {
  const html = page.section === null ? heroHtml() : pageHtml(page);
  const htmlPath = join(work, `${page.key}.html`);
  const rawPath = join(work, `${page.key}.png`);
  writeFileSync(htmlPath, html);

  // --dump-dom と --screenshot を同時に取る = 「撮った絵」と「実測比」が同じ描画に由来する
  const dom = execFileSync(
    CHROME,
    [
      '--headless=new', '--disable-gpu', '--hide-scrollbars', '--force-device-scale-factor=1',
      `--window-size=${WIDTH},${HEIGHT}`, '--virtual-time-budget=6000',
      `--screenshot=${rawPath}`, '--dump-dom', `file://${htmlPath}`,
    ],
    { encoding: 'utf8', stdio: ['ignore', 'pipe', 'ignore'], maxBuffer: 64 * 1024 * 1024 }
  );

  if (!/data-ready="1"/.test(dom)) throw new Error(`${page.key}: 描画が完了しなかった`);
  const w = Number(dom.match(/data-logo-w="([\d.]+)"/)?.[1]);
  const h = Number(dom.match(/data-logo-h="([\d.]+)"/)?.[1]);
  if (!w || !h) throw new Error(`${page.key}: ロゴの実測値が取れなかった`);
  const ratio = w / h;
  const drift = Math.abs(ratio - LOGO_RATIO) / LOGO_RATIO;
  if (drift > LOGO_RATIO_TOLERANCE) {
    throw new Error(
      `${page.key}: ロゴの縦横比が崩れている（実測 ${ratio.toFixed(4)} / 正 ${LOGO_RATIO.toFixed(4)} / ずれ ${(drift * 100).toFixed(2)}%）`
    );
  }

  // 256 色 + フルディザのパレット PNG（実測 130KB -> 69KB）。
  // `quality` を渡すと色数が削られて淡いグローに量子化の縞（等高線）が出るので指定しない
  const outPath = join(OUT_DIR, `${page.key}.png`);
  await sharp(rawPath).png({ palette: true, colors: 256, dither: 1.0, effort: 10 }).toFile(outPath);
  const bytes = statSync(outPath).size;

  report.push({ ...page, logoW: w, logoH: h, ratio, bytes, out: relative(DOCS, outPath) });
}

rmSync(work, { recursive: true, force: true });

if (only) {
  console.log(`--only=${only}: ${targets.length} 枚だけ焼き直した（manifest は更新しない）`);
  for (const p of report) console.log(`  ${p.pathname} ${Math.round(p.bytes / 1024)}KB logo ${p.ratio.toFixed(4)}`);
  process.exit(0);
}

const manifest = {
  generatedBy: 'docs/scripts/generate-og.mjs',
  width: WIDTH,
  height: HEIGHT,
  pages: Object.fromEntries(report.map((p) => [p.pathname, `/og/${p.key}.png`])),
};
writeFileSync(MANIFEST, `${JSON.stringify(manifest, null, 2)}\n`);

const worst = report.reduce((a, b) => (b.bytes > a.bytes ? b : a));
console.log(`ロゴの正しい縦横比: ${LOGO_RATIO.toFixed(4)}（viewBox ${vbW}x${vbH}）`);
for (const p of report) {
  console.log(
    `  ${p.pathname.padEnd(34)} ${String(Math.round(p.bytes / 1024)).padStart(4)}KB  ` +
      `logo ${p.logoW.toFixed(1)}x${p.logoH.toFixed(1)} = ${p.ratio.toFixed(4)}  ${p.out}`
  );
}
console.log(`\n${report.length} 枚生成。最大 ${Math.round(worst.bytes / 1024)}KB（${worst.pathname}）`);
console.log(`manifest: ${relative(DOCS, MANIFEST)}`);
