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

## 2026-06-12（Phase 5.5 tmux バックエンド永続化 完成）

- FR-5 実装: spawn を `tmux -L tako new-session -A` 経由に差し替え + layout.json で
  同一 ID 復元（再起動で実行中プロセス・画面内容・AI 操作が継続）。`tako persist` /
  MCP `tako_persist`（19 ツール）、tmuxview 区別表示、シェル統合は OSC パススルー +
  TMUX unset。tmux 不在は直接 spawn へ劣化。罠 3 件は architecture.md「Phase 5.5」に記録
- セルフテスト 95 項目緑（58〜62 追加）+ core e2e 2 本。ファイルツリー root 追従が
  render 依存でオクルージョン中にフレークする既存問題も修正（sync_filetree_root）
- FR-2.15「ターミナルのたまり場」を要件登録（実装は UI 相談後）。FR-2.14.5 / コンセプト追記
- 次: .app 反映（build-app.sh --install）→ ユーザー再起動 OK / Phase 5 再開は FR-3.2 から

## 2026-06-12（実機リグレッション一括修正 + 情報パネル化）

- 根本修正: tmux_bin ログインシェル解決（.app 最小 PATH が tmuxview 空・バックエンド沈黙
  劣化・明示コマンド split 失敗の共通原因）/ 明示コマンドは `$SHELL -l -c` 経由に
- マウス・キー保証: 「マウス要求アプリへ生 SGR 配送」「alt-screen 非マウスでホイールが
  矢印化しない」「CSI u（Shift+Enter / Esc）の tmux 越し往復」を core e2e 化。
  バックエンドペインは disambiguate 常時 ON + conf に extended-keys-format csi-u
- IME 候補位置: pane_cursor_origin を shaping 化（全角行で右へずれる根本原因）
- UI: 固定タブ 0 個 → 右サイドバー情報パネル（FR-2.16。tmux / agents 内部タブ・ドラッグ
  幅調整・`tako panel` + MCP `tako_panel` = 20 ツール）+ ペインタイトルバー（FR-2.1.3 更新）
- セルフテスト 98 項目緑（63 明示コマンド split / 64 panel CLI 追加）
- 次: .app 反映 → ユーザー実機確認（manual-checks「実機リグレッション修正一括」節）

## 2026-06-12（P0: CJK 全滅 + バグ (8) 接続競合 + 復元失敗の解明）

- P0 CJK: ロケール無し環境（Finder 起動）で tmux クライアントが CJK を _ 置換していた。
  backend tmux に -u + ペイン env に LC_CTYPE=UTF-8 既定注入（Terminal.app 方式）。
  LC_ALL=C 強制の e2e で回帰防止
- バグ (8): discovery を instances/ + current 構成に。CLI は生存候補へ自動フォールバック
  （除外キーは socket+token ペア）。セルフテストは TAKO_DISCOVERY_DIR で完全隔離 + 項目 65
- 再起動復元失敗の根因 = 旧ビルドの PATH 問題で layout.json が未保存（保存条件不成立）。
  現ビルドで解消を**実 .app の隔離 HOME e2e**（起動→kill→マーカー→再起動→復元+CLI 到達）で実証
- 次: ユーザー再起動（今回は復元なし・次回から効く）/ 残: ユーザー実機確認

## 2026-06-12（ウィンドウジオメトリ復元 + 引き継ぎ）

- ユーザー実機で再起動復元の完全動作を確認（Phase 5.5 実用レベル到達）。残差分の
  OS ウィンドウフレーム（サイズ・位置・fullscreen/maximized）を layout.json に追加し、
  起動時の WindowOptions へ適用（壊れた保存値は既定 960×600 へフォールバック）
- セルフテスト 101 項目緑。この worker はここで引き継ぎ（未着手一覧は activeContext.md）

## 2026-06-12（スクロール・キー実機バグ一括 + スクロール制御の方式転換）

- 実機 4 バグ + 品質 2 点を根治: 時刻表示（tmux 3.6 copy-mode インジケータ → 空書式 +
  sync_conf で稼働サーバーへ conf 再適用）/ ホイール無反応・Shift+Enter（根因は
  ネスト tmux の既定値。~/.tmux.conf 整備 = NESTED_TMUX_SNIPPET、extended-keys は
  always 必須を実測特定、FR-2.17 要件化）/ トラックパッド端数蓄積
- スクロール制御を方式転換: tako-core::scroll 新設（実体解決 = tty 突き合わせ、
  copy-mode を正確な行数で駆動、キー入力前 cancel = iTerm2 流、カーソル抑止、
  iTerm2 流フェードスクロールバー、CLI / MCP 同一経路）。コアレッシングでヌルヌル化
- 関連コミット: `6b04806` `b0301b0` `4ca3ae3` `de85fb1`。セルフテスト 105 項目緑・
  .app を /Applications へ反映済み
- 次: ユーザー実機確認（manual-checks「スクロール・キー実機バグ一括」節）→
  パネル UI 系タスク（ユーザーから別途）or Phase 5 再開（FR-3.2）

## 2026-06-12（要件一括登録: 配布 / セットアップ / FR-2.18 / FR-2.19 / パネル UI 刷新仕様）

