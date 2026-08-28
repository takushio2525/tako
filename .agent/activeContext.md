# Active Context

> このファイルは AI が毎ターン上書きする現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。

## 現在の対象（2026-08-28）

- **#1023（新ペイン経路の SSH でターミナルが立たない）を根治して merge 済み**（`4405a12`）。
  根因は **UI 経路が `pending_attach` を消化していなかった**こと。dispatch はペインを作る
  ところまでで、PTY 起動は GPUI の `Context` が要るのでキューへ積まれる。IPC / MCP の
  ループは毎回消化しているので **次に来た CLI / MCP のリクエストで初めて立つ**
  （= 「めっちゃ待つ」の正体。エージェントが動いていなければ立たないまま）
- **#1023 を触るときの不変条件**: ①UI から dispatch を直接呼ぶ経路は
  `attach_pending_sessions(cx)` を呼ぶ（番犬 `ui_dispatch_attach_watchdog`）
  ②**CLI / MCP で覗くと観測自身が消化してしまう**ので、手で試すと直って見える。
  測るのは window 直更新だけの隔離セルフテスト項目 132。A/B は `TAKO_1023_LEGACY=1`
- **#1010（SSH の進行状況の可視化）を実装**。①リモートファイルの取得を GUI だけ背景へ
  （CLI / MCP は同期のまま）+ ツリーに回る弧 ②ペインの SSH 接続待ちをヘッダのチップへ
  （失敗は消えずに理由へ置き換わる）。判定は `tako_core::ssh_progress`（純粋関数）
- **#1010 で踏んだ致命傷（再発させない）**: `gpui::percentage()` は **0.0〜1.0 の外で
  panic**（`debug_assert!`）し、初回描画で**アプリごと abort** する。回転は端数
  （`fract()`）を渡すこと。**隠れたウィンドウではスピナーが 1 フレームも描かれない**ので
  セルフテストでは捕まらない = 隔離 GUI を**前面にして**描かせる検証が要る
- **合成クリック（System Events の `click at`）は GPUI に届かない**（実測）。
  ツリー行のクリックが要る目視検証は自動化できない


- **#1011（`claude agents --json` の起動コスト）を実装。#1001 軽量化エピックの C1**。
  ①**前段ガード** = `<config dir>/sessions/<pid>.json`（claude の台帳。CLI の出力と集合まで
  一致するのを実測）で「その走査先に live な claude が居るか」を Node 無しで見て、
  居ない**明示アカウント**の起動を省く ②**鮮度の用途分離** = `AgentScanFreshness`
  （Monitoring 5s / Ui 30s）。**キャッシュは 1 本のまま**（分けると同時に 2 本走って倍になる）。
  実測 **Node 4 → 2 本 / CPU 0.89 → 0.46s（−48%）で結果は完全一致**
- **#1011 を触るときの不変条件**: ①**既定 config dir は必ず起こす**（`always_scan`。
  #571 の「既定を必ず観測する」保証 + 台帳の取りこぼし検出の機会）②台帳の
  `Missing`（ディレクトリ無し）を空と読むのは**他の走査先で仕組みが確認できたときだけ**
  （台帳を書かない claude で全滅するため）③省いた走査先は `Some("[]")` を返す
  （`None` にすると merge の `any_ok` が汚れて #466 の sticky が壊れる）
  ④答えは**従来どおり `claude agents --json`**（台帳は「起こさなくてよい Node」の
  見分けにしか使わない。誤っても Node を起こす側へ倒れる）
- **claude 2.1.232 の実測（再調査を避ける）**: `claude agents --json` に
  **`contextPercentUsed` / `model` が無い**（実測 13 件すべて null）。
  → `orchestrator self` の `ctx_percent` / `ctx_over_threshold` が常に null なので
  **#749 の自動ハンドオフはこの経路からは発火しない**（別 Issue 化が要る）。
  `claude agents --help` に**複数 config dir をまとめて見る手段は無い**（走査先の統合は不可）

## 現在の対象（2026-08-27）

- **#965（リリースの両 OS 同時化）を実装。初回の同時リリースは v0.7.9 で実行する**。
  Windows の配布物はこれまで実機でしか作れず、実機が落ちていると macOS 版だけが出ていた
  （**v0.7.1〜v0.7.8 の 8 リリースが実際に macOS のみ** = その間 Windows の利用者には
  更新が 1 つも見えていない。更新判定は自 OS 向けアセットの有無 = #595）。
  生成を `.github/workflows/release-windows.yml`（タグ push → windows ランナー）へ寄せ、
  `scripts/release.sh` が添付を待ってノートを作り直し、揃わなければ **exit 3** を返す形にした
