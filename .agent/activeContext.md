# Active Context

> このファイルは AI が毎ターン上書きする現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。

## 現在の対象（2026-09-03 未明 = 9/1〜9/3 の 7 PR 一括着地・GUI 反映済み）

- **9/1〜9/3 に merge 済み**（詳細は progress.md）: #1057 setup 導入実行代行（PR #1064）/
  #1058 Win GUI モード導線（#1065）/ #1041 リモート VSCode 風（#1066）/ #1060 PII（#1061）/
  #1063 は計測の錯覚 = PerMonitorV2 不変条件化（#1071）/ #1067 セッション再起動 2 種（#1074）/
  **リモート刷新 柱 1 土台 = Remote Control opt-in + session URL 公開**（#1068/#1069 = PR #1070）。
  main = `d6596ab`
- **/Applications = `d6596ab` 世代・本番 GUI は 9/3 00:33 再起動済み（pid 51132）= 全反映済み**。
  ユーザー目視待ち: ツリー git 色（#1009）/ Finder D&D（#1043）/ リモートフォルダ新挙動（#1041 の
  ツリー先頭 + 自動 SSH）/ ペイン右クリックの再起動 2 項目（#1067）
- **本番 remote は停止中（復旧作業の途中）**: 8/31 未明から GUI 版 Tailscale が稼働して standalone が
  停止 → ユーザー選択は standalone へ戻す。brew tailscaled は **root 起動が必要**
  （`sudo brew services start tailscale`）で、root 化により state が変わり**再ログインが必要**。
  ペイン 1498 で `tailscale up` → `tako remote start` の sudo + ブラウザログイン待ち。
  完了すれば従来 URL（standalone ノード）で復旧する
- **リモート大刷新はエピック #1059**（調査レポート = research/2026-09-01-remote-renewal-claude-official.md。
  分割 A〜H 中 A/B が着地済み。次 = 柱1-C「PWA に Claude で開く」/ 柱1-D「スマホから master 起動」）
- **リポに PR 必須ルールが入った**: docs 含め main 直 push しない（PR 経由）
- **検収の status 読みは `/Applications/tako.app/Contents/MacOS/tako` で叩く**（PATH 先頭の
  `~/dev/tako/target` の stale ビルドだと新フィールドがキーごと無い = #432 と同じ罠を 8/29 に実演）

## #1049 / remote を触るときの不変条件

- serve の読み書きは**公開 URL のノードへ解決した handle**（`tailscale::resolve_serve_handle`）を
  通す（既定探索のままだと二重 tailscaled 環境で**別ノードを読む / 本物を消し残す**）。
  `--socket` 名指しでも**毎周期ノード名を照合**（ログインし直しで改名される）
- **応答している相手からは奪い返さない**（生きた別 daemon と :443 を奪い合うと両方が上限まで暴れる）。
  張り直しは上限 5 回 / 連続 10 回健全で予算回復
- **daemon 内では起動情報 JSON のあと `println!` / `eprintln!` 禁止**（`spawn_daemon` が pipe を
  破棄するので EPIPE panic でスレッドが黙って死ぬ = 実測）。記録は `audit_serve` / health ファイルへ。
  番犬 = `remote_daemon_output_watchdog`
- whois は系統をまたいでも解決する（同一 tailnet の netmap）= 認証経路は入れ替わりの影響なし。
  検証は `bash scripts/test-serve-watch.sh`（偽 tailscale + 隔離 state・本番不可侵・37 件）

## A/B の env（同一バイナリで旧挙動へ戻せる・最近の分）

- `TAKO_1002_LEGACY` / `TAKO_1010_LEGACY` / `TAKO_1011_LEGACY`（+ `TAKO_1011_INJECT_LEDGER_GAP`）/
  `TAKO_1023_LEGACY` / `TAKO_1038_LEGACY`（UDS へ）+ `TAKO_1038_INJECT_UNREACHABLE` /
  `TAKO_1040_LEGACY` / `TAKO_1042_LEGACY` / `TAKO_1043_LEGACY` / `TAKO_1049_LEGACY`
  （+ `TAKO_1049_WATCH_SECS`）。それ以前の一覧は progress.md 8/24 以前の各エントリ。
  **#1063 は env ではなく `__COMPAT_LAYER=DPIUNAWARE`** で非認識にして A/B する（OS の仕掛け）

## Windows 実機まわり（要点。全文は plan の各記録節）

- **実機セルフテストは #1073 で完走を復旧**（`TAKO_APP_SELF_TEST_OK` / exit 0）。
  **停止は世代差ではなく起動の仕方の差**で、`TAKO_ISOLATED=1` を落とすと data dir が
  本番になり項目 41 / 41b（OSC 7 / 133）が走る = 隔離時は skip。A/B は env まで揃える
  （どちらだったかは skip 行の `script=` が `tako-iso-data-<pid>` かで分かる）
