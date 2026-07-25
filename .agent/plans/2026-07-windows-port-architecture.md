# Windows ポーティング アーキテクチャ設計（Issue #467）

- 作成日: 2026-07-25
- 前提資料: `.agent/plans/2026-07-windows-port-survey.md`（実現可能性調査。全数調査と Phase 見積もり）
- 位置づけ: **実装 Issue を切る前に「移植の構造」を確定するための設計**。
  以後の Windows 対応タスクはすべてこの文書の抽象境界に沿って実装する

## 0. この設計が解こうとしている問題

調査レポートは「何が Windows で動かないか」を洗い出した。本設計はその次の問い
「**どう作れば、直した先が腐らないか**」に答える。狙いは 3 つ。

1. `cfg(target_os)` が全ファイルに散らばる未来を避ける（現状すでに 88 箇所 / 15 ファイル）
2. mac で先行開発し、安定した差分を Windows へ一括反映する開発モデルを、
   人間の記憶ではなく**テストとコマンドで**支える
3. tako の不変条件「UI でできることはすべて AI からもできる（MCP / CLI 1:1）」を
   **プラットフォーム不変条件**へ格上げする

---

## 1. 設計原則

### 原則 1: プラットフォーム分岐は抽象境界の内側だけ

`cfg` を書いてよいのは次の 2 箇所に限る。

- `platform/` 配下のプラットフォーム別実装モジュール
- 抽象の**実装選択**を行う 1 行（`pub use macos::Impl as Platform;` のような差し替え点）

呼び出し側（`dispatch` / UI / CLI / orchestrator）は**単一のコードパス**を持つ。
「呼び出し側に `#[cfg]` を足して塞ぐ」変更は原則違反であり、レビューで差し戻す。

### 原則 2: 操作面は単一表面。使えない機能は「無い」のではなく「明示的な縮退」

`Request` / CLI コマンド / MCP ツールの集合はプラットフォーム間で**同一**にする。
Windows で未対応の操作も、ツールとしては存在し、呼ぶと理由つきの構造化エラーを返す。

- 悪い例: Windows ビルドでは `tako_sleep_guard` ツールが MCP 一覧から消える
  → AI は「そんな機能は無い」と誤認し、代替行動も取れない
- 良い例: ツールは存在し、`Pending { note: "…", issue: 4xx }` を返す
  → AI は「今は使えない・理由・追跡先」を理解して回避できる

### 原則 3: 乖離はテストで落とす（人間の記憶に頼らない）

プラットフォーム対応状況を**機械可読のマトリクス**で持ち、新機能が未分類なら
テストが失敗する構造にする（§3）。

### 原則 4: mac 先行開発を止めない

P0 完了以降、`cargo check --target x86_64-pc-windows-msvc` は **macOS 上で常時緑**を保つ。
一括反映時の差分が「純粋な機能実装」だけになり、ビルド修復作業が混ざらないようにする（§5）。

---

## 2. 抽象境界カタログ

各境界は「呼び出し側が知ってよい唯一の型」と「その裏のプラットフォーム実装」の対で定義する。
`所在` 列は現在プラットフォーム依存コードが散在している場所（= 集約元）。

