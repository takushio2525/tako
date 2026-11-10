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

## 2026-11-10（#2867: 番犬 ui_key_notation の規則 A / B が `\u{…}` を小文字の綴りだけで探していた → 値で読む `mark_hits` を足して規則 C と 1 実装に寄せた）
- 修正前は大文字 `\u{21E7}` の注入が素通り → 修正後は規則 A / B / C とも file:line で名指し・リポジトリ全体は新たな名指し 0・カタログ +0 B。PR #2872

## 2026-11-10（#2855: ssh の ControlPath が深い TMPDIR で上限を超えていた（OpenSSH の master は `<ControlPath>.<16 文字>` へ bind するので上限は 86 バイト。旧い安全域 92 は 87〜92 がすり抜け、ログインの後に exit 255 = 使い捨て sshd で実測）→ 境界 B35 `platform::unix_socket::place_with`（IPC と 1 実装）で data dir → $TMPDIR → /tmp の収まり書ける最初へ、無ければ多重化なし（`multiplexing_for`））
- 一巡の番犬の SKIP_IN_CHILD を外した・注入 I1〜I3 で名指し・Windows は B26 で不変・カタログ +0 B。PR #2873

## 2026-11-10（#2850: Windows の clippy の指摘 13 件（宇宙別 12 / 10 / 8・製品 10 / テスト 3）を 0 件にし、windows ジョブと並走する別ジョブ windows-clippy（macOS と同じ 3 つの宇宙・warning-gate.sh・-D warnings なし・キャッシュなし・約 11〜17 分）でゲートした。段にすると windows ジョブが 81.5 分になったので別ジョブへ（今は 49〜66 分 = #2850 の前の幅）。注入 run 42762124414 / 42789746330 で名指しの赤を確認。check-windows.sh も xwin の clippy の 3 宇宙へ）
- PR #2862

## 2026-11-10（#2869 の 1・2: 入れ方のページの更新通知の場所（通知カード + 専用画面）と Gatekeeper の手順（システム設定の「このまま開く」を 1 番目・右クリックは 14 以前だけ・配布物は ad-hoc 署名で公証なし・cask も quarantine 付き）。3 の図は初回起動の流れを戻した後へ回し #2869 は Refs のまま。範囲外: #2876（保留）・#2877）
- PR #2878

## 2026-11-10（#2765: remote daemon の `{keys}` がキー名を文字のまま打っていた（`Escape` → `45 73 63 61 70 65`）→ dispatch の `SendKeys` へ寄せ（`remote::input_request`・空白区切りの列）、キーの断りは 400 で IPC の接続を捨てない・A/B `TAKO_2765_LEGACY=1`・番犬 `issue2765_remote_keys_watchdog`）
- 実経路 `scripts/test-remote-keys-2765.sh`（器のペイン + 実 daemon + PWA の live spec）で修正前 PASS=4 FAIL=19 → 修正後 PASS=29 FAIL=0・カタログ ±0 B。PR #2881

## 2026-11-10（#2841: claude 2.1.322 の auto mode の確認「Auto mode can't check actions in this session」= 番号もカーソルも無いキー案内のダイアログを種別 auto_mode_unchecked で検出し、status / watch / respond（CLI・MCP）で答えられるように。shift+Tab の送り口 BTab（器で CSI u / ESC [ Z）と、描画直後のキーを捨てる claude への 0.8 秒の待ちを足した）
- 本物の claude + 偽 API の隔離 GUI で 18/18（修正前は choice_dialog=null・status busy / idle）・注入 2 通りで名指し・カタログ +0 B。PR #2880

## 2026-11-10（#2879: 初回起動の導線を ボタン → setup → 対話 → master へ。起動時は何も走らせない・対話が済んだ印 `setup.dialogue_done`・走っている印 `setup/running.json`・フロー判定の初回の合図を直した。A/B `TAKO_2879_LEGACY=1`・番犬 `issue2879_no_setup_on_launch_watchdog`）
- 実経路 PASS 46（claude 無し / ログイン済み / 既存の利用者 / A/B）・番犬の注入で file:line 名指し・カタログ −89 B。PR #2882

