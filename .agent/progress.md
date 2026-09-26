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

## 2026-09-26（#1742: 上下移動の桁を表示幅で覚え、選択中の ←→ で畳み、macOS の ⌃A・⌃E・⌃K を足した）
- 桁の記憶を文字数 → 表示幅（全角 2・タブは 4 桁ごとのタブストップ。`unicode-width` は alacritty 経由で既にツリー内の版を直接依存へ）、選択中の素の ←→ は選択の端へ畳む、⌃A（桁 0）/ ⌃E / ⌃K は `editor_keys` の macOS 列だけ（Windows に置かない理由は表の行）。操作は足さず `tako edit move` / `delete`・MCP の既存の口で同じ結果
- 実測: `scripts/test-editor-keys-1652.sh` **37 PASS 0 FAIL**（tako-vd の実打鍵 15 相 + CLI / MCP の字面照合。bash 3.2 で通す）・注入 3 通りが単体と実 GUI で名指しの FAILED → 戻して緑・workspace 5644 passed 0 failed・clippy 3 宇宙 0。描画は全角 13px / タブ 28px で桁の規則（桁 4 = 31.3px）と 3〜5px ずれる = 描画側の別件

## 2026-09-26（#1760: 隔離 GUI はヘルパ経由で tako-vd 以外の面の明示を通さないようにした）
- `launch_isolated_gui` は面を用意できても `TAKO_DISPLAY=0`（= メイン画面）等の明示をそのまま渡していた（偽 GUI で実測）。起動前に `iso_display_allowed` で判定し、通すのは tako-vd の名前 / 記録済み uuid / 空 / 実在し得ない index（N ≥ 100）だけ・通さなければ終了コード 2 + stderr 1 行。旧 `ISOLATED_GUI_DISPLAY` の差し替え口は閉じ、#1697 の ④ は `index:999` へ
- 実測: #1697 の検査が tako-vd 上で PASS=19・④ だけの A/B（面が tako-vd 1 枚の門つき）で `TAKO_1697_LEGACY=1` だと終了せず落ちる・番犬 3 本追加で注入 7 通りすべて file:line 名指しで FAILED → 戻して緑

## 2026-09-26（#1677: ジャンプ履歴（戻る / 進む）を足した）
- 行を指定した OpenFile が「飛ぶ前にいた場所」と着地点を積み、⌃- / ⌃⇧-（Windows は Ctrl+Alt+← / →。Ctrl+- は縮小のため）と CLI `tako jump back|forward|list` / MCP `tako_jump`（action の 1 ツール）が同じ dispatch で戻る / 進む。スタックは `tako_core::jump_history`（同じ行の連続を畳む・上限 100・閉じたペインは開き直す・消えたファイルは捨てる）。積む契機は行指定の open だけ・永続化しない
- `OpenFile` の本体を `dispatch::open_file` へ移し、戻る / 進むの着地も同じ実装を通す（積まないのは `JumpRecord::Skip`）。#1398 の番犬を移動先を見る形へ直した。macOS の ⌃⇧- は shift が落ちて US = `ctrl-_` / JIS = `ctrl-=` で届くので 2 本張った
- 実測: `scripts/test-jump-1677.sh` 31 PASS（CLI と MCP の応答が字面一致）・`test-jump-keys-1677.sh`（visual-test の打鍵経路）緑 + LEGACY で FAILED・注入 7 通りすべて file:line で FAILED → 戻して緑・workspace 5829 passed 0 failed・clippy 3 宇宙 0

