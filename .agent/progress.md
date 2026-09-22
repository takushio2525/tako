# Progress Log

> AI が作業完了時に**末尾へ追記**する時系列ログ。新しいものほど下。
> **1 エントリは 1〜3 行**（何を / どこを / 結果）。詳細は git log・Issue・PR・`.agent/plans/` に委ねる。

このファイルは `AGENTS.md` から `@import` されるので**毎ターン全文が読み込まれる**。
予算（直近 5 作業日 / 20 エントリ / 12 KB）を超えたぶんは `progress-archive.md` へ
1 行で移る。移送は `tako context-budget fix` が行う（冪等・本文は改変しない・
全文は git 履歴に残る）。規約の全文は `AGENTS.md`「起動時ロードの予算」節。

## 追記フォーマット

```markdown

## YYYY-MM-DD（#Issue 一言）
- {何を / どこを / 結果}
- 関連コミット: `{shortsha}` `[種別] 概要`
- 次: {次にやることがあれば 1 行}
```

---

## 2026-09-23（#1278 / #583: CI の Windows の cargo test を blocking にした）
- `continue-on-error: true`（#583 の据え置き）のあいだ**新しい赤が見えなかった**のが本題。全数を `--no-fail-fast` の blocking へ（既定の cargo は最初に落ちたバイナリで打ち切るので、main の CI は tako-app の 1 件で止まり tako-control / tako-core が 0 件実行だった = 1 回で 1 件しか直せない）。名指しの実行検査（#1282 / #1314）は全数より手前へ move
- 4 往復で収束: **21 → 2 → 0 failed**（1 巡目 failure → 3 巡目 success が「blocking になった」実証。わざと壊す必要は無かった）。21 件のうち **13 件は 9/9 のベースライン 24 件に無い** = その後 main へ増えた未検出ぶん。テスト側を直した 20 件は期待値を製品の正から作る型（`current_recipe` / `cli_file_name` / `Path::join` / `join_paths` / `temp_dir`）+ `display()` 比較を `Path` 比較へ + `-EncodedCommand` の復号 + 自前 symlink。ランナー固有は 8.3 短縮名 / git identity 不在 / `core.autocrlf=true` / pwsh 自身の書き込みの 4 系統
- 理由つき skip は **10 件すべて追跡番号つき**。製品側の実バグは #1557 / #1569 / #1571 / #1581 へ起票（製品コードは 1 行も触っていない）。番犬は 2 規則（理由なし ignore を落とす / Windows だけの skip に `#<数字>` を要求）で、`ci_windows_test_compile` の「非ブロッキング据え置き」は反転した

## 2026-09-23（#1547: docs の数値・Issue 参照のズレを直し、docs ビルドと og / リンク検査を CI へ）
- ヒーロー統計 128 / 68 → **152 / 86**（同じページの本文は既に 152 / 86 = 番犬が本文しか見ていなかった）とエージェント 4 ページの 47 → 52 件系。追跡先が closed だった **13 マス**（#757 / #983 / #984 / #1033 / #1067）は open な親エピック #975 へ寄せ、閉じた番号は Note 本文の引用として残した（能力の申告は 1 マスも変えていない）
- 手書き 2 か所は**事実そのものが古かった**: MATRIX の `restore_after_reboot` は codex / agy とも supported（#1238 で配線済み）なのに「PC 再起動後の復元は claude 専用」と書いていた。残る 4 か所（#127 / #357 / #986 / #1013）は根拠としての引用なので引用と読める形へ整えて残した。規約「追跡先は open / 根拠は closed でよい」を conventions.md へ
- 実測: 番犬の注入 **11 通り**すべて file:line 名指しで FAILED → 戻して緑（追跡先検査 3 通り・リンク検査 2 通り・robots 削除も exit 1）。docs ビルド 32 ページ / og:verify 31 / verify-links 内部リンク 1573 本・断片 23 本 OK。workspace 5174 passed 0 failed・clippy 3 宇宙 0・check-windows error 0