## 2026-11-10（#2851: `scripts/check-windows.sh` の引数ありの経路が常に `--workspace` を足して `-p` が負けていた → `-p` / `--package`（`=`・くっつけた形も）か自分の `--workspace` があれば足さない・値の無い `-p` は 2 で止める（cargo-xwin は黙って捨てて全体を検査する）。`-p tako-core` の `Checking` が 4 行 → 1 行。テストは test-warning-gate-1537.sh の節 C）
- PR #2884

## 2026-11-10（#2760: ツリーの断りの通知をルートからの相対へ縮め（`tako_core::paths::shorten_paths_in_text`・括弧の中の `A → A` もまとめる）、通知欄を本文 4 行（82.2px）で止めて中でスクロール。A/B `TAKO_2760_LEGACY=1`・visual-test `tree-notice-2760`・番犬。旧 425px で行が窓の外 → 新 82px で最後の行 594/600）
- PR #2885

## 2026-11-10（#2877: docs の `--sl-color-gray-1..7` が Starlight の約束と逆向き → 同じ 7 色のまま反転。図の説明文・フッターの法務リンク 1.45 / 1.66 → 5.55 / 7.01、検索の抜粋・テーマ切替 1.22 / 1.28 → 13.84 / 14.78、検索ダイアログの枠の地も配色どおりに。postbuild に `verify-contrast.mjs`（向き + 説明文・フッターの 4.5）を足し、並びを戻す注入で名指しで落ちる）
- PR #2888

## 2026-11-11（#2869 の 3 / #2876: 入れ方のページ「2. 起動」・図 welcome-banner.webp・動く図 WelcomeBanner と 3 場面を #2879 のボタン → setup → 対話 → master の形へ。図は tako-vd の隔離 GUI で読み戻し撮影・pii-scan 0 件。Windows の場面の $PROFILE の食い違いも解消。範囲外は #2889 に起票）
- PR #2890

## 2026-11-11（#2883: CI 待ちの期待名を実行した木ではなく PR の head の workflow から導き（読めないときだけ手元の木 + 1 行の警告）、base にだけある job がこの PR で走っていなければ既定で緑にせず新しい終了コード 6（`TAKO_WAIT_PR_CHECKS_SET=head` で head の集合・A/B `TAKO_2883_LEGACY=1`・案内は `lib/pr-workflow-gap.sh`））
- 再現は修正前 2（上限まで）→ 修正後 6（2 回の観測）・既存 303 + 新規 94 が bash 3.2 / 5 で緑・注入で 36 件が名指しで落ちる・実 PR #2878 の head から期待名 7 本。PR #2887

## 2026-11-11（#2889 / #1572 / #1587: docs の `tako setup` の説明を「揃っていれば質問なし・未検出の依存は `[y/N]`・最後に対話のアシスタント」へ。手順 10 で対話を開く / 開かない（`--yes`・`--answers`・端末なし・未ログイン）を書き分け、トラブルシューティングを症状の見出しへ。旧 id 2 つは tako-anchor で残した）
- 砂場の `tako setup` 7 通りで根拠を取り、描画 17/17・verify-links・同ページ内 1703 本を確認。起票 #2897（CLI の help）・#2898（verify-links の同ページ内）。PR #2899

## 2026-11-11（#2874: CI の間欠 `verify_pid_identityは記録より前に起動した本物を通す` の原因は macOS の `/bin/sh` = shim の exec し直しの窓（約 0.2 ms は args が読めず ps が `(sh)` を返す）。判定は設計どおりでテストの子を `/bin/sleep` + `arg0` へ・落ちたら材料を添える）
- 注入 A（窓を開けたままの代役）修正前 5/5 FAILED → 修正後 5/5 ok・本体 3000 回で 3 → 0・カタログ +0 B。PR #2894