## 2026-09-26（#1768: ペインの PTY の子と remote daemon が GUI の fd を受け継がないようにした）
- 修正前は GUI が CLOEXEC 無しで開いた Metal のシェーダキャッシュが全ペインの子の fd 4 / 5 に 100%（隔離 GUI 3000 / 3000・本番の tmux クライアント 23 本）。`platform::fd_inherit`（B27）の `seal_inherited_fds` を fork 後・exec 前の子で走らせる（PTY は `spawn_sealed` で包んで atfork の子ハンドラ・daemon は既存の `pre_exec`）
- 親で掃く案 (a) は Metal を塞いだが GUI が相方を握るパイプが 1 / 3000 残ったので、子の中で掃く (b) を採った。`scripts/test-pty-fd-leak-1768.sh` の 3000 回・同条件で 修正前 other 3000 / (a) pipe 1 / (b) pipe 0・other 0
- 注入 6 通り（包み・子ハンドラ・(a) 戻し・daemon・確保・逃げ道）が file:line 名指しで FAILED → 戻して緑。実 PTY の統合テスト 9 本・daemon の単体・番犬 4 本

## 2026-09-27（#1757: バイト位置の切り詰めの残りを直し、文字数の切り詰めを 1 実装へ寄せた）
- peer_messaging の `&raw[len..]` は書き直されたファイルで文字の途中を指して panic（修正前に単体テストで実測）→ `raw.get(len..)` で「位置が無効 = 全文」へ。chat_view の `label[..1]` は文字単位の `capitalize_first` へ（family は既知の語に絞られていて今の入口からは届かない潜在バグ）
- 文字数の切り詰めを `tako_core::text::truncate_chars` の 1 本へ（tako-app の `truncate` 68 呼び出し・context_budget の私有版を寄せ、transcript の同名関数は `summary_line` へ改名して数える部分を委譲）。範囲添字の検出を `tests/common/range_index.rs` へまとめ #1728 / #1746 の番犬が呼ぶ。規約は conventions.md「文字列をバイト位置で切らない」
- 実測: 注入 A/B 2 か所が file:line 名指しで FAILED → 戻して緑・workspace 5768 passed 0 failed・clippy 3 宇宙 0。repo 全体の危ない型は 50 件あり番犬化は保留

## 2026-09-27（#1679: LSP の診断を波線・右パネル・tako lsp diagnostics で出した）
- publish を受けた時点で「サーバへ送った本文の写し」で tako の座標へ写し（`LineIndex`）、manager の URI 別の表 1 つを波線・右パネル・CLI / MCP `tako_lsp` が読む。UI へは bounded 128 のキュー（溢れたら全部読み直す印）。pull 型は申告しない（flycheck は push だけ・pull はタイマーが要る = #772）。閉じた / サーバが止まったら捨てる
- 右パネルの diagnostics タブは LSP の文書があるときだけ（常設すると #1479 の段で既定 320px のラベルが全員ぶん落ちる）。5 本 + 3 桁バッジが 220px で 0.5px 溢れるぶんは `IconsTight` を梯子の最後に足した（4 本は不変）
- 実測: visual-test `preview-code` の 2 枚目で基準との差分は帯の外 0 px・4 色の最小距離 61.8・行末の波の振れ幅 3.0px・右パネル 6 行・閉じたら保持 0、`TAKO_1007_LEGACY=1` で FAILED。`scripts/test-lsp-diagnostics-1679.sh` 16 PASS（実 rust-analyzer の E0308 まで）・e2e 6 本（LEGACY で 5 本 FAILED）

## 2026-09-27（#1662: `--wait` に上限を持たせ、auto_close を GUI の終了検知で効かせ、CLI / MCP の run も走らせる前に保存するようにした）
- `--wait` は `probe::poll_with_timeout` の 1 実装（既定 600 秒・env `TAKO_RUN_WAIT_TIMEOUT_SECS`・0 は既定）で、超えたら「まだ実行中」+ exit 1。閉じるのは `dispatch::auto_close_run_pane` の 1 本（GUI の出力のたび / 2 秒ごと / `RunInteractiveStatus`）で、閉じた結末は `Workspace::closed_runs` に控える。保存は dispatch `Run` の `save_previews_before_run` の 1 本へ寄せ、再生ボタンの自前保存を外した
- 実測: `scripts/test-run-wait-save-1662.sh` **32 PASS 0 FAIL**（修正前のバイナリは 14 PASS 18 FAIL = 上限 3 秒でも 15 秒の締め切りまで返らない / `--wait` 無しで閉じない / CLI・MCP とも古い内容が走る）・注入 4 通りすべて file:line 名指しで FAILED → 戻して緑

