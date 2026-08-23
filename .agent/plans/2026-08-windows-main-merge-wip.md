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
材料が OSC 133 の idle 検知 + `cat` なので **psmux 越しにシェル統合が届かない**（#766）と同根
—— **と当時は見立てたが、実際の原因は `$PROFILE` 依存と `cat` 決め打ちの 2 つ**だった
（器は無関係。#889 で根治し、いまの到達範囲は**項目 0〜93**。次の壁は項目 94 = #897）。

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
  `shell_integration::status().effective()` を材料にする項目が続く。~~**#766 が直れば一気に進む**~~
  → **この見立ては外れた**（#766 の完了記録を参照）。セルフテストは `TAKO_ISOLATED=1` が
  `TAKO_PERSIST=0` を立てるので**器なしのペイン**を測っており、器の素通しは関係なかった。
  実際の前提は「`$PROFILE` への配置」と「`cat` 決め打ち」で、どちらも **#889**
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
pane_current_path => C:\Users\winuser   （-c が `…\dir` になり存在しないので落ちた先）
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

`-c <cwd>` だけでなく **`-e KEY=<空白入りの値>` も同じ機序で壊れる**のが同時に直る
（実測: `-e "TAKO_SPACE=a b c"` が `show-environment` でそのまま読める）。
ただし `wrap_options` が `-e` へ載せるのは `TAKO_PANE_ID` / `TAKO_TAB_ID` の**数値だけ**なので、
こちらは**現時点の製品経路からは踏めない**（将来の空白入り値に対する予防）。
**空の引数**も同様: 素の連結では丸ごと消えるが、CRT 規則では `""` として保たれる。

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

##### 製品経路の実機 before/after（隔離 GUI + 実 CLI。persist ON = `backend: psmux`）

同じ隔離インスタンス構成で main（`9136942`）ビルドとブランチビルドを差し替えて測った
（別物であることは `Get-FileHash` で確認: `tako.exe` が `F532F95E…` / `9CFA65C4…`）。
**判定は「応答が返ったか」ではなく「+6 秒後もペインが居るか」**。

| 操作 | before | after |
|---|---|---|
| `tako tab new --cwd "…\prod dir with space"` | 応答は `{"tab":2,"pane":2,…}`・+1s に pane 2 → **+6s に消滅（DIED）** | **SURVIVED**・cwd が空白入りパスのまま |
| `tako tab new --cwd "…\prodplain"`（対照） | SURVIVED | SURVIVED |
| `tako run <空白入り dir のファイル>` | pane 4 生成 → **DIED** | **SURVIVED**・`__TAKO_EXIT=0`・cwd が空白入り |

before が Issue の記述（「応答は返るがペインごと消える」「死ぬまでに画面へは何も出ない」）と
一致している。`tako run` のペインに**プログラムの標準出力（`run-ok-884`）は出ない**が、
これは**空白と無関係**（空白なしディレクトリで同じ `tako:run:` を走らせても `__TAKO_EXIT=0`
だけが見える）= 実行ペインの描画の作りで、#884 の判定材料ではない。

**GUI を session 1 へ投げるときの注意**: `TAKO_ISOLATED=1` は discovery dir を
**pid 由来**（`%TEMP%\tako-iso-discovery-<pid>`）にするので、CLI 側でも
`TAKO_ISOLATED=1` を立てると**別のディレクトリを見て接続できない**。CLI には
GUI の pid から `TAKO_DISCOVERY_DIR` を明示的に渡す。
また**隔離インスタンスは名前付きパイプの primary 名 `\\.\pipe\tako-<user>` を取る**ので、
他の worker が GUI を立てていると相手を secondary（= 復元スキップ）へ落とす。
session 1 は先着と直列に使うこと（今回 1 回踏んで #766 の worker に測り直してもらった）。


##### 実機実測（psmux 3.3.7 / Windows 11）

| 観点 | before（`escape_args` 既定） | after |
|---|---|---|
| `空白を含む引数が1語のまま子へ届く` | **FAILED**（`ARGC=1` にならない） | ok |
| `器ありでも空白入りcwdのペインが生き残る` | **FAILED**（現れたあと消える） | ok |
| `空白を含むenvの値も1語のまま器へ届く` | —（`-e` は製品経路から踏めないので予防） | ok |
| `cargo test --workspace`（実機） | 22 件失敗 | **22 件失敗・失敗テスト名の集合が `diff` で完全一致** |

macOS 側: `test --workspace` **2406 passed / 0 failed** / `fmt` / `clippy`（両 feature）/
クロスチェック **エラー 0・警告リストが main と完全一致**。
番犬（`spawnはargvの組み直しを境界へ委ねる`）は境界呼び出しを外すと FAILED になる。

##### 残っている隣接の穴（別件）

alacritty の `cmdline()` は **`program` を一切エスケープしない**
（`cmd.push_str(&shell.program)`。`escape_args` の対象外）。空白入りの
プログラムパスは `CreateProcessW` の「空白区切りを順に試す」探索に救われて
いるだけなので、`C:\Program.exe` のような細工があると取り違えうる。
本 Issue の症状（cwd）とは層が別なので触っていない。

#### #870 の記録（ホーム解決の一本化。PR で追記・2026-08-22）

**症状**: ターミナルに出た `~/…` 起点のパスが Windows でリンクにならない
（⌘（Ctrl）+ホバーの下線も ⌘+クリックも無反応。絶対パスは効く）。

**原因**: ホーム解決が 2 か所にあり、`links.rs` の `dirs_hint()` が **`HOME` 決め打ち**。
Windows は `HOME` を持たない（`USERPROFILE` が正）ので必ず `None` になり、
`resolve_path` の `~/` 分岐へ到達しても解決できなかった。
同じ意味論は `terminal.rs` に**正しい形で**あった = **書いた場所が 2 つあると片方だけ直る**。

**直し方**: `paths::home_dir()`（判断部は純粋関数 `home_from`）へ一本化し、
`links.rs` / `terminal.rs` は両方そこへ委譲。**`cfg` は持たない**（`HOME` → `USERPROFILE` の
順で見ればどちらの OS でも正しいので `platform/` の境界を増やす必要がない）。
番犬 `ホーム解決の入口がpathsだけである` が、この 2 ファイルから env を直読みする形へ
戻るのを止める（**走査するのは env を読む形だけ**。散文の `USERPROFILE` は拾わない
= 最初はスキップ用のメッセージ文言を誤検知して自分の番犬に落とされた）。

##### 測り方（ここが本題）

**`HOME` を外してから測る。** このマシンの `HOME` は **SSH セッションの Process
スコープにだけ存在**する（実測: `HOME(User)` / `HOME(Machine)` はどちらも空、
`HOME(Process)=C:\Users\winuser`）。GUI 起動の tako には渡らないので、
**SSH でそのまま `cargo test` すると `dirs_hint()` が動いて見えて壊れていることが分からない**
（#877 の作法 12 とまったく同型。あちらは `SHELL`、こちらは `HOME`）。

A/B は**修正だけを戻す**形にした（`dirs_hint` の中身だけ `HOME` 決め打ちへ差し替え、
テストは修正版のまま）。こうすると失敗の原因が**テスト自身の env 読み**ではなく
**製品側の解決経路**に限定される:

| `links::tests::detect_tilde_path`（`HOME` 削除・`USERPROFILE` のみ） | 結果 |
|---|---|
| before（`dirs_hint` = `HOME` 決め打ち） | **FAILED**: `left: 0` / `right: 1` = `~/…` が 1 本も解決されない |
| after（`dirs_hint` = `paths::home_dir()`） | **ok** |

同じ実行の他の links テストのうち `cwd不明でも絶対パスとホーム起点は検出する` /
`detect_absolute_path` / `tuiの装飾付きsoft_wrapをまたぐパスを検出する` は
**after でも FAILED のまま**。これは `/`-絶対パス前提（**#522** の担当範囲）で
ホーム解決とは別の理由 = 既存 22 件ベースラインに含まれる 3 本と同一。

##### セルフテスト 69c の位置づけ（誤解しやすい）

`~/` の判定と期待値は共通解決へ寄せたので、**`HOME` の有無ではなく「実際に解決できるか」**で
必須／スキップが決まる。ただし **69c ブロック自体は `MAIN_SEPARATOR == '/'` でゲートされていて
Windows では走らない**（`links.rs` のパス検出が POSIX 形前提。`C:\…` / UNC / 区切り文字は
**#522 の受け入れ条件 1**）。したがって #870 で Windows の 69c が緑になるわけではなく、
**#522 が入った時点でゲート無しに通る状態**にしてある。

##### 棚卸しで分かったこと（#893 へ起票）

ワークスペース全体を見るとホーム解決は**他に 15 箇所**ある:
正しい形（`HOME` → `USERPROFILE`）で既に書かれている **10 箇所**（同じ 2 行の重複）/
`~` 表示短縮の `HOME` 決め打ち **7 箇所**（Windows では短縮されないだけ）/
実解決の `HOME` 決め打ち **2 箇所**（`ssh_config.rs` は `~/.ssh/config` が読めない）/
**触ってはいけない 2 箇所**（`paths.rs` の `default_data_dir` は各 OS の
データディレクトリ規約としてその OS の変数を読んでいる。`home_dir()` へ寄せると
macOS が `USERPROFILE` も受け入れる挙動変更になり利得がない）。
番犬の走査範囲を広げるのは #893 の作業。

##### 併せて直した

- **空の `HOME` が `USERPROFILE` を隠さない**ようにした。統合前の `terminal.rs` 版は
  `home.or(userprofile).filter(空でない)` の順で、`HOME=`（空）が立っていると
  USERPROFILE を見ずに `None` へ落ちていた（= 本 Issue と同じ症状）
- `links.rs` のユニットテスト 2 本が `std::env::var("HOME").unwrap()` で **Windows では
  panic**（#583 の POSIX 前提失敗の一部）。共通解決 + 理由つきスキップへ

#### スライス 8 の前提: 器の中のシェル統合（#766）— ✅ **完了**（PR は本文末尾の記録）

Windows の既定構成（persist ON = 器が psmux）でシェル統合（OSC 7 / 133）が**まったく働かない**。
ペインのドットが待機中 / 実行中 / 失敗にならず cwd 追従も死ぬ = **セッション完全復元を
使っている人ほど効かない**。器なしのペインでは完全に動く。

##### 器の側では直らない（upstream のソースで確定した）

起票時の実測（素の OSC・DCS の ESC 二重化あり / なしの 3 形すべてが外へ出ず、同時に流した
平文だけが届く）に、**upstream の確認**（`psmux/psmux`。MIT / Rust / crates.io。
2026-08-21 時点の master と v3.3.8）を足して原因を確定した:

| 調べたこと | 結果 |
|---|---|
| `allow-passthrough` を読む側があるか | **無い**。`option_catalog.rs` / `options.rs` の get / set と config パースにしか現れない = tmux 設定互換のスタブ |
| DCS の tmux 形式（`Ptmux`）の実装 | **リポジトリ全体で 0 ヒット** |
| v3.3.8（2026-08-18）の changelog | 該当項目なし = **版数を上げても直らない** |
| 私用 OSC で抜けられるか | **抜けられない**。psmux は「パースして画面モデルへ落とし、クライアントへ描き直す」多重化器で、OSC 8 すら**再直列化**して届けている（upstream #567）。画面モデルに置き場の無いバイト列は原理的に出ない |
| 器が持っている材料 | OSC 7 → `#{pane_current_path}`（#495 / #539）、OSC 133 A/C/D → コマンド名（#469）。**終了コードは持っていない** |

つまり素通しは upstream の新機能が要る話で、しかも **psmux は tako が配っているものではない**
（winget / cargo で各自が入れる）。tako 側で閉じる必要がある。

##### 入れたもの: 側路（`tako_core::osc_sink`）

運ぶのは**解釈済みの状態ではなく OSC バイト列そのまま**で、解釈は PTY 経路と同じ `osc_tap` に
通す。状態機械が 1 本のままなので「macOS では Failed(3) だが Windows では Idle」のような
分岐が構造的に起きない。器の材料（`#{pane_current_path}` 等）を使う案は**終了コードが取れない**
ので採らなかった。

- 器が素通ししないときだけ `TAKO_OSC_SINK=<data_dir>/osc/<pane>.osc` をペインへ注入。
  **`backend::PANE_SCOPED_ENV`**（tmux / psmux が共有する表）へ載せた — 片方だけ足すと
  「tmux では効くが psmux では効かない」という追いにくい差になる
