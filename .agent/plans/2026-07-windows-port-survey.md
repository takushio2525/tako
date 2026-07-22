# Windows 対応 実現可能性調査（Issue #467）

- 調査日: 2026-07-22
- 対象 commit: `0a02fcd`（main）
- 調査方法: リポジトリ静的調査（rg / Cargo.lock / gh api。**cargo build は実行していない** —
  並行 worker 稼働中のため）+ Web 一次情報（出典 URL を各所に併記）
- コード変更: なし（本レポートのみ）

## エグゼクティブサマリ

**結論: Windows 対応は実現可能。ただし「ビルドが通る」と「使い物になる」の間に大きな段差があり、
段差の正体は GPUI ではなく tmux である。**

- GPUI のリスクは 2026 年時点でほぼ消滅した。Zed Windows 版は 2025-10-15 に正式リリース済みで、
  Zed 1.0（2026-04-29）以降も毎週更新されている。描画は DirectX 11 + DirectWrite
- コードベースは Phase 6 を見据えた規律が既に効いている: macOS 専用機能 15 ファイルの大半に
  `#[cfg(not(target_os = "macos"))]` スタブが併設され（25 箇所）、IPC / CLI transport は
  Windows スタブ（`Unsupported` エラー）まで書かれている。CI にも Windows ジョブが存在する
  （ただし **CI は 2026-06-12 から手動停止中** = 直近 40 日分の変更は Windows でコンパイル未検証）
- 最大の障壁は tmux。persist（完全復元）・orchestrator（spawn / 送達確認 / watch / report /
  worker レジストリ）・scroll ミラー・pane_log が tmux コマンドに直接依存しており、
  tmux は Windows ネイティブに存在しない。ここの方針決定（WSL2 / 独自永続層 / 抽象化）が
  工数の支配項
- 見積もり合計: **楽観 43 / 悲観 106 worker-日**（P0〜P4。詳細は §5）。
  最初の一手（P0: CI 再開 + Windows コンパイル成立）は 1〜3 worker-日で、即 Issue 化できる

---

## 1. プラットフォーム依存の全数調査

### 1.1 集計（再現コマンド付き）

実行日 2026-07-22、対象 `crates/`（`poc/` は品質基準対象外のため除外）:

| 調査 | コマンド | 件数 |
|---|---|---|
| A | `rg -c 'cfg\(target_os = "macos"\)' --type rust crates` | **88 箇所 / 15 ファイル** |
| B | `rg -c 'cfg\(unix\)' --type rust crates` | **57 箇所 / 17 ファイル** |
| C | `rg -c 'cfg\(windows\)' --type rust crates` | **5 箇所**（すべて既存スタブ） |
| D | `rg -c 'cfg\(not\(target_os = "macos"\)\)' --type rust crates` | **25 箇所**（非 macOS スタブ） |
| E | `rg -o 'Command::new\("([^"]+)"\)' -r '$1' crates --type rust`（リテラル呼び出しのみ） | open 16 / tmux 13 / osascript 6 / brew 5 / ps 2+2 / defaults 2 / ditto 2 / pmset 1 / sw_vers 1 / cmd 1 / xdg-open 1 ほか |

注: tmux / git / claude は `tmux_bin()` / `git_bin()` 等の変数経由呼び出しが主のため、
E のリテラル集計は下限値。tmux への実依存は §2 参照（`rg -c 'tmux'` で
main.rs 351 / dispatch.rs 161 / remote.rs 155 / tmux_backend.rs 139 ヒット）。

### 1.2 macOS 専用 API・FFI（ファイル:行つき）

