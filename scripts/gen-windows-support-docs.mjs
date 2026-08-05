#!/usr/bin/env node
// doc サイトの「Windows 対応状況」ページを対応マトリクスから生成する（#591）。
//
// **なぜ生成にするか**: 対応マトリクス（crates/tako-core/src/platform/support.rs）は
// 125 機能ある。ページを手書きすると必ずドリフトし、しかも縮退の理由文は
// master / setup の system prompt にも注入される（#516）ので、
// 「docs だけ古い」状態が AI への誤情報と直結する。正は support.rs 1 本に保つ。
//
//   node scripts/gen-windows-support-docs.mjs          # 生成（上書き）
//   node scripts/gen-windows-support-docs.mjs --check  # 差分があれば非ゼロで終了
//
// tako バイナリは --bin で指定できる。省略時は target/debug → target/release → PATH の順。

import { execFileSync } from 'node:child_process';
import { existsSync, readFileSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = dirname(dirname(fileURLToPath(import.meta.url)));
const OUT = join(ROOT, 'docs/src/content/docs/windows-support.md');

// ---------------------------------------------------------------------------
// カテゴリ定義。**MCP ツール名は利用者の語彙ではない**ので、ここで人間向けに畳む。
// 表示順がそのままページの節の順になる（使用頻度の高いものから並べる）。
// ---------------------------------------------------------------------------
const CATEGORIES = [
  {
    title: 'ターミナルの基本',
    intro: 'シェルの起動・入出力・スクロール・コピー & ペースト。',
    keys: [
      'tako_send_input', 'tako_send_keys', 'tako_read_pane', 'tako_scroll_pane', 'tako_list_panes',
      'tako_logs', 'tako_limit_service', 'tako_theme', 'tako_lang', 'tako_settings',
      'tako_check_health', 'tako_telemetry', 'tako_platform',
    ],
  },
  {
    title: 'タブ・ペイン・ウィンドウ',
    intro: '分割 / 移動 / リサイズ、たまり場（バックグラウンド退避）、複数ウィンドウ、メニューバー。',
    keys: [
      'tako_split_pane', 'tako_close_pane', 'tako_focus_pane', 'tako_resize_pane',
      'tako_equalize_layout', 'tako_create_tab', 'tako_select_tab', 'tako_rename_tab',
      'tako_reorder_tab', 'tako_move_pane_to_tab', 'tako_collapse_tab', 'tako_confirm_close',
      'tako_set_title', 'tako_auto_rename', 'tako_window', 'tako_menu', 'tako_panel',
      'tako_background_pane', 'tako_foreground_pane', 'tako_background_list',
      'tako_background_kill', 'tako_open_dir', 'tako_recent', 'tako_ssh_hosts',
      'tako_open_remote',
    ],
  },
  {
    title: 'オーケストレーション（tako master）',
    intro: 'worker の spawn・監視・報告・タスク管理。',
    keys: [
      'tako_orchestrator_spawn', 'tako_orchestrator_launch_status', 'tako_orchestrator_self',
      'tako_orchestrator_worker_status',
      'tako_orchestrator_workers', 'tako_orchestrator_report', 'tako_orchestrator_respond',
      'tako_orchestrator_dialog',
      'tako_orchestrator_handoff', 'tako_orchestrator_run', 'tako_orchestrator_run_status',
      'tako_orchestrator_run_result', 'tako_orchestrator_supervisor', 'tako_orchestrator_ledger',
      'tako_orchestrator_projects', 'tako_orchestrator_profiles', 'tako_orchestrator_accounts',
      'tako_orchestrator_layout', 'tako_task_checkpoint', 'tako_task_list', 'tako_task_resume',
      'tako_task_gate', 'tako_task_gate_check', 'tako_task_gate_show',
    ],
  },
  {
    title: 'セッション永続化',
    intro: 'tako を再起動したときにタブ・ペインと実行中プロセスをどこまで戻せるか。',
    keys: [
      'tako_persist', 'tako_sessions', 'tako_tmux_list', 'tako_tmux_open', 'tako_tmux_kill',
      'tako_tmux_cleanup', 'tako_tmux_resize', 'tako_tmux_select_window',
    ],
  },
  {
    title: 'git 連携',
    intro: '右パネルの git タブ（履歴 / diff / ステージング / ブランチ操作 / コンフリクト解消）。',
    keys: [
      'tako_git_log', 'tako_git_diff', 'tako_git_show', 'tako_git_stage', 'tako_git_unstage',
      'tako_git_commit', 'tako_git_push', 'tako_git_pull', 'tako_git_branch_create',
      'tako_git_checkout', 'tako_git_merge', 'tako_git_merge_abort', 'tako_git_conflicts',
      'tako_git_resolve_agent',
    ],
  },
  {
    title: 'ファイルプレビュー・Web ビュー',
    intro: 'コード / Markdown / 画像 / PDF / 動画のプレビューと、ネイティブ Web ビューペイン。',
    keys: [
      'tako_open_file', 'tako_preview_view', 'tako_preview_outline', 'tako_preview_reload',
      'tako_preview_cache', 'tako_preview_changelog', 'tako_preview_search', 'tako_preview_edit',
      'tako_preview_apply', 'tako_preview_replace', 'tako_preview_save', 'tako_preview_undo',
      'tako_preview_redo', 'tako_preview_autosave', 'tako_preview_link_list',
      'tako_preview_follow_link', 'tako_preview_copy_code', 'tako_pin_preview',
      'tako_video_playback', 'tako_video_seek', 'tako_video_volume', 'tako_web',
    ],
  },
  {
    title: 'コード実行（Code Runner）',
    intro: 'プレビューの再生ボタン・拡張子既定コマンド・対話コマンドの委譲。',
    keys: ['tako_run', 'tako_run_resolve', 'tako_run_defaults', 'tako_run_interactive', 'tako_run_interactive_status'],
  },
  {
    title: 'セットアップ・OS 連携',
    intro: '初回セットアップ、MCP 登録、ファイル操作、ポート検知、スリープ防止。',
    keys: [
      'tako_setup', 'tako_setup_changes', 'tako_setup_mcp', 'tako_agents_sync_rules',
      'tako_stale_binary', 'tako_tree_folder', 'tako_file_op', 'tako_port_detect',
      'tako_sleep_guard', 'tako_fda',
    ],
  },
  {
    title: 'リモートアクセス・自動更新',
    intro: 'スマホからの接続（tako remote）とアプリ内アップデート。',
    keys: [
      'tako_remote_start', 'tako_remote_stop', 'tako_remote_status', 'tako_remote_setup',
      'tako_remote_devices', 'tako_remote_agents', 'tako_remote_messages',
      'tako_remote_scrollback', 'tako_update',
    ],
  },
];

const LABEL = {
  supported: '対応済み',
  degraded: '一部対応',
  pending: '未対応',
  unsupported: '対象外',
};

function resolveBin() {
  const explicit = process.argv.indexOf('--bin');
  if (explicit !== -1 && process.argv[explicit + 1]) return process.argv[explicit + 1];
  for (const rel of ['target/debug/tako.exe', 'target/debug/tako', 'target/release/tako.exe', 'target/release/tako']) {
    const p = join(ROOT, rel);
    if (existsSync(p)) return p;
  }
  return 'tako';
}

function platformJson(bin, platform) {
  // 日本語の doc サイト向けに理由文も日本語で取る（設定は書き換えず env で上書きする）
  const out = execFileSync(bin, ['platform', '--platform', platform, '--json'], {
    encoding: 'utf8',
    env: { ...process.env, TAKO_LANG: 'ja' },
  });
  return JSON.parse(out);
}

function render(win, mac) {
  const byKey = new Map(win.features.map((f) => [f.key, f]));
  const macByKey = new Map(mac.features.map((f) => [f.key, f]));

  // 分類漏れ検出。support.rs に機能が増えたらここで落として気付かせる（T1 と同じ発想）
  const categorized = new Set(CATEGORIES.flatMap((c) => c.keys));
  const missing = [...byKey.keys()].filter((k) => !categorized.has(k));
  const stale = [...categorized].filter((k) => !byKey.has(k));
  if (missing.length || stale.length) {
    const lines = [];
    if (missing.length) lines.push(`カテゴリ未分類の機能: ${missing.join(', ')}`);
    if (stale.length) lines.push(`マトリクスに存在しないキー: ${stale.join(', ')}`);
    lines.push('→ scripts/gen-windows-support-docs.mjs の CATEGORIES を直してください');
    throw new Error(lines.join('\n'));
  }

  const c = win.counts;
  const total = win.total ?? win.features.length;
  const out = [];
  out.push('---');
  out.push('title: Windows 対応状況');
  out.push('description: macOS 先行で開発している機能のうち、Windows でどこまで使えるかの一覧。');
  out.push('---');
  out.push('');
  out.push(':::caution[このページは自動生成です]');
  out.push('内容は tako 本体の対応マトリクス（`crates/tako-core/src/platform/support.rs`）から');
  out.push('`scripts/gen-windows-support-docs.mjs` で生成しています。手で編集しないでください。');
  out.push(':::');
  out.push('');
  out.push('tako は **macOS で先行開発し、安定した差分を Windows へ反映する**進め方をとっています。');
  out.push('Windows 版は現在**テスター向けプレビュー**で、まだ macOS 版と同じではありません。');
  out.push('このページは「いま Windows で何が使えるか」を機能ごとに示します。');
  out.push('');
  out.push('手元の環境での状態は `tako platform` でいつでも確認できます。');
  out.push('');
  out.push('```bash');
  out.push('tako platform                  # この環境の対応状況');
  out.push('tako platform --status pending # 未対応のものだけ');
  out.push('```');
  out.push('');
  out.push('## 全体');
  out.push('');
  out.push('| 状態 | 件数 | 意味 |');
  out.push('| --- | ---: | --- |');
  out.push(`| 対応済み | ${c.supported} | macOS 版と同じように使えます |`);
  out.push(`| 一部対応 | ${c.degraded} | 使えますが機能が落ちます（理由は各表に記載） |`);
  out.push(`| 未対応 | ${c.pending} | まだ実装されていません（追跡 Issue つき） |`);
  out.push(`| 対象外 | ${c.unsupported} | Windows には概念自体が存在しません |`);
  out.push(`| **合計** | **${total}** | |`);
  out.push('');

  for (const cat of CATEGORIES) {
    const rows = cat.keys.map((k) => byKey.get(k));
    const worst = rows.some((r) => r.status === 'pending')
      ? rows.every((r) => r.status === 'pending' || r.status === 'unsupported')
        ? '未対応'
        : '一部対応'
      : rows.some((r) => r.status === 'degraded')
        ? '一部対応'
        : '対応済み';
    out.push(`## ${cat.title}`);
    out.push('');
    out.push(`${cat.intro}（このカテゴリ全体としては **${worst}**）`);
    out.push('');
    out.push('| 機能 | Windows | 補足 |');
    out.push('| --- | --- | --- |');
    for (const r of rows) {
      const m = macByKey.get(r.key);
      // macOS 側が Supported でないものは「mac 先行の差分」ではないので明示する
      const macNote = m && m.status !== 'supported' ? `（macOS でも${LABEL[m.status]}）` : '';
      let note = (r.note || '').replace(/\|/g, '\\|');
      if (r.issue) note += `${note ? ' ' : ''}<br />追跡: [#${r.issue}](https://github.com/takushio2525/tako/issues/${r.issue})`;
      out.push(`| \`${r.key}\` | ${LABEL[r.status]}${macNote} | ${note || '—'} |`);
    }
    out.push('');
  }

  out.push('## この表の読み方');
  out.push('');
  out.push('- 機能名は **MCP ツール名**です。同じ操作が CLI（`tako …`）と GUI からもできます');
  out.push('  （tako の開発原則「UI でできることはすべて AI からもできる」）。');
  out.push('  対応する CLI は [CLI リファレンス](/guides/cli-reference/)、');
  out.push('  ツールの詳細は [MCP ツール一覧](/guides/mcp-tools/)を参照してください');
  out.push('- **未対応の機能も一覧から消していません**。消すと AI エージェントが');
  out.push('  「そんな機能は無い」と誤認して回避行動も取れなくなるためです。');
  out.push('  未対応の操作を呼ぶと、理由と追跡 Issue を含むエラーが返ります');
  out.push('- 縮退の理由は tako 本体の 1 箇所で定義していて、このページ・`tako platform`・');
  out.push('  エージェントの system prompt がすべて同じ文言を使います');
  out.push('');
  return out.join('\n') + '\n';
}

const bin = resolveBin();
let text;
try {
  text = render(platformJson(bin, 'windows'), platformJson(bin, 'macos'));
} catch (e) {
  console.error(`生成に失敗: ${e.message}`);
  process.exit(1);
}

if (process.argv.includes('--check')) {
  const current = existsSync(OUT) ? readFileSync(OUT, 'utf8') : '';
  if (current !== text) {
    console.error('docs/src/content/docs/windows-support.md が対応マトリクスと食い違っています');
    console.error('→ node scripts/gen-windows-support-docs.mjs で再生成してください');
    process.exit(1);
  }
  console.log('windows-support.md は最新です');
} else {
  writeFileSync(OUT, text);
  console.log(`generated ${OUT}`);
}