| # | 境界 | 抽象（trait / API） | 現在の所在（集約元） | macOS 実装 | Windows 実装 |
|---|---|---|---|---|---|
| B1 | シェル解決 | `platform::shell::default_shell() / login_shell_command()` **［P0 で新設済み］** | `tako-core/src/terminal.rs:88-118` | `$SHELL -l` | pwsh 7 → Windows PowerShell → `%ComSpec%` → `cmd.exe` |
| B2 | 永続化バックエンド | `trait SessionBackend`（create / attach / detach / capture / send_keys / list / kill / resize） | `tako-core/src/tmux_backend.rs`、`tmux.rs`、`scroll_mirror.rs`、`pane_log.rs` + 呼び出し側 350+ 箇所 | `TmuxBackend` | `NullBackend`（構造のみ永続化）→ 将来 ConPTY ホスト |
| B3 | 制御 IPC トランスポート | `trait ControlTransport`（listen / accept / connect）+ `PeerIdentity` | `tako-control/src/ipc.rs:51-64`、`tako-cli/src/main.rs:5249-5320`、`discovery.rs:145-169` | Unix domain socket + `SO_PEERCRED` | named pipe + `GetNamedPipeClientProcessId` |
| B4 | リモート ローカルエンドポイント | `platform::local_endpoint`（`bind` / `probe_alive` / `request_raw` / `path_byte_limit`）**［P0 で新設済み］** | `tako-control/src/remote.rs:943, 956, 1395, 1559, 1754` | UDS（現行のまま） | loopback TCP + トークン |
| B5 | プロセス検査・制御 | 検査: `platform::procinfo`（`all_pids` / `parent_of` / `name_of` / `descendants_of` / `listening_ports_of_pid`）／制御: `platform::process::terminate` **［制御側は P0 で新設済み］** | `tako-core/src/ports.rs`、`tako-control/src/agents.rs:81`、`sleep_guard.rs:829`、`remote.rs`（signal 送出） | libproc / `kill(2)` | Toolhelp32 + `GetExtendedTcpTable` / `TerminateProcess` |
| B6 | データ配置 | `paths::data_dir()` | `tako-core/src/paths.rs` | `~/Library/Application Support/tako` | `%APPDATA%\tako` |
| B7 | ファイルロック | `platform::flock::lock_exclusive()` | `tako-control/src/config_io.rs:17,217` | `flock` | `LockFileEx` |
| B8 | OS 連携（開く・ゴミ箱） | `platform::os_integration`（`reveal` / `open_default` / `open_with` **［P0 で新設済み］**、`open_url` / `move_to_trash` は未着手） | `dispatch.rs:1691,6246-`、`sidebar.rs:1438,1450`、`main.rs:568-575` ほか `open` 16 / `osascript` 6 箇所 | `open` / AppleScript | `ShellExecuteW` / `SHFileOperation` |
| B9 | スリープ防止 | `trait SleepGuardBackend`（`apply` / `status`） | `tako-control/src/sleep_guard.rs:364-501` | IOKit + pmset | `SetThreadExecutionState`（蓋閉じ・sudoers は macOS 固有 capability） |
| B10 | ロケール検出 | `platform::locale::system_languages()` | `tako-core/src/i18n.rs:106,132-147` | `defaults read AppleLanguages` | `GetUserDefaultLocaleName` |
| B11 | Web ビュー ホスト | `trait WebviewHost`（attach / detach / resize / key monitor） | `tako-app/src/webview.rs:467-470` | WKWebView 子ビュー | WebView2 子 HWND |
| B12 | ドキュメントレンダラ | `trait PdfRenderer` / `trait VideoPlayer` | `tako-app/src/preview.rs:745,821,1039`、`video_player.rs:11-24`、`tako-core/src/pdf_links.rs`、`preview_outline.rs` | PDFKit / CoreGraphics / AVFoundation | pdfium 等 or `Unsupported` 明示 |
| B13 | シェル統合 | `shell_integration::script_for(shell)` + 注入先解決 | `tako-core/shell-integration/{zshenv.zsh,tako.bash,tako.fish}` | 既存 3 種 | PowerShell profile |
| B14 | 配布・自動更新 | `trait UpdateChannel`（detect / download / apply / restart） | `tako-app/src/update_checker.rs:261-286,707-733` | .app / Caskroom | winget / scoop / zip |
| B15 | プライバシー許可ガイド | `trait PermissionGuide` | `tako-control/src/fda.rs` | TCC / FDA | `Unsupported`（Windows に TCC 相当なし） |

### 2.1 モジュール配置の規約

