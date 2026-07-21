# tako パブリック公開前セキュリティ監査レポート

> 監査日: 2026-06-22
> 対象: tako リポジトリ（main ブランチ、186 コミット）
> 目的: GitHub private → public 化前の包括監査

---

## Critical（即修正必須）

### C-1. ライセンス変更不可 — Apache-2.0 への切り替えは法的に不可能

**状況**: AGENTS.md / Cargo.toml / LICENSE すべてが `GPL-3.0-or-later` を宣言しており、
これは法的に正しい。Apache-2.0 への変更は**依存ツリーの制約で不可能**。

**原因**: GPUI（Zed 製）のトランジティブ依存に以下の GPL-3.0-or-later クレートが含まれる:

| クレート | ライセンス | 出所 |
|---|---|---|
| `zlog 0.1.0` | GPL-3.0-or-later | Zed monorepo |
| `ztracing 0.1.0` | GPL-3.0-or-later | Zed monorepo |
| `ztracing_macro 0.1.0` | GPL-3.0-or-later | Zed monorepo |

Rust のスタティックリンクにより、最終バイナリは GPL-3.0-or-later での配布が**法的に義務**。

**推奨対応**:
- Apache-2.0 への切り替え計画は撤回し、**GPL-3.0-or-later のまま公開**する
- 将来 Zed/GPUI が当該クレートのライセンスを変更した場合のみ再検討可能
- Cargo.toml / AGENTS.md の `GPL-3.0-or-later` 記載はそのまま維持（正しい）

---

## Warning（公開前に対応推奨）

### W-1. .gitignore が最小限すぎる — コントリビューター向け防御不足

**現状（4行のみ）**:
```
/target
/dist
.DS_Store
.claude/
```

**不足パターン**（現在ファイルは存在しないが、コントリビューターが持ち込む可能性）:

| パターン | 理由 |
|---|---|
| `.env*` | 環境変数ファイル誤コミット防止 |
| `*.key` / `*.pem` / `*.p12` | 鍵・証明書 |
| `.vscode/` / `.idea/` | IDE 設定 |
| `*.swp` / `*.swo` / `*~` | エディタ一時ファイル |
| `*.log` | ログファイル |

**推奨対応**: 上記パターンを `.gitignore` に追加

### W-2. Zed クレート 2 つのライセンスメタデータ欠落

| クレート | 問題 |
|---|---|
| `gpui_shared_string 0.1.0` | Cargo.toml に `license` フィールドなし |
| `gpui_util 0.1.0` | 同上 |

Zed monorepo のコンテキストから Apache-2.0 or GPL-3.0 と推定されるが、明示されていない。
tako 側で対応できる問題ではないが、**Zed 側に Issue / PR を出す**ことで改善可能。

**推奨対応**: 認識しておく。必要なら Zed リポへ報告

### W-3. FileOp::Trash の AppleScript エスケープが不完全

**箇所**: `crates/tako-control/src/dispatch.rs:1007-1014`

```rust
let escaped = path_str.replace('\\', "\\\\").replace('"', "\\\"");
format!("tell application \"Finder\" to delete (POSIX file \"{}\" as alias)", escaped)
```

