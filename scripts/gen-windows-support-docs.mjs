#!/usr/bin/env node
// Windows 対応状況ページ（docs）を対応マトリクスから生成する（Issue #591）。
//
// **なぜ生成物なのか**: 機能は 140 件あり、手で書いた表は必ず実装から遅れる。
// マトリクス（crates/tako-core/src/platform/support.rs）は system prompt へも
// 流れる正本（#516）なので、docs をそこから作れば「宣言・prompt・docs」が
// 常に同じ 1 つの事実を指す。
//
//   node scripts/gen-windows-support-docs.mjs           # 生成（上書き）
//   node scripts/gen-windows-support-docs.mjs --check    # 同期検査（CI 用）
//
// カテゴリ未分類の機能があると**失敗する**。機能を追加したらここへ 1 行足すこと
// （分類漏れを検出するための仕掛けで、放置すると表から機能が消える）。

import { execFileSync } from 'node:child_process';
import { readFileSync, writeFileSync, existsSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const REPO = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const OUT = join(REPO, 'docs/src/content/docs/windows-support.md');

// 機能キー → 利用者向けカテゴリ。**順番がページの節順**になる
const CATEGORIES = [
  ['ターミナルの基本', [
    'tako_split_pane', 'tako_close_pane', 'tako_focus_pane', 'tako_resize_pane',
    'tako_equalize_layout', 'tako_move_pane_to_tab', 'tako_scroll_pane',
    'tako_send_input', 'tako_read_pane', 'tako_list_panes', 'tako_set_title',
    'tako_create_tab', 'tako_select_tab', 'tako_rename_tab', 'tako_reorder_tab',
    'tako_collapse_tab', 'tako_pin_tab_title', 'tako_confirm_close',
    'tako_auto_rename', 'tako_autosuggest', 'tako_window', 'tako_menu',
    'tako_background_pane', 'tako_background_list', 'tako_background_kill',
    'tako_foreground_pane', 'tako_show_command', 'tako_run', 'tako_run_resolve',
    'tako_run_defaults', 'tako_run_interactive', 'tako_run_interactive_status',
  ]],
  ['表示とプレビュー', [
    'tako_open_file', 'tako_open_dir', 'tako_preview_view', 'tako_preview_outline',
    'tako_preview_link_list', 'tako_preview_follow_link', 'tako_preview_copy_code',
    'tako_preview_reload', 'tako_preview_cache', 'tako_preview_edit',
    'tako_preview_apply', 'tako_preview_save', 'tako_preview_undo',
    'tako_preview_redo', 'tako_preview_search', 'tako_preview_replace',
    'tako_preview_autosave', 'tako_preview_changelog', 'tako_pin_preview',
    'tako_video_playback', 'tako_video_seek', 'tako_video_volume', 'tako_web',
    'tako_theme', 'tako_lang', 'tako_ui_mode', 'tako_chat_copy', 'tako_panel',
    'tako_tree_folder', 'tako_welcome', 'tako_recent',
  ]],
  ['AI 連携（オーケストレーション）', [
    'tako_orchestrator_spawn', 'tako_orchestrator_self', 'tako_orchestrator_handoff',
    'tako_orchestrator_handoffs', 'tako_orchestrator_profiles',
    'tako_orchestrator_projects', 'tako_orchestrator_accounts',
    'tako_orchestrator_layout', 'tako_orchestrator_workers',
    'tako_orchestrator_worker_status', 'tako_orchestrator_respond',
    'tako_orchestrator_report', 'tako_orchestrator_run',
    'tako_orchestrator_run_status', 'tako_orchestrator_run_result',
    'tako_orchestrator_supervisor', 'tako_orchestrator_ledger',
    'tako_limit_resume', 'tako_limit_service', 'tako_sessions',
    'tako_task_gate', 'tako_task_gate_check', 'tako_task_gate_show',
    'tako_task_checkpoint', 'tako_task_list', 'tako_task_resume',
  ]],
  ['git 連携', [
    'tako_git_log', 'tako_git_diff', 'tako_git_show', 'tako_git_stage',
    'tako_git_unstage', 'tako_git_commit', 'tako_git_conflicts',
    'tako_git_resolve_agent', 'tako_git_checkout', 'tako_git_branch_create',
    'tako_git_merge', 'tako_git_merge_abort', 'tako_git_pull', 'tako_git_push',
  ]],
  ['永続化とセッション', [
    'tako_persist', 'tako_logs', 'tako_tmux_list', 'tako_tmux_kill',
    'tako_tmux_open', 'tako_tmux_select_window', 'tako_tmux_cleanup',
    'tako_tmux_resize',
  ]],
  ['OS 連携', [
    'tako_file_op', 'tako_sleep_guard', 'tako_port_detect', 'tako_fda',
    'tako_shell_integration', 'tako_stale_binary', 'tako_check_health',
    'tako_telemetry',
  ]],
  ['セットアップと設定', [
    'tako_setup', 'tako_setup_bootstrap', 'tako_setup_changes', 'tako_setup_mcp',
    'tako_settings', 'tako_migrate', 'tako_agents_sync_rules',
    'tako_config_share', 'tako_platform',
  ]],
  ['リモートアクセス', [
    'tako_remote_start', 'tako_remote_stop', 'tako_remote_status',
    'tako_remote_setup', 'tako_remote_devices', 'tako_remote_agents',
    'tako_remote_messages', 'tako_remote_scrollback', 'tako_open_remote',
    'tako_remote_folder', 'tako_ssh_hosts',
  ]],
  ['アップデート', ['tako_update']],
];

const STATUS_LABEL = {
  supported: '対応',
  degraded: '一部対応',
  pending: '未対応 / 未実測',
  unsupported: '対象外',
};

const EVIDENCE_LABEL = {
  'self-test': '実機セルフテスト',
  'unit-test': '実機テスト',
  measured: '実機実測',
  unverified: '未実測',
};

function takoBin() {
  for (const p of ['target/debug/tako', 'target/release/tako']) {
    if (existsSync(join(REPO, p))) return join(REPO, p);
  }
  throw new Error('tako CLI が見つかりません。`cargo build -p tako-cli` を先に実行してください');
}

function matrix(platform) {
  const raw = execFileSync(takoBin(), ['platform', '--platform', platform, '--json'], {
    encoding: 'utf8',
    maxBuffer: 32 * 1024 * 1024,
  });
  return JSON.parse(raw);
}

function escapeCell(s) {
  return String(s ?? '').replace(/\|/g, '\\|').replace(/\n+/g, ' ');
}

function render() {
  const win = matrix('windows');
  const byKey = new Map(win.features.map((f) => [f.key, f]));

  // 分類漏れの検出（両方向）
  const classified = new Set(CATEGORIES.flatMap(([, keys]) => keys));
  const missing = win.features.map((f) => f.key).filter((k) => !classified.has(k));
  const stale = [...classified].filter((k) => !byKey.has(k));
  if (missing.length) {
    throw new Error(
      `カテゴリ未分類の機能があります（${missing.length} 件）: ${missing.join(', ')}\n` +
        'scripts/gen-windows-support-docs.mjs の CATEGORIES へ追加してください',
    );
  }
  if (stale.length) {
    throw new Error(
      `マトリクスに無いキーが CATEGORIES に残っています: ${stale.join(', ')}`,
    );
  }

  const c = win.counts;
  const total = win.total;
  const pct = (n) => Math.round((n / total) * 100);

  const out = [];
  out.push('---');
  out.push('title: Windows 対応状況');
  out.push('description: どの機能が Windows で使えるか。対応マトリクスから自動生成しています');
  out.push('---');
  out.push('');
  out.push('tako は macOS で先行開発し、安定した差分を Windows へ反映しています。');
  out.push('このページは **tako 本体が持っている対応マトリクスから生成**しているので、');
  out.push('実装とずれません。手元の環境で最新を引くには次を実行してください。');
  out.push('');
  out.push('```sh');
  out.push('tako platform                      # この環境の対応状況');
  out.push('tako platform --status pending      # まだ使えないものだけ');
  out.push('```');
  out.push('');
  out.push('## 全体');
  out.push('');
  out.push('| 状態 | 件数 | 意味 |');
  out.push('| --- | --- | --- |');
  out.push(`| 対応 | ${c.supported} / ${total}（${pct(c.supported)}%） | macOS と同じように使えます |`);
  out.push(`| 一部対応 | ${c.degraded} | 使えますが機能が落ちます。落ち方は各表の「差分」列 |`);
  out.push(`| 未対応 / 未実測 | ${c.pending} | 未実装のもの、または実装はあるが Windows 実機で確かめていないもの |`);
  out.push(`| 対象外 | ${c.unsupported} | Windows にその概念が無い、または OS が同等機能を標準で持つ |`);
  out.push('');
  out.push('### 「未実測」について');
  out.push('');
  out.push('tako は **実機で確かめたものだけを「対応」と書きます**。実装がプラットフォーム');
  out.push('共通で動く見込みがあっても、Windows 実機で 1 度も実行していないものは');
  out.push('「未対応 / 未実測」に置いています。過大に申告すると、この宣言を読んで動く');
  out.push('AI エージェント（tako は対応状況を system prompt へ渡します）が');
  out.push('「使えるはず」と信じて失敗し続けるためです。');
  out.push('');
  out.push('各表の「根拠」列が判定の裏づけです。');
  out.push('');
  out.push('| 根拠 | 意味 |');
  out.push('| --- | --- |');
  out.push('| 実機セルフテスト | Windows 実機の GUI セルフテスト（通しで失敗 0 件）が実際に通した項目 |');
  out.push('| 実機テスト | Windows 実機の `cargo test` で緑のテスト |');
  out.push('| 実機実測 | Windows 実機で操作を実行して結果を記録したもの |');
  out.push('| 未実測 | まだ実機で動かしていないもの |');
  out.push('');

  for (const [label, keys] of CATEGORIES) {
    const rows = keys.map((k) => byKey.get(k));
    const counts = rows.reduce((acc, f) => {
      acc[f.status] = (acc[f.status] ?? 0) + 1;
      return acc;
    }, {});
    const summary = ['supported', 'degraded', 'pending', 'unsupported']
      .filter((s) => counts[s])
      .map((s) => `${STATUS_LABEL[s]} ${counts[s]}`)
      .join('・');
    out.push(`## ${label}`);
    out.push('');
    out.push(`${summary}`);
    out.push('');
    out.push('| 機能 | 状態 | 差分 | 根拠 |');
    out.push('| --- | --- | --- | --- |');
    for (const f of rows) {
      const diff = f.status === 'supported' ? '—' : escapeCell(f.note);
      const ev = EVIDENCE_LABEL[f.evidence] ?? f.evidence;
      const detail = f.evidence_detail ? `${ev}: ${escapeCell(f.evidence_detail)}` : ev;
      out.push(`| \`${f.key}\` | ${STATUS_LABEL[f.status]} | ${diff} | ${detail} |`);
    }
    out.push('');
  }

  out.push('## この表の作り方');
  out.push('');
  out.push('正本は `crates/tako-core/src/platform/support.rs` の対応マトリクスです。');
  out.push('判定を変えるときは根拠（実機セルフテストの項目・実機で緑のテスト名・実測の記録）を');
  out.push('同時に書く必要があり、書かずに「対応」へ倒すとテストが落ちます。');
  out.push('');
  out.push('このページの再生成と同期検査は次のとおりです。');
  out.push('');
  out.push('```sh');
  out.push('cargo build -p tako-cli');
  out.push('node scripts/gen-windows-support-docs.mjs          # 再生成');
  out.push('node scripts/gen-windows-support-docs.mjs --check   # 同期検査');
  out.push('```');
  out.push('');
  return out.join('\n');
}

const body = render();
if (process.argv.includes('--check')) {
  const current = existsSync(OUT) ? readFileSync(OUT, 'utf8') : '';
  if (current !== body) {
    console.error(
      'docs/src/content/docs/windows-support.md が対応マトリクスと同期していません。\n' +
        '`node scripts/gen-windows-support-docs.mjs` で再生成してコミットしてください。',
    );
    process.exit(1);
  }
  console.log('windows-support.md は対応マトリクスと同期しています');
} else {
  writeFileSync(OUT, body);
  console.log(`生成しました: ${OUT}`);
}