```
crates/tako-core/src/platform/
├── mod.rs          ← 抽象の再エクスポートと実装選択（cfg はここだけ）
├── support.rs      ← サポートマトリクス（§3）
├── macos/          ← macOS 実装
├── windows/        ← Windows 実装
└── unix/           ← macOS 以外の unix と共有する部分
```

`tako-control` / `tako-app` 固有の境界（B3・B4・B9・B11・B12・B14・B15）は
各クレート内に同じ形の `platform/` を置く。**新規に `cfg` を書きたくなったら、
まずその境界が既にあるかを探し、無ければ境界を作るところから始める。**

### 2.2 境界の粒度についての判断

- **alacritty_terminal の PTY 層に境界は作らない**。`tty::Pty` が既に unix pty / ConPTY を
  吸収しており、二重の抽象になる。tako 側で必要なのは B1（シェル解決）だけ
- **B2 は最大の境界であり、単独で設計 Issue を切る**。tmux 依存は 7 サブシステム
  （persist / spawn / 送達確認 / watch / report / scroll ミラー / pane_log）に及び、
  trait の切り方を誤ると macOS 側の復元系（#30 / #113 / #177 / #381 で固めた不変条件）を壊す
- **B12 は「Windows 未対応」を正式な選択肢として許す**。`Unsupported` はマトリクスに
  現れるので、隠れた劣化にはならない

---

## 3. サポートマトリクス（機械可読）とパリティテスト

### 3.1 データ構造

`crates/tako-core/src/platform/support.rs`:

```rust
pub enum Support {
    /// 完全に動く
    Supported,
    /// 動くが機能が落ちる。note は UI / エラーメッセージにそのまま出る
    Degraded { note: &'static str },
    /// 未実装。追跡 Issue 必須
    Pending { note: &'static str, issue: u32 },
    /// そのプラットフォームには概念が存在しない（例: Windows の FDA）
    Unsupported { note: &'static str },
}

pub struct Feature {
    /// MCP ツール名と 1:1 の安定キー
    pub key: &'static str,
    pub macos: Support,
    pub windows: Support,
}

pub const MATRIX: &[Feature] = &[ /* 全機能 */ ];

/// 純粋関数。**macOS 上でも Windows 側の縮退表を検証できる**ことが重要
pub fn support_for(platform: Platform, key: &str) -> Option<&'static Support>;
```

### 3.2 ドリフトを落とすテスト

キー体系は **MCP ツール名を正**とする。tako の開発不変条件「新機能は必ず MCP / CLI から
操作できる」により、**新機能は必ず MCP ツールを増やす**。したがって MCP ツール表を
マトリクスと突き合わせれば、未分類は必ず検出できる。

| テスト | 内容 | 落ちる状況 |
|---|---|---|
| T1 被覆 | `testdata/mcp_tools_snapshot.txt` の全ツール名が `MATRIX` に存在する | **新機能を足してマトリクスに分類し忘れた** |
| T2 逆被覆 | `MATRIX` の全キーが snapshot に存在する | 機能を消したのにマトリクスに残っている |
| T3 CLI 表 | clap の built command から再帰列挙した全サブコマンドが `MATRIX` に存在する | CLI だけ足して MCP を足していない（1:1 違反も同時に検出） |
| T4 説明必須 | `Degraded` / `Pending` / `Unsupported` の `note` が空でない。`Pending` は `issue != 0` | 理由も追跡先も無い縮退 |
| T5 診断一致 | `Pending` / `Unsupported` の操作を dispatch した際のエラー文字列が、マトリクスの `note` 由来であること | メッセージの二重管理（マトリクスと実装がズレる） |
| T6 プロンプト単一ソース | `crates/**/resources/` と `resources/` 配下に `*-windows.*` / `*-macos.*` のような**プラットフォーム別複製ファイルが存在しない** | prompt / 配布物を OS ごとにコピーした |

T1〜T4・T6 は macOS 上で実行でき、Windows 実機を必要としない。

### 3.3 実行時の縮退

