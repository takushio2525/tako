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

## 2026-09-30（#1845: Windows の zip・インストーラーへライセンス 3 本を同梱し、両 OS の組み立てを番犬で固定した）
- `tako.iss` の `[Files]` と `build-installer.ps1` の zip へ `THIRD-PARTY-NOTICES.md` / `THIRD-PARTY-LICENSES.md` を足した（`LICENSE.txt` は従来どおり）。`verify-assets.ps1` が zip を展開して 3 本が元ファイルとバイト一致するかを見て、CI だけがインストーラーを無人インストールしてインストール先も見る。`release-windows.yml` はタグ以外の ref から dispatch するとドライラン（Release へ添付しない）
- 実測: 番犬 `license_bundle_watchdog.rs` 5 本緑・注入 12 通りすべて file:line 名指しで FAILED → 戻して緑・検査関数を pwsh 7.6 で 5 通り（正常 / 欠け / 食い違い / 空 / CI の外）

## 2026-09-30（#1709: tako アプリのデータの扱いのページを最新の main で確かめ直し、窓口をメールと Issue の 2 本立てにして公開した）
- PR #1714 を #1844 の上へ rebase（フッターは作者のサイト → tako のデータの扱い → 共通ポリシー → Cookie 設定）。事実の記述を現行コードと 1 件ずつ突き合わせ、食い違い 4 件（更新確認の時機・自動リネームの発火条件・設定の共有の始め方・導入の条件）を直し、抜けていた事実 5 件を足した
- 窓口: 非公開は contact@takushio2525.com、不具合は GitHub Issue（telemetry.md の削除依頼も同じ）。docs 検査 6 本 rc=0・PC 幅 / スマホ幅の横はみ出し 0 px

## 2026-09-30（#1849: SECURITY.md を置いて脆弱性の非公開の報告先を案内した）
- リポジトリ直下に日英併記の `SECURITY.md`（第一の窓口 = GitHub の Private vulnerability reporting、第二 = メール。対象の版 = 最新の安定版とテスト版・対象範囲・書いてほしいこと・受領の目安 7 日・公開の流れ）。README と privacy.md のお問い合わせ節に導線 1 行ずつ
- 窓口の書き分けは PR #1714 のお問い合わせ節と同じ（非公開 = メール / 不具合・要望 = 公開の Issue）。GitHub の Markdown API で描画して見出し 6・リンク切れ 0 を確認

## 2026-09-30（#1848: macOS の配布物からビルド機のホームパスと署名者の個人名を消した）
- build-app.sh の中だけでホームを `~` へ付け替える（rustc は `--remap-path-prefix` を `CARGO_ENCODED_RUSTFLAGS` で・metallib は PATH 先頭の `scripts/lib/xcrun-remap/xcrun`・専用の `target/release-dist`）。セルフテストの `env!("CARGO_MANIFEST_DIR")` 2 か所は実行時に辿る形へ。署名は既定 ad-hoc。検査 `check_bundle_privacy` を build-app.sh の署名後と release.sh の zip 直前の 2 か所から
- 実測: HOME を含む strings 行 v0.8.24 = 1,064 / 395 → 0 / 0。release.sh は v0.8.24 で rc=1（zip 0 本）・修正後で rc=0。Gatekeeper（quarantine 付き zip）と TCC の要件つき全 19 行の判定が v0.8.24 と一致。`test-bundle-privacy-1848.sh` 33 PASS

## 2026-09-30（#1855: docs サイトの GA4 を同意まで読み込まない新しい形（basic 型）へ移した）
- `docs/astro.config.mjs` の head から gtag.js と inline の config（保険の `consent default` を含む）を消し、ハブの consent.js 1 本に `data-ga-id` を付けた。gtag.js は consent.js が地域と同意を見て差し込む（Refs takushio2525/takushio2525.com#36）。`.agent/conventions.md` の同意の節も同じ形へ
- 実測（ヘッドレス Chromium・国判定だけ差し替え・送信は 204 で打ち切り）: EEA 未選択で Google への要求 0・同意で gtag.js 1 / page_view 1・日本は 1 / 1。旧版の consent.js のままだと全場面 0 なので、merge はエッジの入れ替わりの後

## 2026-09-30（#1857: tmux の attach クライアントだけが外から終わってもペインを閉じず再 attach するようにした）
- Exited で器へ生死を聞き（`list-clients`・上限 3 秒・background）、生きていて他のクライアントが居なければ attach 専用（`attach-session`）で同じペインを張り替える。60 秒に 5 回で止まり、閉じずにチップ + 画面 2 行。persist.log に 1 行・状態は list / read の `backend_reattach`・手動は `tako persist reattach` / MCP `tako_persist` の `reattach`（カタログ +204 B）
- 実測: `scripts/test-reattach-1857.sh` 30 PASS 0 FAIL（v0.8.25 は ① で FAILED = ペインが閉じる）・番犬の注入 5 通りすべて main.rs:行 で FAILED・A/B `TAKO_1857_LEGACY=1`

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
