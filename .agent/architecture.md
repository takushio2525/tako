# architecture.md — 技術設計

> 「どう実現するか」を定義する。要件は `requirements.md`、実装順序は `roadmap.md`。

## 技術スタック

| 領域 | 採用 | 根拠 |
|---|---|---|
| 言語 | Rust | ネイティブ性能、メモリ安全、Warp / Zed / Alacritty の実績 |
| UI フレームワーク | **GPUI**（Zed 製、**zed リポ git rev 固定**） | GPU 描画で Zed 級の速度を出せる唯一級の Rust UI。Zed 本体が実証。バージョン戦略は下記 |
| ターミナルエミュレーション | **alacritty_terminal 0.26+**（crates.io） | 枯れた VT 実装（Apache-2.0、2026-04 更新で活発）。Zed のターミナルも同クレート採用 |
| PTY | **alacritty_terminal::tty**（Phase 0 で確定） | 同クレートが macOS openpty / Windows ConPTY を吸収済み。portable-pty は不要と判断 |
| 非同期 | **GPUI executor + futures channel**（Phase 0 で確定） | PTY IO は alacritty の EventLoop スレッド、UI への通知は channel + `cx.spawn` で足りる。tokio 不要 |

### ⚠️ 採用リスク（明記事項）

1. **GPUI は pre-1.0 であり、破壊的変更が頻発する。**
   Zed 本体の都合で API が変わる前提で付き合う。対策:
   - GPUI への依存を `ui/` レイヤに閉じ込め、コアロジック（ペインツリー・制御プレーン）は GPUI 非依存に保つ
   - **git rev 固定**（`rev = "..."`）で依存し、追従は意識的なタスクとして行う（自動更新しない）
2. **GPUI の Windows 対応**: Phase 0 の調査で「ビルド・起動の成立」リスクはほぼ解消と判断
   （下記「Phase 0 検証結果」参照）。残るのは品質面（スクリーンリーダー欠落等）と実機未検証であること。
3. **GPUI の汎用フレームワークとしての開発減速（2025 年末に Zed が表明）**。
   crates.io リリースは停滞しており、安定供給は期待できない。コミュニティフォーク
   gpui-ce（crates.io に 0.3.x あり、元 Zed 社員主導）が乗り換え先の保険。
   「ui/ に閉じ込める」方針がこのリスクの防波堤を兼ねる。
4. **ライセンス互換性**: GPUI は Apache-2.0、alacritty_terminal は Apache-2.0。
   ただし GPUI の推移的依存に zlog/ztracing（GPL-3.0-or-later）が含まれるため、
   tako 全体は GPL-3.0-or-later を採用。cmux のコードは引き続き絶対に読まない（`concept.md` 参照）。

### Phase 0 検証結果（2026-06-11、詳細は `poc/README.md`）

**結論: Rust + GPUI + alacritty_terminal スタックは成立。** macOS で最小ターミナル
（シェル起動・出力描画・キー入力）が動作した。PoC は `poc/` 配下（本実装とは分離）。

#### GPUI バージョン戦略: zed リポ git rev 固定を採用

- **crates.io 版（0.2.2）は 2025-10-22 以降更新停止**。開発減速宣言もあり再開は期待薄
- Windows まわりの修正・改善が入るのは git 版のみ → **git + `rev` 固定 + 意識的な追従**が唯一の現実解
  （gpui-component / Longbridge Pro など単体利用の先行事例も同方式）
- 検証時 rev: `cafbf4b5df7fedb67fc0f248850a5654efcec5d9`（2026-06-10 の main）

#### git 版 gpui のハマりどころ（実装時に必ず踏む）

- **`gpui_platform` の `font-kit` feature を有効にしないと文字が一切描画されない**（無警告でスタブ化）
- `Application::new()` 廃止 → `gpui_platform::application()`（プラットフォーム実装が別クレートに分離）
- 最新 stable Rust が必要（1.89 不可、1.95.0 で確認）。`rust-toolchain.toml` でピン留めする
- ウィンドウがオクルージョン状態だと display link が止まり再描画されない（仕様）
- `WindowHandle<V>::update` 内での `dispatch_keystroke` はビュー二重借用でパニック → `AnyWindowHandle::update` を使う
- IME（Phase 3.5 で実装。FR-1.9）: `Window::handle_input` は **paint フェーズ限定 API**
  → 何も描かない `canvas` の paint フックから `ElementInputHandler` を登録する。
  `on_key_down` で PTY へ書いたら **`cx.stop_propagation()` 必須**（未処理扱いだと macOS が
  キーを inputContext へ回送し insertText → `replace_text_in_range` で二重入力になる）。
  `StyledText::with_default_highlights` のハイライト範囲は**非重複・昇順**必須
  （重ねると `invalid text run` でパニック）。NSTextInputClient の範囲はすべて UTF-16 オフセット

#### GPUI の Windows 対応の現状（2026-06 時点、Web 調査ベース・実機未検証）

- **Zed 本体の Windows 版は 2025-10-15 に正式リリース済み**（DirectX 11 + DirectWrite のネイティブ実装、
  毎週リリースに組込み）。「Windows 対応は実験的」という認識はもう古い
- gpui 単体も Windows は feature flag 不要の公式サポート。gpui-component（★11k、Windows 対応明記）と
  その商用利用（Longbridge Pro）という git 依存単体利用の実績あり
- ビルド前提: MSVC C++ build tools + **Spectre-mitigated libs** + Windows 10 SDK 10.0.20348.0+ + CMake
- 既知の未成熟箇所（zed リポ issue、`platform:windows` 約 120 件）:
  - **スクリーンリーダー（UIA)対応が完全欠落**（#41138、未解決）— アクセシビリティ要件には致命的
  - フォント描画: Mica 有効時のぼやけ（#56382）、**ターミナル TUI のフォント描画崩れ（#58830）**、リガチャ（#51754）
  - テキスト入力時の GPU 負荷が高い（#37727）、画像アトラス解放漏れ（#56667）
  - IME（日本語含む CJK）は 2025 年末に集中修正されおおむね機能する模様
