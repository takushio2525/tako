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

## 2026-11-04（#2365 の続き: `tako queue <コマンド>` の進みの行（stderr の待ち・開始・諦め・素通しの理由・取り消し）を `ReplyNote` 13 種で、右パネルの usage ビューの数・期間・注記・段の名前を CLI と同じ 1 実装（`display::*_in` / `group_row_name_in` / `usage::notes_in`）で、resources ビューの待つ理由の英語を `ReplyNote::QueueWaitReason` で表示言語へ。日本語の口 4 つを外した）
- CLI の実経路 9 ケース × 日英 × 修正前後で英語表示の日本語入り 15 → 0 行・日本語は全 36 ケースがバイト一致・終了コードの差 0。tako-vd の隔離 GUI で usage ビューの英語の日本語入り 5 → 0 行（日本語は一致）・注入で名指し + E0004・カタログ +0 B。PR #2717

## 2026-11-04（#2711: visual-test の項目 152 は件数を置いたのと同じ update で一覧の着地を捨てさせ（`ListPoll::hold_landings`）、tasks-input は測る間 App Nap を止めて送信ごとに直前の空押しと組にする・A/B `TAKO_2711_LEGACY`・注入 `land` / `step`・番犬 `selftest_snapshot_hold_watchdog`）
- tako-vd で 152 単独 15/15・tasks-input 単独 11/11・注入 land / step は新で緑・旧で名指し NG・全節は描けた 6/6 で両方を通って viewport-lines（#2714）まで・カタログ +0 B。PR #2718

## 2026-11-04（#2714: visual-test の viewport-lines / editor-font の追従の判定を `effective_margin` の設計値（`follow_design`）へ寄せ、fixture を `.txt` に（全節の LSP の帯の割り込みを外す）・前提の確かめ・A/B `TAKO_2714_LEGACY=1`・番犬 `issue2714_follow_design_watchdog`）
- 実 GUI 6 腕 4/4 PASS=16・全節（#2718 を一時的に重ねて）新 3/3 で viewport-lines を越える・カタログ +0 B。PR #2720

## 2026-11-04（#2365 の続き: `tako search`（横断検索・置換・置換の取り消し）と `tako jump` の人向けの表示の枠を `ReplyNote` の 23 種で表示言語へ。行の中身は GUI が表示言語で組む応答の値のまま・`--json` は不変。agent-support は前から表示言語だったので残りの表から外した）
- 偽 GUI に対するビルドした CLI の 20 ケース × 日英 × 修正前後で英語表示の日本語入り 21 → 0 行・日本語は全ケースがバイト一致・終了コードと JSON の差 0・注入で名指し + E0004・カタログ +0 B。PR #2719
- 07:50 に `e70d5b2`（PR #2719 まで）を install

## 2026-11-04（#2228 の 1・2: 手動検証スクリプト 6 本と GUI セルフテスト 2 か所のスタブを空打ちの補助へ通し、番犬を広げた。補助の冪等の判定の取り違えも直した）
- 注入で修正前は 2151 / 1797 / 1507 / 2182 / セルフテスト #601 が落ち修正後は通る・1730 / 1506 / 1733 は修正前も落ちない（理由は PR）。番犬: issue2228 がセルフテストの実行権の直付け、shell_scripts が test-*.sh の素の chmod の増加（103 本・189 か所を凍結）を file:line で名指す・カタログ +0 B。PR #2722

## 2026-11-04（#2365 の続き: `tako context-budget`（check / fix）と `tako test-residue` の人向けの表示の枠を `ReplyNote` の 21 種で表示言語へ。提案の `advice` は表示言語で選ぶ（日本語に固定だった）・残骸の種別の説明と判定の理由は tako-core の日英の対（`--json` の `note` / `detail` は日本語のまま））
- ビルドした CLI の 14 ケース × 日英 × 修正前後で英語表示の日本語入り 95 → 0 行・日本語は全ケースがバイト一致・終了コードの差 0・注入で名指し + E0004・カタログ +0 B。PR #2723

