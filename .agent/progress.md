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

## 2026-09-26（#1749: PWA の e2e がホームへ書かないようにし、モックの版をビルドの版から取るようにした）
- 一時 HOME の実測で `npm run e2e` だけでホームへ PNG 80 枚（`~/Desktop/tako-28{4,5}-evidence/` 16 枚・`~/dev/tako-evidence/<番号>/` 64 枚）。スクショは `e2e/support.js` の `evidencePath()`（既定 = outputDir）へ、モックの版は vite と同じ `workspace-version.js` の `TAKO_VERSION` へ寄せた（旧 0.8.12 等で「表示が古い」バナーが写っていた）
- 実測: 109 passed・一時 HOME 配下 0 件（初回 / 2 回連続 / スクショ spec 単独）・番犬 `issue1749_pwa_e2e_output_watchdog` の注入 4 通りが file:line 名指しで FAILED → 戻して緑

## 2026-09-26（#1506: setup --review のプランの問いの既定を前回値にし、Enter で倍率が退行しないようにした）
- `--review` は前回値を引き継がない経路なので `prompt_plan` へ前回値が届かず、既定が「不明」固定 = Enter・非 TTY の EOF だけで `max-5x` → `max`（GPT / Google も `unknown` へ）。問いを `plan_question`（表示と「番号 → 値」の 1 か所）へ寄せ、既定は**標準 setup が前回値から選ぶ値と同じ規則**で引く（Max 検出時は max 系だけ・選択肢に無い値は値そのものを既定に見せる）。範囲外の番号は答えなかった扱い（旧は選択肢数を問わず 1〜7 を受け、Max の問いで `4` を打つと `max`）
- 実測: `scripts/test-setup-plan-keep-previous-1506.sh` **45 PASS 0 FAIL**（修正前ビルドでは `--review` 系の 11 項目だけ FAIL・`--yes` / MCP と同じ起動 / 初回は修正前から緑）・単体 5 本（15 通りで「Enter の結果 = 標準 setup」を照合）・注入 3 通りすべて file:line で FAILED → 戻して緑・pty-answer を共有する 1499 / 1501 / 1504 / 1509 全緑・workspace 5541 passed 0 failed・clippy 3 宇宙 0

## 2026-09-26（#1657: Code Runner の実行ペインを使い回し、内部マーカーを画面から消して終わりを案内とバッジで伝えた）
- 再生ボタン / `tako run` は同じタブの「同じファイル + 同じプロファイル」の実行ペインを**その位置で差し替える**（`PaneTree::replace` + 旧ペインは close と同じ後始末。実行中なら止めて再実行 = `stopped_running`。`--new-pane` / MCP `new_pane` で増やす）。終了コードは側路ファイル `run-exit/<pane>.code` で運び（画面の `__TAKO_EXIT=` は書けなかったときの退避路だけ）、画面には「[tako] 終了コード N / Enter で…」、タイトルバーに実行中 / 完了 / 失敗 (N) のバッジ。読む側は `run_pane_exit_code` の 1 実装（`--wait`・カードの実行記録 #1724・`list` の `run`・バッジ）
- 実測: `scripts/test-run-pane-reuse-1657.sh` **44 PASS 0 FAIL**（5 回で 1 枚・器つきの capture-pane にマーカー無し・A/B `TAKO_1657_LEGACY=1` で 5 枚 + マーカー・visual-test のバッジ色）・`test-remote-command-card-1724.sh` 55 PASS・注入 4 通りが file:line 名指しで FAILED → 戻して緑・workspace 5562 passed 0 failed・clippy 3 宇宙 0

## 2026-09-26（#1750: ⌘K パレット・Web のアドレスバー・dock の URL 欄の IME をターミナルから入力欄へ戻した）
- 修正前の実測（一時プローブ・隔離 GUI・実マウスの入口）: 3 つとも ASCII と ⌘V は欄に入る一方、変換は**ターミナルペインに束縛**（下線・候補窓はターミナルのカーソル位置）、確定はパレット / アドレスバーで PTY へ、unmark は 3 つとも PTY へ。Web ペインにフォーカスがあるとアドレスバーの確定は消え、git のコミット欄が残ったままパレットを開くと変換は裏のコミット欄へ入った
- `AppTextInput` に `Palette` / `WebAddress` / `WebDockUrl` を足し（パレット最優先 = `handle_key` と同じ順）、4 経路を 1 挿入関数・開く / 閉じるを 1 本ずつ（閉じる出口が変換を捨てる）・Web の 2 欄は `TextField` へ・見えない欄は奪わない・長い URL は `inline_input_window` で詰める
- 実測: セルフテスト項目 155 緑 / `TAKO_1750_LEGACY=1` で FAILED・番犬 `issue1750_palette_web_ime_watchdog`（注入 15 通り）+ 実ソース注入 3 通りが `main.rs:<line>` で名指し → 戻して緑・項目 154 緑・workspace 5798 passed 0 failed・clippy 3 宇宙 0。実 IME は `manual-checks.md`

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