## 2026-09-27（#1507: setup の末尾にスマホからの接続の状態を 1 行出し、未導入は依存の導入口へ寄せた）
- 末尾は Tailscale の有無に関係なく固定文 `スマホからリモート接続するには: tako remote setup`（棚卸し Z17）。`remote_setup::setup_summary_lines` が `check_status`（読み取りのみ）の JSON から状態を決めて `スマホからの接続: …` を 1 行出す（10 通り。未導入は `setup_deps::next_step_line` = 依存チェック段と同じ文面・途中までは `tako remote setup`・公開済みは URL）。導入を聞くのは依存段の 1 回だけ
- 待ちに上限: 検出の `tailscale --version` を `probe::output_with_timeout` へ寄せ、`status --json` の打ち切りを `RunError::TimedOut` の型で持つ（`DaemonNotRunning` へ畳まず `timeouts` へ）。実測: `scripts/test-setup-remote-status-1507.sh` 41 PASS（修正前の tako で 15 FAIL）・時間切れでも 11 秒で完走・打ち切った子 0・番犬 6 本へ注入 4 通りが file:line で FAILED → 戻して緑

## 2026-09-27（#1775: PWA e2e の証拠を spec ごとのサブ dir へ分け、CI だけ 2 workers にした）
- `TAKO_EVIDENCE_DIR` の平置きで `01-list.png` / `05-observe.png` / `06-forbidden.png` が spec をまたいで上書きし、80 回撮って 76 枚しか残らなかった。`evidencePath()` の 1 実装で `<dir>/<spec 名>/` を挟み 80 枚（outputDir 側は不変）。番犬 `issue1749_pwa_e2e_output_watchdog` に規則 5（サブ dir）・6（spec 内の同名）を足し、注入 3 通りが file:line 名指しで FAILED → 戻して緑
- CI の macOS（3 vCPU）は既定 50% で 1 worker = ステップ 129〜138 秒。同ランナーの実測で 2 workers 73〜87 秒・`--repeat-each=3` 327 項目 × 2 台 flaky 0 → ci.yml だけ `--workers=2`（3 は dev サーバーの取り分が無い）。README / commands.md の所要時間を実測値へ

## 2026-09-27（#1784: 隔離 GUI のヘルパが呼び出し側の偽の TAKO_ISOLATED を通さないようにした）
- `launch_isolated_gui` は既定 `${TAKO_ISOLATED:-1}` を `"$@"` の前に置いていたので、引数 / export の `0`・空・`false` がそのまま GUI へ届いていた（偽 GUI で実測）。GUI へは常に `TAKO_ISOLATED=1` を `"$@"` より後ろで渡し、偽（tako の `is_verification_gui` が偽と読む値）は面を起こす前に終了コード 2 + stderr 1 行で断る（#1760 と同じ「使い方の誤り」）
- 実測: 番犬 3 本追加（構造 / `/bin/bash` 3.2 の実走 / tako の真偽との突き合わせ）で注入 6 通りすべて file:line 名指しで FAILED → 戻して緑・tako-vd の実 GUI でも偽は起動せず 2、`on` は実プロセスの env が `TAKO_ISOLATED=1`