## 2026-11-04（#2721: visual-test の tasks-input の送信の間を、押下が返った時点の状態（送信中・未掲載）+ 配送そのもの（`dispatch_event` の内側）の差で判定し、区間全体（visual-test だけの同期の描き直し約 48 ms を含む）は空押しの幅 8 ms 未満の静かな回だけ判定・揺れた回は未実測と名指し・判定は `send_lag_2721` の純関数・A/B `TAKO_2721_LEGACY=1`・注入 `replay` / `preempt` / `heavy` / `defer`）
- tako-vd で単独 7/7（混んだ 2 回は全体を未実測と名指し）・replay は新で 3/3 緑（旧 2/2 NG）・heavy / 旧 #2016 / defer は新で名指し NG・全節 3/3 で tasks-input を通過（止まりは md-edit-resume = #2716）・カタログ +0 B。PR #2724

## 2026-11-04（#2478: セルフテスト項目 128（#945）の脈動の前提を、固定 1500ms ではなく `shell_marks().prompts` を待ってから Running を注入する形へ。測っている間に状態が動いた回は前提の崩れとして名指す。番犬 `selftest_wait_watchdog` の `inject_before_first_prompt`・A/B `TAKO_2478_LEGACY=1`・注入 `TAKO_2478_INJECT=slow-shell:<ファイル>`）
- tako-vd で旧 + 注入は ② で FAILED（`prompts=0->1->2 state_late=Idle`）→ 新は通る・注入なしで項目 128 まで届いた 6 回すべて通過・カタログ +0 B。PR #2725

## 2026-11-04（#2690: CI 待ちのモックテストの間欠の真因 = /bin/bash 3.2 は SIGCHLD を SA_RESTART なしで受け、`done < <(sort …)` の子の終了が次の結論の行の write に重なると EINTR でその行が消えて set -e で 1 に化ける → wait-pr-checks.sh / merge-pr.sh の 3 か所を `done <<<"$( … )"` へ・番犬 `set_eの下で書き込みの最中に届く子のsigchldを増やさない`（既存 27 箇所は凍結））
- 「いつも満杯」の FIFO で修正前は 3.2 で 9 回とも CI と同じ `line 506: echo: write error` で崩れる → 修正後 0（3.2 / 5.3 × 詰め物の窓 0 / 3 ms を 20 回ずつ、PASS=473）・SIGCHLD を塞ぐと崩れない・番犬の注入 5 通りを名指し。合流検証の bash 5 の落ちは再現の道具の欠陥（詰め物の O_NONBLOCK を dup で腕と共有 = EAGAIN）で、FIFO の別の口へ直した・カタログ +0 B。PR #2728

## 2026-11-04（#2716 段 2: visual-test の全節実行の持ち越しで止まる 4 節（md-edit-resume / lsp-context-menu / chat-table / bracket-match）を節が場面・言語サーバ・読み込みを自分で決める形へ（`vt2716_own_scene` / `vt_lsp_override` = restart で覚えを捨てる / drain / 落ち着き待ち）。A/B 4 節とも旧は名指しで FAILED・新は OK、番犬 4 本）
- 全節を成功の出口まで通す確認と #1899 の 3 節の載せは #2726 の後。md-find-restore の間欠（全節 1/4）は別の型で残り・カタログ +0 B。PR #2730

## 2026-11-04（#2736: #2690 の凍結分のうち release.sh 7・test-release-retry.sh 5・lib/launch-services.sh 1 を `done <<<"$( … )"` へ寄せて凍結の表を 27 → 14 箇所に。割り込み方を実測で訂正 = 割り込まれるのは子が生きている間にループの本体で書く行（#2690 の line 506 は最後の行の echo そのもの）で、本体で書かない / 本体に前景の子があるループは落ちない）
- いつも満杯の FIFO で修正前の 3 本は落ちない（release.sh 25 回・test-release-retry.sh 3 回・掃除 23 回）・本体で書く最小の形は 3.2 で 20 回中 19〜20 回・残り 14 箇所を実害の条件で棚卸し（当たりうるのは test-serve-watch.sh:249 と virtual-display.sh:503）・カタログ +0 B。PR #2739

