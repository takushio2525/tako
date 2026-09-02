#!/usr/bin/env node
// agent 対応状況ページ（docs）を能力マトリクスから生成する（Issue #982）。
//
// **なぜ生成物なのか**: OS 軸の windows-support.md（#591）と同じ理由。
// マトリクス（crates/tako-core/src/agent_support.rs）は診断・エラーメッセージ・
// system prompt が引く正本なので、docs をそこから作れば
// 「宣言・診断・docs」が常に同じ 1 つの事実を指す。
//
//   node scripts/gen-agent-support-docs.mjs           # 生成（上書き）
//   node scripts/gen-agent-support-docs.mjs --check    # 同期検査（CI 用）
//
// カテゴリ未分類の能力があると**失敗する**。マトリクスへ行を足したらここへも
// 1 行足すこと（分類漏れを検出するための仕掛けで、放置すると表から消える）。
//
// **ページの中身（縮退の説明・系統別セットアップガイド）を仕上げるのは #992**。
// ここが作るのは生成の骨格と `--check` の配管。

import { execFileSync } from 'node:child_process';
import { readFileSync, writeFileSync, existsSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const REPO = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const OUT = join(REPO, 'docs/src/content/docs/agent-support.md');

// 能力キー → 利用者向けカテゴリ。**順番がページの節順**になる
const CATEGORIES = [
  ['セットアップ', [
    'setup_detect', 'setup_auth_check', 'setup_auth_launch', 'setup_plan_detect',
    'setup_cli_install', 'setup_profile_recommend', 'setup_rules_sync',
    'setup_mcp_register', 'setup_model_picker',
  ]],
  ['オーケストレーター（master / solo）', [
    'master_launch', 'solo_launch', 'master_system_prompt', 'master_mcp',
    'master_handoff', 'master_auto_handoff', 'master_ctx_percent',
  ]],
  ['worker の起動', [
    'worker_spawn', 'agent_select_at_spawn', 'worker_trust',
    'worker_bypass_preaccept', 'effort_control', 'account_switch',
  ]],
  ['worker の監視', [
    'worker_status_detect', 'worker_status_structured',
    'worker_prompt_undelivered', 'worker_death_resume',
  ]],
  ['worker への指示と応答', [
    'worker_prompt_delivery', 'worker_delivery_peer',
    'worker_permission_dialog', 'worker_choice_dialog', 'worker_cli_control',
    'worker_mcp',
  ]],
  ['報告と会話ログ', [
    'worker_report_scrollback', 'worker_report_transcript',
    'sessions_catalog', 'sessions_resume',
    'session_restart_harness', 'session_restart_handoff',
  ]],
  ['利用制限', [
    'worker_limit_detect', 'worker_limit_autoresume', 'worker_limit_metrics',
    'limit_service_switch',
  ]],
  ['その他', ['git_resolve_agent']],
];

const STATUS_LABEL = {
  supported: '対応',
  degraded: '一部対応',
  pending: '未対応',
  unsupported: '対象外',
};

const EVIDENCE_LABEL = {
  source: 'コード本文',
  'self-test': '実機セルフテスト',
  'unit-test': 'テスト',
  measured: '実測',
  'by-design': '上流の仕様',
  unverified: '未確認',
};

function takoBin() {
  for (const p of ['target/debug/tako', 'target/release/tako']) {
    if (existsSync(join(REPO, p))) return join(REPO, p);
  }
  throw new Error('tako CLI が見つかりません。`cargo build -p tako-cli` を先に実行してください');
}

function matrix() {
  const raw = execFileSync(takoBin(), ['agent-support', '--json'], {
    encoding: 'utf8',
    maxBuffer: 32 * 1024 * 1024,
  });
  return JSON.parse(raw);
}

function escapeCell(s) {
  return String(s ?? '').replace(/\|/g, '\\|').replace(/\n+/g, ' ');
}

function render() {
  const m = matrix();
  const byKey = new Map(m.features.map((f) => [f.key, f]));
  const agents = m.agents;

  // 分類漏れの検出（両方向）
  const classified = new Set(CATEGORIES.flatMap(([, keys]) => keys));
  const missing = m.features.map((f) => f.key).filter((k) => !classified.has(k));
  const stale = [...classified].filter((k) => !byKey.has(k));
  if (missing.length) {
    throw new Error(
      `カテゴリ未分類の能力があります（${missing.length} 件）: ${missing.join(', ')}\n` +
        'scripts/gen-agent-support-docs.mjs の CATEGORIES へ追加してください',
    );
  }
  if (stale.length) {
    throw new Error(`マトリクスに無いキーが CATEGORIES に残っています: ${stale.join(', ')}`);
  }

  const total = m.matrix_total;
  const out = [];
  out.push('---');
  out.push('title: エージェント別の対応状況');
  out.push('description: claude / codex / agy / ローカル LLM でどこまで同じことができるか。能力マトリクスから自動生成しています');
  out.push('---');
  out.push('');
  out.push('tako は **Claude Code を基準に実装してきました**。ほかのエージェント CLI でも');
  out.push('worker を立てて使えますが、機能によっては落ちるか、まだ使えません。');
  out.push('このページは **tako 本体が持っている能力マトリクスから生成**しているので、');
  out.push('実装とずれません。手元で最新を引くには次を実行してください。');
  out.push('');
  out.push('```sh');
  out.push('tako agent-support                        # 全系統の表');
  out.push('tako agent-support --agent codex          # codex の理由つき一覧');
  out.push('tako agent-support --agent agy --status pending   # まだ使えないものだけ');
  out.push('```');
  out.push('');
  out.push('## 全体');
  out.push('');
  out.push(`能力 ${total} 件の内訳です。`);
  out.push('');
  out.push('| エージェント | 対応 | 一部対応 | 未対応 | 対象外 |');
  out.push('| --- | --- | --- | --- | --- |');
  for (const a of agents) {
    const c = m.counts[a.key];
    const base = a.baseline ? '（基準）' : '';
    out.push(
      `| ${a.label}${base} | ${c.supported} / ${total} | ${c.degraded} | ${c.pending} | ${c.unsupported} |`,
    );
  }
  out.push('');
  out.push('### 状態の意味');
  out.push('');
  out.push('| 状態 | 意味 |');
  out.push('| --- | --- |');
  out.push('| 対応 | Claude Code と同じように使えます |');
  out.push('| 一部対応 | 使えますが機能が落ちます。落ち方は各表の「差分」列 |');
  out.push('| 未対応 | tako 側の実装が無い、または**まだ調べていない**もの。追跡先の Issue 番号が付きます |');
  out.push('| 対象外 | そのエージェント CLI にその手段がそもそも無いもの |');
  out.push('');
  out.push('**「未対応」と「対象外」を混ぜていません**。調べていないものは「対象外」ではなく');
  out.push('「未対応」に置いています。まだ道があるかもしれないものを「無理」と書くと、');
  out.push('この宣言を読んで動く AI エージェントがその道を永久に避けてしまうためです。');
  out.push('');
  out.push('各表の「根拠」列が判定の裏づけです。');
  out.push('');
  out.push('| 根拠 | 意味 |');
  out.push('| --- | --- |');
  out.push('| コード本文 | tako 自身の実装がそうなっていること（配線の有無）を引用したもの |');
  out.push('| 上流の仕様 | エージェント CLI 側の仕様・設計判断で、実測する対象がそもそも無いもの |');
  out.push('| 実測 | 実際に動かして結果を記録したもの |');
  out.push('| テスト | 緑のテストが担保しているもの |');
  out.push('| 未確認 | まだ確かめていないもの |');
  out.push('');
  out.push('## ローカル LLM について');
  out.push('');
  out.push('現時点では **1 つも成立していません**（表の値はほぼ「未対応」です）。');
  out.push('第一歩は codex CLI を Ollama へ向ける経路で、TUI 前提を外した一級対応は');
  out.push('その次の段階です。');
  out.push('');

  for (const [label, keys] of CATEGORIES) {
    out.push(`## ${label}`);
    out.push('');
    const head = agents.map((a) => a.label).join(' | ');
    out.push(`| 能力 | ${head} | 根拠 |`);
    out.push(`| --- | ${agents.map(() => '---').join(' | ')} | --- |`);
    for (const k of keys) {
      const f = byKey.get(k);
      const cells = agents.map((a) => {
        const cell = f.agents[a.key];
        const label = STATUS_LABEL[cell.status] ?? cell.status;
        if (cell.status === 'supported') return label;
        // 表示言語に依存しない ja を明示的に使う。`note` は実行環境の言語で
        // 変わるので、これを使うと生成した人のロケールで中身が変わってしまう（#591）
        const issue = cell.issue ? ` [#${cell.issue}](https://github.com/takushio2525/tako/issues/${cell.issue})` : '';
        return `${label}${issue}<br />${escapeCell(cell.note_ja)}`;
      });
      const ev = EVIDENCE_LABEL[f.evidence] ?? f.evidence;
      const detail = f.evidence_detail ? `${ev}: ${escapeCell(f.evidence_detail)}` : ev;
      out.push(`| **${escapeCell(f.summary_ja)}**<br />\`${f.key}\` | ${cells.join(' | ')} | ${detail} |`);
    }
    out.push('');
  }

  out.push('## この表の作り方');
  out.push('');
  out.push('正本は `crates/tako-core/src/agent_support.rs` の能力マトリクスです。');
  out.push('Claude Code 以外について「使える」「落ちる」「使えない」と書くときは根拠');
  out.push('（コード本文の引用・上流の仕様・実測の記録）を同時に書く必要があり、');
  out.push('書かずに倒すとテストが落ちます。');
  out.push('');
  out.push('このページの再生成と同期検査は次のとおりです。');
  out.push('');
  out.push('```sh');
  out.push('cargo build -p tako-cli');
  out.push('node scripts/gen-agent-support-docs.mjs          # 再生成');
  out.push('node scripts/gen-agent-support-docs.mjs --check   # 同期検査');
  out.push('```');
  out.push('');
  return out.join('\n');
}

const body = render();
if (process.argv.includes('--check')) {
  const current = existsSync(OUT) ? readFileSync(OUT, 'utf8') : '';
  if (current !== body) {
    console.error(
      'docs/src/content/docs/agent-support.md が能力マトリクスと同期していません。\n' +
        '`node scripts/gen-agent-support-docs.mjs` で再生成してコミットしてください。',
    );
    process.exit(1);
  }
  console.log('agent-support.md は能力マトリクスと同期しています');
} else {
  writeFileSync(OUT, body);
  console.log(`生成しました: ${OUT}`);
}
