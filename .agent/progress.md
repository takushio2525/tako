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

## 2026-10-02（#1853 / #1592: 安定版への昇格で Homebrew cask と docs の「最新の安定版」も動かすようにした）
- `release.sh --promote` が夜間の素のタグ（prerelease を外して Latest）も受け付け、続けて tap の cask を公開アセットの sha256 で更新（PR → merge → tap の main を読み直し）、releases.md を `check-releases-page.mjs --set-stable` で書き換えた PR を `merge-pr.sh` で merge。後続の失敗は exit 4・打ち直しは同じコマンド。bash 3.2 の set -u + EXIT trap が exit 0 に化ける穴も塞いだ
- 実測: `scripts/test-release-promote-1853.sh` 125 PASS（注入 9 通りすべて FAILED）・既存の retry 55 / nightly 129 緑・docs の build + 検査 6 本 rc=0。releases.md の安定版は v0.8.26 へ

## 2026-10-02（#1683: LSP の整形（全体 / 範囲）と保存時整形（既定 off）を足した）
- 当て方は `TextBuffer::apply_changes` の 1 実装（安定ソート → 重なり拒否 → 最小化 → `apply_edit` 1 回 = undo 1 回）、範囲の外は変えない。CLI `tako lsp format [--range]` / `format-on-save` + MCP `tako_lsp` の action（ツール増やさず +841 B）+ 編集メニュー・⇧⌘I（Win は Ctrl+Shift+I）。保存時整形は明示的な保存だけで自動保存では整形しない
- 実測: `scripts/test-lsp-format-1683.sh` 51 PASS 0 FAIL（v0.8.26 は 30 FAIL）・実 rust-analyzer の整形が rustfmt とバイト一致し undo 1 回で戻る・番犬の注入 7 通りを file:line で名指す

## 2026-10-02（#1860: ファイルツリーでファイル・フォルダのコピー / 切り取り / 貼り付けをできるようにした）
- ⌘C / ⌘X / ⌘V（Windows は Ctrl）と右クリックの 3 項目。コピーは `tako_core::file_copy`（Finder 式の別名・排他作成で上書き 0・失敗は作った分だけ戻す・リンクはリンク）、切り取りの貼り付けは #1834 の `run_file_move`、貼り付け先と中身の選び方は `file_clipboard`。OS のクリップボードは境界 B28（NSPasteboard / CF_HDROP）。dispatch `FileOp` の copy / clipboard_* / paste → `tako file copy|clipboard|paste` / MCP `tako_file_op`（+709 B）。2 秒ポーリングが古い一覧で貼ったものを消す競合も直した
- 実測: `scripts/test-tree-clipboard-1860.sh` 48 PASS 0 FAIL（実マウス・実キー 7 場面・A/B `TAKO_1860_LEGACY=1` で FAILED・CLI/MCP 字面一致 13 組・一般のペーストボード往復は保存して戻す）・番犬 18 本（注入 9 通り file:line 名指し）

## 2026-10-02（#1682: LSP 補完（予測変換）= 打鍵中の一覧・仮想化・キーの優先順位・CLI / MCP）
- core `lsp::completion`（読み取り・rank・truncate・route_key 20 通り・Session）+ `replace_position_ranges`（並べ方・重なり・最小化は #1683 の `order_changes`、当て方は離れた範囲をつないだ差分 = undo 1 回で範囲の外を抱えない）→ manager の取り消しの列（`$/cancelRequest`）・resolve・一時 didOpen は持ち手で加わる → dispatch 3 段 + `lsp_completion_apply` → CLI `tako lsp completion` / MCP `tako_lsp` の `action=completion` → GUI `lsp_completion_ui`（`gpui::list`）
- 実測: `scripts/test-lsp-completion-1682.sh` 22 PASS（1000 件で組む行 10・基準画像との差分は一覧の中だけ・A/B 2 通りで FAILED・実の rust-analyzer で `s.le` → len）・e2e 8 本・注入 6 通りで名指しの FAILED