- `tako.ps1` は書き先があれば OSC を**束ごと 1 ファイルへ差し替える**（`133;D` + `133;A` +
  OSC 7 を 1 回で。個別に書くと最後だけ残って**終了コードが消える**）。`.new` へ書いて rename
- tako は定期更新で「中身が前回と変わっていたら」`TerminalSession::feed_osc_bytes` へ通す。
  **側路を持つペインが無ければ即 return** なので macOS / tmux のコストはゼロ
- **器の能力申告（`osc_passthrough`）は変えていない**。psmux が素通ししないのは事実のままで、
  変わったのは tako が素通しに依存しなくなったこと。経路は
  `shell_integration::osc_transport()`（`pty` / `side-channel`）で読める

##### 実機実測（`ssh win`。session 1 へ `schtasks /it`）

**criterion 1 = 製品経路**（CLI → IPC → GUI → ペイン → psmux の器）で before / after:

| 観測 | main `dc975df` | 本ブランチ |
|---|---|---|
| 起動後の `state` | `unknown` | **`idle`** |
| `cmd.exe /c exit 3` の後 | `unknown` / `exit_code` なし | **`failed` / `exit_code = 3`** |
| `cd` の cwd 追従 | `C:\Users\winuser`（動かない。区切り `\` = spawn 値） | `C:/Users/winuser` → **`C:/Users/winuser/dev`**（区切り `/` = OSC 7 由来） |
| `tako shell-integration` の警告 | `[警告] 永続バックエンド（psmux）が…働かない` | **出ない** |
| 側路ファイル | 無し | `iso/osc/2.osc` 57 バイト |

側路の中身をバイト単位で確認したもの（束が 1 個 = 上書き方式が効いている）:

```
<ESC>]133;D;0<BEL><ESC>]133;A<BEL><ESC>]7;file:///C:/Users/winuser/dev<BEL><ESC>]133;B<BEL>
```

**副産物の実測**: セルフテストの項目 41 / 41b のスキップ理由が
`blocked_by_backend=Some("永続バックエンド（psmux）が…働かない")` から **`None`** へ変わった
（= 器の素通しへの依存が消えたことがテスト側の診断にも現れた）。残る `installed=false` は
`TAKO_ISOLATED` の data_dir 隔離と `$PROFILE` が指す本番パスの食い違いで、#889 へ追記した。

統合テスト（実 psmux + 実 pwsh）は **7/7 緑**（うち 1 本は `data_dir` にあえて空白を入れた
ケース = `-e` に空白入り値が流れる最初の経路。#887 の根治がそのまま効いている実測）。既存の
`器の中では統合が読み込まれてもoscが外へ出ない` は**そのまま緑** = 素通しは直っていないことと
側路が届けていることを同時に固定する。実機の `cargo test --workspace` は **branch 22 / main（`c210aad`）22 で失敗テスト名まで
`Compare-Object` が IDENTICAL**（新規ゼロ）。

##### criterion 2（項目 93 の到達範囲）: 止まっていた原因は **2 つとも #766 の射程外**だった

plan の見立て（「項目 93 以降は `effective()` を材料にするので #766 が直れば一気に進む」）は
**外れていた**。診断行が決め手:

```
TAKO_SELF_TEST_694: pane=48 state=Some(Unknown) alt=Some(false) role=None backend=None busy=None
```

**`backend=None`** = 器が絡んでいない。セルフテストは `TAKO_ISOLATED=1` が `TAKO_PERSIST=0` を
立てるので**器なしのペイン**を測っている。器なしなら OSC は元から PTY で通る。

1. **`$PROFILE` にシェル統合が未配置**（実機の環境前提）。`tako shell-integration install` を
   実行しただけで項目 93 の (c)（判定表）は通過した
2. その次の (d) は `split_pane(…, vec!["cat"])` の **`cat` 決め打ち**で止まる。Windows の `cat` は
   PowerShell のエイリアスのみ（実体なし。`CreateProcess` が失敗）→ 対象ペインが即死 →
   スターターの配送先が消える

**main（`9136942`）とブランチで同じ (d) に止まる**ことを A/B で確認（170 行 / 176 行）=
**到達範囲は同一で、新たに止めた項目はゼロ**。1 / 2 はテスト側の前提なので **#889 に起票**した。

##### 兄弟セッションとの並行（#881 / #884）

- **psmux 本体もリリース物も触っていない**ので #881 とは衝突しない（`backend/psmux.rs` への
  変更は env 許可リストの 1 行だけ）
- **#884（PR #887）が前提**だった。`-e TAKO_OSC_SINK=<path>` は `-c <cwd>` と同じ露出で、
  `escape_args` が false のままだと**空白入りパスが 3 語に割れる**。しかも `-e` はそれまで
  `TAKO_PANE_ID` / `TAKO_TAB_ID` の**数値だけ**だったので、#766 が
  **`-e` に空白入り値を流す最初の経路**になる = 「#887 が無いと新規に壊れる」関係。
  #887 のマージ後に rebase し、統合テストの `data_dir` を**あえて空白入り**にして固定した
- 実機の名前付きパイプ（`\\.\pipe\tako-winuser`）は先着が primary を取るので、**兄弟が
  隔離 GUI を立てている窓で測ると secondary 扱いになる**（復元スキップ）。#884 の worker が
  時刻つきで知らせてくれたので、その窓に当たった 1 本を測り直した。**session 1 は 1 本ずつ**

##### 「逆向きの固定」が要る修正だった（型として残す）

側路は「psmux が素通しするようになった」と誤読されやすい。そこで
**既存テスト `器の中では統合が読み込まれてもoscが外へ出ない` をそのまま緑に保つ**ことで、
「素通しは直っていない」と「側路が届いている」を**同時に**固定した。片方だけ見ると
原因の理解がずれる修正では、この 2 本立てが説明にも回帰検出にも効く。
#870（`HOME` 決め打ち）でも同型が要った（「`HOME` を外すと落ちる」+
「`HOME` があるときは従来どおり緑」）ので、Windows 系の env 依存の修正では既定の型にしてよい。

##### 側路が `HOME` の罠に露出していないことの確認（#870 / #893 と併せて）

`paths::default_data_dir()` の Windows 分岐は `%APPDATA%` →（無ければ）
`%USERPROFILE%\AppData\Roaming` で、**`HOME` を一切見ない**。`TAKO_OSC_SINK` は
そこ由来なので、SSH セッションにしか存在しない `HOME`（#870 の根因）には依存しない。
ただし `USERPROFILE` フォールバックがあるので**ユーザー名の空白**は通るため、
#887 依存はそのまま残る（空白入り `data_dir` のケースで固定済み）。
ホーム解決の残り 15 箇所の分類は **#893**
#### #872 の記録（ウィンドウ 0 枚の無音終了。PR で追記・2026-08-22）

**症状（Issue の書き方）**: 「2 枚目のウィンドウを作るとアプリが終了コード 0 で静かに終わる」。
セルフテストは `TAKO_SELF_TEST_77: 開始` の次の行が無く、項目 78 以降が 1 つも測れない。

**Issue の前提は外れていた**。`check` は**成功時に何も印字しない**ので、
「開始の直後に走るのは `window new` だけ」という推論が成り立たない。実測すると
2 枚目の生成は**元から通っていた**:

```
TAKO_SELF_TEST_77: 開始 windows=(1, 1)
TAKO_SELF_TEST_WINDOW_OPEN: ウィンドウ open: logical=2 gpui 枚数=2
TAKO_SELF_TEST_77: 2 枚目 registered=true pty=true drawn=true   ← 作れて描けている
...
TAKO_SELF_TEST_WINDOW_CLOSED: … 残り gpui=0 論理=1 → …アプリを終了する（#872）
EXITCODE=0                                                       ← ここで死ぬ
```

**真因**: GPUI の `QuitMode::Default` は **`cfg!(not(target_os = "macos"))` = 非 macOS で
「最後のウィンドウが閉じたらアプリ終了」**（`crates/gpui/src/app.rs` の `update_window` →
`trail` → `quit_on_empty`）。終了は `PostQuitMessage(0)` → **`ExitProcess(0)`** なので
panic でも FAILED でもなく、tako 側のログにも痕跡が残らない。
死んでいたのは項目 77 ではなく**項目 79（macOS 固有の Dock 復帰）が窓を 0 枚にした瞬間**で、
旧コードは 77 / 79 / 80 を 1 つの `if cfg!(windows)` でまとめてスキップしていたため
「77 で死ぬ」に見えていた。

**直し方**: 寿命の方針を UI ツールキットから取り上げて tako が持つ。

- 境界 `platform::window_lifecycle`（`LastWindowClose::{KeepAliveForReopen, Quit}`）に
  「最後のウィンドウが閉じたらどうするか」を 1 か所だけ置く。判定は純粋関数なので
  **macOS 上から Windows 側の方針を検証できる**（`support` と同じ作法）
- `cx.set_quit_mode(QuitMode::Explicit)` で自動終了を止め、実行は
  `handle_window_close`（= ユーザーの ✕ / Alt+F4 の経路）だけが行う。
  これで「ユーザーが最後の窓を閉じた」と「tako が内部都合で 0 枚にした」が分かれる
- `on_window_closed` で **0 枚になった瞬間を必ず 1 行残す**（`viewport_closed_log`）。
  `open_viewport_window` も成否と枚数を残す。**次に同じ経路を踏んだ人が黙って溶かさないため**
- 「最後の 1 枚」判定を `cx.windows().len()`（= 設定画面・アップデート画面まで数える）から
  **`self.viewports.len()`（tako が見せている窓の数）**へ。前者だと「設定画面を開いたまま
  最後のタブ窓を閉じる」が最後の 1 枚扱いにならず、そのあと設定画面を閉じると
  Windows で終了も再表示もできないプロセスが残る

**A/B は同一バイナリで取れる**: `TAKO_872_NO_QUIT_GUARD=1` が旧挙動（GPUI の既定）。

##### 実機実測（Windows 11 / debug / `TAKO_ISOLATED=1`）

| 観点 | before（`TAKO_872_NO_QUIT_GUARD=1`） | after |
|---|---|---|
| セルフテストの停止位置 | 項目 79b の 0 枚化で **EXITCODE=0**（無音） | **項目 93（#694）** = main と同じ |
| 項目 77（2 枚目） | 実は通っていた（`registered=true pty=true drawn=true`） | 同じ |
| 項目 79b（内部都合の 0 枚） | プロセスが消える | `内部 close 後 gpui 枚数=0` → `開き直し後 Some((1, EntityId(1v1), 2))` |
| `tako window new`（GUI + CLI） | **落ちない**（2 窓・`gpui 枚数=2`・送達と read も通る） | 同じ |
| session 1 の可視ウィンドウ | `974x607 title=[tako]` が **2 枚** | 同じ |
| 最後の窓を ✕（`WM_CLOSE`） | 終了する（GPUI の自動終了） | 終了する（**tako が明示。persist.log に方針つきで残る**） |
| `cargo test --workspace`（実機） | 22 件失敗 | 22 件失敗・**失敗テスト名の集合が完全一致** |

macOS 側: `test --workspace` **2411 passed / 0 failed**（main 2406 + 新規 5）/ `fmt` /
`clippy`（両 feature）/ クロスチェック **エラー 0・警告リストが main と完全一致** /
隔離セルフテスト **`TAKO_APP_SELF_TEST_OK`（完走）**。

##### 実測で分かった作法（次に踏む人向け）

1. **`check` は成功時に黙る**。だから「最後に出たログの直後の処理が犯人」は成り立たない。
   同じ罠を封じるため、0 枚化の瞬間そのものに診断を足した
2. **`cfg!` のスキップは 1 項目ずつにする**。77 / 79 / 80 を 1 つの `if` でまとめたせいで、
   「79 が原因」が「77 が原因」に見えた。スキップ理由も項目ごとに書く
3. **entity の寿命はプラットフォームで違う**。窓が 0 枚になると最後の強参照
   （ウィンドウの root view）が落ちるので、**Windows では `TakoApp` entity ごと解放される**
   （実測。macOS は残る = #381 の「同一 entity で開き直す」設計は macOS の retain に
   依存している）。解放されると `reopen_or_restore` は保存レイアウトから**別の TakoApp** を
   作る側へ落ちるので、0 枚を跨ぐ検証はテスト側で entity を掴んで測る対象を固定する
4. **0 枚化は production と同じ後始末順で作る**（`drop_viewport` → `remove_window`）。
   `remove_window` だけだと `viewports` に古い組が残り、`reopen_or_restore` が
   「もう開いている」と誤認して開き直さない（macOS で 1 回踏んだ）
5. **session 0（SSH）から session 1 のウィンドウは列挙できない**。`EnumWindows` /
   `MainWindowTitle` は空を返すので、「本当に画面に出ているか」は `schtasks /it` で
   session 1 に**プローブを投げて**測る。道具は `C:\Users\winuser\dev\tako-evidence-872\` に
   残してある（`winprobe.ps1` = 可視ウィンドウの列挙 / `wmclose.ps1` = ✕ 相当の `WM_CLOSE` /
   `st-after.cmd` `st-before2.cmd` = セルフテストの A/B / `gui.ps1` = 隔離 GUI の起動）
6. **`Start-Process` で投げたビルドは SSH セッションが切れると死ぬ**。長い処理は
   `Invoke-CimMethod Win32_Process Create`（ジョブの外に出る）か `schtasks` で投げる

##### 副産物（重い方）: 途中で死んだ run が「OK + 終了コード 0」を出していた

`on_app_quit` の `TAKO_APP_SELF_TEST_OK` は「全 check 通過後にだけ quit が来る」前提だった。
ところが **quit 経路は最終項目の cmd-q 以外からも通る**（ウィンドウ 0 枚の自動終了・
最後のタブの close）ので、#872 の無音終了は条件次第で**偽の緑**になる。Windows 実測:

```
TAKO_SELF_TEST_WINDOW_CLOSED: … 残り gpui=0 … → …アプリを終了する（#872）
TAKO_APP_SELF_TEST_OK      ← 項目 79b で死んでいるのに OK
EXITCODE=0
```

つまり「`TAKO_APP_SELF_TEST_OK` なら合格」という運用の前提が、**この経路では成り立って
いなかった**。前提を明示のラッチ（`SELF_TEST_AT_FINAL_STEP`）にして、立っていない quit は
`TAKO_APP_SELF_TEST_FAILED` + exit 1 で落とすようにした（番犬つき）。修正後の同じ before は
`最終項目より前に quit した` + `EXITCODE=1`。

##### 実機テストの差分は 1 件だけで、それは #766 の負荷依存フレーク

`cargo test --workspace --no-fail-fast`（実機）は main = 22 件失敗、本ブランチ = 23 件。
増えた 1 件は `psmux_backend.rs` の `器のホイールは上下対称で最下部でcopy_modeを抜ける`
（#766 で新設）。全体走行では 67 秒かかって
`遡るための履歴が作れない`（貼り付けた `1..80 | ForEach-Object { "LINE $_" }` が途中で切れて
PowerShell が継続行 `>>` に入る）で落ちるが、**単独で 3 回連続 pass（各 6 秒）**。
負荷で送達が崩れる #640 と同型なので **#896 に起票**した。私の変更は tako-app の
ウィンドウ寿命と tako-core の新モジュールだけで、psmux 経路には触っていない。

##### 副産物: 項目 81 は #381 以降ずっと空振りしていた

項目 79 でウィンドウを開き直すとハンドルが差し替わるのに、取り直しが**項目 81 の後ろ**に
あった。項目 81 は `setup_ok` が false になるだけで**何も検証せずに素通り**していた
（`if setup_ok { … }` なので FAILED にもならない）。取り直しを 81 の前へ移し、
前提が崩れたら FAILED にした。

#### #889 の記録（セルフテスト項目 93 の 2 原因。PR #900・2026-08-22）

**症状**: 隔離セルフテストが項目 93（#694 GUI ライク表示モードの判定）で必ず止まり、
**93 以降（GUI モード / チャット / 設定画面 / limit-resume）が 1 つも走らない**。
2 原因ともテスト側で、#889 の切り分けどおりだった。

##### 原因 1: `cat` を argv リテラルで直書きしていた

`Some(vec!["cat".into()])`。**Windows の `cat` は `Get-Content` のエイリアスで実体が無い**うえ、
Windows の `login_shell_command` は argv を包まずそのまま `CreateProcess` へ渡すのでペインが即死する。
判定側は消えたペインでも既定の `Terminal` が成り立つため「実行中のペインは据え置き」は
**通ってしまい**、送達検証だけが配送先を失って落ちていた。
→ `ShellDialect::echo_stdin_command()`（POSIX は `["cat"]` のまま / PowerShell は
標準入力を 1 行ずつ読んで書き戻すループ）。同じ直書きが項目 97（#720）にもあったので一緒に寄せた。

##### 原因 2: 素のシェルペインが実機の `$PROFILE` に依存していた

スターターの前提「アイドルシェル」は OSC 133 の Idle で決まる。unix は spawn 時の env 注入で
完結するが、PowerShell は `$PROFILE` 経由で、セルフテストは `TAKO_ISOLATED=1` で data_dir を
隔離するため **`status()` が見る `<隔離 data_dir>/shell-integration/tako.ps1` と `$PROFILE` が
指す本番のパスが別物**になる = **実機の配置状態でテストの結果が変わる**。
→ `ShellDialect::integration_shell_command()`（統合を自分でドットソースした対話シェル。
`-NoLogo -NoProfile -NoExit -Command . <script>` = `tests/shell_integration_powershell.rs` と同形）。
POSIX は `None` = 既定シェルをそのまま起こすので従来と同じ経路。

##### 実機 A/B（4 本。`$PROFILE` の状態も変数にした）

この機は #766 の検証で `tako shell-integration install` 済みなので、**原因 2 は
「配置済み」の状態では隠れる**。本番スクリプト（`%APPDATA%\tako\shell-integration\tako.ps1`）を
リネームして隠すと「未配置の実機」を再現できる（`$PROFILE` のブロックは `Test-Path` で
守られている）。

| アーム | 本番スクリプト | 結果 |
|---|---|---|
| main `551fa0b` | 配置あり | **FAILED**（93 (d) `…tako master が届く`）= 原因 1 |
| main `551fa0b` | 隠す | **FAILED**（93 (c) `判定表: アイドルシェル → スターター`。診断 `state=Some(Unknown) backend=None`）= 原因 2 |
| branch `f41e26e` | 隠す | **項目 93 全通過** → 次の停止は項目 94 |
| branch `f41e26e` | 配置あり | **項目 93 全通過** → 同じく項目 94 |

診断行（そのまま証拠になる）:

```
selftest 93: shell=Some("…\PowerShell\7\pwsh.exe")
  script=Some("…\Temp\tako-iso-data-14016\shell-integration\tako.ps1")
  integration_shell=Some([pwsh, -NoLogo, -NoProfile, -NoExit, -Command, ., <script>])
  echo_stdin=["powershell", "-NoProfile", "-Command", "while ($true) { … ReadLine() … WriteLine …}"]