| 分類 | 箇所 | 内容 | Windows での対応 |
|---|---|---|---|
| 動画再生 | `crates/tako-app/src/video_player.rs:11-24`（`#[link]` AVFoundation / CoreMedia / CoreVideo / CoreFoundation、cfg 36 箇所 = 最多） | AVPlayer FFI で動画プレビュー | Media Foundation で書き直し or ffmpeg 経由 or 当面スタブ（`:604-638` に非 macOS スタブ既設） |
| PDF プレビュー | `crates/tako-app/src/preview.rs:745`（CoreGraphics）`:821`（ImageIO）`:1039`（PDFKit `#[link]` + objc_msgSend）、`crates/tako-core/src/pdf_links.rs:3`・`preview_outline.rs:22`（PDFKit 由来データ） | PDF ラスタライズ・テキスト層・リンク・目次 | Windows に OS 標準 PDF フレームワーク相当なし → pdfium-render 等の採用判断 or 当面スタブ（`:525,543` に非 macOS スタブ既設） |
| Web ビュー | `crates/tako-app/src/webview.rs:467-470`（objc_msgSend で NSEvent キーモニタ）、wry 0.55（workspace） | WKWebView 子ビュー統合 | wry は Windows = WebView2 対応（§3）。キーモニタは Win32 フック等で代替（`:644` に非 macOS スタブ既設） |
| listen ポート検知 | `crates/tako-core/src/ports.rs:4,254,278,310`（libproc `proc_pidinfo` / `proc_pidfdinfo`、cfg 10 箇所） | ペイン配下プロセスの listen 検知 | `GetExtendedTcpTable` + Toolhelp32（`.agent/architecture.md:297` に Phase 6 方針記載済み） |
| スリープ防止 | `crates/tako-control/src/sleep_guard.rs:364-420`（IOKit FFI: IOPSGetTimeRemainingEstimate / IORegistry AppleClamshellState）`:479`（pmset）`:498-501`（sudoers NOPASSWD 登録）、cfg 13 箇所 | 電源アサーション・蓋閉じ制御 | `SetThreadExecutionState` で大幅に簡素化可能（蓋閉じ・sudoers 相当は不要） |
| FDA ガイド | `crates/tako-control/src/fda.rs:20,43,59` | macOS TCC の Full Disk Access 誘導 | Windows に TCC なし → 機能ごと N/A（`:16,77` に非 macOS スタブ既設） |
| ゴミ箱・Finder | `crates/tako-control/src/dispatch.rs:1691`（`open -R`）`:6246-`（osascript AppleScript で Trash）、`crates/tako-app/src/sidebar.rs:1438`（osascript）`:1450`（open） | ファイル操作 UI | `SHFileOperation` / trash クレート、Explorer 起動 |
| URL / ファイルを開く | `crates/tako-app/src/main.rs:568-575` ほか open 16 箇所 | 既定アプリで開く | `main.rs:569-573` に **`cmd /C start` の Windows 分岐が既にある**。他 15 箇所は open 直書きで要共通化 |
| ロケール検出 | `crates/tako-core/src/i18n.rs:106,132-147`（`defaults read AppleLanguages`） | GUI 起動時の言語判定 | `GetUserDefaultLocaleName` / 環境変数 |
| 自動更新 | `crates/tako-app/src/update_checker.rs:261-286,707-733`（/Applications・Caskroom 判定・`open -n` 再起動） | 配布系統判別 + zip 差し替え | Windows 配布設計とセット（§5 P4）。winget / scoop / インストーラ判別に書き直し |
| データディレクトリ | `crates/tako-core/src/paths.rs:38-41` | **Windows は `None` を返す** = layout.json / settings.json / token / persist.log の置き場が無い | `%APPDATA%\tako` 追加（数行。ただし全永続系の前提なので P1 必須） |
| ロック | `crates/tako-control/src/config_io.rs:17,217`（flock） | 設定ファイルのプロセス間排他 | `LockFileEx`（fs4 クレート等）へ差し替え |
| プロセス祖先辿り | `crates/tako-control/src/agents.rs:81`（`ps`）、`crates/tako-control/src/sleep_guard.rs:829` | エージェント⇔ペイン対応付け | Toolhelp32Snapshot |
| デフォルトシェル | `crates/tako-core/src/terminal.rs:88-91`（**Windows は `None`** = ペイン spawn 不能）`:115-118`（login_shell_command はパススルー） | シェル起動 | PowerShell / `%COMSPEC%` 既定化（P1 必須） |