- **`load=unknown` は解消**（境界 `platform::sysload` = B25。Windows は `load=cpu14%/12cpu`）
- 実機テストのベースラインは
  **2026-09-02 に取り直して 21 件**（失敗名まで照合。plan「後続 worker への引き継ぎ」節の表。
  それ以前の「22 件」は `tako-control` が未実行だった頃の値なので当てにしない）
- **実機 SSH は 9/3 時点で疎通**（`ssh win`）。未着手の実機バグ: #935 / #936 / #970 /
  #972 / #973 / #974（#967 は #1073 で解消）。#971 は #1038 で実装済み・実機実測待ち
- **#1063（1.22 倍あふれる）は製品の欠陥ではなかった**: 素の `powershell.exe` は DPI 非認識で、
  `GetWindowRect` は物理 ÷ 倍率・`CopyFromScreen` は物理のまま = スクショがクロップになる。
  **実機を測るときは `scripts/windows/measure-window.ps1`**（PerMonitorV2 を宣言し検算する）
- **Windows の `cargo test` は 9/2 まで `tako-control` が 1 件も走っていなかった**（#983 の
  POSIX 決め打ち）。#1063 の PR で解消。以後スイートを回すときは件数が跳ねることに注意
- 規約: マトリクスは根拠なしに倒さない（`windows_evidence`・T7 が落とす）/ テストに理由文を
  直書きしない / 生成 docs は `gen-windows-support-docs.mjs --check` が CI で見る

## 測り方の要点（繰り返し踏む分だけ）

- セルフテストは `-u TERM -u COLORTERM` で起動する（tako のペイン内から起こすと TERM を継承して
  項目 1b が確定で落ちる）/ `cargo test` には `TAKO_DATA_DIR` を渡す（#944）/
  隔離インスタンスの kill は **ps でフルパス確認 → 明示 pid**（8/29 に pgrep 先頭撃ちで誤爆した）
- 合成クリック（System Events）は GPUI に届かない。ドラッグは合成 PlatformInput なら届く（#725 / #1043）

## リモート刷新（エピック #1059）柱 1 の土台

- **#1068 / #1069 実装済み**（`b42abb8` / `ed69192`。ブランチ `feat/1068-1069-remote-control-optin`）。
  プロファイル `remote_control`（既定 false）で `--remote-control` を渡し、
  `tako sessions link` / MCP / `/api/agents` / `/api/v2/panes` が公式 URL を 1 実装で返す
- **不変条件**: 不適格な環境ではフラグを付けない（付けると claude が起動時に落ちる）/
  証明できるときだけ断る（プラン・ZDR はローカルから分からないので断らない）/
  URL を捏造しない（connected 以外は url も id も持たない）/ アカウント UUID を保持しない
- **実測で分かった前提**: `bridge_status` 行は `--remote-control` つきのセッションだけに出る
  （アカウント既定の自動接続では出ない = `bridge-session` が予備段の主役）。
  この機の既定アカウントは**自動接続が ON** なので、非 opt-in の worker も connected になりうる
- 検証の作法: 隔離は `TAKO_ISOLATED=1` + `TAKO_PERSIST=1` + `TAKO_TMUX_SOCKET=<専用>`
  （`TAKO_ISOLATED` 単独だと `TAKO_PERSIST=0` になり器が無く pane → session 解決が空振りする）。
  実 URL はハッシュだけで扱う（リポ・ログへ実値を残さない）

## 次の一手

- **GUI 再起動で全反映 → ユーザー目視**（#1009 / #1043 の close 判断・#1042 は報告者確認待ち）
- **#1041**（リモートフォルダを開いたらツリー先頭 + ターミナル自動 SSH）が着手可能（#1040 着地済み）
- #975 残: #986（worker MCP）→ #987〜#992。open バグ #1013 / #1015 / #1021 / #1022 / #1030 /
  #1033 / #1034 / #1035
- #1001 残: C2 / C3 / C6 / C7（main.rs 直列群・1 本ずつ）・C5 / C8〜C10（調査系）
- #1007 IDE 化: S0 の一部（#1016）済み → S1（LSP 基盤）。正本 = research/2026-08-28-ide-editor-report.md
- リモート大刷新エピック（ユーザー意向）: リモート側セッション永続等。#1038 GUI 構成の通し検証と
  合わせて設計 Issue を master が提案予定

## 現フェーズで Read すべき設計書

- `.agent/plans/2026-08-remote-folder.md`（SSH / リモートの設計・実測・罠。#1040 の §17 含む）
- `research/2026-08-28-ide-editor-report.md`（#1007 着手時）+ `research/2026-08-28-perf-profile-report.md`（#1001 着手時）
- `crates/tako-core/src/platform/support.rs`（対応マトリクスの正本。判定を触るなら必ず読む）
