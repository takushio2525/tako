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

## 2026-10-24（#2359: GUI の fd 上限を起動時に min(hard, 10240, kern.maxfilesperproc) へ上げ、ペインの PTY の子には起動時の値を戻す・check-health に `fd_limit` 節と 80% の warning）
- 本番 28 ペインで 121 / 256 本（4 本 / ペイン）→ 再起動後 10240・枯渇の注入で修正前 0/5 → 修正後 5/5 通る・カタログ +0 B。器の tmux の天井は #2367。PR #2364

## 2026-10-24（#2357: 実 tmux のテストの間欠の真因はテスト = 「読むと消す」`take_pane_died_dump` を書き出しの出現で叩き、書きかけを消していた → 器が消えたと `probe_session` で確かめてから 1 回読む・同型 4 か所も）
- 注入で main は CI と同文言で FAILED → 修正後 ok・30 回 × 並列 1 / 4 で全通過・カタログ +0 B。PR #2366

## 2026-10-24（#2360: 起動時の IPC ソケットの生存確認が connect 1 回で、control.json の無い 2 つ目が満杯の生きた受け口を奪っていた → `ipc_socket::judge_occupant` が持ち主の pid と繋ぎ直しまで見る・奪わなかった 2 つ目はセカンダリ・`ipc.fixed_path`）
- 実経路 修正前 27 PASS 22 FAIL → 修正後 49 PASS・A/B `TAKO_2360_LEGACY=1`・カタログ +0 B。net_stop の間欠は #2369。PR #2368

## 2026-10-24（#2355: 項目 154 の E4 の間欠の真因はテスト = 流していたシェルのループが ^C を取りこぼす（素の zsh で 450 回中 9 回）→ ループをサブシェルで包み、止まったことを状態で待つ）
- A/B `TAKO_2355_LEGACY=1` + 注入 `lostint` で旧は FAILED・新は通過・5 回完走・カタログ +0 B。漏れの判定の偽の緑は #2371。PR #2370

## 2026-10-24（#2369: net_stop の到達確認のテストが listener を drop した番号を「閉じたポート」に使い、並列の spawn が fd を子へ持ち出すと繋がっていた → 繋いだソケットの手元側の番号を握って使い、`ConnectionRefused` で落ちることも確かめる）
- 持ち出しの注入で修正前 FAILED → 修正後 ok・全体と並列 30/30・カタログ +0 B。同型の LSP テスト 2 本は #2373、CLOEXEC の非原子性は #1788 へ。PR #2374

## 2026-10-24（#2367: 器の tmux サーバーへ引き上げた fd 上限を渡す = attach クライアントだけ戻さず、器のペインの子は GUI 自身の段 `__tako-container-exec` で起動時の値へ戻してから tmux と同じ形で exec。check-health の `fd_limit.container`・FR-2.2.16）
- 実経路 22 PASS（修正前は soft 64 で window 53 本・新しいペインが消える → 修正後は 10240・器のシェルは 64 のまま）・カタログ +0 B。17:45 に install・本番の器は soft 2560。PR #2376

## 2026-10-24（#2352: 番犬の 3.2 の写しが行継続を 3.2 と同じ文脈で取り除いてから読む = `join_continuations32`。波括弧・pipefail・rm の 3 本が通す・表に行継続の飾り）
- 突き合わせ 修正前 63 箇所 → 0・注入で 63 に戻る・リポジトリの名指しは前後とも 0・カタログ +0 B。申し送り #2378。PR #2375

## 2026-10-24（#2365 の 1 本目: dispatch と前段（CLI / MCP）の理由の文と orchestrator layout の値の断りを `tako_control::Reason` / `tako_core::LayoutParamError` の日英の網羅 match へ。MCP の stdio ブリッジは設定から表示言語を決める）
- 実経路 修正前 PASS=69 FAIL=30 → 修正後 PASS=99・日本語表示 66 本バイト一致・注入 4 通りで file:line を名指し・カタログ +0 B。残りは #2365 の続き。PR #2379

## 2026-10-24（#2361: Windows の accept ループがパイプのインスタンスを作れないと break して止まっていた → `ipc_socket::create_with_retry` が 100 ms から倍々・最長 5 秒の間で作り直す・persist.log は途切れの始まりと戻りの 2 行・check-health に `ipc.pipe`）
- Windows CI で修正前 FAILED → 修正後緑・番犬の注入で ipc.rs:498 を名指し・カタログ +0 B。Windows 実機は未確認（manual-checks に手順）。PR #2380