`dispatch` の入口で `support_for(current_platform(), req.feature_key())` を引き、
`Pending` / `Unsupported` なら統一エラー `DispatchError::Unsupported { key, note, issue }` を返す。
**エラーメッセージはマトリクスの `note` をそのまま使う**（二重管理を作らない）。

さらに `Request::feature_key(&self) -> &'static str` を**網羅 match** で実装する。
新しい `Request` バリアントを足すと match が非網羅になり**コンパイルエラー**になるため、
テストより早い段階でキー付けを強制できる。

### 3.4 マトリクスを実行時に見せる

原則 2 に従い、マトリクス自身も操作面に載せる。

- CLI: `tako platform [--platform windows] [--status pending]`
- MCP: `tako_platform`

これにより AI（master）が「この環境で今使える機能」を自己認識でき、
Windows 上の master が縮退を踏んでも自力で回避経路を選べる。

---

## 4. 単一ソース化（system prompt・設定・配布物）

### 4.1 禁止事項

`master-system-windows.md` のような**プラットフォーム別ファイルの複製を作らない**（T6 で機械検出）。
複製は必ずドリフトする。

### 4.2 方式: 単一正本 + 条件付き断片の注入

対象の正本:

- `crates/tako-control/src/orchestrator/default_system_prompt.md`（master）
- `crates/tako-control/src/orchestrator/solo_system_prompt.md`（solo）
- `resources/setup/system-prompt.md`（setup アシスタント）
- `resources/setup/changes.yaml`（setup 追従。revision 連番）

レンダリング時に `PlatformFacts` を注入する。

```rust
pub struct PlatformFacts {
    pub os_label: &'static str,       // "macOS" / "Windows"
    pub shell_label: &'static str,    // "zsh" / "PowerShell"
    pub data_dir_example: String,     // 実際の data_dir
    pub degraded: Vec<&'static str>,  // マトリクスの Degraded / Pending の note
}
```

正本には**プレースホルダを 1 種類だけ**置く（`{{platform_notes}}`）。
`degraded` はマトリクスから自動生成するため、prompt 側の記述更新は不要。

`changes.yaml` の各 revision には任意の `platforms:` フィールドを設ける
（省略 = 全プラットフォーム）。未知の値はパースで弾く。

### 4.3 コマンド例の扱い

AGENTS.md の「最も簡単なコマンドを提案する」原則（#322）は維持する。
パス例のみ `PlatformFacts.data_dir_example` から差し込み、コマンド名や引数は分岐させない。

---

## 5. mac 先行開発 → Windows 一括反映のワークフロー

### 5.1 mac 上で Windows ビルドを腐らせない

**実測（2026-07-25、本 worktree）: macOS から全 4 クレートの Windows クロス check が成立する。**

必要なもの:

- `rustup target add x86_64-pc-windows-msvc`
- `cargo install cargo-xwin`（MSVC CRT + Windows SDK を自動取得）
- `brew install llvm`（`clang-cl` / `lld-link` / `llvm-lib` / `llvm-rc`）

gpui の build script は Windows マニフェストを `llvm-rc` で埋め込むが、
`.rc` 内の相対パスが `OUT_DIR` 基準で解決されるため、そのままでは
`resources/windows/gpui.manifest.xml` が見つからず失敗する。
**`INCLUDE` 環境変数に gpui のクレートディレクトリを渡すと解決する**（実測）。

この手順は **`scripts/check-windows.sh` に固定済み**（P0 で作成）。前提ツールが未導入なら
不足コマンドを列挙して終了コード 2 で抜けるため、CI 不使用の方針と衝突しない。
PWA の `dist` が無ければ自動でビルドする（`rust_embed` の埋め込み先）。

P0 完了後は、これを**通常の品質ゲートに加える**（fmt / clippy / test と並べる）。

