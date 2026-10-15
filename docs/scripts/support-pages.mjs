#!/usr/bin/env node
// 対応状況の 2 ページ（`windows-support.md` / `agent-support.md`）を、コミットされた
// マトリクスの写し（`src/data/windows-support.json` / `src/data/agent-support.json`）から
// 組み立てる（Issue #2104）。
//
// **なぜ md をコミットせずビルド時に組むのか**: 以前は md そのものをコミットしていた
// （#591 / #982）。md には「全体」の件数・節ごとの件数のように**どの行が変わっても書き換わる行**と、
// 対応表では離れているのにページの上では隣り合う行があり、対応表（support.rs /
// agent_support.rs）の別々の行を直す PR が並ぶと、後の PR が毎回 md で衝突していた
// （2026-10-15 の 1 晩で 3 回。そのたびに rebase → 作り直し → CI 40 分の回し直し）。
// 写しの JSON は**件数を持たず、1 行 = 1 フィールド・行の並びは対応表と同じ**なので、
// 別の行を直す PR どうしは JSON でも別の行に当たり、衝突しない。件数はここで数える。
//
// 写しを作ること・写しが tako 本体と食い違わないことの検査は `scripts/gen-*-support-docs.mjs`
// が受け持つ（tako バイナリが要る。CI の macOS ジョブが `--check`）。ここは node だけで動く
// （Cloudflare Pages のビルド環境には Rust も tako も無い）。
//
//   node scripts/support-pages.mjs     # 2 ページを書き出す（astro の build / dev が自動で呼ぶ）
//
// カテゴリ未分類の機能があると**失敗する**。対応表へ行を足したらここの CATEGORIES へも
// 1 行足すこと（分類漏れを検出するための仕掛けで、放置すると表から機能が消える）。

