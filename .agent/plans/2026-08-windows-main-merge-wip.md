# windows/467 × main 統合の作業記録（2026-08-21）

`windows/467-ipc-orchestration-local` へ `origin/main`（bb3033a）をマージする作業の途中状態。
**このコミットはビルドが通らない**（下記 4 ファイルに衝突マーカーが残っている）。

## 規模（実測）

| 指標 | 値 |
|---|---|
| main が統合ブランチより先行 | 133 コミット |
| 統合ブランチが main より先行 | 75 コミット |
| 衝突ファイル | 45 |
| 衝突 hunk | 213 |
| 衝突行（ours + base + theirs） | 27,353 |
| histogram diff での改善 | なし（28,869 行に増える = 真の衝突） |

大物ファイルの main → win467 実質差分（= 再適用が必要な Windows デルタ）:

| ファイル | main→win |
|---|---|
| tako-app/src/main.rs | +3,868 / −25,075 |
| tako-control/src/dispatch.rs | +2,550 / −4,486 |
| tako-cli/src/main.rs | +1,218 / −1,600 |
| tako-app/src/update_checker.rs | +1,118 / −629 |
| tako-control/src/orchestrator/mod.rs | +802 / −1,061 |
| tako-app/src/status_bar.rs | +666 / −9 |
| tako-control/src/mcp.rs → mcp/catalog.rs | 0 / −3,053（#750 のモジュール分割と衝突） |

## 未解決（マーカーが残っているファイル）

- `crates/tako-app/src/main.rs`（44 hunk）
- `crates/tako-control/src/dispatch.rs`（40 hunk）
- `crates/tako-cli/src/main.rs`（13 hunk）
- `docs/src/content/docs/guides/keyboard-shortcuts.md`（7 hunk。win467 が表を
  「操作 / macOS / Windows・Linux」の 3 列へ再構成（#585）した一方、main は 2 列のまま
  行を約 47 追加。どちらかの表形式へ寄せて行を移植する作業が要る）

## 止めた理由（機能スコープの判断が要る 3 件）

統合ブランチは Windows 対応だけでなく、**main に存在しない一般機能**も抱えている。
これらは main 側の同一サブシステムの進化と正面衝突しており、
「どちらの設計を採るか」はマージ作業ではなく設計判断。

1. **#665 起動保証 + worker 常時監視**
   `supervisor.rs` は win467 が main の復旧サブシステム（`SupervisorContext` /
   `RecoveryEntry` / `recover_api_error` / `recover_limit_dialog` / `recover_dead` /
   `recover_prompt_undelivered` = 計 434 行）を**意図的に削除**して
   イベントストリーム設計（`SupervisorEvent` / journal / `decide_auto_action` /
   `supervisor_run`）へ置き換えている。main はその削除された側へさらに
   #748（ダイアログ）/ #813（上限自動復帰）/ #401 を積んでいる。**union は成立しない。**
   → 本 WIP では `supervisor.rs` を main 正（全文）にした。
   結果として #665 の「常時監視」は落ち、「起動保証」（`launch.rs` /
   `launch_assurance.rs` / `launch-status`）だけが宙に浮いている。
   **半分だけ残すのは surface が不整合になる**（MCP catalog は main 正なので
   `tako_orchestrator_launch_status` が公開されない = 開発不変条件 5 違反）。
   決めるべきこと: #665 を丸ごと落として後日 main へ移植し直すか、
   main の復旧サブシステムの上に #665 を再設計するか。

2. **#662 AskUserQuestion 対話（`tako orchestrator dialog` / `respond --answer` / `tako keys`）**
   main の #748 が「番号つき / 番号なしの選択肢ダイアログ全般」を一般化して
   カバーしており、`--choice` 省略の下見も持つ。**機能が重複している。**
   → 本 WIP では protocol の `answers` / `dry_run` フィールドは残したが、
   dispatch / CLI 側は未解決のまま。

3. **#709 claude アカウントの一覧・切替（`tako account`）**
   main は `tako orchestrator accounts`（#504 / #548）で同じ領域を持つ。
   `account_cli.rs` はマージ後のツリーに存在しない（main 側に無い）。

## 解決した内容（41 ファイル）— 判断の根拠つき

### main 正に倒した（Windows デルタを再適用したものは併記）