- 実装なしの要件登録のみ（親子セッション立て直しのための退避点）: roadmap Phase 7 へ
  配布二本立て（DMG 直 DL + Homebrew Cask）+ 自動アップデート必須（単一アーティファクト）、
  FR-2.14.6 セットアップ画面（自動診断チェックリスト + ボタン一発導入）、
  FR-2.18 未表示の子の自動サーフェス、FR-2.19 localhost ポートパネルを新設
- 第2部のパネル UI 刷新（下部ステータスバー + 内部タブ 1 本化）も実装せず
  FR-2.16.4〜2.16.7 として仕様化。実装の入口・要点は activeContext.md「次の一手」
- 次: パネル UI 刷新の実装（FR-2.16.4〜2.16.7）から再開

## 2026-06-12（緊急修正: スクロール全滅の根治 = tmux ロケールサニタイズ + 署名安定化）

- スクロール全滅（方式転換後の実機初回）を根治: 根因は Dock 起動 .app のロケール無し →
  tmux 3.6 が C ロケールクライアントへの出力で制御文字を `_` 化 → タブ区切りパース全滅。
  `tmux::tmux_command()`（LC_CTYPE=UTF-8 注入 + LC_ALL 除去）へ全 tmux 呼び出しを集約。
  **tmuxview 空表示バグも同根で解消**。e2e 2 本（カナリア + 注入後 TAB 保持）追加
- バグ3 ジオメトリ復元: 現ビルドに欠陥なし（旧バイナリに保存コードが無かった一回限り）。
  フルスクリーン往復は隔離 HOME で閉ループ検証済み
- バグ2 権限ダイアログ連発: build-app.sh を ad-hoc → Apple Development 証明書の自動検出
  署名へ（TCC がビルドをまたいで権限を保持。無ければ ad-hoc 劣化 + 警告）
- 次: ユーザーが tako を再起動して 3 件の実機確認 → パネル UI 刷新（FR-2.16.4〜2.16.7）

## 2026-06-12（パネル UI 刷新 FR-2.16.4〜2.16.8 完成）

- 下部ステータスバー新設（左 = ファイルツリー、右 = tmux / git トグル。「◧ panel」廃止）+
  パネル内部タブ 1 本化（agents → 統合 tmux ビュー: タブ枠 + 全ペイン入れ子 + ゴミ箱 kill。
  旧 tmuxview 削除）+ タブ未表示 tmux の「管理外 / kill漏れ?」区別表示（FR-2.16.8 追加要件）+
  ファイルツリーの CLI / MCP 経路新設（Panel に filetree。view wire は tmux | git）
- 関連コミット: `c91f7b3` `[機能追加] パネル UI 刷新`。セルフテスト 107 項目緑・.app 反映済み
- 次: ユーザー再起動 → manual-checks「パネル UI 刷新」節の実機確認 / 次タスクは相談
  （Phase 5 再開 FR-3.2 or FR-2.19 ポートパネルが候補）

## 2026-06-12（Esc「27u」挿入バグ根治）

- 根因 = tmux 3.6 は受信 CSI 27u を内側ペインの kitty 要求に関係なく素通し（実測）×
  tako がバックエンドペインで Esc 単押しを常時 CSI 27u 送出。`CsiUMode` 導入で
  バックエンド強制時の Esc は素の `\e` に（修飾付き CSI u = Shift+Enter は維持）。
  e2e 2 本 + 単体テスト追加。別件: ロケールカナリアの挙動反転を観測 eprintln へ降格
- 次: ユーザー再起動 → manual-checks「Esc『27u』挿入バグ修正」節の実機確認

## 2026-06-13（実機バグ 3 件一括修正: 管理外誤判定 / kill 確認見切れ / ステータスバー消失）

- ① attach 済み外部 tmux セッション（例: master-tako）の「管理外」誤判定 →
  clients の tty 突き合わせで該当タブ枠へ window 一覧ごと紐付け表示（FR-2.16.9 要件化）
  ② kill 確認 UI をメッセージ + ボタンの縦積みへ共通化（render_kill_confirm、見切れ根治）
  ③ ステータスバー消失 = taffy flex 子の min-content 最小サイズが根因 → 中段 min_h(0) +
  各バー flex_none（教訓は architecture.md）。セルフテスト 109 項目緑（61f 追加）
- 次: ユーザー再起動 → manual-checks「実機バグ 3 件一括修正」節の実機確認

## 2026-06-13（Phase 5 再開: コードプレビュー / Markdown / タブ = ワークスペース）

- FR-3.2 コードプレビュー（syntect を `Highlighter` trait で抽象化 + 行番号）/ FR-3.3
  Markdown（pulldown-cmark。目アイコンで code ⇔ markdown トグル、mode は CLI / MCP 可）/
  FR-3.1 改（ファイルツリーをタブ内全ペイン cwd のマルチルート = ワークスペース表示へ刷新）。
  dispatch `OpenFile` + `tako open` + MCP `tako_open_file`（計 21 ツール）+ layout.json 永続化
- 関連コミット: `2ad0115` `[機能追加] コードプレビュー / Markdown トグル / タブ=ワークスペースのツリー刷新`
- セルフテスト 114 項目緑（66/66b/67 追加）。実装メモは requirements.md FR-3.1〜3.3
- 次: ユーザー再起動 → manual-checks「ワークスペース機能第 1 弾」節 / 次タスクは相談
  （FR-3.6 git graph or FR-2.19 ポートパネルが候補）