### 1.3 IPC / ソケット（cfg(unix) の主要部）

- `crates/tako-control/src/ipc.rs:51-64` — サーバーは UnixListener。`#[cfg(windows)]` は
  「named pipe は Phase 6」の明示 `Unsupported` スタブ
- `crates/tako-cli/src/main.rs:5249-5320` — CLI transport も同様（unix = UnixStream、windows = スタブ）
- `crates/tako-control/src/discovery.rs:145-148,165-169` — `socket_alive`（UnixStream 接続試行）と
  0o700 パーミッション（`PermissionsExt`）
- `crates/tako-control/src/remote.rs` — cfg(unix) 18 箇所 / `std::os::unix` 13 参照で最多。
  tailscale serve → **UDS** プロキシ（`:876-897`）+ X-Forwarded-For を `tailscale whois` 照合
- `std::os::unix` の import 分布: remote.rs 13 / discovery.rs 4 / ipc.rs 3 / text_edit.rs 2 /
  ports.rs, lib.rs, dispatch.rs, tako-cli, tako-app 各 1

### 1.4 シェル統合・スクリプト・CI

- シェル統合は zsh / bash / fish のみ同梱（`crates/tako-core/shell-integration/zshenv.zsh`,
  `tako.bash`, `tako.fish`）。Windows の主シェル PowerShell 用の OSC 7 / 133 統合は存在しない
  （architecture.md:687 の抽象表にも「Windows = PowerShell」と方針だけ記載）
- `scripts/build-app.sh:2,34` — 「macOS 専用」明記（iconutil / sips / codesign / ditto 依存）。
  `release.sh` / `nightly-release.sh`（launchd 前提）も同様に macOS 専用。
  Windows 配布・自動リリースはゼロから別系統が必要
- CI（`.github/workflows/ci.yml`）には **windows-latest ジョブが既にあり**（Spectre-mitigated libs
  の VS インストーラ追加 → build + test）、最後に走った 2026-06-12 時点では全ジョブ成功。
  ただし `gh workflow list` の実測で **`disabled_manually`** 状態。
  つまり 2026-06-12 以降の全変更（#155 webview / video player / PDF 拡充 / remote 刷新 /
  設定画面ほか約 40 日分）は Windows コンパイル未検証

---

## 2. アーキテクチャ上の重依存

### 2.1 tmux — 最大の障壁

依存の実態（`rg -c 'tmux' crates --type rust` 上位: tako-app/main.rs 351、
tako-control/dispatch.rs 161、remote.rs 155、tako-core/tmux_backend.rs 139、tmux.rs 76）:

| サブシステム | tmux への依存内容 | tmux 不在時の現行挙動 |
|---|---|---|
| persist（FR-5 / Phase 5.5） | 全 PTY を tmux session 化し再起動で画面ごと完全復元（`tmux_backend.rs`） | **無害に劣化する設計が既にある**: #30 で「tmux 不在 = 構造のみ永続化（保存 cwd の新シェル復元）」を実装済み |
| orchestrator spawn | `new-session -e` で TAKO_PANE_ID 直接注入（progress.md 2026-06-26）、worker レジストリは tmux_session キー（#390） | tmux 前提。直接 PTY ペインへの spawn 自体は可能だが追跡系が全滅 |
| プロンプト送達確認 | `claude_tui.rs:429 deliver_via_tmux`（capture-pane で画面採取 → send-keys で Enter 再送） | tmux 経由の検証付き送達が使えず、素の PTY write のみ |
| worker 監視・報告 | watch / worker_status（画面採取）、report 第 1 層 = `capture-pane -p -J -S`（#364）、pane_log のバックエンド採取 | 取得不能（直接ペインは alacritty history から部分代替可） |
| スクロール | バックエンドペインは capture ベースのローカル履歴ミラー（#159、`scroll_mirror.rs`） | 直接ペイン経路（display_offset 方式）は tmux 非依存で動く |
| remote | ペイン画面の scrollback / screen API が tmux ターゲット解決前提の箇所多数（remote.rs 155 ヒット） | 直接ペインのフォールバックはあるが機能縮退 |

