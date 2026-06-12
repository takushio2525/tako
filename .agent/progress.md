# Progress Log

> AI が作業完了時に**末尾へ追記**する時系列ログ。新しいものほど下。
> 直近の作業のみ参照、エントリ 30 件超 or 14 日より古いものは `progress-archive.md` への移送を提案する。
> 自動削除はしない。常にユーザー確認を経る。

## 追記フォーマット

```markdown
## YYYY-MM-DD
- {一行サマリ。何を/どこを/結果}
- 関連コミット: `{shortsha}` `[種別] 概要`
- 次: {次にやることがあれば 1 行}
```

---

## 2026-06-11（プロジェクト開始）

- リポジトリ初期化（git init + GitHub private リポ作成）、AGENTS.md / .agent/ 構成導入
- 仕様書一式作成: concept / requirements / architecture / roadmap + README（日英）+ LICENSE（Apache-2.0）
- 未決事項: MCP トランスポート（Phase 3）、ハイライタ選定（Phase 5）、`tako` コマンド名衝突調査、Linux 対応の扱い
- 次: Phase 0 — GPUI Windows ビルド検証スパイク + 最小ターミナル描画 PoC

## 2026-06-11（Phase 0 完了）

- Phase 0 実施: GPUI 最小ウィンドウ（crates.io 0.2.2 / git rev 固定の両方）+ alacritty_terminal 最小ターミナル PoC が macOS で成功。スタック採用確定、GPUI は git rev 固定戦略
- Windows は Web 調査で成立見込み高と判断（Zed 正式リリース済み）。実ビルドは Phase 1 CI / Phase 6 実機へ残タスク化。検証結果・ハマりどころは architecture.md / poc/README.md に反映
- 関連コミット: `c1427b4` `f0e68ff` + ドキュメント反映コミット
- 次: Phase 1 — Cargo ワークスペース構成と CI（windows スモーク含む）から着手

## 2026-06-11（Phase 1 前半完了 + 仕様拡充）

- 仕様: FR-2.5 AI レイアウト操作セット / 設計原則 5 AI フルコントロール / FR-2.6 注釈レイヤ / FR-3.8 Web ビューペイン（方式候補は architecture.md）を要件化
- 実装: 4 クレートワークスペース + PaneTree ドメインモデル（GPUI 非依存・テスト 24 本）+ tako-app 最小ターミナル（セルフテスト緑）+ CI が macOS / Windows 両緑（Phase 0 残タスクの Windows スモーク完了）
- 関連コミット: `c1ae3e0` `bd69d91` `5f26d45` `559bbc5` `d9c5f8b` `fc3dad2`
- 次: Phase 1 後半 — 複数ペイン描画・タブ UI・スクロールバック

## 2026-06-11（Phase 1 後半完了 + ビジョン・要件拡充）

- 仕様: 設計原則 5 を開発不変条件へ昇格 / ビジョン明文化（AI 主体駆動開発）/ FR-4 テーマ /
  FR-2.7 成果物プレゼン（ユースケース 3 つ + 行動規範）/ FR-2.8〜2.11（フィードバック・cmd+K・
  集約センター・タイムライン）を要件化しロードマップへ配置
- 実装: tako-core に Theme / screen（色解決スナップショット、テスト 37 本）/ TerminalSession 拡張
  （リサイズ・選択・スクロール・ペースト）。tako-app は複数ペイン + タブバー + iTerm2 キーバインド +
  色・カーソル・選択コピペ・PTY 追従。セルフテスト 13 項目緑
- 関連コミット: `10ddd3d` `7d1bda3` `9f433e8` `0037034` `092e0a6` `e346cfe` `b84ae6b`
- 次: Phase 2 — 環境変数注入 + IPC + `tako` CLI

## 2026-06-11（Phase 2 完了）

- Layer 1 実装: TAKO_* 環境変数注入 + IPC サーバー（UDS 0600 + CSPRNG トークン認証）+
  `tako` CLI（split/send/focus/list/read/close/title/resize/equalize/tab 系）。
  操作ディスパッチは tako-control::dispatch に一元化（Phase 3 の MCP も同じ層を呼ぶ）。
  セルフテスト 29 項目緑（ペイン内シェルから実 CLI を叩く e2e 含む）
- 関連コミット: `3bfdedc` `14e16b2` `0b5858f` `83d17ad` + ドキュメント反映
- 次: Phase 3 — 内蔵 MCP サーバー（dispatch 共有、TAKO_MCP_URL、Claude Code 設定ゼロ接続）

## 2026-06-11（Phase 3 コア完了）