- **未検証**: この PC は macOS のため Windows 実ビルドは未実施。Phase 1 の CI 整備時に
  GitHub Actions の windows ランナーで PoC をビルド・スモークし、実機級の検証は Phase 6 で行う（`roadmap.md`）

## 全体レイヤ構成

```
┌─────────────────────────────────────────────┐
│ ui/        GPUI 依存はここだけ                  │
│  ターミナルビュー / タブバー / ペインレイアウト   │
│  サイドバー（ファイルツリー / git graph）        │
│  提案チップ                                    │
├─────────────────────────────────────────────┤
│ control/   制御プレーン（GPUI 非依存）           │
│  ipc サーバー（Layer1 CLI の受け口）            │
│  mcp サーバー（Layer2）                        │
│  detect（Layer3: OSC / listen ポート検知）      │
├─────────────────────────────────────────────┤
│ core/      ドメインモデル（GPUI 非依存）         │
│  Workspace / Tab / PaneTree / Pane            │
│  TerminalSession（alacritty_terminal + PTY）  │
├─────────────────────────────────────────────┤
│ platform/  OS 差分の吸収                       │
│  PTY 生成 / IPC トランスポート / プロセス監視    │
└─────────────────────────────────────────────┘
```

依存方向: `ui → control → core → platform`。逆依存・循環依存禁止。
**core と control を GPUI 非依存に保つ**ことが、GPUI 破壊的変更リスクの防波堤。

クレート分割（Cargo ワークスペース、Phase 1 で確定）:
`tako-core` / `tako-control` / `tako-app`（GPUI バイナリ）/ `tako-cli`（Layer1 CLI バイナリ）。

## ドメインモデル

```
Workspace
└── Tab (= エージェントグループ。1 グループ = 1 タブ)
    └── PaneTree (二分木: Split { axis, ratio, children } | Leaf(Pane))
        └── Pane
            ├── TerminalPane (TerminalSession を保持)
            ├── PreviewPane (Code | Markdown | Pdf | Editor)
            └── WebViewPane (URL を表示。実現方式・リスクは「Web ビューペイン」節、後段フェーズ)
```

- `PaneId` / `TabId` はプロセス生存期間中ユニークな整数 ID（環境変数・CLI で使う）
- 自動生成ペインは必ず「呼び出し元 Pane が属する Tab」に挿入する（FR-2.1.2）
- Pane は `role`（任意ラベル）と `origin`（user / cli / mcp / suggestion）を持ち、
  UI 表示とポリシー制御（FR-2.3.5）に使う

### ⚠️ PTY セッション破棄のハマりどころ（2026-06-11 常用クラッシュの教訓）

- **alacritty に既定シェル解決を任せない**（`tty::Options.shell = None` 禁止）。macOS では
  setuid root の `login` ラッパ経由になり、ペイン close 時の `Pty::drop` が
  `kill(login, SIGHUP)` を権限エラーで失敗（返り値無視）→ `child.wait()` 永久ブロック →
  **close のたびに master fd・signal fd・IO スレッド・login プロセスがリーク**する。
  本家 alacritty はウィンドウ close = プロセス終了のため顕在化しないが、tako はペイン単位で
  セッションを破棄するので直撃する（fd 枯渇 → PTY 生成失敗）。
  tako は `$SHELL` をユーザー権限で直接 spawn する（`TerminalSession::default_shell`、`-l` 付き）
- **PTY 生成失敗で panic しない**。GPUI のイベント処理は FFI コールバック内のため、
  panic は unwind できず SIGABRT でアプリごと落ちる。`spawn_session` は Result を返し、
  失敗時はペインを巻き戻して CLI / MCP へエラー応答する。
  回帰はセルフテスト 40 / 40b（split→close ストレス + fd リーク検査）で機械検証する

## 制御プレーン（コンセプト①の 3 層）

### 環境変数注入（共通基盤）

TerminalSession がシェルを spawn する際に注入する:

| 変数 | 内容 |
|---|---|
| `TAKO_PANE_ID` | 呼び出し元ペインの ID |
| `TAKO_TAB_ID` | 所属タブの ID |
| `TAKO_SOCKET` | IPC エンドポイント（macOS: Unix domain socket パス、Windows: named pipe 名） |
| `TAKO_MCP_URL` | 内蔵 MCP サーバーの接続先（Layer2 自動発見用。**Phase 3 で注入開始**） |
| `TAKO_TOKEN` | 接続認証トークン（セッション毎に生成。外部プロセスの接続拒否に使う） |

Phase 2 時点では `TAKO_MCP_URL` 以外の 4 つを `TerminalSession::spawn`（`SpawnOptions.env`）
経由で注入済み（tako-app の `spawn_session`）。

### Layer 1: CLI（`tako-cli`）→ ✅ 実装済み（Phase 2、2026-06-11）

- 単一バイナリ `tako`。`TAKO_SOCKET` + `TAKO_TOKEN` を読んで IPC サーバーに JSON-RPC で接続
- pane 指定省略時は `TAKO_PANE_ID` を呼び出し元として使う（FR-2.2.7）
- `TAKO_SOCKET` が無ければ「tako の外で実行されている」エラー（FR-2.2.8）
- サブコマンド: `split` / `send` / `focus` / `list` / `read` / `close` / `title` /
  `resize` / `equalize` / `tab new` / `tab select` / `tab move-pane`（カタログは FR-2.5）
- IPC プロトコルは MCP ツールと同じ操作セットに 1:1 対応させ、実装を共有する

### IPC トランスポート（Phase 2 実装メモ）

- ワイヤ形式: **1 行 1 JSON の JSON-RPC 2.0 サブセット + `token` フィールド拡張**
  （`crates/tako-control/src/protocol.rs` が正）。操作セットは FR-2.5 と 1:1
