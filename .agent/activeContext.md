# Active Context

> このファイルは AI が**毎ターン上書き更新**する現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。ここには履歴を残さない。
> セッション開始時に AGENTS.md の直後に必ず読む。

## 現在の対象（2026-06-12 夜・実機バグ緊急修正の直後）

- **スクロール全滅リグレッション根治済み**: 根本原因は「Dock 起動の .app はロケール
  環境変数ゼロ → tmux 3.6 が C ロケールクライアントのコマンド出力中の制御文字を `_` に
  サニタイズ → タブ区切りパースが全滅」。`tmux::tmux_command()`（LC_CTYPE=UTF-8 注入）を
  全 tmux 子プロセスの唯一の入口にして解消。**tmuxview 空表示バグも同根で同時解消**
  （詳細は `architecture.md`「実機リグレッション」節）
- バグ3（ジオメトリ復元が効かない）: 調査の結果**現ビルドに欠陥なし**。19:06 の再起動で
  小窓になったのは、終了した旧バイナリがジオメトリ保存コード（c5eff90, 15:00）より
  古かったため保存値が無かった一回限りの事象。フルスクリーン往復は隔離 HOME の
  閉ループ検証で動作確認済み（**次の再起動から正しく復元される**）
- バグ2（権限ダイアログ連発）: ad-hoc 署名の CDHash がビルドごとに変わり TCC が毎回
  リセットされるのが原因。build-app.sh をキーチェーンの **Apple Development 証明書
  自動検出**で署名するよう変更（ユーザーは Xcode 由来の有効な証明書を保有。
  次回ビルド以降の承認はビルドをまたいで保持される。今回の入れ替え直後だけ再承認 1 回）
- ステータス: セルフテスト 105 項目緑・.app 反映済み・**ユーザーの再起動待ち**
- 最終更新: 2026-06-12

## 保留中の実装タスク（このバグ修正の前からの続き）

要件一括登録済み: ① 配布・自動アップデート（Phase 7）② FR-2.14.6 セットアップ画面
③ FR-2.18 子の自動サーフェス ④ FR-2.19 ポートパネル ⑤ **FR-2.16.4〜2.16.7
パネル UI 刷新（次の実装タスク）**

## 次の一手: パネル UI 刷新の実装（FR-2.16.4〜2.16.7）

仕様の正は `requirements.md` FR-2.16（📝 仕様化 2026-06-12 の 4 行 + 実装メモ）。要点:

1. **下部ステータスバー新設**（Zed / VSCode 風）: 左 = ファイルツリートグル、
   右 = tmux 管理・git 管理トグル。上部の「◧ panel」ボタンは廃止して集約。
   トグル状態の取得・操作は CLI / MCP からも（開発不変条件。
   **ファイルツリーは現状 cmd+B のみで CLI / MCP 経路が無い → 新設**）
2. **パネル内部タブの 1 本化**: 現 agents ビュー（中身あり）を「tmux」へリネームし、
   旧 tmuxview（空表示バグ）を削除統合。タブごとに「タブ名ラベル付き四角枠」+
   枠内に全ペインの入れ子表示。各ペイン行右にゴミ箱 →「kill していいですか?」確認 →
   kill（dispatch 経由）。行は折り返し / 省略（…）で見切れさせない。
   **セッション列挙が正しく動くことを保証**（旧 tmuxview の空表示バグ解消）
3. git トグルの表示先は将来の git graph（FR-3.6）。実装まではプレースホルダ等を実装時に決める
4. 区切りごとにコミット・push。設計の大きな分岐は master に報告して止まる。
   完了後はビルド → .app 反映（`scripts/build-app.sh --install`）→ CI 緑まで確認

実装の入口: `crates/tako-app/src/main.rs` の `render_panel()` / `render_tmuxview()`
（1517 行付近）/ `render_agents_view()`（1857 行付近）/ 「◧ panel」ボタン（2530 行付近）/
`panel_state()`・`set_panel()`（3079 行付近）、`tako-control` の `protocol.rs::PanelViewWire` +
`dispatch.rs::Panel` + `mcp.rs::tako_panel`、ファイルツリーは `filetree.rs` + cmd+B（3628 行付近）。

## 既知バグ（次の worker が修正）

- [ ] **Escape で「27u」が入力欄に挿入されることがある**（2026-06-12 報告）。
  extended-keys（CSI u）対応の副作用: ESC のキーコード 27 の CSI-u エンコード断片
  （`CSI 27;1u` 等）がチェーンのどこかで解釈されず、エスケープ部分だけ食われて
  残りが文字として漏れている。tako の kitty / CSI-u 対応（`handle_key` の CSI u 送出・
  disambiguate 常時 ON）とネスト tmux の extended-keys 設定
  （`NESTED_TMUX_SNIPPET` の always / extkeys）の整合を調査して直すこと。
  関連: `architecture.md`「スクロール制御」節 + FR-2.17 の実装メモ（CSI u の罠）、
  core e2e の CSI u 往復テスト（tmux_backend / scroll）