tmux の Windows 事情（Web 裏取り）: tmux は Windows ネイティブに存在しない。
MSYS2 に 3.6a パッケージはある（2025-12 ビルド）がソケット・システムコール挙動に差異があり、
公式推奨は WSL2。ConPTY 直叩きの互換実装（psmux、tmux-windows）も出現しているが若い。
出典: [MSYS2 packages](https://packages.msys2.org/packages/tmux?variant=x86_64) /
[tmux.app Windows ガイド](https://tmux.app/install/windows/) /
[psmux 紹介](https://zenn.dev/sora_biz/articles/psmux-windows-native-tmux?locale=en) /
[tmux-windows](https://github.com/bitcode/tmux-windows)

**選択肢比較**:

| 案 | 内容 | 工数感 | 機能制限 | 保守コスト |
|---|---|---|---|---|
| A: WSL2 必須 | tmux は WSL2 内で走らせ、tako（Win ネイティブ）から `wsl tmux ...` を叩く | 小〜中（10〜20 worker-日。パス変換・socket 橋渡し・PATH 解決の泥仕事） | WSL2 導入が前提 = 「ゼロコンフィグで一般ユーザー」の設計原則に反する。Windows ネイティブシェル（PowerShell）のペインは persist 対象外になる歪み | 中（WSL 相互運用の挙動変化に追従） |
| B: ConPTY ネイティブ + 独自永続層 | tmux の使用箇所（session 化・capture・send-keys・new-session -e）を自前のセッションホストプロセス + スクロールバック保持で置換 | 大（30〜60 worker-日。デタッチ可能なセッションホスト = ミニ tmux の自作） | 完全復元・orchestrator をネイティブで実現可能。ただし成熟まで長い | 大（自前永続層のバグは全プラットフォームの信頼性問題に直結） |
| C: バックエンド抽象化 + 段階導入（推奨） | `tmux_backend` 相当を trait 化し、Windows は当面「バックエンドなし」= 構造のみ永続化（#30 の既存劣化経路をそのまま正式仕様化）。orchestrator は「送達確認なし・レジストリは PID ベース」の縮退モードを定義。将来 B を差し込む | 中（15〜30 worker-日） | 初期リリースは「再起動で画面内容は消える（構成は戻る）」「worker 監視が粗い」 | 小〜中（抽象境界が明確になり macOS 側の見通しも改善） |

推奨は C。根拠: #30 で「tmux 不在 = 構造のみ永続化」の劣化経路が既に実装・検証済みであり
（Homebrew 配布先 = tmux 無し環境はすでに本番実績がある）、Windows 初期リリースを
この既存経路に乗せるのが最小リスク。A はゼロコンフィグ原則違反、B は初期投資が過大。

### 2.2 Unix ドメインソケット（tako.sock / remote UDS）

- OS レベル: Windows 10 1803（build 17063）以降 AF_UNIX 対応
- Rust std レベル: `std::os::unix::net` は Windows ではコンパイル不可。Windows 向けの
  `std::os::windows::net::UnixListener` は **nightly の実験的 API**（feature
  `windows_unix_domain_sockets`、[rust-lang/rust#147335](https://github.com/rust-lang/rust/pull/147335)、
  [nightly doc](https://doc.rust-lang.org/nightly/std/os/windows/net/struct.UnixListener.html)）。
  stable では uds_windows 等のクレートか named pipe
- **認証上の注意（#287 P1-2 に直結）**: Windows の AF_UNIX には `SO_PEERCRED` /
  `getpeereid` 相当のピア資格情報取得がない。UDS 化で得ようとしている「接続元プロセスの
  同一ユーザー検証」は、Windows では named pipe（`GetNamedPipeClientProcessId` +
  トークン照合）の方が素直に実現できる。**IPC は named pipe を第一候補**とし、
  architecture.md:212 の既定方針（`\\.\pipe\tako-<pid>`）を維持するのが妥当
- remote の tailscale serve は UDS をプロキシターゲットにしている（remote.rs:876-897）。
  tailscale serve の unix ターゲットは対応していても root 限定などの制約があり
  （[tailscale/tailscale#9771](https://github.com/tailscale/tailscale/issues/9771)、
  [serve CLI doc](https://tailscale.com/docs/reference/tailscale-cli/serve)）、
  Windows では **loopback TCP ターゲット（`127.0.0.1:port`）への切り替え**が現実解。
  この場合、UDS で担保していた「同一マシン・同一ユーザーのみ接続可能」が弱まるため、
  トークン認証の再強化とセットで設計する

---

## 3. GPUI・主要依存クレートの Windows 対応状況

### 3.1 GPUI（最重要）

- tako の取得元: zed リポ git rev 固定
  `cafbf4b5df7fedb67fc0f248850a5654efcec5d9`（Cargo.toml workspace / Cargo.lock:2675。
  gpui 0.2.2 相当）。この rev のコミット日は **2026-06-10**（gh api 実測）= 約 6 週間前
- Zed 本家の Windows 状況（一次情報）:
  - 正式リリース 2025-10-15。DirectX 11 + DirectWrite、専任 Windows チーム + 毎週更新
    （[Zed for Windows is here](https://zed.dev/blog/zed-for-windows-is-here)）
  - Zed 1.0 が 2026-04-29 に出て以降も stable 継続（現行 1.8.2 = 2026-06-24。
    [Stable Releases](https://zed.dev/releases/stable)、[Wikipedia](https://en.wikipedia.org/wiki/Zed_(text_editor))）
  - オープン issue `platform:windows` ラベル: **116 件**（2026-07-22 に
    `gh api search/issues` で実測。うち "terminal" を含むもの 47 件）。
    2026-06 調査時の約 120 件から横ばい = 枯れてはいないが発散もしていない
- ビルド前提は CI に実装済み: MSVC + **Spectre-mitigated libs**（ci.yml:44-62 で
  VS インストーラ追加済み）+ rust 1.95.0（rust-toolchain.toml）
- 残る実機確認事項: ターミナル TUI フォント描画（zed #58830 系）、日本語 IME、
  120Hz+ ディスプレイ。これは §6 リスク 1・3

### 3.2 その他の主要依存

| クレート | バージョン（Cargo.lock） | Windows 対応 | 出典 |
|---|---|---|---|
| alacritty_terminal | 0.26.0（Cargo.lock:141-144） | ConPTY 対応済み（Windows 10 1809+）。`tty::Pty` がプラットフォーム差を吸収 | [alacritty リポ](https://github.com/alacritty/alacritty)（tty/windows モジュール）、[architecture.md:12 の Phase 0 判断と一致] |
| wry | 0.55.1（Cargo.lock:8497-8499） | Windows = WebView2（Edge Chromium）。Windows 7〜11 対応、webview2-com 依存。WebView2 ランタイムは Win11 標準搭載 | [docs.rs/wry 0.55.1](https://docs.rs/wry/0.55.1/wry/) |
| syntect | regex-fancy 構成 | oniguruma C 依存を **意図的に回避済み**（Cargo.toml コメント「Windows CI」明記） | Cargo.toml workspace.dependencies |
| notify | 8.2 | Windows = ReadDirectoryChangesW（Cargo.toml コメントに明記） | Cargo.toml workspace.dependencies |
| tiny_http / tungstenite / ureq / clap / serde 系 | — | いずれも pure Rust でクロスプラットフォーム | crates.io 一般 |
| エージェント CLI（claude） | 外部依存 | **ネイティブ Windows 対応済み**（2025〜。PowerShell `install.ps1`、WSL 不要） | [ITECS ガイド](https://itecsonline.com/post/how-to-install-claude-code-on-windows) ほか複数の 2026 年ガイド |

---

## 4. リモート（PWA）・MCP・CLI の可搬性評価

- **MCP**: 内蔵 MCP は tiny_http の Streamable HTTP（tokio 非依存）で、HTTP 部分は
  そのまま Windows で動く。architecture.md:231 にも「named pipe 未実装の間は HTTP 側が
  受け皿」と設計済み。stdio ブリッジ（`tako mcp serve`）は CLI transport（→ named pipe 化）
  が通れば動く。`claude mcp add` 相当の登録フロー（setup-mcp）はパス表記
  （`~/.claude/settings.json`）の Windows 化のみ
- **CLI**: 通信層は ipc.rs / tako-cli の transport スタブ差し替え（§2.2）に集約されており、
  コマンド群のロジック自体はほぼ可搬。ただし出力・案内文の unix パス前提と、
  `TAKO_SOCKET` 環境変数の意味（pipe 名へ）を揃える必要がある
- **リモート（PWA）**: PWA 本体（`web/tako-remote/`、npm ビルド）はブラウザ側なので可搬。
  サーバー側（remote.rs）は ① UDS → loopback TCP 化（§2.2）② tmux ターゲット解決に
  依存する screen / scrollback API の縮退（§2.1）③ tailscale CLI 自体は Windows 版が存在、
  の 3 点の作り替えで成立する。難度は中
- **セットアップ／依存チェック**: `tako setup` の brew 案内（setup.rs:435）・FDA ステップは
  Windows で winget 案内・N/A 化が必要。シェル統合の自動注入は PowerShell profile 対応が新規

---

## 5. 段階的ポーティング計画と見積もり

前提: 見積もりは worker-日（1 worker が 1 日で消化する作業量。楽観/悲観の幅付き）。
**Windows 実機または VM が P1 以降は必須**（P0 のみ CI で完結可能）。
CI は現在 disabled_manually のため、P0 の前提として「CI の再有効化」または
「Windows VM / 実機の用意」のどちらかをユーザーが決める必要がある。

| Phase | 内容 | Exit Criteria | 楽観 | 悲観 | 前提条件 |
|---|---|---|---|---|---|
| **P0: コンパイル成立の回復** | CI Windows ジョブ再有効化 → 直近 40 日分の変更で入った Windows ビルドエラーを cfg スタブで潰す（video_player / preview PDF / webview は非 macOS スタブが既設なので、漏れの補修が中心）。`paths.rs` の `%APPDATA%\tako` 追加もここで | CI の windows ジョブ（build + test）が緑 | **1** | **3** | CI 再有効化（Actions 課金・分数の確認）。実機不要 |
| **P1: 最小 GUI 起動** | `default_shell` の PowerShell/ConPTY 既定化（terminal.rs:88）、キーバインドの cmd→ctrl マッピング、フォントフォールバック、GPUI ウィンドウ起動確認、日本語 IME の初期動作確認 | Windows 実機で tako が起動し、PowerShell ペインで文字が打てて日本語が入力できる | 4 | 10 | Windows 11 実機 or VM（ARM Mac の場合 Parallels + Windows 11 ARM。x64 バイナリ検証は別途） |
| **P2: ターミナル + ペイン + CLI/MCP** | IPC named pipe 実装（ipc.rs / tako-cli transport / discovery の pipe 対応）、PowerShell シェル統合（OSC 7/133）、ports の GetExtendedTcpTable、config_io の LockFileEx、agents.rs の Toolhelp32、リンク検出の Windows パス対応、セルフテストの Windows 完走 | 分割・タブ・CLI 全コマンド・MCP 接続（claude ネイティブ Windows 版）がセルフテストで緑 | 8 | 18 | P1 完了。claude CLI Windows 版での e2e |
| **P3: orchestrator + persist（縮退モード）** | §2.1 案 C: バックエンド trait 抽象化、Windows = 構造のみ永続化（#30 経路の正式化）、orchestrator の非 tmux 縮退（送達確認なし・PID ベースレジストリ・alacritty history ベースの report）、UI の機能可否表示 | Windows で `tako master` → worker spawn → 完遂 → 報告取得の一連が動く（画面完全復元なしは許容） | 15 | 40 | P2 完了。方針 C の設計レビュー（tmux 完全代替 = 案 B はこの Phase に含めない） |
| **P4: remote + 配布 + 自動更新 + プレビュー同等** | remote の loopback TCP + tailscale Windows 対応、PWA e2e。配布: winget / scoop + zip、コード署名（EV 証明書 or Azure Trusted Signing の調達判断）、update_checker の Windows 系統。プレビュー: WebView2（wry）、PDF（pdfium 採用判断）、動画（当面スタブ可） | Windows ユーザーに「使ってみて」と言える品質（roadmap Phase 6 の Exit Criteria） | 15 | 35 | P3 完了。署名手段の調達（外部リードタイムあり） |
| **合計** | | | **43** | **106** | |

備考:
- P0 は他 Phase と独立に即着手でき、着手すれば「40 日分の未検証」という現在進行形の
  劣化を止められる（Issue 化推奨。受け入れ条件 = CI windows ジョブ緑 + 以後 PR で必須化）
- P1〜P2 は並行可能な部分が多い（IPC と シェル統合は独立）
- 動画プレビュー（AVFoundation 36 箇所）と PDF（PDFKit）は「Windows では当面未対応」を
  明示する選択肢があり、その場合 P4 は楽観側に寄る（-5〜10 worker-日）
- CI 停止が続く場合の代替検証手段: ローカル Windows VM での `cargo build/test` +
  `TAKO_SELF_TEST=1` 起動（セルフテストは GUI 実画面で機械検証する設計のため VM で有効）

---

## 6. リスク・未知数トップ 5

| # | リスク | 影響 | 調査で潰せるか / 実装しないと分からないか |
|---|---|---|---|
| 1 | **GPUI Windows のターミナル用途での描画品質**（TUI フォント描画・リガチャ・高リフレッシュレート。zed の platform:windows 116 件の一部） | tako の主用途がターミナルなので、エディタでは許容される描画問題が致命傷になり得る | **実装しないと分からない**（P1 の実機起動で最初に判明する。だからこそ P1 を早く・小さく回す） |
| 2 | **tmux 代替の永続化アーキテクチャ**（§2.1） | 工数の支配項。案 C でも trait 境界の切り方を誤ると macOS 側の安定性（#30/#113/#177/#381 で固めた復元系）を壊す | 設計は**調査・レビューで潰せる**（P3 着手前に設計 md を切る）。縮退モードの使用感は実装しないと分からない |
| 3 | **日本語 IME**（GPUI Windows の TSF 対応品質） | 日本語ユーザーが主対象。macOS では EntityInputHandler で作り込んだ領域（FR-1.9） | **実装しないと分からない**（P1 で最小確認、P2 で手動チェックリスト化。Zed 側は 2025 年末に CJK 集中修正済みという状況証拠あり = architecture.md:68） |
| 4 | **IPC の認証設計**（Windows AF_UNIX に SO_PEERCRED 相当なし。#287 P1-2 の UDS 化方針と衝突） | セキュリティ要件の再設計が必要。named pipe なら `GetNamedPipeClientProcessId` で代替可 | **調査で潰せる**（P2 着手前に #287 側と方針をすり合わせる。本レポートで論点は特定済み） |
| 5 | **wry `build_as_child` + GPUI の子 HWND 統合**（webview.rs は macOS の NSView 前提で書かれており、Windows の HWND 親子付け + イベントルーティングは未検証） | Web ビューペイン（FR-3.8）が Windows で成立しない可能性。座標・フォーカス・キーモニタの作り直し | 半々: wry の Windows child view 対応は**ドキュメント調査で確認可能**だが、GPUI ウィンドウとの統合品質（#155 で macOS でも苦労した領域）は**実装しないと分からない** |

次点: コード署名の調達リードタイム（EV 証明書 / Azure Trusted Signing。技術でなく手続き）、
CI の Windows ランナー費用（Spectre libs インストールで毎回 +数分）。

---

## 付録: 再現コマンド全文

```sh
# A〜D: cfg 全数（2026-07-22 実測、対象 crates/）
rg -c 'cfg\(target_os = "macos"\)' --type rust crates   # 計 88（15 ファイル）
rg -c 'cfg\(unix\)' --type rust crates                   # 計 57
rg -c 'cfg\(windows\)' --type rust crates                # 計 5
rg -c 'cfg\(not\(target_os = "macos"\)\)' --type rust crates  # 計 25
# E: 外部コマンド（リテラルのみ）
rg -o 'Command::new\("([^"]+)"\)' -r '$1' crates --type rust | sort | uniq -c | sort -rn
# tmux 依存の分布
rg -c 'tmux' crates --type rust | sort -t: -k2 -rn
# 依存バージョン
rg -n 'name = "gpui"' -A3 Cargo.lock    # 0.2.2 / zed rev cafbf4b5
rg -n 'name = "alacritty_terminal"' -A3 Cargo.lock  # 0.26.0
rg -n 'name = "wry"' -A2 Cargo.lock     # 0.55.1
# CI 状態
gh workflow list --all                   # CI disabled_manually
gh run list --limit 5                    # 最終実行 2026-06-12（全 success）
# zed の Windows issue 件数
gh api -X GET search/issues -f q='repo:zed-industries/zed is:issue is:open label:platform:windows' --jq '.total_count'  # 116
# gpui rev の日付
gh api repos/zed-industries/zed/commits/cafbf4b5df7fedb67fc0f248850a5654efcec5d9 --jq '.commit.committer.date'  # 2026-06-10
```

## 出典一覧（Web）

- Zed for Windows 正式リリース（2025-10-15、DirectX 11 + DirectWrite）: https://zed.dev/blog/zed-for-windows-is-here
- Zed Stable Releases（1.0 = 2026-04-29、現行系列）: https://zed.dev/releases/stable / https://en.wikipedia.org/wiki/Zed_(text_editor)
- wry 0.55.1 プラットフォーム（Windows = WebView2）: https://docs.rs/wry/0.55.1/wry/
- Rust Windows AF_UNIX（nightly 実験的）: https://github.com/rust-lang/rust/pull/147335 / https://doc.rust-lang.org/nightly/std/os/windows/net/struct.UnixListener.html
- tailscale serve の unix ターゲット制約: https://github.com/tailscale/tailscale/issues/9771 / https://tailscale.com/docs/reference/tailscale-cli/serve
- tmux の Windows 事情: https://packages.msys2.org/packages/tmux?variant=x86_64 / https://tmux.app/install/windows/ / https://github.com/bitcode/tmux-windows / https://zenn.dev/sora_biz/articles/psmux-windows-native-tmux?locale=en
- Claude Code ネイティブ Windows 対応: https://itecsonline.com/post/how-to-install-claude-code-on-windows
- alacritty の ConPTY: https://github.com/alacritty/alacritty/issues/4794（ConPTY 実装の議論）