- **操作のディスパッチは `tako-control::dispatch`（`ControlHost` trait）に一元化**。
  tako-app の IPC 受信ループと Phase 3 の MCP サーバーが**同じ dispatch を呼ぶ**ことで、
  設計原則 5（UI でできることはすべて AI からもできる）のセマンティクスを一箇所に保つ
- unix: `$TMPDIR/tako-<pid>-<seq>.sock`（パーミッション 0600）+ 32 バイト CSPRNG トークン
  （getrandom）。accept スレッド + 接続毎スレッドのブロッキング IO で受け、リクエストは
  futures channel で UI スレッドへ渡して dispatch する（tokio を持ち込まない方針を維持）
- CLI / MCP からの `close` は「最後のタブの最後のペイン」を拒否する
  （アプリ終了に等しい操作は UI の cmd+W のみ。FR-2.5.9 の安全性方針）
- **接続情報の永続化と発見（FR-2.2.9、2026-06-12）**: アプリ起動時に
  `<data_dir>/control.json`（0600 / 親ディレクトリ 0700。tmp + rename で原子的に更新）へ
  socket / token / mcp_url を書き出す（`tako-control::discovery`）。CLI は
  環境変数 → 発見ファイルの順で解決し、env があっても**接続不可・認証失敗のときだけ**
  フォールバックする（操作エラーはそのまま返す）。ソケットパスは PID 入りのまま
  （安定パスは複数インスタンスで取り合いになるため不採用）。複数インスタンスは
  最新起動がファイルを上書き = 最新優先。終了時の削除はしない（GPUI の終了経路で
  Drop が保証されない。残骸は接続失敗として顕在化し、誤接続はトークンで防がれる）
- **TODO(Phase 6): Windows named pipe**。`IpcServer::start` と CLI の transport は
  `#[cfg(windows)]` でスタブ化済み（サーバー起動失敗でもアプリは IPC なしで継続する）。
  実装時の検討事項:
  - パイプ名規約（`\\.\pipe\tako-<pid>` 想定）を `TAKO_SOCKET` に入れる
  - アクセス制御: UDS の 0600 に相当する DACL（同一ユーザー限定）+ トークン認証の二段
  - ConPTY 環境での env 注入は alacritty_terminal の `tty::Options::env` 依存のため実機確認

### Layer 2: 内蔵 MCP サーバー → ✅ 実装済み（Phase 3、2026-06-11）

#### 構成: トランスポート非依存エンジン + 2 トランスポート

- **MCP エンジン**（`tako-control::mcp`）: initialize / tools/list / tools/call の JSON-RPC
  処理とツールカタログ。**実行は IPC と同じ `dispatch` を呼ぶだけ**（操作セマンティクスの
  一元化 = 設計原則 5。MCP 固有の操作実装はゼロ）
- **トランスポート 1: Streamable HTTP**（`McpServer`、tako-app に内蔵）:
  127.0.0.1 の空きポートに tiny_http（同期・スレッドベース。tokio を持ち込まない方針を維持。
  公式 SDK rmcp は tokio 必須のため不採用）で立て、URL を `TAKO_MCP_URL` として全ペインへ注入。
  認証は `Authorization: Bearer <TAKO_TOKEN>`（IPC とトークン共有）+ Origin ヘッダ検証
  （非 localhost は 403。DNS リバインディング対策）。POST のみ実装（GET の SSE ストリームは
  サーバー発信を持たないため 405）。**Windows でも動く**（named pipe 未実装の Phase 6 まで、
  Windows のエージェント連携は HTTP 側が受け皿になる）
- **トランスポート 2: stdio ブリッジ**（`tako mcp serve`、tako-cli）:
  stdin/stdout で MCP を話し、実行だけ IPC へ `origin="mcp"` で中継する。
  接続情報は**起動される度に** `TAKO_SOCKET` / `TAKO_TOKEN` / `TAKO_PANE_ID` を環境変数から
  読む（エージェントの子プロセスとして起動されるため、ペインのシェル → エージェント →
  ブリッジと環境が継承される = 呼び出し元ペインの特定が自動で成立する）。
  tako の外で起動された場合は **0 ツール**を返して無害化する

#### Claude Code「設定ゼロ接続」の検証結果（2.1.172、2026-06-11）

- Claude Code には**環境変数だけから MCP サーバーを自動発見する機構が無い**。
  登録経路は `.mcp.json`（プロジェクト）/ user・local スコープ設定 / `--mcp-config` フラグのみ
- プロジェクト `.mcp.json` の自動生成案は不採用: ユーザーのリポジトリを汚す・承認プロンプトが
  出る・URL とトークンがセッション毎に変わり静的ファイルと相性が悪い
  （`.mcp.json` は `${VAR}` 展開を持つため `${TAKO_MCP_URL}` 参照は可能だが、
  tako 外で開いたときに壊れた設定として残る）
- **採用: user スコープへの stdio ブリッジ登録（初回 1 回だけ）**

  ```sh
  claude mcp add --scope user tako -- /path/to/tako mcp serve
  ```

  以後はどのプロジェクト・どのペインでも設定なしでペイン操作ツールが使える。
  ブリッジが毎回環境変数を読むため URL / トークンの変動に強く、tako 外では 0 ツールで邪魔しない
- 実機検証は `scripts/verify-claude-mcp.sh`（GUI なしで IPC + MCP + dispatch を立てる
  `tako-control` の example `mcp_host` 内で実物の `claude -p` を実行。stdio / HTTP の両経路。
  ユーザーのグローバル claude 設定は変更しない `--mcp-config --strict-mcp-config` 方式）

#### 公開ツール（実装済み 12 個。FR-2.5 と 1:1）

`tako_list_panes` / `tako_split_pane` / `tako_send_input` / `tako_read_pane` /
`tako_focus_pane` / `tako_close_pane` / `tako_resize_pane` / `tako_equalize_layout` /
`tako_set_title` / `tako_create_tab` / `tako_select_tab` / `tako_move_pane_to_tab`