| ファイル | 判断 |
|---|---|
| `Cargo.toml` / `Cargo.lock` | バージョンは main の 0.7.4 |
| `orchestrator/supervisor.rs` | main 正（全文）。上記 1 のとおり |
| `mcp/catalog.rs` | main 正（全文）。win467 の `mcp.rs` 単一ファイルは #750 で分割済み。Windows 側の MCP ツール追加は再移植が必要 |
| `transcript.rs` | main 正 + **Windows デルタ再適用**: `resume_env_prefix_for` を `agent::env_unset` / `env_assign`（シェル方言）へ。テストも方言追従版へ |
| `orchestrator/mod.rs` | main 正 + **Windows デルタ再適用**: env 注入を方言部品へ / #592 の `run_claude_agents_json_for`（Windows は claude 直起動。ログインシェルが無い） |
| `update_checker.rs` | main 正（#595 は実リリース 28 件で照合済み・Windows アセットも扱う）+ `CURRENT_VERSION` を `TAKO_FULL_VERSION` へ（#723 の `-win.N`）。**`effective_current_version()`（インストーラーの DisplayVersion 参照）は未移植** |
| `status_bar.rs` | main 正。main が #616 で `render_update_banner` を撤去済みなので win467 の改修は moot |
| `stale_binary.rs` | main 正（#772 の PATH 走査）+ 保険の `which` に `no_console_window`（#628） |
| `sessions.rs` | main の `env_prefix` 引数構造 + win467 の `launch_with_role`（方言）。configdir 前置テストも方言追従版へ |
| `orchestrator/agent.rs` | main の `EnvPlan` + win467 の `env_unset` / `env_assign` 出力 |
| `orchestrator/registry.rs` | main の #530 フィールド + win467 の `launch` フィールド（上記 1 の宙ぶらりん分） |
| `orchestrator/wait.rs` | **win467 の `WatchStreaks` 抽出を保持**し、main の #748 `ChoiceWaiting` / #577 permission フォールバックを `evaluate` へ移植 |
| `protocol.rs` | main の `inherit: Option<bool>` / `choice: Option<String>`（#748）+ win467 の `answers` / `dry_run`（#662。上記 2） |
| `i18n.rs` | main の `testing::lang_guard`（#608）+ win467 の `macos_preferred_language` 削除（`platform::locale` へ移設済み）を尊重 |
| `platform/support.rs` | 両側の改善を合成（win467 の `any_pending_on_windows` + main の `lang_guard` / `gate_in`） |

### 両方採用（加算衝突）

`platform/mod.rs`（モジュール宣言）/ `tako-core/src/lib.rs` / `agents.rs`（#592 Toolhelp32 + main の sticky live）/
`terminal.rs`（#686 copy mode gate + #816 wakeup gate）/ `file_icons.rs` / `ui_text/palette.rs` /
`.agent/progress.md` / `.agent/manual-checks.md` / `.agent/orchestrator.md` / `docs/.../cli-reference.md`

### 個別合成

`autorename.rs`（main の `lang` 引数 + #586 `no_console_window`）/ `ui_text/common.rs`（`update()` → `close()` 改称に追従 + `copy` / `paste`）/
`preview_render.rs`（#826 `body_virtualized` + #654 スクロール診断）/ `settings_window.rs`（#550 隠しファイル行 + B16 既定フォント）/
`tako-control/src/lib.rs`（re-export に `AccountParams`）/ `remote.rs`（#662 フィールドを既定値で埋める）/
`tako-cli/src/setup.rs`（#513 設定共有 + シェル統合チェック）/ `default_system_prompt.md`（#662 節 + #748 節、末尾行は #748 版）/
`issue652_resume_e2e.rs`（win467 の方言対応版 + main の「/private/tmp 直下」意図を unix 側で維持）/
`AGENTS.md`（Windows 専用行を残しつつ main の更新行を正に）/ `.agent/requirements.md`（FR-5.9 は Windows 方言版、FR-5.11 / FR-5.15 は main）/
`docs/.../index.mdx`（Windows タブ + main の #549 / #600 段落）/ `docs/.../mcp-tools.md` / `.agent/activeContext.md`（main 正・後で上書き前提）

## 推奨（次に取るべき道）

**main → win467 のマージを続けるより、win467 の Windows 対応を main へスライス移植する方が安い。**

根拠:
- main は `tako-app/src/main.rs` だけで +25,075 行（#786 / #787 / #801 / #803 / #816 / #821 /
  #826 / #830 の描画刷新一式）。マージはこの再構成すべてと戦うことになる
- win467 の Windows デルタは大物 6 ファイルで **約 10,200 行**で、しかも**加算的**
  （`platform/` 新モジュール・`cfg` 分岐・backend 実装）
- win467 の `mcp.rs` は main の #750 モジュール分割より前の姿
- チームは既にこのパターンを採っている（`windows/693-pdf-links` /
  `improve/656-md-preview-cherry-pick` / `windows/521-video` / `windows/722-auto-rename` /
  `windows/728-sessions` / `windows/528-remote` / `windows/shift-enter-fix`）

スライス候補（いずれも main を base に、依存の薄い順）:

1. `platform/` 境界の追加分（`console` / `exe` / `font` / `ime` / `install_info` / `locale` / `process` / `procinfo`）
2. 永続化バックエンド `backend/psmux.rs` + ConPTY（#518 / #519）
3. IPC named pipe（#467）
4. キーボード / IME / フォント / コンソール抑止（#517 / #575 / #582 / #585 / #586）
5. ウィンドウコントロール + in-window メニューバー（#584 / #657）
6. インストーラー + リリース（#587 / #723）
7. シェル統合 PowerShell（#525）
8. doc / 対応マトリクス（#528 / #591 / #515）

一般機能（#665 / #662 / #709 / #640）は Windows 対応とは独立に、
main の現行設計（#748 / #749 / #790 / #813 / #504 / #548）との重複を整理してから判断する。
