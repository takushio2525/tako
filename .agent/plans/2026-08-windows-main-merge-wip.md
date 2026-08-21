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

### 1. `platform/` 境界（基盤）— ✅ **完了**（2026-08-21。PR #845 / `windows/467-slice1-platform`）

- 持ち込む新規: `crates/tako-core/src/platform/{console,exe,font,ime,install_info,locale,process,procinfo}.rs`
  / `crates/tako-app/src/platform/{mod.rs,pdf/{mod,macos,windows}.rs}`
- 編集: `crates/tako-core/src/platform/mod.rs`（mod 宣言）/ `platform/support.rs`（マトリクス）
- 呼び出し側の `cfg` 除去（#522 の `os_integration` 集約と同じ作法）
- **main 側に既にあるもの**: `platform/{clock,quit_signal,release_assets,shell,support}.rs`。
  重複させない
- 依存: なし
- 検証の効く場所: `support.rs` のパリティテスト T1〜T6 が macOS 上で走る。
  番犬テスト「OS 連携の直呼びが境界の外に残っていない」も同様

#### 完了記録（2026-08-21）

24 ファイル / +5,197 / −961。移植元は `windows/467-main-merge-wip`（`mod.rs` と
`support.rs` は合成済みの側を材料にした）。

**持ち込んだもの**: 境界 8 本（`console` / `exe` / `font` / `ime` / `install_info` /
`locale` / `process` / `procinfo`）+ tako-app 側の `platform/pdf/{mod,macos,windows}.rs`。
8 本とも **`crate::` を一切参照しない自己完結**で、Windows 実装は raw FFI
（kernel32 / imm32 / advapi32 / iphlpapi）なので **tako-core への新規依存はゼロ**。

**呼び出し側を寄せた分**（macOS は挙動不変）: `theme.rs` の `font_family`（境界の macOS
実装が `"Menlo"` を返す = 同値）/ `i18n.rs` の `macos_preferred_language`（優先順と
「環境変数で決まったら OS へ問い合わせない」遅延を純関数 `detect_with` でテスト固定）/
`tako-cli/setup.rs` の `find_command`（境界の unix 実装が同じログインシェル経路）。

**PDF（B12）**: `preview.rs` のインライン `mod pdf_render`（743 行）を境界へ移設。
macOS 実装は**移設のみで論理不変**（正規化差分は `use` の移動と dedent 後の rustfmt 改行だけ）。
`cfg` 2 分岐は `capabilities()` 照会 1 本になった。`ui_text` の `pdf_macos_only` は
`pdf_unsupported_platform` へ改称（対応 OS が増えても文言が腐らない）。
Windows 実装のためだけに `windows`（`Windows.Data.Pdf`）と `lopdf` を
`[target.'cfg(windows)'.dependencies]` で追加 = macOS のビルドグラフには載らない。

**マトリクスは 1 件も動かしていない**。境界を敷いただけで通しで動くのはスライス 2 以降。
ここで Supported へ倒すと `PlatformFacts` 経由で system prompt に誤情報が流れる（#516）。

##### 実測

| ゲート | 結果 |
|---|---|
| `cargo fmt --all --check` | 緑 |
| `cargo clippy --workspace --all-targets -- -D warnings`（feature 有無とも） | 緑・警告 0 |
| `cargo build --workspace --all-targets` | 緑（3m46s） |
| `cargo test --workspace` | **2134 passed / 0 failed**（main 基準 2070 → +64） |
| 隔離セルフテスト | `TAKO_APP_SELF_TEST_OK`（FAILED 0。SKIP 3 = 63 / 76d / 104 はウィンドウ非表示の既知条件） |
| visual-test 全節 | `TAKO_VISUAL_TEST_OK`・**98 checkpoint**（記録済みベースラインと同数） |
| `scripts/check-windows.sh` | **エラー 0 / 警告 11**（ベースライン 16 から減少） |
| CI | macOS / Windows / Pages **全 pass** |

クロスチェックの警告が 16 → 11 に減ったのは、main では `pdf_render` が macOS 限定で
`PdfCharBox` / `PdfTextLine` 等が Windows ビルドで構築されず dead_code 警告になっていたのが、
Windows 実装の追加で使われるようになったため。

**Windows 実機**: ビルド成功（7m16s）。`cargo test --no-fail-fast` は
`tako-app` 408 / **0 failed**、`tako-cli` 53 / **0 failed**、`tako-control` 939 / 24 failed、
`tako-core` 626 / 5 failed、`platform_parity` **10 / 0**。
失敗 29 件は**すべて main 由来**で、内訳は #583 の既知 18 件 + #583 計測（2026-07-27）以降に
main へ増えた同系 6 件（`acceptance_gates` 4 = #244 / `config_share::env` 1 = #513 /
`stale_binary` 1 = #772）+ `tako-core` 5 件（#583 は fail-fast で tako-core が
**一度も実行されていなかった**ため未記録。`--no-fail-fast` で初めて可視化）。

##### 副産物: 境界の番犬が Windows で必ず落ちていたのを根治

`os連携の直呼びが境界の外に残っていない`（#522）は許可リストを `/` 区切りで持つのに
`strip_prefix` は Windows で `\` を返すため、許可が 1 件も一致せず**境界の実装本体
（`platform/os_integration.rs`）自身が違反として報告されて**いた。
Windows 実機で **9 passed / 1 failed → 10 passed / 0 failed** を実測。
#515 の「macOS 上から Windows 側を検証できる」前提が Windows 側で崩れていたので基盤で直した。

##### 次スライスへの申し送り

1. **`exe::find` へ寄せていない直呼びが 3 件残る**: `tako-core/lib.rs` の `resolve_bin` /
   `tako-control/config_share/env.rs` の `find_gh` / `tako-app/preview.rs` のメディアバイナリ解決。
   win467 も未着手（`setup.rs` だけ寄せてある）。寄せると **Windows の解決挙動が変わる**ので、
   tmux / gh / ffmpeg を持つ各スライスで検証つきに寄せる
2. **境界は置いたが配線は各スライスの仕事**: `console` → スライス 2 / `ime` `font` `process`
   → スライス 4 / `install_info` → スライス 6 / `procinfo` → スライス 9。
   tako-core は lib なので `pub` API は dead_code にならず、置いただけでも緑のまま
3. **`support.rs` の `縮退理由の一覧は重複しない` は WIP 版を持ち込まないこと**。
   WIP 版は macOS 側にも縮退がある前提（#657 の in-window メニューバー）になっているので、
   #657 を入れるスライス 5 と同時でないと落ちる
4. **パリティテストのキー直書きは撤去済み**（`any_pending_on_windows()`）。
   git 対応スライスが `tako_git_log` を Supported にしても落ちない
5. **PDF テストは自前生成 PDF になった**（`build_test_pdf`）。Windows 実装の検証は
   `能力表は矛盾しない` / `どのプラットフォームでも同じ api が生えている` /
   `取れない付加情報は空で返る` の 3 本が両 OS で同じことを見る

##### 環境メモ（実測）

- 兄弟セッションが同じワークスペースを並行ビルドしていると **swap が枯れて `cc` が
  SIGKILL される**（実測: swap 27.5/28.7 GB・空きメモリ 14% で `linking with cc failed:
  signal: 9`）。`-j 2`（および内側の `cargo build` にも効く `CARGO_BUILD_JOBS=2`）で回避できた
- セルフテストの `#680: リンク md の座標キャッシュ生成` は **load 依存で落ちる**
  （load 11.1 で失敗 → 7.1 で成功）。`wait_for_preview_maps` は paint 由来の
  座標マップを待つので、負荷が高いと 80×50ms の上限に収まらない

### 2. 永続化バックエンド（psmux / ConPTY。#518 / #519）— ✅ **完了**（2a: PR #848 / 2b: PR #849）

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

#### 2a 完了記録（2026-08-21。PR #848 / `windows/467-slice2-psmux`）

9 ファイル / +3,714 / −60。**器（psmux）とその抽象**までを入れ、
**ConPTY の外側 PTY の文字コード（#655 / #659）は 2b へ送った**。

##### 入れたもの

- `backend/psmux.rs`（1,214 行）: 器の起動・列挙・kill・orphan 判定・`capture-pane` 採取。
  **スライス 1 の `platform::process::no_console_window` を配線**（GUI プロセスから
  psmux を起こすとコンソール窓が明滅するため）
- `backend/owner.rs`（359 行）: 器のオーナー記録。psmux は tmux の `#{client_pid}` に
  相当するものを観測できないので、「どの tako-app が握っているか」を tako 側で持つ
- `tests/psmux_backend.rs`: 実バイナリでの適合検証。**器の内側のコードページ固定
  （`pin_container_encoding` → `platform::console`）はここに含む**（器と不可分）

##### 到達手段を「採取」と「送出」に分けた

psmux は `capture-pane` が動く一方で送出が信頼できない。`DetachedAccess` 1 本のままだと
「送れないから読めもしない」に倒れ、**psmux で読める画面まで塞がる**。

- `DetachedCapture`（読み）を分離。`detached_capture()` の既定実装が `detached()` から
  引き上げるので、**送出できる器（tmux）の呼び出し側は 1 行も変わらない**
- 採取しかしない **5 経路**を capture 側へ（`Request::Read` の detached / `capture_scrollback_joined` /
  `finish_worker_status` / `apply_worker_status_corrections` / `send_target_screen`）。
  送出する **3 経路**（`Send` の 2 箇所 + `respond_to_choice_dialog`）は `detached_session` のまま
- `Holder` に `HolderKind`（`Client` / `Owner`）。tmux はクライアント PID を返し呼び出し側が
  祖先を辿るが、psmux は器の実装側で生存確認済みの所有 pid を返す（#177 のガードが両方で効くように）

##### 実測

| ゲート | 結果 |
|---|---|
| fmt / clippy（feature 有無とも、`-D warnings`） | 緑・**0 findings** |
| `cargo test --workspace`（macOS） | **2192 passed / 0 failed**（スライス 1 後 2136 → +56） |
| 隔離セルフテスト | `TAKO_APP_SELF_TEST_OK`（FAILED 0） |
| `scripts/check-windows.sh` | エラー 0 / 警告 11（スライス 1 と同数） |

**Windows 実機**: `tests/psmux_backend.rs` が **14 passed / 0 failed（17.36s）**。
`skip:` 行が 1 本も出ず**全件が実際に走った**（macOS は 13 件が 0.00s = 全件スキップ）。
実 psmux 3.3.7 で器の attach 復帰・前方一致 kill の巻き込み防止・採取・cwd 往復・
器内 pid・**器の中のシェルのコードページ utf8 固定**（macOS には無い `#[cfg(windows)]` 項目）を確認。