- 誤用しにくさを優先した設計: `tako_send_input` / `tako_read_pane` は **pane 必須**
  （省略で自ペインへ誤送信する事故を防ぐ）。`tako_close_pane` は pane 省略 = 自己片付け
  （FR-2.5.4）。スキーマは `additionalProperties: false` + enum で締める
- initialize の `instructions` とツール説明文に FR-2.7.5 の行動規範を埋め込み済み
  （レビューを求めるときは見せろ / 読んでほしければ開け / 方針相談は例を作って並べろ /
  終わったら片付けろ / 操作前に list で現状把握）
- 後段フェーズの追加ツール（案）: `tako_open_file`（プレビュー表示、FR-2.5.11）/
  `tako_open_url`（Web ビュー、FR-2.5.12）/ `tako_annotate`（注釈オーバーレイ、FR-2.6）/
  `tako_show_file` / `tako_show_diff` / `tako_show_url`（AI 成果物プレゼンテーション、FR-2.7）
- 呼び出し元ペイン特定（FR-2.3.3）: stdio = `TAKO_PANE_ID`、HTTP = `X-Tako-Pane` ヘッダ。
  pane 省略時のデフォルト対象が呼び出し元（= 同タブ）になる。ハードなスコープ強制は
  FR-2.3.5 のポリシー制御と併せて後段

### Layer 3: パッシブ検知 → OSC 7/133 は実装済み（2026-06-11）

- **OSC 7**（cwd 通知）→ ファイルツリーの cwd 連動（コンセプト②でも使う）
- **OSC 133**（プロンプトマーク）→ コマンド単位の区切り・実行中/完了の把握
- シェル統合スクリプトは zsh / bash / fish / PowerShell を同梱し、可能な範囲で自動注入
- **実装メモ（2026-06-11）**: vte は OSC 7/133 を unhandled で捨てるため、
  `EventedPty` の委譲ラッパ `TapPty`（`tako-core::osc_tap`、分割読み耐性スキャナ）で
  PTY 読み取りバイト列を EventLoop 手前で観測する（バイト列は不変更）。
  検知は `SessionEvent::Osc` → `TerminalSession` の cwd / `CommandState` へ反映し、
  dispatch の list（CLI / MCP）に `cwd` / `state` / `exit_code` として公開。
  シェル統合の注入は `tako-core::shell_integration`（zsh = ZDOTDIR / bash = PROMPT_COMMAND /
  fish = XDG_DATA_DIRS の 3 点セットを判定なしで常時注入。
  `TAKO_NO_SHELL_INTEGRATION=1` で無効化。PowerShell は Phase 6）
- **listen ポート検知** → 検知層は実装済み（2026-06-12、FR-2.4.2。`tako-core::ports`）
  - macOS: libproc（`proc_listpids` → `proc_bsdinfo.e_tdev` とペインの PTY スレーブ rdev の
    突き合わせで「ペイン配下」を判定 → `PROC_PIDLISTFDS` + `PROC_PIDFDSOCKETINFO` で
    LISTEN 中 TCP を列挙）。libc に無い `socket_fdinfo` 系は SDK ヘッダから転記し、
    **自プロセスで実際に listen するユニットテストで ABI を e2e 検証**している。
    Windows: `GetExtendedTcpTable`（Phase 6）
  - ポーリング方式（3 秒）。スキャンは background executor、結果は TerminalSession に保持し
    list / MCP の `listen_ports`（port / pid / process）として公開
- 検知結果は**提案チップ**（「localhost:5173 をプレビューで開く？」）として UI に出すだけ。
  承諾時のみペイン生成（強制分割はしない）。設定で全体を無効化可能（FR-2.4.4）

## Phase 5.5: tmux バックエンド永続化（FR-5。2026-06-12 実装）

全ペインの PTY を tako 専用 tmux サーバー（`tmux -L tako`。ユーザーの既定サーバーとは
分離・専用 conf でユーザーの `~/.tmux.conf` は読まない）のセッションとして保持し、
再起動時に attach し直して実行中プロセス・画面内容ごと完全復元する。

- **spawn 経路**（`tako-core::tmux_backend::wrap_options`）: シェル直接 spawn の代わりに
  `tmux -L tako -f <conf> new-session -A -D -s tako-<rand>` を PTY 子プロセスにする。
  `-A` で「新規作成」と「再 attach」が同一コマンド（消えていたセッションは `-c` の
  保存 cwd で開き直しになる）。`-D` で多重起動時は最新インスタンスへ収束
- **レイアウト**（`tako-control::layout` → `<data_dir>/layout.json`）: タブ / 分割ツリー /
  タイトル / role / cwd / セッション名を**ペイン・タブ ID ごと**保存し、復元時は同じ ID を
  再現（`Pane::restore` 等が採番カウンタを fetch_max で先へ進める）。これで tmux 内で
  生き続けるプロセスの `TAKO_PANE_ID` / `TAKO_TAB_ID` が再起動後も有効。旧 socket/token は
  CLI / MCP ブリッジの control.json フォールバック（FR-2.2.9）が吸収する。
  保存は 2 秒ポーリング + dispatch 後 + cmd+Q 時（差分時のみ書き込み）
- **PC 再起動時の Claude 会話復旧**（Issue #139）: `tako-control::agents` が
  `claude agents --json` を 1 回取得し、tmux `pane_pid` への祖先照合で確定した
  backend session → Claude session ID 対応を 5 秒ごとにバックグラウンド更新する。
  `layout.json` の各ペインへ session ID を保存し、復元時は backend tmux session が
  **存在しない場合だけ** transcript の存在・ID 形式を確認して、新規ログインシェルの PTY へ
  `claude --resume <session-id>` を投入する。backend が生存する通常の tako 再起動では
  再 attach のみで二重起動しない。検出成功時に一覧から消えたペインの関連は削除し、
  ユーザーが明示終了した古い会話を次回 PC 起動で勝手に戻さない。制御は既存 persist 設定を共有
