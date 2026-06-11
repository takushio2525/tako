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
4. **ライセンス互換性**: GPUI は Apache-2.0、alacritty_terminal は Apache-2.0 で、
   tako の Apache-2.0 と互換。cmux（GPL-3.0）のコードは絶対に読まない（`concept.md` 参照）。
   **zed リポ内の gpui 以外のクレート（terminal 等）は GPL 系のため同様にコードを読まない**
   （gpui / gpui_platform 等 Apache-2.0 のものだけ参照可）。

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

## 制御プレーン（コンセプト①の 3 層）

### 環境変数注入（共通基盤）

TerminalSession がシェルを spawn する際に注入する:

| 変数 | 内容 |
|---|---|
| `TAKO_PANE_ID` | 呼び出し元ペインの ID |
| `TAKO_TAB_ID` | 所属タブの ID |
| `TAKO_SOCKET` | IPC エンドポイント（macOS: Unix domain socket パス、Windows: named pipe 名） |
| `TAKO_MCP_URL` | 内蔵 MCP サーバーの接続先（Layer2 自動発見用） |
| `TAKO_TOKEN` | 接続認証トークン（セッション毎に生成。外部プロセスの接続拒否に使う） |

### Layer 1: CLI（`tako-cli`）

- 単一バイナリ `tako`。`TAKO_SOCKET` + `TAKO_TOKEN` を読んで IPC サーバーに JSON-RPC で接続
- pane 指定省略時は `TAKO_PANE_ID` を呼び出し元として使う（FR-2.2.7）
- `TAKO_SOCKET` が無ければ「tako の外で実行されている」エラー（FR-2.2.8）
- サブコマンド: `split` / `send` / `focus` / `list` / `read` / `close` / `title` /
  `resize` / `layout`（レイアウト操作の要件カタログは FR-2.5）
- IPC プロトコルは MCP ツールと同じ操作セットに 1:1 対応させ、実装を共有する

### Layer 2: 内蔵 MCP サーバー

- control/ 内で起動し、`TAKO_MCP_URL`（+ `TAKO_TOKEN`）で公開
- トランスポートは Streamable HTTP（localhost バインド）を第一候補（Phase 3 で確定）
- 公開ツール（案）: `tako_split_pane` / `tako_send_input` / `tako_read_pane` /
  `tako_focus_pane` / `tako_list_panes` / `tako_set_title` /
  `tako_close_pane` / `tako_resize_pane` / `tako_apply_layout`（均等化・最大化等のプリセット）/
  `tako_get_pane`（個別ペインの状態取得）
- ツールセットは **FR-2.5（AI レイアウト操作セット）**を網羅する。
  《AI での開発がやりやすく、かつ AI が開発をアシストできる》が設計方針:
  エージェントが自分の作業ペインを片付けたり、成果物提示のためにレイアウトを
  整えたりできるよう、読み取り（ツリー構造 + ジオメトリ）と操作を対で公開する。
  各操作のセマンティクスは tako-core の PaneTree API と 1:1 対応させる
- 設計原則 5「AI フルコントロール」は**開発不変条件**（`requirements.md`）: すべての機能は
  追加した時点で MCP / CLI から操作可能でなければならず、**UI でできる操作はすべてツール化**する。
  そのため新機能の操作ロジックは必ず tako-core の操作 API として実装し、UI と MCP / CLI が
  同じ API を呼ぶ（コマンドディスパッチの一元化）。
  後段フェーズの追加ツール（案）: `tako_create_tab` / `tako_move_pane`（タブ振り分け、FR-2.5.10）/
  `tako_open_file`（プレビュー表示、FR-2.5.11）/ `tako_open_url`（Web ビュー、FR-2.5.12）/
  `tako_annotate`（注釈オーバーレイ、FR-2.6）
- 接続トークンから呼び出し元ペインを特定し、**操作スコープのデフォルトを同タブ内に制限**（FR-2.3.3）
- エージェント側の自動発見方式（mcp.json の自動生成 or 各エージェント CLI の規約への追従）は
  Phase 3 で詰める。Claude Code を最初のリファレンス対象とする

### Layer 3: パッシブ検知

- **OSC 7**（cwd 通知）→ ファイルツリーの cwd 連動（コンセプト②でも使う）
- **OSC 133**（プロンプトマーク）→ コマンド単位の区切り・実行中/完了の把握
- シェル統合スクリプトは zsh / bash / fish / PowerShell を同梱し、可能な範囲で自動注入
- **listen ポート検知**: ペイン配下のプロセスツリーを監視し、新規 listen ポートを検知
  - macOS: libproc（`proc_pidinfo` 系）。Windows: `GetExtendedTcpTable`
  - ポーリング方式。間隔は数秒オーダーで負荷を抑える
- 検知結果は**提案チップ**（「localhost:5173 をプレビューで開く？」）として UI に出すだけ。
  承諾時のみペイン生成（強制分割はしない）。設定で全体を無効化可能（FR-2.4.4）

## コンセプト②の実現

- **ファイルツリー**: OSC 7 で得た cwd をルートに表示。ファイル監視は notify クレート
- **コードプレビュー**: シンタックスハイライトは tree-sitter または syntect（Phase 5 で選定）
- **Markdown**: pulldown-cmark でパースし GPUI で描画
- **PDF**: 優先度 C。pdfium バインディング等、Phase 5 で要否ごと再判断
- **軽い編集**: ロープ構造（ropey）+ 最小限の編集操作。LSP はやらない（Non-goal）
- **git graph**: gitoxide または git2-rs でコミットグラフを取得し GPUI で描画

## Web ビューペイン（FR-3.8、後段フェーズ）

**GPUI には webview 要素が無い**ため、実現には GPUI の外の仕組みが要る。
候補と技術リスク（採否は後段フェーズの検証スパイクで判断、`roadmap.md`）:

1. **ネイティブ webview の重ね合わせ（第一候補）**:
   macOS は WKWebView を NSView として GPUI ウィンドウへ addSubview、
   Windows は WebView2 を子 HWND として配置し、ペイン矩形に位置・サイズを追従させる。
   ランタイム同梱不要で軽い（NFR-3/7 と整合）。
   リスク: GPUI の GPU 合成レイヤとネイティブビューの z オーダー協調
   （GPUI 描画はネイティブビューの**下**にしか出せない＝注釈オーバーレイ FR-2.6 と干渉）、
   タブ切替・分割変更時の表示/非表示とクリッピング、フォーカス・IME・キーボードショートカットの
   ルーティング分断、スクリーンショット系機能に映らない
2. **オフスクリーンレンダリング**: CEF / Servo 系でテクスチャ化して GPUI 内で描画。
   z オーダー問題は消えるが、依存が巨大でメモリ・配布サイズが NFR-3/7 に反する。保留
3. **外部ブラウザ起動（フォールバック）**: ペイン統合を諦め `open <url>` する。
   Phase 5 以前でも `tako_open_url` の暫定実装として先行提供できる

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
