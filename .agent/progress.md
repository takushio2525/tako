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

## 2026-09-27（#1662: `--wait` に上限を持たせ、auto_close を GUI の終了検知で効かせ、CLI / MCP の run も走らせる前に保存するようにした）
- `--wait` は `probe::poll_with_timeout` の 1 実装（既定 600 秒・env `TAKO_RUN_WAIT_TIMEOUT_SECS`・0 は既定）で、超えたら「まだ実行中」+ exit 1。閉じるのは `dispatch::auto_close_run_pane` の 1 本（GUI の出力のたび / 2 秒ごと / `RunInteractiveStatus`）で、閉じた結末は `Workspace::closed_runs` に控える。保存は dispatch `Run` の `save_previews_before_run` の 1 本へ寄せ、再生ボタンの自前保存を外した
- 実測: `scripts/test-run-wait-save-1662.sh` **32 PASS 0 FAIL**（修正前のバイナリは 14 PASS 18 FAIL = 上限 3 秒でも 15 秒の締め切りまで返らない / `--wait` 無しで閉じない / CLI・MCP とも古い内容が走る）・注入 4 通りすべて file:line 名指しで FAILED → 戻して緑

## 2026-09-27（#1507: setup の末尾にスマホからの接続の状態を 1 行出し、未導入は依存の導入口へ寄せた）
- 末尾は Tailscale の有無に関係なく固定文 `スマホからリモート接続するには: tako remote setup`（棚卸し Z17）。`remote_setup::setup_summary_lines` が `check_status`（読み取りのみ）の JSON から状態を決めて `スマホからの接続: …` を 1 行出す（10 通り。未導入は `setup_deps::next_step_line` = 依存チェック段と同じ文面・途中までは `tako remote setup`・公開済みは URL）。導入を聞くのは依存段の 1 回だけ
- 待ちに上限: 検出の `tailscale --version` を `probe::output_with_timeout` へ寄せ、`status --json` の打ち切りを `RunError::TimedOut` の型で持つ（`DaemonNotRunning` へ畳まず `timeouts` へ）。実測: `scripts/test-setup-remote-status-1507.sh` 41 PASS（修正前の tako で 15 FAIL）・時間切れでも 11 秒で完走・打ち切った子 0・番犬 6 本へ注入 4 通りが file:line で FAILED → 戻して緑

## 2026-09-27（#1775: PWA e2e の証拠を spec ごとのサブ dir へ分け、CI だけ 2 workers にした）
- `TAKO_EVIDENCE_DIR` の平置きで `01-list.png` / `05-observe.png` / `06-forbidden.png` が spec をまたいで上書きし、80 回撮って 76 枚しか残らなかった。`evidencePath()` の 1 実装で `<dir>/<spec 名>/` を挟み 80 枚（outputDir 側は不変）。番犬 `issue1749_pwa_e2e_output_watchdog` に規則 5（サブ dir）・6（spec 内の同名）を足し、注入 3 通りが file:line 名指しで FAILED → 戻して緑
- CI の macOS（3 vCPU）は既定 50% で 1 worker = ステップ 129〜138 秒。同ランナーの実測で 2 workers 73〜87 秒・`--repeat-each=3` 327 項目 × 2 台 flaky 0 → ci.yml だけ `--workers=2`（3 は dev サーバーの取り分が無い）。README / commands.md の所要時間を実測値へ