- Layer 2 実装: MCP エンジン（dispatch 共有・12 ツール・行動規範埋め込み）+ Streamable HTTP
  （TAKO_MCP_URL 注入、Bearer + Origin 検証）+ stdio ブリッジ `tako mcp serve`。
  Claude Code は env 自動発見機構なし → user スコープ登録 1 回で以後ゼロ設定が現実解。
  実 claude で stdio / HTTP 両経路の実機検証 OK、セルフテスト 36 項目緑
- 仕様追加: FR-3.10 画像プレビュー / Phase 3.5 日常使い品質（IME = M 格上げ + .app 化）/
  FR-5 セッション永続性
- 関連コミット: `a63f50e` `[機能追加] Layer 2 内蔵 MCP サーバー` + 仕様 3 コミット
- 次: Phase 3.5（IME + .app バンドル）/ Phase 4（パッシブ検知 + role/状態表示 UI）

## 2026-06-11（Phase 3.5 完了）

- 日常使い品質: IME 変換中表示（FR-1.9。EntityInputHandler + オーバーレイ + 候補位置出し、
  セルフテスト 39 項目 + manual-checks.md 新設）と .app バンドル化
  （scripts/build-app.sh: icns / Info.plist / CLI 同梱 / --verify / --install、release profile）。
  アイコンは A 案採用を assets/icon/README.md に記録
- 関連コミット: `1a8e698` `[機能追加] IME 変換中表示` / `0d0c0da` `[機能追加] .app バンドル化`
- 次: ユーザーの日常常用開始（manual-checks.md）/ Phase 4（パッシブ検知 + role/状態表示 UI）

## 2026-06-11（常用初日バグ修正 + 境界ドラッグリサイズ）

- バグ修正 3 件: ①TERM 未設定で tmux が落ちる→spawn で TERM=xterm-256color/COLORTERM=truecolor
  既定注入（options.env 優先）②初期 cwd が `/`→$HOME 既定（継承は OSC 7 で Phase 4）
  ③Backspace は \x7f で正しく症状は①の二次効果と判明、特殊キーの byte unit test 追加
- 機能: ペイン境界ドラッグリサイズ。tako-core に borders/set_split_ratio/ratio_for_position
  追加（pre-order index で分割特定、ユニットテスト 3 本）。UI は透明ハンドル div + cursor +
  グローバル on_mouse_move。セルフテスト 44 項目（1b〜1e/5b 追加）緑
- 関連コミット: `d8f3752` `[修正] TERM/cwd` / `52294ab` `[修正] 特殊キー総点検`
- 次: 日常常用継続 / Phase 4（パッシブ検知 + role/状態表示 UI）

## 2026-06-11（常用クラッシュ根治 + Phase 3 完了 + Phase 4 前半）

- 常用クラッシュ根治: login ラッパ起因の close ごとの fd/スレッド/プロセスリーク →
  $SHELL 直接 spawn で解消、PTY 生成失敗の expect panic → Result 化でエラー応答に。
  境界ドラッグ状態の残留（MouseUp 取りこぼし）も修正。教訓は architecture.md へ
- Phase 3 完了 + Phase 4 前半: role/title バッジ + 状態ドット UI（FR-2.1.3〜2.1.4）、
  TapPty による OSC 7/133 検知（osc_tap.rs）、zsh/bash/fish シェル統合自動注入
  （FR-2.4.1）、list・MCP への cwd/state/exit_code 公開、split の cwd 継承。
  セルフテスト 55 項目緑
- 関連コミット: `44c794e`（クラッシュ根治）`30827b6`（シェル統合）`1f6ff12`（状態 UI）他
- 次: Phase 4 後半（listen ポート検知・提案チップ・集約センター FR-2.10）

## 2026-06-12（接続情報の永続化 FR-2.2.9）

- control.json（0600/0700、tmp+rename）へ socket/token/mcp_url を永続化し、CLI は
  env → ファイルの順で解決（接続不可・認証失敗のみフォールバック）。アプリ再起動後の
  外部長寿命プロセスから手作業ゼロ接続を実機検証。MCP ブリッジは env あり時のみ
  フォールバック（tako 外 0 ツール維持）。セルフテスト 59 項目緑
- 次: Phase 4 後半（listen ポート検知・提案チップ・集約センター FR-2.10）

## 2026-06-12（常用フィードバック一括対応）

- スクロールバック出し分け（wheel_action: mouse reporting 転送 / alt screen 矢印変換 /
  通常画面自前）+ スクロールバー（FR-2.5.13。tako scroll / tako_scroll_pane / list 公開）
- Shift+Enter（Config.kitty_keyboard 有効化 + CSI u 送出 + 修飾付き機能キーの xterm 形式）、
  IME 候補位置（ライブ変換の文書全体オフセットを marked 内へ解釈）、全角行の選択座標
  （shaping ベース cell_at + ScreenLine::cell_cols）、ペイン × ボタン（dispatch 経由）
