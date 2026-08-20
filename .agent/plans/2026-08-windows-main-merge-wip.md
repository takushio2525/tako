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

---

## 正式決定（2026-08-21、master 裁定）

1. **#665 / #662 / #709 は統合から落とす**。#662 は main の #748（選択肢ダイアログ全般 +
   `--choice` 省略の下見）、#709 は main の `tako orchestrator accounts`（#504 / #548）で
   代替済み。#665 は「main の復旧サブシステムの上に Windows 差分だけ後日移植」として追跡継続
2. **進め方はスライス移植を正式採用**。`windows/467-main-merge-wip` は判断ログとして残置し、
   マージは再開しない（PR #588 もマージしない）
3. Windows 検証機の stale `tako.exe` 2 プロセスは kill 済み。**既定 target が使える**
   （kill 後に `cargo build --workspace` = 19.15s / exit 0 を実測）

## スライス移植の推奨順序と依存関係

**すべて `origin/main` を base に切る**。1 スライス = 1 ブランチ = 1 PR。
各スライスの Definition of Done に「macOS の CI 緑」と「Windows 実機での該当機能の実測」を含める。

```
  1. platform/ 境界（基盤）
        │
        ├──> 2. 永続化バックエンド（psmux / ConPTY）
        │         │
        │         └──> 7. シェル統合（PowerShell）
        │
        ├──> 3. IPC named pipe
        ├──> 4. 入力系（キーボード / IME / フォント / コンソール抑止）
        │         │
        │         └──> 5. ウィンドウコントロール + in-window メニューバー
        ├──> 6. インストーラー + リリース
        └──> 9. スリープ防止 + 蓋閉じ継続
                  │
        すべて ───┴──> 8. doc / 対応マトリクスの最終棚卸し
```

### 1. `platform/` 境界（基盤）— **最初にやる。他の全スライスがここを呼ぶ**

- 持ち込む新規: `crates/tako-core/src/platform/{console,exe,font,ime,install_info,locale,process,procinfo}.rs`
  / `crates/tako-app/src/platform/{mod.rs,pdf/{mod,macos,windows}.rs}`
- 編集: `crates/tako-core/src/platform/mod.rs`（mod 宣言）/ `platform/support.rs`（マトリクス）
- 呼び出し側の `cfg` 除去（#522 の `os_integration` 集約と同じ作法）
- **main 側に既にあるもの**: `platform/{clock,quit_signal,release_assets,shell,support}.rs`。
  重複させない
- 依存: なし
- 検証の効く場所: `support.rs` のパリティテスト T1〜T6 が macOS 上で走る。
  番犬テスト「OS 連携の直呼びが境界の外に残っていない」も同様

### 2. 永続化バックエンド（psmux / ConPTY。#518 / #519）

- 持ち込む新規: `crates/tako-core/src/backend/{psmux,owner}.rs` /
  `crates/tako-core/tests/{psmux_backend,encoding_conpty}.rs` / `poc/conpty-survival/`
- 編集: `crates/tako-core/src/backend/{mod,null,tmux}.rs`
- 依存: **1**（`platform::console` = コードページ固定 / `platform::process` = コンソール窓抑止）
- **前提作業**: Windows 検証機に psmux を入れる（`.agent/windows-setup.md` §3.5）。
  未導入だと `tests/psmux_backend.rs` の 8 件が
  `psmux: no server running on session ...` で落ちる（2026-08-21 実測）。
  未導入環境では `#[ignore]` にするかも同時に決める
- 注意: main は #817 で `pty_loop.rs` を新設し PTY reader を自前ループにしている。
  ConPTY 側はこの新しいループの上に載せる（upstream の `EventLoop` 前提で書かない）

### 3. IPC named pipe（#467）

- 持ち込む新規: `crates/tako-control/src/platform/named_pipe.rs`
- 編集: IPC のトランスポート選択（`ipc.rs`）
- 依存: **1**
- 独立性が高いので 2 と並行してよい

### 4. 入力系: キーボード / IME / フォント / コンソール抑止（#517 / #575 / #582 / #585 / #586）

- 編集: `crates/tako-app/src/keybindings.rs`（45 本の個別分類）/ `main.rs`（IME アンカー）/
  `settings_window.rs`（既定フォント）/ `autorename.rs` / `stale_binary.rs` /
  `orchestrator/mod.rs`（コンソール窓抑止の呼び出し）
- 依存: **1**（`platform::{font,ime,process}`）
- **main 側との衝突に注意**: main は #781（stale claude バナーの会計）/ #803（ヘッダの
  兄弟化）/ #787（端末グリッドの Element 化）でペイン内の幾何を作り直している。
  win467 の #582（IME 候補位置）/ #647（テキスト領域の共通の正）は
  **main の `pane_text_area_rect` の上に載せ直す**。旧実装を戻さない