## 2026-09-27（#1653: 検索・置換で大文字小文字を区別し（既定）、単語単位のトグルを足した）
- `find_all` が常に小文字化し `replace_all("value"→"item")` が `Value::new()` を `item::new()` にしていた。`SearchOptions`（既定 = 区別する）を tako-core → dispatch（省略時は `SearchOptions::resolve` の 1 実装）→ CLI `-i` / `-w` → MCP `case_sensitive` / `whole_word` → 検索欄の SVG トグル 2 つへ 1:1。小文字写しは区別しない検索のときだけ作り本文が変わるまで使い回す（1 MB の 1 打鍵 4.449 → 0.509ms / 区別しない 1.251ms）
- 実測: `scripts/test-search-case-1653.sh` **25 PASS 0 FAIL**（tako-vd でトグルを実マウスで押して 3 → 5 → 4 → 2 → 3 件・置換で `Value` が残る + CLI / MCP の字面一致）・番犬 `issue1653_search_case_watchdog` + 単体へ注入 6 通りすべて file:line 名指しで FAILED → 戻して緑

## 2026-09-27（#1807: fd 継承の番犬が無関係な PR の CI で間欠的に落ちる件を、検査の片の偽陽性として直した）
- 真因は実装でもテストの並列性でもなく、検査の片 `fd_inherit::inherited_probe_script` の `{ : >&N; } 2>/dev/null`。bash（macOS の `/bin/sh`）は `>&N` の前に元の fd 2 / 1 を 10 以上の空き番号へ退避するので、閉じた 10 / 11 を「開いている」と読んでいた（CI で落ちた回は fd 10）。外部コマンドの fork 子で試す形（`/usr/bin/true 2>/dev/null >&N`）へ替え、daemon と PTY の両方の番犬が同時に直った
- 実測: 開く番号を 3〜20 で固定すると修正前は 10 / 11 だけ 100% FAILED（PTY 経路も同じ）→ 修正後 3 回ずつ全 ok。全 lib 20 回は修正前後とも失敗 0（手元では番号が 10 / 11 に当たらなかった）。seal を外す注入で両経路とも全番号で file:line 名指しの FAILED、旧い片へ戻す注入で片の単体テストが `[4, 10, 11, 12]` で FAILED → 戻して緑

## 2026-09-27（#1680: 定義ジャンプ（⌘クリックで定義先を新しいペインに）を足した）
- ⌘ホバーの下線（md リンクと同じ 1 実装）・⌘クリック・`tako lsp definition|declaration|type-definition|implementation` / MCP `tako_lsp` が同じ 3 段（UI で準備 → background で問い合わせ → UI で着地）を通る。offload に UI スレッドの続き（`OffloadOutcome::OnUi`）を足し、IPC ループの後処理を `after_dispatch` へ切り出した。着地は `open_file` 経由でジャンプ履歴へ積み、同じファイルは読み直さない。編集モードでなくても問い合わせのあいだだけ didOpen する
- 実サーバ: rust-analyzer は読み込み前に空で答えるので `experimental/serverStatus` を待って問い直す（tako の実ソースで初回 14 秒で `file_uri.rs:29` へ新ペイン・2 回目は使い回し）。clangd は `#include` → `foo.h` を新ペインで。実測: `scripts/test-lsp-goto-1680.sh` 38 PASS 0 FAIL（新ペイン / 使い回し / 同じペイン / #include / 戻る / 複数候補 / 3 状態 / CLI と MCP の字面一致 / UTF-16 / 未応答中も UI が止まらない）・e2e 11 本・番犬の注入 7 通り + 実ソースへの注入 A（新ペインを開かない）/ B（使い回さない）が dispatch.rs:762 / 761 を名指しで FAILED → 戻して緑
- #1791（S2 診断）の上へ合流: MCP `tako_lsp` は 1 本のまま action 5 つ（既定 diagnostics）・MATRIX の `tako_lsp` は 1 行（Windows の根拠に両方の e2e）・要件は FR-3.31 へ振り直し