- **tmux 不在時の劣化と診断**（Issue #30。2026-07-02）: レイアウトの保存・復元は
  **tmux が無くても機能する**（PTY のみ直接 spawn に劣化。保存時は `session: None` +
  cwd、復元時は保存 cwd で新シェルを開く「構造のみ復元」）。かつては保存・復元とも
  `tmux_backend::available()` にゲートされており、tmux 未導入の配布先（Homebrew cask は
  tmux を依存に含まない）で **persist 全体が無音で不活性化**していた。結果は
  `<data_dir>/persist.log`（`tako-control::diag`。復元成否・理由・明示削除を記録、
  256KB で `.old` ローテート）と `tako persist` / MCP `tako_persist` の
  `layout_path` / `layout_exists` / `last_restore` / `log_path` で診断できる。
  破損 layout.json は上書きせず `layout.json.corrupt` へ退避する
- **close 整合**: 明示 close（×・cmd+W・CLI / MCP close）= バックエンドセッションも kill。
  アプリ終了・クラッシュ = detach のみ（= 永続化）。PTY 死亡由来の close
  （SessionNotice::Exited。`CloseReason` で明示 close と区別）はセッションを kill せず、
  全滅時も layout.json を保持する（Issue #30。2026-07-03 実機: サーバー死で全タブ道連れ）。外部 tmux に attach しただけの
  ペインは何も kill しない（kill 対象は `backend_sessions` 登録分のみ）。詳細は
  `requirements.md` FR-5 の close 整合節
- **共存のための conf**（`<data_dir>/tmux-backend.conf`、毎起動再生成）:
  `status off` / `prefix None`（tmux の UI・キー介入ゼロ）、`mouse on`（マウス要求
  アプリへの SGR 生転送に必要。**非マウスペインのスクロールは SGR ではなく
  tako 自身が copy-mode を駆動する** → 下記「スクロール制御」）、`allow-passthrough on`、
  `extended-keys always` + `terminal-features extkeys`（kitty / CSI u 維持）、
  `history-limit 10000`、`update-environment TAKO_*`、
  `copy-mode-position-format ''`（copy-mode 右上の位置インジケータを消す。tmux 3.6 の
  既定書式は先頭行タイムスタンプ = **時刻表示**を含み、スクロール中に謎の時計として
  見える実機バグ (2) の正体だった。位置は tako 側スクロールバーが示す）。
  **conf はサーバー起動時にしか読まれない**ため、稼働中サーバーへは起動時に
  `tmux_backend::sync_conf`（`source-file`）で再適用する（下記の罠）
- **シェル統合の共存**: tmux は OSC 7 / 133 を外へ転送しないため、統合スクリプトが
  バックエンド配下（`$TMUX` のソケット basename が `tako*`）では OSC を
  `\ePtmux;…\e\\` パススルーで包む。同時に **TMUX / TMUX_PANE を unset** し、
  ユーザー自身の `tmux` 利用（ネスト）を素通しにする（バックエンドは見えない裏方）
- **tty 突き合わせの維持**: ペイン配下プロセスの制御端末はバックエンドサーバー側の
  ペイン tty になるため、spawn 後に `list-panes -t =<session> -F '#{pane_tty}'` で解決して
  `TerminalSession::set_tty_name` で差し替える（listen ポート検知 FR-2.4.2 と
  tmuxview FR-2.13.2 が引き続き機能する）

### スクロール制御（`tako-core::scroll`。2026-06-12 夜に方式転換）

バックエンドペインのスクロールバックは tmux 側にあり、ユーザーが自前 tmux を
ペイン内で attach していれば**ネスト先サーバー**にある。当初は SGR ホイールを
流し込んで tmux 既定バインドの copy-mode に任せていたが、実機で
① 1 イベント = 5 行で「ばっ」と飛ぶ ② copy-mode に入りっぱなしでキー入力が
飲まれる ③ copy-mode カーソルが画面に居座る、の 3 症状が出た。現方式:

- **解決**: `scroll::resolve_target` がペインの tty とネスト候補サーバーの
  `list-clients` を突き合わせ、実体（Backend / Nested）を特定する
- **駆動**: `scroll_by` / `scroll_to` が `copy-mode -e` + `send-keys -N n -X
  scroll-up/down` を正確な行数で発行（履歴ゼロでは copy-mode に入らない）。
  ターゲット指定は `=セッション名:`（**末尾コロン必須**。`=name` 単体は
  "can't find pane" になる）
- **出し分け**: 対象ペインの `mouse_any_flag` が立っていれば（vim 等）従来どおり
  生 SGR を転送（core e2e が保証）。それ以外は tako 駆動
- **UI 側**（`ScrollCtl`）: ホイールは pending に積んで 1 つの tmux 操作へ
  コアレッシング（洪水対策）。キー入力・IME 確定の前に `cancel` を同期実行して
  iTerm2 流「打ったら最下部へ戻って入力」。copy-mode 中はカーソル強調を抑止
  （`screen_opts`）。スクロールバーは tmux の position / history を表示し、
  スクロール中だけ表示 → フェードアウト
- **CLI / MCP**: dispatch の `Scroll` が同じ `tako-core::scroll` を呼ぶ
  （開発不変条件）。応答を UI が `sync_scroll_from_dispatch` で取り込み、
  AI のスクロールでもバーが出る

### ⚠️ スパイクで踏んだ罠（再発防止）

- **tmux に明示コマンドを渡すと `default-shell -c <cmd>` で実行される**: この非対話 zsh
  ラッパーが tako のシェル統合 .zshenv を読んで ZDOTDIR を消費し、内側の対話シェルに
  統合が届かなくなる。**既定シェル起動ではコマンドを渡さず** tmux の default-shell
  （ログインシェル直接 spawn）に任せる
- **`"${var//$'\e'/$'\e\e'}"` はダブルクォート内で置換側の `$'…'` がリテラル**になる
  （zsh / bash 共通）。ESC の二重化は `local esc=$'\e'` を経由する
- **`display-message -p` はクライアント無しだと空を返す**。detached セッションの
  pane_tty 取得は `list-panes -F` を使う