- `crates/tako-core/src/keys.rs` は #662 由来（`tako keys`）。**#662 を落とす決定**なので
  持ち込まない。Windows のキー送出で必要になったらそのとき最小限で足す

### 5. ウィンドウコントロール + in-window メニューバー（#584 / #657）

- 持ち込む新規: `crates/tako-app/src/menu_bar.rs` /
  `assets/icons/ui/window_{maximize,restore}.svg`
- 編集: `main.rs` / `tab_bar.rs` / dispatch の `WindowState`
- 依存: **1**, **4**（メニューのアクセラレータがキーバインドの分類に乗る）
- 注意: アイコンは `EMBEDDED_ASSETS` への登録漏れ検査テストがあるので必ず登録する（#561 の副産物）

### 6. インストーラー + リリース（#587 / #723）

- 持ち込む新規: `installer/windows/{build-installer.ps1,make-icon.ps1,release-windows.ps1,tako.iss}`
  / `assets/icon/tako.ico` / `.cargo/config.toml`（MSVC CRT 静的リンク）/
  `crates/tako-app/build.rs` / `crates/tako-cli/build.rs`（`TAKO_FULL_VERSION`）
- 編集: `update_checker.rs` に `effective_current_version()`（インストーラーの
  `DisplayVersion` を最優先。#723 の無限「更新あり」ループ対策）
- 依存: **1**（`platform::install_info`）
- **main 側に既にあるもの**: #594 / #595 のアセット命名規則
  （`platform::release_assets` + `scripts/lib/release-assets.sh`）は main が正で、
  Windows アセット（`tako-vX-windows-x86_64.exe` / `.zip`）も既に扱える。
  win467 の #528 `UpdateTarget` は**持ち込まない**（#595 が上位互換）
- 本 WIP で済んでいる分: `CURRENT_VERSION` を `env!("TAKO_FULL_VERSION")` にする最小差分は
  `windows/467-main-merge-wip` に入っている。参考にできる

### 7. シェル統合 PowerShell（#525）

- 持ち込む新規: `crates/tako-core/shell-integration/tako.ps1` /
  `crates/tako-core/tests/shell_integration_powershell.rs`
- 編集: `shell_integration.rs` / `osc_tap.rs`（`file:///C:/...` の先頭 `/` 落とし）/
  `platform/support.rs` / `resources/setup/changes.yaml`
- 依存: **1**, **2**（器越しの OSC パススルー）
- **WIP が保全ブランチにある**: `windows/525-shell-integration`（`f58a994`）。未完成なので
  完成させるところから
- `crates/tako-core/src/shell_send.rs`（#640）はこのスライスに含める
  （resume 注入・起動コマンド投入が送達確認経路を通るため）

### 8. doc / 対応マトリクスの最終棚卸し（#528 / #591 / #515）

- 持ち込む新規: `scripts/gen-windows-support-docs.mjs` / `docs/.../windows-support.md`（生成物）
- 編集: `docs/.../getting-started/index.mdx`（Windows タブ）/
  `docs/.../guides/keyboard-shortcuts.md`（**3 列化** = 操作 / macOS / Windows・Linux）/
  `platform/support.rs` の最終確認
- 依存: **1〜7 のすべて**（表が実態とずれると system prompt に誤情報が流れる。#516）
- `keyboard-shortcuts.md` は main が 2 列のまま行を約 47 追加しているので、
  **3 列へ寄せて main の行を移植する**作業が要る。
  `windows/467-main-merge-wip` にこの 7 hunk が未解決のまま残してあるので材料になる

### 9. スリープ防止 + 蓋閉じ継続 + ポート検知（#524 / #697 / #724）

- 持ち込む新規: `crates/tako-control/src/platform/{lid,power}.rs` /
  `crates/tako-control/tests/lid_residual_windows.rs`
- 編集: `sleep_guard.rs` / `crates/tako-core/src/ports.rs`（`pane_key()` 経由の判定）
- 依存: **1**
- **WIP が保全ブランチにある**: `windows/724-port-crash`（`7633d8b`。ポート検知のクラッシュ修正）/
  `windows/727-sleep-settings`（`91cc13f`。設定 UI）

### 持ち込まないもの（今回の裁定で確定）

| 対象 | 理由 |
|---|---|
| `crates/tako-app/src/launch_assurance.rs` / `crates/tako-control/src/orchestrator/launch.rs` / registry の `launch` フィールド / `launch-status` | #665。main の復旧サブシステム上に再設計してから別途 |
| `crates/tako-control/src/dialog.rs`（win467 版）/ `crates/tako-core/src/keys.rs` / protocol の `answers` / `dry_run` | #662。main の #748 で代替済み |
| `crates/tako-control/src/orchestrator/accounts.rs` / `tako account` CLI | #709。main の `tako orchestrator accounts`（#504 / #548）で代替済み |
| win467 の `mcp.rs` 単一ファイル | main の #750 でモジュール分割済み。Windows 由来の MCP ツールは `mcp/catalog.rs` へ個別に足す |
| `supervisor.rs` の win467 版（イベントストリーム設計） | #665 と同じ扱い。main の復旧サブシステムが正 |