## 未着手タスク（優先順はユーザーと相談）

- [ ] **パネル UI 刷新（FR-2.16.4〜2.16.7）** ← 次の実装タスク（上記）
- [ ] **Phase 5 再開**: FR-3.2 コードプレビュー + `tako_open_file`（下記「再開手順」）→
      FR-3.3 Markdown（pulldown-cmark）→ FR-3.6 git graph（git CLI 子プロセス。
      右サイドバー情報パネルの内部タブとして追加 = FR-2.16.2）
- [ ] **FR-2.19 localhost ポートパネル**（パネル UI 刷新後が自然。要件登録済み）
- [ ] **FR-2.18 未表示の子の自動サーフェス**（フェーズ未定。要件登録済み）
- [ ] **FR-2.14 MCP ゼロコンフィグオンボーディング**（配布前必須。FR-2.14.6
      セットアップ画面を含む）
- [ ] **FR-2.17 ネスト tmux の検出・診断・ワンタップ適用**（Phase 7）
- [ ] **FR-2.15 ターミナルのたまり場**（UI の見せ方をユーザーと相談してから着手）
- [ ] 常用確認の残り: manual-checks.md「スクロール・キー実機バグ一括」節ほか
- [ ] 描画のグリッド不一致（全角 advance ≠ セル幅 ×2）の要否判断

## Phase 5 再開手順（FR-3.2 から）

1. プレビューペイン種別の導入（app 側 `previews: HashMap<PaneId, …>` が現構造に素直。
   terminals と同居）
2. syntect 依存追加（**純 Rust 構成**: default-features = false + `regex-fancy` /
   `default-syntaxes` / `default-themes`）。**`Highlighter` trait で抽象化**（ユーザー指示）
3. dispatch `OpenFile { pane, path }` + CLI `tako open <path>` + MCP `tako_open_file`
   （開発不変条件。ツール 21 個目）
4. `main.rs` の `open_file_row()` がプレースホルダで待っている

## 直近の観点・指摘（実装時に踏みやすい点）

- **スクロール関連の罠**: tmux のペインターゲットは `=セッション名:`（末尾コロン必須）。
  tmux はペインからの kitty 要求（`\e[>1u`）を認識しない → extended-keys は always。
  terminal-features の extkeys 明示が無いとネスト tmux は CSI u 入力を**捨てる**。
  conf はサーバー起動時のみ読まれる → 稼働サーバーへは `sync_conf`（起動時に呼ぶ）
- **ネスト tmux の推奨設定の正は `tmux_backend::NESTED_TMUX_SNIPPET`**。ユーザーの
  `~/.tmux.conf` は適用済み。設定適用前から起動中の claude には Shift+Enter が効かない
  （再起動で有効）
- **tmux バックエンドの要点**: spawn は `tmux_backend::wrap_options`、レイアウトは
  `tako-control::layout`、close 整合は requirements.md FR-5 節。罠は architecture.md
  「Phase 5.5」節 + 「スクロール制御」節
- **バックエンドペインは disambiguate 常時 ON**（handle_key）。マウス / CJK / CSI u /
  ネストチェーンの保証は core e2e（tmux_backend / scroll）が回帰防止
- **接続情報**: `instances/control-<pid>.json` + current。CLI は生存候補へ自動フォールバック
- **設定**: `<data_dir>/settings.json`（auto_rename / port_detect / tmux_persist）
- セルフテストは **105 項目**。IME 項目は稀にフレーク（再実行で緑）。tmux 項目は
  隔離ソケット + kill-server、接続情報は隔離ディレクトリ
- gpui ソース参照は `~/.cargo/git/checkouts/zed-*/cafbf4b/crates/gpui*` のみ（Apache-2.0）

## 現フェーズで Read すべき設計書

- パネル UI 刷新着手時: `requirements.md` FR-2.16（仕様の正）+ FR-2.13 / FR-2.10
  （統合対象の旧仕様）+ 上の「次の一手」の実装の入口
- Phase 5 再開時: 上の「再開手順」+ `architecture.md`「コンセプト②の実現」
- スクロール / ネスト tmux に触るとき: `architecture.md`「スクロール制御」+
  `requirements.md` FR-2.17
- 配布・オンボーディング着手時: `roadmap.md` Phase 7 + `requirements.md` FR-2.14 +
  `concept.md` ビジョン節

## 関連ファイル / リンク

- リポジトリ: https://github.com/takushio2525/tako（private）
- 仕様一式: `.agent/concept.md` / `requirements.md` / `architecture.md` / `roadmap.md`
- 手動チェック: `.agent/manual-checks.md` / .app 生成: `scripts/build-app.sh`
- MCP 実機検証: `scripts/verify-claude-mcp.sh`