- **conf（`-f`）はサーバー起動時にしか読まれず、サーバーは tako の再起動を生き残る**:
  バージョン更新で conf を変えても既存サーバーには永久に届かない。起動時・persist
  有効化時に `sync_conf`（`tmux source-file`）で再適用する（e2e:
  `sync_confは稼働中サーバーへ設定を再適用する`）。サーバー不在時の `source-file` は
  エラー終了するだけでサーバーを自動起動しない（検証済み）
- 検証は `tako-core::tmux_backend` の e2e テスト（detach → 再 attach の内容復元 /
  OSC 7 パススルー）+ セルフテスト 58〜62（隔離ソケット `TAKO_TMUX_SOCKET`）で機械化済み

### ⚠️ 実機リグレッション（2026-06-12 常用報告）と恒久対策

- **Dock 起動の .app は PATH が最小構成**（/usr/bin:/bin:…）で Homebrew の tmux が
  見えない → tmuxview が空 + バックエンドが沈黙劣化 + 明示コマンド split が PTY 失敗。
  対策: ① `tmux::tmux_bin()`（`TAKO_TMUX_BIN` → PATH → 既知の場所 → ログインシェル
  `command -v` の順で解決・キャッシュ）を全 tmux 呼び出しで使う。
  ② 明示コマンドは `terminal::login_shell_command`（`$SHELL -l -c "…"`）で包んで spawn する
  （ユーザーの PATH・環境で実行。直接 exec しない）
- **Dock 起動の .app はロケール環境変数もゼロ → tmux クライアントが C ロケールになり、
  tmux 3.6 はコマンド出力中の制御文字を `_` にサニタイズする**: `-F "…\t…"` の
  タブ区切り出力が `master-2_1781179563_0` になり、tako 内の**全 tmux パースが沈黙全滅**
  （tmuxview 空表示 + tako 駆動スクロール無反応の共通根本原因。kill 系だけ動くのは
  フォーマット出力を使わないため。シェルから叩くと UTF-8 ロケールで再現しない罠）。
  対策: `tmux::tmux_command()`（`LC_CTYPE=UTF-8` 注入 + `LC_ALL` 除去）を全 tmux
  子プロセスの唯一の入口にする。ペイン側 CJK 対策（LC_CTYPE 既定注入・P0）と同型。
  e2e: `ロケール無し環境でもタブ区切り出力が壊れない`（C ロケールで `_` 化する
  カナリア + 注入後の TAB 保持）
- **マウスレポートの保証**（tako の存在意義。Zed の同症状が自作の動機）:
  「内側アプリがマウスを要求したら必ず生の SGR イベントが届く」「alt-screen 非マウス
  ペインへのホイールが矢印キーに化けない」を core e2e で常時検証する。
  バックエンド `mouse on` で claude（マウス非要求・通常画面）へのホイールは tmux
  copy-mode = チャットが遡れる
- **修飾付きキー（Shift+Enter 等）の CSI u 送出は全ペイン常時有効**
  （`CsiUMode::ModifiedOnly` が既定。Issue #28 で backend 限定 → 全ペインに変更）:
  修飾付き Enter はレガシー形式だと素の `\r` に潰れて区別不能な一方、Claude Code は
  kitty protocol を要求・クエリせずとも CSI u 入力を解釈する（2026-07-02 v2.1.198
  素の PTY で実測）。内側の kitty 要求は tmux から外側端末へ伝わらない（内側が要求
  しても外側 Term の DISAMBIGUATE は立たない）ため「要求を見てから送る」は不可能で、
  常時送出が正。tmux バックエンドペインは extended-keys always + csi-u 形式が内側へ
  届け、直接 spawn ペイン（tmux 無し環境 = Homebrew 配布の既定）はそのまま届く。
  旧実装は backend 限定だったため tmux 無し環境で Shift+Enter 改行が死んでいた。
  CSI u 非対応アプリ（素の zsh 等）では修飾付き Enter が「3;2u」風の文字列になるが、
  backend ペインは 2026-06-12 から同挙動で実害報告なし（受容済みトレードオフ）。
  ただし **Esc 単押しは CSI 27u にせず素の `\e` を送る**: tmux 3.6 は受信した
  CSI 27u を内側ペインの kitty 要求の有無に関係なく素通しする（extended-keys
  on / always どちらでも。実測）ため、CSI u 非対応アプリ（素の zsh、kitty を
  pop 中の claude 等）の入力欄に「27u」が文字として挿入される（2026-06-12
  実機バグ）。素の `\e` は escape-time で正しく解釈され内側へ素のまま届く。
  Esc を CSI 27u で送るのはアプリ自身の kitty 要求を外側 Term が直接見た場合
  （`CsiUMode::Full`）のみ。往復 + 「27u」非漏出は e2e 済み、Shift+Enter の
  GUI 実キー経路はセルフテスト 45b で回帰防止
- **IME 候補・未確定文字列の位置は shaping で出す**: `pane_cursor_origin` を
  col × セル幅の線形換算にすると全角行で打ち進めるほど右へずれる（描画は実フォント幅）。
  `cell_at` の逆写像（`ScreenLine::cell_cols` + `shape_line`）で求める
- **GPUI（taffy）の flex 子は overflow: visible だと自動最小サイズ = min-content**
  （2026-06-13 実機「下部ステータスバーが消える」の根因）: ルート flex 列の中段
  （flex_1）に min-height 制約が無く、サイドバー / パネルの内在コンテンツ高
  （ファイルツリー行数・tmux 一覧の量）がウィンドウ高を超えると中段が縮めず、
  ステータスバーが画面外へ押し出される（コンテンツ量依存なので再現が不定に見える）。
  対策: 中段に `min_h(0)` + タブバー / ステータスバー / パネルヘッダに `flex_none()`。
  **スクロールしない固定バーを flex 列に置くときは必ずこのペアを付ける**。
  Zed 本体が `min_h_0` を多用しているのも同じ理由

