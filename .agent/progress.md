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

## 2026-09-23（#1545: getting-started を 9/22 の setup 着地へ追従させた）
- 旧ページは **#1502 が明示的に否定した PATH 手順**（`.app` の実行ファイル置き場を `~/.zshrc` の `export PATH` へ）を読者に指示し続け、同じページ内で「CLI は導入しない」（トラブルシューティング）と「導入 → PATH → ログインの 3 段を案内する」（tip）が矛盾していた。FR-2.14.5 の現行仕様（`$HOME/.local/bin/tako` の symlink + `~/.zprofile`）へ差し替え、旧手順を踏んだ読者向けに「その行は消してよい」の移行案内を足した
- 任意依存が `[y/N]` で導入まで通ること（#1499 / #1509 / #1524）と、詰まった段があっても止まらず「残り N 件」で終わること（FR-2.14.12 = #1501）を追記。`--version` / `--changes` / `--check` の例は現行ビルド（v0.8.17 / rev 19）の実出力へ。「質問ゼロ」3 行を言い直し、次のステップに `/features/remote/` と `/guides/keyboard-shortcuts/` を足した
- 実測: 貼った `--check` は隔離 HOME の再実行と**マスク以外バイト単位で一致**（49 行）・`Contents/MacOS` は docs 全体で 0 hit・`npm run build` 32 ページ警告 0・`verify-og` 31 ページ OK

## 2026-09-23（#1546: releases.md に v0.8 系を書き、最新の安定版の位置を移した）
- Issue の「現行 v0.8.17」は実態と違った: `gh release view` で v0.8.0 だけ `isPrerelease=false`（Latest）、v0.8.1〜v0.8.17 は全部 prerelease。夜間リリースは常にテスト版で出て昇格は `release.sh --promote` の手動（`.agent/release.md`）なので、見出しは「v0.8 系（2026-08-28 〜）— 最新の安定版は v0.8.0」とし、2 種類の関係を節冒頭に表 1 枚で置いた。柱は 4 つ（リモート刷新 / ユーザータスク / setup の代行化 / エージェント間の同等化）
- ブリーフの柱から 2 件を落とした: **#1500 は 9/22 着地だが v0.8.17（9/22 05:00 の夜間版）に載っておらず CHANGELOG にも節が無い = 未リリース**（代わりに v0.8 で実際に出た #1057 / #989 / #1002 で組んだ）。Web ビューは v0.4.0 の機能で v0.8 にあるのは #1481 の修正だけ。本文の記述 60 件はすべて Issue 番号で CHANGELOG の v0.8 範囲と機械照合（不一致 0）
- 番犬 `docs/scripts/check-releases-page.mjs` を新設し CI の macOS ジョブへ 1 ステップ。現行の版は **Cargo.toml / CHANGELOG / git tag の最大値**（checkout は `fetch-depth: 1` でタグを持たないので Cargo.toml が主）。落ちるのは A 系列の節が無い / B 先頭でない / C ラベルが古い節に残る、の 3 つ。注入 4 通り落ちる + 偽陽性 1 通り通る。docs ビルド 32 ページ成功・警告 0 → 0・初稿の `**…（日付）**です` が right-flanking 条件を満たさず素の `**` で出ていたのを実測で発見して修正

## 2026-09-23（#1536: UI の絵文字を GPUI の描画プリミティブへ置き換えた）
- Issue が名指しした ☕ / 🌐 / 📌 / ⏏ は #217 で既に SVG 化済みで、残っていたのは 4 箇所 = `right_panel.rs` の `⠿`（U+283F）→ `ui_icon::GRIP` と `⬆`（U+2B06）→ `UNSHELVE`（**どちらも定数とアセットはあるのに参照 0 件 = 未配線**）・`preview_render.rs` の `↔`（U+2194）→ 新設 `SWAP`・`ui_text/preview.rs` の `▶\u{fe0e} 再生` → 語だけにして `PLAY` を render 側で並べる。最後の 1 件は**旧カタログ検査の範囲に U+25B6 が無くてすり抜けていた**形で、異体字セレクタでテキスト表示へ倒す書き方は逃げ道として認めない
- 判定を `tako_core::emoji::is_emoji`（Unicode の Emoji プロパティ）の 1 実装へ寄せ、`ui_text` のカタログ検査と新番犬の両方が呼ぶ。番犬 `issue1536_no_emoji_ui_watchdog` は `crates/tako-app/src` の**本番コードの文字列リテラルの中身だけ**を見る（テスト領域は `production_range::scan`・コメントは新設した `code_view::literals_only` が落とす・`\u{XXXX}` も復号）。例外はファイル × 文字 × 件数 × 理由で、`main.rs` のセルフテストが流す claude TUI の画面データ 6 種 21 件だけ
- 実測: 注入 12 通りすべて一致（絵文字を戻す 8 通りは file:line 名指しで FAILED・対照 4 通り = コメント / `#[cfg(test)]` / `×` / FE0E 単独は緑）。visual-test に `no-emoji` 節を足し、隔離 GUI（tako-vd）で復帰ボタンの実矩形が **14 色**。A/B で `ui_asset!("unshelve")` を外すと **1 色**（#562 の登録漏れ = 無言で描かれない）で落ちる。workspace 5118 passed 0 failed・clippy 3 宇宙 0

## 2026-09-23（#1554: 復元の内訳が全ペインを説明するようにした）
- 「復元成功: N タブ / M ペイン（…）」の内訳合計が M と合わない行が本番 52 行中 8 行あり、個別の spawn 失敗は `eprintln!` 止まり（GUI の stderr はどこにも出ない）で痕跡ゼロだった。AGENTS.md が「再起動後にエージェントが戻らないとき」の正本として案内している診断そのものの穴。無言の `continue` は Issue の 2 経路ではなく 5 経路（たまり場 / 退避タブ配下 / Web ビュー / spawn 失敗 / resume 入力の宛先なし）
- **実測で差の正体は「たまり場・退避」だった**: 合わない 8 行はすべて #1487 着地（9/21）以降で、そのときの layout.json は 17 ペイン中 1 件がたまり場・1 件が退避タブ配下（Web ビューとプレビューは 0 件）。この 2 種は「表に出すときに起こす」設計なので失敗にせず別カテゴリで数える。件数・1 行目・2 行目の正本を `tako_control::restore_report` へ寄せ、区間（ラベルと件数）の列が合計の出どころ = カテゴリを 1 つ落とすと合計も落ちる形に。個別の失敗は `復元失敗（ペイン N）: <分類>: <エラー>` を 1 行ずつ、それでも合計が合わなければ食い違い自体を 1 行（FR-5.7.1 / A/B は `TAKO_1554_LEGACY=1`）
- 実測: `scripts/test-restore-breakdown-1554.sh` **39 PASS 0 FAIL**（隔離 GUI で実 tako-app を 4 回起動。CI 未登録 = 実 GUI が要る）・単体 9 + 番犬 8・注入 12 通りすべて file:line 名指しで FAILED → 戻して緑・workspace 5144 passed 0 failed・clippy 3 宇宙 0・check-windows error 0。[提案] = `$VAR` の直後の全角で bash が変数名へ取り込む罠（番犬候補）/ たまり場・退避のペインが復元で器を引き継がず「復帰」タブへ別ペインとして戻る（実測済み・要 Issue）

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