## 2026-09-23（#1544: orchestrator projects list を dispatch 経由へ寄せた）
- `add` / `remove` は #1453 で dispatch へ寄せたのに `list` だけ `ProjectsConfig::load()` の直読みが残っていた（同じ関数に「直したもの」と「残したもの」が並ぶ = #1453 で実際に壊れた形）。3 分岐すべてを `dispatch_orchestrator_projects` の 1 本へ
- 実測: 隔離 `TAKO_ORCHESTRATOR_DIR` での前後 A/B **7 ケースすべて stdout / stderr / exit がバイト一致**（未登録 / 空 / 3 件 / 桁境界 15・16・17 + 日本語キー / 空 desc / 壊れた YAML=exit 1 / 空ファイル）。本番 projects.yaml は sha256 一致で無改変
- 番犬 `issue1544_projects_dispatch_watchdog` 3 本（注入 4 通り + 空振り検査）。直読みを戻す注入で `main.rs:4537` を名指しして FAILED → 戻して緑

## 2026-09-23（#1504: Windows のシェル統合を tako setup の段として入れた）
- `shell_integration::install()` の呼び手が CLI と MCP だけで **setup も installer も呼んでいなかった**（棚卸し Z9）ので、Windows は人が `tako shell-integration install` を打つまで OSC 7 / 133・cwd 追従・入力予測・自動命名の素材が死んでいた。段を `tako setup`（bootstrap より前）と `--check` へ 1 実装で通し、**配置が要るかは `cfg!(windows)` ではなく `Delivery` で分岐**させたので、macOS 上でも Profile 経路の表示・冪等・失敗を全部検査できる（実機を持たない CI の穴を作らない）
- 倒した判断: ユーザーのファイルへ書く前に「何をどこへ」を出して**同意扱いで続行**（`[y/N]` を出さない = `--yes` / 非 TTY / 端末ありで出力が一致。先例は #1502 の PATH 設置）／失敗は `RemainingKind::ShellIntegration` として末尾へ（段は `Result` を返さないので `?` で setup を止められない）／2 回目は予告も出さず差分ゼロ
- 実測: `scripts/test-setup-shell-integration-1504.sh` **54 PASS 0 FAIL**（CI 登録。隔離 HOME で `$PROFILE` のマーカーが HOME のどこにも書かれないことを毎回確認）・A/B `TAKO_1504_LEGACY=1` は 34 PASS（`<data_dir>/shell-integration` が作られない = #1504 前の症状）・注入 11 通りすべて FAILED → 戻して緑（段を外すと実経路が 21 FAIL）・回帰 5 本（#1499 52 / #1501 81 / #1503 28 / #1509 49 / multiagent）全緑・workspace 5157 passed 0 failed・clippy 3 宇宙 0・check-windows error 0

## 2026-09-23（#1557 / #1581: pid の生存判定を境界の 1 実装へ寄せた）
- `remote.rs` の私的 `is_process_alive` は非 unix で**無条件 false** = Windows で生きている daemon を全部「居ない」と読み、`daemon_status` の `running` も pid 再利用時の誤 kill 防止も掃除判断も揃って誤っていた。`tako_core::platform::process::pid_alive` へ委譲。macOS も EPERM は「居る」・`pid_t` 範囲外は「居ない」へ揃う（どちらも state を消さない / 撃たない側）。偽の緑だった `is_process_aliveは存在しないpidをfalseで返す` に `u32::MAX` / `0` を足した
- **#1581 の案 1 は既に入っていた**: `test_residue.rs` は `ports::process_alive` を一度も使っておらず（`git log -S` 0 件）、`OwnerProbe` は #1296 の初版から境界を通っている。失敗テストは子の終了直後に数える形で、残る 3 件は**子自身の pid** = 起動時の掃除では原理的に直らない（`tako-agent-config-` は `auto: false` でそもそも対象外）。案 2 / 案 3 は #1581 に残す（PR は Refs のみ）
- 実測: 番犬 5 本・実ファイル注入 3 通りすべて file:line 名指しで FAILED → 戻して緑・workspace 5147 passed 0 failed・clippy 3 宇宙 0・check-windows error 0・`windows-support.md --check` 同期。Windows 実機は CI ジョブが唯一の実行証拠