MCP / CLI 経由で任意のパスが渡される。`"` と `\` のエスケープはあるが、
NUL バイトや改行文字を含むパスでの挙動が未検証。実害リスクは低い
（ファイルシステムが NUL を拒否する）が、**AppleScript インジェクションの余地**がある。

**推奨対応**: 入力パスの改行・制御文字をバリデーション or 拒否する

### W-4. `unsafe impl Send for VideoPlayer` が脆弱

**箇所**: `crates/tako-app/src/video_player.rs:115`

AVPlayer/AVFoundation は main-thread-only API が多い。現在は GPUI のメインスレッド
コールバック内でのみ使用されているが、`Send` impl があるため、将来 background thread
から呼ばれると UB（未定義動作）になる。

**推奨対応**: コメントで制約を明記し、可能なら `Send` impl を除去してアーキテクチャで保証

---

## Info（参考・対応任意）

### I-1. シークレット漏洩 — 問題なし ✅

- ソースコード内: ハードコードされた API キー・トークン・パスワードなし
- git 履歴内: `ANTHROPIC_API_KEY` / `sk-ant-` / `ghp_` 等のパターンなし
- `.env` / `.key` / `.pem` ファイル: 存在しない（git 履歴含む）
- SSH 鍵 / 証明書: なし
- テスト内のトークン: `"test-token"` / `"secret"` / `"bogus-token"` はダミー値のみ
- `TAKO_TOKEN` は実行時に CSPRNG で生成。ハードコード値なし

### I-2. 個人情報 — 問題なし ✅

- メールアドレス: ソースコード内になし（git コミットの `<email>` のみ）
- ハードコードパス: `/Users/<user>` はソース内ゼロ。テストは `/Users/foo` 等のダミー
- 実名: ソース・ドキュメント内になし
- GitHub ユーザー名 `takushio2525`: Cargo.toml / README / build-app.sh に存在するが、
  公開リポのオーナーとして意図的

### I-3. git コミットメール

git 履歴のすべてのコミットで `<email>`（git author メール）が author。
公開後にこのメールアドレスが見える。GitHub の「noreply」アドレスに切り替える場合は
公開前に `git filter-branch` / `git filter-repo` での書き換えが必要
（186 コミットなので現実的な作業量）。

**推奨対応**: 意図的であれば問題なし。プライベートにしたければ公開前に書き換え

### I-4. TODO / FIXME コメント — 3 件のみ、問題なし

すべて `Phase 6: Windows named pipe TODO` 等の技術的参照。内部事情や不適切な内容なし。

### I-5. unsafe ブロック — 約 50 箇所、概ね妥当

| ファイル | 箇所数 | 用途 | 評価 |
|---|---|---|---|
| `ports.rs` | 7 | macOS libproc FFI | ✅ バッファ・戻り値チェック適切 |
| `terminal.rs` | 1 | `ioctl(TIOCPTYGNAME)` | ✅ 固定バッファ |
| `osc_tap.rs` | 1 | mio trait impl | ✅ API 要件 |
| `preview.rs` | 3 | CoreGraphics PDF | ⚠️ RAII でないリソース管理 |
| `video_player.rs` | 30+ | AVFoundation ObjC | ⚠️ Send impl（W-4 参照）|

### I-6. ネットワーク入力バリデーション — 適切 ✅

- MCP HTTP サーバー: `127.0.0.1:0` にバインド（外部非公開）
- Origin 検証 + Bearer トークン認証（DNS リバインディング対策）
- IPC: Unix ソケット `0o600` パーミッション + トークン認証
- トークン: `getrandom`（CSPRNG）32 バイト hex

### I-7. コマンド実行 — 概ね安全 ✅

tmux / git / ffmpeg / claude 等の外部コマンドは `Command::new().arg()` 経由で
引数を渡しており、シェル展開を経由しない。W-3 の AppleScript のみ要注意。

### I-8. パストラバーサル — 設計上許容

FileOp（Rename/CreateFile/CreateDir/Trash）は任意の絶対パスを受け付けるが、
ターミナルアプリとしてファイルシステム全体を操作できるのは正常な設計。
`name` パラメータ（新規ファイル名）の `/` `\` 拒否バリデーションは正しく実装されている。

### I-9. TOCTOU — 低リスク

`FileOp::Rename/Create` で `new_path.exists()` チェック → 操作のパターンあり。
ローカルアプリ内操作で実害は軽微。

### I-10. .agent/ ディレクトリの公開

progress.md / progress-archive.md に AI の作業ログが含まれる。
秘密情報はないが、開発プロセスの内部記録。OSS として公開するなら問題ないが、
意図的かどうかの確認を推奨。

### I-11. poc/ ディレクトリ

Phase 0 の検証コード 3 件がトラック済み。AGENTS.md で「品質基準の対象外」と明記。
公開して問題はないが、コントリビューターが混乱しないよう README に注記あり。

### I-12. 依存関係のセキュリティ — `cargo audit` 実行済み ✅

**既知の脆弱性（vulnerability）: ゼロ**（732 クレートをスキャン）

警告のみ 6 件（すべて GPUI のトランジティブ依存。tako 側で対処不可）:

| クレート | 種別 | 内容 |
|---|---|---|
| `async-std 1.13.2` | unmaintained | 開発終了（RUSTSEC-2025-0052） |
| `bincode 1.3.3` | unmaintained | メンテ終了（RUSTSEC-2025-0141） |
| `instant 0.1.13` | unmaintained | メンテ終了（RUSTSEC-2024-0384） |
| `paste 1.0.15` | unmaintained | メンテ終了（RUSTSEC-2024-0436） |
| `proc-macro-error2 2.0.1` | unmaintained | メンテ終了（RUSTSEC-2026-0173） |
| `swash 0.2.8` | yanked | crates.io から削除済み |

いずれもセキュリティ脆弱性ではなくメンテナンス状態の警告。GPUI が依存を更新すれば解消する。

### I-13. MPL-2.0 依存（3 クレート）

`cbindgen` / `dwrote` / `option-ext` が MPL-2.0。ファイルレベルの copyleft であり
GPL-3.0 と互換。tako がこれらのソースを改変しない限り問題なし。

### I-14. NOTICE ファイル

現在なし。GPL-3.0 では必須ではないが、サードパーティの attribution として
設置するのが good practice。

---

## サマリ

| 区分 | 件数 | 主な内容 |
|---|---|---|
| **Critical** | 1 | Apache-2.0 への変更不可（GPL 依存） |
| **Warning** | 4 | .gitignore 強化 / AppleScript エスケープ / unsafe Send / Zed ライセンスメタ |
| **Info** | 14 | シークレットなし / 個人情報なし / unsafe 妥当 / ネットワーク認証適切 |

**結論**: シークレット漏洩・個人情報漏洩の Critical 問題はなく、GPL-3.0-or-later のまま
公開する分にはセキュリティ上の大きな懸念はない。Warning 4 件は公開前の対応を推奨する。
