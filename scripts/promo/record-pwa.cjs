#!/usr/bin/env node
// tako:run: NODE_PATH=web/tako-remote/node_modules node scripts/promo/record-pwa.cjs
// 解説動画（#1081）7 章「スマホから」の素材: tako remote の PWA（web/tako-remote）を
// iPhone のビューポートで開き、Playwright の画面録画で mp4 素材にする。
//
// daemon は使わず API を page.route でモックする（e2e/screenshots.spec.js と同じ作法）。
// 理由: 実 daemon は tailscale serve を張る = 本番の Tailscale / remote 状態に触れるため、
// 収録では**画面（PWA の UI そのもの）**だけを本物にし、データはデモ用にする。
// 動画のテロップに「画面はデモ用データ」と明記する（正確性の要件）。
//
// 前提: web/tako-remote の dev サーバーが TAKO_PWA_PORT（既定 5199）で動いていること
//   cd web/tako-remote && npx vite --port 5199 --strictPort
// 使い方:
//   NODE_PATH=web/tako-remote/node_modules node scripts/promo/record-pwa.cjs [出力ディレクトリ]
// 出力: <out>/scenes/pwa-raw.mp4（1920x1080。スマホ画面を中央に置いた合成）+ pwa-beats.tsv
// 撮り方: 連番スクリーンショット（4 fps・3x）→ ffmpeg。recordVideo は使わない（下記）
const { chromium, devices } = require('@playwright/test');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { execFileSync } = require('node:child_process');

const OUT = process.argv[2] || process.env.TAKO_PROMO_OUT || path.join(os.homedir(), 'Desktop', 'tako-promo');
const PORT = process.env.TAKO_PWA_PORT || '5199';
const BASE = `http://localhost:${PORT}`;
// PWA は Cargo.toml の version を自分の版として持ち /api/me の version と比べる。
// ずれると「表示が古い」帯が出るので、リポジトリの版数を読んで同じ値を返す
const cargo = fs.readFileSync(path.join(__dirname, '..', '..', 'Cargo.toml'), 'utf8');
const VERSION = (cargo.match(/^version\s*=\s*"([^"]+)"/m) || [])[1] || '0.0.0';

const CWD = '/private/tmp/tako-demo/awesome-app';
const ME = { registered: true, device_id: 'demo-iphone', name: 'iPhone', role: 'interact',
  login: 'demo', host: 'demo-mac', version: VERSION, app_connected: true };