## 2026-09-23（#763: リンクを開く修飾キーをプラットフォームごとに 1 箇所で決めた）
- リンク経路 13 サイト（ターミナルのクリック / cmd+右クリック #1182 / ホバー 6 / md・PDF のクリック / リリースノート）が `Modifiers::platform` を直読みし、Windows は Win+クリック要求だった。判定を `tako_core::platform::keys::link_modifier_active(platform, platform_key, control)`（macOS = command のみ / Windows = control のみ。`platform || control` を素で足すと macOS の Ctrl+クリック = 右クリック相当と衝突）へ寄せ、GPUI 側は `keybindings::link_modifier_active` / `link_modifiers` / `non_link_modifiers` の 3 本だけが `Modifiers` を触る形にした。表記も同じ表を見る `keys::link_click` で MCP カタログ・CLI ヘルプ・docs が実行 OS に追従する
- 実測: 番犬 `issue763_link_modifier_watchdog`（4 規則）へ注入 12 通りすべて file:line 名指しで FAILED → 戻して緑・隔離 GUI（tako-vd）のセルフテストが `TAKO_APP_SELF_TEST_OK` 完走（310 秒 / 240 診断行）で `TAKO_SELF_TEST_763: file_click=true dir_click=true wrong_modifier_opened=false`・workspace 5204 passed 0 failed・clippy 3 宇宙 0・check-windows error 0
- 次: Windows 実機での Ctrl+クリック実測は #467 の実機レーンで（offline のため未検証）

## 2026-09-23（#1540: MCP ツールカタログの説明文を圧縮し、Issue 番号を落とした）
- AI が引けない Issue 番号を description / inputSchema から**195 箇所すべて**落とし（根拠は `// 出自: #…` のソースコメントへ 83 ツールぶん移送）、`tako_orchestrator_worker_status` を 3,587 → 1,566 字へ。残り 76% は識別子なので地の文は 368 字（これ以上は応答キー・enum 値を捨てることになる）。ダイアログ構造は `tako_orchestrator_respond`、`prompt_delivery_failure` の値は `tako_orchestrator_workers`、`delivery` の項目は `tako_read_pane` を正本に寄せた
- `next_step` / `degraded` を返す 9 ツールへ読み方を明記し、`tako_setup` の `orchestrator` / `sleep_guard`（カタログ唯一の description 欠落）を補い、導線の無かった 6 ツール（`select_tab` / `recent` / `git_push` / `git_pull` / `preview_undo` / `redo`）へ前提ツールを足した。規則は `.agent/conventions.md` の新節、上限は #1539 の 210 KB → **200 KB** へ締め直し
- 実測: カタログ 201,535 → 198,830 バイト・52,028 → 51,076 トークン（tiktoken `o200k_base`）・Issue 番号 195 → 0。番犬 `mcp説明文にissue番号を書かない` へ注入 3 通りすべてツール名指しで FAILED → 戻して緑。識別子は**カタログ全体で消失 0**（機械照合）・workspace 5172 passed 0 failed・clippy 3 宇宙 0・check-windows error 0

## 2026-09-23（#1578: CLI 出力から絵文字を消し、is_emoji の番犬を CLI へ広げた）
- 本番リテラルの実測は 18 件（Issue の `ℹ`5 / `⚠`1 に加え `✓`4 / `✗`3 / `❯`6）。14 件を文字ラベルへ置換し、**語彙は発明せず `setup.rs` から引いた**（同じ文言を setup.rs は既に `[OK] …` で出していて main.rs だけが取り残されていた）。同じ列の `─` / `△` も揃えないと混在列になるのでその 2 つの match だけ寄せた
- 走査は `tests/common/emoji_scan.rs` の 1 実装へ寄せ #1536 の番犬も載せ替えた。残る 4 件（`mcp/catalog.rs` の `❯`）は AI だけが読むツールカタログなので理由つき ALLOW。規約は `.agent/conventions.md`「絵文字を出さない」節
- 実測: 隔離 CLI の before/after 4 経路・origin/main の形へ戻す注入で 14 件すべて file:line 名指しで FAILED → 戻して緑（#1536 は注入中も緑）・workspace 5178 passed 0 failed・clippy 3 宇宙 0