- FR-2.12（AI 自動リネーム）を要件登録（実装は次ターン以降）。セルフテスト 69 項目緑
- 関連コミット: `2e1f718` `6c7ef60` `0693120` `fa18c47` `44f4699` `8fed3ca` 他
- 残課題: 描画のグリッド不一致（全角 advance ≠ 2 セル）は座標変換のみ吸収、描画は未対応
- 次: FR-2.12 実装 → Phase 4 後半

## 2026-06-12（tmuxview FR-2.13 完成）

- tmux 見える化: tako-core::tmux（取得層・パースはユニットテスト）+ dispatch
  TmuxList/TmuxKill（tty 突き合わせで tako タブ・ペイン対応付け）+ `tako tmux list/kill` +
  MCP 2 ツール（15 個）+ 右端固定タブ UI（確認つき kill、2 秒更新）。セルフテスト 73 項目緑
- CI の fmt 検査落ち（clippy のみ確認していた）を修正し緑復帰。教訓: コミット前は
  `cargo fmt --all --check` も回す
- 次: FR-2.12（実行体の設計分岐の回答待ち）→ Phase 4 後半

## 2026-06-12（AI 自動リネーム FR-2.12 完成）

- 方式 1 = tako 常駐（ユーザー承認済み）で実装: TitleSource（手動優先）+ タブ rename API
  （`tako tab rename` / MCP `tako_rename_tab`）+ 検知ループ（autorename.rs、指紋 +
  デバウンス 4 秒 + クールダウン 30 秒）+ `claude -p --model claude-haiku-4-5-20251001`
  子プロセス（プロンプト 1 本・タイムアウト 30 秒）+ ヒューリスティックフォールバック +
  OFF 設定（settings.json + `tako autorename` / MCP。計 17 ツール）。セルフテスト 77 項目緑
- 次: Phase 4 後半（listen ポート検知 → 提案チップ → 集約センター）。
  claude 実呼び出しの見た目は常用確認（manual-checks.md）

## 2026-06-12（listen ポート検知 FR-2.4.2 完成）

- tako-core::ports 新設: libproc（proc_listpids / PROC_PIDFDSOCKETINFO）+ tty 突き合わせで
  ペイン配下の LISTEN 中 TCP を 3 秒ポーリング検知し、list / MCP の listen_ports へ公開。
  socket_fdinfo は SDK 転記 + 自プロセス listen のユニットテストで ABI 検証。
  セルフテスト 79 項目緑（nc -l の e2e 含む）
- 次: 提案チップ（FR-2.4.3〜4）は表示位置・承諾アクションの設計分岐を**ユーザーへ確認
  してから**着手 → 集約センター（FR-2.10）

## 2026-06-12（提案チップ FR-2.4.3〜4 完成 + FR-2.14 要件化）

- 提案チップ: 検知ペイン下端インライン（新規ポート diff で生成、却下は同ポート存続中
  再提案しない）、承諾 = open_preview（外部ブラウザ。Phase 5 で Web ペインへ差し替える
  抽象点、ユーザー承認済み）。OFF は settings.port_detect + tako portdetect / MCP
  tako_port_detect（計 18 ツール）。セルフテスト 83 項目緑
- FR-2.14（MCP ゼロコンフィグオンボーディング）を要件登録（実装は Phase 7 前）
- 次: 集約センター（FR-2.10）で Phase 4 完了

## 2026-06-12（集約センター FR-2.10 完成 = Phase 4 完了）

- 右端固定タブ「agents」: 全タブ・全ペインの状態を注目度順（エラー > 入力待ち > 実行中 >
  不明）に集約、クリックで dispatch Focus 経由ジャンプ（タブ切替も伴う）。
  agents タブに全ペイン集約ドット。tmuxview と同型の固定タブパターン。
  セルフテスト 84 項目緑。これで Phase 4（Layer 3 パッシブ検知）完了
- 次: Phase 5（ワークスペース機能）か FR-2.14 前倒しをユーザーと相談

## 2026-06-12（ファイルツリー FR-3.1/3.7 完成 → Phase 5 一時中断）

- Phase 5 着手: 技術選定確定（syntect + Highlighter trait 抽象 / git CLI 子プロセス /
  pulldown-cmark。architecture.md に記録）→ ファイルツリー完成（filetree.rs に状態・
  読み込み・フラット化を分離 + ユニットテスト 4 本、cmd+B トグル、cwd 追従、
  content_origin シフトでペイン座標系と連動）。セルフテスト 86 項目緑
- **ユーザー指示で Phase 5 を一時中断、Phase 5.5（tmux バックエンド永続化）を別 worker が
  先行**。中断点と再開手順は activeContext.md「Phase 5 の中断点」
- 次: Phase 5.5（別 worker）/ 再開時は FR-3.2 コードプレビュー + tako_open_file