### ⚠️ UI スレッド同期処理のパフォーマンス教訓（2026-06-13 実機報告）

- **syntect ハイライトを UI スレッドで同期実行してはいけない**: release でも 200ms+
  （debug で 3s）。`preview::load_fast` で平文を即表示（0.8ms）し、ハイライトは
  `spawn_highlight` で background executor へ（色は後から付く 2 段階 UX）
- **render() 内で stat syscall を呼んではいけない**: `sync_filetree_roots` が各ペインの
  cwd に `is_dir()` を毎フレーム発行していた。OSC 7 由来の cwd は信頼して stat を
  省略し、削除された cwd は 2 秒ごとの `refresh` で回収する
- **定期ポーリング（2 秒タイマー）のファイル I/O は background へ**: `FileTree::refresh`
  の `read_dir_sorted` を main thread で回すと ~9ms/回。`refresh_targets` → background
  `scan_dirs` → main thread `apply_refresh` の 3 段階に分離
- **原則**: UI スレッド上で 1ms 以上かかるファイル I/O や CPU 計算を同期実行しない。
  やむを得ない場合は計測値をコメントに残し、非同期化の TODO を添える

## コンセプト②の実現

技術選定（2026-06-12 ユーザー確認済み。候補比較は選定時のセッションログ）:

- **ファイルツリー**: OSC 7 で得た cwd をルートに表示。更新は当面ポーリング
  （notify クレートの導入は必要性が出たら再判断）
- **コードプレビュー**: シンタックスハイライトは **syntect**（bat / delta / gitui 採用の
  定番。言語セット同梱・導入容易、プレビュー中心 + 軽い編集の要件に十分）。
  ただし**ハイライタは小さな trait（`Highlighter`）で抽象化**し、編集機能が本格化したら
  tree-sitter（Zed と同じ構文木ベース・インクリメンタル）へ差し替え可能な構造にする。
  純 Rust 構成（`regex-fancy` 系 feature）にして oniguruma の C 依存は避ける（Windows CI）
- **Markdown**: **pulldown-cmark**（rustdoc / mdBook 採用のデファクト。イベントストリーム型で
  GPUI の独自描画に写しやすい）でパースし GPUI で描画
- **PDF**: 優先度 C。pdfium バインディング等、Phase 5 で要否ごと再判断
- **軽い編集**: `tako-core::TextBuffer` の UTF-8 `String` + 最小限の編集操作。プレビューの
  編集可能上限が 1MB / 5000 行で、ropey の利点が出る巨大文書は安全上編集不可にするため、
  新規依存 ropey は追加しない。LSP はやらない（Non-goal）
- **git graph**: **git CLI 子プロセス**（VS Code / lazygit と同方式。新規依存ゼロで
  tmux 取得層と同パターン、開発者環境に git は必ずある）で `git log --format` 等を
  パースして取得し GPUI で描画。gitoxide（API 発展途上）/ git2-rs（C 依存）は不採用。
  **2026-06-14 実装完了**: `tako-core::git` モジュール（`git_bin()` 解決 + log/branch/status/diff
  パーサ。ユニットテスト 5 本）。右パネルの git ビュー = ブランチ + 変更ファイル + コミット
  グラフ + diff 表示のアコーディオン。cwd 連動 2 秒ポーリング。dispatch `GitLog`/`GitDiff` +
  CLI `tako git log/diff` + MCP `tako_git_log`/`tako_git_diff`（計 25 ツール）

実装（2026-06-13。FR-3.1 改 / FR-3.2 / FR-3.3 完成）:

- **プレビューはペイン種別**: `tako-app::previews: HashMap<PaneId, PreviewState>`。
  載っているペインは render_pane が早期分岐してファイル内容を描く（PTY なし・
  attach_session を呼ばない）。読み込み・ハイライト・Markdown ブロック化は GPUI 非依存の
  `tako-app/src/preview.rs`（`Highlighter` trait + SyntectHighlighter。差し替え点）
- **操作は dispatch `OpenFile`** に一元化: UI クリック / `tako open` / MCP `tako_open_file`
  が同一経路。表示先解決（自身がプレビュー > 同タブ既存を再利用 > 分割新設）も dispatch 側。
  ControlHost に `preview_state` / `set_preview` / `preview_pane_of_tab` フックを追加
- **ファイルツリーは「タブ = ワークスペース」**: `sync_filetree_roots()` がアクティブタブ内
  全ペインの cwd を集めて `FileTree::set_roots()`（マルチルート。重複除去・既存ルートの
  展開状態維持）へ渡す。プレビューペイン（cwd なし）は自然にスキップされる
- **永続化**: layout.json の `PaneLayout.preview {path, mode}`（serde default で後方互換）。
  復元時は spawn せず `preview::load` で開き直す

実装（2026-07-12。FR-3.5 完成、#126）:

- **ドメインモデル**: `tako-core::text_edit::TextBuffer` に編集操作と保存競合検知を集約。
  UTF-8 バイト境界を不変条件とし、日本語の BS / Delete も 1 Unicode scalar 単位で扱う
- **UI**: `TakoApp::preview_edits` がペイン別バッファを保持し、`preview_render.rs` は描画した
  `StyledText` 自身の `TextLayout` をカーソル・選択へ再利用する（#145 で固定セル幅換算から変更）。
  逆写像は GPUI の raw glyph index と次の UTF-8 境界の実キャレット座標を比較して最近傍を選ぶ。
  ファイル / mode 差し替え時は旧座標キャッシュを即時無効化するため、Markdown の文字サイズ・
  日本語・タブ・スクロール・差し替え後も描画と逆写像が一致する。
  編集時は入力ごとの syntect 全再解析を避けて平文を即描画し、タイトルバーに編集切替・
  dirty・保存結果を表示。IME は既存
  `EntityInputHandler` を編集バッファへ振り分け、PDF / Markdown 読み取り選択は従来経路のまま
