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

### 後続 worker への引き継ぎ（2026-08-21 時点。スライス 1 / 2a / 2b / 3 完了）

main の到達点: `be55553`（1）→ `7cf97cb`（2a）→ `2947a19`（2b）→ PR #850（3）。
**残りはスライス 4 / 5 / 6 / 7 / 8 / 9**。依存グラフ上、いま着手できるのは
**4（入力系）**・**6（インストーラー）**・**7（シェル統合 PowerShell。1 と 2 が揃ったので解放）**・
**9（スリープ防止 / ポート検知）**。5 は 4 の後、8 は最後。

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
   `#680` の項目は load 依存で落ちるので負荷が高いときは回し直す

#### 現在の Windows 実機ベースライン（`ssh win`。psmux 3.3.7 導入済み）

| スイート | 結果 |
|---|---|
| `tako-app` / `tako-cli` | 409/**0** / 53/**0** |
| `tako-control` (lib) | 950 / **25 failed** |
| `tako-core` (lib) | 665 / **5 failed** |
| `platform_parity` | 10 / 0 |
| `encoding_conpty` | 5 / 0 |
| `psmux_backend` | 16 / 0 |

**失敗 30 件はすべて main 由来**（#583 の既知 18 + 以降 main へ増えた同系 7 + tako-core 5）。
内訳と根拠は #583 の 2026-08-21 のコメント。スライスごとにこの表と突き合わせ、
増減があれば `TAKO_BACKEND=none` 等で「自分の変更が原因か」を切り分けてから報告する。

macOS 側のベースライン: `test --workspace` **2194 passed / 0 failed** /
visual-test **98 checkpoint** / クロスチェック **エラー 0・警告 10**。

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