import { existsSync, readFileSync, writeFileSync } from 'node:fs';
import { dirname, join, relative, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const DOCS = resolve(dirname(fileURLToPath(import.meta.url)), '..');

/** マトリクスの写し（コミットする） */
export const WINDOWS_DATA = join(DOCS, 'src/data/windows-support.json');
export const AGENT_DATA = join(DOCS, 'src/data/agent-support.json');
/** 組み立てたページ（コミットしない。docs/.gitignore） */
export const WINDOWS_PAGE = join(DOCS, 'src/content/docs/windows-support.md');
export const AGENT_PAGE = join(DOCS, 'src/content/docs/agent-support.md');

const STATUSES = ['supported', 'degraded', 'pending', 'unsupported'];

// ── 写し（`tako … --json` → src/data/*.json）────────────────────────────────
//
// ページが読むフィールドだけを、**決まった順で 1 行ずつ**残す。件数（counts / total /
// matrix_total）は持たない（どの行が変わっても書き換わる = 並列の PR が必ず衝突する行になる）。
// `note` / `summary` は実行環境の言語で中身が変わるので持たず、言語に依存しない `*_ja` を使う

const WINDOWS_FIELDS = ['key', 'status', 'note_ja', 'evidence', 'evidence_detail'];
const AGENT_FIELDS = ['key', 'summary_ja', 'evidence', 'evidence_detail'];
const AGENT_CELL_FIELDS = ['status', 'note_ja', 'issue'];
const AGENT_META_FIELDS = ['key', 'label', 'baseline'];

function pick(obj, fields) {
  const out = {};
  for (const k of fields) {
    if (Object.hasOwn(obj, k)) out[k] = obj[k];
  }
  return out;
}

/** `tako platform --platform windows --json` → 写し */
export function windowsSnapshot(win, generator) {
  return {
    _note: `生成物（${generator}）。手で直さず対応表を直して作り直す（#2104）`,
    features: win.features.map((f) => pick(f, WINDOWS_FIELDS)),
  };
}

/**
 * `tako agent-support --json` → 写し。
 *
 * 系統ごとのマスは**系統の一覧（`agents`）の順**で並べる。tako の出力の中のマスの並びは
 * serde_json の `preserve_order` が効くかどうか（ビルドする crate の組み合わせ = feature の
 * 統合）で変わる（実測: `cargo build -p tako-cli` だと辞書順、workspace 全体だと定義順）。
 * 出力の並びをそのまま写すと、手元と CI で写しが食い違って `--check` が落ちる
 */
export function agentSnapshot(m, generator) {
  const order = m.agents.map((a) => a.key);
  return {
    _note: `生成物（${generator}）。手で直さず能力マトリクスを直して作り直す（#2104）`,
    agents: m.agents.map((a) => pick(a, AGENT_META_FIELDS)),
    features: m.features.map((f) => {
      const extra = Object.keys(f.agents).filter((k) => !order.includes(k));
      if (extra.length) {
        throw new Error(`${f.key} に系統の一覧に無い列があります: ${extra.join(', ')}`);
      }
      return {
        ...pick(f, AGENT_FIELDS),
        agents: Object.fromEntries(order.map((k) => [k, pick(f.agents[k] ?? {}, AGENT_CELL_FIELDS)])),
      };
    }),
  };
}

/**
 * 写しの書式。**1 行 = 1 フィールドの整形**にする（1 行 = 1 機能に詰めると、対応表で隣り合う
 * 2 行を別々の PR が直したとき JSON の隣り合う 2 行が変わり、git はそれを衝突として扱う）。
 * 整形なら隣の機能との間に `}` / `{` / `"key"` の変わらない行が必ず挟まる
 */
export function serialize(data) {
  return `${JSON.stringify(data, null, 2)}\n`;
}

// ── 件数（写しから数える）────────────────────────────────────────────────

function tally(statuses, where) {
  const counts = Object.fromEntries(STATUSES.map((s) => [s, 0]));
  for (const s of statuses) {
    if (!Object.hasOwn(counts, s)) {
      throw new Error(`${where}: 知らない状態 "${s}" があります（support-pages.mjs の STATUSES を直す）`);
    }
    counts[s] += 1;
  }
  return counts;
}

/** Windows の件数（`tako platform --json` の counts / total と同じ値になる） */
export function windowsCounts(win) {
  return {
    counts: tally(win.features.map((f) => f.status), 'windows-support.json'),
    total: win.features.length,
  };
}

/** agent ごとの件数（`tako agent-support --json` の counts / matrix_total と同じ値になる） */
export function agentCounts(m) {
  const counts = {};
  for (const a of m.agents) {
    counts[a.key] = tally(
      m.features.map((f) => f.agents[a.key]?.status),
      `agent-support.json の ${a.key}`,
    );
  }
  return { counts, matrix_total: m.features.length };
}

/** キーの並びに依らない比較用の文字列（tako の集計はキーを辞書順で出す） */
export function canonicalJson(x) {
  if (Array.isArray(x)) return `[${x.map(canonicalJson).join(',')}]`;
  if (x && typeof x === 'object') {
    return `{${Object.keys(x)
      .sort()
      .map((k) => `${JSON.stringify(k)}:${canonicalJson(x[k])}`)
      .join(',')}}`;
  }
  return JSON.stringify(x);
}

// ── Windows 対応状況（旧 scripts/gen-windows-support-docs.mjs の render）────────

// 機能キー → 利用者向けカテゴリ。**順番がページの節順**になる
const WINDOWS_CATEGORIES = [
  ['ターミナルの基本', [
    'tako_split_pane', 'tako_close_pane', 'tako_focus_pane', 'tako_resize_pane',
    'tako_equalize_layout', 'tako_move_pane_to_tab', 'tako_scroll_pane',
    'tako_scrollback',
    'tako_send_input', 'tako_read_pane', 'tako_links', 'tako_list_panes', 'tako_set_title',
    'tako_create_tab', 'tako_select_tab', 'tako_rename_tab', 'tako_reorder_tab',
    'tako_collapse_tab', 'tako_pin_tab_title', 'tako_confirm_close',
    'tako_auto_rename', 'tako_autosuggest', 'tako_window', 'tako_menu',
    'tako_background_pane', 'tako_background_list', 'tako_background_kill',
    'tako_foreground_pane', 'tako_show_command', 'tako_run', 'tako_run_resolve',
    'tako_run_defaults', 'tako_run_interactive', 'tako_run_interactive_status',
  ]],
  ['表示とプレビュー', [
    'tako_open_file', 'tako_jump', 'tako_open_dir', 'tako_preview_view', 'tako_preview_outline',
    'tako_preview_link_list', 'tako_preview_follow_link', 'tako_preview_copy_code',
    'tako_preview_reload', 'tako_preview_cache', 'tako_preview_edit',
    'tako_preview_apply', 'tako_preview_edit_range', 'tako_preview_cursor',
    'tako_preview_move', 'tako_preview_delete', 'tako_lsp', 'tako_lsp_server',
    'tako_preview_save', 'tako_preview_undo',
    'tako_preview_redo', 'tako_preview_search', 'tako_preview_replace', 'tako_search',
    'tako_preview_autosave', 'tako_preview_changelog', 'tako_pin_preview',
    'tako_video_playback', 'tako_video_seek', 'tako_video_volume', 'tako_web',
    'tako_theme', 'tako_lang', 'tako_ui_mode', 'tako_chat_copy', 'tako_panel',
    'tako_tree_folder', 'tako_welcome', 'tako_recent',
  ]],
  ['AI 連携（オーケストレーション）', [
    'tako_orchestrator_spawn', 'tako_orchestrator_self', 'tako_orchestrator_handoff',
    'tako_orchestrator_handoffs', 'tako_orchestrator_profiles',
    'tako_orchestrator_guide',
    'tako_orchestrator_projects', 'tako_orchestrator_adopt',
    'tako_orchestrator_accounts',
    'tako_orchestrator_layout', 'tako_orchestrator_workers',
    'tako_orchestrator_worker_status', 'tako_orchestrator_respond',
    'tako_orchestrator_report', 'tako_orchestrator_run',
    'tako_orchestrator_run_status', 'tako_orchestrator_run_result',
    'tako_orchestrator_supervisor', 'tako_orchestrator_ledger',
    'tako_limit_resume', 'tako_limit_service', 'tako_sessions',
    'tako_session_restart',
    'tako_task_gate', 'tako_task_gate_check', 'tako_task_gate_show',
    'tako_task_checkpoint', 'tako_task_list', 'tako_task_resume',
    'tako_todo',
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
    'tako_shell_integration', 'tako_stale_binary', 'tako_check_health', 'tako_queue',
    'tako_telemetry', 'tako_test_residue', 'tako_mod',
  ]],
  ['セットアップと設定', [
    'tako_setup', 'tako_setup_bootstrap', 'tako_setup_changes', 'tako_setup_deps',
    'tako_setup_mcp', 'tako_setup_models',
    'tako_settings', 'tako_migrate', 'tako_agents_sync_rules',
    'tako_config_share', 'tako_platform', 'tako_agent_support', 'tako_context_budget',
  ]],
  ['リモートアクセス', [
    'tako_remote_start', 'tako_remote_stop', 'tako_remote_status',
    'tako_remote_setup', 'tako_remote_devices', 'tako_remote_agents',
    'tako_remote_messages', 'tako_remote_scrollback', 'tako_open_remote',
    'tako_remote_folder', 'tako_ssh_hosts', 'tako_remote_shortcuts',
  ]],
  ['アップデート', ['tako_update']],
];

const WINDOWS_STATUS_LABEL = {
  supported: '対応',
  degraded: '一部対応',
  pending: '未対応 / 未実測', // 行ごとの内訳は「根拠」列（未実測かどうか）で読む
  unsupported: '対象外',
};

const WINDOWS_EVIDENCE_LABEL = {
  'self-test': '実機セルフテスト',
  'unit-test': '実機テスト',
  measured: '実機実測',
  'by-design': 'OS の仕様',
  unverified: '未実測',
};

function escapeWindowsCell(s) {
  return String(s ?? '').replace(/\|/g, '\\|').replace(/\n+/g, ' ');
}

/** 分類漏れの検出（両方向） */
function checkCategories(categories, features, what) {
  const keys = features.map((f) => f.key);
  const known = new Set(keys);
  const classified = new Set(categories.flatMap(([, ks]) => ks));
  const missing = keys.filter((k) => !classified.has(k));
  const stale = [...classified].filter((k) => !known.has(k));
  if (missing.length) {
    throw new Error(
      `カテゴリ未分類の${what}があります（${missing.length} 件）: ${missing.join(', ')}\n` +
        'docs/scripts/support-pages.mjs の CATEGORIES へ追加してください',
    );
  }
  if (stale.length) {
    throw new Error(`マトリクスに無いキーが CATEGORIES に残っています: ${stale.join(', ')}`);
  }
}

export function renderWindowsSupport(win) {
  const byKey = new Map(win.features.map((f) => [f.key, f]));
  checkCategories(WINDOWS_CATEGORIES, win.features, '機能');

  const { counts: c, total } = windowsCounts(win);
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
  // pending は「未実装」と「未実測」が混ざる。合算だけ見せると
  // 「半分動かない」と読まれるので内訳を出す（実測が無いだけのものが大半）
  const unverified = win.features.filter(
    (f) => f.status === 'pending' && f.evidence === 'unverified',
  ).length;
  out.push(
    `| 未実測 | ${unverified} | 実装はあり macOS と同じ経路を通るが、Windows 実機でまだ動かしていないもの |`,
  );
  out.push(
    `| 未対応 | ${c.pending - unverified} | Windows 側の実装が無い、または動かないことが分かっているもの |`,
  );
  out.push(`| 対象外 | ${c.unsupported} | Windows にその概念が無い、または OS が同等機能を標準で持つ |`);
  out.push('');
  out.push('### 「未実測」について');
  out.push('');
  out.push('tako は **実機で確かめたものだけを「対応」と書きます**。実装がプラットフォーム');
  out.push('共通で動く見込みがあっても、Windows 実機で 1 度も実行していないものは');
  out.push('「未対応 / 未実測」に置いています。過大に申告すると、この宣言を読んで動く');
  out.push('AI エージェント（tako は縮退の件数と引き方を system prompt へ渡し、理由の全文は');
  out.push('`tako orchestrator guide platform` で引けるようにしています）が');
  out.push('「使えるはず」と信じて失敗し続けるためです。');
  out.push('');
  out.push('各表の「根拠」列が判定の裏づけです。');
  out.push('');
  out.push('| 根拠 | 意味 |');
  out.push('| --- | --- |');
  out.push('| 実機セルフテスト | Windows 実機の GUI セルフテスト（通しで失敗 0 件）が実際に通した項目 |');
  out.push('| 実機テスト | Windows 実機の `cargo test` で緑のテスト |');
  out.push('| 実機実測 | Windows 実機で操作を実行して結果を記録したもの |');
  out.push('| OS の仕様 | Windows の仕様・設計判断で、実測する対象がそもそも無いもの |');
  out.push('| 未実測 | まだ実機で動かしていないもの |');
  out.push('');

  for (const [label, keys] of WINDOWS_CATEGORIES) {
    const rows = keys.map((k) => byKey.get(k));
    const counts = rows.reduce((acc, f) => {
      acc[f.status] = (acc[f.status] ?? 0) + 1;
      return acc;
    }, {});
    const summary = STATUSES
      .filter((s) => counts[s])
      .map((s) => `${WINDOWS_STATUS_LABEL[s]} ${counts[s]}`)
      .join('・');
    out.push(`## ${label}`);
    out.push('');
    out.push(`${summary}`);
    out.push('');
    out.push('| 機能 | 状態 | 差分 | 根拠 |');
    out.push('| --- | --- | --- | --- |');
    for (const f of rows) {
      // 表示言語に依存しない ja を明示的に使う。`note` は実行環境の言語で
      // 変わるので、これを使うと生成した人のロケールで中身が変わってしまう
      const diff = f.status === 'supported' ? '—' : escapeWindowsCell(f.note_ja);
      const ev = WINDOWS_EVIDENCE_LABEL[f.evidence] ?? f.evidence;
      const detail = f.evidence_detail ? `${ev}: ${escapeWindowsCell(f.evidence_detail)}` : ev;
      out.push(`| \`${f.key}\` | ${WINDOWS_STATUS_LABEL[f.status]} | ${diff} | ${detail} |`);
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

// ── エージェント別の対応状況（旧 scripts/gen-agent-support-docs.mjs の render）────

// 能力キー → 利用者向けカテゴリ。**順番がページの節順**になる
const AGENT_CATEGORIES = [
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
    'worker_model_default_inherit',
  ]],
  ['worker の監視', [
    'worker_status_detect', 'worker_status_structured',
    'worker_idle_with_background',
    'worker_prompt_undelivered', 'worker_death_resume', 'network_autoresume',
    'worker_refusal_detect', 'worker_refusal_work_proof',
    'worker_login_expired_detect', 'claude_mod_state', 'claude_mod_band', 'claude_mod_buttons', 'claude_mod_cards', 'claude_mod_chat', 'claude_mod_ui',
    'claude_mod_install',
  ]],
  ['worker への指示と応答', [
    'worker_prompt_delivery', 'worker_delivery_peer',
    'worker_permission_dialog', 'worker_choice_dialog', 'worker_cli_control',
    'worker_mcp',
  ]],
  ['報告と会話ログ', [
    'worker_report_scrollback', 'worker_report_transcript',
    'sessions_catalog', 'sessions_resume', 'resume_launch_args',
    'session_restart_harness', 'session_restart_handoff',
    'restore_after_reboot',
  ]],
  ['利用制限', [
    'worker_limit_detect', 'worker_limit_autoresume', 'limit_autocontinue_upstream',
    'worker_limit_metrics', 'worker_entitlement_detect',
    'limit_service_switch',
  ]],
  ['リモートからの操作', ['remote_control']],
  ['その他', ['git_resolve_agent']],
];

const AGENT_STATUS_LABEL = {
  supported: '対応',
  degraded: '一部対応',
  pending: '未対応',
  unsupported: '対象外',
};

const AGENT_EVIDENCE_LABEL = {
  source: 'コード本文',
  'self-test': '実機セルフテスト',
  'unit-test': 'テスト',
  measured: '実測',
  'by-design': '上流の仕様',
  unverified: '未確認',
};

function escapeAgentCell(s) {
  // `<` を残すと `brain/<id>` のような書き方が HTML タグ扱いで**消える**（実測）。
  // 表のセルへ入るのは常に平文なので、山括弧はここで実体参照へ倒す。
  // `<br />` は呼び出し側が escapeAgentCell の外で足すので影響しない
  return String(s ?? '')
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/\|/g, '\\|')
    .replace(/\n+/g, ' ');
}

export function renderAgentSupport(m) {
  const byKey = new Map(m.features.map((f) => [f.key, f]));
  const agents = m.agents;
  checkCategories(AGENT_CATEGORIES, m.features, '能力');

  const { counts: agentCount, matrix_total: total } = agentCounts(m);
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
  out.push('入れ方・つなぎ方は系統ごとのページにあります:');
  out.push('[エージェントの選び方](/agents/) /');
  out.push('[Claude Code](/agents/claude/) /');
  out.push('[OpenAI Codex CLI](/agents/codex/) /');
  out.push('[Antigravity CLI](/agents/agy/) /');
  out.push('[ローカル LLM](/agents/local-llm/)。');
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
    const c = agentCount[a.key];
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

  // 系統ごとの縮退ダイジェスト（#992）。**能力別の表と同じ 1 つのマトリクスから作る**ので、
  // 「選ぶ前に何が落ちるか」を読むための別の切り口でありながら本文とずれない。
  // 同じ理由のマスは 1 つにまとめる（agy の master 系 / ローカル LLM の未成立が
  // 1 行ずつ並ぶと読めなくなるため）
  for (const a of agents) {
    if (a.baseline) continue;
    const c = agentCount[a.key];
    // 1 つも成立していない系統に「選ぶと落ちる」は成り立たない（そもそも選べない）
    const usable = c.supported > 0;
    out.push(`## ${a.label} ${usable ? 'を選ぶと落ちるもの' : 'でまだ使えないもの'}`);
    out.push('');
    out.push(
      usable
        ? `対応 ${c.supported} / ${total} 件。以下は Claude Code との差分です` +
            '（同じ理由のものはまとめています）。'
        : `対応 ${c.supported} / ${total} 件。この系統が成立したときに埋まるマスの一覧です` +
            '（同じ理由のものはまとめています）。',
    );
    out.push('');
    for (const status of ['degraded', 'pending', 'unsupported']) {
      const rows = m.features.filter((f) => f.agents[a.key].status === status);
      if (!rows.length) continue;
      out.push(`### ${AGENT_STATUS_LABEL[status]}（${rows.length} 件）`);
      out.push('');
      // 理由（note_ja + issue）でまとめる。挿入順 = マトリクスの並びを保つ
      const groups = new Map();
      for (const f of rows) {
        const cell = f.agents[a.key];
        const key = `${cell.note_ja ?? ''}\u0000${cell.issue ?? ''}`;
        if (!groups.has(key)) groups.set(key, { cell, items: [] });
        groups.get(key).items.push(f);
      }
      for (const { cell, items } of groups.values()) {
        const issue = cell.issue
          ? `（追跡: [#${cell.issue}](https://github.com/takushio2525/tako/issues/${cell.issue})）`
          : '';
        const reason = cell.note_ja ? escapeAgentCell(cell.note_ja) : '理由の記載がありません';
        out.push(`- **${reason}**${issue}`);
        for (const f of items) {
          out.push(`  - ${escapeAgentCell(f.summary_ja)}（\`${f.key}\`）`);
        }
      }
      out.push('');
    }
  }

  for (const [label, keys] of AGENT_CATEGORIES) {
    out.push(`## ${label}`);
    out.push('');
    const head = agents.map((a) => a.label).join(' | ');
    out.push(`| 能力 | ${head} | 根拠 |`);
    out.push(`| --- | ${agents.map(() => '---').join(' | ')} | --- |`);
    for (const k of keys) {
      const f = byKey.get(k);
      const cells = agents.map((a) => {
        const cell = f.agents[a.key];
        const label = AGENT_STATUS_LABEL[cell.status] ?? cell.status;
        if (cell.status === 'supported') return label;
        // 表示言語に依存しない ja を明示的に使う。`note` は実行環境の言語で
        // 変わるので、これを使うと生成した人のロケールで中身が変わってしまう（#591）
        const issue = cell.issue ? ` [#${cell.issue}](https://github.com/takushio2525/tako/issues/${cell.issue})` : '';
        return `${label}${issue}<br />${escapeAgentCell(cell.note_ja)}`;
      });
      const ev = AGENT_EVIDENCE_LABEL[f.evidence] ?? f.evidence;
      const detail = f.evidence_detail ? `${ev}: ${escapeAgentCell(f.evidence_detail)}` : ev;
      out.push(`| **${escapeAgentCell(f.summary_ja)}**<br />\`${f.key}\` | ${cells.join(' | ')} | ${detail} |`);
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

// ── 書き出し ────────────────────────────────────────────────────────────

function readData(path) {
  let raw;
  try {
    raw = readFileSync(path, 'utf8');
  } catch {
    throw new Error(
      `マトリクスの写しを読めません: docs/src/data/${path.split('/').pop()}\n` +
        '作り方は scripts/gen-windows-support-docs.mjs / scripts/gen-agent-support-docs.mjs',
    );
  }
  return JSON.parse(raw);
}

/** 中身が同じなら書かない（astro dev の監視を余計に起こさない）。書いたら true */
export function writeIfChanged(path, body) {
  if (existsSync(path) && readFileSync(path, 'utf8') === body) return false;
  writeFileSync(path, body);
  return true;
}

/** 2 ページを写しから組み立てて書き出す。書き換えたページのパスを返す */
export function writeSupportPages() {
  const pages = [
    [WINDOWS_PAGE, renderWindowsSupport(readData(WINDOWS_DATA))],
    [AGENT_PAGE, renderAgentSupport(readData(AGENT_DATA))],
  ];
  return pages.filter(([path, body]) => writeIfChanged(path, body)).map(([path]) => path);
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    const written = writeSupportPages();
    console.log(
      written.length
        ? `組み立てました: ${written.map((p) => relative(DOCS, p)).join(', ')}`
        : '対応状況の 2 ページは最新です',
    );
  } catch (e) {
    console.error(e instanceof Error ? e.message : String(e));
    process.exit(1);
  }
}