## 2026-09-27（#1758: CLI の出力をパイプで途中で閉じても panic しないようにし、gate の証拠の字下げを揃えた）
- std の `println!` が EPIPE で panic していた（実測: `agent-support --json | head -1` で panic 2 つ + 終了コード 101。Windows は `ERROR_NO_DATA`）。tako-cli の出力マクロ 4 種を `stdio.rs` の同名マクロへ差し替え、標準出力の切断は `resume_unwind` で静かに 0、標準エラーの切断は捨てて続行（失敗の 1 を保つ）。SIGPIPE を既定へ戻す案は Windows に効かず `remote serve` の daemon を殺すので不採用。`gate set / check / show` の証拠は全行へ同じ字下げ
- 実測: 統合テスト 4 本（読み手を先に閉じたパイプ / 111 KB の 1 行読み / `2>&1` 形で終了コード保持 / 宣言順の番犬）+ unit 2 本。注入 7 通りすべて FAILED → 戻して緑。`mcp serve` の終わり方 4 通りは修正前と字面一致・workspace 5971 passed 0 failed・clippy 3 宇宙 0

## 2026-09-27（#1811: テスト判定から外れたら安全側へ倒し、deps/ の外のテストバイナリが本番へ書かないようにした）
- `is_test_process()` が置き場（`deps/`）だけで決まり、`/tmp` へコピーしたテストバイナリが本番の `recent.json` 上書き・`cli-dir` を空に・実 agent CLI を実 HOME で起動した。製品の `main`（`tako-app` / `tako`）の 1 文目で `paths::mark_product_process()` を呼び、「`deps/` にある **または** 宣言が無い」をテストとする（規則は `judge_test_process` の 1 実装・倒した回は stderr へ 1 回知らせる）
- 実測: 一時 HOME を本番に見立てた再現で修正前 17 ファイル → 修正後 0（同じ名前 / 改名とも）・製品 `tako` は一時 HOME の本番相当を解決し知らせ無し・番犬 3 段（入口の静的検査 / deps 外の子 / 製品 CLI の実行時）へ注入 5 通りすべて FAILED → 戻して緑

## 2026-09-27（#1797: setup の依存段が tailscale を remote と同じ検出で探すようにした）
- 依存段（`setup_deps::resolve`）は tailscale を PATH だけで探し、PATH 外の App Store 版を「見つかりません」→ `--yes` で brew 版まで入れていた（#1038 の 2 系統同居）。正本 `tailscale::detect_tailscale`（Runnable / Unrunnable / Absent）を足して依存段・`find_tailscale`・remote setup [1/5] が読む。在るが動かない CLI は導入済みと読み入れ直させない。`run_tailscale_within` の自前の待ち（子の終了後に join）は `probe::output_with_timeout` へ寄せた
- 実測: `scripts/test-setup-tailscale-detect-1797.sh` 55 PASS（修正前の tako で 25 FAIL = PATH 外で brew 1 回 → 0 回 / 孫がパイプを握る tailscale で 60 秒の締め切り → 5 秒）・番犬 6 本へ注入 5 通りが file:line で FAILED → 戻して緑・setup 系の隔離 10 本全緑（1499 / 1505 は tailscale を `TAKO_TAILSCALE_BIN` で閉じ込めるよう直した）・workspace 5919 passed 0 failed・clippy 3 宇宙 0・check-windows 0

## 2026-09-27（#1783: TAKO_VD_NAME に物理画面の名前を渡しても tako-vd と見なさないようにし、面の指定の判定を面を起こす前へ移した）
- `ensure` の締めの先頭に「器が作った仮想ディスプレイか」（内蔵 = CGDisplayIsBuiltin / 器の一覧に名前が無い）を置き、物理画面なら終了コード 3 と理由 1 行で断る（修正前はスタブ実測で rc=0・その面の uuid を記録・内蔵なら器へ main を撃っていた）。ヘルパは 3 を使い方の誤り（2）として返し、`TAKO_DISPLAY` の判定を面を起こす前へ移した（uuid の記録との突き合わせだけは起こした後。FR-4.8.20〜22）
- 実測: 注入 9 通りすべて file:line 名指しで FAILED → 戻して緑・モック 154 PASS（/bin/bash 3.2）・本物の tako-vd で ensure rc=0（構成の前後差分なし）・書き方の誤った `TAKO_DISPLAY` は ensure を呼ばず 0.01 秒で rc=2