全体は `tako-app` 409/**0** / `tako-cli` 53/**0** / `tako-control` 944/25 / `tako-core` 663/5 /
`platform_parity` 10/**0**。**スライス 1 の 29 件 → 30 件**で、増えた 1 件は
`dispatch::tests::issue822_…`（#822 が main へ入って増えたテスト）。
**`TAKO_BACKEND=none`（スライス 1 相当）でも同じ行で同じように落ちる**ことを実測したので
psmux 化が原因ではなく、既存の「spawn のコマンド組み立てが POSIX 前提」の系に属する。

→ **plan が見込んでいた「psmux e2e 8 件」は解消**（psmux 導入 + 本スライスで 14/0）。

##### 2b（残り）でやること

`tests/encoding_conpty.rs` / `poc/conpty-survival/` と、**外側 PTY** のコードページ固定。

- win467 の `terminal.rs` は `alacritty_terminal::event_loop::EventLoop` の上に書かれているが、
  **main は #817 で `pty_loop.rs` を新設して自前ループへ置き換えている**（1 MiB スタック
  バッファの排除）。win467 に `pty_loop.rs` は存在しないので**移植ではなく再実装**
- 併せて #686（器の copy mode ゲート）の `TerminalSession::wheel_scrolled_back` /
  `arm_copy_mode_exit` を実装する。器側の契約（`pane_in_mode` / `copy_mode_exit_bytes`）は
  2a に入っているので、消費側だけ。**2a では `psmux_backend.rs` の該当 2 本を外して送った**
  （ファイル末尾のコメントに明記）
- **教訓**: その 2 本は `#[cfg(windows)]` なので **macOS のゲートが全部緑でも Windows で
  E0599 になる**。実機ビルドで初めて分かったので、2b は**実機ビルドを先に通す**こと

##### 統合で判明した点（後続スライスも踏む）

`owner.rs` が `<data_dir>/backend-owners` へ書くため、#513 の共有カタログの
fail-closed 番犬が「未分類のパスがある」で落ちた。**`Local` として分類**した（共有すると
別マシンの pid を持ち主と読み、#177 のガードが誤作動する）。win467 は #513 より前に
分岐しているのでこの宣言を持っていない。**data dir へ書くものを増やすスライスは必ずここに宣言が要る**。

#### 2b 完了記録（2026-08-21。PR #849 / `windows/467-slice2b-conpty`）

**恐れていた「#817 の pty_loop の上への再実装」は実際には不要だった。**
#817 が置き換えたのは **PTY の読み取りループ**で、**書き込み経路（`notifier`）と
ホイール経路の形は変わっていない**。win467 の実装はそのまま載った
（読み取りループには 1 行も触っていない）。

##### 入れたもの

- **外側 PTY のコードページ固定（#655 / #659）**: `TerminalSession::spawn` で
  `platform::console::pin_pane_to_utf8_when_ready(child_pid)` を呼ぶ。
  ConPTY は OEM コードページ（日本語版 Windows は CP932）で始まるため、
  放っておくと子が吐いた UTF-8 を conhost が CP932 と解釈し **tako が受け取る前に**壊れる。
  境界の実体はスライス 1 で既に main に入っていたので、**呼ぶだけ**だった
- **`TerminalSession::child_pid()`（#592）**: pid を起動時に確定して保持・公開。
  `pty_child_pid` は `cfg(unix)` / `cfg(windows)` で alacritty の API 形の差を吸収する。
  **`platform/` へ持ち上げなかった**のは、分岐しているのが OS ではなく
  **外部クレートの型（`tty::Pty`）の API 形**で、境界へ置くと `platform/` が
  alacritty へ依存してしまうため（境界の自己完結を保つ）
- **#686 の copy mode ゲート（消費側）**: `CopyModeGate`（depth + 解除バイト列の純状態）+
  `write()` での in-band 前置 + ホイール転送 2 箇所での勘定 +
  `wheel_scrolled_back` / `arm_copy_mode_exit` / `disarm_copy_mode_exit`。
  解除を器へ別経路（`send-keys -X cancel`）で撃つと「解除が届く前に打鍵が copy mode に
  食われる」競合が残るので、**同じ書き込みの先頭へ混ぜる**
- `tests/encoding_conpty.rs` / `poc/conpty-survival/`（`poc` は workspace から exclude 済み）
- **2a で外した #686 依存の 2 本を復帰**（`psmux_backend.rs` は 1,105 行の元の姿へ）

##### 実測

macOS: fmt / clippy（feature 有無とも）**0 findings** / `test --workspace`
**2194 passed / 0 failed**（2a 後 2192 → +2 = `CopyModeGate` の単体テスト）/
隔離セルフテスト `TAKO_APP_SELF_TEST_OK` / visual-test **98 checkpoint** /
クロスチェック エラー 0・警告 11（2a と同数）。

**Windows 実機**:

| スイート | 結果 |
|---|---|
| `encoding_conpty` | **5 passed / 0 failed**（0.52s。skip なし） |
| `psmux_backend` | **16 passed / 0 failed**（19.48s。2a の 14 → #686 の 2 本が復帰） |
| `tako-app` / `tako-cli` | 409/**0** / 53/**0** |
| `tako-control` / `tako-core` | 944/25 / 663/5（**2a と同数 = 新規失敗ゼロ**） |
| `platform_parity` | **10 / 0** |

`encoding_conpty` の 5 本は「起動時に utf8 へ固定される」「utf8 を吐く子の出力が化けない」
「日本語ファイル名 / 絵文字 / 長文も化けない」「途中で cp932 へ切り替えても固定し直せば戻る」
「PowerShell 5.1 のペインでも utf8 になる」。#686 の 2 本は
「copy mode 滞在中の打鍵が in-band 解除で届く」「ホイールは上下対称で最下部で copy mode を抜ける」。

##### 効いた作法

**2a の教訓どおり Windows 実機ビルドを先に通した**。macOS では気づけない
`#[cfg(windows)]` 由来のエラーが 1 件だけ出て（`encoding_conpty` が要求する
`TerminalSession::child_pid()` が main に無い）、そこだけ直せば済んだ。
macOS のゲートを先に全部回してから実機へ行くと、この 1 件のために全部やり直しになる。

### 3. IPC named pipe（#467）— ✅ **完了**（PR #850）

- 持ち込む新規: `crates/tako-control/src/platform/named_pipe.rs`
- 編集: IPC のトランスポート選択（`ipc.rs`）
- 依存: **1**
- 独立性が高いので 2 と並行してよい

#### 完了記録（2026-08-21。PR #850 / `windows/467-slice3-namedpipe`）

4 ファイル / +371 / −92。**plan の記載より 1 ファイル多い**: `tako-cli` の
クライアント側も直さないと「pipe のサーバーはあるがクライアントが居ない」状態になる
（plan は `ipc.rs` + `named_pipe.rs` だけを挙げていた）。

##### 入れたもの

- `platform/named_pipe.rs`（224 行・境界 B3）: `\\.\pipe\tako-*` のサーバー
  インスタンス生成・クライアント接続・生存プローブ。**バイトストリームの確立だけ**を担い、
  ワイヤ形式（1 行 1 JSON）は持たない。依存クレートを増やさないため最小の FFI 宣言
- `ipc.rs`: ワイヤ処理を **`mod conn`（`<R: Read, W: Write>` のトランスポート非依存）**へ
  抽出し、`unix_imp` / `windows_imp` はストリームの作り方だけを持つ形へ。
  main の `process_line` は**バイト等価で移動**（正規化差分ゼロ）。ソケットパス選択
  （`temp_socket_path` / `preferred_socket_path` の `TAKO_SELF_TEST` 分岐と固定パス）も不変
- `tako-cli`: `mod transport` を「OS 別 `connect()` + 共通 `roundtrip_on()`」へ。
  main は `#[cfg(unix)]` / `#[cfg(windows)]` で**モジュールを 2 つ**持っていたが、
  分岐するのは接続の張り方だけなので 1 つに畳んだ

アクセス制御は named pipe の既定 DACL（作成ユーザーのフルアクセス。Everyone は read のみで
リクエストを書き込めない）+ 全リクエストのトークン検証の二段（unix の 0600 + トークンに相当）。
`GetNamedPipeClientProcessId` による PeerIdentity 検証は B3 の後続タスク。

##### 実測

macOS: fmt / clippy（feature 有無とも）**0 findings** / `test --workspace`
**2194 passed / 0 failed**（スライス 2b と同数 = 新規テストは Windows 限定）/
隔離セルフテスト `TAKO_APP_SELF_TEST_OK` / visual-test **98 checkpoint** /
クロスチェック エラー 0・**警告 10**（2b の 11 から 1 件減 = `tako-cli` の
windows スタブが持っていた未使用警告が transport 統合で消えた）。

**Windows 実機**: `ipc::windows_tests` **3 passed / 0 failed**
（`正しいトークンでリクエストが往復する` / `不正なトークンは認証エラーで拒否される` /
`連続接続が全件処理される`）。全体は `tako-app` 409/**0** / `tako-cli` 53/**0** /
`tako-control` 950/25 / `tako-core` 665/5 / `platform_parity` **10/0** /
`encoding_conpty` **5/0** / `psmux_backend` **16/0**。
**失敗は 30 件のままで新規ゼロ**（tako-control は 944→950 passed と増えただけ）。

Windows 実機ビルドは**一発で通った**（2b の教訓どおり実機を先に回した）。

### 4. 入力系: キーボード / IME / フォント / コンソール抑止（#517 / #575 / #582 / #585 / #586）— ✅ **完了**（PR #852）

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

#### 完了記録（2026-08-21。PR #852 / `windows/467-slice4-input`）

21 ファイル / +1,717 / −162（+ 追い修正 1 ファイル）。**plan の見立てより 15 ファイル多い**:
plan は 6 ファイル（`keybindings.rs` / `main.rs` / `settings_window.rs` / `autorename.rs` /
`stale_binary.rs` / `orchestrator/mod.rs`）を挙げていたが、コンソール窓抑止（#586 / #628）は
**GUI から到達する子プロセス起動が 38 箇所**あり、`dispatch.rs` / `remote.rs` /
`config_share/` / `update_checker.rs` / `tako-cli/setup.rs` / `tako-core` の
`git.rs` / `tmux.rs` / `lib.rs` にも及んだ。

##### 入れたもの

- **キーバインド（#517 / #585 / #575 / #648）**: `key_bindings()` を「共通」+
  `macos_only_bindings()` + `platform_bindings()` の 3 段へ。非 macOS へ OS 慣習の
  **45 本**を追加（macOS の 45 本は 1 本も変えない）。割当の原則 4 つは番犬テストで固定:
  `ctrl-<英字>` 単独は奪わない（例外は `ctrl-v`）/ 衝突は `ctrl-shift-` へ逃がす
  （同じ C0 バイトへ潰れるので入力手段が減らない）/ `alt-` は矢印だけ（#575 の meta を殺さない）/
  shift と記号・数字は組み合わせない（GPUI Windows の正規化と一致しない）
- **`cmd-h` / `cmd-alt-h` / `cmd-m` を macOS 限定に**。Win+Alt+H が届くと `HideOthers` →
  `gpui_windows::hide_other_apps` の `unimplemented!()` で **app ごと abort** するため
  バインドを張らないことで経路ごと塞ぐ。「非 macOS に無いこと」も逆向きに固定した
- **Alt+印字文字の meta 送出（#575）** と `printable_char_from_key`。AltGr（Windows では
  Ctrl+Alt）は ESC を前置しない
- **`is_paste_keystroke`（#546）を非 macOS の実バインドへ追随**。ずれると「入力欄に
  フォーカスがあるときだけ貼り付けが死ぬ」ので、バインド表との一致をテストで固定
- **パレットへのショートカット併記（#648）**。表示は必ず `key_bindings()` から導出する
  （手書きの一覧を別に持つと確実に食い違う）。main が持つ `open-settings` も対象に足した
- **IME（#582）**: `anchor_rect_y` / 変換ごとの `invalidate_character_coordinates()` /
  `CFS_EXCLUDE` の除外領域通知。**win467 の旧実装は戻さず**、main の
  `pane_cursor_origin_for_ime` / `cell_size_for_pane`（#781 / #803 / #787 で作り直した会計）の
  上に載せ直した
- **既定フォント（#585）**: 設定画面のプレースホルダを `platform::font` 経由へ
- **コンソール窓抑止（#586 / #628）**: 38 箇所を `platform::process::no_console_window` へ
  （未抑止の件数は実測で 118 → 80 へ減った）。
  番犬テスト `コンソール窓を抑止していない子プロセス起動が増えていない` は**件数を固定**する
  方式（ファイル単位の許可リストは「そのファイルなら何個でも増やせる」穴になる）。
  残り 21 ファイルは理由つきで表に残した

##### 落としたもの（次スライスへ）

**#623（IME の未確定文字列が勝手に確定される）は入れていない**。`platform::ime` の
`is_associated` / `reassociate` / `guard_action` はスライス 1 で main に入っており
呼び出しを足すだけだが、`guard_action` の `refocus: !focus_held` は **macOS でも発火する**
経路で、main が win467 の分岐後に足した設定ウィンドウ・アップデートウィンドウ
（別 GPUI ウィンドウ）との相互作用が未実測。「macOS 挙動不変」を崩さないため分離した。

##### 実測

macOS: fmt / clippy（`-D warnings`）**0 findings** / `test --workspace`
**2207 passed / 0 failed**（ベースライン 2194 + 新規 13 = 完全一致）/
visual-test **98 checkpoint**（数値もベースラインと一致）/
クロスチェック **エラー 0・警告 10**（`origin/main` を同条件で実測し**同一の 10 件**）。

**Windows 実機**: `cargo build --workspace` は**一発で通った**（11m37s / エラー 0）。
`cargo test --workspace --no-fail-fast` の結果:

| スイート | ベースライン | 実測 |
|---|---|---|
| `tako-app` | 409 / 0 | 426 / 1 → **修正後 427 / 0**（実機で再確認） |
| `tako-cli` | 53 / 0 | 53 / 0 |
| `tako-control` (lib) | 950 / 25 | **950 / 25**（完全一致） |
| `tako-core` (lib) | 665 / 5 | **665 / 5**（完全一致） |
| `platform_parity` | 10 / 0 | **11 / 0**（番犬 1 本新設） |
| `encoding_conpty` | 5 / 0 | 5 / 0 |
| `psmux_backend` | 16 / 0 | 16 / 0 |

失敗 31 件のうち **30 件は #583 の既知（POSIX 前提）**で完全一致。残り 1 件は
`app_menu_tests::macos慣習のショートカットがバインドされている` = **自分が作った回帰**で、
`cmd-h` / `cmd-alt-h` / `cmd-m` を macOS 限定にしたのにテストが無条件で存在を要求していた。
テストを両方向へ拡張して解消（`d99f70a`）。**macOS のゲートは全部緑だったのに実機で
落ちた** = 引き継ぎ 1 か条（実機ファースト）がそのまま効いた事例。

隔離セルフテストは**完走しない**。切り分け結果:

- 「ANSI 赤の解決」= 負荷依存フレーク（load 10.2 で落ち、load 5.8 では通過）。#796 の系列
- 「MCP tako_chat_copy (#725)」= **main 由来の決定的失敗**。素の `origin/main`（`e947524`）を
  `~/dev` 配下の別 worktree で同条件に走らせ、**同一の診断値**
  （`md_has_fence=false` / `code=` が本文全体）で落ちることを確認した。負荷は無関係
  （load 4.60 でも 10.50 でも同一）。**→ #853 に起票**。この項目が後半にあるため
  以降の項目が走らない状態なので、次スライスの検証でも同じところで止まる

### 5. ウィンドウコントロール + in-window メニューバー（#584 / #657）— ✅ **完了**（PR #860）

- 持ち込む新規: `crates/tako-app/src/menu_bar.rs` /
  `assets/icons/ui/window_{maximize,restore}.svg`
- 編集: `main.rs` / `tab_bar.rs` / dispatch の `WindowState`
- 依存: **1**, **4**（メニューのアクセラレータがキーバインドの分類に乗る）
- 注意: アイコンは `EMBEDDED_ASSETS` への登録漏れ検査テストがあるので必ず登録する（#561 の副産物）

#### 完了記録（2026-08-21。PR #860 / `windows/467-slice5-window-menu`）

18 ファイル / +2,610 / −92。**実機で tako GUI が初めて立った回**。

##### 入れたもの

- `menu_bar.rs` 新設（メニュートリガー + その場展開サブメニュー + ドロップダウン +
  自前ウィンドウコントロール）。行を出すかは `MENU_BAR_HEIGHT`（cfg で 0 / 30）で決めるが
  **関数本体は両 OS でコンパイルする**（`#[cfg]` で消すと macOS 側が一切通らず実機でしか壊れに気づけない）
- `top_chrome_height()` 新設。ペイン矩形・IME アンカー・境界ドラッグの起点を 1 か所へ。
  #684 の番犬テストの式（`viewport.height - px(...)` の出現回数 1）もこれに追随させた
- タブバー上の対話要素すべてに `.occlude()`（#576）。`tab-scroll-area` には**付けない**
- `app_menus()` をヘルパー分割し、非 macOS は Explorer 慣習（アプリ名メニュー無し）へ。
  `HideOthers` / `ShowAllApps` / `HideApp` は番犬テストで**構造的に排除**
- 3 経路 1:1（`tako window minimize|maximize|restore` / `tako menu` 4 種 /
  MCP `tako_window` +3 action と `tako_menu` 新設 = **135 ツール**）

##### plan の見立てとの差 2 件

1. **`tako_menu` の macOS は `Supported` にした**（win467 は `Degraded`）。main には `Degraded` の
   エントリが 1 件も無く、倒すと `PlatformFacts` 経由で **macOS の system prompt** に縮退注記が入り、
   `known_limitations_markdown`（#594）経由で **macOS のリリースノートに「既知の制限」節が生える**
   （実際に `縮退一覧はマトリクスから生成される` と
   `known_limitations_is_empty_when_nothing_degraded` の 2 テストが落ちた）。
   macOS はメニューバーがネイティブで完全に動くので `Degraded` は実態より重い。
   使えない `open` / `close` は `require_in_window_menu` が理由と代替を名指しで返す。
   **後続スライスも「マトリクスを倒す前に #594 のリリースノートへの波及を見る」こと**
2. **main の menu テスト 3 本が macOS 決め打ち**で、Windows では
   `menus[0].name == "tako"` を要求して落ちる。win467 のプラットフォーム別版
   （`#[cfg(target_os = "macos")]` + `windowsのメニュー構成はexplorer慣習の並び`）へ差し替えた。
   引き継ぎ 1 か条（実機ファースト）を守っていなければ見逃していた類

##### 同梱: main 由来バグ（`tako` CLI が Windows で起動できない）

`Cli::parse()` の構築だけで Windows の既定 1MB スタックを溢れ、debug ビルドの `tako.exe` が
**どのサブコマンドでも** `thread 'main' has overflowed its stack` で落ちていた。
実機 A/B（`crates/tako-cli/src/main.rs` だけ `origin/main` へ差し替えて再ビルド）で
**main 由来と確定**。スライス 3 の IPC 検証はユニットテストだったため実バイナリのこの経路を
踏んでおらず、**Windows では CLI が丸ごと使えない状態**だった。16MB スタックのワーカースレッドへ移して解消。
→ **以後のスライスは「実バイナリの CLI を 1 回叩く」を検証に入れる**

##### 実測

macOS: fmt / clippy（`-D warnings`、visual-test feature 有無とも）**0 findings** /
`test --workspace` **2228 passed / 0 failed**（ベースライン 2217 + 新規 11）/
クロスチェック **エラー 0・警告 10**（`origin/main` を同条件で実測し**同一の 10 件**）。

**Windows 実機**: `cargo build --workspace` は**一発で通った**（8m03s）。
`cargo test --workspace --no-fail-fast` は **失敗 30 件 = ベースライン完全一致・新規ゼロ**
（tako-app 440/0・tako-cli 53/0・tako-control 955/**25**・tako-core 668/**5**・
platform_parity 11/0・encoding_conpty 5/0・psmux_backend 16/0）。

**実機 GUI**（`schtasks /it` で session 1 へ起動 → 実マウス / 実キーで操作）:
`Zed::Window` / `title=[tako]` / メニュー行 `ファイル 編集 表示 ウインドウ ヘルプ` + `─ □ ✕` を描画。
実マウスでメニュートリガー・タブバーの `+`・最小化・最大化・復元・閉じるがすべて成立（#576 の
`HTCAPTION` に食われない）。F10 / ← → / Esc のキーボード操作、ホバー切替、`menu invoke`、
最小化中の `tako window restore`（render 停止中でも IPC ハンドラの sync が復帰経路）、
アイコンの出し分け（`⧉` ⇄ `□` を 5 倍拡大で目視）まで確認。
**CLI（named pipe）と MCP（stdio ブリッジ）が実 GUI に対して初めて通った**。
証拠は `~/dev/tako-evidence/467-s5/`。

##### 実機セルフテストは項目 2 で止まる（スライス 7 待ち）

`TAKO_SELF_TEST=1` を実機で回すと **`TERM / COLORTERM 注入` で FAILED** し、以降が走らない
（シェル統合 = #525 = スライス 7 の領域）。項目 118（メニューバー）は手動の実 GUI 操作で
代替検証した。**スライス 7 完了後に実機セルフテストを通しで回すこと**。

##### 別 Issue へ切り出し

**#861**: 336 logical px 以下の極端に狭い幅でメニュー行がウィンドウコントロールと重なる
（`menu_bar_triggers` が `WINDOW_CONTROLS_PX` を差し引かない。タブバー側は引いている非対称）。
実用外の幅・macOS 影響なしのため低優先。

### 6. インストーラー + リリース（#587 / #723）— ✅ **完了**（PR #851）

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

#### 完了記録（2026-08-21。PR #851 / `windows/467-slice6-installer`）

19 ファイル / +1,697 / −34。plan の見立てとの差は 2 点:

1. **アセット名は win467 のままでは持ち込めなかった**。win467 は
   `tako-setup-<tag>-x64.exe` / `tako-<tag>-windows-x64.zip` を出すが、main の正
   （`platform::release_assets`）は `tako-<tag>-windows-x86_64.{exe,zip}`。
   そのまま入れると `tako update` が自 OS 向けアセットを掴めない = **#595 の事故そのもの**。
   plan は「main が既に Windows アセットを扱える」とだけ書いていて、
   **リリース側（PowerShell）を寄せる作業が要ることに触れていなかった**
2. **`-win.N` の版数意味論もこのスライスに含める必要があった**。plan は
   `effective_current_version()` の追加だけを挙げていたが、main の
   `ParsedVersion::parse("0.5.13-win.3")` は `None` を返す（`-test.` 以外の
   プレリリースを弾く）ため、**インストーラーの記録が「読めない値」として捨てられ
   `effective_current_version()` が死んだコードになる**。`win_num` と
   プラットフォーム考慮の比較まで入れて初めて #723 が成立する

##### 入れたもの

- `installer/windows/lib/release-assets.ps1`（95 行・**新設**）: 命名規則の
  **PowerShell 側の写し**。Inno Setup の `OutputBaseFilename`（`/DAssetBaseName=` で注入）も
  zip 名もここから組む。`tako.iss` には名前を書き下さない（ISCC を手で叩いたときだけ
  使うフォールバックだけ `#ifndef` で持つ）
- `release_assets.rs` に同期テスト 2 本。`powershell_mirror_declares_same_constants` は
  **pwsh 不要**（ファイルを読んで定数を突き合わせる）なのでどの環境でもドリフトを検出し、
  `powershell_mirror_generates_identical_names` は pwsh があれば実行して生成結果を比べる
  （macOS ランナー / Windows には pwsh がある。無い環境ではスキップ）
- `build.rs` ×2: アイコン / バージョン情報リソースの埋め込み + `TAKO_FULL_VERSION` の emit。
  **ターゲット（`CARGO_CFG_TARGET_OS`）とホスト（`cfg!(windows)`）の二重ガード**で
  macOS のクロス検査を落とさない。アイコングループの ID は 1 固定（gpui が
  `LoadImageW` で自モジュールの ID 1 を引くため。変えるとタスクバーだけ既定アイコンへ戻る）
- `.cargo/config.toml`: MSVC の `+crt-static`。`[target.x86_64-pc-windows-msvc]` だけなので
  **macOS のビルドには構造的に影響しない**（`grep -E "^\["` で節が 1 つだけであることを確認済み）
- `update_checker.rs`: `effective_current_version()`（`OnceLock` で 1 度だけ解決 =
  描画から毎フレーム レジストリを引かない）/ `resolve_current_version()`（純関数）/
  `ParsedVersion::win_num` / `suffix_rank()` / `is_newer_release(platform)`。
  表示 5 箇所（`update_window` / `about_window` ×2 / 「更新なし」メッセージ / status JSON）を
  effective 経由へ。`CURRENT_VERSION` の直接参照は **HTTP の User-Agent だけ**に絞った

##### macOS の判定を変えていないことの担保

`suffix_rank()` は `-win.N` を持たない版に対して旧 `Ord`（stable > test、test 同士は番号順）と
**完全に同じ順序**を返す。`is_newer_release` も `(None, None)` では `suffix_rank` 比較に落ちるので
既存データでは挙動がビット等価。実リリース 28 件のスナップショットテスト
（`test_real_releases_macos_judgement_identical_to_before_595`）が緑のまま。

##### 実測

macOS: `fmt --all --check` 緑 / `clippy --workspace --all-targets -- -D warnings`
**0 findings**（`--features visual-test` も）/ `test --workspace`
**2204 passed / 0 failed**（スライス 3 の 2194 + **新規 10 本**）/
クロスチェック **エラー 0・警告 10**（ベースライン同数。内訳も同一 =
`video_player` 7 + `tako-control` 3 で、いずれも既存）/
`scripts/release.sh --notes-only` の生成物が不変（macOS のダウンロード表・手順とも）。

**Windows 実機で配布物を実際に作った**（ISCC は
`%LOCALAPPDATA%\Programs\Inno Setup 6\ISCC.exe` に導入済み）:

```
pwsh -File installer/windows/build-installer.ps1 -Version v0.7.4-win.1
  → dist/windows/tako-v0.7.4-win.1-windows-x86_64.exe   16,805,628 bytes
  → dist/windows/tako-v0.7.4-win.1-windows-x86_64.zip   22,277,860 bytes
```

- **名前が main の命名規則そのもの**（win467 なら `tako-setup-v0.7.4-win.1-x64.exe`）。
  実成果物 2 件を `release_assets` に食わせて `tag=v0.7.4-win.1 / Windows / X86_64` に解け、
  `select(Windows, X86_64)` が `.exe` を、`select(MacOs, Arm64)` が `None` を返すことまで確認
- **`#723` の連鎖が実バイナリで成立**: `tako-app.exe` に `0.7.4-win.1` が焼けている
  （タグ → `TAKO_WIN_NUM` → build.rs → `TAKO_FULL_VERSION`）。FileVersion は `0.7.4`、
  `OriginalFilename` は `tako-app.exe` / **`tako.exe`**（winresource の既定を意図的に上書き）
- **crt-static が効いている**: `dumpbin /DEPENDENTS` に `VCRUNTIME` / `api-ms-win-crt` の
  import なし = 素の Windows で VC++ 再頒布可能パッケージ不要
- zip の中身は `tako/{tako-app.exe, tako.exe, tako.ico, LICENSE.txt}`（GUI と CLI が同階層。
  `resolve_tako_binary()` が兄弟として `tako.exe` を引くので必須）
- **ガードが実際に落ちる**: `/DAssetBaseName` 無しの ISCC は
  `Error on line 43 ... AssetBaseName undefined` で **exit 2 / Compile aborted**。
  `release-windows.ps1` は `gh` 未認証 + HEAD≠タグ を検出してビルド前に exit 1

**Windows 実機のゲート**（`cargo build --workspace` は**一発で通った** = exit 0 / 9m07s。
crt-static と build.rs 2 本を含む）:

| スイート | 結果 | ベースライン |
|---|---|---|
| `tako-app` | 416 / **0** | 409 / 0（**+7** = #723 の新規テスト） |
| `tako-cli` | 53 / **0** | 53 / 0 |
| `tako-control` (lib) | 950 / 25 failed | 950 / 25 failed |
| `tako-core` (lib) | 667 / 5 failed | 665 / 5（**+2** = PowerShell 同期テスト） |
| `platform_parity` | 10 / 0 | 10 / 0 |
| `encoding_conpty` | 5 / 0 | 5 / 0 |
| `psmux_backend` | 16 / 0 | 16 / 0 |

**失敗は 30 件のままで新規ゼロ**。失敗名も突き合わせ済みで全部 POSIX 前提
（`execute_command_*` / `symlink` / `tilde` / `pidpath` / `links::` / spawn のコマンド組み立て）=
#583 の既知パターン。`release_assets` は Windows でも **11/11 緑**
（`shell_mirror_generates_identical_names` だけ `#[cfg(unix)]` で除外されるので macOS の 12 と 1 本差）。

##### 検出力（壊して落ちることを実測）

| 壊し方 | 落ちたテスト |
|---|---|
| PS 写しの arch を `x64` へ | `powershell_mirror_declares_same_constants` |
| PS 写しの名前フォーマットを壊す | 上記 + `powershell_mirror_generates_identical_names` |
| `is_newer_release` の Windows 補正を外す | `installed_win3_sees_no_update_for_win3` |
| インストーラー記録を無視する | 上記 + `resolve_current_version_prefers_installer_record` |
| `-win.N` の解析をやめる（main の元実装） | 上記 + `parsed_version_reads_win_suffix` ほか計 4 本 |
| `.iss` がアセット名を組み立て直す | `inno_setup_does_not_build_asset_names_itself`（行番号つきで名指し） |
| `OutputBaseFilename` を win467 の直書きへ戻す | 同上 |

`build.rs` が消えた場合は `env!("TAKO_FULL_VERSION")` が**コンパイルエラー**になるので、
テストより強く縛られている。

##### 次スライスへの申し送り

- **対応マトリクスは触っていない**（作法 4）。#587 / #723 を Supported へ倒すのはスライス 8
- `installer/windows/lib/` に PowerShell の共有部品を置く場所ができた。Windows 側の
  リリース補助を足すときはここへ（`scripts/lib/` の対応物）
- `make-icon.ps1` は `.ico` がコミット済みなので通常は走らない。`System.Drawing` 依存で
  **Windows 専用**なので、macOS からは呼べない（`build-installer.ps1` は `.ico` が
  在ることを前提に飛ばす）
- `release-windows.ps1` は **gh の認証を前検査で要求する**。Windows 機の `gh` トークンは
  無効なまま（落とし穴節のとおり）なので、実際のアップロードは
  **Mac 側で `gh release upload` する**か Windows の `gh auth login` を通す必要がある

### 7. シェル統合 PowerShell（#525）— ✅ **完了**（PR #855）

- 持ち込む新規: `crates/tako-core/shell-integration/tako.ps1` /
  `crates/tako-core/tests/shell_integration_powershell.rs`
- 編集: `shell_integration.rs` / `osc_tap.rs`（`file:///C:/...` の先頭 `/` 落とし）/
  `platform/support.rs` / `resources/setup/changes.yaml`
- 依存: **1**, **2**（器越しの OSC パススルー）
- **WIP が保全ブランチにある**: `windows/525-shell-integration`（`f58a994`）。未完成なので
  完成させるところから
- `crates/tako-core/src/shell_send.rs`（#640）はこのスライスに含める
  （resume 注入・起動コマンド投入が送達確認経路を通るため）
  → **7 では入れなかった**。OSC を出す話と、ペインへコマンドを送り込む話は
  どちらも他方の前提になっていない（分けても片方が壊れない）ので、
  1 PR = 1 まとまりを保つために外した。**#640 はスライス 7b として残っている**

#### 完了記録（2026-08-21。PR #855 / `windows/467-slice7-shellint`）

23 ファイル / +2,190 / −60。plan の見立てとの差が 3 件あった。

##### 1. 移植元が plan の指定と違う（**共通手順が通らない**）

plan の共通手順は「対象ファイルを `origin/windows/467-ipc-orchestration-local` から
持ち込む」だが、**`tako.ps1` と `shell_integration_powershell.rs` は win467 に存在しない**。
在ったのは保全ブランチ `windows/525-shell-integration` だけ。

しかもその WIP は **#600 / #614 / #816 / #513 より前**の main から分岐しているので、
ファイルをそのまま `git checkout` すると次を巻き戻す:

| 巻き戻るもの | WIP が実際にやっていること |
|---|---|
| #600 / #614（入力予測） | `shell_integration.rs` から zsh-autosuggestions 一式（約 250 行）を削除 |
| #816（Ground 読み飛ばし） | `osc_tap.rs` の `scan()` を 1 バイト送りへ戻し、同一性テストも削除 |
| #513（設定共有） | `changes.yaml` の revision 13 / 14 を別内容へ振り直し |

**後続スライスも保全ブランチを使うときは同じ確認が要る**（`git diff origin/main..<WIP>` を
機能単位で読み、持ち込む差分だけを選ぶ）。

##### 2. 器の能力に `osc_passthrough` を足す必要があった

plan は編集対象に `platform/support.rs` を挙げていたが、**実際に足りなかったのは
`BackendCapabilities`** のほう。前任が実測していた「psmux 3.3.7 は
allow-passthrough 相当を受理するが素通ししない」を型で表す場所が main に無く、
`Status::effective()` が書けなかった。判定は器に尋ねる 1 箇所（`backend_block()`）だけ。

##### 3. `platforms:` 機構の最初の実使用

`changes.yaml` の `platforms:`（スライス 1 で入った）はこれまで**使われていなかった**。
revision 15 で初めて使ったので、既存テスト
`platforms省略は全プラットフォーム対象`（「全エントリが未指定」を前提にしていた）が落ちた。
**縛るべき不変条件は「未指定のものは両方に出る」**なので、そこを見る形へ書き換えた。

##### 入れたもの

- `shell-integration/tako.ps1`（189 行）: PSReadLine の `PSConsoleHostReadLine` を包んで
  **実際の送信**を捉える（コマンド探索フックを主経路にできない理由は WIP のコメントに
  実測が残っていた = PSReadLine 自身の探索で誤爆する）。不在時は
  `PreCommandLookupAction` へ落ちる。`$?` / `$LASTEXITCODE` はユーザーの prompt へそのまま渡す
- `shell_integration.rs`（591 → 899 行）: 境界 B13 を `mod imp` へ分離。配置は
  **バイト列のまま**切った貼ったするので CP932 のプロファイルを壊さない。
  `$PROFILE` は PowerShell 自身に**16 進で**尋ねる（OneDrive のリダイレクトで
  決め打ちが外れる + 5.1 のリダイレクト出力は OEM コードページ）
- `osc_tap.rs`: `strip_drive_slash`（`file:///C:/…` の先頭 `/` 落とし）だけを追加
- CLI `tako shell-integration` + MCP `tako_shell_integration`（**135 ツール**）+
  dispatch を `tako_control::shell_integration::run` の 1 実装で共有
- `changes.yaml` revision 15（`platforms: ["windows"]`）

##### 実測（macOS）

`fmt` / `clippy --all-targets -- -D warnings` **0 findings**（`--features visual-test` も）/
`test --workspace` **2223 passed / 0 failed**（スライス 6 の 2204 + **新規 19 本**）/
隔離セルフテスト `TAKO_APP_SELF_TEST_OK` / クロスチェック **エラー 0・警告 10**
（ベースライン同数・内訳同一）。

**`scripts/check-windows.sh --all-targets` も エラー 0**。素の
`check-windows.sh` は `--all-targets` を付けないので **Windows 専用の
integration test（`#![cfg(windows)]`）が型検査されない**。作法 1 を前倒しするなら
これを渡すとよい（今回は実機ビルドが一発で通った）。

##### 実測（Windows 実機）

`cargo build --workspace` **一発 exit 0 / 7m59s**。

**`shell_integration_powershell` 6 passed / 0 failed**（36 秒）:
pwsh 7 の cwd 追従 / pwsh 7 の状態（idle・running・failed）/ 5.1 の cwd /
5.1 の状態 / **器の中では OSC が外へ出ない**（psmux の制約をテストで固定）/
統合なしでは何も報告されない。**#525 の受け入れ条件 3 はここで満たした**。

CLI の通し（release バイナリ。理由は下記 #856）:

| 検査 | 結果 |
|---|---|
| `install` の配置先 | pwsh 7 と 5.1 の**両方**。実パスは `…\OneDrive\ドキュメント\PowerShell\profile.ps1`（= 決め打ちが外れる構成そのもの） |
| `status` | `installed: true` かつ **`effective: false`** + `blocked_by_backend`（psmux） |
| 冪等性 | 2 回目は両方 `unchanged` |
| 置いたブロックの符号 | **非 ASCII バイトは BOM の 3 バイトのみ**（パスに日本語があっても `[char]0xNNNN` へ逃げている） |
| `uninstall` | 両方 `deleted` + **バイト列が完全復帰** |

全体は `tako-app` 416/**0** / `tako-cli` 53/**0** / `tako-control` 954/25 /
`tako-core` 682/5 / `platform_parity` 10/0 / `encoding_conpty` 5/0 / `psmux_backend` 16/0。
**失敗 30 件のままで新規ゼロ**（新規テストは Windows でも全部通り、
control 950→954・core 668→682 と増えただけ）。

##### マトリクスは Pending → Degraded へ倒した

実機で通しで確認できたので作法 4 の条件を満たす。ただし**器が psmux だと OSC が
外へ出ない**ので `Supported` ではなく `Degraded`（note は
`WIN_SHELL_INTEGRATION_PSMUX`）。この note はリリースノートの
Known limitations にもそのまま出る。

##### 起票して閉じた #856（**スライス 5 が既に直していた**）

検証中に debug の `tako.exe` が `--version` すらスタックオーバーフローするのを見つけて
#856 を起こしたが、**スライス 5（`0880c26`）が同じバグを既に直していた**。
このブランチの base が `83bbdc0`（スライス 6）= スライス 5 より前だったため、
測ったのが修正前のツリーだった。**rebase 後は debug でも起動する**。#856 は重複として close。

測って足せた情報だけ残す:

- PE の `SizeOfStackReserve` は **1,048,576 バイト = Windows の既定**
  （macOS / Linux のメインスレッドは 8 MB なので同じコードでも落ちない）
- **`cargo test` では検出できない**。libtest は各テストを別スレッドで回すので
  メインスレッドの制限に当たらない（`tako-cli` は 53 passed のまま）= #583 に現れない
- 実際の対処は `tako-cli/src/main.rs` で本処理を `.stack_size(16 MiB)` のスレッドへ載せる形

**教訓（スライス 5 の申し送りと同じ）**: ユニットテストは実バイナリの起動経路を踏まない。
検証には**実バイナリの CLI を 1 回叩く**を必ず入れる。


### 7b. 起動コマンドの送達確認（#640）— ✅ **完了**（PR #869）

- 持ち込む新規: `crates/tako-core/src/shell_send.rs` / `crates/tako-core/tests/shell_send_e2e.rs`
- 編集: `lib.rs` / `host.rs` / `dispatch.rs`（4 経路 + MockHost + 回帰 2 本）/ `tako-app/main.rs`
- 依存: **2**（器 psmux が起動直後の入力を落とす前提そのもの）
- 移植元は単一コミット `1107742`（win467 上の PR #670）

#### 完了記録（2026-08-21。PR #869 / `windows/640-shell-send`）

8 ファイル / +1,152 / −24。**plan の見立てとの差 3 件**（いずれも「#640 より後に main へ
入った変更との衝突」で、移植元には存在しない）。

##### 1. `diag::flow_log` が main に無い

#640 のクローズ手順が参照する切り分け経路（`TAKO_FLOW_DIAG=1 TAKO_PERF_LOG=<path>` で
「起動コマンド送達: pane=N 段階 → 段階」を採る）は `flow_log` に依存するが、これは
**#623 由来で main へ入っていない**。20 行の自己完結な関数なので 7b で新設した。

##### 2. #761 が「起動コマンドは `queue_write` に積まれる」前提を持っていた

`#640` は 4 経路を `queue_write` → `queue_command_flow` へ移すので、その前提のテストが壊れる。
**#761（2026-08-05）は shell_send（07-30）より後に main へ入った**ため移植元には無い衝突:

| 壊れた側 | 対処 |
|---|---|
| `dispatch.rs` の `successor_launch_cmd`（unit 9 本が依存） | `command_flows` を見るよう適応 |
| セルフテスト項目 102（#761） | 同じく `command_flows` へ。`ShellSendFlow::command()` を新設 |
| セルフテスト項目 102b（#792） | 起動コマンドを捨てる先に `command_flows` を追加 |

`command()` は**診断ログには使わない**（ログ用途は `command_len` / `stage_name` のまま）。
セルフテストが「何が積まれたか」を見るための口。なお項目 102 は `queue_command_flow` の
TakoApp 側配線が抜けていれば必ず落ちるので、**実アプリの配線の検出力もここで得ている**
（MockHost のテストは dispatch までしか見ない）。

##### 3. `with_test_project` の cwd が `/tmp` 決め打ちだった（作法 11 の実例）

移植した回帰テスト 2 本が Windows で落ちたので調べると、**spawn 系テスト一族 12 本が
同じ理由で前から落ちていた**（`Operation("cwd が存在しない: /tmp")`）。存在するディレクトリ
なら何でもよいので `std::env::temp_dir()` へ。macOS は挙動不変（cwd 値を assert するテストは無い）。

→ **実機の失敗が 31 → 20 件**（ベースライン 29〜30 から純減）。

#### 実機実測（`ssh win`。隔離 GUI を `schtasks /it` で session 1 へ）

受け入れ条件 1 = #640 の症状解消。

| 観測 | 結果 |
|---|---|
| 実測ハーネス 旧経路（書きっぱなし） | **0/4 到達** |
| 実測ハーネス 新経路（送達確認つき） | **4/4 到達** |
| 同 日本語入り本文 | **3/3 到達** |
| 製品経路（`orchestrator spawn`）で起動コマンドが全文届く | **5/5**（中抜けゼロ） |
| 同 コマンド行が実行された | **5/5** |
| `flow_log` の段階遷移 | `シェル準備待ち → エコー待ち → 実行確認`（書き直し 0 回・長さ 55） |

旧経路と新経路はどちらも本番のフロー上限（120 秒）で観測している（短く切ると「落ちた」と
「遅れているだけ」を区別できない）。

#### 7b が残した宿題

- **#867（新規起票）**: 送達は直ったが、届いた起動コマンドが **PowerShell で解釈できない**。
  `orchestrator/mod.rs:1723` の env 前置きが POSIX の `VAR=value cmd` 形式で、
  PowerShell には無い構文（`$env:VAR='v'; cmd` が要る）。**#640 の 4 経路すべて**と
  `tako master` / `solo` が該当する。#865 の `platform::shell_dialect` は
  **セルフテストが打ち込む文字列に限定された境界**なので製品側は対象外
- `tako_send_input` で**既存ペイン**へ送る経路は書きっぱなしのまま（移植元の意図的な範囲外。
  Ctrl+C による復旧が任意のペインに対しては危険なため別設計が要る）


### 7c. 起動コマンドの env 前置きをシェル方言へ（#867）— ✅ **完了**（PR #874）

7b（#640）で「起動コマンドがペインへ全文届く」ようになったが、**届いた命令が PowerShell で
解釈できず**エージェントが起動しなかった。その残り半分。7b の実機検証で見つけて起票した分。

- 持ち込む新規: `crates/tako-control/src/launch_cmd.rs`（win467 には無い。**新規設計**）
- 編集: `orchestrator/agent.rs` / `orchestrator/mod.rs` / `transcript.rs` /
  `platform/shell.rs`（`default_shell` を `pub` へ）
- 依存: **2**（器の中のシェルが PowerShell であること）、**7b**（送達が成立していること）

#### 完了記録（2026-08-21。PR #874 / `windows/867-shell-dialect-env`）

##### 対象は 5 フロー / 3 関数

呼び出し元を全数確認した結果、**#640 の 4 経路 + master / solo が 3 関数に集約されていた**。

| 関数 | 通る経路 |
|---|---|
| `agent::build_worker_cmd` | orchestrator spawn / git resolve のエージェント |
| `orchestrator::build_master_cmd` | `tako master` / `tako solo` / handoff の後任 master |
| `transcript::resume_env_prefix_for` | `sessions resume` / worker レジストリの `resume_command` |

`tako master` も `Request::Send` でペインへ**打ち込む**経路なので同じ関数を通る。
各関数に構文を明示する `*_in` 版を分け、既定版はペインの既定シェルから引く。

##### 変換規則（実機で 6 項目とも検証済み）

| POSIX | PowerShell |
|---|---|
| `VAR='v' cmd` | `$env:VAR='v'; cmd` |
| `export K=v; ` | `$env:K='v'; ` |
| `unset K; ` | `Remove-Item -LiteralPath 'Env:K' -ErrorAction SilentlyContinue; ` |
| `"$(cat p)"` | `"$(Get-Content -Raw -LiteralPath 'p')"` |
| `'…'`（`'\''`） | `'…'`（`''`） |

`-ErrorAction SilentlyContinue` は未設定でも行が止まらないため（POSIX の `unset` と挙動を揃える）。
`Get-Content -Raw` は `cat` と違い**末尾改行を保つ**（codex の system prompt では無害）。

##### #865 との調整（判断の記録）

#865 が同じ判定を持つ `platform::shell_dialect` を**セルフテスト用**に作っていた。
当初は「あちらの merge を待って後乗り」で合意したが、見込み 30〜60 分が 1.5 時間超になり、
その間も `shell_dialect.rs` が育ち続けた（`print_lines` / `quote_arg` 等）。

- 待つ = #867（**Windows でエージェントが起動できるか** = ミッションのコア）の完了時刻が読めない
- スナップショットを取り込む = #865 側に rebase コンフリクトを押し付ける

ので**依存を切って先に出した**（あちらのファイルへの差分ゼロ = コンフリクト構造的にゼロ）。
判定が 2 本になることは自覚しており **#873 で一本化を起票**。#865 担当者へ通知・合意済み。

**教訓**: 兄弟セッションの未マージ成果に依存するときは、①相手の残り見込みを聞く
②その見込みが外れたときの代替を先に決めておく。ファイルを共有するのではなく
**型を引数で受け取る形にしておくと、後から寄せるのが安い**。

##### macOS を 1 バイトも変えないためにクォートを 2 系統にした

`quote`（必要なときだけ引用 = 従来の `sh_quote`）と `quote_always`（元コードが `'{x}'` と
直書きしていた箇所）。片方に寄せると `--append-system-prompt-file '/tmp/p.md'` が
引用なしに変わり既存スナップショットが落ちる。

##### 既定版が「動いているシェル」を見るようになった副作用（実機で 22 件）

POSIX 文字列を固定するスナップショット群が Windows で必ず落ちた。テストが固定したいのは
POSIX 形式そのものなので、**構文を明示する `*_in` 版を呼ぶ形へ寄せた**（28 箇所）。
dispatch 経由の 2 件は builder を直接呼べないので期待値を構文非依存にした。

→ 作法 11（プラットフォーム決め打ちのテストを疑う）に、**「自分の変更で決め打ちを
作り込むこともある」**を追記する価値がある。

#### 実機実測（`ssh win`。隔離 GUI を `schtasks /it` で session 1 へ）

| 観測 | 結果 |
|---|---|
| 生成された起動コマンド | `$env:TAKO_ORCHESTRATOR_ROLE='worker:p867'; claude --effort max` |
| `is not recognized` エラー | **消滅**（4 回の観測すべてで False） |
| **claude が起動** | TUI 描画を確認（`[Opus 5 (1M context) · MAX]` / ctx バー / auto mode） |
| **プロンプトが届いた** | `❯ 1 と 2 を足した数だけを答えてください。説明は不要です。` |
| **claude が応答** | `● Login expired · Please run /login`（**この機の claude ログイン期限切れ**。
起動と送達と応答は成立） |
| **env が実プロセスへ届いた** | claude.exe の PEB を読み `TAKO_ORCHESTRATOR_ROLE=worker:p867` を確認 |
| `flow_log` | `シェル準備待ち → エコー待ち → 実行確認`（書き直し 0 回・長さ 62） |

実機テストは **22 件失敗 = 7b ベースライン 19 + #868 由来 3**（新規ゼロ。
`resolve_cwd_existing_dir` は逆に通るようになった）。

#### 7c が残した宿題

- **#873**: 方言判定の一本化（`LaunchSyntax` → `ShellDialect`）。#865 merge 後
- **claude のログインが切れている**ので「3」という中身の回答までは未確認。
  対話 `/login` が要るので実アカウントに触れない範囲で止めた
- 兄弟セッションからの申し送り: **#875**（`spawn_command_pane` が `/bin/sh -c` 決め打ちで
  Windows では PTY が立たない = #666 のカード実行と #453 の Code Runner が死んでいる）。
  `platform/shell.rs` に `run_pane_command` を足す設計が保全ブランチにあるとのこと

### 8 の前提: セルフテストの方言対応（#865）— ✅ **完了**（PR は本文末尾の記録）

スライス 5 / 7 の申し送りは「実機セルフテストが項目 2（TERM / COLORTERM 注入）で止まるので
スライス 7 完了後に通しで回す」だったが、**この見立ては誤り**だった。止まっていたのは
シェル統合ではなく**テストの書き方**で、`echo TERMCHK=$TERM,$COLORTERM` の
`$TERM` は PowerShell では未定義の PowerShell 変数（環境変数は `$env:TERM`）。
機能は正常なのに必ず落ち、**以降の項目が 1 つも走らない = Windows のカバレッジ 0** だった。

#### 入れたもの

- `crates/tako-core/src/platform/shell_dialect.rs`（**新設**）: 打ち込む文字列の方言差を
  閉じ込める境界。方言は OS ではなく `default_shell()` が選んだプログラムから引く
  **純粋関数**（`from_program`）なので、macOS から PowerShell 側の生成結果を全部テストできる。
  `cmd.exe` / fish は変換先が無いので `None`（呼び出し側が「対象外」と明示する）
- 語彙: `echo`（`${NAME}` を環境変数参照へ展開）/ `arith` / `marker` / `emit_ansi(_line)` /
  `seq` / `sleep` / `exit_status` / `cd` / `mkdir_and_cd` / `program`（PowerShell は
  呼び出し演算子 `&`）/ `discard_output` / `on_success(_echo)` / `on_output_contains_echo` /
  `on_env_set_echo` / `on_ipc_endpoint_ready_echo` / `on_cwd_is_home_echo` / `with_env` /
  `without_env` / `quote_arg` / `print_lines` / `assign_output` / `repeat` / `sequence` /
  `paint_and_hold` / `shell_snippet_argv` / `shell_snippet_command` / `emit_numbered_lines` /
  **`clear_line_key`**（後述）
- セルフテスト側は打ち込む文字列を全部この境界経由へ。**機能そのものが Windows に無い項目は
  「何が無いか」を理由に明示スキップ**（ログに追跡 Issue 付きで出る）

#### PowerShell 側の形は実機で 1 つずつ測って決めた（pwsh 7 と Windows PowerShell 5.1 の両方）

| 罠 | 実測 | 採った形 |
|---|---|---|
| 裸の引数のカンマ | `echo A=$env:X,$env:Y` が **2 行に割れる**（配列区切り） | 必ず二重引用符で包む |
| `&&` | 5.1 に無い | `cmd; if ($LASTEXITCODE -eq 0) { … }` |
| `` `e `` | 7 専用 | `$([char]27)` |
| 引用符付きパス | **式として評価され実行されない** | 呼び出し演算子 `& "…"` |
| `Ctrl+U` | **PSReadLine（Windows モード）に存在しない**（`Get-PSReadLineKeyHandler -Bound` に出ない） | `Escape`（`RevertLine`）。セルフテストは行を捨てる道具として 30 か所で使っていた |
| `test -S` | 受け口は named pipe | `Test-Path $env:TAKO_SOCKET`（`\\.\pipe\…` に対して実在で true / 不在で false を実測） |
| `printf '%b'` | 無い | `Write-Host -NoNewline "…"`（`` `n `` / `$([char]27)` へ翻訳） |
| `/bin/sh -c` | 無い | `powershell -NoProfile -Command` |

#### 実機の到達範囲（`ssh win` / `schtasks /it` で session 1 へ GUI を投げる）

**修正前**: `TAKO_APP_SELF_TEST_FAILED: TERM / COLORTERM 注入` / `EXITCODE=1`（9 秒で終了）。

**修正後**: **項目 0〜92 が通る**。止まるのは項目 93（#694 GUI モード判定表）で、
材料が OSC 133 の idle 検知 + `cat` なので **psmux 越しにシェル統合が届かない**（#766）と同根。

スキップした項目と理由（すべてログに出る。**直れば自動で検証が復活する形**にしてある）:

| 項目 | 理由 | 追跡 |
|---|---|---|
| 1d | tmux は POSIX シェル環境の道具（器は psmux） | #519 |
| 40b の fd 検査 | `/dev/fd` が無い | — |
| 41 / 41b | `shell_integration::status().effective()` が false（psmux が OSC を素通ししない） | #766 |
| 41c / 41d | zsh 不在（元から自動スキップ） | — |
| 45b | 受信バイトを画面へ出す仕掛け（cooked TTY の ECHOCTL）が POSIX 専用 | #729 |
| 48 / 59〜62 | **本物の tmux が無い**（実機の `tmux` は psmux が同名で入れているもの） | #866 |
| 53 / 54 | listen 役の `nc` とジョブ制御が POSIX 専用 | — |
| 66 / 66b-2 / PDF 選択 / 150% の文字座標 | `platform::pdf::capabilities().text_layer` が false | #693 |
| 69b の根因再現 / グリフ隔離 | shaper とフォント寸法に依る前提 | — |
| 69c | `links.rs` のパス検出が POSIX 形前提（`std::path::MAIN_SEPARATOR == '/'` で判定） | #522 / #870 |
| 71 | wry の WebView2 が `data:` URL で **abort**（`InvalidUri(Empty)` を COM コールバック内で unwrap） | #724 |
| 77 / 79 / 80 | **2 枚目のウィンドウを作るとアプリが静かに終了する**（exit 0 / panic 無し） | #872 |
| ~~91 の実行検査~~ | ~~コマンド実行ペインが `/bin/sh` 決め打ちで **PTY が立たない**~~ → **#875 で解消**（PR #879）。この行の SKIP は出なくなり、実行検査が緑で通る | #875 |

#### 起票した製品バグ（すべて実機実測つき）

- **#866**: psmux が tmux の `=name`（完全一致ターゲット）を解釈できず `tako tmux kill` が効かない
  （`=` の有無だけを変えた A/B つき）
- **#870**: `links.rs` のホーム解決が `HOME` 決め打ち（Windows は `USERPROFILE`）。
  ホーム解決が `terminal.rs` と 2 か所にあることが原因
- **#872**: 2 枚目のウィンドウ生成でアプリが静かに終了する
- **#875**: コマンド実行ペイン（#666 カード / #453 Code Runner）が `/bin/sh` 決め打ちで立たない
  → **解消済み**（PR #879。下の「#875 の記録」節）
- **#724 へ追記**: 症状②の正確な panic 位置（`wry/src/webview2/mod.rs:910`）とスタック

#### テスト側で直した「macOS では見えなかった穴」

- 項目 90 / 66c の Markdown が **`drain_pending_preview_loads` を呼んでいなかった**。
  macOS は前のファイルの座標キャッシュが残っていて**空振りで緑**になっていた
  （Windows は直前の PDF が text_layer を持たずキャッシュが空なので顕在化）
- 項目 66b-2 の座標検査が `line = 40` 決め打ち。#821 の仮想リスト以後は
  **画面に出ている行しかレイアウトを持たない**ので、寸法が違う環境で panic した
- 項目 48 / 87 の「出来事」を固定待ちで見ていた（#796 の作法へ揃えた）
- 項目 67 のマルチルート判定が `roots().any(|r| r.ends_with("tmux"))` の**名前決め打ち**
  （`Path::ends_with` は成分の完全一致で大小も区別する = `…\Local\Temp` で外れる）
- 項目 76b / 76d が**分割に失敗した状態で最後のペインを閉じ**、アプリを終了させていた
  （「静かに走らなかった」を作る形）。セルフテスト中の終了に発生源を出す診断も入れた

#### 実機で GUI セルフテストを回すレシピ（そのまま使える）

```powershell
# 1 度だけ: session 1 へ投げるタスクを作る（SSH は session 0 で GUI が出ない）
schtasks /create /tn tako865 /tr "C:\Users\<user>\dev\tako-evidence-865\run-selftest.cmd" /sc once /st 23:59 /it /f
# run-selftest.cmd の中身（TAKO_ISOLATED=1 / TAKO_SELF_TEST=1 / CARGO_BUILD_JOBS=2 を立ててログへ）
schtasks /run /tn tako865
# ログは UTF-8 で読む（cp932 で読むと日本語が化ける）
[System.IO.File]::ReadAllText("…\selftest.log", [System.Text.Encoding]::UTF8)
```

1 反復は**実機の増分ビルド 1.2〜2.4 分 + セルフテスト 1.5 分**。32 反復回した。

#### 次の一手（スライス 8 へ）

- 項目 93 以降（GUI モード / チャット / 設定画面 / limit-resume）は
  `shell_integration::status().effective()` を材料にする項目が続く。**#766 が直れば一気に進む**
- マトリクスは**触っていない**（作法 4）。ただし棚卸しの材料は揃った:
  上の表がそのまま「Windows で何が動いて何が動かないか」の実測一覧になる。
  いま `tako_theme` / `tako_open_file` / `tako_preview_view` 等が `Pending` のままだが、
  **セルフテストは実機でそれらを通している**（= Supported / Degraded へ倒せる）



#### 7c の後始末: 方言判定の一本化（#873）— ✅ **完了**（PR #878）

#867 の着手時点で #865 が未マージ・活発に変更中だったため、起動コマンド側は独自の
`LaunchSyntax` を持っていた。#865（PR #876）が入ったので寄せた。**挙動不変**。

- `launch_cmd::LaunchSyntax` を廃し `platform::shell_dialect::ShellDialect` へ（`pub use` で再公開）
- 入口を `launch_dialect()` に改名し、**知らないシェルを POSIX へ倒すのはここ 1 か所**に閉じた。
  `from_program` が `None` を返すのはセルフテスト側が「対象外」を明示できるようにするためで、
  **起動コマンドで止めると今まで動いていた環境でエージェントが起動しなくなる**
  （用途で `None` の扱いが変わるのが正しい、という結論）
- **番犬テスト**: 方言を表す enum の定義がワークスペースに 1 つだけであることをソース走査で固定

##### クォートは統合しなかった（実測に基づく判断）

`ShellDialect::quote_arg`（`tako_core::shell::quote_for_shell` 経由）と
`launch_cmd::quote`（`sh_quote`）は**安全文字の集合が違う**。10 入力中 7 件が相違し、
実運用の値も含む:

| 入力 | `quote`（起動コマンド） | `quote_arg`（セルフテスト） |
|---|---|---|
| `worker:p867`（role） | `'worker:p867'` | `worker:p867` |
| `検証`（日本語ラベル） | `検証`（Unicode 英数を素通し） | `'検証'` |
| `a,b` / `x@y` / `50%` / `a+b` | 引用する | 素通し |

起動コマンドの文字列は spawn 応答の `command` やレジストリの `resume_command` として
**ユーザーと AI に見える**うえ、既存スナップショットが「#120 以前と同一文字列」を固定して
いるので、寄せると見える文字列が変わる。**リファクタで倒してよい話ではない**と判断し、
両者が違うことを固定するテストを足して意図として残した。

##### 実機実測

| 観測 | 結果 |
|---|---|
| 実機テスト | **22 件失敗 = #867 後のベースラインと完全一致（新規ゼロ）** |
| セルフテストの停止位置（本ブランチ） | 項目 93（#694 判定表） |
| 同（main `b2634a1` で A/B） | **同一項目** = 到達範囲（項目 92 まで）は維持 |

macOS: `test --workspace` 2377 passed / 0 failed / fmt / clippy（両 feature）/
隔離セルフテスト `TAKO_APP_SELF_TEST_OK` / クロスチェック**警告リストが現 main と完全一致**。
番犬の検出力は一時的に enum を 2 個にして FAILED を実測。

##### 兄弟セッションとの並行の作法（3 本同時に回して学んだこと）

このセッションでは #867 / #865 / #875 が同時に走った。効いたのは次の 3 つ:

1. **着手前に「触るファイル」を相互に宣言する**。#875 とは 3 本 / 4 本で重なりゼロを
   先に確認できたので、以後の調整コストがゼロになった
2. **共有リソース（実機）は時間で区切って明示的に受け渡す**。「あと 15 分で解放」「40 分渡す」
   「20 分で返す」のように**数字で言う**と待ち時間が読める。GUI セルフテストは前面を
   奪い合うと SKIP / FAILED が増える（作法 7）ので、GUI を立てる検証だけは直列にした
3. **番犬テストは「相手が踏むかもしれない」ことを先に伝える**。#873 の番犬（方言 enum は
   1 つだけ）は #875 が新しい enum を作ると落ちるので、merge 前に共有した（結果、
   相手は `ShellDialect` をそのまま使っていたので無害だった）
#### #875 の記録（実行ペインの起動コマンド。PR #879・2026-08-21）

**症状**: #666 カードの「新規ペインで実行」/ #453 Code Runner の `tako run` /
`tako run-interactive` / `tako show-command --run` が Windows で何も起きない。
`dispatch::spawn_command_pane` が `/bin/sh -c` と POSIX の後置きを直書きしていた。

**直し方**: 組み立てを境界 B1（`platform::shell::run_pane_command`）へ。POSIX 側は
従来の直書きとバイト一致（テストでリテラル固定）、Windows 側は PowerShell へ
`-EncodedCommand`（base64 / UTF-16LE）。方言判定は `ShellDialect::from_program` の
使い回しで、**新しい enum は作っていない**（#873 の番犬に引っかからない）。
`tako:shell` 宣言の包み方も同じ判定 1 本（`declared_shell_command`）。
マーカーの正は `EXIT_MARKER_PREFIX` の 1 個にして組み立て側と `find_exit_marker` が共有する。

**plan の見立てとの差 3 件**:

1. **保全ブランチの `run_pane_command` をそのまま持ち込むと `WindowsShell` enum +
   `resolve_windows_shell_kind` という「3 本目の方言判定」が生える**。#865 の
   `ShellDialect` と #867 の `LaunchSyntax` を #873 が 1 本へ寄せている最中なので、
   判定は `from_program` の使い回しにし、`cmd.exe` 分岐は作らなかった
   （マーカー契約を cmd で満たすのは別物になるうえ、`powershell.exe` は System32 に必ず在る）
2. **persist ON（器 = psmux）では 1 回目の修正でも PTY が即死した**。psmux は内側コマンドの
   第 1 語の引用符を剥がさないので空白入りのプログラムパスを運べず、`inner_command` の
   `cmd.exe /c '…'` 包みは**実測で効かない**（doc の「実測成功」は古い。**#881 に起票**）。
   実行ペインは最初から 1 語で書ける形（`pwsh.exe`）を渡して回避した
3. **自分の変更で決め打ちを作り込んだ（作法 11 の実例が 3 本目）**。`/bin/sh` を無条件に
   期待するテストが dispatch に 3 本あり、macOS のゲートは全部緑のまま
   **実機だけ 23 件失敗（ベースライン 22 + 1）**になった。境界の出力との突き合わせへ変え、
   POSIX 固有の形は `#[cfg(unix)]` の中へ残す形にした

**セルフテスト項目 91(d) の厳格化**: #865 が入れた「PTY が立たないときだけ実行検査を外す」
緩和（`no_run_pty`）を撤去した。起動経路が壊れたら SKIP ではなく FAILED にする。

##### 実機実測（`ssh win`。隔離 GUI を `schtasks /it` で session 1 へ）

| 観点 | before（main b2634a1） | after（`aaed733`） |
|---|---|---|
| `tako run-interactive` | `error: PTY を起動できなかった` | pane 生成 → `TAKO875-RI-OK` + `__TAKO_EXIT=0` → status が exit_code 0 で auto_close |
| `tako run`（`tako:run` 宣言） | 同上 | 出力 + マーカー（狭いペインだと出力行は画面外へ流れる。ペインを広げれば出る） |
| `tako show-command --run` | 同上（カード作成だけ成功） | pane 生成 → 出力 + マーカー・15 秒後も生存 |
| ペイン数 | 1 のまま | 1 → 2 |
| persist ON（psmux） | 同じく PTY が立たない | 器つきで生存（`tmux_session` が付く） |
| 終了コードの解決 | — | `cmd /c exit 7`→7 / cmdlet 失敗→1 / `exe 失敗; 成功`→0 / `成功; cmdlet 失敗`→1（direct / psmux とも一致） |
| 引用符・日本語 | — | `echo "hello world 875"` / `echo 'single 875'` / `echo 日本語のテスト875` すべて素通り |
| セルフテスト項目 91 | `ran=false new_pane=None has_pty=false` + `SKIPPED: 91(d)` | `ran=true waited=0.8s new_pane=Some(PaneId(47))`・**SKIP 行が消える** |
| セルフテストの停止位置 | 項目 93（#694） | **同じ項目 93**（到達範囲は不変） |
| SKIP 一覧 | 18 行 | 17 行。**差分は 91(d) の 1 行だけ**（Compare-Object で確認） |
| `cargo test --workspace` | 22 件失敗 | **22 件失敗・集合が完全一致**（新規ゼロ・解消ゼロ） |

macOS: fmt / clippy（`--features visual-test` 有無とも）/ `test --workspace` **2386 passed / 0 failed** /
`check-windows.sh --all-targets` **エラー 0・警告リストが main と完全一致**。

#### 7c / #873 の続き: agents 走査の Windows 対応（#877）— ✅ **完了**（PR #882）

#867 が直したのは「ペインへ**打ち込む**コマンド」で、こちらは **tako 自身がシェルを起こす経路**。
`claude agents --json` の走査が `$SHELL -l -c <シェル片>` の直書きで、Windows では必ず失敗していた。

##### 実機で 2 通りとも壊れていた（修正前）

| プロセス env | 結果 |
|---|---|
| `SHELL` 未設定（**GUI 起動と同じ状態**） | `/bin/sh` へ落ちて `CreateProcess: The system cannot find the file specified` → `tako remote agents` が `exit 1` |
| `SHELL=powershell.exe`（SSH セッションの副作用） | `-l : The term '-l' is not recognized…`。`;` の後ろの `claude agents --json` だけがたまたま走るので**前置き（`unset` / `export`）が黙って実行されない** |

`SHELL` は **Process スコープにしか無い**（`[Environment]::GetEnvironmentVariable("SHELL","User")` /
`"Machine"` はどちらも空）。SSH セッションが持っているだけで GUI 起動の `tako.exe` には渡らないので、
実運用は上の行 = 全滅側。**「SSH で測ると動いているように見える」罠**なので、以後この一族を
測るときは `Remove-Item Env:SHELL` を先に打つこと。

##### 入れたもの

抽象境界 **B21（`tako_core::platform::child_cmd`）**。「tako 自身がユーザーの環境で CLI を
1 回走らせる」形だけを持つ。ペインの PTY（B1 = `platform::shell`）とは別物なので独立させた
（#875 が `shell.rs` を大改修中だったのでコンフリクトもゼロ）。

| | 形 | 理由 |
|---|---|---|
| unix | `<$SHELL> -l -c <シェル片>`（**従来と 1 バイトも同じ**） | `.app` を Dock から起動すると PATH が最小構成になり Homebrew / npm 導入の CLI が見つからない |
| Windows | `platform::exe::find`（B16）で解決した実体を直接起動 | `SHELL` も `-l -c` も無い。**rc に相当するものが無いので env 前置きが要らず**、`Command::env` / `env_remove` だけで確定する |

- 走査コマンドは `AGENTS_SCAN_ARGV` の 1 か所から「POSIX シェル片」と「argv」の両形を作る
  （片方だけ直すずれが構造的に起きない）
- `diag::flow_log`（`TAKO_FLOW_DIAG=1`）へ失敗理由の**分類だけ**を出す。呼び出し側は
  CLI 未検出 / spawn 失敗 / claude の異常終了（認証切れ等）のどれも同じ `None` になるので、
  ログが無いと切り分けられなかった。claude の出力そのものは載せない（AGENTS.md の絶対ルール）

##### 実機実測（`ssh win`。GUI 不要 = `tako remote agents` は daemon も GUI も要らない）

| 観測 | main `c8c9fbb` | 本 PR |
|---|---|---|
| `tako remote agents`（`SHELL` 無し） | `exit 1` / `error: claude agents --json の実行に失敗（…）` | `exit 0` / 稼働中の claude を 1 件返す |
| e2e `issue877_agents_scan_e2e`（同一ファイルを両 HEAD へ当てた A/B） | **FAILED** | **ok**（`query_agent_status(…) -> status="idle"`） |
| `agents-auto`（`resolve_session_id_for_backend`） | — | **成立** — `器のペイン一覧（socket=tako）: [("tako-s877probe:0.0", 24536)]` → `resolve_session_id_for_backend(tako-s877probe) -> Some("cd75581b-…")` |
| `flow_log`（claude を PATH から外した状態） | — | `agents 走査: claude の実体を解決できない（PATH に無い）`。正常時は 1 行も書かない |
| `cargo test --workspace --no-fail-fast` | 22 failed | **22 failed（失敗テスト名まで完全一致 = 新規ゼロ）** |

**認証は要らない**: 実測で claude は `Not logged in · Run /login` の TUI でも
`agents --json` に `status: idle` で載る（この機の claude はログイン期限切れのまま）。
`kind: interactive` / `sessionId` / `pid` も全部入る。

**器（psmux）越しでもペイン対応付けが効く**のが分かったのも収穫。`psmux -u -L tako new-session`
で作ったセッションは `tmux -L tako list-panes -a -F "#{session_name}…"` で
**接頭辞なしの素の名前**（`tako-s877probe:0.0`）で返るので、`agents::tmux_pane_pids` /
`resolve_session_id_for_backend` の `starts_with("<session>:")` がそのまま通る。
（`-L` を落として作ったセッションは `-L tako` から見えない = 名前空間が分かれる。
測るときは tako と同じ `-u -L tako` で作ること）

##### 検出力

| 戻し方 | 落ちるもの |
|---|---|
| オーケストレーション層へ `var("SHELL")` を再導入 | 番犬テスト `agents走査がposixシェルの直起動へ戻っていない`（`mod.rs:2387` を名指し。実測済み） |
| 走査を main の実装へ戻す | Windows 実機の `issue877_agents_scan_e2e`（実測済み） |
| Windows 側を POSIX シェル経由へ倒す | `windowsの実機ではposixシェルを経由しない`（`#[cfg(windows)]`） |
| unix 側を直接起動へ倒す | `unixの実機ではログインシェル経由になる`（`#[cfg(unix)]`） |
| POSIX 前置きの文字列を変える | `agents走査のposix前置きは従来と同一で環境変数も同時に指定する` |

##### #877 が残した宿題

- **同型の一族は手つかず**（スライス 8 / #875 の対象）。`$SHELL -l -c` を直書きしている残りは
  `platform::exe`（B16 の unix 実装。境界の内側なので正）/ `tako-core/src/lib.rs` /
  `tako-app/src/autorename.rs` / `tako-app/src/preview.rs` /
  `tako-control/src/config_share/env.rs` / `tako-control/src/setup_bootstrap.rs`。
  **どれも B21 へ寄せられる形**（`user_env_cli` に `command -v <name>` 相当を渡すだけ）
- **マトリクスは 1 件も動かしていない**（作法 4）。`tako_orchestrator_watch` /
  `tako_orchestrator_worker_status` を Supported / Degraded へ倒すのはスライス 8。
  ただし材料は揃った: 走査・`query_agent_status`・`agents-auto` の 3 段が実機で通る
- `worker_status` / `watch` の**応答 JSON まで**（IPC + dispatch 込み）は GUI が要るので未実測。
  実測したのは走査 →`query_agent_status`（`status_source = agents`）→
  `resolve_session_id_for_backend`（`status_source = agents-auto`）の 3 段で、
  そこから応答 JSON までの間は macOS のユニットが固めている純ロジックだけ

#### #881 の記録（器へ渡す内側コマンドの第 1 語。PR で追記・2026-08-21）

**症状**: persist ON（器 = psmux）で `SpawnCommand.program` に空白入りのパスを指定すると、
ペインは生えるが PTY が即死する（`tako split -- "C:\Program Files\x\y.exe"`）。

**原因**（`remain-on-exit on` を生きているサーバーへ直接立てて死因を採取して確定）:
psmux は内側コマンドを**単語分割の過程で引用符ごと落として** `CreateProcess` へ渡す。
`tmux_backend::wrap_options` の `shell_quoted(inner)` はプログラムまで単引用符で括るので、
`'C:\Program Files\…'` という名前のプログラムを探しに行って見つからず、器が既定シェルへ
丸投げして `Unexpected token '-NoLogo'` で死んでいた。

**直し方**: `BackendCapabilities::quotes_program`（tmux / 器なし = true、psmux = false）を新設し、
組み立てを `backend::inner_command_line`（判断部は純粋関数 `compose_inner_command`）へ 1 本化。
psmux 側は `platform::program_path`（**抽象境界 B18**）で 8.3 短縮名 → 実行ファイル名の順に
「空白を含まない 1 語」へ落とす。`GetShortPathNameW` の FFI は境界の中だけ。

**#875 の回避は撤去した**（受け入れ条件 2）。実行ペインが第 1 語を実行ファイル名へ落として
いたのは器がこれを運べなかったための回避で、器側が面倒を見るようになったので
**解決したフルパスをそのまま渡す**（取り違えの余地が無い方）。

##### plan の見立てとの差（最重要）

**「psmux 側の修正になるはず」は当たっていたが、直す関数を間違えた。**
`psmux::inner_command` を直して実機で試したら**症状が 1 ミリも変わらなかった**。
理由は `PsmuxBackend::wrap_spawn` に**呼び出し元が無い**こと（tako-app は
`tmux_backend::wrap_options` を直接叩いている）。スライス 2a が入れた backend trait は
spawn 経路では**まだ使われていない**。**#885 に起票**。

教訓: 実機で「直したのに変わらない」ときは、**その関数が本当に呼ばれているかを
`grep '\.関数名('` で確かめる**。今回は死にゆくペインの画面を 300ms 間隔で採取して
初めて実際の行（`'C:\Program Files\…' -NoLogo`）が見え、単引用符の出どころが分かった。

##### 実機実測（psmux 3.3.7 / Windows 11）

| 観点 | before（main `dc975df`） | after |
|---|---|---|
| `tako split -- "<空白入りフルパス>" -NoLogo` | 応答は pane 2 だが 8 秒後に消滅 | **生存**（`panes=[1,2]`・プロンプト表示） |
| `tako split -- pwsh.exe -NoLogo`（対照） | 生存 | 生存 |
| `tako split -- cmd.exe /c pause`（対照） | 生存 | 生存 |
| #875 の 3 経路（persist ON） | 生存 | 生存（`__TAKO_EXIT=0`。回避撤去後も不変） |
| 終了コード | — | `cmd /c exit 7`→7 / cmdlet 失敗→1 |
| `cargo test --workspace` | 22 件失敗 | **22 件失敗・集合が完全一致** |
| GUI セルフテスト | 項目 91 `ran=true` | 項目 91 `ran=true`・**SKIP/FAILED 一覧が #875 完了時と完全一致** |

##### 併せて起票したもの

- **#884**: 空白を含む **cwd** でペインが即死する（`-c <cwd>` が argv のまま届いていない疑い。
  psmux 単体では同じ argv で生存するので**層が違う**）。`C:\Users\First Last\…` を持つ
  マシンでは日常的に踏む
- **#885**: tako-app の spawn が backend の `wrap_spawn` を通らない（上記の教訓の恒久対処）

**#866 とは別物**（統合しない）: #866 は psmux の**ターゲット解決**（`=name` を解さない）で、
本件は**内側コマンドの引用**。今回 `-t <window 名>` がセッション名 `<sock>__<name>` として
解決される様子も観測したが、これも target 解決の話で引用とは機序が違う。

#### #884 の記録（PTY へ渡す argv の引用。PR で追記・2026-08-21）

**症状**: persist ON（器 = psmux）で `cwd` に空白を含むディレクトリを指定すると、
`tab new --cwd` / `tako run` のペインが**応答は返るのに消える**。
`C:\Users\First Last\…` のようにユーザー名へ空白が入るマシンでは日常的に踏む。

**原因層の確定（対照実測）**: psmux 単体へ**同じ引数を 3 通りの引用で**渡して切り分けた。
`Process.StartInfo.Arguments` を 1 本の文字列で渡す = ConPTY へ渡るコマンドラインと同じ形。

| 渡し方（`new-session -d -s … -c <dir>`） | 結果 |
|---|---|
| A: cwd を引用（正しい argv 相当） | **生存**・cwd も正しい |
| B: cwd を素のまま（`escape_args=false` の出力そのもの） | **セッションが存在しない** |
| C: 対照 = 素のままだが空白を含まない cwd | **生存** |

**psmux は無罪**（A と C が生きる）。**tako の argv → コマンドライン変換**が犯人。

**機序**（`remain-on-exit on` の conf を当てて死因を採取して確定）:

```
capture-pane => with: The term 'with' is not recognized as a name of a cmdlet, ...
pane_current_path => C:\Users\shioz   （-c が `…\dir` になり存在しないので落ちた先）
```

`-c C:\…\dir with space` が `-c` `C:\…\dir` `with` `space` へ割れ、
`new-session` は**余った語を shell-command と解釈して実行する**。
tako の器設定は `remain-on-exit` が off なのでペインは即破棄され、**画面には何も出ない**。

**コード上の原因**: `TerminalSession::spawn` が `tty::Options` を
`..tty::Options::default()` で組んでいたため、alacritty の
**`escape_args` が `false`** のままだった。Windows の `cmdline()` は
`program` と `args` を**素の空白で連結するだけ**なので、tako の argv 形の
`SpawnCommand.args` が Windows でだけ「生のコマンドライン断片」に意味が変わっていた。

**直し方**: `platform::shell::apply_arg_escaping`（境界 B1）を新設し、
`tty::new` へ渡す前に必ず通す。Windows は `escape_args = true`（CRT 規則）、
unix は恒等（`execvp` へ argv がそのまま渡る）。**語ごとの引用を自前で組まない**のは、
CRT 規則（引用符の前の連続バックスラッシュを倍にする等）を二重実装しないため。

`-c <cwd>` だけでなく **`-e KEY=<空白入りの値>` も同じ機序で壊れていた**のが
同時に直る（実測: `-e "TAKO_SPACE=a b c"` が `show-environment` でそのまま読める）。

##### #881 を巻き戻していないことの確認

`escape_args = true` にすると、器へ渡す**内側コマンド 1 本**（`inner_command_line` の
出力）も CRT 規則で 1 語へ括られる。psmux が単語分割の主体なので壊れないかを実測した:

| 形 | 結果 |
|---|---|
| `-c "<空白入り>" "pwsh.exe -NoLogo"`（after の形） | 生存・`cmd=pwsh`・cwd 正しい |
| `-c <空白入り> pwsh.exe -NoLogo`（before の形） | セッションが存在しない |
| `… "pwsh.exe -NoLogo -NoExit -Command 'Write-Output ok'"`（#881 の単引用符入り） | 生存・`ok` が出力される |

単引用符は CRT 規則の対象外なので**そのまま psmux の単語分割へ渡る** = #881 の
`program_path::single_token` の前提は不変。

##### テストの検出力で踏んだ罠（最重要）

最初に書いた e2e は「器がそのセッションのペインを**一度でも**正しい cwd で返したら合格」に
していたため、**修正を戻しても通った**（検出力ゼロ）。実測で機序を確定:

```
t=+200ms  => tako-884-27448 0 C:\…\Temp\tako-884 cwd 27448
t=+600ms  => tako-884-27448 0 C:\…\Temp\tako-884 cwd 27448
t=+1200ms => (no panes)
```

`-c` が割れて存在しないディレクトリになると psmux は**クライアントの cwd** へ落ちるが、
`TerminalSession::spawn` は `working_directory`（`CreateProcessW` の
`lpCurrentDirectory`）にも同じ cwd を渡しており、**そちらは引用の影響を受けない**。
そのため壊れていても 1 秒弱は「正しい cwd のペインが居る」ように見える。
出現を待ったうえで **4 秒の生存を見張る**形に直した（Issue の症状
「応答は返るがペインごと消える」そのものの判定）。

教訓: **「壊れている側で落ちること」を実際に確かめるまでテストは完成していない**。
とくに tako は同じ情報（cwd）を器へ 2 経路で渡しているので、
片方が壊れても一時的に正常に見える。

##### 実機実測（psmux 3.3.7 / Windows 11）

| 観点 | before（`escape_args` 既定） | after |
|---|---|---|
| `空白を含む引数が1語のまま子へ届く` | **FAILED**（`ARGC=1` にならない） | ok |
| `器ありでも空白入りcwdのペインが生き残る` | **FAILED**（現れたあと消える） | ok |
| `cargo test --workspace`（実機） | 22 件失敗 | **22 件失敗・集合が完全一致** |

macOS 側: `test --workspace` **2406 passed / 0 failed** / `fmt` / `clippy`（両 feature）/
クロスチェック **エラー 0・警告リストが main と完全一致**。
番犬（`spawnはargvの組み直しを境界へ委ねる`）は境界呼び出しを外すと FAILED になる。

##### 残っている隣接の穴（別件）

alacritty の `cmdline()` は **`program` を一切エスケープしない**
（`cmd.push_str(&shell.program)`。`escape_args` の対象外）。空白入りの
プログラムパスは `CreateProcessW` の「空白区切りを順に試す」探索に救われて
いるだけなので、`C:\Program.exe` のような細工があると取り違えうる。
本 Issue の症状（cwd）とは層が別なので触っていない。

### 8. doc / 対応マトリクスの最終棚卸し（#528 / #591 / #515）

- 持ち込む新規: `scripts/gen-windows-support-docs.mjs` / `docs/.../windows-support.md`（生成物）
- 編集: `docs/.../getting-started/index.mdx`（Windows タブ）/
  `docs/.../guides/keyboard-shortcuts.md`（**3 列化** = 操作 / macOS / Windows・Linux）/
  `platform/support.rs` の最終確認
- 依存: **1〜7 のすべて**（表が実態とずれると system prompt に誤情報が流れる。#516）
- `keyboard-shortcuts.md` は main が 2 列のまま行を約 47 追加しているので、
  **3 列へ寄せて main の行を移植する**作業が要る。
  `windows/467-main-merge-wip` にこの 7 hunk が未解決のまま残してあるので材料になる

### 9. スリープ防止 + 蓋閉じ継続 + ポート検知（#524 / #697 / #724）— ✅ **完了**（PR #863）

- 持ち込んだ新規: `crates/tako-control/src/platform/{lid,power}.rs` /
  `crates/tako-control/tests/lid_residual_windows.rs`
- 編集: `sleep_guard.rs` / `crates/tako-core/src/ports.rs`（`pane_key()` 経由の判定）/
  `crates/tako-core/src/backend/mod.rs`（`is_plumbing_process`）/ `agents.rs`（親子マップ）/
  `dispatch.rs` + `tako-cli`（3 経路 1:1）/ `config_share/catalog.rs`
- 依存: **1**

#### plan の見立てとの差（実測で分かったこと）

1. **`agents::process_parent_map` の配線が必須だった**（持ち込み表に無かった）。`ps` 直叩きなので
   Windows では常に空 → `ProcessSnapshot` も空 → **sleep guard の既定モード
   `while-agents-running` が busy_agents=0 のまま一度も発動しない**（stale binary 検知 #772 も同様）。
   スライス 1 が置いた `platform::procinfo` を配線して成立させた。`agents.rs` に `cfg(windows)` は
   書かず「境界が答えを持っているか」で分岐する（FFI の転記を二重に持たないため）
2. **#724 の症状①（器の偽ポート）は持ち込むべき**だった。psmux は IPC に TCP ループバックを使い
   サーバーを**クライアントの子**として起こすので、器つきのペインが例外なく 1 個の偽 listen を持つ。
   実機で **21 個の psmux プロセスが LISTEN 中**なのを確認済み。除外なしでは
   「ポート検知が動く」の実測が壊れた機能の実演になる。**症状②（WebView2 の借用 panic）は未着手**
3. **`<data_dir>/lid-guard.json` は #513 の共有カタログへ宣言が要る**（作法 3 のとおり踏んだ）。
   Local + 専用 note（共有すると別マシンの電源プラン GUID と元値を持ち込む）
4. **win467 版のテストはそのまま入れると危ない**: 単体テストが `TAKO_DATA_DIR` を差し替えるので
   同一バイナリの並列テストを巻き込む（#608 と同型）。記録 I/O をパス引数版へ切り出し、
   機械全体で 1 つしかない状態（電源要求・電源プラン・記録キャッシュ）を触るテストは
   `platform::testing::machine_state_lock()` で直列化した
5. **非 Windows の `imp::Guid = ()`** は clippy の `let_unit_value` で落ちる（macOS の `-D warnings`）。
   専用のサイズゼロ型にして境界の都合を呼び出し側の `allow` へ漏らさない
6. `power.rs` の doc にあった「Windows に蓋閉じ継続の仕組みは無い」は **#697 が実測で覆した前提**
   なので削除した

#### 実機実測（`ssh win`。session 1 へ `schtasks /it` で GUI を投げる）

| 観点 | 実測 |
|---|---|
| アイドル防止 | `mode=on` で `powercfg /requests` の SYSTEM に `[PROCESS] …\tako-app.exe`、`mode=off` で消える |
| busy 判定 | アイドルなシェルだけのときは**倒さない**（電源要求 absent・lid 0x00000001 のまま）。長時間の子プロセスを走らせると PRESENT へ |
| 蓋閉じ継続 | 稼働中に AC が `0x00000001 → 0x00000000`、記録は `{"scheme":"381b4222-…","ac":1,"dc":null}`。`powercfg /qh` の目でも一致 |
| 自動解除 | エージェントが終わると**自分で** `0x00000001` へ戻り記録も消える（#779 の 60 秒保険ぶんの遅れがある） |
| 残留復元 | `kill -9` で倒したまま落としても、次回起動で `0x00000001` へ戻る（persist.log に `lid-sleep: 蓋閉じ継続を解除` が残る） |
| ポート検知 | `tako list` が `pane 3 ports=[8123/node.exe]`。同時刻に psmux が **21 個** LISTEN しているが 1 個も報告されない |
| `tako sleep-guard install-lid-sleep` | Windows で成功（`この OS では追加の権限も登録も不要です`）。以前は osascript を起こして必ず失敗していた（#727 の症状 2） |

**注意（実機の作法）**: SSH セッションは session 0 で **DirectX デバイスが無く**、
そこから `tako-app.exe` を起動すると `Creating DirectX renderer` /
`DXGI_ERROR_NOT_CURRENTLY_AVAILABLE (0x887A0022)` で即 panic する。GUI が要る検証は
必ず `schtasks /it` 経由（作法 12）。また `TAKO_ISOLATED=1` は **persist を OFF にする**ので、
器（psmux）が要る検証では `TAKO_PERSIST=1` を明示する（ソケットは `tako-iso-<pid>` のまま隔離される）。
CLI は `TAKO_DISCOVERY_DIR=%TEMP%\tako-iso-discovery-<pid>` を指すと隔離 GUI へ届く。
`tako split` は tako の外から叩くので `--pane` が必須。

スライス 9 の道具は `C:\Users\shioz\dev\` に残してある（次スライスで使い回せる）:
`s9-launch.ps1`（schtasks から session 1 へ GUI を投げる。persist ON）/
`s9-drive.ps1`（SSH 側から CLI で駆動して観測）/ `s9-final.ps1`（受け入れ観点の通し）/
`s9-lidcycle.ps1`（蓋の倒す → 自動解除 → `kill -9` 残留 → 起動時復元の 4 段）。
採取物は `C:\Users\shioz\dev\tako-evidence-s9\`。

#### スライス 9 が残した宿題

- **#724 の症状②（「ブラウザで開く」で abort）は未着手**。wry の `build_as_child` が
  `wait_with_pump` で入れ子メッセージループを回し、GPUI の `App` 借用中に
  foreground runnable が再入して二重借用 panic → `extern "system"` を跨ぐので abort。
  WIP は `windows/724-port-crash` の `82d3dcb`（`webview.rs` の `CREATION_PUMPS_EVENT_LOOP` +
  `main.rs` の遅延生成キュー、計 260 行）
- **#727（設定画面のスリープ系が macOS 前提）は未着手**。ボタンが**必ず失敗する**症状は
  dispatch を `prepare_lid_control` 経由にしたぶん解消したが、文言（「Mac が眠って…」/
  「sudoers を登録」）と状態表示の欠落は残る。WIP は `windows/727-sleep-settings` の
  `5791a03`（`settings_sleep.rs` 新設 + 設定タブ、計 820 行）
- **`tako sleep-guard status` は CLI プロセス自身の状態を返す**（`sleep_guard_local` 経由で
  IPC を通らないため `assertion_held` / `busy_agents` が常に 0）。**macOS でも同じ**の
  main 由来の設計。実際の保持は `powercfg /requests`（Windows）/ `pmset -g assertions`（macOS）で見る
- **`setup` の対話フロー（L3 の蓋閉じ案内）は未移植**。`--check` の表示だけ能力ベースへ直した。
  win467 の該当箇所は `setup.rs` の 663〜731 行付近
- マトリクス（`tako_sleep_guard` / ポート検知）は**動かしていない**。スライス 8 の棚卸しで
  Supported / Degraded を決める材料として上の実測表を使う

### 後続 worker への引き継ぎ（2026-08-21 時点。スライス 1 / 2a / 2b / 3 / 4 / 5 / 6 / 7 / 9 完了）

main の到達点: `be55553`（1）→ `7cf97cb`（2a）→ `2947a19`（2b）→ `e947524`（3）→
`83bbdc0`（6）→ `015ef6d`（4）→ PR #860（5）→ PR #863（9）→ PR #855（7）。
**残りはスライス 8（棚卸し）だけ**（7b = PR #869 / 7c = PR #874 / 7c の後始末 = PR #878 /
agents 走査 = PR #882 で完了）。8 は 1〜7b のすべてに依存するので最後。

**スライス 5 が残した宿題**: ①実機セルフテストが項目 2（`TERM / COLORTERM 注入`）で止まるので
**スライス 7 完了後に通しで回す**こと → **スライス 7 完了時に実施済み（結果は 7 の完了記録）**
②#861（極端に狭い幅でメニュー行がコントロールと重なる）。
また、スライス 5 で **`tako` CLI が Windows で一切起動できなかった main 由来バグ**（1MB スタック超過）
を直したので、**以後のスライスは検証に「実バイナリの CLI を 1 回叩く」を必ず入れる**
（ユニットテストだけでは実バイナリの起動経路を踏まない）。

**スライス 4 が残した宿題**: #623（IME の未確定文字列が勝手に確定される）。
`platform::ime` の `is_associated` / `reassociate` / `guard_action` はスライス 1 で
main に入っており呼び出しを足すだけだが、`guard_action` の `refocus: !focus_held` は
**macOS でも発火する**ので、main が後から足した設定ウィンドウ・アップデートウィンドウ
（別 GPUI ウィンドウ）との相互作用を実機で確かめてから入れること。
`#[cfg(windows)]` で呼び出しを囲えば macOS は不変にできる。

**Windows 実機の注意（#856）**: debug ビルドの `tako.exe` は起動時に
スタックオーバーフローする（main 由来。`cargo test` では出ない）。CLI を実機で
叩く検証は `cargo build --release -p tako-cli`（約 8 分）が要る。

#### 毎スライスで守る作法（実測で効いたもの）

1. **Windows 実機ビルドを先に通す**。`#[cfg(windows)]` のコードは
   **macOS のゲートが全部緑でも実機で E0599 になる**（2a で 1 回踏み、2b / 3 では
   先に回して一発で通した）。順序: 実機 build → 実機 test → macOS 全ゲート
2. **`-j 2` と `CARGO_BUILD_JOBS=2`**。兄弟セッションと並行ビルドすると swap が枯れて
   `cc` が SIGKILL される（実測: swap 27.5/28.7 GB で `linking with cc failed: signal: 9`）。
   セルフテストは内側で `cargo build -p tako-cli` を起こすので `CARGO_BUILD_JOBS` も要る
3. **`#513` の共有カタログは fail-closed**。`<data_dir>` へ書くものを増やしたら
   `config_share/catalog.rs` へ分類（shared / local / secret）を宣言しないとテストが落ちる。
   win467 は #513 より前に分岐しているのでこの宣言を持っていない
4. **対応マトリクスは触らない**。機能が Windows で通しで動くまで Supported へ倒すと
   `PlatformFacts` 経由で system prompt へ誤情報が流れる（#516）。棚卸しはスライス 8
5. **plan の見立ては疑う**。2a では「#817 の `pty_loop` の上への再実装が要る」と書いたが、
   実測では #817 が置き換えたのは**読み取りループだけ**で書き込み経路は不変、
   win467 の実装がそのまま載った（2b で訂正）。スライス 3 も plan は 2 ファイルと
   書いていたが実際は 4 ファイル（`tako-cli` のクライアント側が要る）
6. **`git show "$W:path"` は波括弧で囲む**（`"${W}:path"`）。zsh の履歴修飾子 `:c` / `:r` が
   効いて壊れる（このセッションで踏んだ）
7. 隔離セルフテスト / visual-test は**ウィンドウが完全に隠れると描画が止まる**ので、
   項目 63 / 76d / 104 が SKIP になるのは既知（`TAKO_APP_SELF_TEST_OK` なら合格）。
   `#680` の項目は load 依存で落ちるので負荷が高いときは回し直す。
   **兄弟セッションが同時に GUI を立てていると前面を奪い合って SKIP / FAILED が増える**
   （スライス 6 で踏んだ = 相手の visual-test と重なって #737 の項目が落ちた）。
   `pgrep -fl tako-app` で相手の稼働を確かめ、単独で回し直してから原因を判断する
8. **`cargo fmt` は commit より前に回す**。`fmt --check` → `fmt` → そのまま commit だと
   整形結果が commit に入らず、CI のフォーマット検査だけが落ちる（スライス 6 で 1 回踏んだ）。
   `git status` が clean になってから push する
9. **保全ブランチから持ち込むときは「どの main から分岐したか」を先に見る**。
   `windows/525-shell-integration` は #600 / #614 / #816 / #513 より前から分岐していて、
   ファイルをそのまま `git checkout` すると**それらを巻き戻す**（スライス 7 で踏みかけた）。
   `git diff origin/main..<WIP> -- <path>` を機能単位で読み、持ち込む差分だけを選ぶ
10. **`scripts/check-windows.sh --all-targets` を渡すと Windows 専用の integration test も
   macOS で型検査できる**（素のクロスチェックは `--all-targets` を付けないので
   `#![cfg(windows)]` のテストファイルは見ていない）。作法 1 の前倒しに効く
11. **PowerShell を SSH 越しに叩くときは `-EncodedCommand`**（base64 の UTF-16LE）にする。
   入れ子の引用符が壊れるうえ、`[Console]::OutputEncoding` が shift_jis なので
   日本語出力が化ける。冒頭に `[Console]::OutputEncoding=[System.Text.Encoding]::UTF8;` と
   `$ProgressPreference="SilentlyContinue"`（CLIXML のノイズ抑止）を足しておく
10. **マトリクスを Supported / Degraded へ倒す前に #594 のリリースノートへの波及を見る**。
    `Support::Degraded` の note は `PlatformFacts` 経由で system prompt へ入るだけでなく、
    `known_limitations_markdown` 経由で**そのプラットフォームのリリースノート**にも出る。
    macOS 側に `Degraded` を 1 件でも作ると macOS のリリースノートに「既知の制限」節が生える
    （スライス 5 で `tako_menu` の macOS を Supported に倒し直した経緯）
11. **main のテストがプラットフォーム決め打ちでないかを疑う**。スライス 5 では main の
    menu テスト 3 本が `menus[0].name == "tako"` を無条件に要求しており、Windows で必ず落ちた。
    macOS のゲートは全部緑なので、**実機ビルド → 実機テストを先に回す**以外に検出手段が無い。
    **自分の変更で決め打ちを作り込むこともある**（7c で踏んだ）: 既定の関数が
    「動いている環境」を見るようにすると、それを呼ぶスナップショットテストが
    その場で決め打ちに化ける（実機で 22 件）。**環境を引数で受け取る `*_in` 版を分け、
    テストはそちらを呼ぶ**のが型。macOS のゲートは最後まで緑なので実機でしか出ない
12. **`$SHELL` を材料にするコードを SSH 越しに測ると「動いている」ように見える**（#877 で踏んだ）。
   このマシンの `SHELL` は **Process スコープにしか無い**（`User` / `Machine` はどちらも空）ので
   SSH セッションだけが持っている。GUI 起動の `tako.exe` には渡らないため、
   `Remove-Item Env:SHELL` を先に打たないと**壊れている経路が通ってしまう**
   （`SHELL=powershell.exe` のとき `-l -c "<前置き>; claude agents --json"` は前半が失敗して
   `;` の後ろだけ走る = 半分だけ動く）。実機で env 依存を測るときは
   **GUI 起動時の env を再現してから**測る

13. **GUI を実機で見るには `schtasks /it`**。SSH セッションは session 0（サービス）で、
    そこから起動したウィンドウは session 1 の対話デスクトップに出ず、`EnumWindows` からも見えない。
    `schtasks /create ... /it /rl highest` + `/run` で session 1 へ投げる。スクリーンショットと
    座標操作をするスクリプトは冒頭で `SetProcessDPIAware()` を呼ぶ（呼ばないと座標が仮想化されて
    クリックが外れる）。`Add-Type` の C# に `static Main` という名前のメソッドを書くと
    「エントリポイントの署名が違う」で**コンパイルが落ちる**ので別名にする。
    スライス 5 の道具は `s5-launch.ps1` / `s5-capture.ps1` / `s5-drive.ps1`（`C:\Users\shioz\dev\`）

#### 現在の Windows 実機ベースライン（`ssh win`。psmux 3.3.7 導入済み）

**最新の実測は main `c8c9fbb`（#877 の A/B で取った。所要 561 秒 / `-j 2`）**:

| スイート | 結果 |
|---|---|
| `tako-app` (lib) | 445 / **0** |
| `tako-cli` (lib) | 53 / **0** |
| `tako-control` (lib) | 1025 / **15 failed** |
| `tako-core` (lib) | 766 / **7 failed** |
| `platform_parity` | **12** / 0 |
| `encoding_conpty` | 5 / 0 |
| `psmux_backend` | 16 / 0 |
| `shell_integration_powershell` | 6 / 0 |

**失敗 22 件はすべて main 由来**（#583 の既知分 + 以降 main へ増えた同系。#867 / #873 の実測と同数）。
#877 では branch / main の両方でスイートを回し、**失敗テスト名まで `Compare-Object` で
完全一致（IDENTICAL）** を確認した = 新規ゼロ。スライスごとにこの表と突き合わせ、
増減があれば `TAKO_BACKEND=none` 等で「自分の変更が原因か」を切り分けてから報告する。
**件数だけでなく名前で突き合わせる**のが確実（同数のまま入れ替わることがある）:

```powershell
# ログから失敗名を抜いて比較する（両 HEAD で同じことをする）
$fails = @(); $in = $false
foreach ($l in (Get-Content <log> -Encoding UTF8)) {
  if ($l -match "^failures:$") { $in = $true; continue }
  if ($in) { if ($l -match "^\s{4}(\S.*)$") { $fails += $Matches[1] } elseif ($l.Trim() -ne "") { $in = $false } }
}
Compare-Object $branchFails ($fails | Sort-Object -Unique)
```

macOS 側のベースライン: `test --workspace` **2386 passed / 0 failed**（#877 時点。
スライス 5 後は 2228、#873 時点は 2377）/ visual-test **98 checkpoint** /
クロスチェック **エラー 0・警告 10**。

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
- **fresh worktree は `web/tako-remote/dist/` を持たない**（`.gitignore` 済み = 未追跡）。
  `rust_embed` の `#[folder = "../../web/tako-remote/dist/"]` が解決できず
  **tako-control のコンパイルが即失敗する**（`PwaAssets::get` の E0599 が連鎖）。
  `cargo test --workspace` を実機で回す前に `npm run build` するか、既存 worktree から
  `dist` をコピーする（#884 で踏んだ。**macOS のクロスチェックが緑でも落ちる**種類の失敗）
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