- **制御プレーン**: `ControlHost` の編集フックへ dispatch 3 操作を 1:1 で写し、CLI / MCP
  が同じ core バッファを操作する。未保存変更があるプレビューペインのファイル差し替えは拒否
- **PDF 選択（#145）**: PDFKit を明示リンクしてページ文字列・行矩形・文字矩形を抽出し、
  CoreGraphics のページ座標から表示画像座標へ変換する。PDFKit 未ロード時にテキストレイヤが
  無言で空になる回帰を、実文字矩形まで必須アサーションする macOS テストで防ぐ
- **PDF 選択の描画（#152）**: 画像と重ねる絶対配置 canvas は `.top_0().left_0()` を必須とする。
  省略すると GPUI の static position が直前の画像下端になり、誤った矩形同士の往復テストだけが
  通って実マウス座標と描画は外れる。文字矩形収集と選択描画を分離し、選択はペイン最終子の専用
  `paint_layer` で PDF の polychrome sprite より前面へ合成する。`visual-test` feature の Metal
  RGBA 読み戻しで PDF 選択と C++ / Python の読み取り・編集を対象矩形の実ピクセル差分まで検証する
- **syntect の行入力（#152）**: `SyntaxSet::load_defaults_newlines()` へ `str::lines()` の
  改行除去済み文字列を渡さない。`LinesWithEndings` で状態遷移に必要な改行を維持し、UI の行要素へ
  変換するときだけ末尾改行を除く。パス解決は読み取り / 編集で単一の `syntax_for_path` を使う

## Web ビューペイン（FR-3.8）→ ✅ wry ネイティブ統合で実装（2026-07-13、#155）

**GPUI には webview 要素が無い**ため、第一候補だった「ネイティブ webview の重ね合わせ」を
**wry 0.55**（Tauri の webview ライブラリ。Apache-2.0 OR MIT。macOS = WKWebView /
Windows = WebView2）で実装した。CDP ミラー方式 PoC（ヘッドレス Chrome +
スクショポーリング）は座標ずれ・入力中継の品質限界・Chrome 依存のため置き換え。

- **接続**: `gpui::Window` は `raw_window_handle::HasWindowHandle`（macOS では
  AppKitWindowHandle = GPUI の NSView）を実装しており、
  `wry::WebViewBuilder::build_as_child()` にそのまま渡せる。初回 render で
  `WindowHandleBox` に採取し、dispatch（IPC / MCP）からの生成でも使う
  （gpui-component の GPUI × wry 統合と同構成）
- **bounds 追従**: render_webview_pane が pane_text_areas と同じ絶対論理座標を
  `set_bounds`（Logical）へ渡す。差分呼び出しで AppKit 往復を抑制
- **入力**: OS がネイティブ webview へ直接配送（クリック・スクロール・キー・IME）。
  tako 側の中継は不要
- **タブ維持（dock）**: ページ = `WebViewEntry` を PaneId から独立管理。ペインを
  閉じても wry インスタンスが生き、SPA 状態・ログイン・スクロール位置が維持される。
  ステータスバー 🌐 ボタン → dock パネル（flex 列内 = webview と重ならない）から
  ワンクリック復帰。永続化は layout.json（PaneLayout.webview + LayoutFile.webview_dock）
- **可視性同期**: render 末尾で「今フレーム描画されなかった webview」（非アクティブ
  タブ・dock 退避）と D&D 中の全 webview を `set_visible(false)`。隠す際は
  `focus_parent()` でキー入力を GPUI へ返す
- **AI 操作**: dispatch `Request::Web`（action 式 9 操作）+ CLI `tako web` + MCP
  `tako_web`。JS 評価は 2 段階 API（eval → token → eval_result）— dispatch は
  UI スレッドで走り、wry のコールバックも UI スレッド配送のため同期待ちは
  デッドロックする（`webview.rs` の設計コメント参照）

**既知の制約（z オーダー）**: ネイティブビューは GPUI の GPU 合成レイヤの**上**に乗る。
GPUI のオーバーレイ（ピン留め窓 FR-2.16.15・ホバープレビュー・コンテキストメニュー・
注釈オーバーレイ FR-2.6）は webview ペインの上では隠れる。ドロワー・ステータスバー・
サイドバー・dock パネルは flex レイアウト内のため重ならない。D&D 中は全 webview を
隠してドロップターゲットを見せる。将来 FR-2.6 を webview 上でも使う場合は
オフスクリーン合成（CEF/Servo 級の重量）か webview 内 JS オーバーレイが要る。
スクリーンショット系機能（terminal_screen_lines 等）には webview の中身は映らない

## AI 誘導・注釈レイヤ（FR-2.6、後段フェーズ）

ペイン上のハイライト・指し示しは **GPUI の描画だけで完結する見込み**（deferred / overlay 描画、
ネイティブビュー不要）。対象指定は「ペイン ID + グリッド座標（行・列範囲）or 相対矩形」を
MCP / CLI から受け、UI 層がレイアウト矩形に変換して描画する。
入力はオーバーレイを素通しし、ユーザー操作・明示消去・タイムアウトで消す（FR-2.6.3）。
注意: Web ビューペイン上だけはネイティブビューが最前面になるため別方式が要る（上記リスク参照）。

## プラットフォーム抽象（platform/）

| 関心事 | macOS | Windows |
|---|---|---|
| PTY | openpty | ConPTY |
| IPC | Unix domain socket | Named pipe |
| プロセスツリー / listen ポート | libproc | Toolhelp32 / GetExtendedTcpTable |
| シェル統合 | zsh / bash / fish | PowerShell |

trait で抽象化し、core/ と control/ は platform/ の trait のみに依存する。

## セキュリティ方針

- IPC / MCP は localhost のみ + セッション毎のランダムトークン必須（FR-2.3.4）
- `tako send` / `tako_send_input` は任意コマンド実行と等価な力を持つため、
  「トークンを持つ = アプリ内で起動されたプロセスのみ」が防御線
- リモート接続機能は持たない（Non-goal: クラウド機能なし）