const PANES = { api_version: 2, panes: [
  { id: 1, title: 'master', role: 'master', agent_type: 'claude', cwd: CWD, state: 'running',
    surface: 'foreground', position: '1/4', tab_id: 1, tab_title: 'awesome-app', cols: 120, rows: 40,
    focused: true, session_id: 'demo-master', model: 'sonnet', tmux_target: 'tako-m:0.0', activity: 'idle',
    preview: ['⏺ worker を 3 体立てました。api / ui / docs です。', '  完了報告が来たら検収して結果をまとめます。', '', '✻ Cogitated for 42s'] },
  { id: 2, title: 'api', role: 'orchestrator-worker-claude', agent_type: 'claude', cwd: CWD, state: 'running',
    surface: 'foreground', position: '2/4', tab_id: 1, tab_title: 'awesome-app', cols: 120, rows: 40,
    focused: false, session_id: 'demo-w1', model: 'haiku', tmux_target: 'tako-w1:0.0', activity: 'busy',
    preview: ['⏺ src/api.py を読んでいます', '', '⏺ Searching for 3 patterns, reading 2 files…', '', '✽ Misting… (1m 12s · ↓ 3.1k tokens)'] },
  { id: 3, title: 'ui', role: 'orchestrator-worker-claude', agent_type: 'claude', cwd: CWD, state: 'running',
    surface: 'foreground', position: '3/4', tab_id: 1, tab_title: 'awesome-app', cols: 120, rows: 40,
    focused: false, session_id: 'demo-w2', model: 'haiku', tmux_target: 'tako-w2:0.0', activity: 'idle',
    preview: ['⏺ 4 files changed, tests green', '', '✻ Baked for 58s'] },
  { id: 4, title: 'docs', role: 'orchestrator-worker-claude', agent_type: 'claude', cwd: CWD, state: 'running',
    surface: 'foreground', position: '4/4', tab_id: 1, tab_title: 'awesome-app', cols: 120, rows: 40,
    focused: false, session_id: 'demo-w3', model: 'haiku', tmux_target: 'tako-w3:0.0', activity: 'permission',
    permission_dialog: { command: 'Bash command: rm -rf build', options: ['Yes', 'Yes, and don\'t ask again for rm commands', 'No'], highlighted: 0 },
    preview: ['⏺ ビルド成果物を消してから再ビルドします。', '', '╭──────────────────────────╮', '│ Bash command             │', '│ rm -rf build             │', '│ Do you want to proceed?  │', '│ ❯ 1. Yes                 │', '│   2. No                  │'] },
]};
const MESSAGES = {
  'demo-w3': { session_id: 'demo-w3', messages: [
    { role: 'user', text: 'docs/ のビルドを通して。古い build/ を消してからやり直してよい', timestamp: '2026-09-03T10:41:00Z' },
    { role: 'assistant', text: 'docs のビルド設定を確認します。', tools: [ { name: 'Read', summary: 'docs/config.toml' }, { name: 'Bash', summary: 'ls build/' } ], timestamp: '2026-09-03T10:41:20Z' },
    { role: 'assistant', text: '古い成果物が残っているので、`build/` を消してから再ビルドします。実行前に確認をお願いします。', timestamp: '2026-09-03T10:41:40Z' },
  ] },
  'demo-w1': { session_id: 'demo-w1', messages: [
    { role: 'user', text: 'bash scripts/task.sh api を実行して、出力の最終行を報告して', timestamp: '2026-09-03T10:40:00Z' },
    { role: 'assistant', text: 'タスクを実行します。', tools: [ { name: 'Bash', summary: 'bash scripts/task.sh api' } ], timestamp: '2026-09-03T10:40:10Z' },
    { role: 'assistant', text: '最終行: `done api: 4 files changed, tests green`', timestamp: '2026-09-03T10:40:30Z' },
  ] },
};
const SCREEN = { lines: ['$ bash scripts/task.sh api', 'task api', '  * reading source files', '  * applying changes', '  * running tests', '  * all checks passed', 'done api: 4 files changed, tests green'], cursor: { x: 0, y: 7 }, size: { cols: 120, rows: 40 } };