### 各スライス共通の作業手順

1. `git worktree add ~/dev/tako-wt-<slice> -b windows/<issue>-<slug> origin/main`
2. 対象ファイルを `origin/windows/467-ipc-orchestration-local` から持ち込む
   （`git checkout origin/windows/467-... -- <path>` で始めて、main の現行 API に合わせて直す）
3. Mac でゲート: `cargo fmt --all --check` / `cargo clippy --workspace --all-targets -- -D warnings`
   / `cargo test --workspace` / `scripts/check-windows.sh`（エラー 0）
4. PR → **macOS CI 緑**（Windows ジョブのテストは #583 の既知失敗があるため
   `continue-on-error` のまま。合格条件は macOS 全ジョブ緑）
5. Windows 実機（`ssh win`）で該当機能を実測。既定 target が使えるようになったので
   `cargo build --workspace` / `cargo test --workspace --no-fail-fast` をそのまま回せる
6. squash merge → `--delete-branch`

### 落とし穴（実測済み）

- **Windows 機からは push できない**（`gh` トークン無効 + GCM が SSH セッションで
  wincredman に触れない）。成果物は Mac 側で commit / push する。どうしても Windows で
  作ったものを持ち出すなら `git bundle` を作って `scp` で Mac へ運ぶ
- Windows の `[Console]::OutputEncoding` は shift_jis なので、SSH 経由で git の
  UTF-8 出力を読むと文字化けする（**表示だけの問題**。コミット本体は正しい UTF-8）。
  日本語のコミットメッセージを Windows 側で作るときは UTF-8 バイトを直接書いた
  ファイルを `git commit -F` で渡す
- `#583` の既知失敗は 2026-08-21 時点で「12 件解消 / 6 件継続 / 新規 11 件 / psmux e2e 8 件」。
  スライスごとに Windows のテスト結果を #583 と突き合わせて増減を書き残す

#### Mac 側の git remote 操作が信用できないことがある（2026-08-21 実測）

このセッションでは以下が繰り返し起きた。**「push が失敗した」と読み取って同じ push を
何度もやり直す・force push に手を伸ばす、という誤動作を避けるため必ず頭に入れておく。**

1. **`git ls-remote` が古い値を返す**。push が成功しているのに、直後の
   `git ls-remote --heads origin refs/heads/<branch>` が**更新前の SHA を返し続けた**
   （4 ブランチ中 1 本だけ反映されたように見えて、実際は 1 本だけ本当に反映されていた回と、
   全部反映済みなのに全部未反映に見えた回の両方があった）。`git fetch` を挟んでも
   remote-tracking ref が追随しないことがある
2. **`git push` が無応答のまま返る / 出力が空になる**。`git-credential-osxkeychain` や
   `git-remote-https` が **signal 9 で死ぬ**ことがあり、そのとき stderr は
   `died of signal 9` か、**何も出ないまま exit 0** になる。バックグラウンド実行だと
   ログファイルが 0 バイトで終わる
3. 結果として「push は成功しているのに失敗に見える」状態になる。この状態で押し直すと
   `cannot lock ref ...: is at <新 SHA> but expected <旧 SHA>` が返る。
   **これは衝突ではなく「もう入っている」という意味**なので、force push してはいけない

**確認は GitHub API で行う**（ここだけは常に正しい値を返した）:

```sh
gh api repos/takushio2525/tako/git/ref/heads/<branch> -q .object.sha
# ファイル内容まで見るなら（ref の / は %2F でエスケープする）
gh api 'repos/takushio2525/tako/contents/<path>?ref=windows%2F467-...' -q .content | base64 -d
```

- push は**前景で長めのタイムアウト**を取る方が結果が読める（バックグラウンドだと
  無応答のまま完了扱いになりログが空になる）
- 巨大なマージコミットの push は 1 回目で落ちても 2 回目で通ることがある。
  ただし**押す前に上記 API で「もう入っていないか」を必ず見る**

#### zsh の履歴修飾子で refspec が壊れる

`git push origin "$C:refs/heads/$B"` は zsh では `$C:r`（root 修飾子）と解釈され、
`...2aefs/heads/...` のような壊れた refspec になって
`src refspec ... does not match any` で落ちる。このセッションで 2 回踏んだ。
**`"${C}:refs/heads/${B}"` と必ず波括弧で囲む。**