- **リリースを触るときの不変条件**: ①判定の正は `tako-core::platform::release_assets`
  （`missing_platforms` / `is_complete` / `os_requirement`）で、sh / PowerShell の写しは
  同期テストが拘束する ②配布物の検査は `installer/windows/lib/verify-assets.ps1` の 1 実装を
  CI と実機が共有する ③版数の比較は**数値部分**（`--promote` は Cargo.toml=`0.8.0-test.1` /
  タグ=`v0.8.0` で同一 commit になるため厳密一致にすると落ちる）④片肺の検査は
  `scripts/release.sh --check-assets [tag]`、モックテストは `scripts/test-release-retry.sh`
- **#975（agent parity エピック）の緊急 2 件**（別ラインの作業。Windows 系とは独立）:
  **#981 は完了**（codex のサンドボックス解除を明示 opt-in へ。`bypass_sandbox` 既定 false +
  既存プロファイルは移行で `true` = 挙動不変。A/B は `TAKO_981_LEGACY=1`）/
  **#983 は変更 1 のみ**（agent CLI の実在をペイン作成前に確かめる。A/B は `TAKO_983_LEGACY=1`。
  変更 2 / 3 = 送達観測手段の無い agent を `NotApplicable` のまま黙らせない・系統の網羅は
  **#982 の merge 後**）
- **codex の実測（#981 の根拠。再調査を避ける）**: 承認スキップとサンドボックス解除は
  同一フラグしか無い。既定（read-only）は cwd 内への書き込みすら不可 / `workspace-write` は
  ネットワークと cwd 外が不可 / `danger-full-access` で両方可 → worker の実務
  （cargo・gh・data dir 書込）が成立しないので中間状態は作らず 2 択にした
- **`tako master` / `tako solo` は表示言語を初期化していなかった**（`i18n::CURRENT` の静的既定が
  En）。#983 で 1 行足して揃えたが、**他にも `Note` 由来の文言を出す CLI 経路があれば同じ穴**

- **#932（ちらつき）第 2 ラウンド: タブ切り替えの「遅れリサイズ」を突き止めて根治した**。
  裏タブのペインは `render_pane` を通らないので、#647 が入ったあとも**幾何の変更**
  （ウィンドウ寸法・サイドバー幅・バナー）が届かず、表に出した瞬間に初めて
  リサイズ = SIGWINCH が飛んでいた（実測 裏 116x37 / 表 88x33 → 表に出した瞬間 88x33）。
  割り出しを表示中と同じ 1 本（`pane_text_area_of` → `grid_cells`）へ寄せて解消。
  A/B は `TAKO_932_NO_OFFSCREEN_GEOMETRY=1`。**実機で症状が消えたかはユーザー確認待ち**
- **#932 で潰した仮説（実測で否定。再調査の周回を避ける）**: 器（tmux）はリサイズで
  画面を消さない（`ED 2` 0 回・再描画は 0.1〜0.4ms で完了）/ 実 claude の TUI は
  SIGWINCH で消えない（4.7ms 刻みで一度も半分未満にならない）/ タブ切り替え・分割比変更・
  ウィンドウ寸法変更でグリッドが空になることは無い（1〜5ms 刻みで `grid_blackouts=0`）。
  詳細は `.agent/architecture.md`「裏タブのペインは「表に出たときの寸法」へ合わせる」
- **#467 Windows 移植はスライス 1〜9 がすべて main へ入り、最後の 8（棚卸し）も完了**。
  残りは「実機バグの消し込み」と「未実測項目の消し込み（#937）」だけ
- **#591（対応マトリクスの棚卸し + docs ページ）を完了**。判定は
  **supported 69 / degraded 13 / pending 56 / unsupported 2**（棚卸し前は 4 / 2 / 132 / 2）。
  `Feature::windows_evidence` を新設し **T7 が根拠なしの Supported を落とす**。
  docs は `docs/src/content/docs/windows-support.md`（生成物・CI で `--check`）
- **Issue の「完了（`cf7c9a4`）」は main に入っていなかった**（#658 と同じ型）。
  `windows/467-*` ブランチのコミットを「入っている」と読まないこと