selftest 93d: launch_line="tako master" expected="tako master"
```

##### 次の壁は項目 94（#702 alt screen）= **Enter を LF で送っている**（#897 へ起票）

```
TAKO_SELF_TEST_702_ALT2: inner_alt=false backend=None
  tail="… C:\Users\winuser>>|> Write-Host -NoNewline "$([char]27)[?1049h"; Start-Sleep 3600"
```

`>>` は PSReadLine の継続行プロンプトで、書き込んだコマンドは**確定していない**。
端末の Enter は CR で、PowerShell は素の LF を継続行の開始と解釈する（#766 の注記と同じ実測）。
セルフテストの他の PTY 直書きは既に `\r` を使っており、ここだけ `\n` が残っていた。
**実機テストの失敗 1 件（`psmux_backend::copy_mode滞在中の打鍵がin_band解除で届く`）も同じ原因**なので、
直せばセルフテストが 94 以降へ進み、実機テストの失敗も 23 → 22 件へ戻る。

##### 実機ベースラインの更新: **22 → 23 件**（main 側の変化）

main `551fa0b` と branch `f41e26e` の両方で `cargo test --workspace --no-fail-fast` を回し、
**失敗テスト名が `Compare-Object` で IDENTICAL**（before=23 / after=23）= 新規失敗ゼロ。
増えた 1 件は上記 psmux の e2e（#583 へコメント済み）。
`tako-app` 446 → 448 / `tako-core` 795 → 797 の差は #889 が足したテスト 4 本。

##### 見つけた製品バグ（この PR では直さない）

- **#898**: `dispatch::which` が POSIX 専用の `which` コマンド決め打ちで、Windows では常に `None`
  （実測: `which tako` は「認識されません」/ `where tako` は解決する）。`resolve_tako_binary()` が
  裸の `tako` へ落ち、**stale claude バイナリ検知（#498）は常に無効**。境界 **B16
  （`platform::exe::find`）** へ寄せる話
- **#899**: スターター（#694）/ welcome バナー（#549）のコマンド投入が LF + POSIX クォート。
  LF が行を確定しないことは項目 94 の診断で実測済み。`starter_action` は GUI クリック専用で
  CLI / MCP から叩けないため（設計原則 5 の穴）カードそのものの実測はできていない

##### 作法として残すもの

- **番犬テスト `selftest_pane_command_watchdog`**: セルフテストがペインの起動コマンドを
  argv リテラル（`command: Some(vec![…])` / `split_pane(app, cx, Some(vec![…]))`）で組んでいたら
  ソース走査で落とす。パターンは `concat!` で分割して書く（番犬自身のソース行が検査対象に入るため）
- **テストが製品の組み立てを決め打ちしない**: 項目 93 (d) の期待値は
  `welcome::launch_command_line` から作る（macOS の実測値は従来と同一文字列）。
  決め打ちのままだと #898 を直した瞬間にテストが壊れる
- **項目 21（`tako title / role 設定`）は固定待ち 800ms で高負荷時に落ちる**（1 回踏んだ。
  再実行で通る）。#796 の作法から漏れている 1 件

#### #897 の記録（PTY へ書く Enter を CR へ。PR #901・2026-08-22）

**症状**: #889 で項目 93 が開いた直後の壁。隔離セルフテストが項目 94（#702 alt screen）で
必ず止まり、**94 以降が 1 つも走らない**。Issue の切り分けどおり原因はテスト側 1 か所。

##### 原因: Enter を LF で書いていた

端末が Enter として送るのは **CR**。素の LF は PSReadLine が**継続行（`>>`）の開始**と
解釈するのでコマンドが確定しない。POSIX 側は tty の ICANON + ICRNL が CR も LF も改行へ
倒すので、**CR に寄せれば両方の方言で通る**（方言差ではないので `ShellDialect` ではなく
`self_test::pty_line`（本文 + CR）に置いた）。残っていた LF は 6 か所
（項目 94 / 項目 95c の `claude` 起動 / visual-test のカーソル形状・非表示・復帰・`clear`）。

##### 実機 A/B（同じ worktree・同じ手順で HEAD だけ替えた）

| アーム | HEAD | 結果 |
|---|---|---|
| main | `eac860a` | **FAILED**（項目 94）。到達 = 94 |
| branch | `b003965` | **項目 94 通過** → 次の停止は**項目 100（#737）**。到達 = 100 |

main 側の診断（`tail` は逆順の末尾 80 文字）:

```
TAKO_SELF_TEST_702_ALT2: inner_alt=false backend=None
  tail="0063 peelS-tratS ;\"h9401?[)72]rahc[($\" enilweNoN- tsoH-etirW >>|>zoihs\\sresU\\:C "
TAKO_APP_SELF_TEST_FAILED: alt screen: tmux クライアントに騙されず、実 alt screen は据え置き (#702)
```

読み下すと `C:\Users\winuser>` の次の行が `>> Write-Host -NoNewline "$([char]27)[?1049h"; Start-Sleep 3600`
= **PSReadLine の継続行プロンプトで、書いたコマンドが確定していない**。
branch 側ではこの診断行が**そもそも出ない**（= 判定が通った）。

##### 94 の先で初めて Windows を通った項目（branch 側のログ。すべて緑）

```
97-SETTLE: sequence=[Preparing, Starter] reached_starter=true                  ← #720 準備中
TAKO_SELF_TEST_725_INDEX / _SELECT / _COPY / _MCP / _LONG / _SCROLL            ← #725 チャット選択・コピー
TAKO_SELF_TEST_739_PROFILES / _LAUNCH / _CTX                                   ← #739 起動カードのプロファイル
```

項目 94（#702）・95（#716）・96（#721）・97（#720）・98（#725）・99（#739）が
**Windows で初めて走って通った**。

##### 次の壁は項目 100（#737 チャット入力欄）= **#903 へ起票**

**#897 の LF ではない**（製品の `Send` は `normalize_newlines_for_keys` で CR へ倒している）。
`paint_and_hold` が組む PowerShell コマンド自体も実機の pwsh 7 で
**そのまま構文を通って箱を描く**ことを確認済み（`Invoke-Expression` で PARSE_OK・
罫線と `❯` と ESC 列が出た）。

`got=Some(None)` だけでは切り分けられなかったので、**この PR で診断行に画面末尾 6 行と
`pane_display_for` を足した**（#796 の作法）。取り直した結果が決定的だった:

```
TAKO_SELF_TEST_737: expected="Try \"how does <filepath> work?\"" got=None display=Chat tail=""
```

**`tail=""`** = ペインの画面に**空でない行が 1 本も無い** =
箱が塗れていないどころか**シェルがプロンプトすら出していない**。
`display=Chat` なのでチャット表示の側は成立している。
つまり **シェルの準備を待たずに送っていて、起動途中の PTY が打鍵を落としている**（#640 と同型）。
項目 100 は分割の後 `wait(cx, 500)` の**固定待ちだけ**で `send` し、
`await_box!` は**送り直さない**ので最初の 1 回が落ちるとそのまま FAILED になる
（項目 94 は 40×100ms の準備待ちを持っている）。

##### 実機テストのベースライン（この Issue で分かったこと）

**結論から書く: ベースラインは 23 件ではなく 22 件で、psmux の e2e は
`schtasks /it`（session 1）で回さないと構造的に落ちる。**（この Issue の最大の収穫）

そこへ辿り着くまでの実測。`Invoke-CimMethod Win32_Process Create`（= session 0）で
`cargo test --workspace --no-fail-fast -j 2` を投げると失敗が **run ごとに揺れた**。
branch の通し走行は 31 件失敗で、増えた 8 件は**すべて psmux / spawn の e2e**
（`器のホイールは…` / `器はクライアント切断後も…` / `保持していないセッションの…` /
`明示コマンドつきの器が起動する` / `器の中のシェルのコードページを…` /
`一覧と存在確認とcwdが往復する` / `copy_mode_の位置を読み戻せる` /
`器ありでも空白入りcwdのペインが生き残る`）。単独 `--test-threads=1` に落としても
落ちる顔ぶれが入れ替わるだけだった。

**構造上、この PR がこれらを壊すことはあり得ない**: 差分は
`crates/tako-app/src/main.rs`（`mod self_test` の中）と `.agent/conventions.md` だけで、
`psmux_backend` / `spawn_arg_quoting` は **tako-core の integration test**。
main と branch でこれらのテストバイナリは同一の入力から作られる。

**真因**（#866 worker の実測。このセッション中に共有された）: **SSH（session 0）で作った psmux の
detached セッションは約 1 秒で自然死する**（`new-session -d` の +500ms は `ls` に出て
+1000ms で消える。session 1 で作ったものは残る）。`Invoke-CimMethod Win32_Process Create`
で `cargo test` を投げると session 0 なので、psmux e2e が**測り方のせいで**落ちる。

実際 #897 の検証でこれを踏み、単独走行（`--test-threads=1`）でも
**main = 10 件失敗 / branch = 7 件失敗**（psmux 16 本中）と **main のほうが悪い**結果になった
（兄弟セッションの並行ビルドは増幅要因であって主因ではない）。

**同じ HEAD を `schtasks /it`（session 1）で回し直したら psmux_backend が 16 / 0 で全緑**
（23.59 秒。session 0 では 91〜175 秒かけて 8〜10 件失敗）。`spawn_arg_quoting` も 3 / 0。
**ワークスペース全体の失敗はちょうど 22 件で、名前もベースラインと完全一致**した:

| スイート | session 1 の結果 |
|---|---|
| `tako-app` (bin) | 453 / **0** |
| `tako-cli` (lib) | 53 / **0** |
| `tako-control` (lib) | 1027 / **15 failed**（ベースライン同一） |
| `tako-core` (lib) | 800 / **7 failed**（ベースライン同一） |
| `platform_parity` | 12 / 0 |
| `encoding_conpty` | 5 / 0 |
| `psmux_backend` | **16 / 0** ← session 0 では 8〜10 件失敗 |
| `shell_integration_powershell` | 7 / 0 |
| `spawn_arg_quoting` | 3 / 0 |

つまり **ベースラインは 23 件ではなく 22 件**で、#889 が足した 23 件目
（`psmux_backend::copy_mode滞在中の打鍵がin_band解除で届く`）と **#896 のフレークは
どちらも session 0 で測っていた副作用**だった。以後、実機のテストは
**`schtasks /it` で回す**（#896 へコメント済み）。

**残骸の後始末を忘れない**（この run で踏んだ）: 隔離セルフテストと psmux e2e は
**psmux サーバー（プロセス名は `tmux.exe`）と pwsh の孤児を残す**。`-L tako-iso-<pid>` /
`-L tako-884test-<pid>` が自分の残骸で、`-L tako` は本番。溜まると psmux e2e の
失敗が増えるので、run のたびに**明示 pid** で落とす。

##### 作法として残すもの

- **番犬テスト `selftest_pty_enter_watchdog`**: `.write(…)` に渡す式を**括弧の釣り合いで
  切り出して** LF エスケープを探す。項目 94 は `format!(` と `"{}\n",` が別の行にあり、
  **行単位の走査では見つからなかった**（#897 が長く残った理由）。文字列リテラルを
  読み飛ばしながら数えるのでリテラル中の括弧で釣り合いが壊れない
- **#897 のコメントにあった「psmux e2e の失敗も同じ LF が原因」は誤り**。
  `psmux_backend.rs` の打鍵は導入時（`2947a19`）から `\r` で、真因は #896 の見立て
  （器が起動直後の入力を落とす = #640 と同型）のほう。Issue にも訂正を入れた

#### #903 の記録（疑似 TUI をファイル駆動へ + シェル片を `-EncodedCommand` へ。PR #908・2026-08-22）

**症状**: #897 で項目 94〜99 が開いた直後の壁。項目 100（#737 チャット入力欄）が
「合成した入力ボックスが画面に出ない」で必ず止まり、**100 以降が 1 つも走らない**。

##### Issue の仮説は外れていた（3 段の実測で機序を確定）

Issue の見立ては「シェルの準備を待たずに送っているので起動途中の PTY が打鍵を落とす」
（#640 と同型）だった。準備待ち + 送り直しを入れても直らず、**足した診断で
別の機序が 3 つ出てきた**。同一バイナリの A/B（`TAKO_903_*`）で 1 つずつ潰した。

| # | 実測 | 機序 |
|---|---|---|
| 1 | 送信直後は `session=Some(...)`、14.5 秒後に `session=false backend=None` | 状態切替の **Ctrl+C で器（psmux）の client が終了**し、外側 PTY ごと死んでペインが閉じる。client 自身が PowerShell スクリプトなので Ctrl+C で pipeline ごと終わる |
| 2 | 器を外すと (a)〜(f) が通り **(g) だけ落ちる** | (g) の楽観 echo は `session.is_alt_screen()`（**外側 PTY** の alt screen）を条件にする。器なしで alt screen へ入ると今度は**内側** alt screen 扱いになり表示が Chat → Terminal へ落ちるので、**器つきでしか作れない状況**だった |
| 3 | 器つきに戻すと画面に `Try "how does <filepath> work?"` だけが出て `─` と `❯` が消える（`nonempty=1`） | **器越しの打鍵から非 ASCII が落ちる**。psmux へ直接印字させた対照実験では出力経路は無傷（`capture-pane` の生バイトが `e29480` / `e29daf` / `c2a0`）だったので、落ちているのは打鍵側 |

**器は外せない / 打ち込めない**の両立が要件だと分かったので、**状態を打鍵ではなく
ファイルの書き換えで切り替える**形にした（`ShellDialect::repaint_file_loop`）。
疑似 TUI はペイン自身のコマンドとして起動する = 項目 101 / 105 / 111 と同じ流儀。

##### 4 つめの機序: 器は内側コマンドを自分で単語分割する（#875 の 3 層問題）

ファイル駆動にしても**ペインが即死**した（`ready=true` の直後は `session=Some(...)`、
7 秒後に `session=false`）。psmux へ直接投げた対照実験で確定:

| 渡し方 | 実測 |
|---|---|
| `powershell -NoProfile -Command '<引用符入りの片>'`（`shell_snippet_command` の旧形） | `list-sessions` に出ず `no server running on session …` = **即死** |
| `powershell -NoProfile -EncodedCommand <base64>` | **生存**して画面を描き続けた |

`ShellDialect::shell_snippet_command` の PowerShell 側を `-EncodedCommand`
（base64 / UTF-16LE）へ寄せた。符号化は #875 の
`platform::shell::encode_powershell_command` を `pub(crate)` にして**実装 1 つ**を共有。
base64 は `A-Za-z0-9+/=` だけなので単語分割・引用符解釈・コマンドライン組み立ての
どの層も通り、**非 ASCII も UTF-16 のまま運べる**（機序 3 にも当たらない）。

##### 実機 A/B（同じ worktree・HEAD だけ替えた）

| アーム | 結果 |
|---|---|
| 旧挙動（器つき + 打ち込み + Ctrl+C） | **FAILED**（項目 100）。`session=false` / `tail=""` |
| 器なし + 準備待ち + 送り直し | (a)〜(f) は通るが **(g) で FAILED**（`is_alt_screen` が false） |
| 器つき + ファイル駆動 + `-Command` | **FAILED**（ペイン即死。単語分割） |
| **器つき + ファイル駆動 + `-EncodedCommand`** | **項目 100 通過**（4 状態すべて `tries=1`。`ready=true waited=2.3s outer_alt=Some(true) inner_alt=false`）。2 回連続で再現 |

到達範囲は **項目 0〜100**。次の壁は**項目 101（#749）**で、fixture ペインが
PTY を持たない（`TAKO_SELF_TEST_749_CTX: seen=None session=false size=None backend=None`。
`TAKO_SELF_TEST_749_SPAWN` は出ないので **spawn は成功していて後で終了している**）
= **#906 へ起票**。

##### 検出力（最終バイナリで旧経路へ戻して確認）

`TAKO_903_LEGACY=1` を付けて同じバイナリで回すと**項目 100 が FAILED**:

```
TAKO_SELF_TEST_737_PAINT: expected="Try \"how does <filepath> work?\"" ready=false tries=1 legacy=true
TAKO_SELF_TEST_737: … session=true state=Some(Idle) child=Some(4492) backend=Some("tako-778ee07e7b07")
  nonempty=1 tail="Try \"how does <filepath> work?\""
```

この run はペインが生き残った（Ctrl+C が準備待ちの後に着弾した）ぶん、**機序 3 が
そのまま見える**: 画面に残っているのは ASCII の本文 1 行だけで `─` と `❯` が消えている。
非 ASCII の送達は**製品側の疑い**として **#907** へ分離した（`tako send` / worker への
プロンプト送達の第 2 層が Windows + persist ON で日本語を落とす）。

##### 作法として残すもの

- **番犬テスト `打ち込む疑似画面のfixtureはシェルの準備を待っている`**: `paint_and_hold` の
  使い方は「ペインの起動コマンドとして渡す」か「準備を待ってから打ち込む」の 2 通りしか
  許さない（ソース走査。前後 100 行に `shell_snippet_command` か `wait_for_pane_ready`）
- **`self_test::wait_for_pane_ready`**: 新しいペインへ最初の打鍵を送る前の準備待ちを 1 本化
  （画面に空でない行が出るか OSC 133 の Idle）。ダイアログ fixture の同型ループもここへ寄せた
- **PTY 起動の失敗理由を捨てない**: `spawn_session` の `Err` を捨てると
  「起動できなかった」が「画面に出ない」として現れ、原因が疑似 TUI 側にあるように見える
  （#903 が長引いた理由の 1 つ）。項目 100 / 101 の両方で `spawn_error` を出すようにした
- **実機の孤児は run のたびに掃除する**: 隔離セルフテストは psmux サーバー 6 個前後 +
  pwsh を残す。溜めたまま（psmux 19 / pwsh 56）走らせたら**項目 20 / 24 の固定待ちが落ちた**
  （`tako read` / `tako focus`。掃除後は同じ HEAD で通った）。掃除は
  「tako-app が 1 つも居ない」を確かめてから `-L tako-iso-*` を明示 pid で落とす
- **`git stash` を A/B に使わない**: 変更が無いと no-op なのに `git stash pop` が
  **他 worker の古い stash を pop** してコンフリクトを作る（このセッションで 1 回踏んだ。
  `git restore --source=HEAD` で戻し、stash は失われていない）。ファイルを
  `git checkout <sha> -- <path>` で差し替える方が安全
- **`cp` は `-i` の別名かもしれない**: 上書きの確認待ちで 10 分ハングした。スクリプトでは
  `command cp -f` を使う
#### #866 の記録（tmux の完全一致ターゲット。PR #902・2026-08-22）

**症状**: `tako tmux kill` が Windows で効かない。#865 で実機セルフテストが深く回るようになって
項目 48（`tako tmux list` / `kill`）の **kill だけ**が落ちることから起票された。

##### 原因: psmux は `-t =name` を解決せず「消えるまで待つ」だけ

`tako_core::tmux` は取り違え防止に **`-t =name`**（tmux の完全一致ターゲット。#181 / #32）を
渡すが、実機の `tmux` は psmux（winget の `marlocarlo.psmux` が `tmux.exe` を置く）で、
これを解釈しない。**session 1（GUI と同じデスクトップ）で 2 セッションを立てた同一ソケット**の実測:

| ターゲット | 結果 |
|---|---|
| `kill-session -t =keepa` | **exit 1 / 5158ms** `psmux: kill-session: session 'z866probe__keepa' still present after 5s`（`keepa` / `keepb` とも残る） |
| `kill-session -t kee`（前方一致だけ） | exit 0 / 25ms（**何も消さない** = psmux は素の名前でも完全一致） |
| `kill-session -t keepa` | exit 0 / 181ms（`keepa` だけが消え `keepb` は残る） |

つまり `=` を落としても取り違えは起きず（2 行目 / 3 行目の対照）、落とさないと無反応になる。

##### 測り方の罠: SSH（session 0）から測ると `=` でも成功して見える

SSH セッションから `new-session -d` で作った psmux セッションは **約 1 秒で自然死する**
（実測: `t=+500ms` で `ls` に出て `t=+1000ms` で消える）。psmux の `=` 経路は
「消えるまで 5 秒待つ」だけなので、**その自然死を成功として返す**（`=` は 962〜1745ms、
素の名前は 200ms 前後 = この差が待ちの分）。Issue 起票時の「3/3 決定的に失敗」と
本セッション序盤の「成功して見える」は同じ挙動の裏表で、**session 1 で測ると決定的に落ちる**。

##### 直し方: `=` を組み立てる場所を 1 本にした

- `tako_core::tmux` に `announces_only_tmux`（純関数。本物の tmux は `-V` で tmux しか名乗らない /
  psmux は `tmux 3.3.7` に加えて `psmux 3.3.7 (…)` と自分を名乗る）+ `TmuxTargetSyntax`
  （Exact / Plain）+ `version_announcement`（`-V` を 1 度だけ）+ `exact_target` / `session_pane_target`
- 散在していた `format!("={…}")` の直書き **33 箇所**を全部この境界経由へ
  （tako-core / tako-control / tako-app / e2e テスト）。**macOS は文字列がバイト等価**
- 番犬テスト `tmuxの完全一致ターゲットの直書きが境界の外に残っていない`（parity テスト）+
  規約を `.agent/conventions.md` の新節へ
- **`BackendCapabilities` には足さなかった**。`tako tmux *` は「任意の tmux サーバー」を触る層で、
  器（backend）とは別物 —— しかもセルフテストは `TAKO_ISOLATED=1` = `TAKO_PERSIST=0` で
  **器なし**なので、器の能力で分岐すると項目 48 は直らない
- 名前は `TmuxDialect` ではなく `TmuxTargetSyntax`。#873 の番犬（方言 enum は 1 つだけ）が
  正しく落ちたので、**シェル方言とは別の軸**であることを名前で分けた

##### 製品経路の実機 A/B（session 1・同一バイナリ・env だけを変えた）

項目 48 は `tako-test` と `tako-test2` を立て、**CLI から前者だけを kill して後者が残る**ことを
見る（`tako-test` は `tako-test2` の前方一致でもあるので、完全一致になっていない実装だと
「消えない」か「隣も消える」のどちらかで落ちる）。経路は CLI → IPC → GUI → psmux。

| アーム | 診断行 | 結果 |
|---|---|---|
| `TAKO_866_KEEP_EXACT_TARGET=1`（旧挙動） | `（項目 48: kill 後の一覧 = ["tako-test", "tako-test2"]）` | **`TAKO_APP_SELF_TEST_FAILED: tako tmux kill でセッションが消える`** / exit 1 |
| 既定（このブランチ） | `（項目 48: kill 後の一覧 = ["tako-test2"]）` | **項目 48 通過** |

macOS（実 tmux 3.6b）でも同じ診断行で通り（`["tako-test2"]`）、隔離セルフテストは
`TAKO_APP_SELF_TEST_OK` で完走した（skip は蓋閉じの既知 2 件 = 項目 63 / 76d）。

##### セルフテスト項目 48 の gate を「本物の tmux」→「駆動できる CLI があるか」へ

項目 59〜62 / 68 / 73 は attach / send-keys 前提（psmux は `detached_access` false）なので
従来どおり本物の tmux だけで回す。スキップ理由も #866 から #519 へ書き換えた。

##### 実機で確かめた「関連 tmux 系コマンド」（session 1・素の名前）

| コマンド | psmux |
|---|---|
| `list-sessions -F` / `list-windows -a -F` | 動く（タブ区切りをそのまま返す） |
| `list-clients -F` | **書式を無視**して自前の 1 行を返す（`parse_sessions` は突き合わせ不能 = 無害） |
| `has-session` / `capture-pane -p` / `display-message -p` | 動く |
| `select-window` / `kill-window` | 動く（`=` 付きでも動くが素の名前で統一） |
| `resize-window -x -y` | exit 0 だが **幅が変わらない**（`#{window_width}` は 120 のまま）= psmux 側の未対応。`tako tmux resize` は Pending のまま |

##### スライス 8（棚卸し）への申し送り

マトリクスは**触っていない**（作法 4）。この実測で倒せる / 倒せないの材料はこう:

| キー | 実測 |
|---|---|
| `tako_tmux_list` / `tako_tmux_kill` | **製品経路（CLI → IPC → GUI）で通した**（セルフテスト項目 48）。Supported へ倒せる |
| `tako_tmux_select_window` | psmux 単体で動く（`select-window` / `list-windows` とも）。製品経路は未測 |
| `tako_tmux_cleanup` | kill が効くようになったので理屈では動く（**未測**。orphan 掃除は本番セッションに触るので隔離での確認が要る） |
| `tako_tmux_resize` | **Pending 継続**（psmux が `-x` を反映しない） |
| `tako_tmux_open` | **Pending 継続**（`env TMUX= tmux attach-session` = POSIX の `env` と attach 前提） |

#### #727 の記録（設定画面のスリープ系。PR #904・2026-08-22）

**症状**: Windows の設定画面（スリープ防止タブ）が macOS 前提のまま。蓋閉じ継続は #697 で
**権限不要**に実装済みなのに「sudoers を登録 / 解除」ボタンが並び、説明文は「**Mac** が眠って…」。
さらに**いま効いているのかがどこにも出ていない**（macOS はステータスバーのチップが補うが、
Windows は蓋の開閉を観測しない = `lid_state_detectable() == false` ぶんチップが薄い）。

**WIP の扱い**: `origin/windows/727-sleep-settings` の `5791a03`（保全コミット）に
`settings_sleep.rs` + 設定タブの改修が丸ごと入っていた。**再実装ではなく移植**し、
main の現行 API へ合わせたうえで次の 3 点を足した:

1. `Device`（`Mac` / `Pc`）を**値として持ち回す**。WIP は `ui_text` の中に
   `fn is_mac() -> bool { cfg!(target_os = "macos") }` を置いていたが、それだと
   **macOS 上から Windows 側の文言を検証できない**（#515 の方針に反する）。
   OS を見るのは `Device::detect()` の 1 か所だけにした
2. 蓋閉じ継続の説明は `desc_sleep_lid(needs_privileged_setup: bool)` に。**main には
   #697 の分岐が入っていなかった**（win467 側だけ）ので、Windows でも
   「sudoers の登録が必要」と出ていた = Issue の棚卸しより 1 件多い
3. 「反映中」と「AC 未接続 / エージェント待ち」の境目を
   `sleep_guard::should_hold_assertion` / `should_disable_lid_sleep`（この PR で `pub` 化。
   **ロジックは 1 行も変えていない**）と**総当たりで一致**することをテストで固定。
   WIP はコメントで「揃える」と書くだけだった

**表示構成は純粋関数へ**: `SleepTabPlan`（状態行 + 行 / ボタンの出し分け）と
`visible_texts()`（その構成で画面に出る文字列すべて）を `settings_sleep` に置き、
描画（`render_sleep_tab`）は並べるだけにした。おかげで「Windows に macOS 固有の語が
出ない」を **GUI を起こさずに `cargo test` で**検査できる（実機の `cargo test` でも回る）。

##### 実機実測（`ssh win`。GUI は `schtasks /it` で session 1）

| 観点 | 実測 |
|---|---|
| 修正前（v0.5.13-win.3 = Issue 報告と同じ版） | 「**Mac** が眠って…」/「sudoers を登録」「sudoers を解除」/ 状態表示なし（スクショ取得） |
| 修正後 | 「**この PC** が眠って…」/ sudoers ボタン**消滅** / 「いまの状態」= アイドル防止・蓋閉じ継続・電源 + 更新ボタン |
| 状態と実効（アイドル） | busy ペインつき mode=on で表示「有効（自動スリープを止めています）」+「エージェント 1 体が稼働中」← 同時刻の `powercfg /requests` SYSTEM が `[PROCESS] …\tako-wt-727\target\debug\tako-app.exe / tako: sleep guard (always on)`。mode=off にすると表示も SYSTEM も消える |
| 状態と実効（蓋閉じ） | 表示「有効（蓋を閉じても動き続けます）」のとき `<data_dir>\lid-guard.json` が存在（`{"scheme":"381b4222-…","ac":0,"dc":null}`）。off にすると記録が消える |
| 外部（CLI）変更への追随 | `tako sleep-guard set --mode off` → タブ再表示で表示も「オフ」へ。on / off / while-agents-running の 3 状態を実測 |

**測り方の落とし穴（次の worker のために）**:

- **`powercfg /requests` は管理者権限が要る**。`schtasks /it`（session 1）の対話トークンは
  非昇格なので中で叩くと失敗する。**SSH セッション側は既に昇格している**
  （`IsInRole(544)` が True）ので、GUI を session 1 に置いたまま **SSH 側から**読む
- **`CopyFromScreen` は「画面」を撮る**ので、対象ウィンドウが他のウィンドウに隠れていると
  **別アプリの画素**が入る。しかも GPUI は**完全に隠れると描画を止める**（macOS の
  セルフテスト項目 63 / 76d / 104 のスキップ理由と同じ）ので、隠れたまま撮ると
  **古いフレーム**が残る。実際にこれで「モードのセグメントだけ古い」1 枚を撮ってしまい、
  製品バグかと 30 分疑った。`SetForegroundWindow` + 1.2 秒待ってから撮ると解消
- **この機の AC 側の蓋アクションは既に `0x00000000`**（= 何もしない）。tako が倒しても
  値が動かないので、**レールの値だけでは効きを確かめられない**（tako の記録ファイルと
  `powercfg /requests` を見る）。`set_stay_awake(_, false)` = **AC レールだけ**倒す設計（#697）
  なので DC も動かない。production の `lid-guard.json` は不在 = 誰の保持でもない素の値
- **busy 判定には器が要る**。`TAKO_ISOLATED=1` は persist を OFF にするので、
  ペインに psmux の器が無く**子プロセスを走らせても busy_agents が 0 のまま**になる
  （スライス 9 のスクリプトが `TAKO_PERSIST=1` を明示していたのはこのため）。
  器を使ったら**明示 pid で** `tmux -L tako-iso-<pid> kill-server` まで片付ける
- `tako split` にコマンドを渡すには **`--` が要る**（`split [OPTIONS] [-- <COMMAND>...]`）
- **`tako sleep-guard` は IPC を通らない**（`sleep_guard_local`）。CLI から見えるのは
  CLI プロセス自身の状態なので、`assertion_held` / `busy_agents` は常に 0。
  **GUI の実状態を読める場所はこの設定画面だけ**（スライス 9 の申し送りどおり）

##### 残り（別 Issue 候補）

- `ui_text::sleep_guard` の**他の理由文**（`reason_always_on` / `reason_agents_running` /
  `reason_no_prevention` / `reason_thermal` / `chip_active` の英語側）はまだ「Mac」と言う。
  Issue #727 の棚卸しは `reason_system_disabled` だけを挙げていたのでこの PR も**そこだけ**
  直した。ステータスバーのポップオーバーに残るので **#905 として起票**
- `setup` の対話フロー（L3 の蓋閉じ案内）は未移植（スライス 9 の申し送りのまま）

#### #907 の記録（器つきペインへの送達で非 ASCII が落ちる。2026-08-22）

**症状**: #903 の副産物。Windows の既定構成（persist ON = 器 psmux）で `tako send` すると
**cp932 に無い文字が黙って消える**。

##### 層の確定（同じ tako バイナリ・同じ `tako send` 経路で器の有無だけ替えた）

| アーム | 送った hex（`テスト─❯`） | 届いた hex |
|---|---|---|
| 器なし（`TAKO_BACKEND=none`） | `e38386 e382b9 e38388 e29480 e29daf` | **完全一致** |
| 器あり（psmux） | 同上 | `e38386 e382b9 e38388` = **`─`(U+2500) と `❯`(U+276F) が落ちる** |

**カタカナ・漢字は通る**（`テスト` / `日本` は cp932 にある）。落ちるのは cp932 に無い文字だけ
なので、Issue に書いた「日本語プロンプトが壊れる」は**半分外れ**（かな・カナ・漢字の指示は届く。
壊れるのは罫線・記号・絵文字を含む文）。犯人は **psmux の client の打鍵経路**
（`PromptFlow` の貼り付けではない: 器なしの同じ経路がバイト等価だった）。

##### 器の注入口は UTF-8 をそのまま運ぶ（修正の実現性）

psmux 3.3.7 の `--help` に `send-keys -l` / `load-buffer` / `paste-buffer` がある。
実機で `テスト─❯日本` を投げたら **両方ともバイト等価**で届いた:

```
PAYLOAD_HEX=e38386e382b9e38388e29480e29dafe697a5e69cac
A_hex(send-keys -l)      = 412d e38386e382b9e38388e29480e29daf e697a5e69cac 2d41   ← 一致
B_hex(load+paste-buffer) = 422d e38386e382b9e38388e29480e29daf e697a5e69cac 2d42   ← 一致
```

`DetachedAccess::send_text` が psmux で未対応なのは「送出の信頼性」の話（#519）で、
**注入口そのものは使える**というのがこの Issue の収穫。

##### 直し方

- `BackendCapabilities::keystrokes_ascii_only`（新設）= 「器の client の打鍵が ASCII しか
  運べない」を能力として表に出す（psmux = true / tmux・器なし = false）
- `SessionBackend::inject_text`（新設・既定は未対応）を psmux が `send-keys -l` で実装。
  本文は**引数**として渡すので Windows のコマンドライン（UTF-16）経由で落ちない
- 迂回の判断は純粋関数 `backend::needs_text_injection`（**非 ASCII かつ落とす器のときだけ**）。
  ASCII は従来どおり打鍵 = 経路を増やして挙動差を作らない。Enter も ASCII なので打鍵のまま
  （「貼り付けと分離した単独キー」= #95 / #32 の規約を維持）
- 送出側 2 か所を `delivery::inject_non_ascii` へ寄せた（`dispatch::Send` の直接書き込みと
  PromptFlow の貼り付け）。注入の成否は `persist.log`（`送達: 器へ注入 …`）。
  失敗したら警告して**打鍵へ縮退**する（無音で失うより、従来の壊れ方に留める）

##### 実機 A/B（after / 検出力）

| アーム | 器あり arm の受信 hex |
|---|---|
| 修正前（`TAKO_907_NO_INJECT=1`。同一バイナリ） | `…MARKJ-e38386e382b9e38388-MARKJ`（落ちる） |
| 修正後 | `…MARKJ-e38386e382b9e38388e29480e29daf-MARKJ`（**バイト等価**） |

ASCII のみの対照（`MARKA-ASCII-MARKA`）は before / after / 器なし のすべてで一致 = 経路を
増やしていないことの裏付け。実機セルフテストは **#903 と同じ項目 101（#906）で止まる**
= 送達経路に触ったが新規回帰ゼロ。実機テストは **22 件失敗 = ベースライン一致**。

##### 測り方の落とし穴（3 回踏んだ。ここを外すと結論が逆になる）

- **PowerShell は子プロセスの stdout を既定で ANSI（cp932）として読む**。
  `capture-pane -p` / `tako read` を素で捕まえると測定側で化け、
  「送達が壊れた」に見える（最初のプローブがこれで `繝・せ繝遺楳` を出した）。
  `[Console]::OutputEncoding` と `$OutputEncoding` を UTF-8 にしてから測る
- **`tako persist off` は器つきの既存ペインを失う**。器あり / 器なしを 1 インスタンスで
  比べようとすると測定対象が消えるので、**インスタンスを 2 本**（`TAKO_BACKEND=none` で 1 本）立てる
- CLI から製品経路を駆動する形: 隔離 GUI の control.json は
  `%TEMP%\tako-iso-discovery-<pid>`、`tako list` は `tabs[].panes[].id`、
  **`tako split` の出力は素の数値**（JSON ではない）、`tako read` は素のテキスト、
  `tako persist off` は位置引数、`tako split` にコマンドを渡すには `--` が要る
- **fixture は「シェル自身の echo」が一番強い**。`[Console]::In.ReadLine()` の echo ループを
  ペインのコマンドにする形は、alt screen（器の client が smcup を出す）と PromptFlow の
  入力欄検証に足を取られて配送されず、4 通りすべて空振りした
#### #905 の記録（スリープ防止ポップオーバーの文言。PR #909・2026-08-22）

**症状**: #727 で設定画面と `reason_system_disabled` は能力ベースへ直したが、**ステータスバーの
チップ + 詳細ポップオーバーには「Mac」が残っていた**（#727 の棚卸しがそこだけを挙げていたため、
あの PR も 1 本しか直していない）。Windows でもアイドル防止（#524）も蓋閉じ継続（#697）も
効くので、同じ状態になると「Mac を自動スリープさせていません」と別 OS の話を読ませる。

**直し方**: #727 の `settings_sleep::Device` をそのまま使い、機械を名指す 5 本
（`chip_active` / `reason_always_on` / `reason_agents_running` / `lid_sleeps` / `thermal_note`）を
呼び名で出し分ける。集約側（`chip_label` / `reason` / `lid_behavior`）は呼び名を**受け取る**形へ
変えたので、`Device::detect()` を呼ぶのは `render_sleep_guard_overlay` の先頭 1 か所だけ
（= macOS 上から Windows 側の文言をテストできる）。

**drift 対策 2 本**（#727 の `visible_texts` と同じ思想）:

- `popover_texts(state, device)` = 「この状態でチップとポップオーバーに出る文字列すべて」。
  状態を受け取るので高温注記のような条件つきの行も実際に出るときだけ入る
- **番犬テスト**が `status_bar::render_sleep_guard_overlay` のソースを走査し、そこで呼ばれて
  いる文言関数がすべて `popover_texts` に載っていることを検査する。`ui_text::` の中にも
  `text::` という並びがあるので**識別子境界**で弾く（最初これで `sleep_guard` を誤検出した）
- macOS 不変は**日英の実文字列**で押さえた（相対比較では「両方いっしょに壊れた」を検出できない）。
  そのために `tests_support::with_lang` を追加

##### 実機実測（`ssh win`。GUI は `schtasks /it` で session 1）

チップを**実際にクリックして**ポップオーバーを開き、日本語と英語の 2 枚を撮った
（証拠は `~/dev/tako-evidence/905/`）。変更した 5 本のうち 3 本は英語側だけなので両方見る。

| 表示 | 実測 |
|---|---|
| 日本語 | いまの状態が「常時オンの設定のため、**この PC** を自動スリープさせていません」 |
| 英語 | チップ **"Keeping this PC awake"** / Status **"Always-on is enabled, so this PC is kept from sleeping"** / On lid close **"This PC sleeps as usual, stopping running processes"** |

**測り方の落とし穴（#727 の記録に続くもの）**:

- **ポップオーバーは CLI / MCP から開けない**（クリック専用の UI 状態）。Win32 の
  `SetCursorPos` + `mouse_event` で実クリックする。座標はスクリーンショットを 1 枚撮って
  そこから読む（チップはステータスバーの中央付近・ウィンドウ相対で約 (557, 730)）
- **表示言語の切り替えは 3 通り試して 1 つだけ効いた**: ①起動前の `tako lang en` は
  IPC 相手が居ないので**届かない**（`settings.json` は `"language": "system"` のまま）
  ②起動後の `tako lang en` も**この隔離構成では表示が変わらなかった**（出力も空。
  原因未確定なので製品バグとは断定しない）③**起動前に `settings.json` を直接書き換える**
  のは効くが、**BOM を付けると全部が既定値へ落ちる**（PowerShell 5.1 の
  `Set-Content -Encoding utf8` は BOM を付ける → serde が読めず、言語だけでなく
  スリープ設定も既定へ戻ってチップごと消えた）。`[System.IO.File]::WriteAllText` +
  `UTF8Encoding($false)` で解決

#### #906 の記録（器が拒否する符号化ペイロード。2026-08-22）

**症状**: #903 で項目 100 を通した直後の壁。項目 101（#749 自動ハンドオフ）が
`TAKO_SELF_TEST_749_CTX: seen=None session=false size=None state=None backend=None tail=""`
で必ず止まり、**101 以降が 1 つも走らない**。`TAKO_SELF_TEST_749_SPAWN` は出ない
（= `spawn_session` は成功している）のに検査の時点でペインが居ない。

##### Issue の当たり（`Clear-Host` / 60 連の `` `n `` / `Start-Sleep 3600` のどれかが器の中で落ちる）は外れ

器（psmux）へ**同じシェル片を直接投げる対照実験**（`new-session -d` + `list-sessions` +
`capture-pane -p`。session 1 = `schtasks /it`）で、落ちているのは**psmux の
`new-session` そのもの**だと分かった。失敗の表示は
`psmux: アクセスが拒否されました。(os error 5)` / `Access is denied.` の 2 通り
（`(os error 5)` は Rust の `io::Error` の表示形式）で、終了コードは 1 / 5:

| アーム | 実測 |
|---|---|
| 項目 101 の片そのまま | **exit 5**（順序を入れ替えて 5/5・別 run で 4/4）。セッションは作られない |
| 同じ片 + 末尾に空白 1 個 | **exit 0** で生存し画面も描く |
| 同じ片で `Start-Sleep 30` / 改行 3 連 / 本文を `XMARK` へ / 保持をループへ | どれも exit 0 |
| **本文だけ差し替えた同じ長さの新品 4 本** | **4 本とも exit 5** = 残骸の衝突ではなく内容依存 |

`Clear-Host` を外しても落ちる・`Start-Sleep 3600` 単体では落ちないので、当たりは全部外れ。

##### 条件は「base64 が `==` で終わる」（同一長の A/B で判別できる）

長さを 1 文字ずつ動かして総当たりした結果、**符号化ペイロードの末尾**が判別子だった。
**同じ base64 長で padding だけを変えると結果が反転する**のが決め手:

| base64 長 | `==`（2 個） | `=`（1 個） | パディング無し |
|---|---|---|---|
| 448 | **exit 5** | — | exit 0 |
| 544 | **exit 5** | exit 0（540） | exit 0 |
| 576 | **exit 5**（5 回） | exit 0（580） | exit 0 |

- `==` が落ちるのは**長さの帯の中だけ**（実測で 448〜576。256〜416 と 752 は `==` でも通る）。
  帯の上端は測り切っていないので、**`==` を出さない側へ寄せる**のが安全側
  （`=` 1 個・パディング無しは 164〜752 の全実測で通った）
- **コマンドライン側は無関係**: `==` の後ろに `-NoLogo` を足す / 引数順を変える /
  行末に空白を足す、のいずれでも落ちる = トークンの位置ではなく**ペイロードの内容**が条件
- psmux は winget 配布のコンパイル済み exe（`pmux.exe` / `psmux.exe` / `tmux.exe` の
  3 本とも同一バイト）でソースが無いため、これ以上の内部機序は追わない

##### tako 側の連鎖（なぜ「spawn は成功しているのに居ない」に見えるか）

器の `new-session` が失敗すると psmux の client が終了する → tako から見ると
**外側 PTY の子が死んだだけ**なので `spawn_session` は `Ok` を返し、そのあと
`CloseReason::Exited` でペインが閉じる。器の設定は `remain-on-exit` off なので
**画面には何も残らない**。だから `session=false size=None state=None backend=None tail=""`
という「最初から無かった」ように見える形になる。

##### 直し方: 符号化の出口で二重パディングを作らない

`platform::shell::container_safe_script`（純粋関数）を新設し、UTF-16 の要素数が
3 の倍数になるよう**末尾へ空白を 1 個**足す（バイト数 = 要素数 × 2 なので、
要素数 ≡ 2 (mod 3) のときだけ足せばバイト数が 3 の倍数 = パディング無しになる）。
入れたのは `encode_powershell_command` の 1 箇所なので、セルフテストのシェル片（#903）と
**実行ペイン（#875 = 製品経路）**の両方が同じ経路で守られる。末尾の空白は
PowerShell から見て何もしないので意味は変わらない。`TAKO_906_NO_PAD=1` で修正前へ戻せる。

**局所修正では足りなかった**理由: 項目 101 の fixture をファイル駆動（#903 の
`repaint_file_loop`）へ替える案も測って通ったが、**項目 111（#813）の API エラー
fixture も同じ帯に入っている**（b64 長 560 / `==`）ので、壁が 101 から 111 へ移るだけだった。
符号化の出口で閉じると本文の長さに依らなくなる。

##### 実機 A/B（同一バイナリ・env だけを変えた）

| アーム | 結果 |
|---|---|
| `TAKO_906_NO_PAD=1`（旧挙動） | **項目 101 で FAILED**。`TAKO_SELF_TEST_749_CTX: seen=None session=false size=None state=None backend=None tail=""`（Issue の報告と同一） |
| 既定（このブランチ） | **項目 101 通過** → 102（#761 / #792）103（#772）105（#778）106（#781）110（#803）111（#813）112（#815）114（#826）115（#830）が **Windows で初めて緑**。到達範囲は**項目 0〜115** |

##### 次の壁: 項目 116（#835 Finder の「このアプリケーションで開く」）

`TAKO_APP_SELF_TEST_FAILED: 116: file URL が 4 本ともパスへ戻る (1) (#835)`。
原因は**両側の POSIX パス前提**で、器とも符号化とも無関係:

- `self_test::file_url` は `path.display()` をそのままパーセント符号化するので、
  Windows では `file://C%3A%5CUsers%5C…`（`file:///C:/…` にならない）
- `open_files::file_url_to_path` は復号結果が `/` で始まらないと `None` を返す

4 本のうち通ったのは 1 本（`/` 始まりのダミー）だけ = 観測値 `(1)` と一致する。
**#913 へ起票**（#835 自体は macOS の Finder 固有機能なので、直し方は
「Windows の入口に合わせて項目を gate する」か「`open_files` に Windows 形を教える」の判断が要る）。

##### 実機テストのベースライン 22 件（失敗名。#906 で `--no-fail-fast` で全数採取）

`cargo test --workspace` は**既定で fail-fast** なので、tako-control が落ちた時点で
tako-core の 7 件が走らず「15 件」に見える。照合するときは `--no-fail-fast` を付ける。

tako-control（15）: `acceptance_gates::tests::execute_command_true_false` /
`…::execute_command_with_cwd` / `…::execute_command_with_output` /
`…::gate_check_skips_custom` / `…::gate_check_with_command` /
`config_share::env::tests::リポジトリ配下の実体も外部管理として検出する` /
`dispatch::tests::tree_folder_symlink経由でも削除できる` /
`…::tree_folder_symlink経由の重複追加は1エントリに畳まれる` / `…::tree_folder_追加と一覧と削除` /
`orchestrator::tests::resolved_env_expands_tilde` /
`remote::tests::daemon_stop_implはpid再利用時にkillしない` /
`remote::tests::is_process_aliveは現在のプロセスをtrueで返す` /
`setup_bootstrap::tests::導入計画は何をどこに入れるかを必ず含む` /
`stale_binary::tests::test_pidpath_self` / `…::ランチャ探索は実行可能な通常ファイルだけを拾う`

tako-core（7）: `links::tests::cwd不明でも絶対パスとホーム起点は検出する` /
`links::tests::detect_absolute_path` / `links::tests::tuiの装飾付きsoft_wrapをまたぐパスを検出する` /
`shell_profile::tests::path判定は完全一致で行う` / `…::既にpathにあるならファイルを触らない` /
`tab::tests::pinned_folder_symlink経由でも削除できる` / `…::pinned_folder_symlink経由の重複は畳まれる`

##### 作法として残すもの

- **器へ直接投げる対照実験を先にやる**（#903 と同じ）。tako 越しだと「spawn は成功」に
  見えるので、器の `new-session` の**終了コードと stderr を採る**まで機序が見えない
- **「内容依存」と「位置・残骸依存」は必ず切り分ける**: 最初の 2 回は落ちるのが
  常に 1 番目と 3 番目だったので位置の交絡を疑い、順序を入れ替えた 3 回目で内容依存を確定した。
  さらに**本文だけ差し替えた同じ長さの新品**を測って残骸の衝突も落とした
- **1 文字だけ変える A/B が最強**（末尾に空白 1 個で生存へ反転した）。長さ・パディング・
  本文のどれが効いているかは、1 つずつ動かさないと分からない
- 実機の psmux は winget の `marlocarlo.psmux`。`pmux.exe` / `psmux.exe` / `tmux.exe` は
  同一バイト（6,883,328 B）でソースは同梱されない

#### #913 の記録（file URI のドライブレター規則を境界へ。2026-08-23）

**症状**: #906 で項目 101 を通した直後の壁。項目 116（#835 Finder の「このアプリケーションで
開く」）が `116: file URL が 4 本ともパスへ戻る (1)` で止まる。

##### 原因は**両側の POSIX 前提**（器とも符号化とも無関係。コードで確定）

| 層 | 実際 |
|---|---|
| テスト側 `self_test::file_url` | `path.display()` をそのままパーセント符号化するので、Windows では `file://C%3A%5CUsers%5C…`（`file:///C:/…` にならない） |
| 製品側 `open_files::file_url_to_path` | 復号結果が `/` で始まらないと `None`。ドライブレター形式を知らない |

4 本のうち通ったのは `/` 始まりのダミー 1 本だけ = 観測値 `(1)` と一致する。

##### 決め手: 同じ規則が既に**もう 1 か所**にあり、そちらは Windows 形を扱えていた

`osc_tap` の `strip_drive_slash`（OSC 7 の cwd 追従。RFC 8089 の Windows 形式を落とす）が
**同じ判定をすでに持っていた**。つまり「RFC 8089 の規則が 2 か所にあり、片方だけが
POSIX 専用のまま取り残されていた」= #870（ホーム解決）・#873（方言判定）と同型の問題。

`tako_core::file_uri` を新設して `strip_drive_slash` を移し、`osc_tap` と
`open_files` の両方がそこを通る形にした。**プラットフォームで分岐しない**のが要点で、
判定は URI の形だけ（`/` + ASCII 英字 + `:` の直後が `/` か終端）で決まるので
macOS 上から Windows 形の入力を検査できる（#515 の方針）。

**`percent_decode` は統合しなかった**。2 実装あるが**不正入力の方針が用途で違う**
（OSC 7 は `%zz` を拒否 = 端末の壊れたバイト列で cwd を誤って移さない / 開く経路は
素通り = 落として別のファイルを開くより「開けない」で止める）。意図的な分岐なので
`file_uri` の doc にその理由を書いた。

##### 踏んだ細部

- **「絶対パスか」の判定はドライブレターを落とす前**に行う。落とした後の `C:/x` で
  見ると弾いてしまう（file URI のパス部は必ず `/` 始まりなので、判定はその形に対して行う）
- `:` は RFC 3986 の unreserved ではないので `self_test::file_url` は `%3A` を出す。
  受け口は復号してから境界へ渡すので**リテラルの `:` も `%3A` もどちらも通る**
  （テストで両者が同じパスへ戻ることを固定した）
- 番犬テストは**テストモジュールより前だけ**を走査する。ファイル全体を見ると
  番犬自身が書いた文字列（`is_ascii_alphabetic`）に当たって自分で落ちる（1 回踏んだ）

##### 実機 A/B（同一バイナリ・env だけを変えた）

| アーム | 結果 |
|---|---|
| main（修正なし。#906 セッションで実測） | **項目 116 で FAILED**（`116: file URL が 4 本ともパスへ戻る (1)`） |
| 既定（このブランチ） | **項目 116 通過**。`TAKO_SELF_TEST_835: tabs=3->6 new=[("読み物.md", 1, Some("\\?\C:\…\読み物.md"), false), ("プロジェクト", 1, None, true), ("unknown.xyzzy", 1, Some(…), false)]` = macOS と同じ 3 タブ。**117 / 118 も通り到達範囲は 0〜118** |

##### 同一バイナリの A/B は**単体で決定的に**取った（実機は途中でフレークした）

`TAKO_913_LEGACY=1` は**テスト側と製品側の両方**に入れてある（片方だけだと
「半分直った状態」になって A/B にならない。実機で 1 回踏んだ）。macOS の単体テストで
決定的に振れる:

```
$ TAKO_913_LEGACY=1 cargo test -p tako-app --bin tako-app file_url
… windowsのパスもrfc8089の形のurlになる ... FAILED
  left: "file://C%3A%5CUsers%5Cme%5Ca.md"     ← Issue の報告と同じ壊れた形
 right: "file:///C%3A/Users/me/a.md"
… 作ったurlは受け口でパスへ戻る ... FAILED
test result: FAILED. 1 passed; 2 failed
```

実機の legacy アームは**項目 116 へ届く前に 3 回落ちた**（`tako read` ×1 / #702 ×2）。
`TAKO_913_LEGACY` は項目 116 以外に触らないので無関係のはずで、**直後に同じ掃除をして
after アームを回したら同じく早期（`tako read`）で落ちた** = 環境ドリフト（この時間帯の
実機は GUI セルフテストがフレークする状態）と切り分けた。到達範囲の実測は
その前の安定していた 3 run（うち 2 run が項目 116 を通過して 119 まで到達）を採る。

##### 次の壁: 項目 119（#868 install_plan）

`119: install_plan が公式コマンド・置き場所・権限を含む (#868) lines=6`。
セルフテストが unix の導入手順をリテラル期待している（`claude.ai/install.sh` /
`.local/bin/claude`）が、Windows の計画は `install.ps1` で区切りが `\` = **116 と同種の
POSIX 前提**。製品側（`agent_install::recipe(Windows, Claude)`）は正しい
→ **#920 へ起票**（期待値を計画そのものから作る形が筋）。

##### 作法として残すもの

- **孤児は run のたびに掃除する（改めて実証）**: 1 回目の run は項目 111（#813）の
  idle fixture が画面に出ずに落ちた。`-L tako-iso-*` の tmux/psmux 4 個を落として
  再実行したら**同じバイナリで通った**（elapsed 284s → 短縮）。#903 の作法どおり
- **共有ツリー `~/dev/tako` では作業しない**（このタスクから徹底）。
  `git worktree add -b <branch> ~/dev/tako-wt-<番号> origin/main` で分ける。
  fresh worktree は `web/tako-remote/dist/` を持たないので既存ツリーからコピーする

#### #920 の記録（install_plan の期待値を計画から作る → **Windows のセルフテストが完走**。2026-08-24）

**症状**: #913 で項目 116 を通した直後の壁。項目 119（#868 `install_plan`）が
`119: install_plan が公式コマンド・置き場所・権限を含む (#868) lines=6` で止まる。
`lines=6` = 計画は正しく 6 行返っていて、落ちていたのは**期待値の突き合わせだけ**。

##### 原因はテスト側の unix リテラル（製品側は正しい。コードで確定）

| 条件 | Windows の実際 | 判定 |
|---|---|---|
| `claude.ai/install.sh` | `https://claude.ai/install.ps1`（`interpreter = "powershell"`） | **不一致** |
| `.local/bin/claude` | 相対は `.local/bin/claude.exe`、しかも表示は `\` 区切り | **不一致** |
| `sudo` | `InstallPlan::lines()` の固定行 | 一致 |

##### 直し方: 期待値を `agent_install::current_recipe` から作る

`InstallRecipe` は `platform` を引数で受ける純粋関数なので、**テストが OS を知らずに済む**。
取得元は `recipe.source.url`、置き場所は `recipe.launcher_rel` / `payload_rel`
（**両 OS で `/` 区切りの静的文字列**）と突き合わせ、表示側は比較の前に `/` へ寄せる。
権限の説明は `管理者権限`（プラットフォームに依らない語）で見る。

**同型のリテラルが単体テストにもあった**: `setup_bootstrap::tests::導入計画は何をどこに
入れるかを必ず含む` は**実機ベースライン 22 件の 1 つ**で、原因は同じ
（`.local/bin/claude` を `/` 区切りで期待しているのに `launcher_path_in` が `PathBuf` =
実行中 OS の区切りになる）。リテラルを消して**両プラットフォームぶんを macOS から検証**する
形にし、「他方の手順が混ざっていない」（計画がプラットフォームに依らなくなる退行の検出）も足した。
**これでベースラインは 22 → 21 に減る**。

##### 実機実測: **セルフテストが完走した**（`TAKO_APP_SELF_TEST_OK` / exit 0）

```
TAKO_SELF_TEST_868: step=Some("auth") plan_lines=6 dry_run_performed=Some(false) rejected=true
TAKO_APP_SELF_TEST_OK
EXITCODE=0
```

**FAILED 0 件**。**main を取り込んだ後（#915 handoff のプロジェクト単位化 / #916 自動
マイグレーション / #919 リモートフォルダで自己テストが約 950 行増えた状態）でも完走**した
（`TAKO_SELF_TEST_915_MIGRATE` / `_915_PROMPT` / `_915_IDEMPOTENT` / `_915_UNRESOLVED` /
`_915_FILES` / `TAKO_SELF_TEST_916` が全部出て FAILED 0・skip 19）。

skip は 19 件で全部理由つきの既知（psmux が本物の tmux でない系 /
PDF の text_layer 不在 #693 / WebView2 の panic #724 / macOS 固有の項目 79 /
POSIX 専用の道具 = nc・ジョブ制御・`/dev/fd`・ECHOCTL / links の POSIX 前提 #522 /
蓋閉じで未描画になる項目）。**#865 で項目 1b が落ちていた状態（カバレッジ 0）から、
9 本の Issue（#866 / #870 → #913 / #872 / #875 / #877 / #881 / #884 / #889 / #897 /
#903 / #906 / #913 / #920）を積んで全項目に到達した**。

##### 分離した Issue

- **#925**: 導入計画の権限説明が Windows でも「sudo」と言う（`InstallPlan` が `platform` を
  持っていないので呼び名の出し分けには設計判断が要る = #905 と同型）

#### #898 の記録（コマンド解決を実行ファイル探索の境界へ。2026-08-24）

`which` は **Windows に存在しない**（実測: `Get-Command which` → NOT FOUND）のに、
コマンド解決が `which` の起動決め打ちだった。**tako.exe が PATH 上に居るのに tako 自身には
「無い」ように見える**状態。境界 B16（`platform::exe::find`）へ寄せた。

##### Issue の一覧より 2 箇所多い（走査で見つけた）

`which` の直起動を全走査すると、Issue が名指しした `dispatch.rs` の 2 箇所に加えて
`stale_binary.rs`（dispatch とは別の `which_claude` 複製）と
`tako-app/src/settings_window.rs`（設定画面のエージェント検出。claude / codex / agy を
導入済みでも「未検出」表示）があった。**Issue の一覧を信じず値／形で走査する**のが要点。

##### 同じ関数にもう 1 つの POSIX 前提

`resolve_tako_binary` の ③「実行中バイナリの隣」が `dir.join("tako")` 決め打ち。
Windows の隣は `tako.exe` なので**常に空振り**して裸の `tako` へ落ちていた。
`std::env::consts::EXE_SUFFIX` で組む形にすれば `cfg` は増えない。

##### 実機の A/B（製品側 2 ファイルだけ `git checkout origin/main -- <path>` で差し替え）

| 観測点 | BEFORE（main） | AFTER |
|---|---|---|
| `resolve_tako_binary()` | **`tako`**（裸） | `…\target\debug\tako.exe` |
| MCP 自動登録の `command`（`setup-mcp --project`） | `"tako"` | `"C:\\…\\tako.exe"` |
| 同・通った経路 | `setup_mcp_direct`（claude 解決が失敗） | `setup_mcp_via_cli`（`claude mcp add`） |
| `launcher_path()`（`.local\bin` を PATH から外して高速路を空振りさせた場合） | **`None`** | `Some(…\claude.exe)` |

**同一プロセス内の対照が決め手**: BEFORE の同じ run で `exe::find("tako")` は正しいパスを
返しているのに `resolve_tako_binary()` は裸へ落ちている = ②の `which` が `None` を返し
③も空振りしたことが 1 回の観測で見える。別ビルドを 2 本並べる必要がない。

##### Issue の記述を 1 点訂正

「stale 検知が Windows で**常に**無効」ではなく、**#772 の高速路（プロセス PATH の stat 走査）が
空振りしたときだけ**無効。素の PATH に `claude.exe` が居る実機では高速路が拾えており、
BEFORE でも検知は動いていた。無効になる実条件は「インストーラの PATH 更新が実行中プロセスへ
伝播していない」か「claude が npm シム（`claude.cmd`）で入っている」（高速路は `claude.exe`
しか stat しない）。**症状の再現条件を作ってから A/B を取る**。

##### #899 との関係（統合しない判断）

`welcome::launch_command_line` は `resolve_tako_binary()` を `shell_quote` に通す。
安全文字が `[A-Za-z0-9._-/]` なので **Windows の絶対パスは `:` と `\` で「安全でない」判定**に
なり POSIX 形の `'…'` で囲まれる（PowerShell は式として評価するので実行されない）。
つまり #898 は #899 の症状 2 を**顕在化させる**。ただし #899 の症状 1（行末が LF なので
PSReadLine が継続行にして確定しない。#897 で実測）により**観測される最終結果は変わらない**
ので、値を記録するテストだけ置いて是正は #899 へ渡した（`ShellDialect::program()` 経由へ）。

##### 踏んだ罠

- **`Start-Process` で投げた長い処理は SSH セッションが切れると死ぬ**（ログ 0 バイト・
  cargo も居ない、で気づいた）。作法どおり `Invoke-CimMethod`（`Win32_Process.Create`）で投げ、
  **リダイレクトに頼らずスクリプト自身が `Out-File` でログへ書く**形にすると確実
- **`git checkout origin/main -- <path>` は index にも入る**ので、戻すのは
  `git checkout -- <path>` では**足りない**（index の main 版で上書きされる）。
  `git restore --source=HEAD --staged --worktree -- <path>` を使う
- **純粋関数のテストでも `Path::join` の結果を期待値のリテラルに書くと Windows だけ落ちる**
  （区切りが `\`）。期待値も同じ `join` から作る（#920 と同じ型。実際に 1 度落とした）
- 実機の孤児 `pwsh` が 13 個（1〜2 日前のもの）溜まっていた。run の前に
  「tako-app が 1 つも居ない」を確かめてから**明示 pid で**落とす

##### 実機スイートの照合（`--no-fail-fast`）

| 段階 | 失敗数 | 内訳 |
|---|---|---|
| 最初の run | 23 | 21（ベースライン）+ 自作テスト 1 + #930 |
| テスト修正後 | **22** | 21（ベースライン）+ **#930（main 由来）** = **新規ゼロ** |

`tako-control --lib` 単体では **14 failed = このクレートのベースライン 14 と一致**、
新規 6 テストは全部 Windows で緑。#930 は `origin/main`（`1d75598`）を実機で直接
チェックアウトして同じ失敗を再現したので main 由来と確定した。

**現在の実機ベースラインは 21 ではなく 22 件**（#919 が #906 のベースライン記録より後に
main へ入ったぶん）。#930 が直れば 21 へ戻る。

##### #899 の症状 2 の実測値（Windows）

```
resolve_tako_binary          -> C:\Users\<win>\dev\tako\target\debug\tako.exe
launch_command_line("master") -> 'C:\Users\<win>\dev\tako\target\debug\tako.exe' master
POSIX クォートで囲まれているか: true
```

macOS では `/Applications/tako.app/...` が安全文字だけなので囲まれない（= 回帰なし）。

##### 分離した Issue

- **#930**: `tako-core` の `remote_fs_e2e::解決できないホストは接続前に分類される` が
  Windows 実機で失敗（#919 由来）。**速い FAILED と >60 秒のハングの両方**を観測した
  ので、名前解決不能の枝は分類だけでなく戻ってこない経路がある疑い

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

スライス 9 の道具は `C:\Users\winuser\dev\` に残してある（次スライスで使い回せる）:
`s9-launch.ps1`（schtasks から session 1 へ GUI を投げる。persist ON）/
`s9-drive.ps1`（SSH 側から CLI で駆動して観測）/ `s9-final.ps1`（受け入れ観点の通し）/
`s9-lidcycle.ps1`（蓋の倒す → 自動解除 → `kill -9` 残留 → 起動時復元の 4 段）。
採取物は `C:\Users\winuser\dev\tako-evidence-s9\`。

#### スライス 9 が残した宿題

- **#724 の症状②（「ブラウザで開く」で abort）は未着手**。wry の `build_as_child` が
  `wait_with_pump` で入れ子メッセージループを回し、GPUI の `App` 借用中に
  foreground runnable が再入して二重借用 panic → `extern "system"` を跨ぐので abort。
  WIP は `windows/724-port-crash` の `82d3dcb`（`webview.rs` の `CREATION_PUMPS_EVENT_LOOP` +
  `main.rs` の遅延生成キュー、計 260 行）
- ~~**#727（設定画面のスリープ系が macOS 前提）は未着手**~~ → **完了**（PR #904。
  上の「#727 の記録」節）。ボタンが必ず失敗する症状はスライス 9 の dispatch 変更で
  解消済みだったので、残っていた文言と状態表示の欠落を片付けた
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
    スライス 5 の道具は `s5-launch.ps1` / `s5-capture.ps1` / `s5-drive.ps1`（`C:\Users\winuser\dev\`）

#### 現在の Windows 実機ベースライン（`ssh win`。psmux 3.3.7 導入済み）

**最新の実測は main `551fa0b`（#889 の A/B で取った。合計 23 件）**:

| スイート | 結果 |
|---|---|
| `tako-app` (bin) | 446 / **0** |
| `tako-cli` (lib) | 53 / **0** |
| `tako-control` (lib) | 1027 / **15 failed** |
| `tako-core` (lib) | 795 / **7 failed** |
| `platform_parity` | **12** / 0 |
| `encoding_conpty` | 5 / 0 |
| `psmux_backend` | 15 / **1 failed** ← #766 以降に増えた分（#897） |
| `shell_integration_powershell` | **7** / 0（#766 の側路テストを含む） |

**失敗 23 件はすべて main 由来**（#583 の既知分 + 以降 main へ増えた同系。#867 / #873 / #877 /
#766 では 22 件で、増えた 1 件 = `psmux_backend::copy_mode滞在中の打鍵がin_band解除で届く` は
**テスト側が Enter を LF で送っている**のが原因（#897。直せば 22 件へ戻る）。
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
- **Windows/MSVC のバイナリを ASCII 走査して Rust の関数名を探しても見つからない**
  （シンボル名は分離した `.pdb` 側で、この repo の debug profile は `.pdb` を出さない）。
  「このバイナリはどちらのアームか」を確かめる手段としては**使えない**（#884 で 1 回誤用した）。
  ビルド時の `git rev-parse HEAD` を記録し、アーム間で `Get-FileHash` が違うことと、
  **観測された挙動そのもの**を根拠にする
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