## 2026-11-04（#2726: visual-test の項目 109 の 3 択とセルフテストの 97(e) / 739 が本物の claude / codex / agy を起こしていた → 109 は PATH の先頭の偽物で確かめてから押す・スターターは積んだ行を `withdraw_launch`・82b はコンフリクトの事前確認・番犬 `fake_agents_2726` が 3 形を file:line 名指し）
- 全節（届いた範囲）/ 109 ×3 / セルフテストで ps の本物の対話起動 0・ブラウザ 0・SPY の端末つき 0（修正前 1）・カタログ +0 B。PR #2738

## 2026-11-04（#2740: #2736 で「当たりうる」とした 2 箇所を 3.2 に注入して実測 → test-serve-watch.sh の偽のピアは kill の後に刈り取らず見出しを echo して落ちる（切り出しで 20 回中 9 回）ので kill の直後に wait + `# tako:sigchld-ok` で凍結表から外した。virtual-display.sh の caffeinate は実経路（別プロセスの ensure・set -e なし）では 60 回中 0 回なので残した）
- 凍結表 14 → 13 箇所・修正後は 3.2 / 5.3 とも 0 回・棚卸しの表を実測で更新・カタログ +0 B。PR #2742

## 2026-11-04（#2727: セルフテスト項目 115 の注入した会話が 2 秒 tick の定期更新で消えて `display=Starter` で切れていた → 差し込みの update で pin し、その場の 1 周で「会話が残る」を確かめる。#739 の項目に選ぶ直前のペインの診断。番犬 `chat_fixture_pin_watchdog`・A/B `TAKO_2727_LEGACY=1`）
- tako-vd で旧 1/1 FAILED（`kept=false`）→ 新 5/5 + 最後まで OK・#2741 の回 0・カタログ +0 B。PR #2744

## 2026-11-05（#2365 の続き: `tako remote start` / `stop` / `status` の stderr の行を `ReplyNote` の 11 種で表示言語へ。旧版・起こしている途中・自己検査の停止の説明は `remote_generation` / `remote_starting` / `remote_serve` の `*_in` が応答の構造の欄から組む（stdout の JSON は日本語のまま））
- ビルドした CLI の 11 ケース（本物の daemon と差し替えを含む）× 日英 × 修正前後で英語表示の日本語入り 25 → 0 行・日本語と JSON は修正前とバイト一致・終了コードの差 0・注入 7 組で名指し + E0004・カタログ +0 B。PR #2748

## 2026-11-05（#2747: 番犬 `chat_fixture_pin_watchdog` の既知の残り 9 件を確かめ、本当の pin 漏れは visual-test #745 だけ（旧は会話が消えても消える前の配置を測る偽の PASS）→ 差し込みの update で pin + その場の 1 周 + 測る時点で会話が残る検査・A/B `TAKO_2747_LEGACY=1`。残り 8 件は守られている形（次の `.await` より前に片付ける = 番犬の規則・それ以外 = 理由つきの印）で、表は空になり削除）
- tako-vd で旧 PASS=3（名指しで FAILED）・新 3/3 緑・カタログ +0 B。PR #2749

## 2026-11-05（#2737: 狭いペインの描画の Markdown で ⌘F → Enter のヒットが下に隠れていた → 追従の後に描いた行の実寸で確かめ直して追い直す（描画の頭・上限 3 回・A/B `TAKO_2737_LEGACY=1`・番犬 `issue2737_follow_recheck_watchdog`））
- #2716: 全節実行と言語サーバの節は `TAKO_LSP_AUTOSTART=1` が前提 = 欠けたら `TAKO_VISUAL_TEST_UNMET`（終了コード 3）で名指して止める。次は python RGBA の段 1・カタログ +0 B。PR #2750