- **#617（ゴミ箱が完全削除）は main へ移植して解消**（実装は win467 の `d528058` /
  `4752eee` に在り main には 1 行も入っていなかった = #658 / #591 と同じ型）。
  `SHFileOperationW` + `FOF_ALLOWUNDO` へ差し替え、その他 unix は**削除へ劣化させずエラー**。
  表記は `os_integration::file_manager()` 1 か所で決めて `FileManager` を値で配る。
  **実機は offline なので #617 は open 維持**（実機確認項目は Issue コメント）
- **#722（AI タブ命名が Windows で一度も走らない）も main へ移植して解消**。
  `autorename::detect_claude()` だけが B16（`platform::exe::find`）へ寄せられておらず、
  `$SHELL -l -c "command -v claude"` が Windows で必ず失敗 → `.ok()?` で `None` →
  `OnceLock` なので永久に無効、という**黙って死ぬ**形だった。判断部分を純粋関数
  `resolve_claude` に切り出し、`TAKO_AUTORENAME_DIAG=1` で理由を出せるようにした。
  マトリクスは **`Supported` へは倒さず `Degraded` のまま**理由文を #760 の実態
  （素材が不変なので命名はタブごとに 1 回だけ）へ差し替えた
- **棚卸しで確認した残りの製品バグ（未着手）**: **#935**（受け入れゲートが `sh -c`）/
  **#936**（古い claude の警告が出ない。#726 の続き）
- **#937 の消し込みで見つけた製品バグ（未着手）**: **#970**（`open-in dir` の cwd が
  `///?/C:/…` へ壊れ、そのタブの git 操作が全滅）/ **#971**（remote の tailscale serve が
  unix ソケット target で Windows 非対応 = デーモンを起動できない）/ **#972**（remote
  scrollback が器の境界を通らない）/ **#973**（autosave が CLI / MCP 編集で不発。macOS も同じ）/
  **#974**（psmux が持たないオプションを tako が conf へ書いていて毎回警告）
- **実機の claude は OAuth 期限切れ**（`Failed to authenticate: OAuth session expired`）。
  会話が要る検証（#722 の AI 命名 / report の transcript 層 / run の完遂 / setup の対話の通し）は
  ログインし直すまで測れない
- **#937（消し込み完了）**: 未実測 47 件を Windows 実機で実測し **未実測 0 件**へ。
  判定は **supported 110 / degraded 13 / pending 15 / unsupported 2**。残る pending 15 は
  「実装が無い / 動かないと分かっている」もので未実測ではない
- **#467 Windows 移植はスライス 1〜9 が全部 main へ入り、棚卸し（8）も完了**。
  残るのは実機バグの消し込みだけ（下記）
- **未着手の製品バグ**: **#935**（受け入れゲートが `sh -c`）/ **#936**（古い claude の
  警告が出ない。#726 の続き）/ **#970**（`open-in dir` の cwd が `///?/C:/…` へ壊れ git 全滅）/
  **#971**（remote の tailscale serve が unix ソケット target で Windows 非対応）/
  **#972**（remote scrollback が器の境界を通らない）/ **#973**（autosave が CLI / MCP 編集で
  不発。macOS も同じ）/ **#974**（psmux が持たないオプションを conf へ書いて毎回警告）/
  **#967**（セルフテスト項目 97 (d) が `tako.exe` を見ておらず 98 以降が走らない。製品は正しい）
- **実機の claude は OAuth 期限切れ**（`Failed to authenticate: OAuth session expired`）。
  会話が要る検証（#722 の AI 命名 / report の transcript 層 / run の完遂 / setup の対話）は
  ログインし直すまで測れない
- **A/B の env（同一バイナリで旧挙動へ戻せる）**: `TAKO_920_LEGACY` / `TAKO_913_LEGACY` /
  `TAKO_906_NO_PAD` / `TAKO_907_NO_INJECT` / `TAKO_903_LEGACY` / `TAKO_866_KEEP_EXACT_TARGET` /
  `TAKO_932_NO_OFFSCREEN_GEOMETRY` / `TAKO_961_LEGACY` / `TAKO_966_LEGACY` /
  `TAKO_1011_LEGACY`（+ 故障注入 `TAKO_1011_INJECT_LEDGER_GAP`）/ `TAKO_1023_LEGACY` /
  `TAKO_1010_LEGACY`