## 2026-10-02（#1866: 全体テストで tmux 系が毎回別の 1 本落ちる = 実 tmux e2e の器の数え方が隣の起動中の器を畳んでいた）
- 真因（1259）: `tests/common/tmux_e2e.rs` が「new-session が返ってから数に入る」「減らしてから別に 0 か見て畳む」で、3 本目の起動中に先の 2 本が返ると器を kill + ソケット削除。叩く前に予約・減算〜kill を 1 ロックへ。A/B `TAKO_1866_LEGACY=1`・差し込み口の固定テスト・番犬 3 規則（1 実装外で畳む / 数に入る前に叩く / tako-core の器の名前の重複）
- 実測: new-session の返りを遅らせる注入で main 1/1・旧アーム 3/3 が 202 行で FAILED、修正後 3/3 緑・全体テスト 3 回連続緑（6263 passed）。1857 は自分の器のソケットだけ消す注入で同じ行・同じ文言を再現（消し手は未特定。kill の結果と器の状態を失敗文言へ）

## 2026-10-02（#1684: コードの本文の右クリックメニューに言語サーバの項目を足した）
- 識別子の上でだけ 定義 / 宣言 / 型定義 / 実装へ移動・コードを整形・選択範囲を整形 を先頭へ（出し分けは `tako_core::lsp::menu::items` = 申告に無い項目は出さない）。押すと `lsp::menu::item_request` の要求を ⌘クリック・編集メニューと同じ 3 段へ。握手前はすぐ開いて 1 行「問い合わせています」→ 背景で起こして差し替え。CLI `tako lsp menu` / MCP `tako_lsp` の `action=menu`（+261 B）。メニューの行の高さを見積もりと同じ定数にして下端の見切れも直した
- 実測: `scripts/test-lsp-menu-1684.sh` 27 PASS 0 FAIL（実マウス・A/B `TAKO_1684_LEGACY=1` で FAILED・申告 4 通り・CLI/MCP 字面一致・実の rust-analyzer で 2.3 秒で着地）・注入 7 通りすべて file:line で FAILED

## 2026-10-02（#1661: Markdown を編集して抜けたら描画へ戻し、目次を作り直すようにした）
- 表示をエディタの行へ落とす判定を `refresh_preview_from_editor` の 1 か所（`EditState::shows_editor_lines`）へ寄せ、抜けたら**本文から**描き直す（5,000 行以下はその場・超えたら background）。抜けた後の save / reload・競合中も描画のまま・見ていた節の見出しから描く。編集中も目次が使える（`source_line`。CLI / MCP は既存の preview-outline）。layout へは抜けた先のモード
- 実測: `scripts/test-md-edit-resume-1661.sh` 49 PASS 0 FAIL / main（048a2d9）は 22 PASS 23 FAIL（① で code のまま・目次 ERR）・A/B `TAKO_1661_LEGACY=1` で ① と visual 節が名指しで FAILED・番犬への注入 8 通りすべて file:line で FAILED・workspace 6362 passed・カタログ +132 B（並置 #1872 / ⌘F の同型 #1873）

## 2026-10-02（#1864: set -e と EXIT trap を併用するスクリプトが bash 3.2 で途中の死を exit 0 に化けさせるのを塞いだ）
- 条件は「set -e + EXIT trap + 展開エラー（set -u の未定義変数・`${x:?}`・不正な置換・readonly）」で、`/bin/sh` も同じ。番人の 1 実装 `scripts/lib/exit-guard.sh`（`tako_exit_trap` / `tako_exit 0`。印の無い 0 は 1）へ 11 本を寄せた（nightly-release.sh・release.sh --promote・promo 2・テスト 6・verify-setup-multiagent）。番犬 3 規則を `shell_scripts.rs` へ
- 実測: 注入 A/B は修正前 rc=0 → 修正後 rc=1（nightly は Test 18 で番人を素の trap に戻すと 0・本物で 1 + ログと通知）。nightly 139 / promote 125 / retry 55 緑