**ゲートの強さ**: エラーのみをゲートする。現時点で警告が 13 件残るが、その全部が
「macOS 専用実装に対する `dead_code`」であり、各境界の Windows 実装が入るにつれて自然に消える。
ここに `allow` を撒くと原則 1（分岐は境界の内側だけ）に反するため、
**警告ゼロ化は各境界の実装完了時に達成する**。警告件数が 13 から増えたときは、
新しい macOS 専用コードが境界の外に漏れたサインとして扱う。

### 5.2 クロス check で担保できること・できないこと

| 担保できる | 担保できない（Windows 実機が必要） |
|---|---|
| 型検査・cfg の破れ・API の存在 | リンク（シンボル解決・import lib） |
| 新機能を足したときの Windows 側コンパイル崩れ | 実行時挙動（GUI 描画・ConPTY・IME） |
| マトリクスのパリティテスト（純粋関数として検証） | Windows API 呼び出しの正当性 |

### 5.3 「Windows 反映パス」定型手順

一括反映のたびに次を上から実行する。**この手順以外に何を見るべきかを考えなくてよい**状態を保つ。

1. `tako platform --platform windows --status pending` で作業リストを得る
   （= マトリクスの `Pending` 一覧。これが反映すべき差分そのもの）
2. 各項目について、§2 のカタログから担当する抽象境界を特定する
3. **境界の Windows 実装だけを書く**。呼び出し側は触らない
   （触る必要が出たら、それは境界の切り方が間違っているサイン）
4. マトリクスを `Supported` / `Degraded` へ更新する
5. `scripts/check-windows.sh` が緑
6. `cargo test --workspace`（パリティテスト T1〜T4・T6 を含む）が緑
7. Windows 実機で `cargo build --workspace` + `TAKO_SELF_TEST=1` 完走

---

## 6. Issue ツリーとの対応

各実装 Issue は「**どの境界を作る / 使うか**」を必ず明記する。
「とりあえず cfg で塞ぐ」型のタスクは作らない。

到達順序はユーザー決定（2026-07-25）に従う。
**プレ版 v0（素のターミナル + タブ / ペイン管理）を最初のマイルストーン**とし、
以降のプレ版シリーズは **永続 → git タブ → プレビュー** の順で移植する。

| # | Issue | 主担当の境界 | 実機要否 | 依存 |
|---|---|---|---|---|
| #514 | P0: コンパイル成立 + クロス check ゲート化 | B1・B4・B5 制御・B6・B8（いずれも境界の新設） | 最終確認のみ | — |
| #515 | 基盤: 抽象基盤とサポートマトリクス | `platform/` 骨格・§3 全体 | **不要** | #514 |
| #516 | 基盤: system prompt / setup の単一ソース化 | §4 | **不要** | #515 |
| #517 | **プレ版 v0: 素のターミナル + タブ / ペイン管理** | B1 | 必要 | #514 |
| #518 | プレ版 v1a: [設計] バックグラウンド永続の抽象化 | B2（設計のみ） | **不要** | #515 |
| #519 | プレ版 v1b: バックグラウンド永続の実装 | B2（実装） | 必要 | #518・#517 |
| #520 | プレ版 v2: git タブ | 既存境界の利用 | 必要 | #517 |
| #521 | プレ版 v3: プレビューと Web ビュー | B11・B12 | 必要 | #517 |
| #522 | ターミナル日常品質と OS 連携の集約 | B8 | 必要 | #517 |
| #523 | 制御 IPC の named pipe | B3 | 必要 | #514 |
| #524 | OS API 群 | B5 検査・B7・B9・B10・B15 | 必要 | #515 |
| #525 | シェル統合（PowerShell）と setup | B13・§4 | 必要 | #523・#516 |
| #526 | orchestrator の Windows 縮退モード | B2（orchestrator 側） | 必要 | #519・#523・#525 |
| #513 | 設定の git 共有（mac ⇔ Windows） | B6（`%APPDATA%` と対） | 必要 | #514・#515 |
| #527 | self-hosting 検証 | — | 必要 | #526・#513 |
| #528 | remote / 配布 / 自動更新 | B4（Windows 実装）・B14 | 必要 | #519 |