## 2026-10-24（#2373: LSP の取得のテスト 2 本（1944 のオフライン / 1956 の確かめ落ち）が drop した listener の番号を「オフラインの配布元」に使い、子へ持ち出されると繋がっていた → #2369 と同じ「繋いだソケットの手元側の番号を握る」形を `tests/common/closed_port.rs` へ）
- 持ち出しの注入で修正前 2 本 FAILED → 修正後 ok・並列 30/30・カタログ +0 B。PR #2381

## 2026-10-25（#2365 の 2 本目: tako-core のエラー型 10 種（編集 4・Session・Runner・CommandCard・Backend・LSP の ParseError 2）を `text_in` の日英へ・「まだ実行中」は `says_still_running`）
- 英語表示で日本語入り 14 → 0・日本語はバイト一致・注入 2 回で番犬が file:line を名指し・カタログ +0 B。PR #2383

## 2026-10-25（#2371: セルフテスト項目 154 の漏れの判定が器越しのペイン（history=0）で画面の外へ流れた漏れを見落とす偽の緑 → `st2371_assert_no_leak` がグリッド + 器の履歴を読む・項目 155 も同じ判定へ）
- 修正前 + 注入 `leak` で通過 → 修正後は「器の履歴」と名指して FAILED・A/B `TAKO_2371_LEGACY=1`・カタログ +0 B。PR #2377

## 2026-10-25（#2378: 番犬の 3.2 の写しの字句が `"${ … }"` の中の `'…'` / `$'…'` を 3.2 と同じに引用符として読む = `ParamCtx`）
- 突き合わせ 修正前 28 箇所 → 0・注入で 28 に戻る・リポジトリの名指しは前後で 0・カタログ +0 B。PR #2386

## 2026-10-25（#2284 の再発: 真因はテスト = 看取った `Child` を drop してから読み、Windows が再利用した pid の別プロセスを `Alive` と読んだ → `process::ReapedChild` を持ったまま読む）
- 注入 `TAKO_2284_INJECT_REUSE` で修正前は CI と同じ `lid.rs:1071` で FAILED → 修正後 ok（macOS / Windows）・カタログ +0 B。PR #2385

## 2026-10-25（#2387: issue1977 の LSP の古い版の片付けのテストの `dead_pid()` を `reaped_child()` へ。test_data_residue の `reaped_pid` は対象外として conventions に例外を記録）
- 注入で修正前 `:548` FAILED → 修正後 ok・並列 24/24 × 2・カタログ +0 B。PR #2389

## 2026-10-25（#2350: 端末の問い合わせ（OSC 10/11/12/4・CSI ? 996 n・CSI 14 t）に答え、mode 2031 を購読した器（tmux）へテーマの切り替えを知らせる・OSC 52 の読み出しは断る・check-health の `terminal_queries`）
- 修正前は色と 14 t が無応答（器越しは OSC 4 の後 5.5 秒止まる）→ 修正後は両方で答えテーマにも追従・カタログ +63 B。PR #2382

## 2026-10-25（#2365 の 3 本目: 保存の断りに添える案内を `Reason` へ移し、プレビュー・編集・ファイル操作・ペイン / タブ操作の断り 34 種を足した（51 → 85 種））
- 実経路 修正前 PASS=157 FAIL=44 → 修正後 PASS=201・日本語表示 63 本バイト一致・注入 4 通り・カタログ +0 B。PR #2390

## 2026-10-25（#2393: 実 tmux のテストが `kill-server` の後にソケットファイルを残していた件を、失敗の経路も含めて `remove_socket_file` / `kill_server` する形へ。番犬 `テストが落とした器はソケットファイルまで消す`）
- 修正前 1 回 5 個 → 修正後 0（失敗の経路の注入も 0）・番犬は main の版で 10 か所を名指し・カタログ +0 B。PR #2396

## 2026-10-25（#2391: 走らせた人の env（TAKO_ISOLATED / TAKO_SELF_TEST / TAKO_DATA_DIR）で結果が変わるテスト 6 本を、env を組んだ子プロセスで見る形へ）
- tako-core 全テスト × env 5 通りの修正前の赤 → 修正後は全部緑・tako-control + tako-cli の 4 failed → 0・注入 3 通り・カタログ +0 B。PR #2397

## 2026-10-25（#2365 の続き: `tako mod ui` の断り（UiError 一式）を日英の網羅 match へ。setup / persist.log / migrate は日本語固定）
- A/B 25 件で日本語はバイト一致・英語表示の日本語 25 → 0・注入 5 通りで file:line を名指し・カタログ +0 B。PR #2394