## 2026-09-27（#1763: 実行中のテーマの読み直しでも読めない色を persist.log へ残すようにした）
- 起動時だけが警告を残し、`reload_theme`（`tako theme` / MCP / 設定画面）とタブバーのトグルは `resolve_theme()` の警告を捨てていた。3 経路とも `load_theme_logged` → `settings::ThemeWarningLog` の 1 本へ寄せ、前回と同じ警告は出さず増えたぶんを起動時と同じ「無視」、消えたぶんを「解消」で出す
- 実測: `scripts/test-theme-reload-1763.sh` 修正前 16 PASS 10 FAIL（読み直し 0 行）→ 修正後 30 PASS 0 FAIL（CLI / MCP / toggle / 壊れた JSON）・注入 7 通りすべて file:line 名指しで FAILED → 戻して緑

## 2026-09-27（#1660: 10 万行 / 10 MB まで編集できるようにした）
- 上限の正本 `preview_limit`・超えたら理由と値を画面 / CLI / MCP へ。全文の塗りは background・行頭索引・描画の差し替えだけ更新
- 実測: 8.6 万行で編集開始 19 ms・1 打鍵 中央値 3.2〜3.7 ms。`test-large-file-edit-1660.sh` 緑
- #1800 / #1802 と合流: 閲覧中の ⌘F で CRLF の強調が消える退行を節 `large-file-decor` で再現 → 修正・番犬に規則 6

## 2026-09-27（#1819: 実行中に 2 回目以降に壊れた設定の中身も退避へ積んで残すようにした）
- `quarantine_unreadable` は `.unreadable.bak` が在ると何も写さず、settings.json の申告も 1 プロセス 1 回 = 実行中に 2 回目に壊れた中身は保存（load → 既定値 → save）で跡形もなく消えていた（隔離 GUI で実測。recent / shortcuts / layout（`.corrupt` の rename）も同型）。2 本目以降を `.unreadable.<n>.bak` へ積み（同じ中身は積まない・上限 10 本で 1 本目を残して 2 本目から押し出し = persist.log に記録）、申告は壊れた中身ごとに 1 回へ
- 実測: `scripts/test-unreadable-quarantine-1819.sh` before 21 PASS / 17 FAIL → after 38 PASS / 0 FAIL・注入 5 通りすべて file:line 名指しで FAILED → 戻して緑・workspace 5995 passed 0 failed・clippy 3 宇宙 0
## 2026-09-27（#1730: 設定なしでプロジェクトの .venv / uv / poetry 等で走るようにした）
- `.py` が常に PATH の `python3` で走っていたので、S1 の検出（`runtime_env`）を `Run` / `RunResolve` へ配線した。組み込み既定の `py` 行は `${python} ${fileBase}`（実行環境が無ければ今とバイト一致）、activation は境界 B1 の `compose_run_script` でコマンドの先頭へ埋める。Tier P（道具の場所・版・環境の置き場・conda の一覧）は `probe::output_with_timeout` だけを通し、答えを「事実」として覚えて `Run` は子プロセス無しで重ねる（`tako-control::runtime_probe`）。`RunResolve` は `prepare_offload` へ載せ、`runtime` / `runtimes` / `config` / `project_root` / `probe` と `refresh` を足した
- 実測: `scripts/test-runner-runtime-1730.sh` **24 PASS 0 FAIL**（venv / なし / uv run / poetry 不在 / 壊れた venv / 単独 .py / 空白と日本語 / 固まる Tier P を 3 秒で打ち切り / CLI と MCP の字面一致 / `TAKO_1730_LEGACY=1` で ① が FAILED）・Tier F 0.08〜0.31 ms（子 300 × 3 段で 1.6〜2.6 ms）・workspace 5927 passed 0 failed・clippy 3 宇宙 0・MCP カタログ +434 B（191,776 → 192,210）