**#513（設定の git 共有）の位置づけ**: `%APPDATA%` のパス設計（B6）と対になる。
共有してよい宣言的設定・秘匿情報・マシンローカル状態の 3 分類が要件であり、
分類はサポートマトリクスと同じ「機械可読 + テストで担保」の考え方を適用できる。
self-hosting（#527）では mac で書いた設定を Windows へ持ち込むため、実質の前提になる。

---

## 7. P0 時点の実測記録（2026-07-25）

`cargo xwin check --workspace --target x86_64-pc-windows-msvc`（macOS / worktree `feat/467-win-p0`）:

- **エラーは `crates/tako-control/src/remote.rs` の 5 件のみ**に集約されていた
  （`std::os::unix::net::UnixStream` 4 箇所 = L943 / L1395 / L1559 / L1754、
  `tiny_http::Server::http_unix` 1 箇所 = L956）。これが境界 B4 を新設する根拠
- 警告 5 件: `fda.rs:8` 未使用 import、`dispatch.rs:1769/1873/1906` 到達不能式、
  `remote.rs:1456` 到達不能文。いずれも既存の非 macOS スタブが `return` の後に
  共通コードを残している形
- `tako-core` は `libc::getuid`（`tmux_backend.rs`）と `paths.rs` の Windows `None` が初期の障壁
- gpui / wry / alacritty_terminal / ring はいずれも Windows ターゲットでコンパイル可能だった
  （調査レポートの「依存クレートは対応済み」という結論を実測で裏付け）

これは調査レポートの P0 見積もり（1〜3 worker-日）と整合する。
**Windows 移植の障壁は依存クレートでもコンパイルでもなく、§2 の B2（tmux）と実行時挙動にある**
という調査の結論が、実測でより強く裏づけられた。

### 7.1 P0 で実際に行った修正

原則 1 に従い、**呼び出し側に `cfg` を足す修正は 1 件も行っていない**。
すべて境界の新設か、既存の境界（`paths` / `fda`）の内側への閉じ込めで解いた。

| 修正 | 境界 | 内容 |
|---|---|---|
| `terminal.rs` の既定シェル解決 | B1 新設 | `platform::shell` へ集約。Windows は `None`（= ペイン spawn 不能）から pwsh 7 → Windows PowerShell → `%ComSpec%` → `cmd.exe` の解決へ。**解決ロジックは純粋関数にしてあり macOS 上で 4 本のテストが回る** |
| `remote.rs` の UDS 直叩き 5 箇所 | B4 新設 | `platform::local_endpoint` へ集約。呼び出し側から `std::os::unix` が消えた |
| `remote.rs` の signal 送出 | B5（制御側）新設 | `platform::process::terminate`。`#[cfg(not(unix))] return Err` の到達不能コードも解消 |
| `dispatch.rs` の Finder / 既定アプリ / アプリ指定 3 箇所 | B8 新設 | `platform::os_integration` へ集約 |
| `paths.rs` の Windows `None` | B6（既存） | `%APPDATA%\tako` → `%USERPROFILE%\AppData\Roaming\tako` の順で解決 |
| `tmux_backend.rs` の `libc::getuid` 2 箇所 | B2 の内側 | `socket_dir()` に抽出。tmux ソケット配置は将来 `TmuxBackend` impl の一部になる |
| `ports.rs` の非 macOS スタブ | B5（検査側）の差し込み口 | Windows 実装を入れる位置として残し、意図を明記 |
| `fda.rs` の import | B15（このモジュール自体が境界） | macOS 限定に閉じた |

**挙動差の申告**: `admin_request` の失敗時メッセージが、接続 / 送信 / 受信の 3 種から
「daemon との通信に失敗 (path): 詳細」の 1 種に統一された。正常系に影響はなく、
この文字列に依存するテスト・ドキュメントは無いことを確認済み。