## 2026-11-11（#2886: 入れ方・使い方動画の v2（250 秒）。初回起動を「ボタン → `tako setup` → 対話（Step 1〜4・偽のエージェント）→ master」で撮り直し、Claude Code が無いときの場面も実画面に。台本 181 句は OCR と一致（読み違い 5）・個人情報 0 件・本物の HOME は不変）
- 制作中に起票: #2891（`resolved_worker_effort` の食い違い）/ #2892（setup の MCP の行の絶対パス）/ #2893（対話の Step 1 の `created_default`）。PR #2895

## 2026-11-11（#2856: `ports.rs` の macOS 実装（libproc）を境界 B5 `platform/procinfo/macos.rs` へ、OS をまたぐ腕を `procinfo.rs` へ移し、`ports.rs` は re-export だけ・cli の check-health の出し分けも `procinfo::tako_app_alive` へ。番犬 `platform_parity.rs` 3 本）
- 集計器で境界の外の production 406 → 360（`ports.rs` 45 → 0）・トークン比較で一致 143 / 相違 5（呼び先の付け替えだけ）・隔離 GUI のポート検知と check-health の gui 節は移設前後で diff 空・カタログ +0 B。PR #2896

## 2026-11-11（#2897: `tako setup --help` / `tako --help` の説明を「質問ゼロ」から今の契約（端末なら足りないものを聞く・ログイン済みなら最後に対話のアシスタント・`--yes` / `--answers` は聞かずに入れて対話も開かない）へ。番犬 `help_text_parity` が `offer_for` / `opens_dialogue` / `assume_yes` と突き合わせる）
- 注入 3 通りで名指しで落ちる・砂場の実バイナリ 13 項目が修正前後で同じ判定・カタログ +0 B。`launch_agent` のずれは #2902 へ起票。PR #2903

## 2026-11-11（#2858: 窓の枠の OS 差（set_window_frame / toggle_window_zoom / restore_window / app_menus / MENU_BAR_HEIGHT / WINDOW_CONTROLS_PX / TRAFFIC_LIGHTS_SPACER と cfg! 2 つ）を新設の境界 B36 `tako-app/src/platform/window_chrome.rs` へトークン一致で移設（1524 トークン一致・集計器の境界の外 406 → 389）。番犬 `platform_parity::窓の枠のos差が境界の外に戻っていない`・tako-vd で move / resize / maximize / restore の矩形が前後一致・カタログ +0 B）
- PR #2900

## 2026-11-11（#2857: `sleep_guard.rs` の macOS の腕（`mod iokit`・pmset・QoS）を境界 B9 へ = IOKit の FFI は `platform/power/iokit.rs`・sudoers + pmset は `platform/lid/pmset.rs`・OS をまたぐ口は `power.rs` / `lid.rs`。`update` / `status` は 1 本の経路、sudoers の手段は能力 `lid_requires_privileged_setup`。番犬 `sleep_guard_rsの本番コードにosのcfgが戻っていない`）
- 集計器で境界の外の production 360 → 331（`sleep_guard.rs` 29 → 0）・`mod iokit` の中身 1761 トークン一致・`tako sleep-guard status` 7 ケースは移設前後で diff 空・見つけた本番の穴（隔離 GUI の終了が本番の disablesleep を戻す）は #2904・カタログ +0 B。PR #2905

## 2026-11-11（#2898: docs の verify-links.mjs が同じページ内の `href="#…"` もそのページの id（見出し・tako-anchor・name）と突き合わせる。空の `#`・`#_top` は対象外・集計と足場に本数・`checkSite` + 置き場の引数・境界テスト 23 件）
- 今の dist で同じページ内 1703 本・切れ 0。写しの注入で旧は緑・新は名指しで落ちる。PR #2901