(async () => {
  fs.mkdirSync(path.join(OUT, 'scenes'), { recursive: true });
  // Playwright の recordVideo は iPhone エミュレーション（deviceScaleFactor 3）と組むと
  // ページが左上 1/4 に描かれ残りが灰色になる（実測）。連番スクリーンショット
  // （3x = 1170x2532）を 4 fps で撮って ffmpeg で繋ぐ方が確実で、しかも鮮明
  const vdir = fs.mkdtempSync(path.join(os.tmpdir(), 'tako-promo-pwa-'));
  const browser = await chromium.launch();
  const iphone = devices['iPhone 14'];
  const ctx = await browser.newContext({ ...iphone, locale: 'ja-JP' });
  const page = await ctx.newPage();
  const SHOT_MS = 250;
  let shotIdx = 0;
  let shooting = true;
  const shooter = (async () => {
    while (shooting) {
      const t = Date.now();
      try {
        await page.screenshot({ path: path.join(vdir, `s${String(shotIdx++).padStart(5, '0')}.png`) });
      } catch (_) { /* ページ遷移中は撮れないことがある。次の周で撮る */ }
      const rest = SHOT_MS - (Date.now() - t);
      if (rest > 0) await new Promise((r) => setTimeout(r, rest));
    }
  })();
  const json = (b) => (r) => r.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify(b) });
  await page.route('**/api/me', json(ME));
  await page.route('**/api/v2/panes', json(PANES));
  await page.route('**/api/health', json({ status: 'ok', version: VERSION }));
  await page.route('**/api/agents', json({ agents: [] }));
  await page.route('**/api/panes/*/screen*', json(SCREEN));
  await page.route('**/api/panes/*/scrollback*', json({ lines: SCREEN.lines }));
  await page.route('**/api/panes/*/respond', json({ ok: true }));
  await page.route('**/api/sessions/*/messages*', (r) => {
    const m = r.request().url().match(/sessions\/([^/]+)\/messages/);
    r.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify(MESSAGES[m && m[1]] || { messages: [] }) });
  });
  await page.route('**/ws?*', (r) => r.abort());
  await page.route('**/sw.js', (r) => r.fulfill({ status: 200, contentType: 'application/javascript', body: '' }));

  const beats = [];
  const t0 = Date.now();
  const beat = (n) => { beats.push([n, ((Date.now() - t0) / 1000).toFixed(2)]); };
  const wait = (ms) => page.waitForTimeout(ms);

  await page.goto(`${BASE}/#/`);
  await page.waitForSelector('.pane-card', { timeout: 15000 });
  beat('list');
  await wait(5000);
  await page.mouse.wheel(0, 500);
  await wait(3000);
  await page.mouse.wheel(0, -500);
  await wait(3000);
  // 承認待ちの worker（docs）を開く → チャット表示 + 承認カード
  await page.goto(`${BASE}/#/panes/4`);
  await page.waitForSelector('.chat-scroll', { timeout: 15000 });
  beat('chat');
  await wait(9000);
  // 完了した worker（ui）のチャット表示 → 一覧へ戻る
  // （term ビューは WebSocket の画面プッシュが前提で、モックでは読み込み中のまま = 撮らない）
  await page.goto(`${BASE}/#/panes/2`);
  await page.waitForSelector('.chat-scroll', { timeout: 15000 });
  beat('chat2');
  await wait(6000);
  await page.goto(`${BASE}/#/`);
  await page.waitForSelector('.pane-card', { timeout: 15000 });
  beat('back');
  await wait(4000);
  shooting = false;
  await shooter;
  const elapsed = (Date.now() - t0) / 1000;
  await ctx.close();
  await browser.close();

  const shots = fs.readdirSync(vdir).filter((f) => f.endsWith('.png')).length;
  if (!shots) throw new Error('スクリーンショットが 1 枚も無い: ' + vdir);
  const fps = (shots / elapsed).toFixed(3);   // 実測フレームレートで尺を実時間に合わせる
  const out = path.join(OUT, 'scenes', 'pwa-raw.mp4');
  // スマホ画面（縦長）を 1920x1080 の暗い背景の中央へ
  execFileSync('ffmpeg', ['-v', 'error', '-y', '-framerate', fps, '-i', path.join(vdir, 's%05d.png'),
    '-vf', 'scale=-2:1000,pad=1920:1080:(ow-iw)/2:(oh-ih)/2:color=0x0d1117,fps=30,format=yuv420p',
    '-c:v', 'libx264', '-preset', 'medium', '-crf', '18', out], { stdio: 'inherit' });
  console.log(`   ${shots} 枚 / ${elapsed.toFixed(1)}s = ${fps} fps`);
  fs.writeFileSync(path.join(OUT, 'scenes', 'pwa-beats.tsv'), beats.map((b) => b.join('\t')).join('\n') + '\n');
  fs.rmSync(vdir, { recursive: true, force: true });
  console.log('== pwa-raw.mp4:', out);
  console.log(beats.map((b) => `   beat ${b[0]} @ ${b[1]}s`).join('\n'));
})().catch((e) => { console.error(e); process.exit(1); });
