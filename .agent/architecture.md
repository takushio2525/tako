# architecture.md — 技術設計

> 「どう実現するか」を定義する。要件は `requirements.md`、実装順序は `roadmap.md`。

## 技術スタック

| 領域 | 採用 | 根拠 |
|---|---|---|
| 言語 | Rust | ネイティブ性能、メモリ安全、Warp / Zed / Alacritty の実績 |
| UI フレームワーク | **GPUI**（Zed 製） | GPU 描画で Zed 級の速度を出せる唯一級の Rust UI。Zed 本体が実証 |
| ターミナルエミュレーション | **alacritty_terminal** | 枯れた VT 実装。Zed のターミナルも同クレートを採用しており GPUI との組み合わせ実績がある |
| PTY | portable-pty 等（Phase 0 で選定） | macOS / Windows（ConPTY）の差を吸収 |
| 非同期 | GPUI の executor を基本、必要なら tokio（Phase 0 で判断） | |

### ⚠️ 採用リスク（明記事項）

1. **GPUI は pre-1.0 であり、破壊的変更が頻発する。**
   Zed 本体の都合で API が変わる前提で付き合う。対策:
   - GPUI への依存を `ui/` レイヤに閉じ込め、コアロジック（ペインツリー・制御プレーン）は GPUI 非依存に保つ
   - バージョンを Cargo.lock で固定し、追従は意識的なタスクとして行う（自動更新しない）
2. **GPUI の Windows 対応は進行中（未完成）。**
   Windows 対応は tako の必須要件なので、これが最大の技術リスク。
   → **Phase 0 を「GPUI の Windows ビルド検証スパイク + 最小ターミナル描画 PoC」とし、
   ここで成立しなければスタック再検討に戻る**（`roadmap.md` 参照）。
3. **ライセンス互換性**: GPUI は Apache-2.0、alacritty_terminal は Apache-2.0 / MIT で、
   tako の Apache-2.0 と互換。cmux（GPL-3.0）のコードは絶対に読まない（`concept.md` 参照）。

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
            └── PreviewPane (Code | Markdown | Pdf | Editor)
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
- サブコマンド: `split` / `send` / `focus` / `list` / `read` / `close` / `title`
- IPC プロトコルは MCP ツールと同じ操作セットに 1:1 対応させ、実装を共有する

### Layer 2: 内蔵 MCP サーバー

- control/ 内で起動し、`TAKO_MCP_URL`（+ `TAKO_TOKEN`）で公開
- トランスポートは Streamable HTTP（localhost バインド）を第一候補（Phase 3 で確定）
- 公開ツール（案）: `tako_split_pane` / `tako_send_input` / `tako_read_pane` /
  `tako_focus_pane` / `tako_list_panes` / `tako_set_title`
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