## 2026-10-08（#1877 S0: tako mod の設計と試作 — 実物の Claude Code 2.1.294 で mod を動かし通信路と導入方式を決めた）
- 設計書 `.agent/plans/2026-10-tako-mod.md`。隔離 GUI のペインで試作 mod を動かし `$.session.usage()`（初回応答まで tokens / rateLimits は欠ける・window と cost は claude が答える）・`turn.*`・権限 / 質問待ち（`classic.PermissionRequest`）・`session.append`・帯 / ペイン / ボタンからの `tako split` を実測
- 決定: mod → tako は `$.process.run` で tako CLI（8.5 ms。tako 再起動をまたぐ tmux worker でも CLI フォールバックで繋がるのはこれだけ。MCP は `--strict-mcp-config` で policy 拒否・HTTP 直は 1.5 ms だが再起動で URL / トークンが古びる）。導入は `<data_dir>` へ展開 + ペインの env `CLAUDE_CODE_PLUGIN_DIRS`（設定ファイルを書かない・設定 dir の数に依らない）。版の下限 2.1.294。スライス S1〜S6 + 調査を子 Issue へ

## 2026-10-08（#1681: LSP ホバー = 識別子にマウスを乗せると型・doc のカード・CLI / MCP）
- core `lsp::hover`（Hover の 3 形・16,000 字の上限・範囲・能力）→ manager の `hover`（マウスは補完と同じ取り消しの列 + `open: false` = 開いている文書に加わるだけ = 乗せただけでサーバを起こさない）→ dispatch 3 段（`show` でカード = `ControlHost::show_lsp_hover`）→ CLI `tako lsp hover` / MCP `tako_lsp` の `action=hover`（+290 B）→ GUI `lsp_hover_ui`（`render_block` 経由・1 フレーム目に測って語の行の上下へ・編集メニュー / パレットの口・右クリックメニューには載せない）
- 実測: e2e `issue1681_lsp_hover` 9 本・番犬の注入 8 通りを file:line で名指し・`scripts/test-lsp-hover-1681.sh` 23 PASS（visual-test `hover` 8 相 = 基準画像との差分は矩形の外 0 px・100 回で保持件数が増えない・A/B `TAKO_1681_LEGACY=1` で FAILED / `hover-real` で実の rust-analyzer の `String` の doc がカードに出る / CLI・MCP 19 項目）

## 2026-10-08（#1873: 閲覧中の ⌘F 検索を閉じたら描画へ戻し目次を作り直すようにした）
- 開閉を `open_preview_search_bar` / `close_preview_search_bar` の 1 実装へ寄せ（Escape・⌘F のトグル・CLI / MCP）、閲覧中に描画から開いた検索なら #1661 の `restore_rendered_preview` で戻してセッションも畳む（code へ落ちていたときだけ描き直す）。CLI `tako edit search --open|--close` / MCP `tako_preview_search` の `visible`（ツールは増やさない）。A/B `TAKO_1873_LEGACY=1`
- 実測: 修正前の main は Escape 後 mode=code・目次 0 件（visual 節）。`scripts/test-md-find-restore-1873.sh` 43 PASS 0 FAIL・番犬の注入 8 通りを file:line で名指し

## 2026-10-08（#1879: tako mod S1 = Claude Code の mod の同梱・展開・ペインへの注入と状態報告）
- mod（`crates/tako-core/claude-mod/`）を `<data_dir>/claude-mod/tako/` へ展開し、claude 2.1.294 以上のペインの env（`CLAUDE_CODE_PLUGIN_DIRS` / `TAKO_CLI`）で読ませる（設定ファイルは書かない・tmux は `-e` 固定）。mod は 1 秒 flush / 15 秒 heartbeat で `tako mod report` を叩き、GUI のメモリに 45 秒の鮮度で持つ。`tako mod [on|off]` / MCP `tako_mod`（report は載せない = FR-2.42.6）。組織アカウントで classic 系が mod へ届かないのを実測し、tool.call / tool.check で拾う形を足した（FR-2.42.7）
- 実測: `scripts/test-claude-mod-1879.sh`（隔離 GUI・実 claude）全段 41 PASS 0 FAIL・`claude plugin test` 11 本（注入 3 通りで fail）・番犬 3 本（注入 9 通り名指し）・カタログ +421 B