- **#982（agent 能力マトリクス）完了 = #975 エピックの土台**。`tako-core::agent_support` が
  「どの agent がどこまで使えるか」の正本（40 能力 × claude / codex / agy / ローカル LLM）。
  **claude 以外を断定するなら根拠が必須**（T7 相当が落とす）で、**未調査を `Unsupported` へ
  倒さない**（`Pending` + 追跡 Issue）。agent 種別の enum は 5 つ並存のまま対応を機械検証する
  （統合しない理由と寄せ先一覧は `.agent/agent-enums.md`）。以降のスライスは
  「1 マスを動かして根拠を書く」粒度

## 対応マトリクスを触るときの規約（#591）

- **根拠なしに `Supported` / `Degraded` / `Unsupported` へ倒さない**。`windows_evidence` へ
  「実機セルフテストの項目 / 実機で緑のテスト名 / 実測の記録」のどれかを書く。
  書けないなら `Pending` + `notes::WIN_UNVERIFIED` + 追跡 #937 のまま置く（T7 が落とす）
- 宣言は `PlatformFacts` 経由で master / solo / setup の system prompt へ流れる（#516）。
  **過大申告はエージェントを誤らせ、過小申告は使える機能を回避させる**
- 理由文は「〜が前提」ではなく**実際に何ができないか**を書く（回避行動が取れる形）
- docs は生成物。`cargo build -p tako-cli && node scripts/gen-windows-support-docs.mjs`。
  新機能はスクリプトの `CATEGORIES` へ 1 行足す（足さないと生成が落ちる）
- **テストに理由文を直書きしない**。期待値はマトリクスから作る（#920 / #591 の両方で踏んだ）

## 実機セルフテストの到達範囲（#920 後の実測）

**完走している**（`TAKO_APP_SELF_TEST_OK` / exit 0 / FAILED 0 / skip 19）。
skip 19 は全部理由つきの既知で、内訳がそのまま「Windows で動かないもの」の一覧になる:
psmux が本物の tmux でない系（#519）/ PDF の text_layer 不在（#693）/ WebView2 の panic（#724）/
macOS 固有の項目 79（#872）/ POSIX 専用の道具（nc・ジョブ制御・`/dev/fd`・ECHOCTL）/
links の POSIX 前提（#522）/ 蓋閉じで未描画になる項目。

**実機テストのベースラインは 22 件**（21 + #930）。失敗名まで照合する（全数は plan の
「#906 の記録」節）。**製品の縮退を指すもの**（acceptance_gates 5 = #935 / stale_binary 2 = #936 /
remote 2）と**テスト側の POSIX 前提**（`/tmp` 直書き・区切り決め打ち・symlink）は別物。

## 実機テストの読み方（要点。全文は plan の各記録節）

- **psmux の e2e / GUI セルフテストは `schtasks /it`（session 1）で回す**。SSH（session 0）で
  作った psmux の detached セッションは約 1 秒で自然死する
- **孤児は run のたびに掃除する**。「tako-app が 1 つも居ない」を確かめてから
  `-L tako-iso-*` を**明示 pid で**落とす（`-L tako` は本番）
- **GUI 起動時の env を再現してから測る**（`SHELL` / `HOME` は SSH セッションの Process
  スコープにしか無い）。長い処理は `schtasks` か `Invoke-CimMethod` で投げ、
  **ログの `EXITCODE=` 行で完了を待つ**
- **測定側も UTF-8 にする**（`[Console]::OutputEncoding`）。ログは `-Encoding UTF8` で読む
- **`git stash` を A/B に使わない**。`git checkout <sha> -- <path>`。ただし**未コミットのまま
  `git checkout HEAD -- <path>` は自分の変更を全部捨てる**
- **fresh worktree は `web/tako-remote/dist/` を持たない**（`rust_embed` が埋め込むので即失敗）。
  既存 worktree からコピーする。**docs も `npm ci` が要る**
- **`cp` が `-i` の別名かもしれない**: スクリプトでは `command cp -f` を使う

## 測り方の落とし穴（#932 で踏んだ。他の検証にも効く）

- **セルフテストは `-u TERM -u COLORTERM` で起動する**。tako のペインへ渡る TERM は
  親から継承されるので、tako のペインの中（`TERM=tmux-256color`）から起こすと
  項目 1b（TERM / COLORTERM 注入）が**決定的に落ちる**（main でも 3/3）。GUI 起動には
  親の TERM が無いので、外して測るのが本番と同じ条件
- **`cargo test` は本番 data dir へ書く**（#944）。`TAKO_DATA_DIR` を渡さないと本番
  `perf.log` へ入り、しかもテストプロセスは `mark_main_thread()` を呼ばないので
  **全部「メインスレッド専有」と誤記録**される（本番ログの 643 行の正体）
- **`visual-test` の全節は現状 main でも `term-grid attrs-underline` で止まる**（#943。
  `ul_strip=32` が期待の 40 未満。e703e40 で同じ数値を実測）

- **#1002（setup のモデル実取得ピッカー）完了 = #975 エピックのスライス**。取得手段の正本は
  `tako-control::agent_models`（**codex = `codex debug models`** / **agy = `agy models`** /
  **claude は一覧コマンドが無い** = 同梱エイリアス + 取得不可の明示）。CLI `tako setup models` /
  MCP `tako_setup_models`（**142 ツール**）。**対話ピッカーは `--review` だけ**（標準経路の
  質問ゼロ = #262 を壊さない）。**1 番は常に「CLI の既定に任せる」**（#27 / #67）
- **agy の `--effort` は実在する**（実測: `agy models` の全モデルで `low|medium|high` の検証が
  走る。表示名の `(High)` は別物。**未知のモデル名のときだけ**「effort 非対応」と言う）。
  マトリクスの `effort_control` を Supported へ倒し worker 起動へ渡した。A/B は `TAKO_1002_LEGACY=1`
- **能力マトリクスの構造的な限界（再調査を避ける）**: `claudeは基準系なので全て対応済み` が
  claude = 全 Supported を強制するので、**claude が最弱になる能力は 1 マスで表せない**。
  #1002 は能力を「setup でモデルを選べるか」へ切り出し、取得手段の差は実行時の
  `source`（cli / builtin / none）+ `failure.kind` で表した
- **GUI のモデル候補は「押したときだけ background」**（設定画面 → プロファイルの model 行）。
  `agy models` はネットワーク取得なのでタブを開くたびに取ってはいけない。取得の形は
  `settings_window.rs` の `refresh_agent_clis`（#168 の教訓）と同じ
- **setup を測るときの作法（#1002 で踏んだ）**: `script -q` + パイプ入力は**先頭 1 行が消える**
  → `expect` で 1 問ずつ送る / zsh スクリプト内の `cmd &` は **stdin が /dev/null** になり
  TTY 判定が false（ピッカーが出ない）/ `setup_dir()` は `TAKO_DATA_DIR` を見ない
  （`~/Library/Application Support/tako/setup` へ書く。中身は同梱テンプレなので無害）/
  **`~/.claude.json` と `~/.claude/.claude.json` は稼働中の claude が常時書く**ので
  「変更検知 → 復元」の対象にしてはいけない

## 次の一手

- **v0.7.9 の初回同時リリース**（#965。PR merge 後）: main を pull → `git tag -a v0.7.9` を
  push（ここで Windows のワークフローが走る）→ `scripts/release.sh --test`。
  release.sh が Windows の添付を待ってノートを作り直し、揃わなければ exit 3 を返す
- **#1002 の残り**: モデル一覧の Windows 実機実測（マトリクスは `Pending` + 追跡 #937 のまま）。
  ローカル LLM の一覧は #990 の範囲（`Agent::Local` は `Pending`）
- **#935 / #936**: どちらも「境界へ寄せる」既存の型がある（#875 = B1 / #898 = B16 /
  スライス 9 = procinfo）ので寄せ先は決まっている
- **#970〜#974**: #937 の消し込みで見つけた実機バグ。#971 が片付くまで remote 系は測れない
- **シェルスクリプトを書くときは変数の直後の全角に注意**（#837）。番犬は
  `crates/tako-control/tests/shell_scripts.rs`（`scripts/` 配下の .sh を全部走査する）
- **PR がコンフリクトしていると GitHub は `pull_request` の CI を作らない**（#965 で 20 分
  溶かした）。`gh api .../actions/runs?head_sha=<sha>` が **0 件**で、同時刻に他ブランチの run は
  作られている、という形で現れる。ワークフローの yaml を疑う前に
  `gh pr view <N> --json mergeable,mergeStateStatus`（`CONFLICTING` / `DIRTY`）を見る

## 現フェーズで Read すべき設計書

- `.agent/plans/2026-08-windows-main-merge-wip.md`（「8 の記録」節に棚卸しの作法と根拠の在庫表・
  ベースライン失敗名の切り分け。「#920 の記録」節に完走までの経緯と skip 19 の内訳。
  各 Issue の記録節に実機の測り方）
- `.agent/plans/2026-07-windows-port-architecture.md`（境界の定義）
- `crates/tako-core/src/platform/support.rs`（対応マトリクスの正本。判定を触るなら必ず読む）
