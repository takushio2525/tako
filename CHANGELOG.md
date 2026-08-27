# Changelog

All notable changes to this project will be documented in this file.
The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

Platform-specific entries are tagged with `[Windows]` / `[macOS]` right after the
change-type tag. Entries without a platform tag apply to every platform.
プラットフォーム固有の項目は種別タグの直後に `[Windows]` / `[macOS]` を付ける
（無印 = 全プラットフォーム共通）。規約の詳細は `.agent/conventions.md`。

## [0.8.0] - 2026-08-28

Nightly minor release (automated). Changes since v0.7.10:
夜間マイナーリリース（自動）。v0.7.10 以降の変更:

- [機能追加] setup でエージェントごとのモデル一覧を実取得して選べるようにする (#1002) (#1014)
- [改善] claude agents --json の起動コストを削る: 前段ガード + 鮮度の用途分離 (#1011) (#1018)
- [ドキュメント] progress.md に #1006（SSH 接続の開き方 2 点）の記録を追記
- [改善] SSH 接続の開き方 2 点: ペインメニューからの SSH 化 + 既定を現在タブの新ペインへ (#1006) (#1017)
- [機能追加] 夜間リリースの次回バージョン予約: patch 固定をやめ minor / major を夜間発火に乗せる (#1005) (#1008)
- [改善] codex の利用上限を構造化データで扱い自動復帰を成立させる / agy の可否を実測で確定 (#985) (#1003)

## [Unreleased]

### Changed / 変更

- [改善] SSH 接続の開き方を 2 点変えた (#1006)
  ① **ペインの右クリックメニューに「このペインでリモート接続…」を追加**。ホストを選ぶと
  そのペイン自体が SSH セッションになる（ペインもタブも増えず、ペイン ID も変わらない）。
  素のシェルのペインにだけ出る（全画面 TUI・実行中・AI エージェント・プレビューには出さない）。
  ② **ファイルメニュー「リモート接続…」の開き先を新タブから「いま開いているタブの新ペイン」へ**。
  CLI / MCP も同じ既定で、`--target split | tab | pane`（MCP は `target`）で選べる。
  併せて GUI 経路が dispatch を通るようになったので、#919 の契約（接続前バナー・
  ConnectTimeout・失敗の理由を画面に残す）がファイルメニュー経路にも効く。

  ① The pane context menu gains "Connect this pane via SSH…": picking a host turns that
  very pane into an SSH session — no new pane, no new tab, same pane ID. It only appears
  for plain shell panes (not full-screen TUIs, running commands, agent panes or previews).
  ② "Open Remote…" now opens a new pane in the current tab instead of a new tab. The CLI
  and MCP share the default and can choose with `--target split | tab | pane`. The GUI path
  now goes through the same dispatch, so the #919 guarantees (connecting banner,
  ConnectTimeout, failures staying on screen) finally apply to it too.

## [0.7.10] - 2026-08-27

Nightly patch release (automated). Changes since v0.7.9:
夜間パッチリリース（自動）。v0.7.9 以降の変更:

- [改善] codex worker の状態監視を claude 同等へ: 構造化ソース + 弱マーカーの agent 別分離 (#984) (#1000)
- [修正] 非 claude agent の spawn の無言死を塞ぐ: CLI の実在をペイン作成前に確かめる (#983) (#999)

## [0.7.9] - 2026-08-27

テスト版。**初めて macOS と Windows の配布物が同じリリースに揃う**節目のリリース。
これまで Windows 版の配布物は実機でしか作れず、実機が落ちていると macOS 版だけが出る
「片肺リリース」になっていた（v0.7.8 までが実際にそう）。配布物の生成を GitHub Actions の
windows ランナーへ寄せ、リリースは両 OS が揃って初めて成立する形にした。

Test release. The first release where **macOS and Windows binaries ship together**. Until
now the Windows artifacts could only be produced on physical hardware, so whenever that
machine was offline the release went out macOS-only (which is what actually happened through
v0.7.8). Artifact generation now runs on a GitHub Actions windows runner, and a release is
only considered complete when both platforms are attached.

### Added / 追加

- [機能追加] リリースの macOS / Windows 同時化 (#965)
  タグを push すると windows ランナーがインストーラー exe とポータブル zip を作り、
  同じ Release へ添付する。`scripts/release.sh` はその添付を待ってからリリースノートを
  実アセットで作り直すので、ダウンロード表・**動作要件**・Windows 手順・Known limitations が
  揃った状態で公開される。片方の OS しか無いリリースは終了コード 3 で検出し、
  `scripts/release.sh --check-assets [tag]` でいつでも検査できる。夜間パッチリリースも
  同じ経路を通る。
  Pushing a tag now builds the Windows installer and portable zip on a windows runner and
  attaches them to the same Release. `scripts/release.sh` waits for that attachment before
  regenerating the release notes from the real assets, so the download table, **system
  requirements**, Windows install steps, and known limitations are all present when the
  release goes public. A release with only one platform's assets is reported as exit code 3,
  and `scripts/release.sh --check-assets [tag]` can audit any published release. The nightly
  patch release takes the same path.
- [機能追加] リモートファイルの編集・保存を SFTP の書き戻しで開放 (#966)
  Opened up editing and saving remote files through SFTP write-back.
- [機能追加] `tako setup` / `setup-mcp` が codex・agy にも tako の MCP を登録する (#979)
  `tako setup` and `setup-mcp` now register tako's MCP server with codex and agy as well.
- [機能追加] ペインの ssh を検知してリモートフォルダを自動追加し、ツリーの見た目を統合 (#976)
  Panes running ssh are detected so the remote folder is added automatically, and the tree
  now looks the same as a local one.

### Fixed / 修正

- [修正] タブバーのスクロールが効かない問題を根治（occlude した要素からホイールを中継する） (#961)
  Fixed the tab bar refusing to scroll horizontally (wheel events are now relayed from
  elements that occlude the scroll area).
- [修正] [Windows] スターター / welcome のコマンド投入を方言と送達確認つき経路へ寄せる (#899)
  The starter and welcome buttons did nothing on Windows; the command is now written through
  the shell-dialect and delivery-confirmed path.
- [修正] [Windows] タブの AI 自動命名が走らない問題を根治（claude の解決を実行ファイル探索の境界へ） (#722)
  AI tab naming never ran on Windows because the claude lookup went through a login shell.
- [修正] [Windows] ファイル操作を是正: 完全削除をごみ箱移動へ + エクスプローラー表示・既定アプリ (#617)
  "Move to trash" permanently deleted files on Windows; it now goes to the Recycle Bin, and
  reveal-in-Explorer / open-with-default-app are wired up.
- [改善] 実行中タブのドット脈動を有限回にしてフレーム要求の恒久化を止める (#945)
  The running-tab dot animation kept requesting animation frames forever, holding CPU.
- [修正] シェルスクリプトで変数の直後に全角文字が続く箇所を機械検出する番犬を追加 (#965)
  `$var（` は bash が全角のバイトを変数名へ取り込んで `set -u` で即死するが `bash -n` では
  見つからない。`scripts/` 配下の .sh を走査するテストを常設し、潜在していた 1 件も直した。
  Added a watchdog test that scans every `.sh` under `scripts/` for `$var` immediately
  followed by a multi-byte character (bash absorbs those bytes into the variable name and
  dies under `set -u`, and `bash -n` cannot see it). One latent occurrence was fixed.

### Changed / 変更

- [修正] codex のサンドボックス解除を明示 opt-in へ（既定でバイパスしない） (#981)
  Sandbox bypass for codex is now explicit opt-in instead of always-on.

- [ドキュメント] [Windows] 対応マトリクスの未実測 47 件を実機で消し込み、未実測 0 件へ (#937)
  Cleared all 47 unverified entries in the platform support matrix on real Windows hardware.

## [0.7.8] - 2026-08-25

Nightly patch release (automated). Changes since v0.7.7:
夜間パッチリリース（自動）。v0.7.7 以降の変更:

- [修正] タブを切り替えた瞬間の端末リサイズを根治: 裏タブのペインを「表に出たときの寸法」へ合わせる (#932) (#958)
- [修正] 端末の行の字の大きさもペインのフォントサイズへ追従させる (#947) (#957)
- [修正] フォントサイズ変更が表示中タブ以外のペインへ届かない問題を根治 (#647) (#467) (#952)
- [機能追加] tako sessions を器なし構成でも動かす: 検出の二段構えと境界経由のペイン列挙 (#728) (#467) (#956)
- [修正] UI ストール診断の誤認を正し、端末取り込み経路を計測対象に入れる (#643) (#467) (#954)
- [修正] IME を壊す「入力ハンドラ未登録のフレーム」を作らない (#623) (#467) (#953)
- [修正] docs のコードブロックで帯と本体の接合部が分離して見える問題を根治 (#950) (#955)
- [修正] IME 未確定文字列の大きさを変換先ペインのフォントサイズへ追従させる (#940) (#949)
- [機能追加] 静止画面のちらつきを機械検証する visual-test 節を新設 (#932) (#942)
- [修正] tako-app を release で GUI サブシステムへリンクし、無音死を防ぐ (#586) (#467) (#941)
- [ドキュメント] README を再構成 + Homebrew 導入手順とドキュメントサイトへの導線を追加 (#939)
- [改善] 対応マトリクスを実測根拠つきで棚卸し + docs「Windows 対応状況」ページ (#591) (#938)

## [Unreleased]

### Added

- [機能追加] `tako setup` がエージェントごとの**モデル一覧を実取得**して選べるようにした
  (#1002)。取得は各 CLI の一覧コマンド（**codex = `codex debug models`** / **agy = `agy models`**）で、
  codex はモデル別の effort 語彙とコンテキスト長まで並ぶ。**claude は一覧コマンドを持たない**
  （`claude models` はプロンプトとして解釈される）ので同梱のエイリアス（`opus` / `sonnet` /
  `fable`）を並べ、「実取得ではない」ことを明示する。取得の失敗は 5 種に分類して
  「理由 + 次の一手」を返す（未導入 = 導入コマンド / 未ログイン / 一覧コマンドが無い /
  コマンド失敗 / 書式が変わった）。一覧は `tako setup models [--agent <系統>] [--json]` /
  MCP `tako_setup_models` から読める。**対話ピッカーは `tako setup --review` だけ**で、
  標準 `tako setup` は質問ゼロのまま「いま何が選ばれているか + 変えたいときのコマンド」を
  出す。**1 番は常に「CLI の既定に任せる」**（tako はモデルを固定しない）。未導入の系統も
  選択肢に並び、選ぶと導入手順が返る。選択は既定プロファイルの model / effort へ
  ロック付きで書くので既存の設定は壊れない（master が別系統のときは master 側へ書かない）
- [Added] `tako setup` can now **fetch the real model list from each agent CLI** and let you
  pick one (#1002). The list comes from each CLI's own command (**codex = `codex debug
  models`**, **agy = `agy models`**); for codex it includes the per-model effort vocabulary
  and context window. **claude has no such command** (`claude models` is interpreted as a
  prompt), so tako offers its built-in aliases (`opus` / `sonnet` / `fable`) and says
  explicitly that this is not a live fetch. Failures are classified into five kinds with a
  reason and a next step (not installed — with the install command / not signed in / no list
  command / command failed / format changed). Read the list with `tako setup models
  [--agent <agent>] [--json]` or MCP `tako_setup_models`. **The interactive picker only runs
  under `tako setup --review`**; plain `tako setup` stays question-free and just prints what
  is selected plus the command to change it. **Option 1 is always "leave it to the CLI
  default"** — tako never pins a model for you. Agents that are not installed are still
  listed, and picking one returns install guidance. The choice is written to the default
  profile's model / effort under a lock, so the rest of your profile is untouched (and the
  master side is left alone when master runs a different agent).
- [機能追加] 設定画面（Cmd+, → プロファイル）の model 行からも同じ候補を選べるようにした
  (#1002)。「候補を取得」を**押したときだけ** background で一覧を取るので、タブを開く動作は
  今までどおり軽い（`agy models` はネットワーク取得で数秒かかる）。自由入力は残してある。
  agy の effort チップも同じ変更で出るようになった
- [Added] The settings screen (Cmd+, → Profiles) can pick from the same list on its model row
  (#1002). The list is fetched in the background **only when you press "Fetch options"**, so
  opening the tab stays as fast as before (`agy models` does a network round-trip that takes
  seconds). Free-text entry is still available. agy's effort chips now appear too.
- [機能追加] agy の worker へも `--effort` を渡すようにした (#1002)。`agy models` が挙げる
  全モデルで `--effort low|medium|high` が受理されることを実測した（表示名の `(High)` 等は
  モデル側の設定で `--effort` とは別物）。旧挙動は `TAKO_1002_LEGACY=1`
- [Added] agy workers now receive `--effort` as well (#1002). Measured that
  `--effort low|medium|high` is accepted for every model `agy models` reports (the `(High)`
  suffix in a display name is a property of the model, not the `--effort` flag). Set
  `TAKO_1002_LEGACY=1` for the previous behaviour.
- [機能追加] ペインで `ssh <host>` に入るだけで、そのホストのフォルダがファイルツリーへ
  並ぶようにした (#976)。明示的な「リモートからフォルダを開く」操作は要らない（初期パスは
  sftp の初期 cwd = リモートのホーム）。ルートは**ローカルフォルダと同じ形**（フォルダ名 +
  フォルダアイコン）で**ローカルの後ろに**並び、SSH であることと相手は行末のバッジ
  （`SSH <host>`）が示す。**切断してもフォルダは消さず**バッジが「切断」へ変わる
  （右クリックの「再読み込み」で復帰）。宛先を取り違える形（`-p` / `-J` /
  `-o Hostname=` / `ssh host <コマンド>` / `-N` 等）は**見送って理由を残し**、鍵・agent で
  入れない相手はパスワードを聞かずに見送る。**アイドル時のコストはゼロ**: 走査の材料は
  OSC 133 のコマンド状態とペインの子 pid で、指紋が動いたときと 60 秒の保険
  （生きた ssh を抱えている間だけ）でのみプロセス表を採り、採取は #772 / #779 の
  `ProcessSnapshot` へ相乗りする（実測: 自動追加 ON / OFF どちらもアイドル 120 秒で
  採取 6 回 = **同数**）。実測では `ssh <host>` から検知 5 秒・ツリー表示 12 秒。
  切替は 設定画面 → リモート / `tako remote-folder auto [on|off]` / MCP
  `tako_remote_folder` の `action: "auto"`（既定 ON）。Windows はプロセスのコマンド行を
  採れないので自動検知は働かない（明示経路はそのまま使える）
- [Added] Entering `ssh <host>` in a pane now makes that host's folder appear in the file
  tree (#976) — no explicit "Open Remote Folder" step, and the initial path is the remote
  home (sftp's initial cwd). The root looks exactly like a local workspace folder (name +
  folder icon) and sits after the local ones; a badge at the end of the row (`SSH <host>`)
  says it is remote and which machine it is. **Disconnecting never removes the folder** —
  the badge turns red and reads "offline", and the right-click "Reload" brings it back.
  Command lines where tako cannot guarantee it would reach the same machine (`-p`, `-J`,
  `-o Hostname=`, `ssh host <command>`, `-N`, …) are skipped with a recorded reason, and
  hosts that need a password are skipped instead of prompting. Idle cost is zero: the scan
  is gated on OSC 133 command state plus the pane's child pid, runs only on fingerprint
  changes and a 60 s safety net (only while a live ssh is tracked), and shares the
  `ProcessSnapshot` introduced by #772 / #779 (measured: 6 captures per idle 120 s with the
  feature both on and off). Measured latency: detected 5 s after `ssh <host>`, folder shown
  at 12 s. Toggle it in Settings → Remote, `tako remote-folder auto [on|off]`, or MCP
  `tako_remote_folder` with `action: "auto"` (on by default). Windows cannot read process
  command lines, so auto-detection does not run there (the explicit path still works)

### Fixed

- [修正] codex / agy / claude の CLI が入っていない環境で worker を spawn すると、tako が
  構造化された失敗を 1 つも出さずに「成功」と報告していた問題を修正 (#983)。ペインには
  `command not found` が出るだけ、送達検査は「実行された」までしか見ず、`prompt_delivery` も
  claude 以外は `n/a` だったため、**spawn は成功したと言われたのに worker が何もしない**
  状態（無言死）になっていた。**ペインを作る前に**実行ファイルの実在を確かめ、無ければ
  理由 + 次の一手（公式の導入コマンド・参考 URL・`tako setup` の案内）を返す。
  失敗しても空ペインもレジストリの active も残らない。解決した実行ファイルは spawn 応答の
  `agent_path` に載る。同じ検査は `tako master` / `tako solo` / 引き継ぎの後任 master /
  コンフリクト解消エージェントにも入っている
- [Fixed] Spawning a worker when the agent CLI (codex / agy / claude) is not installed no
  longer reports success while nothing happens (#983). Previously the assembled command was
  piped straight into the shell, so the pane just showed `command not found`: the delivery
  check only verifies that *something* ran, and `prompt_delivery` returns `n/a` for
  non-claude agents — a silent death. tako now checks that the executable exists **before
  creating the pane** and, when it is missing, returns the reason plus the next step (the
  official install command, a reference URL and a pointer to `tako setup`). No empty pane
  and no active registry entry is left behind, and the resolved executable is reported as
  `agent_path` in the spawn response. The same check guards `tako master`, `tako solo`, the
  successor master created by a handoff, and the conflict-resolver agent
- [修正] codex を master / solo / worker に使うとき、承認とサンドボックスを丸ごと外す
  `--dangerously-bypass-approvals-and-sandbox` が**無条件で付いていた**のを、
  プロファイルの `bypass_sandbox`（既定 false）による明示 opt-in へ変更 (#981)。
  claude master には相当するフラグが無いので系統間で既定の安全性が非対称で、
  外す手段（opt-out）も無かった。既存プロファイルは自動マイグレーション（#916）で
  `bypass_sandbox: true` が書かれるので**挙動は変わらない**（新規だけ安全側になる）。
  操作は CLI `--bypass-sandbox` / MCP `bypass_sandbox` / 設定画面のトグルの 3 経路。
  起動時に「サンドボックスが今どうなっているか」を 1 行出す（外れていないときも出して
  外し方を添えるので、承認プロンプトで止まっても理由が画面に残る）
- [Fixed] Launching codex as master / solo / worker no longer passes
  `--dangerously-bypass-approvals-and-sandbox` unconditionally; it is now an explicit
  opt-in via the profile's `bypass_sandbox` (default false) (#981). claude master has no
  equivalent flag, so the previous default made the two agents asymmetric in safety with
  no way to opt out. Existing profiles are auto-migrated (#916) to `bypass_sandbox: true`,
  so **behavior does not change for them** — only newly created profiles get the safe
  default. Configurable from the CLI (`--bypass-sandbox`), MCP (`bypass_sandbox`) and the
  settings window, and the launch output now states whether the sandbox is on or off
- [修正] タブバーの横スクロールが効かず、タブが増えると画面外に埋もれてアクセスできない
  問題を根治 (#961)。原因は #576（`0880c26` で main へ）がタブピルへ付けた `occlude()`。
  GPUI の `hit_test` は `occlude()` で走査を **break** するため、祖先である
  `tab-scroll-area` の hitbox が hit test から落ち、`overflow_x_scroll` の発火条件
  （`should_handle_scroll()`）が **タブピルの上では常に false** になっていた
  （ピルが領域のほぼ全面を覆うので事実上どこでも効かない）。`occlude()` は
  Windows のボタン死（#576）を防ぐために必要なので外さず、**スクロール領域の中で
  occlude する要素がホイールを自分で中継する**形にした。中継の計算は GPUI 既定と
  同じ意味論なので、ピルの上と隙間の上で挙動が一致する
- [Fixed] The tab bar can be scrolled again; tabs beyond the visible width are no longer
  unreachable (#961). #576 added `occlude()` to each tab pill so Windows would stop
  swallowing clicks, but GPUI's hit test *breaks* at an occluding hitbox, so the
  scroll container behind the pills never satisfied `should_handle_scroll()` — and the
  pills cover almost the whole strip. The pills keep `occlude()` (removing it brings
  #576 back) and now relay wheel events to the scroll container themselves, using the
  same delta rules GPUI applies, so scrolling over a pill and over a gap behave alike.

### Changed

- [改善] 実行中タブのドット脈動を「走り始めの合図」に限り、フレーム要求の恒久化を
  止めた (#945)。状態ドットは `Running` のあいだ 2 秒周期で脈動していたが、GPUI は
  アニメーションが動いているあいだ**毎フレーム再描画を要求する**ため、エージェント
  （claude / codex）のようにフォアグラウンドで走り続けるペインがあるタブでは、
  操作していなくてもアプリがアイドルに到達しなかった（#786 / #801 / #803 で削った
  毎フレームの固定費がここで復活していた）。脈動を有限回（2 秒 × 3）で終わらせ、
  以後は色だけの静的表示にした。実測: 出力ゼロのペイン 1 枚で tako 自身の CPU が
  **19.09% → 2.93%**。走り始めの合図は残るので「いま何か走っている」は従来どおり分かる
- [Changed] The pulsing dot on a running tab now pulses only while a command *starts*,
  instead of forever (#945). GPUI requests a new frame on every frame while an animation
  is running, so a tab holding a long-lived foreground process — any coding agent — kept
  the whole app off its idle path and re-paid the per-frame cost that #786 / #801 / #803
  had removed. The dot now pulses three times and then stays a solid color. Measured on an
  otherwise silent pane: tako's own CPU dropped from **19.09% to 2.93%**.

### Fixed

- [修正] タブを切り替えた瞬間に端末がリサイズされる問題を根治（裏タブのペインを
  「表に出たときの寸法」へ合わせる）(#932)。#647 は非表示ペインへ**セル寸法**の変更を
  届けるようになったが、使う領域が「最後に描かれたときの領域」だったため、
  ウィンドウ寸法・サイドバー幅・バナーの出入りといった**幾何の変更**は届かず、
  そのタブを表に出した瞬間に初めてリサイズ = SIGWINCH が飛んでいた（中の TUI が
  画面を作り直すので切り替えのたびに描画が乱れる）。実測: 裏 116x37 / 表 88x33 →
  表に出した瞬間 88x33 へ変化していたのが、追従後は最初から 88x33 で
  **切り替えの瞬間に 1 度も変わらない**
- [Fixed] Terminals no longer resize at the moment you switch tabs (#932). #647 started
  pushing *cell size* changes to off-screen panes, but it used each pane's last rendered
  rect, so *geometry* changes (window size, sidebar width, banners) never reached them:
  the pane was resized — sending SIGWINCH into the running program — exactly when its tab
  was brought to the front. Off-screen panes now derive the rect they will get when shown,
  through the same single code path used for the visible tab.

## [0.7.7] - 2026-08-24

Nightly patch release (automated). Changes since v0.7.6:
夜間パッチリリース（自動）。v0.7.6 以降の変更:

- [修正] スターター / welcome のコマンド投入を方言と送達確認つき経路へ寄せる: Windows で押しても何も起きない問題 (#899) (#467) (#931)
- [修正] コマンド解決を実行ファイル探索の境界へ寄せる: Windows で tako / claude / tmux が常に見つからない問題 (#898) (#467) (#929)
- [修正] public リポの現行コードから実ユーザー名・実ホームパスを除去 + 再発防止の番犬を新設 (#927) (#928)
- [修正] install_plan の期待値を計画そのものから作る: セルフテスト項目 119（#868）が Windows で止まる問題 (#920) (#467) (#926)
- [機能追加] リモートからフォルダを開く（SSH 先のワークスペース化）+ 接続失敗の無言化を根治 (#919) (#65) (#924)
- [機能追加] 設定ファイルのスキーマ変更を常に自動マイグレーションにする + 未対応箇所の棚卸しと是正 (#916) (#923)
- [機能追加] handoff をプロジェクト単位へ再設計し旧形式を自動移行する（後任へ渡すのは管轄分だけ） (#922)
- [修正] file URI のドライブレター規則を境界へ一本化: セルフテスト項目 116（#835）が Windows で止まる問題 (#913) (#467) (#921)
- [修正] PowerShell の PATH ブロックが非 ASCII のパスで $PROFILE を壊す問題（#868 / #525） (#890)

## [0.7.6] - 2026-08-23

Nightly patch release (automated). Changes since v0.7.5:
夜間パッチリリース（自動）。v0.7.5 以降の変更:

- [修正] 器が拒否する符号化ペイロードを作らない: セルフテスト項目 101（#749）が Windows で止まる問題 (#906) (#467) (#914)
- [機能追加] ドキュメントサイトに OGP / Twitter Card を追加: ページごとの OG 画像 (#911) (#912)
- [修正] 器つきペインへの送達で非 ASCII が落ちる問題を根治: 打鍵ではなく器の注入口へ入れる (#907) (#467) (#910)
- [修正] スリープ防止ポップオーバーの文言も呼び名で出し分ける: Windows に「Mac」が残る問題 (#905) (#467) (#909)
- [修正] セルフテスト項目 100（#737）を Windows で通す: 器へ打ち込まずファイル駆動 + シェル片は -EncodedCommand (#903) (#467) (#908)
- [修正] 設定画面のスリープ防止タブを能力ベースの表示へ: Windows で macOS 前提の案内が出る問題 (#727) (#467) (#904)
- [修正] tmux の完全一致ターゲットを 1 つの境界へ寄せる: psmux で tako tmux kill が効かない問題 (#866) (#467) (#902)
- [修正] セルフテストが PTY へ書く Enter を CR へ寄せる: 項目 94 が Windows で必ず止まる問題 (#897) (#467) (#901)

## [0.7.5] - 2026-08-22

Nightly patch release (automated). Changes since v0.7.4:
夜間パッチリリース（自動）。v0.7.4 以降の変更:

- [修正] セルフテストのペイン起動コマンドを方言境界へ寄せる: 項目 93 が Windows で必ず止まる問題 (#889) (#467) (#900)
- [修正] ウィンドウ 0 枚の無音終了を根治: 寿命の方針を tako が持つ (#872) (#895)
- [ドキュメント] 作業文脈を #766 完了後の状態へ更新 (#766) (#467) (#894)
- [修正] 器が OSC を素通ししない環境へシェル統合の側路を入れる (#766) (#467) (#891)
- [修正] ホーム解決を paths::home_dir へ一本化する: Windows で `~/` のリンクが効かない問題を根治 (#870) (#467) (#892)
- [修正] PTY へ渡す argv を「1 語 = 1 引数」で届ける: 空白入り cwd でペインが即死する問題を根治 (#884) (#467) (#887)
- [ドキュメント] login_shell_sees が意図的な unix 専用であることをコードに書く（#868 / #877） (#888)
- [修正] 器へ渡す内側コマンドの第 1 語を「引用符の要らない 1 語」にする (#881) (#467) (#886)
- [ドキュメント] 作業文脈を #877 完了後の状態へ更新 (#877) (#467) (#883)
- [修正] agents 走査を子プロセス境界へ寄せる: Windows で必ず失敗していた問題を根治 (#877) (#467) (#882)
- [修正] 実行ペインの起動コマンドをシェル方言境界へ寄せる: Windows で PTY が立たない問題を根治 (#875) (#467) (#879)
- [ドキュメント] #873 の完了記録と並行作業の作法を残す (#873) (#880)
- [リファクタ] シェル方言の判定を ShellDialect に一本化する (#873) (#878)
- [修正] セルフテストの打ち込むコマンドを方言対応にする: Windows のカバレッジ 0 → 項目 92 まで (#865) (#876)
- [修正] エージェント起動コマンドの env 前置きをシェル方言へ寄せる (#867) (#467) (#874)
- [機能追加] tako setup のゼロスタート対応: Claude CLI 未導入の環境から一発で始められるようにする (#868) (#871)
- [修正] Windows 移植スライス 7b: 起動コマンドの送達確認 (#640) (#467) (#869)
- [機能追加] Windows 移植スライス 7: PowerShell シェル統合を main へ移植 (#467) (#855)
- [ドキュメント] スライス 9 マージ後の現在状態へ activeContext を更新 (#467) (#864)
- [機能追加] Windows 移植スライス 9: スリープ防止 / 蓋閉じ継続 / ポート検知 (#467) (#863)
- [修正] セルフテスト項目 110（#803）が高負荷で落ちる原因を測定側で根治 (#858) (#862)
- [機能追加] Windows 移植スライス 5: ウィンドウコントロール + in-window メニューバー (#467) (#860)
- [ドキュメント] #853 の作業履歴を progress.md へ追記 (#859)
- [修正] セルフテストが注入した会話を定期更新が消して #725 項目が詰まる問題を根治 (#853) (#857)
- [機能追加] Windows 移植スライス 4: 入力系（キーボード / IME / フォント / コンソール窓抑止）を main へ移植 (#467) (#852)
- [機能追加] Windows 移植スライス 6: インストーラー / 配布系を main へ移植 (#467) (#851)
- [機能追加] Windows 移植スライス 3: IPC を named pipe 対応にする (#467) (#850)
- [機能追加] Windows 移植スライス 2b: ConPTY の文字コードと copy mode ゲート (#467) (#849)
- [機能追加] Windows 移植スライス 2a: 永続化の器（psmux）を main へ移植 (#467) (#848)
- [機能追加] Windows 移植スライス 1: platform 境界の基盤を main へ移植 (#467) (#845)
- [機能追加] リミット後の自動復帰をプロファイル既定で spawn worker へ適用 (#822) (#846)
- [改善] ビルド出力の .app が Launch Services に重複登録されるのを根治 (#837) (#844)
- [修正] セルフテストのクォート漏れで #600 系が本番 data dir だと確定失敗する問題を根治 (#833) (#843)

## [0.7.4] - 2026-08-19

Nightly patch release (automated). Changes since v0.7.3:
夜間パッチリリース（自動）。v0.7.3 以降の変更:

- [修正] Web ビューペインのちらつきを根治 (#838) (#839)

## [0.7.3] - 2026-08-18

Nightly patch release (automated). Changes since v0.7.2:
夜間パッチリリース（自動）。v0.7.2 以降の変更:

- [機能追加] Finder の「このアプリケーションで開く」で新しいタブが開く (#835) (#836)

## [0.7.2] - 2026-08-16

Nightly patch release (automated). Changes since v0.7.1:
夜間パッチリリース（自動）。v0.7.1 以降の変更:

- [改善] チャットビューの行数比例リークを仮想化で根治 (#830) (#834)
- [改善] ウィンドウ close の失敗を発生源つきで記録する (#828) (#832)
- [修正] ライブリロードの位置保持検査が空振りしていたのを直す (#826) (#831)
- [修正] Markdown プレビューの行数比例リークを仮想化で根治 (#826) (#829)
- [改善] 取り込み経路の支配項（イベント配送）を削減 (#816) (#827)
- [修正] コードプレビューの行数比例リークを仮想化で根治 (#821) (#825)
- [改善] PTY reader の 1 MiB スタックバッファを根治 (#817) (#823)
- [改善] 構文セットを使っている間だけ載せる (#815) (#824)
- [機能追加] 利用上限後のペイン単位の自動復帰 (#813) (#820)

## [0.7.1] - 2026-08-15

Nightly patch release (automated). Changes since v0.7.0:
夜間パッチリリース（自動）。v0.7.0 以降の変更:

- [ドキュメント] v0.7.0 リリースの作業記録を反映

## [0.7.0] - 2026-08-15

安定版。v0.6.0 以降にテスト版チャンネル（夜間パッチ v0.6.1〜v0.6.11）で検証してきた
変更をまとめて安定版として提供する。柱は 3 つ。**ターミナルを初めて触る人でも使える
GUI ライク表示モード**（スターターと claude の会話ビュー）、**描画コストの大幅削減**
（同じ画面を出すのに使う CPU をおよそ 1/4〜1/3 まで落とし、ついでに長年の描画バグを
根治した）、**AI 連携の底上げ**（コマンド提案カード・master の自動引き継ぎ・worker への
確実な指示送達・設定のデバイス間共有）。Windows は引き続き移植の基盤のみで、配布物は
macOS のみ。

Stable release. Consolidates everything validated on the test channel (nightly patches
v0.6.1 through v0.6.11) since v0.6.0. Three pillars: a **GUI-like display mode** that makes
tako usable without knowing a terminal (starter cards and a real conversation view for
Claude panes), a **large cut in rendering cost** (roughly a quarter to a third of the CPU
for the same screen, plus root fixes for long-standing drawing bugs), and **stronger AI
integration** (command suggestion cards, automatic master handoff, reliable instruction
delivery to workers, and configuration sharing across devices). Windows remains groundwork
only; binaries are still macOS-only.

### Highlights / ハイライト

- **GUI ライク表示モード — ターミナルを知らなくても使える** (#691, #694, #702, #715, #716, #717, #718, #719, #720, #725, #737, #739, #745, #746)
  `tako ui-mode gui`（タブバーのボタン / ⌘K パレット / MCP からも）で、何も動いていない
  ペインが「AI チームに任せる / AI と 1 対 1 で話す / コマンド入力へ」の 3 ボタンになり、
  claude と対話中のペインは**会話ビュー**になる。会話ビューはモデル名・状態・コンテキスト
  残量バー・Markdown 描画・ツール / 思考の折りたたみ・入力欄・スラッシュコマンドボタン・
  承認カードを備え、本文はドラッグで選択して ⌘C でコピーできる（発話をまたいで選択可能。
  発話ごと・コードブロックごとのコピーボタンもある）。入力欄は claude TUI の入力行の
  **ミラー**なので、Enter / Shift+Enter・IME・画像ペースト・ゴースト提案が TUI と完全に
  一致し、表示を切り替えても状態がズレない。ペインを作った直後は「準備中…」で覆い、
  direnv のログや起動途中の画面を見せない。**表示レイヤだけの切り替え**で、PTY・tmux
  セッション・実行中プロセスには一切影響しない。
  *A display mode that makes tako usable without terminal knowledge. Idle shell panes turn
  into three buttons ("let an AI team handle it" / "talk with one AI" / "go to the command
  line"), and panes running Claude become a **conversation view** with the model name,
  status, a remaining-context bar, Markdown rendering, collapsible tool/thinking blocks, an
  input box, slash-command buttons, and approval cards. The body is selectable and
  copyable with ⌘C across message boundaries, with per-message and per-code-block copy
  buttons. The input box is a **mirror of Claude's own TUI input line**, so Enter,
  Shift+Enter, IME composition, image pastes, and ghost suggestions behave identically and
  never drift when you switch modes. Freshly created panes are covered by a "preparing"
  screen instead of showing direnv logs and startup noise. This is a **presentation layer
  only** — PTYs, tmux sessions, and running processes are untouched.*
- **描画コストの大幅削減 + 長年の描画バグの根治** (#782, #786, #787, #801, #803)
  端末グリッドを行ごとの div スタックから専用の描画要素へ置き換え、ペイン本体・ペイン
  ヘッダ・各種クロームをそれぞれ独立したキャッシュ単位に切り出し、見えていないペインの
  出力では再描画しないようにした。実測で、裏タブが毎秒 200 行を出し続ける状況の CPU は
  22.3% → 2.6%、17 タブ表示中は 36.65% → 8.94%、1 フレームあたりの命令数は実務的な
  文字密度で 15.68M → 6.42M（−59%）。この置き換えの過程で、**下線（SGR 4）や ⌘ホバーの
  下線が 1 ピクセルも描かれない** (#797)、**全角文字が続く行で最大 1 セルぶん左へ詰まる**
  (#798) という 2 つの実バグも同時に消えた。行の高さをセル高に合わせたので、`g` や `y` の
  下が切れることもなくなっている。
  *Replaced the terminal grid (a stack of per-row divs) with a dedicated element, split the
  pane body, pane header, and chrome into independent cache units, and stopped repainting
  for output from panes you cannot see. Measured: CPU while a background tab prints 200
  lines/second went from 22.3% to 2.6%; with 17 tabs open, 36.65% to 8.94%; instructions per
  frame at realistic text density from 15.68M to 6.42M (−59%). The rewrite also eliminated
  two real bugs: **underlines (SGR 4) and ⌘-hover underlines never drawing at all** (#797)
  and **rows of full-width characters drifting up to one cell to the left** (#798). Row
  height now matches cell height, so descenders are no longer clipped.*
- **AI が提示するコマンドをワンクリックで実行できる** (#666, #681, #703)
  AI が会話の中に書いたコマンドは、TUI の物理的な折り返しのせいでコピーすると壊れる。
  `tako show-command`（MCP `tako_show_command`）で提示されたコマンドは、**ターミナル領域を
  縮めて作った専用の帯**にカードとして出る（会話・入力欄・フッターと重ならない）。
  カードにはコピーと「新しいペインで実行」が付き、GUI モードの会話ビューでは Markdown の
  コードブロック風にインライン表示される。
  *Commands an AI writes into the conversation break when copied, because the TUI wraps them
  physically. `tako show-command` (MCP `tako_show_command`) presents them as a card in a
  **dedicated strip carved out of the terminal area**, so it never overlaps the conversation,
  input box, or footer. Each card offers copy and "run in a new pane"; in the GUI-mode
  conversation view it renders inline like a Markdown code block.*
- **AI 系設定を git 1 本でデバイス間共有** (#513, #793)
  `tako config init` / `link` / `push` / `pull`（MCP `tako_config_share` と 1:1）で、claude の
  グローバル指示（`CLAUDE.md` / snippets / commands / templates）と tako の宣言的設定
  （profiles / projects / accounts / local-rules / settings）を 1 つの git リポジトリで
  mac ⇔ Windows 共有する。共有対象は**ホワイトリスト**で、載っていないものは共有しない。
  秘匿情報（token / credentials / `.claude.json`）とマシンローカル状態（layout / sessions /
  workers / ペインログ）は構造的に除外し、ファイル単位で切れないもの（アカウントの
  `config_dir`・profile の `env`）はフィールド単位で落とす。絶対パスはホーム部分が `~` に
  正規化されるので、ホームの位置が違うマシン同士でも同じリポジトリを使える。`tako setup` は
  配線済みかどうかを検出して案内するだけで、**質問は増えない**。
  *Share your AI configuration across devices through one git repository: `tako config init`
  / `link` / `push` / `pull` (1:1 with MCP `tako_config_share`). It covers Claude's global
  instructions (`CLAUDE.md`, snippets, commands, templates) and tako's declarative settings
  (profiles, projects, accounts, local rules, settings). What gets shared is an
  **allow-list** — anything not in the catalog is never shared. Secrets (tokens,
  credentials, `.claude.json`) and machine-local state (layout, sessions, workers, pane
  logs) are excluded structurally, and parts that cannot be split by file (an account's
  `config_dir`, a profile's `env`) are stripped field by field. Absolute paths store the
  home prefix as `~`, so machines with different home locations can share one repository.
  `tako setup` only detects and explains it — no new questions.*
- **master が自分で引き継ぐ** (#749, #792)
  master のコンテキスト使用率が閾値（既定 60%。プロファイルごとに 50〜60% で設定可）を
  超えると、tako が master へ引き継ぎ開始を指示する（ユーザーの操作は不要）。master は
  引き継ぎファイルを最新化して後任を同じタブ・同じ role・同じプロファイルで立ち上げ、
  後任が引き継ぎファイルと実態を突き合わせ、**前任の入力欄にユーザーの未送達の指示が
  残っていないかを確認してから**前任ペインを閉じる。呼び出し自体は前任を閉じないので、
  後任の起動に失敗しても master を失わない。引き継ぎファイルは「知識（マシン非依存）」と
  「実行状態（このマシン限定）」の 2 節に分かれ、後任は前者を前提にしてよく後者は必ず
  実態で確認する（旧書式もそのまま読める）。`/compact` の自動実行は「話が通じなくなる」
  ため意図的に採らない。設定は `tako orchestrator profiles set <名前> --ctx-threshold 55` /
  `--auto-handoff false`（設定画面 → プロファイルでも同じ）。
  *When the master's context usage crosses a threshold (default 60%, settable per profile
  between 50% and 60%), tako tells it to hand over — no user action needed. The master
  refreshes its handoff file and starts a successor in the same tab with the same role and
  profile; the successor cross-checks the handoff against reality, verifies that no
  undelivered user instruction is sitting in the predecessor's input box, and only then
  closes the old pane. The call itself never closes it, so a failed successor launch cannot
  leave you without a master. Handoff files are now split into "knowledge (machine
  independent)" and "runtime state (this machine only)" — the successor may trust the first
  and must re-verify the second (old single-section files still work). `/compact` is
  deliberately not automated: it makes the conversation lose its thread.*
- **worker への指示が生成中でも取りこぼされない** (#790)
  claude worker への送達を 2 層にした。第 1 層は claude の Cross-Session Messaging
  （受信箱へ直送）で、画面解析もキー操作も伴わないため生成中でもキューに入って取りこぼさず、
  長文もバイト等価に 1 回で届く（実測 43,449 バイト）。使えない場合だけ従来のキー操作経路
  （貼り付け + 分離 Enter + 空検証）へ落ちる。対象は**エージェント管理下の worker 宛だけ**で、
  master への指示や承認の代行は従来経路のまま。
  *Instruction delivery to Claude workers is now two-layered. The first layer is Claude's
  Cross-Session Messaging (straight into the recipient's inbox): no screen scraping, no
  synthetic keystrokes, so messages queue correctly even mid-generation and long payloads
  arrive byte-for-byte in one shot (43,449 bytes measured). It falls back to the previous
  keystroke path only when unavailable. This applies **only to agent-managed workers** —
  instructions and approvals aimed at a master still use the old path.*
- **アップデートの見せ方を作り直した** (#616, #690)
  下部ステータスバーの表示を廃止し、上部の通知カード（× で閉じるとそのバージョンは以後
  通知しない）と専用画面（現在 / 最新バージョン・チャンネル・配布系統・配布物・
  リリースノート・「今すぐ更新」）に分けた。リリースノートは Markdown で描画され
  （見出し / 表 / リスト / コード / 引用）、リンクは ⌘+クリックで既定ブラウザへ。
  *Reworked how updates surface: removed the status-bar entry in favour of a dismissible
  top notification card (dismissing hides that version for good) and a dedicated screen
  showing current and latest version, channel, install origin, assets, release notes, and
  an update button. Release notes render as Markdown (headings, tables, lists, code,
  quotes), with ⌘-click opening links in your default browser.*

### Added / 機能追加

- **GUI モードのスターターでプロファイルを選んで起動できる** (#739)
  プロファイルが 2 つ以上あるとき「AI チームに任せる」「AI と 1 対 1 で話す」のカード右端に
  **▾** が出て、一覧から選ぶと `tako master -<名前>` がシェルに入る。各項目には担当
  プロジェクト / 起動フォルダ / モデルの手がかりが付く。カード本体のクリックは従来どおり
  既定プロファイル（引数なし）の起動。あわせて会話ビューのヘッダに、コンテキスト使用率が
  80% を超えたときだけ**押せる**「/compact で会話を軽くする」ヒントが出る。
  *When you have more than one profile, the "let an AI team handle it" and "talk with one
  AI" cards get a **▾** on their right edge; picking from the dropdown writes
  `tako master -<name>` into the shell, with each entry hinting at its assigned projects,
  working folder, or model. Clicking the card itself still launches the default profile with
  no arguments. The conversation header also grows a clickable "shrink the conversation with
  /compact" hint once context usage passes 80%.*
- **Markdown プレビュー: リンクの ⌘+クリックとコードブロックのコピーボタン** (#680)
  `[text](url)` を ⌘+ホバーで下線表示し、⌘+クリックで既定ブラウザへ（**http / https のみ**
  開き、`javascript:` や相対パス・アンカーは拒否する）。コードブロックの右上には装飾なしの
  全文をクリップボードへ送るコピーボタンが常時出る。CLI `tako preview-link-list` /
  `preview-follow-link` / `preview-copy-code` と MCP も 1:1。
  *⌘-hover underlines `[text](url)` links and ⌘-click opens them in the default browser
  (**http/https only**; `javascript:`, relative paths, and anchors are refused). Code blocks
  get an always-visible copy button that copies the undecorated full text. Exposed 1:1 as
  `tako preview-link-list` / `preview-follow-link` / `preview-copy-code` and MCP tools.*
- **設定画面に「プロファイル」タブ** (#721)
  master / solo の起動プロファイルを GUI のフォームで編集できる（一覧・全項目編集・
  新規 / 複製 / 削除。`default` は削除不可）。書き込みは CLI・MCP と同じ経路を通るので、
  検証も排他制御もそのまま効く。
  *Edit master and solo launch profiles from a form in Settings (list, all fields, create /
  copy / delete; `default` cannot be deleted). Writes go through the same path as the CLI
  and MCP, so validation and locking apply unchanged.*
- **[macOS] 入力予測の確定キーを案内し、Tab でも確定できるようにした** (#614)
  予測（薄いゴースト）の直後に `[→ か Tab で確定]` を薄く出す（既定 10 回で自動的に消え、
  `tako autosuggest hint off` で恒久 OFF にもできる）。加えて**予測が出ていてカーソルが
  行末にあるときだけ** Tab が確定になり、それ以外の Tab は従来どおりの補完のまま
  （補完メニューの巡回も不変）。Tab 確定は `tako autosuggest tab off` で切れる。
  *Shows how to accept an input suggestion right where you are looking: a faded
  `[→ or Tab to accept]` hint after the ghost text (fades away after 10 command lines, and
  can be turned off permanently). Tab now also accepts — but only while a suggestion is
  shown and the cursor is at the end of the line, so ordinary Tab completion and
  completion-menu cycling are untouched.*
- **[macOS] Finder の「このアプリケーションで開く」に tako が出る** (#708)
  フォルダやファイルを tako で開けるようになった。
  *tako now appears in Finder's "Open With" menu for folders and files.*
- **worker の選択肢ダイアログを種別つきで扱えるようにした** (#748)
  ダイアログの検知を文言依存から**構造検知**（番号つき / 番号なしの 2 経路）へ一般化し、
  種別（permission / trust / bypass / usage limit / plan 確認 / 選択）を分類して
  `WORKER_DIALOG` イベントと `choice_dialog` フィールドで公開する。応答
  （`tako orchestrator respond`）は `--choice` を省略すると**送信せず選択肢の構造だけ返す**
  ので下見ができる。番号つきは番号キーだけで確定し、番号なしは矢印移動 + ラベル一致検証 +
  Enter。**ダイアログ表示中の `tako send` は選択肢つきのエラーで拒否される**（テキストが
  キー操作として食われ、数字が選択を確定させてしまうため）。
  *Dialog detection is now **structural** (numbered and unnumbered variants) instead of
  wording-based, and classified by kind (permission, trust, bypass, usage limit, plan
  confirmation, selection), surfaced through a `WORKER_DIALOG` event and a `choice_dialog`
  field. `tako orchestrator respond` without `--choice` returns the structure without
  answering, so an agent can look before it leaps. Numbered dialogs are confirmed with the
  number key alone; unnumbered ones move with arrows, verify the highlighted label, then
  press Enter. **`tako send` is refused while a dialog is up** (text would be eaten as key
  presses and digits would confirm a choice), with the available options in the error.*

### Changed / 改善

- **Markdown プレビューの全面的な品質改善** (#656)
  GFM のテーブルを罫線・ヘッダ帯・ゼブラ・列アライメント付きの表として描画し、見出し
  6 段・コード・インラインコード・引用のネスト・リストマーカー・チェックボックスの配色を
  作り直した。選択とコピーはセル単位で解決する。
  *GFM tables now render as real tables (rules, header band, zebra striping, column
  alignment), and the palette for all six heading levels, code, inline code, nested quotes,
  list markers, and checkboxes was rebuilt. Selection and copy resolve per cell.*
- **CPU を使わない待ち方へ** (#772, #779)
  stale binary 検知（メインスレッドを毎 tick 289〜323ms 専有していた）を 1 回の
  プロセススナップショットに束ねて背景へ出し、変化があったときだけ走査するようにした。
  sleep guard も同じスナップショットを共有し、`ps` の起動を 75 秒あたり 34 回から 3 回へ
  （約 91% 減）落とした。
  *Stale-binary detection (which occupied the main thread for 289–323ms every tick) now
  batches into a single process snapshot, runs in the background, and only rescans on
  change. Sleep guard shares that snapshot, cutting `ps` launches from 34 per 75 seconds to
  3 (about 91% fewer).*
- **リモート画面の情報設計と操作** (#615, #621)
  スマホ側のペイン選択画面を、**そのペインで今何が起きているか**が分かる設計に変えた
  （タブでのグループ化・状態ピル・チップ・実際の直近出力のプレビュー。従来は最も古い履歴の
  先頭を表示していた）。Mac 側のリモートカードはステータスバーのインジケータ直上に
  アンカーされ、起動 / 停止のトグルが付いた。
  *The phone-side pane picker now shows **what is actually happening in each pane** (tab
  grouping, status pills, chips, and a preview of the latest output — it used to show the
  top of the oldest history). On the Mac, the remote card is anchored directly above its
  status-bar indicator and carries a start/stop toggle.*
- **サイドバー幅のクランプ規則を全経路で統一** (#789)
  ドラッグは「ウィンドウ幅の 50%」、CLI / MCP は「固定 600px」と上限が食い違っていた。
  規則を 1 か所（下限 120px / 上限 = ビューポート幅の 50%）へ統一し、状態は要求値・描画は
  実効幅に分けたので、ウィンドウを狭めても要求値は書き換わらず、広げ直せば元の幅へ戻る。
  *The drag handle clamped to 50% of the window while the CLI and MCP clamped to a fixed
  600px. Both now share one rule (min 120px, max 50% of the viewport), and the requested
  width is kept separate from the effective width — shrinking the window no longer
  overwrites your preference, and widening it restores the original size.*
- **ドキュメントサイトを v0.6.0 に追従** (#620)
  CLI 68 コマンド / MCP 128 ツールを全数機械照合して記述の食い違いをゼロにし、
  リリースページを再構成、モバイル表示の実バグも直した。
  *Documentation site brought in line with v0.6.0: all 68 CLI commands and 128 MCP tools
  machine-checked against the implementation, the releases page restructured, and real
  mobile-layout bugs fixed.*

### Fixed / 修正

- **PC 再起動後に master ペインだけ claude の会話が復元されない** (#652)
  *After a machine restart, the master pane alone failed to resume its Claude conversation.*
- **コードプレビューの構文色がライトテーマで読めない** (#669)
  シンタックスハイライトのテーマがダーク固定で、ライトでも暗い配色のままだった（既定の
  文字色でコントラスト比 1.36:1）。色の変換を 1 か所に集め、Markdown 内のコードブロックと
  同じ輝度クランプを通すようにした。描画時に変換するのでテーマ切り替えに即応し、ダークは
  従来の色のまま。
  *Syntax highlighting was pinned to a dark theme and stayed dark in light mode (1.36:1
  contrast for default text). Colour conversion is now centralized and passes through the
  same luminance clamp as Markdown code blocks; it converts at draw time, so theme switches
  apply instantly and dark mode is unchanged.*
- **git パネルのボタンが押しても反応しない** (#496)
  ルート要素の「押したら入力フォーカスを落とす」処理がボタンの押下と同時に状態を消して
  いたため、コンフリクト解消エージェントの 3 択・ブランチ名の入力欄・作成・キャンセルが
  **マージ以来 1 度も発火していなかった**。
  *A global "clear text-input focus on mouse down" handler wiped the state at the moment of
  the press, so the conflict-resolver's three agent buttons, the branch-name field, create,
  and cancel had **never fired since they were merged**.*
- **設定画面のプロファイルタブの描画が崩れる** (#738)
  幅が auto の親の中で折り返すチップ群の幅が確定せず、チップが 1 個ずつ縦に折り返される
  一方で行の高さは 1 行ぶんで見積もられ、伸びたチップ群が次の行に重なっていた。
  *Wrapping chip groups inside an auto-width parent never resolved their width: chips
  wrapped one per line while the row height was still measured as a single line, so the
  overflowing chips overlapped the row below.*
- **チャットビューの入力欄の重なり、IME の位置ズレ、md テーブルの崩れ、画像つき発話の二重表示** (#737, #745, #746)
  claude が空欄のときも箱の中に自前の案内文を描くのに、tako がプレースホルダを重ねて
  読めなくしていた（#737）。IME の未確定文字列と候補ウィンドウは、チャット表示が
  ターミナルグリッドを描かないのにセル座標をアンカーにしていたため画面上のどこも指して
  いなかった（#737）。テーブルはセルが幅 0 まで潰れて 1 文字ずつ縦積みになっていた
  （#745）。画像を添付した発話は、楽観表示が生の TUI 入力行を持っていたため transcript と
  突き合わず二重に見えていた（#746）。
  *Claude draws its own hint text inside the input box even when empty, and tako layered a
  placeholder on top of it (#737). IME preedit text and the candidate window anchored to
  terminal cell coordinates, which point nowhere in a chat view that draws no grid (#737).
  Table cells collapsed to zero width and stacked one character per line (#745). Messages
  with image attachments appeared twice because the optimistic echo held the raw TUI input
  line and never matched the transcript (#746).*
- **IME の位置と選択座標がずれる** (#781)
  stale claude バナーの高さ 28px がテキスト領域の会計から漏れていた。この会計は PTY の
  行数・マウス座標の変換・IME のアンカーの共通の正なので、バナーが出た瞬間に全部ずれていた。
  *The 28px stale-Claude banner was missing from the text-area accounting — the single
  source of truth for PTY row count, mouse coordinate mapping, and the IME anchor — so
  everything shifted the moment the banner appeared.*
- **縦に積む UI の表示中にペインの PTY 行数が可視行数を超える** (#684)
  *Panes reported more PTY rows than were visible while a stacked UI element (a banner) was
  shown.*
- **消えたタブ・ペインを「いつ何で失ったか」で追えるようにした** (#770)
  「再起動で消えた」と報告された喪失が、実際にはタブの × による close だったと実測で確定
  した。セッション kill とタブ close は**発生源つき**で `persist.log` に残るようになり
  （`close:gui-tab` / `close:gui` / `close:kbd` / `close:dispatch` / `exit`）、バックアップ
  世代は「tmux セッションを持つペインが消える保存」でも作られるようになった。
  *A loss reported as "everything vanished on restart" turned out to be a tab closed with
  its × button. Session kills and tab closes are now recorded in `persist.log` **with their
  origin** (`close:gui-tab`, `close:gui`, `close:kbd`, `close:dispatch`, `exit`), and a
  backup generation is taken whenever a save would drop a pane that owns a tmux session.*
- **引き継ぎ後の master が worker の設定で起動し、default プロファイル扱いになる** (#761)
  後任の起動が worker 用のコマンド構築を通っていたため、モデルも role も間違ったうえに
  master の system prompt が一切付いていなかった。
  *The successor was built with the worker command path, so it launched with the wrong
  model and role — and with no master system prompt at all.*
- **後続の送信に失敗すると worker の初回プロンプトが「未達」に化ける** (#778)
  *A failed follow-up send flipped the worker's initial prompt back to "undelivered".*
- **Code Runner の `tako run` が指定していないのに新ペインへフォーカスを奪う** (#676)
  *`tako run` stole focus to the new pane even when focus was not requested.*
- **[macOS] リモートサーバーの停止で子プロセスが defunct として残り、停止が失敗と報告される** (#619)
  起動した daemon の終了ステータスを誰も回収していなかった。`kill(pid, 0)` はゾンビにも
  成功するため、実際には停止できているのに「SIGTERM 後 5 秒経っても終了しない」を返していた。
  *Nobody reaped the daemon's exit status, and since `kill(pid, 0)` succeeds for zombies the
  stop path reported "did not exit 5 seconds after SIGTERM" even though the server had
  actually stopped.*
- **worker レジストリに死んだエントリが残り続ける** (#658)
  ペインも器も見えなくなった worker に印を付け、5 分続いたものだけを closed として畳む
  ようにした（`resume` コマンドは畳んだ後も引ける）。
  *Workers whose pane and session are both gone are now marked and, only after five minutes,
  folded into `closed` — their resume command stays available afterwards.*

### Internal / 開発基盤

- **MCP 実装の整理** (#750, #752, #755)
  公開契約の完全スナップショットを先に入れてから、133 ツールの実装を catalog / request /
  HTTP / tests / facade へ責務別に分割した（スナップショットの差分ゼロで挙動不変を担保）。
  *Added a complete snapshot of the public contract first, then split the 133-tool
  implementation into catalog, request, HTTP, tests, and facade modules — with a zero-diff
  snapshot proving behaviour did not change.*
- **テストの信頼性** (#608, #625, #668, #796, #799)
  表示言語のグローバルを共有していたテストの競合、並列負荷下で落ちる tmux e2e の 3 根因、
  visual-test のインデントガイド節（ここで止まって以降の全節が実行されていなかった）、
  隔離セルフテストの「固定時間で待つ」26 組を潰した。端末グリッドの描画には、置き換えの
  前後を突き合わせるための visual-test 回帰検出網を先に用意した。
  *Fixed a shared display-language global that made tests race, three root causes of tmux
  e2e flaking under parallel load, the visual-test indent-guide section (which stopped the
  run so later sections never executed), and 26 fixed-duration waits in the isolated
  self-test. A visual-test regression net for the terminal grid was built before the grid
  rewrite so before/after could be compared.*
- GUI ライク表示モードの詳細仕様 (#691)、GUI モードのスターター導線と過渡期の扱い (#720)。
  *Detailed specification for the GUI-like display mode, plus starter guidance and
  transition handling.*

## [0.6.11] - 2026-08-12

Nightly patch release (automated). Changes since v0.6.10:
夜間パッチリリース（自動）。v0.6.10 以降の変更:

- [改善] 端末グリッドを専用 Element へ置き換え、下線と全角の描画ずれを根治 (#787) (#800)
- [改善] 端末グリッド描画の visual-test 回帰検出網を整備 (#787) (#799)

## [0.6.10] - 2026-08-11

Nightly patch release (automated). Changes since v0.6.9:
夜間パッチリリース（自動）。v0.6.9 以降の変更:

- [機能追加] setup に設定共有の検出・案内・代行導線を追加 (#793) (#794)
- [修正] git パネルのクリックが一括 dismiss に食われる問題を根治 (#496) (#795)

## [0.6.9] - 2026-08-09

Nightly patch release (automated). Changes since v0.6.8:
夜間パッチリリース（自動）。v0.6.8 以降の変更:

- [ドキュメント] Issue 786 の完了状況を記録 (#786)
- [改善] クローム・ペインをビュー単位のキャッシュへ (#786) (#788)
- [改善] 見えないペインの出力で全面再描画しない (#782) (#785)

## [0.6.8] - 2026-08-07

Nightly patch release (automated). Changes since v0.6.7:
夜間パッチリリース（自動）。v0.6.7 以降の変更:

- [ドキュメント] Issue 779 の完了状況を記録 (#779)
- [改善] sleep guard の ps 起動を変化時だけに削減 (#779) (#783)
- [修正] IME 位置・選択座標のズレを根治: stale claude バナーの高さをテキスト領域の会計に含める (#781) (#784)
- [ドキュメント] Issue 778 の完了状況を記録 (#778)
- [修正] 後続send失敗のprompt未達誤検知を防ぐ (#778) (#780)
- [ドキュメント] #770 のセルフテスト項目番号を 104 に訂正（#772 との rebase で繰り上がったため）
- [修正] 再起動ではなくタブ × close だった喪失を、記録と復旧の両面で根治 (#770) (#774)
- [修正] stale binary 検知がメインスレッドを毎 tick 400ms 専有する問題を根治 (#772) (#773)

## [0.6.7] - 2026-08-06

Nightly patch release (automated). Changes since v0.6.6:
夜間パッチリリース（自動）。v0.6.6 以降の変更:

- [修正] handoff の後任 master が worker 設定で起動し default プロファイル扱いになる問題を根治 (#761) (#767)

## [0.6.6] - 2026-08-05

Nightly patch release (automated). Changes since v0.6.5:
夜間パッチリリース（自動）。v0.6.5 以降の変更:

- [修正] 実 claude 引き継ぎ e2e（101c）を実際に測れるようにする (#749) (#756)
- [リファクタ] MCP 133ツール実装を責務別モジュールへ分割 (#755)
- [改善] worker の選択肢ダイアログ対応を総点検: 構造検知・種別つき通知・安全な応答 (#748) (#753)
- [リファクタ] MCP公開契約の完全スナップショットを追加 (#750) (#752)
- [機能追加] master の自動ハンドオフ: ctx 閾値超過で引き継ぎ → 後任が前任ペインを閉じる (#749) (#751)
- [修正] チャットビューの md テーブル崩れと画像つき発話の二重表示を根治 (#745) (#746) (#747)
- [ドキュメント] activeContext: #691 全フェーズ完了と 08-04 再起動を反映 (#744)

## [0.6.5] - 2026-08-04

Nightly patch release (automated). Changes since v0.6.4:
夜間パッチリリース（自動）。v0.6.4 以降の変更:

- [修正] チャット入力欄の重なり描画と IME 位置ズレを根治 + 作業中インジケータ / 枠 / busy 中の吹き出し (#737) (#742)
- [機能追加] GUI モード G4: スターターのプロファイル選択 ▾ + ctx 80% の /compact ヒント (#739) (#740)
- [修正] 設定画面プロファイルタブの描画崩壊を根治（チップ群の折り返し幅を確定させる）(#738) (#741)
- [機能追加] チャットビューのテキスト選択・コピー（ドラッグ選択 + ⌘C + 発話単位のコピーボタン）(#725) (#736)
- [改善] チャット入力欄を TUI ミラー + 打鍵パススルーにする（#718 箱サイズ + #719 完成度 6 点） (#735)

## [0.6.4] - 2026-08-02

Nightly patch release (automated). Changes since v0.6.3:
夜間パッチリリース（自動）。v0.6.3 以降の変更:

- [改善] GUI モード: 起動の過渡期に生ターミナルを見せない + スターターに setup 導線 (#720) (#734)
- [機能追加] 設定画面に「プロファイル」タブを新設し master / solo の起動設定を GUI 編集できるようにする (#721) (#731)
- [改善] GUI モードのチャットビューを「ちゃんと使えるチャット UI」にする（#715 表示品質 + #716 G3） (#717)
- [ドキュメント] activeContext: 08-01 完了分（#702/#703/#708）を反映 (#714)
- [機能追加] GUI モード G2: チャットビュー（読み取り）— claude ペインを会話表示にする (#702) (#713)
- [機能追加] Finder の「このアプリケーションで開く」に tako を出す (#708) (#711)
- [改善] コマンド提案カードをターミナル領域を縮めた専用帯へ移し会話との重なりをゼロにする (#703) (#710)

## [0.6.3] - 2026-08-01

Nightly patch release (automated). Changes since v0.6.2:
夜間パッチリリース（自動）。v0.6.2 以降の変更:

- [改善] アップデート詳細のリリースノートを Markdown レンダリング表示へ (#690) (#699)
- [機能追加] GUI ライク表示モード G1: モード基盤 + スターター 3 ボタン (#694) (#698)
- [ドキュメント] progress: #691 GUI モード仕様策定の完了を記録
- [ドキュメント] GUI ライク表示モード（初心者向け UI）の詳細仕様書 (#691) (#692)
- [ドキュメント] activeContext: 07-30 バッチのユーザー目視 OK を反映

## [0.6.2] - 2026-07-31

Nightly patch release (automated). Changes since v0.6.1:
夜間パッチリリース（自動）。v0.6.1 以降の変更:

- [修正] 縦に積む UI（バナー等）表示中にペインの PTY 行数が可視行数を超える問題を根治 (#684) (#689)
- [ドキュメント] #680 完了を progress / activeContext へ反映
- [機能追加] Markdown プレビュー: リンクの ⌘+クリックでブラウザ起動 + コードブロックのコピーボタン (#680) (#685)
- [改善] コマンド提案カードを会話内容にアンカーするインライン表示へ (#681) (#683)
- [ドキュメント] activeContext を 07-30 バッチ完了状態へ更新
- [修正] Code Runner の tako run が focus 未指定でも新ペインへフォーカスを奪う問題を根治 (#676) (#678)
- [修正] コードプレビュー（非 md）の構文色がライトテーマで読めない問題を根治 (#669) (#677)
- [機能追加] AI コマンド提案カード: AI が提示するコマンドをワンクリックコピー / 新規ペイン実行できる (#666) (#675)
- [修正] visual-test のインデントガイド節が main で失敗し以降の全節が止まる問題を根治 (#668) (#673)
- [改善] Markdown プレビューの高品質化: GFM テーブル対応 + 配色・タイポグラフィ全面改善 (#656) (#667)
- [修正] PC 再起動後に master ペインだけ claude 会話が resume されない問題を根治 (#652) (#661)

## [0.6.1] - 2026-07-28

Nightly patch release (automated). Changes since v0.6.0:
夜間パッチリリース（自動）。v0.6.0 以降の変更:

- [ドキュメント] progress に夜間バッチ 9 件の作業記録を追記
- [修正] tmux e2e が並列負荷下でランダムに落ちる 3 つの根因を潰す (#625) (#637)
- [機能追加] AI 系設定を git でデバイス間共有する tako config を追加 (#513) (#636)
- [修正] remote daemon 停止後の defunct 残留と停止の誤失敗を根治 (#619) (#631)
- [改善] リモートのペイン選択画面を「どれがどれだか分かる」情報設計へ (#621) (#629)
- [改善] アップデート UI を上部通知カード + 専用画面へ移設 (#616) (#630)
- [修正] platform::support のテストが表示言語グローバルの競合で確率的に落ちる (#608) (#624)
- [ドキュメント] docs サイトを v0.6.0 追従 + 親しみやすさを残したモダン化 (#620) (#626)
- [機能追加] 入力予測の確定キーを案内し Tab でも確定できるようにする (#614) (#622)
- [改善] リモートカードをインジケータ直上へアンカー + 起動/停止トグル化 (#615) (#618)
- [ドキュメント] v0.6.0 リリースの作業記録を反映

## [0.6.0] - 2026-07-27

安定版ローンチ。v0.5.x のテスト版チャンネル（夜間パッチリリース）で検証してきた
リモート接続の全面刷新を安定版として正式に提供し、オーケストレーションの検知系
（idle / busy / permission / プロンプト送達）を根治した上で、初回起動の導線・入力予測・
ファイルツリー・git タブの使い勝手を整えた。Windows は移植の基盤（抽象境界・対応
マトリクス・クロスコンパイル CI）までが入っており、配布物は引き続き macOS のみ。

Stable launch. Delivers the fully rebuilt remote access stack — validated on the v0.5.x
test channel — as a stable release, fixes the orchestration detection layer (idle / busy /
permission / prompt delivery) at its root, and polishes first-run guidance, input
prediction, the file tree, and the git tab. Windows support lands as groundwork only
(abstraction boundaries, support matrix, cross-compile CI); binaries remain macOS-only.

### Highlights / ハイライト

- **リモート接続を Tailscale Serve 一本へ刷新** (#282, #283, #286, #287)
  Quick Tunnel / Cloudflare relay Worker / 公開 Pages PWA / URL 埋め込みトークン /
  平文 LAN モードを全廃し、tailnet 内限定（WireGuard による E2E 暗号化）の恒久固定 URL に
  一本化した。認証は Tailscale identity 検証 + 機器ペアリング（Mac 上の承認ダイアログ・
  role 別権限・失効の即時反映）の二層で、PWA は daemon が同一 origin で配信し、
  daemon の listen は Unix domain socket。導入は v0.5.x 系で段階的に行ったため、
  **安定版チャンネルでの提供は v0.6.0 が初**になる。設定は `tako remote setup` で完結する。
  *Rebuilt remote access on Tailscale Serve alone: Quick Tunnel, the Cloudflare relay
  Worker, public Pages hosting, URL-embedded tokens, and the plaintext LAN mode are gone.
  Access is tailnet-only over a permanent fixed URL, authenticated in two layers
  (Tailscale identity + device pairing approved on the Mac, with per-role permissions and
  instant revocation). The PWA is served same-origin by the daemon, which listens on a
  Unix domain socket. Rolled out during the v0.5.x cycle; v0.6.0 is the first stable
  release to carry it. Set up with `tako remote setup`.*
- **オーケストレーションの検知系を根治** (#571, #572, #577, #530)
  worker が終わっても `WORKER_IDLE` が出ない / 生成中に人間が打った指示が消える /
  permission ダイアログ待ちが「質問」に化ける / spawn の初期プロンプトが届かない、の
  4 つを実測で原因まで降りて修正した。いずれも文言の当てずっぽうではなく、画面の
  dim 属性・ダイアログが入力欄を奪っているか・画面が変化しているかといった構造で
  判定するようにしてある。
  *Fixed the four long-standing orchestration detection defects at their root:
  `WORKER_IDLE` never firing, instructions typed during generation being swallowed,
  permission prompts being reported as questions, and spawn prompts not arriving.
  Detection is now structural (dim attributes, whether a dialog owns the input line,
  whether the screen is still changing) instead of matching UI wording.*
- [macOS] **入力予測（ゴーストテキスト）を既定 ON で提供** (#600)
  zsh-autosuggestions v0.7.1（MIT）をバージョン固定で同梱し、tako が開いた zsh にだけ
  シェル統合経路で読み込ませる。`~/.zshrc` は書き換えないので tako の外の zsh は不変。
  右矢印で確定、切替は設定画面 / `tako autosuggest` / MCP `tako_autosuggest` の 3 経路で、
  稼働中のペインにも次のプロンプトから効く。
  *Ships history-based input prediction (zsh-autosuggestions v0.7.1, MIT) enabled by
  default, injected only into shells tako itself starts — `~/.zshrc` is never modified.
  Accept with the right arrow; toggle from settings, `tako autosuggest`, or MCP.*
- **初回起動の導線を新設** (#549, #601)
  初回だけタブバー直下にウェルカムバナーを出し、`tako setup` → `tako master` をその場で
  実行できるようにした（⌘K パレットにも常設）。あわせて tako 内のシェルには同梱 CLI の
  ディレクトリを PATH 末尾へ自動で足すので、zip 配布でも `tako` コマンドがすぐ通る。
  *Adds a first-run welcome banner (and matching ⌘K palette entries) that runs
  `tako setup` / `tako master` in place, and auto-appends the bundled CLI directory to
  `PATH` inside tako's own shells so `tako` works even from the zip distribution.*
- **AI 自動リネームの品質改善** (#552)
  同一タブは 5 分以内に再命名しない、`command not found` のような一時的失敗を材料から
  除く、出力言語を UI の表示言語に固定する（簡体字の混入を字種検査で防ぐ）、自動命名
  直後にピン印ワンクリックで固定できる、の 4 点。
  *Auto-renaming now rate-limits per tab (5 min), ignores transient failures such as
  `command not found`, pins its output language to the UI language, and can be locked in
  with a single click right after it renames.*
- **リリース配布と更新チェックのプラットフォーム対応** (#594, #595)
  リリースノートを実アセットから自動生成する方式に変え（ダウンロード表・OS 別の
  インストール手順・Known limitations (Windows)）、更新チェックの候補を「最新リリース」
  から「自分の OS 向けアセットを含む最新リリース」へ変更した。これで macOS 先行 →
  Windows 版を同じタグに後付けする運用をしても、片方の OS に「更新はあるが
  ダウンロードできない」通知が出ない。
  *Release notes are now generated from the actual assets (download table, per-OS install
  steps, Known limitations for Windows), and the in-app update check looks for the newest
  release that has an asset for the running OS and architecture — so shipping macOS first
  and adding Windows assets to the same tag later never produces a dead-end notification.*

### Added / 機能追加

- **git タブ: ブランチ操作とコンフリクト解消エージェント** (#496)
  切替 / 作成 / マージを UI・CLI・MCP へ 1:1 で追加。破壊的操作は既定では実行せず
  「何が起きるか」を出す（`--yes` で実行）。マージは `git merge-tree --write-tree` で
  作業ツリーに触れずにコンフリクトを予測する。未解決ファイルとマージ元 / 先を含む
  プロンプトで claude / codex / agy を同じタブに立てる解消エージェントも新設した。
  *Branch checkout / create / merge exposed 1:1 across UI, CLI, and MCP. Destructive
  operations dry-run by default and predict conflicts without touching the work tree; a
  conflict-resolver agent can be launched in the same tab with a prefilled prompt.*
- **git タブ: コミット詳細とファイル単位ステージング** (#487, #495)
  「変更」「ステージ済み」の 2 セクション + 行ごとの ± ボタン + 一括ステージ + 更新ボタン。
  コミットをクリックすると選択カードの直下に変更ファイル一覧と diff が開く。
  *Two-section staging UI with per-row ± buttons, plus commit details (changed files and
  diffs) that open directly under the selected commit card.*
- **プロファイルの env 注入 / アカウントレジストリ / 専任マスター** (#500, #504, #511, #556)
  プロファイルに `env` / `cwd` / `projects` を追加し、accounts.yaml で複数アカウントを
  管理できるようにした（既定の資格情報を使うアカウントは `--inherit`）。spawn は
  `--account` でワーカーごとにアカウントを切り替えられ、master 起動時は `cwd` へ移動して
  担当プロジェクトをファイルツリーへ自動追加し、system prompt に担当範囲を注入する。
  CLI `tako orchestrator accounts` も MCP と 1:1 で提供する。
  *Profiles gain `env`, `cwd`, and `projects`; multiple agent accounts are managed in
  accounts.yaml (`--inherit` keeps the default credentials). `spawn --account` switches
  accounts per worker, and a master starts in its profile's `cwd` with its projects added
  to the file tree and injected into its system prompt.*
- **リモートインジケータを daemon 停止中も表示** (#590)
  ステータスバーのインジケータを常時表示にし、停止中はクリックで起動、未セットアップなら
  `tako remote setup` を案内する。
  *The remote indicator is always visible; clicking it starts the daemon, or points at
  `tako remote setup` when the machine is not configured yet.*
- **stale な claude バイナリの検知と張り直し** (#498)
  起動時に PATH 上の claude が実在・実行可能かを検証し、失われていれば再検出する。
  *Validates the resolved `claude` binary at startup and re-detects it when it is stale.*
- [macOS] **アプリケーションメニューの整備** (#485)
  About / バージョン表示 / 設定 / 標準の編集・ウインドウ項目を macOS のメニューへ揃えた。
  *Fills in the standard macOS application menu (About, version, Settings, Edit, Window).*
- **プラットフォーム対応マトリクスの公開** (#515)
  この環境でどの機能が使える / 縮退する / 未実装かを `tako platform`（MCP `tako_platform`）
  で参照できる。リリースノートの Known limitations もここから生成する。
  *`tako platform` (MCP `tako_platform`) reports which features are supported, degraded,
  or pending on the current platform; release notes derive their Known limitations from
  the same matrix.*

### Changed / 改善

- **ファイルツリー: ドット項目の既定非表示と新規作成の入力位置** (#550, #559)
  `.` 始まりの項目を既定で隠し（見出しの目アイコン / 右クリック / 設定画面で切替）、
  増えたルートは自動展開して先頭に置く。新規作成のインライン入力欄は「確定後にその項目が
  並ぶ位置」に出るようにし、インデントを通常行と揃えた。
  *Dot-prefixed entries are hidden by default (toggle from the tree header, context menu,
  or settings), newly added roots auto-expand at the top, and the inline create field now
  appears where the new item will actually land.*
- **git タブの使い勝手 4 件** (#560, #561, #562, #570)
  本文の並びを 変更 → コミット → ブランチ → リモート → diff に整理し、変更ファイル行の
  クリックでプレビューを開き、マージ導線を常時表示にした。コミットメッセージ欄の日本語
  入力（IME）が確定先を取り違えてターミナル側へ流れる問題も直した。
  *Reordered the panel (changes → commits → branches → remotes → diff), made changed-file
  rows open a preview, kept the merge affordance always visible, and fixed IME composition
  in the commit message field being delivered to the terminal pane instead.*
- **ペイン close の確認ガードと発生源の記録** (#566)
  ⌘W も × と同じ確認を通るようにし、確認対象は「失うものがあるペイン」（role 付き・
  実行中・子プロセスあり）に限定した。close の発生源（キーボード / UI / dispatch + 呼び出し
  role）をペインログに記録する。
  *⌘W now goes through the same confirmation as the × button, limited to panes that would
  actually lose something, and every close records where it came from in the pane log.*
- **パネルビューの語彙を GUI の表示名へ統一** (#553)
  CLI / MCP の `tmux` を画面と同じ `fleet` に改め、旧称は後方互換で受理し続ける。
  *The panel view formerly called `tmux` in CLI/MCP is now `fleet`, matching the GUI label;
  the old name is still accepted.*
- **設定画面の総点検と言語セレクタ** (#486, #488)
  押せないボタン・未配線のウィジェットを洗い出して修正し、表示言語の切替を設定画面へ
  追加した。
  *Audited the settings window for dead buttons and unwired widgets, and added the display
  language selector.*

### Fixed / 修正

- **orchestrator watch が `WORKER_IDLE` を発火しない** (#571)
  `claude agents --json` をプロセス環境の `CLAUDE_CONFIG_DIR` ごと実行していたため
  別アカウントの worker が「存在しない」ことになり、画面フォールバックは
  「子プロセスがある = busy」で必ず上書きされ、busy 判定はフッター 8 行に届いていなかった。
  3 層すべてを修正した。
  *Root-caused and fixed all three layers: agent enumeration inheriting the wrong
  `CLAUDE_CONFIG_DIR`, the screen fallback always overriding idle because the agent TUI is
  itself a child process, and the busy heuristic not reaching past the 8-line footer.*
- **生成中に人間が打った指示が消える** (#572)
  claude は生成中の打鍵を入力欄ではなく内部キューへ入れる。tako はこれを残留テキストと
  誤認して Enter を空撃ちしていた。入力欄が空かどうかを dim 属性で判定し、キュー滞留を
  `read` / `worker_status` / watch に公開し、生成が止まってもキューが残っていれば送り出す。
  *Typing while an agent is generating queues the text inside the agent, which tako misread
  as leftover input. Emptiness is now judged by dim attributes, queued messages are
  reported through `read` / `worker_status` / watch, and a stalled queue is flushed.*
- **permission ダイアログ待ちが「質問」として通知される** (#577)
  画面推定経路（agents に載らない worker・codex / agy）でダイアログの選択カーソルを入力欄と
  見なしていた。ダイアログが実在すれば `waiting` へ格上げし、`WORKER_PERMISSION` として
  通知する。本文中の「1. … 2. …」の誤検知は、ダイアログが入力欄を奪っているかで切り分ける。
  *When a worker cannot be resolved through `claude agents`, a permission dialog was read as
  the input line. A real dialog now escalates the status to `waiting` and raises
  `WORKER_PERMISSION`, while prose that merely looks like a numbered menu does not.*
- **spawn の初期プロンプトが届かない** (#530)
  `CLAUDE_CONFIG_DIR` を切り替えた初回に出る番号付き選択ダイアログ（テーマ選択など）の
  カーソルを入力欄と誤認し、プロンプトが確定操作に化けていた。文言に依存しない
  ダイアログ判定を入れ、未達時は `prompt_delivery` と再送コマンドを報告する。
  *The numbered dialogs claude shows on a fresh config dir were mistaken for the input
  line, turning the prompt into a menu confirmation. Dialog detection is now
  wording-independent, and undelivered prompts are reported with a resend command.*
- **事前信頼の書き先が claude の設定ディレクトリ外だった** (#558)
  `~/.claude.json` へ書いていたが claude が読むのは `<config dir>/.claude.json`。
  そのためフォルダの事前信頼と bypass の事前承認がどちらも効いていなかった。
  *tako wrote pre-trust entries to `~/.claude.json` while claude reads
  `<config dir>/.claude.json`, so neither folder pre-trust nor bypass pre-approval worked.*
- **stale な `TAKO_PANE_ID` から `tako master` / `solo` が起動できない** (#567)
  復元後などで環境変数が古いペインを指していても起動できるようフォールバックを追加した。
  *Added a fallback so master / solo still start from a shell whose `TAKO_PANE_ID` points at
  a pane that no longer exists.*
- **アカウント切替の欠落 3 件** (#511, #512, #547)
  CLI の `spawn / run --account` が未配線だった件、既定の資格情報を使うアカウントを
  `--inherit` で表現できなかった件、`master_account` が master / solo の起動に適用されて
  いなかった件を修正した。
  *Fixed `--account` missing from the CLI spawn/run paths, the inability to express
  "use the default credentials" (`--inherit`), and `master_account` not applying to
  master / solo launches.*
- **git パネルの描画崩壊・IME 未確定文字列の欠落・セルフテストの中断** (#494, #497, #501)
  コンテンツ総高さがパネル高さを超えると行が圧縮されて重なっていた問題を、固定ヘッダ +
  行が縮まないスクロール本文へ分離して根治。あわせて、メッセージが空・変更が無いときは
  ボタンと ⌘Enter の両方でコミットを拒否して理由を出す、実行中は操作ボタンを無効化する、
  貼り付けた制御文字を正規化する、といった堅牢化も入れた。カーソル非表示ペインで IME の
  下線が出ない問題と、テーマ項目でセルフテストが中断して以降が未実行になっていた問題も直した。
  *Split the git panel into a fixed header and a non-shrinking scroll body (rows used to be
  squeezed and overlap once content exceeded the panel height), blocked commits with a
  visible reason when there is nothing to commit, disabled controls while an operation runs,
  normalized pasted control characters, restored the IME underline on panes that hide the
  cursor, and stopped the self-test aborting at the theme item.*
- **アプリ内テキスト入力で ⌘V が効かない** (#546) /
  **テキスト入力フラグの残留でキー入力が奪われる** (#503)
  *Fixed ⌘V in in-app text fields, and a lingering text-input flag that swallowed keys.*
- **ファイルツリーのインデントガイド線が途切れる** (#589)
  行の border-left では自分の深さの線しか描けず、子孫行の区間で祖先の線が欠けていた。
  *Indent guides now draw the ancestors' verticals too, instead of only the row's own depth.*
- **git タブのブランチ種別を refname で判定** (#544) /
  **CLI `git show` が空応答** (#495) /
  **mp4 プレビューのシークバー** (#484) /
  **隔離モードで更新チェックが走る** (#470)
  *Branch kind is decided by refname, `tako git show` prints its result, the mp4 preview
  seek bar works again, and isolated instances no longer hit the update endpoint.*

### Security / セキュリティ

- 安定版ローンチにあたり、リモート接続の外部セキュリティレビュー (#287) を P0 / P1 ゼロで
  完了している。到達性は tailnet 内に限定され、未登録の端末は画面データを 1 バイトも
  受け取れない。監査ログは接続の事実だけを記録し、画面内容・送信テキスト・トークンは
  記録しない。
  *The external security review of remote access (#287) closed with no P0 or P1 findings
  before this stable launch. Reachability is limited to the tailnet, unpaired devices
  receive no screen data at all, and the audit log records connection events only — never
  screen contents, typed text, or tokens.*

### Internal / 開発基盤

- **Windows 移植の基盤** (#467, #515, #516, #518, #519, #520, #522)
  macOS 側から `cargo check --target x86_64-pc-windows-msvc` が通る状態を作り、機能の
  対応可否を機械可読なマトリクスで持つようにした。永続バックエンド（tmux）を
  `SessionBackend` / `DetachedAccess` の 2 段 trait へ抽象化し、到達手段を `PaneReach` 型で
  表現、`tmux_backend::available()` を廃止して能力ベースの問い合わせへ置き換えた。
  OS 連携（`open` / `osascript`）は境界へ集約し、system prompt と setup 配布物は単一ソース化。
  git のパス表記も `/` 区切り前提の境界を通すようにした。
  *Windows-port groundwork: cross-compilation checks pass from macOS, feature support is a
  machine-readable matrix, the persistence backend is abstracted behind two traits with
  reachability expressed as a `PaneReach` type, OS integration calls are funneled through
  one boundary, prompts/setup assets have a single source, and git paths go through a
  portability boundary.*
- [Windows] **CI の復旧** (#574)
  macOS / Windows 両ジョブに PWA のビルド工程を足して 45 日ぶりに緑に戻した。Windows の
  テストは POSIX 前提の 19 件が残っているため、当面はテストステップのみ非ブロッキング。
  *Restored CI by adding the PWA build step to both runners; the Windows test step stays
  non-blocking until the remaining POSIX-only tests are ported.*
- セルフテストの安定化 (#599)、紹介動画の制作パイプライン (#470)。
  *Self-test stabilization and the promo-video production pipeline.*

## [0.5.13] - 2026-07-27

Nightly patch release (automated). Changes since v0.5.12:
夜間パッチリリース（自動）。v0.5.12 以降の変更:

- [修正] パネルビューの語彙を GUI 表示名 fleet へ統一 (#553) (#564)
- [修正] アプリ内テキスト入力で ⌘V が効かない問題を根治 (#546) (#563)
- [修正] git タブのブランチ種別を refname で判定する (#544) (#554)
- [修正] spawn 初期プロンプトの消失を根治 (#530) (#557)
- [機能追加] CLI に orchestrator accounts を追加（MCP と 1:1） (#556)
- [修正] プロファイルの master_account を master / solo 起動へ適用する (#555)
- [修正] アカウント切替の残欠陥 2 件（CLI --account / inherit） (#543)

## [0.5.12] - 2026-07-26

Nightly patch release (automated). Changes since v0.5.11:
夜間パッチリリース（自動）。v0.5.11 以降の変更:

- [リファクタ] persist のゲートを capabilities へ言い換え — 段取り④ (Refs #519) (#540)
- [リファクタ] dispatch の到達フォールバックを PaneReach 経由へ — 段取り③ (Refs #519) (#539)
- [ドキュメント] activeContext を今夜の merge 状況へ更新 (#538)
- [リファクタ] OS 連携の直呼びを境界 B8 へ集約 (#522) (#537)
- [機能追加] git タブ: ブランチ操作 + コンフリクト解消エージェント (#496) (#534)
- [修正] git タブ: パス表記の可搬性と CRLF 耐性 (#520) (#536)
- [機能追加] 永続バックエンドの抽象境界 B2 を新設 (#519) (#535)
- [機能追加] Windows 移植 基盤: system prompt / setup 配布物の単一ソース化 (#516) (#533)
- [機能追加] Windows 移植 基盤: プラットフォーム対応マトリクスとパリティテスト (#515) (#532)
- [ドキュメント] Windows 永続バックエンドの設計 (#518) (#531)
- [機能追加] Windows 移植 P0: 抽象境界の新設でクロス check を成立させる (#467) (#529)
- [ドキュメント] progress に #495 UX 改善の作業記録を追記
- [改善] git タブ: コミット詳細を選択カード直下に表示 (#495) (#510)
- [ドキュメント] progress に本日の作業記録を追記 + activeContext 更新
- [修正] CLI git show の出力が print_result に未登録で空応答だった (#495)
- [機能追加] stale claude バイナリの検知と張り直し (#498) (#508)
- [機能追加] git タブ: コミット詳細表示 (#495) (#507)
- [修正] テキスト入力フラグの残留でキー入力が奪われる問題を根治 (#503) (#509)
- [機能追加] プロファイル cwd + ファイルツリー自動追加 + 専任マスター (#500 Part 5-7) (#506)
- [機能追加] プロファイル env 注入 + アカウントレジストリ (#500 Part1-4 + #504) (#505)
- [修正] git タブの描画崩壊を根治 + IME 未確定文字列の欠落を修正 (#494, #497, #501) (#502)

## [0.5.11] - 2026-07-25

Nightly patch release (automated). Changes since v0.5.10:
夜間パッチリリース（自動）。v0.5.10 以降の変更:

- [修正] mp4 プレビューのシークバー処理を修正 (#484) (#493)
- [ドキュメント] progress に #487 の作業記録を追記
- [修正/機能追加] git タブ総点検 + ファイル単位ステージング UI (#487) (#492)
- [修正] 設定画面（Cmd+,）の総点検: 押せないボタン・未配線ウィジェットの修正 + 言語セレクタ (#486, #488) (#491)
- [機能追加] macOS アプリケーションメニュー整備: About/バージョン/設定/標準項目 (#485) (#490)
- [改善] 紹介動画 v3: setup 節を対話セットアップの訴求に刷新 (#470) (#489)
- [改善] 紹介動画 v2: テロップ背景 + master オーケストレーション節 (#470) (#483)
- [修正] 隔離モードで更新チェックを止める + 収録スクリプトの取りこぼし修正 (#470) (#482)

## [0.5.10] - 2026-07-24

Nightly patch release (automated). Changes since v0.5.9:
夜間パッチリリース（自動）。v0.5.9 以降の変更:

- [ドキュメント] 紹介動画: 収録待機をキャプチャ可否ベースに変更 + ウィンドウサイズ安定待ち (#470) (#481)
- [ドキュメント] 紹介動画 Phase B: 制作パイプライン一式 + BGM (#470) (#480)

## [0.5.9] - 2026-07-23

- [機能追加] git タブに操作ボタン群: コミット / プル / プッシュ (#472)
  git タブのヘッダにステージ・コミット・プル・プッシュの操作ボタンを追加。
  コミットメッセージ入力欄 + ステージ済みファイル一覧表示。dispatch / CLI / MCP 1:1。
  Add git tab action buttons: stage, commit, pull, push (#472).
  Commit message input + staged file list. All operations exposed via dispatch/CLI/MCP.

- [修正] setup-mcp の登録先を Claude Code が読む場所に修正 (#476)
  `tako setup-mcp` が MCP 設定を書き込む先が Claude Code の実際の読み込みパスと
  異なっていた問題を修正。外部テスター環境でゼロコンフィグ接続が機能しなかった根因。
  Fix setup-mcp writing MCP config to the wrong location (#476).
  The registration path did not match where Claude Code actually reads MCP settings,
  causing zero-config connection to fail on external tester environments.

- [UI] リモート: ペイン一覧カードの情報量増 + role 別スタイル (#433)
  リモート PWA のペイン一覧カードに role バッジ・状態ドット・cwd・コマンド情報を追加。
  master / worker / solo を色分け表示。
  Remote PWA: richer pane cards with role badge, status dot, cwd, command info (#433).
  Color-coded styling for master / worker / solo roles.

- [修正] リモート: チャットビュー切替後の更新停止を修正 (#466)
  `claude agents --json` の一時失敗・列挙漏れで live 解決が消えると、セッション ID が
  カタログの stale 旧世代へ化け、チャットが凍結 transcript を読み続けていた。
  agents.rs に sticky live 解決（失敗時は直近成功値を保持）を追加、
  sessions.rs の resolve を last_seen_at 最新優先に変更。
  Fix remote: chat view freezing after switching panes (#466).
  Transient failures in `claude agents --json` caused the session ID to fall back to
  a stale catalog entry. Added sticky live resolution and last_seen_at-based ordering.

- [修正] リモート PWA の複数ファイル添付対応 (#463)
  チャット入力で複数ファイルを同時添付できるよう修正。
  Fix remote PWA: support multiple file attachments in chat input (#463).

- [機能追加] Code Runner: ファイル実行機能の実装 (#453)
  ファイル内 `tako:run:` 宣言または拡張子既定でワンクリック実行。
  `tako run <file>` / MCP `tako_run` + `tako_run_resolve` + `tako_run_defaults`。
  プレビューヘッダに再生ボタン + プロファイルドロップダウン（2+ 候補時）。
  復元・リロード経路での検出漏れ + Run ペイン即死の根治を含む。
  Add Code Runner: one-click file execution via `tako:run:` declarations or
  extension defaults (#453). Preview header play button with profile dropdown.
  Includes fixes for detection on restore/reload paths and Run pane instant death.

- [修正] リモート PWA の入力 textarea を改行に応じて自動リサイズ (#457)
  Fix remote PWA: auto-resize input textarea as newlines are entered (#457).

- [セキュリティ] remote daemon の TCP listen を UDS 化し identity 偽装を根治 (#287)
  remote daemon のローカル通信を TCP から Unix domain socket へ移行し、
  同一マシン上の他プロセスからの identity 偽装（XFF ヘッダ改竄）を構造的に排除。
  Migrate remote daemon local transport from TCP to Unix domain socket (#287).
  Structurally eliminates identity spoofing via XFF header forgery from co-located processes.

- [機能追加] 設定画面（Cmd+,）の全タブ実装完了（#459）
  M4: Code Runner タブ（拡張子既定テーブル + 編集 + 変数リファレンス）。
  M5: セットアップタブ（CLI 検出状態 / FDA / MCP 登録 / ルール同期 / tako setup 起動）。
  M6: スリープ防止タブ（モード / 電源条件 / 蓋閉じ）+ リモートタブ（状態 / 開始・停止）
  + 高度タブ（settings.json 表示 / 再読み込み / Finder で表示 / 関連ファイル）。
  全操作は既存 dispatch 経由で CLI / MCP との 1:1 を維持。
  Complete settings window (Cmd+,) implementation (#459).
  M4: Code Runner tab (extension defaults table + edit + variable reference).
  M5: Setup tab (CLI detection / FDA / MCP registration / rules sync / run setup).
  M6: Sleep Guard tab (mode / power / lid) + Remote tab (status / start-stop)
  + Advanced tab (settings.json viewer / reload / Finder reveal / related files).
  All operations use existing dispatches to maintain CLI/MCP 1:1 parity.

- [改善] リモート PWA チャットビューの Markdown レンダリング対応
  assistant / user メッセージを marked + DOMPurify でパース・サニタイズし、太字・
  インラインコード・コードブロック・見出し・リスト・引用・リンク・テーブル等を正しく
  表示するように改善。コードブロックは等幅フォント + 背景色付き。XSS 防止済み。
  Add Markdown rendering to the remote PWA chat view. Assistant and user messages
  are now parsed with marked and sanitized with DOMPurify, properly rendering bold,
  inline code, code blocks, headings, lists, blockquotes, links, tables, etc.
  Code blocks use monospace font with a distinct background. XSS-safe.

- [機能追加] UI の日英 i18n: 表示言語の切替（#435）
  UI 文字列をロケールキー化（`ui_text` カタログ + `tr!(日, 英)`）し、日英を切替可能に。
  既定は OS ロケール解決（環境変数 → macOS AppleLanguages）。手動切替は
  CLI `tako lang [ja|en|system]` / MCP `tako_lang` / コマンドパレット
  「表示言語を切替」の 3 経路（設定は settings.json の `language` に永続化、GUI 即時反映）。
  タブバー・パレット・ドロワー・サイドバー・右パネル・リモートパネル・更新バナー・
  close 確認・ポート検知チップ・プレビューペイン等の主要 UI を英語対応。
  Add Japanese/English i18n for the UI (#435). UI strings are externalized into a
  locale catalog (`ui_text` + `tr!(ja, en)`), with the display language resolved
  from the OS locale by default (env vars, then macOS AppleLanguages). Manual
  switching via CLI `tako lang [ja|en|system]`, MCP `tako_lang`, or the command
  palette ("Switch language"); the setting persists to settings.json and applies
  to the GUI immediately. Major surfaces (tab bar, palette, drawer, sidebar,
  panels, update banner, close dialogs, port chips, preview pane) are translated.

- [修正] 隔離/多重起動時の disablesleep 残留解除で本番の蓋閉じ防止を外さない (#449)
  `check_disablesleep_residual()` が pmset disablesleep のマシングローバル状態を
  無条件で 0 に戻していたため、本番インスタンスが busy エージェントのために正当に
  有効化した蓋閉じ防止を、隔離/検証インスタンスが起動時に一時解除する穴があった。
  TAKO_ISOLATED / 他 tako プロセス動作中 / セカンダリモードの 3 条件でスキップする
  ガードを追加。単独残留（クラッシュ後の真の残留）の解除は従来どおり動作する。
  Fix isolated/secondary instances clearing production lid-sleep prevention (#449).
  `check_disablesleep_residual()` unconditionally reset the machine-global
  `pmset disablesleep` to 0, which could momentarily disable lid-sleep prevention
  that a production instance had legitimately enabled for busy agents. Added guards
  to skip residual clearing under TAKO_ISOLATED, when another tako process is
  running, or in secondary mode. Genuine residuals after a crash are still cleared.

- [修正] 隔離起動時に remote state_dir を隔離し本番 state 破壊を防止 (#445)
  `TAKO_ISOLATED=1` での隔離起動時に `TAKO_REMOTE_STATE_DIR` が隔離されず、
  2 秒毎の `daemon_status` ポーリングが本番の remote state ファイルを削除し得た。
  一括隔離ブロックに `TAKO_REMOTE_STATE_DIR` を追加し、`daemon_status` の
  cleanup に隔離モードガードを二重防御として設置。
  Fix remote state_dir not being isolated under `TAKO_ISOLATED=1` (#445).
  The periodic `daemon_status` poll could delete production remote state files
  when running an isolated instance. Added `TAKO_REMOTE_STATE_DIR` to the
  bulk isolation block and guarded `daemon_status` cleanup in isolated mode.

- [セキュリティ] リモートの cross-origin 脆弱性を遮断 (#287)
  ペアリング済み端末上で開いた悪意あるサイトから ts.net URL へ fetch して
  ターミナル画面の読取・任意コマンド実行が可能だった穴を塞いだ。
  REST/WS の `Origin` ヘッダを daemon の `base_url` と完全一致で検証し、
  不一致・欠落を原則 403 で拒否する。CORS `Access-Control-Allow-Origin: *` を
  廃止し、許可 origin のみをエコーする方式に変更。WS upgrade では
  `Sec-WebSocket-Protocol` に `tako-remote` の提示を必須化。
  Block cross-origin terminal access on paired devices (#287).
  Any page opened on a paired phone could fetch the fixed ts.net URL to read
  terminal screens and send arbitrary input. Now the `Origin` header is
  validated against the daemon's `base_url` (exact match); mismatched or
  missing origins are rejected with 403. `Access-Control-Allow-Origin: *` is
  replaced with echoing the allowed origin only. WS upgrade now requires
  `Sec-WebSocket-Protocol: tako-remote` to be presented by the client.

- [スタイル] sleep-guard 状態チップを平易な日本語表示 + クリック詳細ポップオーバーへ刷新 (#440)
  「awake+lid」等の略語表示をやめ、「スリープ防止中」「スリープ防止中・蓋閉じOK」
  「スリープ防止中・高温注意」（高温時は赤字）の初見で意味が取れる表示に変更。
  チップクリックで現在モード・防止が効いている理由（エージェント N 体稼働中）・
  蓋を閉じたときの挙動・設定変更方法（コマンドはクリックでコピー）を平易な文章で
  解説するポップオーバーを新設（ルートオーバーレイ方式で #361 のクリップ問題を回避）。
  UI 文字列は新設の ui_text カタログモジュールに集約し、#435 の i18n（案 B）で
  ロケールキー化しやすい構造にした。
  Redesign the sleep-guard status chip with plain-language labels and a
  click-to-open explainer popover (#440). Replaces the cryptic "awake+lid"
  labels with self-explanatory Japanese text (red-tinted when the machine is
  hot), and adds a popover explaining the current mode, why sleep is being
  prevented (N busy agents), what happens when the lid closes, and how to
  change the setting (command is click-to-copy). All strings now live in a
  new ui_text catalog module structured for the upcoming i18n work (#435).

- [スタイル] リロードアイコンを円弧 + 三角矢じりのブラウザ同型デザインへ刷新 (#438)
  旧デザインは円弧 2 本 + 45° 回転の十字で、小サイズ（12px）では ✕ に誤認されていた。
  単一の 270° 円弧 + 塗り三角の矢じり（Chrome / Material のリロードと同型）へ差し替え。
  ストローク 1.6・矢じり幅はストロークの 3 倍で 12px でも潰れない比率に調整。
  ステータスバーの利用制限リロードボタンと Web ビューのリロードボタン（同一アセット）が対象。
  Redesign the reload icon as a standard arc + solid-triangle arrowhead (#438).
  The old glyph (two arcs + a 45°-rotated cross) read as an ✕ at small sizes.
  Replaced with a single 270° arc and a filled triangular arrowhead matching
  browser reload buttons, tuned to stay legible at 12px. Applies to the usage-limit
  refresh button in the status bar and the webview reload button (same asset).

- [修正] リモート: 数値 PaneId 宛の入力送信が無音失敗する問題を修正 (#428)
  input API が PaneId を "session:0.0" 形式へ変換して dispatch の tmux_session
  （セッション名を期待）へ渡し、deliver 系の `={session}:` 組み立てが
  `=session:0.0:` になり can't find pane で無音失敗していた（HTTP は 200 を返却）。
  PaneId は dispatch の pane に渡して GUI の送達検証フロー（#32/#95）を通し、
  tmux フォールバックにはセッション名のみ渡すよう修正。
  Fix remote: input to numeric PaneId silently failed to deliver (#428).
  The input API passed a "session:0.0" tmux target into dispatch's tmux_session
  (which expects a bare session name), producing an invalid `=session:0.0:`
  target. Now passes the PaneId itself so the GUI delivery-verification flow applies.

- [修正] リモート: ペインを開き直すと term ビューが無限ロードになる問題を修正 (#426, #428)
  WS の init メッセージが term ビュー DOM の未マウント中（初期 chat 表示中）に
  届くと捨てられ、update には loading 解除が無いため永久スピナーになっていた。
  2 回目以降は broadcaster の init キャッシュが接続直後（実測 0ms）に届くため
  必ず発症。init を保留して term ビューのマウント時に適用するよう修正。
  Fix remote: term view stuck loading when reopening a pane (#426, #428).
  WS init arriving before the term view DOM mounted was dropped; updates never
  clear the loading state. Reopened panes always hit this because the cached
  init arrives instantly. Init is now buffered and applied on term-view mount.

- [UI] リモート: モバイルの改行キーで送信されてしまう問題を修正 (#429)
  chat / term 入力欄の Enter を改行入力に変更（モバイルは Shift が無く改行を
  入力できなかった）。送信は送信ボタンまたは cmd/ctrl+Enter に分離。
  Fix remote: mobile Enter key sent the message instead of inserting a newline (#429).
  Enter now inserts a newline; sending is via the send button or cmd/ctrl+Enter.

- [修正] リモート: remote start が PATH 上の別世代バイナリで serve を立てる問題を修正 (#432)
  serve バイナリ解決を /Applications の安定バイナリ優先へ変更（GUI .app と serve の
  世代を揃える。検証モードでは従来どおり自バイナリ）。`tako remote status` と起動
  応答に serve_binary を表示し、稼働中 serve が解決先と食い違う場合は start が
  停止 → 再起動を案内するように変更。
  Fix remote: `remote start` could spawn a stale-generation serve from PATH (#432).
  Binary resolution now prefers the stable /Applications bundle, status/start
  expose the serve binary path, and a generation mismatch prompts a restart.

- [修正] リモート: master ペインが claude チャット画面として検出されない問題を修正 (#439)
  ペインの agent 種別判定が role 文字列だけに依存し、復元・handoff・手動起動などで
  role が空の master 相当ペインは claude と認識されずチャット UI にならなかった。
  `claude agents --json` の pid 祖先辿りでバックエンドセッションごとの稼働中 claude を
  一括解決し（実プロセスの存在証明 = ground truth）、role が無くても対話型 claude が
  動いていれば agent_type=claude + session_id を付与するよう変更。PWA は chat/term の
  自動追従を双方向化（session_id が一時的に欠けても復帰できる）。
  Fix remote: master pane not detected as a claude chat screen (#439).
  Agent-type detection depended only on the role string, so role-less master panes
  (after restore/handoff/manual launch) were never recognized as claude. Now resolves
  the live claude session per backend via pid-ancestry over `claude agents --json`
  and marks a pane as claude when an interactive claude is actually running.

- [修正] リモート: auto mode の自動実行コマンドに承認カードが出る問題を根本修正 (#425)
  承認待ちの判定を transcript の推定から**画面の permission ダイアログ実在**へ再設計した。
  旧実装（#430）は「末尾 tool_use + tool_result 未着」を承認待ちとみなしたが、これは
  auto mode のツール実行中（承認不要）と承認待ち停止の両方で成立し区別できないため、
  実行に時間のかかるツールの間じゅう誤った承認カードが出続けていた。transcript からの
  approval 付与を全廃し、v2 panes API がペイン画面を capture して permission ダイアログを
  検知したときだけ `permission_dialog` を付与する方式へ変更。PWA はこれを唯一の根拠に
  承認カードを表示し、選択肢ボタンは実ダイアログの選択肢そのもの。応答は新設の
  `POST /api/panes/:id/respond`（ダイアログ実在を再検証してから番号キー + Enter を送達）。
  Fix remote: approval cards shown for auto-mode tool calls, root-cause redesign (#425).
  Pending-approval is now determined by the actual on-screen permission dialog rather
  than transcript heuristics. The previous approach (#430) could not distinguish an
  in-progress auto-mode tool call from a genuine approval stop, so cards appeared for
  the entire duration of slow tools. Transcript-based approval is removed; the v2 panes
  API captures the pane screen and attaches `permission_dialog` only when a real dialog
  is present, answered via the new `POST /api/panes/:id/respond`.

- [修正] 赤ボタン close → Dock 復帰でウインドウサイズ・位置がデフォルトに戻る問題を修正 (#412)
  最後のウィンドウ close 時に `drop_viewport` が `window_frames` を削除し、Dock 復帰の
  `reopen_or_restore` → `open_viewport` が保存フレームを参照できずデフォルトサイズになっていた。
  Fix: window size and position not restored after red-button close → Dock reopen (#412).
  `drop_viewport` was clearing the in-memory frame cache on last-window close.

- [改善] 更新チェッカの GitHub API レート制限対策 (#416)
  gh CLI の認証トークンがあれば自動使用（60→5000req/h）、
  2 チャンネル判定を /releases 一覧の 1 リクエストに統合（旧 check_latest の Web リダイレクト
  方式を廃止）、レート制限時はキャッシュ済みの結果を補助表示付きで返すように変更。
  GitHub rate limit countermeasures for the update checker (#416).
  Automatically uses gh CLI auth token when available (60→5000req/h),
  unified two-channel check into a single /releases API request,
  and shows cached results with a note when rate-limited.

- [修正] タブ D&D 挿入位置インジケータが常に右端に固定表示される問題を修正 (#413)
  GPUI の on_drag_move が capture フェーズで全登録要素に hitbox チェックなしで発火する
  ため、+ ボタンのハンドラが常に最後に勝ってインジケータが末尾に固定されていた。
  各ハンドラに bounds.contains チェックを追加し、カーソル直下の要素だけが反応するよう修正。
  Fix tab D&D insertion indicator always appearing at the right end (#413).
  GPUI's on_drag_move fires on all registered elements in capture phase without
  hitbox checking. Added explicit bounds check to each handler.

- [修正] Web ビュー URL 入力欄フォーカス中の paste がペインに入る問題を修正 (#414)
  paste() が on_action 経由で直接呼ばれ handle_key のフォーカスチェーンを迂回していた。
  URL 入力欄・address bar・パレット・インライン編集すべてに paste 経路を追加。
  Fix paste during webview URL input focus leaking to terminal pane (#414).

- [改善] cmd+ドロップ時のハイライトを「パス入力」表示に変更 (#415)
  ファイルの cmd+ドロップ時のオーバーレイが分割ハイライトのままだったのを、
  全面パス入力表示（薄い背景 + カーソルバー + ラベル）に変更。cmd の押し離しに即時追従。
  Show "path insert" overlay instead of split highlight on cmd+drop (#415).

- [機能追加] worker 自動復旧 supervisor (#401)
  watch の検知イベント（usage_limit / api_error / limit_dialog / WORKER_DEAD /
  prompt_undelivered）に対して自動リカバリアクションを実行する常駐ロジックを追加。
  usage_limit: ダイアログ確定 → リセット時刻待ち → 続行ナッジ → busy 検証。
  api_error: バックオフ付き続行ナッジ。WORKER_DEAD: 自動 resume（既定 notify-only、opt-in）。
  prompt_undelivered: レジストリの prompt_head から自動再送。
  同一 worker で N 回（既定 3）失敗するとエスカレーション（自動停止 + 通知のみ）。
  監査ログ（supervisor.log）に全アクションを記録。
  `tako orchestrator supervisor status/set_mode/history` + MCP `tako_orchestrator_supervisor`。
  Add worker auto-recovery supervisor (#401). Automatic actions for usage_limit
  (dialog confirm → wait for reset → nudge → verify), api_error (backoff nudge),
  WORKER_DEAD (auto resume, opt-in), and prompt_undelivered (re-send). Escalation
  after N failures. Audit log + MCP/CLI 1:1.

- [機能追加] worker レジストリ: ペイン消失後も worker を追跡・watch できるように (#390)
  spawn した worker を永続レジストリ（workers.yaml）へ登録し、アプリ再起動・ペイン消失後も
  watch / status / report が tmux session / claude session ID 経由で追跡を継続する。
  pane_id 指定のままでも追跡キーが自動補完されるため既存の watch 運用は無変更で恩恵を受ける。
  `tako orchestrator workers [--all]` + MCP `tako_orchestrator_workers` で一覧、
  watch / status / report に `--worker <ID>` を追加（MCP 1:1）。
  prompt 未達検知も追加: spawn 後に transcript が生成されない claude worker を保守的な
  複合条件（猶予 4 分 + 非 busy）で検出し、worker_status の
  `prompt_delivery: undelivered` + events `prompt_undelivered` で通知する。
  spawn 側も強化: プロンプト貼り付けが claude TUI 初期化中に吸われ入力欄が空のまま
  「送信成功」と誤判定される競合を、入力欄空検出時の再貼り付けで塞いだ。
  エージェント突然死（SIGSEGV 等）も検知: session 検出済み worker の実行プロセスが
  消えると watch が `WORKER_DEAD` を発火し、レジストリの session ID から組み立てた
  `claude --resume` コマンドを提示する（自動 resume はクラッシュループ回避のためしない）。
  Add a persistent worker registry (#390). Spawned workers are recorded in
  workers.yaml so watch / status / report keep tracking via tmux session /
  claude session ID even after panes vanish or the app restarts. Adds
  `tako orchestrator workers`, `--worker <ID>` options (MCP 1:1), undelivered
  prompt detection, agent crash detection (`WORKER_DEAD` with a suggested
  `claude --resume` command), and paste-retry hardening in the spawn flow.

## [0.6.0-test.1] - 2026-07-21

> 注: このテスト版タグは実際には公開されていない（バージョン番号だけが一時的に使われた）。
> ここに並ぶ変更は v0.5.9 以降の安定版・夜間版として配布済みで、安定版としての正式提供は
> [0.6.0] にまとめてある。
> Note: this test tag was never published — only the version number was used briefly.
> Everything listed here shipped in v0.5.9 and later; [0.6.0] is the stable release that
> officially carries it.

テスト版リリース（#403）。remote 全面刷新（Tailscale Serve 一本化 + 機器ペアリング認証）と v0.5.x 期間中の全機能・修正を含む。
Test release (#403). Includes the remote transport overhaul (Tailscale Serve + device pairing auth) plus all features and fixes accumulated during the v0.5.x nightly cycle.

### Breaking / Security

- **remote transport を Tailscale Serve へ一本化** (#282): Quick Tunnel / Cloudflare relay
  Worker / 公開 Pages PWA / URL 埋め込みトークン / `--insecure` 平文 LAN モードを廃止。
  接続は tailnet 内限定・WireGuard E2E 暗号化・恒久固定 URL のみ。
  *Migrate the remote transport to Tailscale Serve; remove Quick Tunnel, the Cloudflare relay
  Worker, public Pages hosting, URL-embedded tokens, and the plaintext LAN mode.*
- **機器ペアリング二層認証を実装 + PWA を daemon 配信化** (#283):
  - 層① Tailscale identity 検証（`tailscale whois` で接続元ノードを照合）+
    層② 機器ペアリング（Mac 承認ダイアログ・role = observe/interact/manage/admin・
    devices.json 永続化）。未登録端末は画面データを 1 バイトも受け取れない。
  - 長寿命 bearer token / QR の token 埋め込み / `tako remote status --show-token` を全廃。
  - 端末失効の即時反映（接続中の WS を即切断）・interact idle timeout（15 分）・
    接続開始終了の macOS 通知・status bar インジケータ + kill switch・監査ログ
    （内容は記録しない）。
  - PWA を daemon の静的配信へ（同一 origin・バージョン一致）。ペアリング画面を新設。
  - CLI `tako remote devices list/revoke` + MCP `tako_remote_devices`（計 92 ツール）。
    **承認・権限昇格は Mac GUI 限定**（AI フルコントロール不変条件の明示的な例外）。
  *Implement two-layer device-pairing authentication and serve the PWA from the daemon
  (same-origin, version-matched); approval and role elevation are Mac-GUI-only.*


### Added / 機能追加

- **`tako remote setup` 対話ウィザード新設** (#286): Tailscale の導入検出 → brew/App Store
  案内 → ログイン確認 → MagicDNS/HTTPS 有効化ガイド → serve 設定 → 自己接続確認 →
  固定 URL の QR（PNG）表示 → スマホ側手順案内。`--yes` / `--answers` で非対話実行可。
  dispatch `RemoteSetup` + MCP `tako_remote_setup` と 1:1（101 ツール）。
  *New `tako remote setup` interactive wizard for Tailscale Serve configuration: detects
  installation, guides login/HTTPS setup, configures serve, generates QR PNG, and shows
  phone-side instructions. Non-interactive via `--yes` / `--answers`. MCP 1:1 (101 tools).*
- `tako setup` の依存チェックに tailscale を追加（任意扱い）。完了サマリ末尾に
  `tako remote setup` への案内を表示。setup changes.yaml rev 11 で既存ユーザーにも配信。
  *Added tailscale to `tako setup` dependency check (optional). Shows `tako remote setup`
  guidance at the end of setup summary. Distributed via changes.yaml rev 11.*


- **リリースチャンネル制（安定版 / テスト版）を実装** (#403): アプリ内更新チェックで安定版とテスト版を区別し、テスト版ユーザーにはテスト版の最新を案内、安定版ユーザーには安定版のみ案内する二系統配布。`release.sh --test`（prerelease リリース）/ `--promote <tag>`（テスト版を安定版に昇格）をサポート。夜間リリースはテスト版として配布。バージョン比較は `-test.N` サフィックスを正しく扱う
  *Implement release channels (stable / test) (#403): in-app update checker distinguishes stable from test builds — test users see the latest test release, stable users only see stable releases. `release.sh --test` creates a prerelease, `--promote <tag>` promotes a test release to stable. Nightly releases are distributed as test builds. Version comparison correctly handles `-test.N` suffixes.*

- 複数ウィンドウ対応: ビューポート方式で別ウィンドウに別タブを表示 (#339)
  単一アプリ状態（タブ・ペイン・tmux・discovery）を複数ウィンドウで共有し、各ウィンドウは
  表示タブだけを持つ。New Window（⌘⇧N）が状態共有の追加ウィンドウになり、タブの
  ウィンドウ間移動・persist でのウィンドウ配置復元・`tako window` CLI / MCP `tako_window`
  （101 ツール）・`tako list` へのウィンドウ情報追加（後方互換）を含む。赤ボタン close は
  複数枚ならタブを残存ウィンドウへ合流（プロセス維持）、最後の 1 枚は従来どおり
  Dock 復帰（#312）と整合

  Multi-window support: viewport model showing different tabs per window (#339).
  A single app state (tabs, panes, tmux, discovery) is shared across windows; each
  window only holds which tab it displays. New Window (⌘⇧N) now opens a
  state-sharing viewport. Includes moving tabs between windows, window layout
  persistence/restore, `tako window` CLI / MCP `tako_window` (101 tools), and
  backward-compatible window info in `tako list`. Closing a non-last window merges
  its tabs into a remaining window (processes preserved); closing the last window
  keeps the #312 Dock-revival behavior.


- orchestrator report: worker 報告の scrollback + transcript 直読 (#364)
  `tako orchestrator report` / MCP `tako_orchestrator_report` で worker の出力を
  ペイン幅に依存しない tmux スクロールバック + claude transcript 2 層で取得（100 ツール）。
  worker-status に history フィールド（履歴行数/バイト）を追加

  orchestrator report: scrollback + transcript-based worker report reading (#364).
  `tako orchestrator report` / MCP `tako_orchestrator_report` retrieves worker output
  via width-independent tmux scrollback + claude transcript two-layer approach (100 tools).
  Added history field (lines/bytes) to worker-status.


- codex の利用制限データ（primary / secondary）を実データで表示: codex TUI フッターの `primary NN%` / `secondary NN%` パターンをスクレイピングし、ステータスバーのサービス切替ドロップダウンに実データを反映。agy は CLI にレート制限表示機能が無いため「未対応」として明示 (#357)
  Codex usage limit data (primary / secondary) now displays real data: scrapes `primary NN%` / `secondary NN%` patterns from the Codex TUI footer and reflects actual data in the status bar service switcher dropdown. agy is marked as "unsupported" since the CLI lacks rate limit display (#357)

- ステータスバーの利用制限表示を改修: 「週」→「7d」表記に統一、サービス切替ドロップダウン（claude / codex / agy）を追加。サービス別の色ドット + ラベルで視覚的区別。選択は settings.json に永続化。データ経路の無いサービスは「--」で明示。CLI `tako limit-service` + MCP `tako_limit_service`（99 ツール）(#321)
  Status bar usage limit display revamp: changed "週" → "7d" notation, added service switcher dropdown (claude / codex / agy) with per-service color dot + label. Selection persists in settings.json. Services without data show "--". CLI `tako limit-service` + MCP `tako_limit_service` (99 tools) (#321)

- ターミナルペインで選択ドラッグ中にビューポート上下端へ到達すると自動スクロールして選択を継続できるようになった。端への近さでスクロール速度が変化する（最小 2 行/秒、最大 30 行/秒）。上方向はスクロールバックへの遡り、下方向は最新出力方向。alt_screen（全画面 TUI）では従来挙動を維持 (#310)
  Terminal pane now auto-scrolls during selection drag when the cursor reaches the top or bottom edge of the viewport, extending the selection into scrollback history. Scroll speed increases with proximity to the edge (2–30 lines/sec). Alt-screen (fullscreen TUI) behavior is unchanged (#310)

- タブバーのタブ D&D 並べ替え: タブを掴んで左右にドラッグすると順序が変わる。挿入位置にアクセントカラーのインジケータを表示。並び順は persist（layout.json）に自動反映。CLI `tako tab reorder` + MCP `tako_reorder_tab`（97 ツール）で操作可能 (#308)
  Tab bar drag-and-drop reorder: drag tabs left/right to change order. An accent-colored insertion indicator shows the drop position. Order persists in layout.json automatically. CLI `tako tab reorder` + MCP `tako_reorder_tab` (97 tools) (#308)

- エラーレポートの自動送信基盤（テレメトリ）: panic / 重大エラーを PII なしで Cloudflare Workers エンドポイントへ自動送信。既定 OFF（opt-in）。送信内容はすべてローカルの telemetry.log に記録される透明性設計。CLI `tako telemetry status/on/off` + MCP `tako_telemetry` で操作可能。スキーマ・保持期間 90 日・削除依頼先を docs に明記 (#333)
  Automatic error reporting (telemetry): sends panic / critical errors to a Cloudflare Workers endpoint with no PII. Disabled by default (opt-in). All sent reports are logged locally to telemetry.log for transparency. CLI `tako telemetry status/on/off` + MCP `tako_telemetry`. Schema, 90-day retention, and deletion contact documented (#333)

- シンタックスハイライト対応形式の大幅拡充: syntect デフォルト 75 構文 → bat 由来の拡張セット 210+ 構文（two-face crate）。TOML・Dockerfile・TypeScript（ネイティブ）・Swift・Kotlin・INI・DotENV・CMake 等が新たに対応。ファイル名ベースの判定も追加（Cargo.lock → TOML、CMakeLists.txt → CMake、.gitignore → Git Ignore 等）。バイナリサイズ増加は約 550KB (#320)
  Syntax highlighting coverage expansion: from 75 default syntect grammars to 210+ via two-face crate (bat-curated set). Newly supported: TOML, Dockerfile, TypeScript (native), Swift, Kotlin, INI, DotENV, CMake, and more. Filename-based detection added (Cargo.lock → TOML, CMakeLists.txt → CMake, .gitignore → Git Ignore, etc.). Binary size increase ~550KB (#320)

- `tako setup` の品質向上: 既存グローバル指示ファイルと同梱推奨ルール（7 項目）の項目レベル差分比較（不足の可能性を具体的に提示、差分ゼロは「差分なし」明示、表示のみでファイル不変）、完了サマリに「次の一歩」（`tako master` の最短導線 + プロファイルの現在値と説明）、tako 内での対話 setup 完了直後の master 開始提案。コマンド案内は最簡形に統一（default プロファイルは `tako master`、`-default` を見せない）。「最も簡単なコマンドを提案する」原則を AGENTS.md / .agent/conventions.md に明文化 (#322)
  `tako setup` quality: item-level diff of existing global instructions against bundled recommended rules (7 sections; concrete gaps reported, "no diff" stated explicitly, display-only), completion summary now includes "next steps" (shortest path via `tako master` + current profile values), and offers to start master right away when run interactively inside tako. Command guidance unified to simplest form (`tako master`, never `-default`); "suggest the simplest command" principle documented in AGENTS.md / .agent/conventions.md (#322)

- 左サイドバー（Files ツリー）の境界ドラッグリサイズ: 右端をドラッグして幅変更、最小 120px / 最大ウィンドウ幅 50% でクランプ、幅は settings.json に永続化、CLI `tako panel --sidebar-width` / MCP `sidebar_width` で操作可能 (#307)
  Sidebar drag resize: drag the right edge to adjust Files sidebar width, clamped to 120px min / 50% of window max, persisted in settings.json, controllable via CLI `tako panel --sidebar-width` / MCP `sidebar_width` (#307)

- 対話コマンドのペイン委譲 `tako run-interactive`: sudo パスワード・ブラウザ認証等のユーザー入力が必要なコマンドを可視ペインに委譲。split → タイトル設定 → コマンド投入をアトミックに実行し、exit code 回収と auto_close で後片付けまで自動化。MCP `tako_run_interactive` / `tako_run_interactive_status` + CLI 1:1（計 94 ツール）(#305)
  Interactive command delegation `tako run-interactive`: delegates commands requiring user input (sudo, browser auth) to a visible pane. Atomically splits, titles, and runs the command with exit code recovery marker. Auto-close on success (configurable). MCP `tako_run_interactive` / `tako_run_interactive_status` + CLI 1:1 (94 tools total) (#305)

- 委任台帳: spawn/run 時に task_type × model × 結果を自動蓄積 + 検収記録 CLI + ユーザーフィードバック反映 + 判断基準の二層化 (#292)
  Delegation ledger: auto-records task_type × model × outcome on spawn/run, CLI/MCP for acceptance recording (record/amend), judgment criteria two-layer injection (built-in defaults + local overrides), survey frequency control (#292)


- Displayed code, Markdown, image, and PDF previews now live-reload after external file changes (#233). Native OS file events watch only the non-recursive parent directories of displayed files; 300ms debouncing coalesces rapid AI writes, and all rereading, syntect / pulldown-cmark work, image loading, and PDF rasterization run in the background before an atomic UI swap. Scroll position, code/Markdown mode, image/PDF zoom and pan are preserved. Editing buffers are never overwritten; external changes surface through the existing conflict state. The setting defaults on, persists in `settings.json`, and is available 1:1 through dispatch `PreviewReload`, `tako preview-reload [on|off]`, and MCP `tako_preview_reload` (80 tools total). Video remains excluded so playback state is not reset
  表示中のコード・Markdown・画像・PDF が外部ファイル変更後にライブリロードされるようにした（#233）。OS ネイティブイベントで表示ファイルの親ディレクトリだけを非再帰監視し、300ms デバウンスで AI の連続 write をまとめる。再読み込み、syntect / pulldown-cmark、画像読み込み、PDF ラスタライズはすべて background で完成させてから UI を差し替える。スクロール位置、code/Markdown モード、画像/PDF の倍率とパンは保持する。編集バッファは上書きせず、既存の競合状態で通知。設定は既定 ON・`settings.json` 永続化で、dispatch `PreviewReload`、`tako preview-reload [on|off]`、MCP `tako_preview_reload`（全 80 ツール）に 1:1 公開。動画は再生状態をリセットしないため対象外


- PDF and image previews now support content zoom without resizing the pane (#234): pinch, Cmd+/Cmd-, modified scroll, header SVG controls, and a compact percentage indicator cover 25–400%; two-finger scrolling pans the enlarged content and Cmd+0 / clicking the percentage returns to fit width. PDF selection bounds follow the actual zoomed and panned page image. The same state is available 1:1 through dispatch `PreviewView`, `tako preview --zoom 150 --page 3`, and MCP `tako_preview_view` (75 tools total), including page selection, pan deltas, reset, and state reads
  PDF・画像プレビューを、ペイン寸法を変えずにコンテンツズーム可能にした（#234）: ピンチ、⌘+/⌘-、修飾キー付きスクロール、ヘッダの SVG 操作と小型倍率表示で 25〜400% に対応。拡大中は 2 本指スクロールでパンし、⌘0 / 倍率クリックで幅フィットへ戻る。PDF の選択矩形はズーム・パン済みの実ページ画像へ追従する。同じ状態を dispatch `PreviewView`、`tako preview --zoom 150 --page 3`、MCP `tako_preview_view`（全 75 ツール）へ 1:1 公開し、ページ指定・パン差分・リセット・状態取得に対応


- `tako setup` now supports claude / codex / agy end to end (#226): it detects every installed CLI, auto-selects a single candidate or presents an authenticated multi-choice list, reads available auth/plan signals without logging credentials, asks only for unavailable Claude / GPT / Google plan details, and generates a plan-sized `profiles/default.yaml` recommendation (CLI-default models, scaled effort, multi-agent delegate policy). Existing system-prompt and project customizations are preserved. A scratch-HOME/PATH verifier covers the single-Claude and multi-CLI flows
  `tako setup` を claude / codex / agy に全面対応（#226）: インストール済み CLI の全検出、単一時の自動選択、複数時の認証状態つき選択、認証情報を出力しないプラン自動検出、取得不能な Claude / GPT / Google プランだけの質問、プラン規模別 `profiles/default.yaml` 推奨（CLI 既定モデル・effort・複数エージェント delegate）を追加。既存 profile の system prompt / projects カスタマイズは保持する。スクラッチ HOME / PATH の検証スクリプトで Claude 単独・複数 CLI の両フローを実測


### Changed / 改善

- タブバーを全ウィンドウ共有にする (#380)
  どのウィンドウのタブバーにも全タブが表示され、New Window 直後から既存タブが全部
  見える。タブをクリックするとそのウィンドウへ表示が自動で移り（`tako window move-tab`
  と同一経路。「同一タブは同時に 1 ウィンドウのみ表示」の排他は維持）、他ウィンドウで
  表示中のタブには W<番号> バッジが付く。⌘数字・次/前タブ巡回も全タブ基準に統一。
  The tab bar is now shared across all windows (#380): every window shows every
  tab (immediately after New Window too). Clicking a tab moves its display to
  that window (same path as `tako window move-tab`; the one-window-per-tab
  exclusivity is kept), and tabs displayed in another window get a W<n> badge.
  Cmd+number and next/prev tab cycling now operate over all tabs.


- 復元・orphan 自動復帰の防御的堅牢化 (#381)
  空レイアウト（タブ / ペイン 0 個）の保存を拒否して既存 layout.json を保護、復元成功時に
  良品スナップショット `layout.json.good` を保全（`tako recover --apply good` で復旧可能、
  一覧にも表示）。orphan 自動復帰は unwrap / アクティブタブ前提 / split 失敗時の孤児登録を
  総点検で除去し、復帰処理全体を catch_unwind で包んでパニックでも起動を続行する
  （9 セッション一括復帰 ×5 回 + 復元併発 ×2 回の隔離反復で安定を実測）。
  Hardened restore and orphan auto-recovery: empty layouts are rejected on save,
  a known-good snapshot (`layout.json.good`) is preserved after each successful
  restore (restorable via `tako recover --apply good`), and the recovery path was
  audited (no unwrap / active-tab assumptions / orphaned registrations) and wrapped
  in catch_unwind so a panic can no longer take down the whole app at startup.


- タブ D&D 並べ替え時のドロップ先挿入位置インジケータを改善 (#371)
  ドラッグ中に挿入位置を示す縦線バー（3px + accent glow）を表示、ソースタブを
  半透明 + 点線ボーダーで「掴まれた」状態に変化。ライト/ダーク両テーマ対応。
  Tab D&D indicator now shows a vertical bar (3px + accent glow) at the drop
  position, and the dragged tab becomes translucent with a dashed border.


- claude session スキャンの Node 起動コスト削減 (#368)
  `claude agents --json` の 5 秒毎無条件実行（1 コア 4% 相当の常時消費）を 3 層で最適化:
  スキャン間隔 5s→30s + イベント駆動即時スキャン、前段ガード（実行中子プロセスの
  有無で Node 起動自体をスキップ）、TTL 2s→5s（watch 重複抑制）。
  アイドル時の Node 起動を完全排除（実測 12 回/分→0 回/分）

  Reduce CPU cost of the claude session scanner (#368). The unconditional
  5-second `claude agents --json` poll (≈4% of one core) is optimised in three
  layers: scan interval 5s→30s with event-driven immediate re-scan on spawn /
  prompt delivery, a pre-check guard that skips the Node launch entirely when no
  child processes are running in backend sessions, and TTL 2s→5s to deduplicate
  watch/worker_status calls. Idle Node launches drop from 12/min to 0/min.

- sleep guard の busy 判定を UI スレッドから background へ移動 (#340)
  persist 復元後の Unknown ペイン常在時、2 秒毎の子プロセス判定（tmux + ps 実行）が
  UI スレッドを p50 42ms 専有し続けていた（#324 で導入、#212 の pmset と同型）。
  判定を background executor へ移し、UI 側は Unknown ペインの収集のみに変更

  Move sleep guard busy detection off the UI thread to the background (#340).
  With Unknown panes persisting after restore, the 2-second child-process check
  (spawning tmux + ps) was occupying the UI thread for p50 42ms per tick
  (introduced in #324; same pattern as the #212 pmset issue). The check now runs
  on the background executor; the UI thread only collects Unknown pane names.


- `tako setup` now completes the standard authenticated single-CLI flow with zero questions (#262): values resolve in detected → previous → default order with source labels, repeated runs are idempotent, detection wins over stale previous plans with an explicit notice, and a final summary replaces repeated confirmations and the unconditional setup-agent dialog. `--yes` is stdin-independent; `--answers <json|@file|->` supplies agent, plans, instructions, profile, projects, orchestrator behavior, and sleep guard non-interactively. The same payload is available through dispatch `SetupRun` and MCP `tako_setup`, enabling an AI to translate Japanese preferences into a complete setup. `--review` retains the explicit conversational review path
  `tako setup` の認証済み単一 CLI 標準フローを質問ゼロへ刷新（#262）。値を detected → previous → default の順に source ラベルつきで解決し、再実行を冪等化。前回プランと再検出値が違えば通知して検出値を優先し、確認連打と setup agent の無条件起動を最終サマリへ置換した。`--yes` は標準入力非依存、`--answers <json|@file|->` は agent・plan・指示・profile・projects・orchestrator 挙動・sleep guard を非対話指定できる。同じペイロードを dispatch `SetupRun` / MCP `tako_setup` に公開し、AI が日本語の希望を完全な setup へ変換可能にした。明示的な対話見直しは `--review` で維持


### Fixed / 修正

- 赤ボタン close → Dock 復帰の TakoApp 二重生成による全タブ消失を根治 (#381)
  最後のウィンドウを赤ボタンで閉じて Dock から復帰すると TakoApp が再生成され、
  旧インスタンスがゾンビ化（CLI / MCP 接続先の分裂・layout 保存の競合）、復元 spawn の
  `-A -D` がゾンビ側の tmux クライアントを強奪して Exited 連鎖 → 縮退 layout 上書き・
  silent death を起こしていた。Dock 復帰・メニューの New Window は生存ワークスペースの
  ウィンドウを開き直す方式に変更（復元も spawn も経ない）。「最後の 1 枚」判定も
  論理ウィンドウ数から GPUI ウィンドウ数に修正。あわせてパニックのローカル記録
  （`<data_dir>/panic.log`、テレメトリ設定と無関係に常時）と終了処理の開始ログを追加し、
  痕跡なしのプロセス消滅を事後調査できるようにした。
  Fixed total tab loss caused by duplicated TakoApp instances on red-button close →
  Dock reopen: the stale instance kept running (split CLI/MCP endpoints, conflicting
  layout saves) and the new instance's restore spawn (`-A -D`) took over its tmux
  clients, cascading pane exits into a degraded layout overwrite or silent death.
  Dock reopen and menu New Window now reopen windows of the live workspace without
  re-restoring. Also added always-on local panic logging (`panic.log`) and an
  app-quit trace for post-mortem of silent process deaths.


- Web ビューにフォーカスがあるとき tako のグローバルショートカット（⌘K / ⌘T / ⌘W 等）が効かない問題を修正: macOS の NSEvent local monitor で ⌘ キーイベントを先取りし、first responder が WKWebView なら GPUI の content view へ戻す。⌘C/⌘V 等の編集系は webview へそのまま渡す。コマンドパレット等のオーバーレイ表示中は webview を非表示にして重なりも解消 (#326)
  Fixed global shortcuts (Cmd+K / Cmd+T / Cmd+W, etc.) not working when a Web view pane has focus: installed a macOS NSEvent local monitor that intercepts Cmd-modified key events and switches the first responder from WKWebView back to the GPUI content view for tako shortcuts while passing editing keys (Cmd+C/V, etc.) through to the webview. Also hides webviews when overlays (command palette, close confirm) are active to prevent z-order conflicts (#326)

- master ペインの workers ドロップダウンの一覧項目に背景色がなく背後のターミナル文字が透けて見える問題を修正: GPUI の描画順でメニューがターミナルテキストエリアの前に描画されていたのをペイン div の最後尾に移動 (#341)
  Fixed workers dropdown list items having no background with terminal text bleeding through: moved the absolute-positioned menu to be painted last within the pane div, after the terminal text area (#341)

- IME の変換候補ウィンドウが途中から表示されなくなる問題を修正: GPUI の focus 喪失（外部 a11y 経由の blur 等）で input handler が登録されなくなり、日本語 IM の printable キーが IME を素通りして ASCII のまま terminal へ入り続ける構造欠陥に、render での focus 自己修復を追加。変換中ペインの close で IME 状態を畳む・確定先の stale ペイン防御・zed 準拠の invalidateCharacterCoordinates 呼び出し（確定時 / フォーカスペイン切替時）も追加 (#332)
  Fixed IME conversion candidate window disappearing mid-session: when GPUI focus is lost (e.g. externally-triggered a11y blur) the input handler is never registered, so Japanese IME printable keys bypass the IME and land as raw ASCII. Added focus self-healing in render, folding of IME composition state when its target pane closes, stale commit-target fallback to the focused pane, and zed-style invalidateCharacterCoordinates on commit / pane-focus change (#332)

- PDF プレビューのリンク ⌘クリックが無反応だった問題を根治: ページ画像 bounds をテキストレイヤからの逆算ではなく描画時に直接記録する方式に変更。テキストのないページでもリンクが動作し、全描画ページのリンクをチェック、ホバー時のカーソル変化 + 下線ハイライトを追加 (#315)
  Fixed PDF preview link Cmd+click being unresponsive: page image bounds are now recorded directly during rendering instead of reverse-estimated from text layers. Links work on text-free pages, hit-testing checks all rendered pages, and hover visual feedback (cursor + underline) was added (#315)

- sleep-guard: 蓋閉じ運用時にディスプレイが点灯したままになる問題を修正。disablesleep=1 中に蓋閉じを検知したら pmset displaysleepnow で画面だけ消灯 (#311)
  sleep-guard: fixed display staying on when lid is closed with disablesleep=1. Now forces display sleep via pmset displaysleepnow when lid closure is detected while system sleep is disabled (#311)

- TAKO_ISOLATED の隔離セルフテストが本番 ledger.yaml に書き込む問題を修正: orchestrator の config_dir を data_dir() ベースに切り替え + ledger prune コマンドの追加（CLI / MCP 1:1）(#303)
  Fixed isolated selftest writing to production ledger.yaml: switched orchestrator config_dir to data_dir()-based resolution (respects TAKO_DATA_DIR / TAKO_ISOLATED) + added ledger prune action for cleanup (CLI / MCP 1:1) (#303)

- MCP 登録パスが消失しても検知・自己修復されない問題を修正: 安定パス優先解決 + ヘルスチェック + master 起動時警告 (#299)
  Fixed MCP registration pointing to a dead binary path going undetected: stable path resolution (/Applications priority), health check on existing registrations, and master startup warning (#299)

- master がタスク受付時に登録プロジェクトの照合を最優先で行うよう順序制約を追加（プロジェクト名の誤認防止）(#263)
  Added project resolution gate (Step 0) to master task intake: registered projects are matched before general exploration or browser access (#263)

- nightly-release が launchd 環境（npm 不在の PATH）で GitHub Release 未作成のまま停止する問題を修正 (#297)
  Fixed nightly-release stopping without creating GitHub Release when npm is not in PATH (launchd environment) (#297)


- Preview image memory is now bounded by a configurable byte-budgeted LRU (#258). PDF pages are decoded only around the visible page, and eviction explicitly removes both GPUI CPU assets and GPU atlas textures; replaced video frames are also dropped from the atlas. Live reload rasterization is single-flight with one latest retry, completed async run history is capped at 256, and `tako preview-cache [max_mb]` / MCP `tako_preview_cache` expose the 512MiB default budget and live usage
  プレビュー画像メモリを設定可能なバイト予算つき LRU で上限化（#258）。PDF は表示ページ近傍だけをデコードし、退避時は GPUI の CPU asset と GPU atlas texture の両方を明示解放し、置換済み動画フレームも atlas から除去する。ライブリロードのラスタライズは single-flight + 最新 1 件再実行、非同期 run 完了履歴は 256 件上限とし、`tako preview-cache [max_mb]` / MCP `tako_preview_cache` で既定 512MiB の予算と利用状況を公開


- PDF text selection no longer turns into a whole-document selection when dragging from line gaps or page margins (#231). PDF pages are also re-rasterized in the background at the quantized device scale × zoom × viewport width, making Retina and zoomed text sharp while preserving the path-and-raster-keyed `PreviewImageCache`
  PDF の行間・ページ余白からドラッグしたとき全文選択になる不具合を修正（#231）。PDF ページは device scale × zoom × 表示幅を量子化した解像度で background 再ラスタライズし、path + raster key の `PreviewImageCache` を維持したまま Retina・ズーム表示の文字を鮮明化


- UI thread no longer blocks on a `pmset` subprocess every 2 seconds (#212): the sleep-guard AC-power check (introduced in v0.5.0 via #173) ran `pmset -g batt` synchronously on the UI thread from the 2-second periodic tick — 20–30ms per call even when idle, stretching to multi-second stalls under CPU saturation (e.g. 4 parallel `cargo build` workers), which surfaced as a sluggish screen, flickering terminal text, and janky scrolling. Replaced with an IOKit FFI call (`IOPSGetTimeRemainingEstimate`, microseconds). Measured: isolated idle instance `periodic_prep` p50 17–59ms / max 116ms → p50 0ms / max 8ms. Also: per-step sub-spans (`periodic_prep:*`) added to perf diagnostics for future attribution, and perf.log lines no longer interleave when written from multiple threads
  UI スレッドが 2 秒毎に `pmset` サブプロセスでブロックされる問題を修正（#212）: sleep guard の AC 電源判定（v0.5.0 の #173 で導入）が 2 秒毎の定期 tick から `pmset -g batt` を UI スレッドで同期実行しており、アイドルでも 1 回 20〜30ms、CPU 飽和時（cargo build 4 並走等）は秒級まで伸びて「画面が重い・ターミナル表記の点滅・スクロールもっさり」として顕在化していた。IOKit FFI（`IOPSGetTimeRemainingEstimate`、マイクロ秒）へ置換。実測: 隔離アイドル環境の `periodic_prep` p50 17〜59ms / max 116ms → p50 0ms / max 8ms。あわせて perf 診断にステップ別サブスパン（`periodic_prep:*`）を追加し、perf.log の複数スレッド並行書き込みによる行混線も修正


### Documentation / ドキュメント

- threat model を `.agent/threat-model-remote.md` に新設（信頼境界・Tailscale 侵害時の
  挙動・CT log 露出・ペアリング承認の人間限定・残存リスク）。
  `.agent/architecture.md` の「リモート機能は持たない」矛盾を解消。
  *New threat model at `.agent/threat-model-remote.md`. Resolved "no remote" contradiction
  in architecture.md.*
## [0.5.8] - 2026-07-20

Nightly patch release (automated). Changes since v0.5.7:
夜間パッチリリース（自動）。v0.5.7 以降の変更:

- [ドキュメント] activeContext / progress を #381 完了で更新
- [修正] 赤ボタン close → Dock 復帰の TakoApp 二重生成による全タブ消失を根治 + 復元の防御的堅牢化 (#381) (#400)
- [機能追加] Finder D&D 改善: ツリー追加 + cmd ドロップ出し分け (#219, #21) (#399)
- [改善] Web ビュー改善: target=_blank 対応 / SVG ツールバー / アドレスバー (#335, #336, #337) (#398)
- [改善] セルフテスト基盤改善: スナップショット検証化 + flaky 安定化 (#358, #343) (#397)
- [修正] Web dock URL 入力のセルフテスト項目 78 を追加 (#375)
- [ドキュメント] activeContext / progress を #391 修正完了で更新
- [修正] setup: 対話エージェントの既定起動を復元 (#391) (#396)
- [機能追加] PDF プレビューのドラッグ選択で端到達時の自動スクロールとページまたぎ選択 (#309) (#395)
- [修正] リサイズ・レイアウト変化時のペイン暗転を根治 (#385) (#394)
- [ドキュメント] activeContext / progress を #357 完了で更新
- [スタイル] UI 掃除: workers 起動ボタン + 失敗トースト/再試行を撤去 (#392)
- [改善] 利用制限表示にリロードボタン追加 + agy を unsupported 明示表示に (#357) (#393)
- [ドキュメント] activeContext / progress を #372 修正完了で更新
- [修正] sleep-guard: 全バックエンドの子プロセス判定で busy 漏れを根治 (#372) (#389)
- [改善] エラーレポート自動送信の Phase 2: 送信キュー・PII マスキング強化・重大エラーフック (#333) (#388)
- [ドキュメント] activeContext / progress を #369 + #374 修正完了で更新
- [改善] orchestrator: probe 一括化 + report --messages (#369 #374) (#387)
- [ドキュメント] activeContext / progress を #315 R2 修正完了で更新
- [修正] PDF リンクの cmd ホバー/クリックが .app 環境で不発になる問題を根治 (#315) (#386)
- [修正] verify_pid_identity の fail-open を fail-safe に変更 (#329) (#384)
- [ドキュメント] activeContext / progress を #375 修正完了で更新
- [ドキュメント] activeContext / progress を #378 実装完了で更新
- [修正] Web dock URL 入力欄のフォーカス不在を修正 (#375) (#383)
- [機能追加] タブ名の自動命名: source パラメータ + 命名規則設定 + master プロンプト追記 (#378) (#382)
- [改善] claude session スキャンの Node 起動コスト削減 (#368) (#376)
- [ドキュメント] activeContext / progress を #371 実装完了で更新
- [スタイル] タブ D&D 並べ替えのドロップ先挿入位置インジケータを実装 (#371) (#373)

## [0.5.7] - 2026-07-19

Nightly patch release (automated). Changes since v0.5.6:
夜間パッチリリース（自動）。v0.5.6 以降の変更:

- [ドキュメント] activeContext / progress を #340 監査完了で更新
- [改善] sleep guard の busy 判定を UI スレッドから background へ移動 (#340) (#370)
- [ドキュメント] activeContext / progress を #339 実装完了で更新
- [機能追加] 複数ウィンドウ対応: ビューポート方式で別ウィンドウに別タブを表示 (#339) (#367)
- [ドキュメント] activeContext / progress を #364 実装完了で更新
- [機能追加] orchestrator report: scrollback + transcript 2 層で worker 報告を取得 (#364) (#366)
- [ドキュメント] activeContext / progress を #338 再修正完了で更新
- [修正] チェンジログビューの git 検出が .app 環境で全滅する問題を根治 (#338) (#365)
- [ドキュメント] activeContext / progress を #308 再修正完了で更新
- [ドキュメント] activeContext / progress を #312 再修正完了で更新
- [修正] タブ D&D がウインドウ移動に食われる競合を根治 (#308) (#363)
- [修正] 赤ボタン close → Dock 復帰でタブが空になるバグを根治 (#312) (#362)
- [ドキュメント] activeContext / progress を #321 再修正完了で更新
- [修正] ステータスバーのサービス切替ドロップダウンが開かない問題を根治 (#321) (#361)

## [0.5.6] - 2026-07-18

Nightly patch release (automated). Changes since v0.5.5:
夜間パッチリリース（自動）。v0.5.5 以降の変更:

- [ドキュメント] activeContext / progress を #287 修正完了で更新
- [改善] Web ビューの読み込み失敗にエラー表示 + リトライ導線を追加 (#327) (#360)
- [ドキュメント] activeContext / progress を #357 完了で更新
- [機能追加] codex の利用制限データ取得: ドロップダウンの「--」を実データに (#357) (#359)
- [修正] IME 変換候補ウィンドウが途中から表示されなくなる問題を修正 (#332) (#350)
- [改善] tako setup の品質向上: ルール項目比較・次の一歩案内・最簡形コマンド案内 (#322) (#330)
- [修正] Web ビューフォーカス時のグローバルショートカット不発を修正 (#326) (#354)
- [機能追加] ステータスバー利用制限表示の改修: サービス切替ドロップダウン + 7d 表記 (#321) (#355)
- [修正] MCP ツールカタログ期待値を 98 に修正（#308 と #338 の squash merge 競合） (#356)
- [機能追加] ターミナルの選択ドラッグ中に上下端到達で自動スクロール (#310) (#353)
- [機能追加] タブバーの D&D 並べ替え + CLI/MCP 操作 (#308) (#352)
- [機能追加] プレビューペインにチェンジログビュー切替 (#338) (#348)
- [ドキュメント] activeContext / progress を #320 完了で更新
- [修正] workers ドロップダウンの背景透過を修正 (#341) (#346)
- [改善] シンタックスハイライト対応形式を 75→210+ に拡充 (#320) (#351)
- [ドキュメント] activeContext / progress を #314 完了で更新
- [改善] ファイルツリー右クリメニュー改善: デフォルトアプリ/アプリ選択で開く + 見切れ修正 (#314) (#349)
- [ドキュメント] activeContext / progress を #333 完了で更新
- [機能追加] エラーレポートの自動送信基盤（テレメトリ）(#333) (#345)
- [修正] 不正 URL による Web ビュー panic クラッシュを根治 (#334) (#347)
- [機能追加] worker の permission ダイアログ検知 + 構造化応答 API (#319) (#344)
- [修正] run-interactive の余分な Enter 注入と exit マーカー行頭限定を根治 (#325) (#342)
- [ドキュメント] activeContext / progress を #313 完了で更新
- [修正] git タブがファイルツリーの表示リポジトリに追随しない問題を根治 (#313) (#331)
- [ドキュメント] activeContext / progress を #324 完了で更新
- [修正] sleep-guard の busy_agents が復元 worker を数えない問題を根治 (#324) (#328)
- [ドキュメント] activeContext / progress を #315 完了で更新
- [修正] PDF プレビューのリンク ⌘クリック無反応を根治 (#315) (#323)
- [ドキュメント] activeContext / progress を #312 完了で更新
- [修正] macOS ウインドウ操作: タブバードラッグ移動 + 赤ボタン後の Dock 復帰 (#312) (#318)
- [修正] sleep-guard: 蓋閉じ運用時にディスプレイが点灯したままになる問題を修正 (#311) (#317)
- [ドキュメント] activeContext / progress を #307 完了で更新
- [機能追加] 左サイドバーのドラッグリサイズ + 幅の永続化・CLI/MCP 操作 (#307) (#316)
- [機能追加] 対話コマンドのペイン委譲 run-interactive を MCP/CLI の標準動作として実装 (#305) (#306)
- [修正] TAKO_ISOLATED の隔離セルフテストが本番 ledger.yaml に書き込む問題を根治 (#303) (#304)
- [修正] MCP 登録パス消失の検知・自己修復を追加 (#299) (#302)
- [機能追加] 委任台帳: タスク×モデル×結果の自動蓄積 + 検収記録 CLI + 判断基準の二層化 (#292) (#301)
- [修正] master タスク受付にプロジェクト照合の順序制約を追加 (#263) (#300)
- [修正] nightly-release の npm 不在による GitHub Release 未作成を根治 (#297) (#298)
- [修正] agents idle をバックグラウンドシェルの子プロセスで覆さない (#289) (#293)

## [0.5.5] - 2026-07-17

Nightly patch release (automated). Changes since v0.5.4:
夜間パッチリリース（自動）。v0.5.4 以降の変更:

- [機能追加] setup 完了後にエージェント起動ランチャーを追加 (#295) (#296)
- [改善] remote の dispatch 統合 + WS broadcaster 化 + API v2 (#281) (#294)
- [修正] remote daemon の封じ込め修正 (#280) (#290)
- [修正] self/spawn の caller 解決に pid 祖先辿りを一次化 (#288) (#291)
- [ドキュメント] Tailscale Serve PoC 実測レポート (#279)
- [ドキュメント] tako remote 全面刷新計画（Tailscale一本化 + UI刷新）に改訂

## [0.5.3] - 2026-07-15

Nightly patch release (automated). Changes since v0.5.2:
夜間パッチリリース（自動）。v0.5.2 以降の変更:

- [ドキュメント] Issue 258の完了状態を記録 (#258) (#261)
- [修正] アプリ全体のメモリ肥大を抑制 (#258) (#260)
- [修正] PDF プレビューの周期的暗転を根治: イベントフィルタ強化 + ファイルスタンプ比較 + ダブルバッファ化 (#257) (#259)

## [0.5.2] - 2026-07-15

Nightly patch release (automated). Changes since v0.5.1:
夜間パッチリリース（自動）。v0.5.1 以降の変更:

- [修正] MCP 全ツールの未知パラメータを検出してエラーにする (#227) (#255)
- [改善] tmux タブのリファクタ: 表示情報の充実・復帰ボタン・orphan 判定改良 (#183) (#254)
- [機能追加] プレビューペインのバックグラウンド退避対応 (#230) (#253)
- [機能追加] 受け入れゲートの状態機械: タスクに機械検証可能な述語を持たせる (#244) (#252)
- [機能追加] プレビュー目次ナビゲーションを実装 (#232) (#251)
- [機能追加] worker 異常イベントの種別拡張: question / model_switched / context_high (#243) (#250)
- [機能追加] worker タスクのチェックポイント永続化と resume 操作 (#242) (#249)
- [ドキュメント] #233 の完了状態を記録 (#248)
- [機能追加] プレビューのライブリロードを実装 (#233) (#247)
- [機能追加] メニューバー拡充: Open Directory/Repository/Remote/Recent + CLI/MCP 1:1 (#20) (#246)
- [ドキュメント] LangGraph 概念の tako 翻訳: オーケストレーション設計メモ (#161) (#245)
- [ドキュメント] PDF品質改善とズームの完了状態を記録 (#231) (#234) (#241)
- PDFプレビュー品質改善とPDF・画像ズーム対応 (#231 / #234) (#240)
- [修正] nightly-release.sh: worktree からの launchd 登録で本体リポへ正規化 (#205) (#239)
- [修正] AI のコマンド操作でフォーカスを奪わないよう統一 (#211) (#238)
- [改善] 狭ペインのヘッダを「...」メニューに集約: 最小化/クローズを選択式に (#229) (#237)
- [機能追加] 外部ファイル D&D の挙動をドロップ先で出し分け (#21) (#236)
- [機能追加] setup をマルチエージェント対応 (#226) (#235)
- [修正] worker の idle/busy 検知精度を改善: 偽 IDLE 根治 + 停滞検知 + 折りたたみ対策 (#224) (#228)
- [改善] 小ペインでの UI 見切れ解消 + プレビューヘッダ刷新 + 右クリックメニュー (#185) (#225)
- [機能追加] ペインのタブ横断 D&D: タブバーへドロップで新タブ化・既存タブへドロップで合流 (#209) (#223)
- [改善] タブバーのオーバーフロー対応: タブ幅自動縮小 + 横スクロール + 自動スクロールイン (#208) (#222)
- [ドキュメント] progress.md に #210/#217 のエントリを追記 + .vite/ を gitignore へ
- [スタイル] UI 大刷新: Claude Design カンプの忠実再現 + 絵文字全廃 + 新規コントロール (#217) (#221)
- [機能追加] sleep-guard の蓋閉じ（lid-close）対応 (#218) (#220)
- [修正] UI スレッドの pmset 同期実行を IOKit FFI へ置換し画面の重さ・点滅を根治 (#212) (#216)
- [修正] 復元後の master role 消失と同一プロファイル複数 master の self/spawn 誤認を根治 (#210) (#215)
- [機能追加] ステータスバーの 🌐 ボタンから Web ビューペインを開く (#207) (#214)
- [改善] mp4 プレビューの操作性改善: ホバー時刻・音量・ループ (#22) (#213)

## [0.5.1] - 2026-07-14

Nightly patch release (automated). Changes since v0.5.0:
夜間パッチリリース（自動）。v0.5.0 以降の変更:

- [改善] プレビュー検索の polish: ヒットハイライト描画・フィールドクリックフォーカス・IME 未確定表示 (#200) (#206)
- [リファクタ] ControlHost trait を 8 つの責務別サブトレイトへ分割 (#86) (#204)
- [改善] MCP HTTP サーバーをリクエスト毎スレッド化し並行処理を可能にする (#84) (#203)
- [機能追加] タブ単位の退避を CLI / MCP から操作可能にする (#85) (#202)
- [機能追加] orchestrator_run の非同期化 (#121) (#201)
- [修正] 検索バーの GUI 直接テキスト入力を実装 (#195)
- [機能追加] master 自己特定 + ctx 監視 + handoff コマンド (#123, #193) (#198)
- [機能追加] プレビュー編集の強化: 自動保存・undo/redo・検索/置換 (#195) (#197)
- [機能追加] セッション会話ログの管理と復元: カタログ + ペイン平文ログ (#112) (#196)
- [ドキュメント] CHANGELOG: #165 レイアウトエンジンを [0.4.0] から [0.5.0] へ移動 (#165)

## [0.5.0] - 2026-07-14

### Added

- Background persistence now automatically recovers orphan tmux sessions on startup (#191): when tako restarts after a crash or `kill -9` where layout.json couldn't be saved in time, surviving `tako-*` sessions are auto-discovered and placed into a "Recovery" tab — no manual `tako recover` or `tako tmux open` needed. Recovered sessions join the protected set so orphan cleanup won't kill them. Secondary mode, persist OFF, and tmux-absent environments are unaffected
  バックグラウンド永続化が起動時に orphan tmux セッションを自動復帰するようになった（#191）: クラッシュや kill -9 で layout.json の保存が間に合わなかった場合、生存している `tako-*` セッションを自動発見し「復帰」タブにまとめて配置する。手動の `tako recover` / `tako tmux open` が不要に。復帰セッションは保護リストに入るため orphan cleanup で kill されない。セカンダリモード・persist OFF・tmux 不在では影響なし

- Sleep prevention via IOKit power assertions (#173): new `tako sleep-guard status/set` + MCP `tako_sleep_guard` (61 tools) prevent idle sleep while agents are running. Three modes: `off` / `on` (always awake) / `while-agents-running` (automatic), with power condition `ac-only` / `always`. Settings persist to config.yaml. Status bar shows a ☕ badge while the assertion is held. App Nap is also disabled. `tako setup` gains a sleep-prevention level chooser (setup changelog rev 7)
  IOKit 電源アサーションによるスリープ防止機能（#173）: `tako sleep-guard status/set` + MCP `tako_sleep_guard`（計 61 ツール）でエージェント実行中のアイドルスリープを防止する。3 モード: `off` / `on`（常時）/ `while-agents-running`（自動）、電源条件 `ac-only` / `always`。設定は config.yaml に永続化。アサーション保持中はステータスバーに ☕ バッジを表示。App Nap も無効化。`tako setup` にスリープ防止レベルの選択を追加（setup changelog rev 7）

- Close confirmation dialog for tabs and panes (#172): clicking the × button now shows a summary of what will be lost (pane count, running processes, active workers, tmux sessions) and asks for confirmation. cmd+click bypasses the dialog for power users. Enter confirms, Esc/background-click cancels. The setting persists in config.yaml (`confirm_close`, default true). `tako confirm-close` (CLI) + MCP `tako_confirm_close` (60 tools)
  タブ・ペインの × ボタンに確認ダイアログを追加（#172）: × クリックで失われるもの（ペイン数・実行中プロセス・稼働中 worker・tmux セッション）を要約表示し確認を求める。cmd+クリックでダイアログをスキップして即クローズ（パワーユーザー動線）。Enter=確定 / Esc・背景クリック=キャンセル。config.yaml に永続化（`confirm_close`、既定 true）。CLI `tako confirm-close` + MCP `tako_confirm_close`（計 60 ツール）

- Major terminal scrolling overhaul — pixel-based rendering, local history mirror, enhanced scrollbar (#159): scrolling is now sub-line smooth (Zed editor's line-fraction approach adapted for bottom-anchored terminals) instead of discrete line jumps. Trackpad inertia uses macOS momentum events natively. For tmux-backed panes, the old copy-mode approach (which ate keystrokes and stuttered on each round-trip) is replaced with a local history mirror (`scroll_mirror`): history is captured from tmux in 500-line ANSI chunks and rendered entirely locally — no more key swallowing or latency. The scrollbar gains hover persistence, thumb thickening, track visibility on hover, and continuous (sub-line) thumb positioning with drag follow. All scroll operations go through the same path for CLI/MCP (`backend_scroll_view`) as the UI (development invariant)
  ターミナルスクロールの大幅改善 — ピクセル単位描画・ローカル履歴ミラー・スクロールバー強化（#159）: 1 行単位の離散ジャンプからサブライン単位のスムーススクロールへ全面刷新（Zed エディタの行小数方式をターミナルの下端アンカーに翻案）。トラックパッドの慣性スクロールは macOS の momentum イベントでネイティブに動作。tmux バックエンドペインでは旧 copy-mode 方式（キー飲み込み・往復レイテンシによるカクつき）を廃止し、ローカル履歴ミラー（`scroll_mirror`）へ置換: 500 行チャンクの ANSI キャプチャを完全ローカルで描画し、キー消失と遅延を構造的に解消。スクロールバーはホバー維持・サム太化・トラック表示・サブライン連続位置と追従ドラッグを追加。CLI / MCP の Scroll 操作も UI と同一経路（`backend_scroll_view`、開発不変条件）

- Nightly patch releases now run locally via launchd (#166): `scripts/nightly-release.sh` replaces the failed cloud routine — runs daily at 5:00 via launchd, auto-bumps patch version + generates CHANGELOG section + builds + tags + publishes a GitHub Release with the macOS binary when main has changes since the last tag. Safety: no-op on clean main / dirty worktree / manual release in progress / concurrent run. `--dry-run` for testing, `--install-launchd` / `--uninstall-launchd` for job management
  夜間パッチリリースを launchd ローカルジョブ化（#166）: 失敗し続けていたクラウドルーチンを `scripts/nightly-release.sh` に置換。launchd で毎日 5:00 に実行し、前回タグ以降に main へ変更があれば patch bump → CHANGELOG 自動節 → ビルド → バイナリ付き GitHub Release を自動作成する。安全装置: 変更なし / dirty worktree / 手動リリース進行中 / 多重起動でスキップ。`--dry-run` でテスト、`--install-launchd` / `--uninstall-launchd` でジョブ管理

- Worker spawn layout engine (#165): spawning workers no longer squeezes every pane into ever-thinner columns. With the new default `master-reserved` policy, the spawning pane (master) keeps its share of the screen (default 50%, configurable 0.1–0.9) and workers tile inside a dedicated worker area on its right: `grid` (1 worker = full area → 2 = stacked → 3–4 = quadrant cross → more columns as needed, default) or `spiral` (alternating half-splits, golden-ratio style). The worker area is recognized via each pane's `spawned_by` chain, so panes the user opened manually are never rearranged — when a worker closes (MCP/CLI close, UI ×, or process exit), only the worker area reflows and master/user panes keep their exact rectangles. Configure via config.yaml `spawn_layout`, `tako orchestrator layout [--policy master-reserved|legacy] [--master-ratio 0.5] [--algorithm grid|spiral]`, or the new MCP tool `tako_orchestrator_layout` (59 tools total); `legacy` restores the old right-split behavior. Master/solo system prompts now instruct agents to prioritize the readability of the master pane and user-opened panes when rearranging layouts
  worker spawn のレイアウトエンジンを新設（#165）: spawn のたびに全ペインが横へ等分圧縮される問題を解消。新既定の `master-reserved` ポリシーでは spawn 元（master）が画面の取り分（既定 50%、0.1〜0.9 で設定可）を維持し、worker は右側の worker 領域内に配置される: `grid`（1 体=全面 → 2 体=上下 → 3〜4 体=十字四分割 → 以降は列を追加。既定）/ `spiral`（縦横交互の半分割、黄金比風）。worker 領域は各ペインの `spawned_by` チェーンで認識するため、ユーザーが手動で開いたペインが再配置されることはない — worker の close（MCP/CLI・UI の ×・プロセス exit）時も領域内だけがリフローされ、master とユーザー由来ペインの矩形は不変。設定は config.yaml の `spawn_layout`、`tako orchestrator layout [--policy master-reserved|legacy] [--master-ratio 0.5] [--algorithm grid|spiral]`、新 MCP ツール `tako_orchestrator_layout`（計 59 ツール）から。`legacy` で従来の右等分割へ戻せる。master / solo の system prompt に「レイアウト操作時は master とユーザー由来ペインの可読性を最優先する」行動規範を追記

- `tako orchestrator watch` now emits a `WORKER_ERROR: tako:<pane> (<kind>)` event when a worker stalls on a known error pattern instead of reporting a misleading `WORKER_IDLE` (#157). Detected kinds (all patterns taken from captured real screens): `api_error` (claude "API Error: Connection closed mid-response" etc. — a resume nudge usually recovers it), `usage_limit` (claude / codex usage-limit stop — wait for the reset time), and `limit_dialog` (codex's rate-limit model-switch dialog — answer the dialog). Extra `detail:` / `action:` lines follow the event line so the master can make a first-level decision without reading the pane. `tako_orchestrator_worker_status` (MCP) and `tako orchestrator status` (CLI) return the same classification 1:1 as `status: "error"` plus an `error` object (`kind` / `detail` / `recommended_action`: resume / wait_reset / respond_dialog), and `tako_orchestrator_run` returns `status: "worker_error"` with the same `error` object while skipping auto-close so the worker's context stays recoverable. Guards against false positives: no detection while busy (auto-retry "Retrying…" screens stay busy), "limit reached, now using …" auto-model-switch notices are ignored, api_error detection is limited to the bottom 15 lines so stale scrollback errors after a recovery don't re-fire, and normal WORKER_IDLE / WORKER_GONE behavior is unchanged
  `tako orchestrator watch` が、worker が既知のエラーパターンで停止したとき紛らわしい `WORKER_IDLE` ではなく `WORKER_ERROR: tako:<pane> (<種別>)` イベントを出力するようになった（#157）。検知種別（パターンはすべて実採取画面由来）: `api_error`（claude の「API Error: Connection closed mid-response」等 — 続行指示で復帰できることが多い）、`usage_limit`（claude / codex の usage limit 到達停止 — 解除時刻まで待つ）、`limit_dialog`（codex のレートリミット・モデル切替ダイアログ — ダイアログに応答）。イベント行に続けて `detail:` / `action:` 行が付き、master がペインを読まずに一次判断できる。MCP `tako_orchestrator_worker_status` / CLI `tako orchestrator status` は同じ判別を `status: "error"` + `error` オブジェクト（`kind` / `detail` / `recommended_action`: resume / wait_reset / respond_dialog）として 1:1 で返し、`tako_orchestrator_run` は `status: "worker_error"` + 同 `error` オブジェクトを返して auto_close をスキップする（worker の文脈を復帰可能なまま残す）。誤検知ガード: busy 中は判定しない（自動リトライ「Retrying…」画面は busy のまま）、「limit reached, now using …」の自動モデル切替告知は無視、api_error は末尾 15 行限定（復帰後にスクロールバックへ残った古いエラーで再発火しない）、既存の WORKER_IDLE / WORKER_GONE の挙動は不変

### Changed

- Test tmux sockets are now cleaned up reliably (#116): `TmuxTestGuard` replaces scattered per-file cleanup structs, `kill_server` deletes socket files, and `cleanup_stale_sockets` auto-collects leftovers from aborted test runs (previously accumulated 4,500+ zombie sockets)
  テスト用 tmux ソケットの掃除を信頼性改善（#116）: 散在していたファイル単位の `Cleanup` を共通の `TmuxTestGuard` に統一し、`kill_server` でソケットファイルを削除、`cleanup_stale_sockets` で中断テストの残骸を自動回収する（従来は 4,500 件以上のゾンビソケットが蓄積）

### Fixed

- Scrolling now works correctly on reattached and tmux-view panes, and worker-status polling no longer blocks the UI (#181): three root causes kept #159's pixel scrolling from working on real restored sessions: (1) the mirror-scroll path only checked `backend_sessions`, missing TmuxOpen view panes (which fell through to the direct-pane path where alt-screen history is 0); (2) with persist ON, the view pane's outer PTY is itself backend-wrapped, and backend-first resolution picked the outer wrapper (history 0) instead of the view target; (3) after persist restore, view panes weren't registered in `tmux_view_panes` and nest-detection only searched the default tmux server, missing `--socket tako` targets. Additionally, `OrchestratorWorkerStatus` dispatch ran `claude agents --json` (550–1100ms, Node startup) synchronously on the UI thread — 2000+ stalls in 2h20m of perf.log, matching user-reported jank timing. Fixes: unified `mirror_scroll_pane` (backend ∪ view), view-target-first resolution, backend socket added to nest candidates, and worker_status split into snapshot (UI thread) / compute (background)
  再アタッチ・ビューペインでスクロールが効かず UI がカクつく問題を修正（#181）: #159 のピクセルスクロールが実機の復元セッションで効かない根因 3 件を修正: (1) ミラースクロール判定が `backend_sessions` のみで TmuxOpen ビューペインが直接ペイン扱い（alt screen = 履歴 0 で不発）、(2) persist ON では外側 PTY 自体が backend ラップされ backend 優先解決で外側（history 0）へ誤解決、(3) 復元後は `tmux_view_panes` 未登録 + ネスト候補が既定サーバーのみで `--socket tako` のビュー先が辿れない。加えて `OrchestratorWorkerStatus` dispatch が `claude agents --json`（550〜1100ms、Node 起動）を UI スレッドで同期実行（perf.log 2 時間 20 分で 2000 件超、ユーザー報告時刻と一致）。修正: `mirror_scroll_pane`（backend ∪ view）統一、ビュー先優先の実体解決、backend socket をネスト候補に追加、worker_status を snapshot（UI）/ compute（background）に分離

- AI-pinned file tree folders no longer duplicate or show stale entries (#171): `canonicalize()` is now used consistently across add/remove/list and `sync_filetree_roots`, preventing duplicates caused by symlinks (e.g. `/tmp` vs `/private/tmp`) or cwd overlap. Dead-folder pruning (`prune_dead_folders`) runs on sync, list, and layout restore, automatically removing entries whose paths no longer exist on disk
  ファイルツリーの AI 追加フォルダの重複・残骸を修正（#171）: add / remove / list と `sync_filetree_roots` の全経路で `canonicalize()` による正規パス比較に統一し、symlink 経由（`/tmp` vs `/private/tmp` 等）や cwd との重複表示を解消。`prune_dead_folders` を sync・list・layout 復元の 3 経路で実行し、実体が消えたエントリを自動除去する

- Fixed app-wide intermittent freezes and sluggish PDF viewing / prompt typing (#168, #115). perf.log analysis of 3.3 hours of real usage identified three culprits, all confirmed by measurement: (1) the `OrchestratorWorkerStatus` dispatch ran `claude agents --json` (a login shell + Node startup, 500ms–1s per call) plus tmux/ps subprocesses **on the UI thread** — 4124 calls averaging 687ms (max 6.2s, 47 minutes of cumulative UI blocking; every recorded 0.5s+ UI stall co-occurred with it); (2) PDF previews rebuilt `gpui::Image` from all page PNGs **every frame** (full byte-hash per image), degrading frame construction to p50 96ms on a 71-page PDF (normally 2ms); (3) opening a PDF rasterized every page synchronously on the UI thread (1354ms block). Fixes: subprocess-bearing read-only dispatches (worker status / git log / git diff) now collect their context on the UI thread in microseconds and run in the background for both CLI and MCP (`TAKO_OFFLOAD=0` restores the old path; `claude agents --json` also gains a 2s TTL cache with lock serialization), preview images are cached as `Arc<gpui::Image>` per pane and reused while the path is unchanged (PDF viewing: p50 96ms → 1–3ms/frame), and PDF/video loading moved to the background behind a "loading…" placeholder (`tako open` PDF response: 1354ms → 48ms). Measured effect on UI responsiveness: a concurrent `tako list` during worker_status dropped from 159–204ms to 4–5ms. Adds a permanent main-thread stall watchdog (`diag::perf_span`): 32ms+ UI-thread occupations are logged with the culprit's tag, 2s+ hangs are reported mid-flight, `TAKO_PERF_VERBOSE=1` emits per-tag latency distributions every 10s, and `TAKO_PERF_LOG` redirects the log for isolated measurements
  アプリ全体の間欠フリーズと PDF 閲覧・プロンプト入力のモサモサを修正（#168、#115）。実運用 3.3 時間分の perf.log 分析で 3 犯を計測特定: (1) `OrchestratorWorkerStatus` dispatch が `claude agents --json`（ログインシェル + Node 起動 = 1 回 500ms〜1s）+ tmux / ps サブプロセスを **UI スレッドで同期実行** — 4124 回・平均 687ms（最大 6.2s、UI ブロック累計 47 分。記録された 0.5s+ の UI ストールは全件これと共起）、(2) PDF プレビューが**毎フレーム**全ページ PNG から `gpui::Image` を再構築（画像ごとに全バイトハッシュ）し、71 ページ PDF でフレーム構築が p50 96ms に劣化（通常 2ms）、(3) PDF を開く瞬間に全ページラスタライズを UI スレッドで同期実行（1354ms ブロック）。修正: サブプロセスを伴う read-only dispatch（worker status / git log / git diff)は UI スレッドでは µs オーダーの文脈収集だけ行い CLI / MCP 両経路で background 実行（`TAKO_OFFLOAD=0` で旧経路。`claude agents --json` には TTL 2 秒キャッシュ + ロック直列化も追加）、プレビュー画像はペインごとに `Arc<gpui::Image>` でキャッシュし path 不変の間は再利用（PDF 表示中: p50 96ms → 1〜3ms/フレーム）、PDF / 動画のロードは「読み込み中…」プレースホルダの背後で background 化（`tako open` の PDF 応答: 1354ms → 48ms）。UI 応答性の実測効果: worker_status 実行中の並行 `tako list` が 159〜204ms → 4〜5ms。恒久のメインスレッド・ストール診断（`diag::perf_span`）を同梱: 32ms 超の UI スレッド専有を犯人タグ付きで記録、2 秒超のハングは実行中に中間報告、`TAKO_PERF_VERBOSE=1` で 10 秒ごとにタグ別レイテンシ分布、`TAKO_PERF_LOG` で隔離実測用にログ先を変更できる

- Fixed mouse escape-sequence fragments (e.g. `4;45;18M` / `<64;12;17M`) leaking into TUI input fields as plain text (#167). When scrolling a mouse-reporting TUI (claude etc.), tako forwards SGR wheel reports; if the byte stream stalls mid-sequence for more than tmux's escape-time (10ms) — which inertial-scroll floods and UI-thread stalls can cause — tmux commits the lone ESC as a key and forwards the remainder as literal text into the inner app's input field (reproduced against real claude in an isolated tmux). Two-layer fix: backend-pane wheel reports no longer travel through the outer client PTY at all — they are injected directly into the tmux server via `send-keys -H` (structured socket data, immune to splitting/escape-time), with SGR/X10 chosen by the inner app's `#{mouse_sgr_flag}`; and all wheel forwarding is token-bucket rate-limited (150 events/s, burst 8) so in-flight bytes stay far below the PTY buffer during stalls. Excess wheel events are dropped, which is harmless for relative scrolling
  マウスエスケープシーケンスの断片（`4;45;18M` / `<64;12;17M` 等）が TUI の入力欄にテキストとして混入するバグを修正（#167）。マウスレポート要求 TUI（claude 等）のスクロールで tako は SGR ホイールレポートを転送するが、バイト列がシーケンス途中で tmux の escape-time（10ms）を超えて停滞する（慣性スクロールの洪水や UI スレッドのストールで起きる）と、tmux が ESC を単独キーとして確定し残りを平文として内側アプリの入力欄へ流していた（隔離 tmux + 実 claude で再現）。二層で修正: バックエンドペインのホイールレポートは外側クライアント PTY を一切通らず `send-keys -H` で tmux サーバーへ直接注入（ソケット越しの構造化データのため分割・escape-time と無縁。SGR / X10 は内側の `#{mouse_sgr_flag}` で出し分け）+ 全ホイール転送にトークンバケットのレート制限（150 イベント/秒・バースト 8）を導入し、停滞時の飛行中バイト量を PTY バッファより十分小さく保つ。超過ホイールイベントは破棄する（相対スクロールのため無害）

- Fixed a critical bug where all terminal panes could vanish from the UI while their processes kept running in backend tmux sessions (#177). A dev/test instance launched with only `TAKO_DISCOVERY_DIR` isolated would pass the multi-instance guard (which only checked discovery's control.json), restore the production layout.json as primary, and its `new-session -A -D` re-attach would steal every tmux client from the live GUI — killing its PTYs in one sweep, after which the periodic save overwrote the healthy layout with the degraded remnant. Three layers of defense were added: a **restore-takeover guard** that scans `tmux list-clients` before restoring and demotes the new instance to secondary mode when any target session still has a client owned by a live tako-app (works regardless of env-var isolation combinations); a **degraded-save guard** that rotates layout.json into generation backups (`.bak.1`–`.bak.3`) before any save that would drop the pane count below half (with a 10-minute rotation guard so cascading shrinks can't push the healthy generation out); and a **one-shot isolation mode** `TAKO_ISOLATED=1` that isolates discovery, persistence, and the tmux socket together so experimental launches can't half-isolate. persist.log lines now include the writing pid for post-incident analysis
  UI から全ターミナルペインが消失する（実体プロセスはバックエンド tmux セッションで生存）重大バグを修正（#177）。`TAKO_DISCOVERY_DIR` だけを隔離した dev / 検証インスタンスが多重起動ガード（discovery の control.json しか見ない）を素通りしてプライマリ判定になり、本番 layout.json を復元 → 再 attach の `new-session -A -D` が稼働中 GUI の tmux クライアントを全部強奪 → PTY 一斉死亡 → 定期保存が縮退レイアウトで健全な layout.json を上書きしていた。三層の防御を追加: **復元強奪ガード**（復元前に `tmux list-clients` を走査し、対象セッションに生きた tako-app 配下のクライアントが居ればセカンダリモードへ降格。環境変数の隔離組合せに依存しない）、**縮退保存ガード**（ペイン数が半分未満に減る保存の前に layout.json を世代バックアップ `.bak.1`〜`.bak.3` へ退避。連鎖縮退で健全世代が押し出されないよう 10 分の回転ガード付き）、**一括隔離モード** `TAKO_ISOLATED=1`（discovery / persist / tmux socket をまとめて隔離し、実験起動の片脚隔離を構造的に排除）。persist.log の各行に書き込み元 pid を付与し、事後調査を容易にした

- Added `tako recover` for restoring the layout from generation backups after a mass pane loss (#177): bare `tako recover` lists layout.json and its backups (tabs / panes / age), `tako recover --apply <generation>` restores one (stashing the current file as `layout.json.pre-recover`), refusing while a tako instance is running (`--force` to override for unrelated data dirs). Recovery steps are documented in the README troubleshooting section
  ペイン大量消失後にレイアウトを世代バックアップから戻す `tako recover` を新設（#177）: 引数なしで layout.json とバックアップの一覧（タブ / ペイン数 / 更新時刻）、`tako recover --apply <世代>` で復元（現行は `layout.json.pre-recover` へ退避）。tako 稼働中は拒否する（別データディレクトリの tako なら `--force` で上書き可）。復旧手順は README のトラブルシューティングに記載

- Fixed a data-loss bug where a concurrent `projects add` could wipe the entire orchestrator projects.yaml (58 entries → only the added one) (#169). Root cause was a three-part chain: the old save used `std::fs::write` (truncate → write, exposing an empty/partial file to concurrent readers), serde_yaml successfully parses empty/partial content as "0 projects" instead of erroring, and read-modify-write had no cross-process serialization. All config-file writes (projects.yaml, profiles/*.yaml, config.yaml) now go through a new `config_io` layer: atomic writes (tmp + fsync + rename), an exclusive `<path>.lock` file lock serializing read-modify-write across processes, fail-loud behavior that refuses to overwrite an unparseable file (including the profiles-set path that silently reset corrupt profiles to defaults), and automatic rotated backups (`.bak.1`–`.bak.3`) before every content change
  並行 `projects add` で orchestrator の projects.yaml が全消失する（58 件 → add した 1 件だけになる）データ消失バグを修正（#169）。根本原因は三段連鎖: 旧 save が `std::fs::write`（truncate → write の 2 段階で並行プロセスに空 / 部分ファイルが見える）、serde_yaml が空 / 部分内容をエラーにせず「0 件」として成功パース、read-modify-write のプロセス間直列化なし。設定ファイル（projects.yaml / profiles/*.yaml / config.yaml）の書き込みを新設の `config_io` 層へ集約: アトミック書き込み（tmp + fsync + rename）、`<path>.lock` の排他ロックによるプロセス間 read-modify-write 直列化、パース不能ファイルを絶対に上書きしない fail-loud 化（破損プロファイルを黙って default に戻していた profiles set 経路も修正）、変更のたびの自動世代バックアップ（`.bak.1`〜`.bak.3`）

## [0.4.0] - 2026-07-13

### Added

- Web view panes are now real native browsers (#155): the CDP-mirror proof of concept (headless Chrome + screenshot polling + click relay) has been replaced with wry's `build_as_child` integration — macOS WKWebView rendered as a true child view of the GPUI window. Clicking, scrolling, typing, and IME input are delivered natively by the OS with zero relay latency. Pages live independently of panes: the pane titlebar gains back / forward / reload buttons plus a minimize button that parks the page in a new web dock (status-bar 🌐 button) with its SPA state, login, and scroll position intact, and a close button that destroys it. Open pages persist in layout.json and reopen by URL after a restart. Everything is exposed 1:1 for AI/CLI via `Request::Web`, `tako web open|list|show|hide|close|nav|eval|eval-result|read`, and the MCP tool `tako_web` (9 actions; in-page interaction uses two-phase JS evaluation: `eval` → token → `eval_result`). Port-detection chips now open their preview in a web view pane next to the detected pane (falling back to the external browser). Replaces `tako_chrome_open` / `tako chrome`
  Web ビューペインが本物のネイティブブラウザになった（#155）: CDP ミラー方式の PoC（ヘッドレス Chrome + スクショポーリング + クリック中継）を wry の `build_as_child` 統合へ全面置換 — macOS の WKWebView を GPUI ウィンドウの真の子ビューとして表示する。クリック・スクロール・文字入力・IME は OS がネイティブ配送し、中継遅延ゼロ。ページはペインから独立して生存: タイトルバーに 戻る / 進む / 再読み込み ボタンと、ページを Web dock（ステータスバーの 🌐 ボタン）へ SPA 状態・ログイン・スクロール位置ごと退避する最小化ボタン、破棄する × を追加。開いたページは layout.json に永続化され、再起動後に URL で開き直される。全操作を `Request::Web` / `tako web open|list|show|hide|close|nav|eval|eval-result|read` / MCP ツール `tako_web`（9 action。ページ内操作は eval → token → eval_result の 2 段階 JS 評価）で AI / CLI に 1:1 公開。ポート検知チップの承諾は検知元ペインの隣に Web ビューペインを開くようになった（開けない場合は外部ブラウザへフォールバック）。`tako_chrome_open` / `tako chrome` は置き換えで廃止

- Editable code previews (#126): text/code files can now enter an in-place edit mode with UTF-8-safe typing, deletion, newlines, cursor movement, selection replacement, paste, dirty indication, and Cmd+S saving. Save refuses read-only files and detects external changes made after editing began instead of overwriting them. The same workflow is available through `tako edit start|status|apply|save|stop` and MCP (`tako_preview_edit`, `tako_preview_apply`, `tako_preview_save`); `tako list` exposes `preview.editing` / `preview.dirty`. Non-text and truncated previews remain read-only for safety
  コードプレビューのその場編集を追加（#126）: テキスト／コードファイルで編集モードへ切り替え、UTF-8 安全な文字入力・削除・改行・カーソル移動・選択置換・貼り付け・dirty 表示・⌘S 保存が可能になった。読み取り専用ファイルは拒否し、編集開始後に外部変更された場合も上書きせず競合を通知する。同じ一連の操作を `tako edit start|status|apply|save|stop` と MCP（`tako_preview_edit` / `tako_preview_apply` / `tako_preview_save`）へ公開し、`tako list` の `preview.editing` / `preview.dirty` で状態を取得できる。非テキストと末尾省略プレビューは安全のため読み取り専用のまま

- New `tako solo [-profile]` command for a 1:1 conversation mode without orchestration (#111): launches claude in a new tab with a solo-specific system prompt that **forbids orchestration** (`tako_orchestrator_spawn` / sub-agents / the Workflow tool) — the solo session does the work directly (read, edit, test, commit) instead of delegating to workers. Designed for economical use on plans like Claude Pro: default `effort=high` (below master's `max`), and recent activity is not preloaded at startup (checked via `git log` on demand). Shares the master `projects.yaml` and `build_master_claude_cmd`, so you can talk in terms of project names ("fix the README in demo") without `cd`. Uses the same profile-argument pattern as master (`-<name>` = profile, bare word = backward-compatible suffix); role and `TAKO_ORCHESTRATOR_ROLE` are `solo` / `solo:<suffix>`, distinct from master's `orchestrator-master`. Solo profiles live in `solo-profiles/`
  オーケストレーション無しの 1 対 1 対話モード `tako solo [-profile]` を新設（#111）: solo 専用の system prompt を付けて新タブで claude を起動する。プロンプトで**オーケストレーションを禁止**し（`tako_orchestrator_spawn` / sub-agent / Workflow ツール）、worker へ委任せず solo セッション自身がファイル編集・テスト・コミットを直接行う。Claude Pro 等のプランでの省トークン運用を想定し、既定 `effort=high`（master の `max` より低い）、「最近やってること」は起動時にロードせず必要時に `git log` で参照する。master と `projects.yaml` / `build_master_claude_cmd` を共有するため、`cd` せずプロジェクト名で（「demo の README 直して」）話せる。プロファイル引数は master と同一パターン（`-<名前>` = プロファイル、裸の語 = 後方互換サフィックス）。role と `TAKO_ORCHESTRATOR_ROLE` は `solo` / `solo:<suffix>`（master の `orchestrator-master` と区別）。solo プロファイルは `solo-profiles/` に置く

- Orchestrator workers can now run on codex and agy in addition to claude (#120): profiles gain `worker_agent` plus per-agent `worker_agents` settings (model, effort mapping, skip_permissions, extra args), and spawn / run / profiles expose the agent choice 1:1 via MCP (`agent` parameter) and CLI (`--agent`, `--worker-agent`, `--agent-*`). TUI handling (input-line, trust-dialog and busy detection) was extended to the union of all three agents based on captured real screens, busy/idle is screen-estimated for agents without OSC signals, and agy's always-on "(Thinking)" footer no longer reads as forever-busy
  オーケストレーションの worker が claude に加えて codex / agy で起動できるようになった（#120）: プロファイルに `worker_agent` とエージェント別 `worker_agents` 設定（model・effort 写像・skip_permissions・追加引数）を追加し、spawn / run / profiles の agent 指定を MCP（`agent` パラメータ）と CLI（`--agent` / `--worker-agent` / `--agent-*`）へ 1:1 公開。TUI 対応（入力欄・信頼ダイアログ・busy 検出）を実採取画面に基づく 3 種の和集合へ拡張し、OSC シグナルの無いエージェントは画面推定で busy/idle を判定する。agy の常時フッター「(Thinking)」が永遠 busy と誤判定される問題も修正済み

- The orchestrator master itself can now be codex (#127): profiles gain `master_agent` (claude / codex), honored by both `tako master` and `tako solo`. For codex, the system prompt is injected via developer instructions and the tako MCP server is wired in with temporary `-c mcp_servers.tako.*` config (TAKO_* env passthrough). A guard keeps a non-claude master's model / effort from propagating to claude workers, and agy as master is rejected with an explicit error. CLI `--master-agent` / MCP `master_agent` expose the setting 1:1
  オーケストレーションの master 自体を codex にできるようになった（#127）: プロファイルに `master_agent`（claude / codex）を追加し、`tako master` / `tako solo` の両方が対応。codex は developer instructions で system prompt を注入し、tako MCP サーバーは `-c mcp_servers.tako.*` の一時設定（TAKO_* 環境変数の引き継ぎ）で配線する。master≠claude のとき model / effort を claude worker へ継承しない波及ガード付き。agy の master 指定は明示エラー。CLI `--master-agent` / MCP `master_agent` で 1:1 公開

- PDF previews now support text selection and clipboard copy (#124): a PDFKit-extracted text layer feeds the same drag-selection / ⌘C / highlight path used by code and Markdown previews. PDFs without a text layer degrade gracefully to view-only
  PDF プレビューでテキスト選択とクリップボードコピーが可能になった（#124）: PDFKit で抽出したテキストレイヤを Code / Markdown プレビューと同じドラッグ選択・⌘C・ハイライト描画パスへ統合。テキストレイヤの無い PDF は従来どおり表示のみ

- Terminal text now supports cmd+click links (#146, #147, #153): URLs (including ones wrapped across lines) open in the default browser, file paths open a preview pane split to the right, and directories split-and-cd. Path resolution tries cwd-relative / ~-expanded / absolute candidates with an existence check and strips `:line:col` suffixes; while cmd is held, link text is underlined and highlighted. #153 fixed five root causes that made path links unreliable (link hover hitting the wrong pane, an empty pane on directory click, unknown cwd in TUI panes without OSC 7, detection skipped entirely when cwd was unknown, and an infinite loop in link scanning)
  ターミナル文字列の cmd+クリックリンクに対応（#146, #147, #153）: URL（行折り返しをまたぐものも連結検出）はデフォルトブラウザで開き、ファイルパスは右分割のプレビューで開き、ディレクトリは右分割 + cd する。パス解決は cwd 相対 / ~ 展開 / 絶対パスの 3 戦略 + 実在チェックで、`:行:列` サフィックスも除去。cmd 押下中はリンク文字列だけに下線 + ハイライトを表示する。#153 でパスリンクを不安定にしていた根本原因 5 件（ホバーの別ペイン誤ヒット・ディレクトリクリック時の空ペイン・OSC 7 なし TUI での cwd 不明・cwd 不明時の検出スキップ・リンク走査の無限ループ）を修正

- AI can pin project folders into the file tree (#134): `tako tree add/remove/list` + MCP `tako_tree_folder` (57 tools) manage per-tab pinned folders that persist in layout.json and merge with the cwd-derived workspace roots
  ファイルツリーへの AI からのフォルダ追加・削除（#134）: `tako tree add/remove/list` + MCP `tako_tree_folder`（計 57 ツール）で、タブ単位のピン留めフォルダを管理する。layout.json に永続化され、cwd 由来のワークスペースルートと合流表示される

- Common agent rules can be synced from one source of truth (#136): `tako agents sync-rules` / `tako agents status` + MCP `tako_agents_sync_rules` (58 tools) embed a source file into each agent's global instruction file (claude / codex / agy) between marker blocks — everything outside the block stays untouched, with automatic backups. Also available as a new `tako setup` item, with sync status shown in `tako setup --check`
  エージェント共通ルールの同期機能（#136）: `tako agents sync-rules` / `tako agents status` + MCP `tako_agents_sync_rules`（計 58 ツール）が、正本ファイルの内容を各エージェント（claude / codex / agy）のグローバル指示ファイルへマーカーブロックで埋め込む。ブロック外は不変・バックアップ付き。`tako setup` の新項目としても提供され、同期状態は `tako setup --check` に表示される

- Full Disk Access guidance (#118): new `tako fda status/open` + MCP `tako_fda` (53 tools) detect whether tako has Full Disk Access and open the exact System Settings pane to grant it; `tako setup --check` includes the same check. This targets macOS TCC folder-access dialogs reappearing on every access
  フルディスクアクセス（FDA）ガイド機能（#118）: `tako fda status/open` + MCP `tako_fda`（計 53 ツール）が FDA の付与状態を検出し、付与用のシステム設定画面を直接開く。`tako setup --check` にも同じチェックを追加。macOS TCC のフォルダアクセス許可ダイアログが毎回出る問題への対策

### Changed

- codex / agy workers now skip approval prompts by default (#132): spawned codex / agy workers run with permissions bypassed unless opted out, and a codex master launches with `--dangerously-bypass-approvals-and-sandbox` — verified against a real codex to be the only mode that also bypasses MCP tool approvals (`-a never` does not). `tako orchestrator profiles set` gains `--worker-model-policy`, and `scripts/clean-target.sh` was added to prune build artifacts
  codex / agy worker の承認を既定でスキップ（#132）: spawn される codex / agy worker は opt-out しない限り承認バイパスで起動し、codex master は `--dangerously-bypass-approvals-and-sandbox` を使う（実 codex での検証により、MCP ツール承認までバイパスするのはこのモードのみ。`-a never` では不十分）。`tako orchestrator profiles set` に `--worker-model-policy` を追加し、target 掃除の `scripts/clean-target.sh` を新設

- Master / solo system prompts now pin project folders proactively (#141): folders for projects mentioned in conversation are added to the file tree before the user has to ask
  master / solo のデフォルト system prompt がプロジェクトフォルダを積極的にピン留めするようになった（#141）: 会話に上がったプロジェクト・関連フォルダを、ユーザーに聞かれる前にファイルツリーへ追加する行動規範を強化

- `tako setup` now walks through Full Disk Access (#143): the setup flow names missing FDA as the cause of repeated TCC folder dialogs, offers to open System Settings on the spot, notes that an app restart is required after granting, and shows a checkmark when already granted. Delivered to existing users as setup changelog rev 6
  `tako setup` の FDA 案内を強化（#143）: TCC ダイアログ頻発の原因が FDA 未付与であることを明示し、その場でシステム設定を開く対話・付与後のアプリ再起動案内・付与済みなら「✓ 済み」表示を追加。setup changelog rev 6 として既存ユーザーにも配信

### Fixed

- Claude Code conversations now resume after a full PC restart (#139): tako periodically associates running Claude session IDs with their tmux-backed panes and stores them in `layout.json`. On restore, an existing backend session is still reattached unchanged; only when that backend disappeared (as happens on reboot) does tako validate the local transcript and run `claude --resume <session-id>` in the recreated pane. Explicitly exited or unidentifiable sessions are not guessed, and the behavior remains controlled by the existing `tako persist` / `tako_persist` setting
  PC 再起動後も Claude Code の会話を復旧（#139）: 実行中 Claude の session ID を tmux backend ペインへ定期的に対応付け、`layout.json` に保存する。復元時、backend session が生存していれば従来どおりそのまま再 attach し、PC 再起動のように backend 自体が消失した場合だけローカル transcript を検証して、再作成したペインで `claude --resume <session-id>` を実行する。明示終了済み・特定不能なセッションを推測で戻すことはなく、既存の `tako persist` / `tako_persist` 設定で制御される

- PDF drag selection is visible again (#152): the PDF text canvas is now pinned to the page image's top-left instead of inheriting a static position below the image, and selection rectangles are composited in a dedicated topmost GPUI layer. Syntax highlighting now preserves line endings required by syntect's parser and uses one path/filename/shebang resolver for both read and edit modes across the bundled standard language set (including C++ and Python), with JavaScript fallback for TypeScript files
  PDF のドラッグ選択ハイライトを再修正（#152）: PDF テキスト canvas を画像直後の static position ではなくページ画像左上へ固定し、選択矩形を GPUI の専用最前面 layer で合成する。シンタックスハイライトは syntect パーサが必要とする行末改行を保持し、読み取り／編集の両モードを同一のパス・特殊ファイル名・shebang 解決器へ統一した。C++／Python を含む同梱標準言語セット全体を対象とし、TypeScript は JavaScript 文法へ安全にフォールバックする

- Preview selection now follows the actual GPUI-shaped text coordinates instead of terminal-cell estimates (#145), including Markdown font sizes, mixed Japanese/ASCII text, tabs, and vertical scrolling. PDF selection uses PDFKit line/character rectangles transformed onto the rendered page, and editable previews keep syntax colors while composing selection/caret highlights. Preview swaps invalidate stale coordinate caches, and self-tests synchronize on real CLI/paint completion instead of fixed delays
  プレビュー選択の座標ずれを修正（#145）: ターミナル固定セル換算をやめ、GPUI が実際に shaping した座標から Markdown の文字サイズ・日本語／半角混在・タブ・縦スクロール後の byte 位置を逆算する。PDF は PDFKit の行／文字矩形を表示ページへ変換して選択し、編集モードでも構文色と選択／キャレットを合成する。ファイル差し替え時は旧座標キャッシュを破棄し、セルフテストは固定待ちではなく実 CLI／paint 完了へ同期する

- Starting a second tako instance no longer destroys panes (#113): the root cause was a three-step chain — the late instance's restore (`new-session -A -D`) hijacked the primary's tmux clients, the resulting cascade of Exited states overwrote layout.json mid-shutdown, and the next startup's orphan cleanup killed live worker sessions that had leaked out of the protected set. A multi-instance guard now starts late instances in a secondary mode (no restore, no layout.json writes, no tmux backend, no socket takeover; `TAKO_FORCE_PRIMARY=1` overrides), startup orphan cleanup skips sessions active within the last hour, and pane-exit / quit handling is idempotent against double-fired exit events. A UI-stall watchdog and dispatch timing now log to perf.log (256KB rotation, event names only), and tmux window capture for hover previews moved off the UI thread
  2 個目の tako 起動でペインが消える問題を根治（#113）: 根因は三段連鎖 — 後発インスタンスの復元（`new-session -A -D`）がプライマリの tmux クライアントを強奪 → Exited 連鎖の途中状態が layout.json を上書き → 次回起動の orphan cleanup が保護から漏れた実行中セッションを kill。多重インスタンスガード（後発はセカンダリモードで起動: 復元しない・layout.json に書かない・tmux バックエンドに乗らない・ソケットを乗っ取らない。`TAKO_FORCE_PRIMARY=1` で上書き可）+ 起動時 cleanup の 1 時間アクティビティ猶予 + 終了イベント二重発火の冪等化で対策。UI ストールウォッチドッグと dispatch 処理時間の perf.log 記録（256KB ローテート・種別名のみ）、ホバープレビュー用 tmux window キャプチャの background 化も同梱

- `tako remote start` no longer fails when a stale daemon holds the port (#129): the port is probed before spawning, a stale tako remote daemon is reclaimed automatically (SIGTERM → poll → SIGKILL), and an unrelated process occupying the port produces a clear error including its PID. `daemon_stop` now polls for actual process exit (up to 5s, then SIGKILL) instead of a fixed 500ms wait, and recovers stale daemons even when the PID file is gone
  stale デーモンのポート占有で `tako remote start` が失敗する問題を修正（#129）: 起動前にポート占有を検知し、stale な tako remote デーモンなら SIGTERM → ポーリング → SIGKILL で自動回収して再起動する。無関係プロセスが占有中なら PID 入りのエラーで案内。`daemon_stop` も固定 500ms 待ちをやめ実際のプロセス終了をポーリングし（最大 5 秒、超過時 SIGKILL）、PID ファイル消失時もポート占有者から stale デーモンを回収する

- Cmd-Q now always quits (#103): Quit was registered only on the root div's `on_action`, making it focus-path dependent — with no focused element (blur, e.g. caused by accessibility tools), both the keybinding and the menu item silently did nothing, and only quitting from the Dock worked. Quit is now a global `cx.on_action` registration, and shutdown work (layout save, discovery cleanup) moved to `cx.on_app_quit` so it also runs on Dock- or OS-initiated quits. The all-panes-exited path keeps its layout delete / keep semantics
  Cmd-Q で終了しないことがある問題を根治（#103）: Quit がルート div の `on_action` のみに登録されフォーカスパス依存だったため、フォーカス無し（blur。a11y ツール等で発生）ではキーバインド・メニューの両経路が無音で不発になり、Dock からの終了だけが効いていた。Quit を `cx.on_action` のグローバル登録へ一本化し、終了処理（layout 保存・discovery cleanup）を `cx.on_app_quit` へ移設（Dock・OS 起因の終了でも保存が走る）。全ペイン終了経路の layout 削除 / 保持の分岐は不変

## [0.3.2] - 2026-07-07

### Fixed

- Multi-master orchestrator spawn no longer sends workers to the wrong tab (#109): when multiple masters run in parallel (`tako master -fable`, `tako master -aram`, etc.) and the caller's `TAKO_PANE_ID` is stale, the spawn fallback now uses `TAKO_ORCHESTRATOR_ROLE` to identify the correct master instead of blindly picking the first one found. The role is propagated through the MCP session (`caller_role` field) from the stdio bridge / HTTP transport to the dispatch layer
  複数 master 並行時に `tako_orchestrator_spawn` の worker が意図しないタブに出る問題を修正（#109）: 呼び出し元の `TAKO_PANE_ID` が stale な場合のフォールバックで、`TAKO_ORCHESTRATOR_ROLE` 環境変数を使って正しい master を特定する。role は MCP セッション（`caller_role` フィールド）を通じて stdio ブリッジ / HTTP トランスポートから dispatch 層まで伝搬する

## [0.3.1] - 2026-07-07

### Security

- `tako remote` is now secure-by-default (#104): the remote server hosts only over an encrypted cloudflared tunnel and **refuses to start** if a tunnel cannot be established (the old silent plaintext-LAN fallback is gone). Plain HTTP LAN mode is available only via the explicit, opt-in `--no-tunnel` replacement `--insecure` (off by default). Token comparison is now constant-time (HTTP Bearer + WebSocket), token/QR state files are written `0o600`, and `remote status` masks the token by default — both the standalone `token` field and the `token=` query embedded in `connect_url` / `fallback_url` — revealing raw values only with `--show-token` (CLI) / `show_token=true` (MCP). The public relay worker gained per-source-IP rate limiting (register 60/min, resolve 240/min). README / docs document that remote access is a legitimate remote-control tool granting arbitrary command execution and is not end-to-end encrypted
  `tako remote` を secure-by-default 化（#104）: リモートサーバーは暗号化された cloudflared トンネル経由でのみホストし、トンネルを張れない場合は**起動を拒否**する（従来の無音の平文 LAN フォールバックを廃止）。平文 HTTP の LAN モードは明示 opt-in の `--insecure`（既定 off）でのみ有効。トークン比較を定数時間化（HTTP Bearer + WebSocket）、token / QR の state ファイルを `0o600` で書き出し、`remote status` は既定でトークンをマスクする — 単体の `token` フィールドに加え `connect_url` / `fallback_url` に埋め込まれた `token=` クエリも伏せ、生値は `--show-token`（CLI）/ `show_token=true`（MCP）でのみ表示。公共リレー worker に送信元 IP 単位のレートリミット（register 60/分・resolve 240/分）を追加。README / docs に、リモートアクセスが任意コマンド実行を許す正規の遠隔操作ツールであり E2E 暗号化ではない旨を明記

### Added

- New CLAUDE.md section template `06-completion-verification` distributed by `tako setup` (#100): defines a completion-verification quality gate — build / lint / tests green, exercise the change end-to-end ("a passing build is not evidence it works"), probe edge cases, re-read the full diff — and an evidence-based report format with an explicit "not verified" section. Registered as setup changelog rev 5 (guided), so existing users are offered the addition interactively on their next `tako setup` without overwriting customizations
  `tako setup` が配布する CLAUDE.md セクションテンプレートに `06-completion-verification`（完了検証）を新設（#100）: 完了報告前の品質ゲート（ビルド・リント・テスト緑 / 変更を実際に動かして観察 =「ビルドが通った ≠ 動く」/ エッジケース確認 / diff の読み直し）と、証拠つき + 「未検証」明示の報告様式を定義。setup changelog の rev 5（guided）として登録し、既存ユーザーは次回の `tako setup` で対話的に追記を提案される（カスタマイズは上書きしない）

### Changed

- Orchestration quality pipeline standardized in the default master system prompt (#100): new `task-intake` block (enumerate the requests in each user message, assign one worker per deliverable with a closed list of merge exceptions, decide parallel vs sequential, post the plan and spawn in the same turn), new `worker-prompt-template` block (a mandatory fill-in template — Task / Background / Scope / Constraints / acceptance criteria / verification steps / git flow / evidence-based report format — with root-cause-first and requirement-bound rules), and new `acceptance` block (inspect worker reports against evidence and diff spot-checks before reporting to the user; send back with a concrete defect list, rethink after 2 failed rounds). Existing block names are unchanged, so `prompt_blocks` customizations keep working. Monitoring / lifecycle blocks absorbed field lessons (idle notifications can misfire — confirm via `tako_read_pane`; never respawn a merely-thinking worker; commit per milestone on long tasks)
  master 用デフォルトシステムプロンプトにオーケストレーション品質パイプラインを標準化（#100）: `task-intake` ブロック新設（依頼を列挙し 1 worker = 1 成果物で割り当て・統合の例外は閉じたリスト・並列/直列判定・分担計画の提示と同ターン spawn）、`worker-prompt-template` ブロック新設（Task / Background / Scope / Constraints / 受け入れ条件 / 検証手順 / Git / 証拠つき報告様式を必須とする穴埋め式の型。根因先行・要件密着タスクの転記ルール込み）、`acceptance` ブロック新設（worker の完了報告を証拠と diff スポットチェックで検査してからユーザーに報告。差し戻しは具体的な欠陥リストで行い、2 回失敗したら方針を再考）。既存ブロック名は不変のため `prompt_blocks` によるカスタマイズはそのまま動く。monitoring / lifecycle ブロックにも運用知見を反映（idle 通知の空振りは `tako_read_pane` で確認・thinking 中の worker を respawn しない・長尺タスクはマイルストーンごとにコミット）

### Fixed

- Enter no longer goes missing in claude TUI worker panes (#95): three delivery paths are hardened. (1) A human Enter pressed on a claude TUI pane is now verified — tako snapshots the input line (`❯ …`) before writing `\r`, and if the same text is still sitting there afterwards (claude occasionally drops Enter while busy), the Enter is automatically re-sent (up to 4 times). (2) `tako_send_input` with empty `text` + `newline: true` becomes a proper "Enter only" delivery: it no longer waits out a pointless 10-second reflection timeout, and its verification actually checks that the input line emptied (previously the empty-prompt check always passed, so it never retried — one silently dropped CR meant permanent stuck text). (3) LF characters written directly to a pane (`text: "\n"`, etc.) are normalized to CR — claude TUI interprets LF as "insert newline", never "submit", so raw-LF sends could clear-looking-but-unsent input. The same Enter-only delivery (send → verify emptied → resend) also applies to the tmux fallback path
  claude TUI の worker ペインで Enter が空振りする問題を修正（#95）: 送達 3 経路を強化。(1) claude TUI ペインへの人間の Enter を検証つきに — `\r` 書き込み前に入力欄（`❯ …`）の内容を控え、書き込み後も同じテキストが残っていれば（busy 中の claude は Enter を取りこぼすことがある）Enter を自動再送する（最大 4 回）。(2) `tako_send_input` の空 `text` + `newline: true` を正式な「Enter 単独送達」に — 無意味な 10 秒の反映待ちタイムアウトを廃止し、検証も「入力欄が空へ戻ったか」を実際に確認する（従来は空プロンプトの検証が常に成功扱いで再送ゼロのため、CR 1 発の取りこぼし = 恒久残留だった）。(3) ペインへ直接書く経路の LF を CR へ正規化 — claude TUI は LF を「改行挿入」と解釈し決して送信しないため、生 LF 送信は「消えたように見えて未送信」になっていた。tmux フォールバック経路にも同じ Enter 単独送達（送信 → 空検証 → 再送）を適用

## [0.3.0] - 2026-07-06

### Added

- `tako setup` now starts with a dependency check stage (#88): claude (required) and tmux / cloudflared / git (optional) are detected with a one-line purpose note each, and missing tools can be installed on the spot via Homebrew (per-tool y/N prompt). cloudflared joined the list following #89 (tunnel-less silent LAN fallback). The same list is shown by `tako setup --check`, and the docs dependency table is kept in sync
  `tako setup` の冒頭に依存ツールチェック段階を追加（#88）: claude（必須）と tmux / cloudflared / git（任意）を用途の一言説明付きで検出し、不足分は Homebrew でその場インストールできる（ツールごとに y/N 確認）。cloudflared は #89（トンネル不成立時の無音 LAN フォールバック）を受けて対象化。同じ一覧は `tako setup --check` にも表示され、docs の依存表も同期
- `tako setup` now tracks setup-related changes across updates (#94): the binary embeds a machine-readable setup changelog (`resources/setup/changes.yaml`, revision-numbered), and the revision applied at the last setup is recorded in `config.yaml` (`setup.applied_revision`). Re-running `tako setup` after an update lists what changed since, writes a `pending-changes.md` brief into the setup directory, and the setup agent follows up in conversation — `auto` entries (new checks, template updates) are applied by the re-run itself and only announced, while `guided` entries (anything touching user-owned files such as a custom `master-system.md`) are confirmed interactively and never overwrite customizations silently. Inspect anytime with `tako setup --changes [--json]` (CLI) or `tako_setup_changes` (MCP, 52 tools total); `tako setup --check` also reports the follow-up status
  `tako setup` にアップデート追従機能を追加（#94）: バイナリに機械可読の setup changelog（`resources/setup/changes.yaml`、リビジョン番号付き）を同梱し、最後に setup したときの適用リビジョンを `config.yaml`（`setup.applied_revision`）に記録。アップデート後に `tako setup` を再実行すると前回以降の変更が一覧表示され、setup ディレクトリに書き出される `pending-changes.md` をもとに setup エージェントが対話で追従する。`auto` の変更（チェック項目追加・テンプレート更新等）は再実行自体が適用を兼ねて通知のみ、`guided` の変更（カスタム `master-system.md` などユーザー所有ファイルに関わるもの）は対話で確認し、カスタマイズを黙って上書きしない。`tako setup --changes [--json]`（CLI）/ `tako_setup_changes`（MCP、計 52 ツール）でいつでも確認でき、`tako setup --check` にも追従状況が表示される

### Security

- `FileOp::Trash` (macOS) now passes the path to `osascript` as an argument instead of concatenating it into the AppleScript source (#80): the Finder delete script uses `on run argv` and reads the path from `item 1 of argv`, so filenames containing `"`, `\`, or newlines can no longer break out of the string literal and inject AppleScript. This removes the reliance on escape correctness (and the prior control-character reject guard is no longer needed). A deterministic test proves an injection payload passed via argv is treated as data (no side effect), and an `#[ignore]` e2e trashes a file whose name contains quotes/backslash/newline
  `FileOp::Trash`（macOS）がパスを AppleScript ソースへ文字列連結せず `osascript` の引数として渡すよう変更（#80）: Finder の削除スクリプトを `on run argv` 化し `item 1 of argv` からパスを読むことで、`"` `\` 改行を含むファイル名が文字列リテラルを抜け出して AppleScript を注入する余地を構造的に排除。エスケープの正しさへの依存（および従来の制御文字拒否ガード）が不要になった。argv 経由のインジェクション payload がデータとして扱われる（副作用なし）ことを決定的テストで実証し、引用符・バックスラッシュ・改行を含むファイル名の削除を `#[ignore]` の e2e で用意
- Relay registration is now protected by a per-machine secret (#78): `tako remote start` auto-generates `<data_dir>/relay_secret` (hex 64, mode 0600) and sends it to `/api/register`; the relay worker stores only its SHA-256 hash and rejects overwrites with a mismatched secret (first-write-wins — legacy secret-less registration is still accepted for unclaimed machine IDs, so old clients keep working). The default relay is now documented as a best-effort public instance that stores only machineId → tunnel URL (no terminal content, no tokens) and can be replaced via `TAKO_RELAY_URL`; self-hosting steps live in `web/tako-remote-worker/README.md`, and the worker gained an offline test suite (`npm test`). Deployed to the production relay on 2026-07-06 (version `5acac8f5`); overwrite protection was verified live against the running instance (mismatched-secret and secret-less overwrites both return 403, resolve keeps the original tunnel URL)
  リレー登録を端末ごとのシークレットで保護（#78）: `tako remote start` が `<data_dir>/relay_secret`（hex 64・0600）を自動生成して `/api/register` に送り、リレー worker は SHA-256 ハッシュのみ保存して secret 不一致の上書きを 403 で拒否（first-write-wins。secret 無しの旧クライアントは未保護 ID に限り従来どおり登録可能で互換維持）。デフォルトリレーは「machineId → tunnel URL のみを保存するベストエフォート公共インスタンス（画面内容・トークンは通らない）」として文書化し、`TAKO_RELAY_URL` で自前リレーへ差し替え可能に。セルフホスト手順は `web/tako-remote-worker/README.md`、worker にオフラインテスト（`npm test`）を追加。2026-07-06 に本番リレーへデプロイ済み（version `5acac8f5`）。稼働中インスタンスに対して上書き保護を実地検証（別 secret・secret 無しの上書きはいずれも 403、resolve は元の tunnel URL を維持）

### Changed

- Remote connection entry point unified to the fixed Pages URL (#91): with the tunnel up and relay registration succeeded, the connect link / QR now always points to `https://tako-remote.pages.dev/#/connect?machine=<id>&...` — the PWA is served by Cloudflare Pages and resolves the machine's current tunnel URL via the KV relay, so bookmarks survive tunnel restarts and the random trycloudflare URL is never shown. The tunnel-direct URL is still printed as a spare link (relay-outage backup), and the daemon-embedded PWA remains the LAN-only fallback. `tako remote status` reconstructs the same link (tunnel state is persisted to `<state_dir>/tako-remote.tunnel`), `tako remote start` now warns visibly when the tunnel could not be established and the URL is LAN-only (#89 visibility), the PWA skips the pointless self-health probe when served from pages.dev and records the daemon version from `/api/health` for compatibility warnings, and `scripts/release.sh --publish` deploys the PWA to Pages via the new `scripts/deploy-pages.sh`
  リモート接続の入口を Pages 固定 URL に一本化（#91）: トンネル確立 + リレー登録成功時の接続リンク / QR は常に `https://tako-remote.pages.dev/#/connect?machine=<id>&...` を指すようになった。PWA は Cloudflare Pages が配信し、KV リレーで各マシンの現在のトンネル URL を解決するため、トンネルが再起動してもブックマークは不変で、trycloudflare のランダム URL はユーザーに見えない。トンネル直 URL は予備リンクとして併記（リレー障害時の受け皿）し、LAN-only 用にデーモン内蔵 PWA も維持。`tako remote status` も同じリンクを再構成（トンネル状態を `<state_dir>/tako-remote.tunnel` に永続化）、トンネルが張れず LAN 限定 URL になった場合は `tako remote start` が明示的に警告（#89 の可視化）、pages.dev 配信時の PWA は自分への無駄な health 試行をスキップし、`/api/health` のデーモンバージョンを互換警告用に記録。`scripts/release.sh --publish` は新設の `scripts/deploy-pages.sh` で PWA を Pages へデプロイする
- License declarations unified to GPL-3.0-or-later across all manifests (#75): added the `license` field to the three `poc/` crates and the three `docs/` / `web/` package.json files. The license itself is unchanged — the repository has declared GPL-3.0-or-later throughout (LICENSE / Cargo.toml / README); this completes manifest-level consistency for the public release
  ライセンス宣言を全マニフェストで GPL-3.0-or-later に統一（#75）: `poc/` クレート 3 つと `docs/` / `web/` の package.json 3 つに license フィールドを追加。ライセンス自体は変更なし（LICENSE / Cargo.toml / README は従来から GPL-3.0-or-later を宣言）。公開に向けたマニフェスト単位の一貫性を仕上げた
- Orchestrator completion-wait polling unified into tako-control (#83): the polling state machine duplicated across MCP `tako_orchestrator_run` and CLI `tako orchestrator run` / `watch` (~300 lines) is now a single engine (`orchestrator::wait`). The tmux-liveness guard against false "gone" during tako restarts — previously CLI-only — now also applies to the MCP path, so `tako_orchestrator_run` no longer misreports `error` while tako restarts
  オーケストレーターの完了待ちポーリングを tako-control へ一本化（#83）: MCP `tako_orchestrator_run` と CLI `tako orchestrator run` / `watch` に重複していたポーリング状態機械（約 300 行）を単一エンジン（`orchestrator::wait`）に統合。CLI のみにあった tmux 生存確認による gone 誤検知防止（tako 再起動中の対策）が MCP 経路にも効くようになり、再起動中の `tako_orchestrator_run` が `error` を誤報告しなくなった

### Fixed

- `tako_orchestrator_run` / `tako orchestrator run` no longer return an empty `output` (#82): the result-read step referenced a nonexistent `content` field of the Read response (actual field: `text`), so the worker's final output was always empty — with `auto_close` defaulting to true, the pane was closed before the master could re-read it. A regression test now asserts the output round-trip
  `tako_orchestrator_run` / `tako orchestrator run` の `output` が常に空になる問題を修正（#82): 出力取得ステップが Read 応答に存在しない `content` フィールド（実際は `text`）を参照していたため worker の成果が常に空だった。`auto_close` 既定 true のため master が読み直す前にペインも閉じられていた。出力の往復を検証する回帰テストを追加

## [0.2.8] - 2026-07-05

### Changed

- Remote UI redesign v3 — PC-safe read-only WebSocket + continuous-scroll reader view (#63): WebSocket auto-resize of cols/rows is completely removed — `/ws?pane=<id>` is now read-only and never affects the PC pane size. Protocol changed to push `init` (history + current screen with ANSI, cursor) on connect, then `update` diffs every 250ms. xterm.js is replaced with a self-contained reader view: one continuous scroll with bottom-following (scroll up to browse history, scroll to bottom to re-follow), line-wrapping for mobile readability, and a custom ANSI SGR parser (`web/tako-remote/src/ansi.js`) — zero added dependencies. Font size A−/A+, pane switching via swipe + header ‹ ›
  リモート UI を再設計 v3 — PC 非破壊の読み取り専用 WebSocket + 連続スクロールリーダービュー（#63）: WS の cols/rows 自動リサイズを全廃し、`/ws?pane=<id>` は読み取り専用で PC のペインサイズに一切影響しなくなった。プロトコルを接続時 `init`（履歴 + 現画面、ANSI 付き + カーソル）→ 250ms 差分 `update` のプッシュ方式に刷新。xterm.js を廃止し、折り返しリーダービュー（1 本の連続スクロール、下端追従・上スクロールで過去閲覧・下端復帰で追従再開）+ 自前 ANSI SGR パーサ（`web/tako-remote/src/ansi.js`）で再実装。依存追加ゼロ。フォント A−/A+、スワイプ + ヘッダー ‹ › でペイン切替

### Fixed

- Half-width characters no longer vanish sporadically in mixed Japanese/ASCII lines (#64): grouped half-width runs (#39) rendered text inside a grid-width div, and GPUI treated that width as a wrap width — a hairline (f32 ULP) overshoot of the shaped width made GPUI wrap the tail word/character onto an invisible second line inside the `overflow_hidden` row (e.g. "ターミナルUI" → "ターミナルU", "Fable 5 + max" → "Fable 5 + "). Rows now set `whitespace_nowrap` (structurally disables wrapping), and glyphs whose advance differs from the cell width (fallback-font symbols like ⏺ ⎿) are excluded from grouping into their own cell-width div so misalignment cannot accumulate. The #39 hang fix (element count reduction) is preserved: ASCII runs stay grouped
  日本語混在行で半角文字が確率的に消える問題を根治（#64）: 半角グループ化描画（#39）はグリッド幅の div 内にテキストを置くが、GPUI はその幅を折り返し幅として扱うため、シェイプ幅がヘアライン（f32 ULP）でも超えると末尾の単語/文字が折り返されて行 div の `overflow_hidden` 外へ消えていた（例:「ターミナルUI」→「ターミナルU」、「Fable 5 + max」→「Fable 5 + 」）。行 div に `whitespace_nowrap` を指定して折り返しを構造的に禁止し、advance がセル幅と一致しないグリフ（⏺ ⎿ 等のフォールバックフォント記号）はグループから除外してセル幅固定の個別 div に隔離、ずれの累積も遮断。#39 のハング解消効果（描画要素数削減）は維持（ASCII 連続はグループ化のまま）
- `migrate_legacy_default_profile` no longer strips user-configured model on every master launch (#67): when the backup file (`default.yaml.backup-1m`) already exists, the migration is considered done and skipped. Previously, each `tako master` / `tako setup` / spawn run re-triggered the migration, removing any model that had been set via `tako orchestrator profiles set --model`
  `migrate_legacy_default_profile` が master 起動のたびにユーザー設定の model を消す問題を修正（#67）: backup ファイル（`default.yaml.backup-1m`）が存在する場合はマイグレーション済みと判断してスキップするようにした。従来は `tako master` / `tako setup` / spawn の実行ごとにマイグレーションが再発火し、`tako orchestrator profiles set --model` で設定した model が消えていた
- Update checker no longer misreports GitHub API rate limits as "no update available" (#59): switched from GitHub API to web redirect-based version detection (not subject to API rate limits), introduced `CheckError` type to distinguish errors from genuine "no update" state, added silent retry on failure (waits until rate-limit reset for 403, 1 hour for others), and surfaced error details in CLI/MCP JSON and status bar
  更新チェッカーが GitHub API レート制限を「更新なし」と誤報告する問題を修正（#59）: GitHub API から Web リダイレクト方式（API レート制限の対象外）に移行し、`CheckError` 型でエラーと「更新なし」を区別、自動チェック失敗時の静かなリトライ（レート制限は reset 時刻まで、他は 1 時間後）を追加、CLI/MCP の JSON とステータスバーにエラー詳細を表示

## [0.2.7] - 2026-07-03

### Fixed

- Release build now includes PWA rebuild (#60): `build-app.sh` runs `npm ci && npm run build` for `web/tako-remote` before `cargo build`, ensuring `rust_embed` always bundles the latest PWA dist. `release.sh` verifies that the bundled JS contains source-derived markers (e.g. history UI strings) to prevent stale dist from shipping again. Without npm, a warning is shown if an existing dist is available; otherwise the build errors
  リリースビルドに PWA ビルド工程を組み込み（#60）: `build-app.sh` が `cargo build` の前に `web/tako-remote` の `npm ci && npm run build` を実行し、`rust_embed` に常に最新の PWA dist を埋め込む。`release.sh` は同梱 JS にソース由来マーカー（履歴 UI 文字列等）が含まれることを機械検証し、stale な dist の再発を防止。npm 不在時は既存 dist があれば警告スキップ、なければエラー終了

## [0.2.6] - 2026-07-03

### Added

- Remote PWA overhaul — two-layer architecture (#42, #26): history layer (`GET /api/panes/:id/scrollback` + client-side rendering with free scroll and text selection) + live screen layer (REST polling → WebSocket push with viewport-linked auto-resize on connect and reset on disconnect). `<input>` → `<textarea>` for Shift+Enter multiline input (#26). Quick keys via tmux send-keys raw sequences + ctrl toggle mode. CLI `tako remote scrollback` / MCP `tako_remote_scrollback` (51 MCP tools total)
  リモート PWA 二層構成刷新（#42, #26）: 履歴レイヤー（`GET /api/panes/:id/scrollback` + クライアント側描画、自由スクロール・テキスト選択対応）+ ライブ画面レイヤー（REST ポーリング → WebSocket プッシュ、接続時ビューポート連動自動リサイズ + 切断時リセット）。`<input>` → `<textarea>` で Shift+Enter 改行対応（#26）。Quick keys を tmux send-keys 生キーシーケンス経由に変更 + ctrl トグルモード。CLI `tako remote scrollback` / MCP `tako_remote_scrollback`（MCP 計 51 ツール）
- Homebrew update failure recovery (#50): detects "broken-brew" state (app exists but cask ledger is missing after a failed `brew upgrade`), offers zip-based fallback update via status bar button, and adds `tako update repair` (re-register cask ledger) / `tako update apply-zip` (force update via GitHub Releases zip). README troubleshooting section updated
  brew 更新失敗の復旧導線（#50）: `brew upgrade` 失敗後の「.app 実体あり・cask 台帳なし」詰み状態を自動検知し、ステータスバーに zip 更新ボタンを表示。`tako update repair`（cask 台帳の再締結）/ `tako update apply-zip`（zip 強制更新）を追加。README トラブルシューティングに復旧手順を追記

### Changed

- Orchestrator master system prompt now includes 6 quality-ops principles derived from cross-PR review (#53): root-cause-first instructions, same-file serialization, DoD for untested areas, integration review layer, master-owned Closes decisions, and completion definition
  オーケストレーター master 共通 system prompt に品質運用原則 6 点を組み込み（#53）: 根因先行の指示、同一ファイル直列化、機械検証なし領域の DoD、統合レビュー層、master が持つ Closes 判断、完遂の定義

### Fixed

- TCC permission prompts ("access data from other apps") no longer reset across rebuilds and in-app updates (#54): the code signature's designated requirement is now pinned to the bundle identifier instead of the signing certificate. Previously the requirement changed whenever the signing identity changed (multiple Apple Development certificates in the keychain, certificate expiry, or ad-hoc fallback), making macOS treat each build as a different app and invalidate previously granted permissions. Note: updating from ≤0.2.5 requires re-granting once due to the requirement migration; granting Full Disk Access to tako.app suppresses the per-target dialogs entirely (see README troubleshooting)
  TCC の許可（「ほかのアプリからのデータへのアクセス」等）が再ビルド・アプリ内更新でリセットされる問題を修正（#54）: コード署名の designated requirement を署名証明書依存から bundle identifier 固定に変更。従来は署名 identity が変わるたび（キーチェーンに複数の Apple Development 証明書・証明書失効・ad-hoc への劣化）に requirement が変わり、macOS が別アプリと判定して付与済み許可を無効化していた。注意: 0.2.5 以前からの更新時は requirement 移行のため 1 回だけ再許可が必要。tako.app にフルディスクアクセスを付与すると対象別ダイアログ自体が出なくなる（README トラブルシューティング参照）
- Remote PWA: soft keyboard Enter now works on mobile (#41): removed empty-input guard in `send()` that blocked bare Enter (needed for Claude Code permission prompts), added `<form>` submit event capture as reliable mobile Enter path, and `enterkeyhint="send"` for soft keyboard send button
  リモート PWA: スマホのソフトキーボードから Enter が送信可能に（#41）: 空入力をブロックしていた `send()` のガードを除去（Claude Code の許可プロンプトに空 Enter で応答するケースに対応）、`<form>` submit イベントで確実にモバイル Enter を捕捉、`enterkeyhint="send"` でソフトキーボードに送信ボタンを表示
- Remote PWA: empty Enter regression from #45 restored + WebSocket zombie reconnection prevented (#51, #52): re-enabled empty-input send button that was disabled during the textarea migration; WS event handlers are now nullified before `close()` to prevent stale pane connections from triggering reconnection timers
  リモート PWA: #45 で落ちた空 Enter 送信経路を復旧 + WS ゾンビ再接続を防止（#51, #52）: textarea 移行で無効化されていた空入力送信ボタンを復活、`close()` 前に WS イベントハンドラを null 化して旧ペインの非同期 onclose による再接続タイマー設定を根治
- Full-width character click now resolves to the correct cell (#37): click coordinate calculation used font shaping advance instead of grid-based `cell_width × column` — unified to the grid coordinate system
  全角文字行のクリックが正しいセルに解決するように修正（#37）: クリック座標計算がフォント shaping の advance 値を使用しておりグリッド座標系（`cell_width × 列番号`）と不一致だった問題を統一
- `orchestrator watch` no longer false-fires WORKER_IDLE when session_id is omitted (#44): pane → backend session → pid ancestor traversal now auto-resolves the session, using `claude agents --json` status as primary signal (screen pattern matching is fallback only)
  `orchestrator watch` が session_id 未指定時に WORKER_IDLE を空振りする問題を根治（#44）: pane → バックエンドセッション → pid 祖先辿りで session を自動解決し、`claude agents --json` の status を一次シグナル化（画面パターン推定はフォールバック）
- Self-test no longer hangs under CPU contention (#39): terminal rendering changed from one-div-per-character to grouped runs of same-style half-width characters, reducing GPUI element count by 60–90%
  CPU 競合下でセルフテストがハングする問題を解消（#39）: ターミナル描画を「1 文字 = 1 div」から同スタイル連続半角文字のグループ化に変更、GPUI 描画要素数を 60〜90% 削減
- IME candidate window no longer appears at bottom-left when cursor is hidden (#29): added `ime_cursor` field to `Screen` that tracks cursor position even when `CursorShape::Hidden` (used by TUI apps like Claude Code). `bounds_for_range` now always returns a valid position
  カーソル非表示時に IME 変換候補ウィンドウが画面左下に出る問題を修正（#29）: `Screen` に `ime_cursor` フィールドを追加し、`CursorShape::Hidden`（Claude Code 等の TUI アプリが使用）でもカーソル位置を追跡。`bounds_for_range` が常に有効な位置を返すよう修正

## [0.2.5] - 2026-07-03

### Fixed

- Shift+Enter now inserts a newline in Claude Code on machines without tmux (#28): modified-key CSI u encoding is now enabled for all panes, not just tmux-backend panes. Homebrew cask installs (which don't depend on tmux) were silently sending bare `\r` instead of CSI u sequences, breaking multiline input in Claude Code
  tmux 未導入環境でも Claude Code の Shift+Enter 改行が効くように修正（#28）: 修飾付きキーの CSI u 送出を tmux バックエンドペイン限定から全ペインに拡大。Homebrew cask 配布先（tmux 非依存）では素の `\r` が送出され、Claude Code のマルチライン入力が動作しなかった
- Tab/pane layout persistence no longer requires tmux (#30): saving and restoring the layout was silently disabled on machines without tmux (e.g. Homebrew installs), losing all tabs across restarts. Without tmux, the layout is still saved and restored with fresh shells at the saved cwd; with tmux, full restore (running processes) works as before
  タブ / ペイン構成の永続化が tmux 必須でなくなった（#30）: tmux 未導入マシン（Homebrew 配布先等）では保存・復元が無音で無効化され、再起動で全タブが消えていた。tmux 不在でも構成は保存され、保存 cwd の新シェルで復元される。tmux があれば従来通り実行中プロセスごと完全復元
- PTY deaths (shell exit, tmux client kicked, backend tmux server killed) no longer kill backend sessions nor delete layout.json (#30): only user/AI-initiated closes (× button, cmd+W, CLI/MCP close) do. When every pane dies at once (e.g. the backend tmux server is killed externally), tako now keeps layout.json and restores the full tab structure on next launch
  PTY 死亡（シェル exit・tmux クライアント kick・バックエンドサーバー kill）ではバックエンドセッションの kill と layout.json の削除を行わなくなった（#30）: 削除はユーザー / AI の明示 close（× / cmd+W / CLI・MCP close）に限定。バックエンド tmux サーバーが外部から kill され全ペインが一斉終了しても layout.json は保持され、次回起動でタブ構成が復元される

### Added

- In-app update with auto-detection of install method (#36): automatically detects whether tako was installed via Homebrew Cask or GitHub Releases and runs the appropriate update command. Shows a confirmation dialog warning that running processes will be lost, then saves layout → applies update → auto-restarts. Also detects duplicate `tako` CLI binaries on PATH. CLI `tako update status/check/apply` + MCP `tako_update` (50 MCP tools total)
  アプリ内更新 + 配布系統自動判別（#36）: Homebrew Cask / GitHub Releases のどちらでインストールされたかを自動判別し、適切な更新コマンドを実行。更新前にプロセス消失を警告する確認ダイアログを表示し、レイアウト保存 → 更新適用 → 自動再起動。PATH 上の `tako` CLI 重複も検知。CLI `tako update status/check/apply` + MCP `tako_update`（MCP 計 50 ツール）
- Persistence diagnostics (#30): restore outcome/reason and explicit layout deletions are logged to `<data_dir>/persist.log` (rotated at 256KB); corrupted layout files are stashed as `layout.json.corrupt`; `tako persist` / MCP `tako_persist` now report `layout_path` / `layout_exists` / `last_restore` / `log_path`
  永続化の診断機能（#30）: 復元の成否・理由・layout.json の明示削除を `<data_dir>/persist.log` に記録（256KB でローテート）。破損した layout.json は `layout.json.corrupt` へ退避。`tako persist` / MCP `tako_persist` が `layout_path` / `layout_exists` / `last_restore` / `log_path` を返すようになった

## [0.2.4] - 2026-07-02

### Fixed

- **Hotfix (#27)**: default orchestrator profile no longer hardcodes `claude-opus-4-6[1m]`, which made `tako master` unusable on Pro plans (1M-context models are Max/API-only). New default is **no model specification** — the master launches with the claude CLI's default model. `[1m]` models now require explicit opt-in in the profile and print a warning at launch
  **緊急修正（#27）**: 既定プロファイルの `claude-opus-4-6[1m]` ハードコードを廃止（1M コンテキスト版は Max/API プラン限定のため、Pro プランで `tako master` が起動不能だった）。新しい既定は**モデル無指定** = claude CLI の既定モデルで起動。`[1m]` モデルはプロファイルへの明示 opt-in のみとなり、起動時に警告を表示
- Automatic migration (#27): a `default.yaml` still containing the old hardcoded `model: claude-opus-4-6[1m]` is detected at startup (`tako master` / `tako setup` / spawn) and the model line is removed with a backup (`default.yaml.backup-1m`). User-specified models other than the old default are respected
  自動マイグレーション（#27）: 旧既定値 `model: claude-opus-4-6[1m]` が残る `default.yaml` を起動時（`tako master` / `tako setup` / spawn）に検出し、バックアップ（`default.yaml.backup-1m`）を取って model 行を除去。旧既定値以外のユーザー指定モデルは尊重する

### Changed

- Config precedence clarified (#27): `profiles/*.yaml` is the single source of truth for master/worker launch settings. The unused `master_model` / `worker_model` / `effort` keys in `config.yaml` are removed (legacy keys are ignored); `config.yaml` now only holds setup state and `auto_close` / `auto_push`. The setup assistant now writes model settings to profiles and no longer recommends 1M-context models to Pro-plan users
  設定の優先順位を明文化（#27）: master/worker の起動設定の正は `profiles/*.yaml` に一本化。誰にも読まれていなかった `config.yaml` の `master_model` / `worker_model` / `effort` キーを廃止（旧キーが残っていても無視される）。`config.yaml` は setup 状態と `auto_close` / `auto_push` のみに。セットアップアシスタントはモデル設定を profiles に書き込み、Pro プランユーザーに 1M コンテキスト版を提案しないよう修正

### Added

- Profile management CLI/MCP (#27): `tako orchestrator profiles list/show/set` (`--model` / `--clear-model` / `--effort` etc.) + MCP `tako_orchestrator_profiles` (49 MCP tools total) — fix a broken profile without editing YAML by hand
  プロファイル管理 CLI/MCP（#27）: `tako orchestrator profiles list/show/set`（`--model` / `--clear-model` / `--effort` 等）+ MCP `tako_orchestrator_profiles`（MCP 計 49 ツール）— YAML 手編集なしでプロファイルを修復可能に
- Orchestrator profile extensions (#25): per-profile worker model policy (`inherit` / `fixed` / `delegate`), system prompt block control (`disable` / `override` / `prepend` / `append`), and session identity injection
  オーケストレータープロファイル拡張（#25）: プロファイル単位の子 worker モデル制御（`inherit` / `fixed` / `delegate`）、system prompt のブロック単位制御（`disable` / `override` / `prepend` / `append`）、セッション identity 注入
- Remote access (#23 Phase A): WebSocket screen push channel `GET /ws?pane=<id>` — server-side 250ms diff detection, ANSI-colored screen + cursor/size (HTTP polling remains as fallback)
  リモートアクセス（#23 フェーズ A）: WebSocket 画面プッシュ `GET /ws?pane=<id>` — サーバー側 250ms 差分検知、ANSI 色付き画面 + カーソル/サイズ（HTTP ポーリングはフォールバックとして維持）
- Remote screen API: `?ansi=1` (colored output for xterm.js), `?lines=N` (scrollback history), cursor position and pane size in response
  リモート画面取得 API: `?ansi=1`（xterm.js 用色付き出力）、`?lines=N`（スクロールバック履歴）、カーソル位置・ペインサイズを応答に追加
- Viewport-linked resize: `POST /api/panes/:id/resize` + CLI `tako tmux resize` + MCP `tako_tmux_resize`
  ビューポート連動リサイズ: `POST /api/panes/:id/resize` + CLI `tako tmux resize` + MCP `tako_tmux_resize`
- Agent list API: `GET /api/agents` (claude agents --json proxy with tmux pane mapping) + CLI `tako remote agents` + MCP `tako_remote_agents`
  エージェント一覧 API: `GET /api/agents`（claude agents --json プロキシ + tmux ペイン対応付け）+ CLI `tako remote agents` + MCP `tako_remote_agents`
- Conversation log API: `GET /api/sessions/:id/messages?tail=N` (normalized Claude Code transcript) + CLI `tako remote messages` + MCP `tako_remote_messages`
  会話ログ API: `GET /api/sessions/:id/messages?tail=N`（Claude Code transcript の正規化）+ CLI `tako remote messages` + MCP `tako_remote_messages`
- Pane close endpoint: `POST /api/panes/:id/close`
  ペインを閉じるエンドポイント: `POST /api/panes/:id/close`

### Changed

- Connect URL token moved to URL fragment (`/#/connect?token=...`) — no longer appears in server/tunnel access logs or Referer
  接続 URL のトークンを URL fragment 化（`/#/connect?token=...`）— サーバー/トンネルのアクセスログや Referer に残らない

### Fixed

- Shift+Enter now inserts a newline in Claude Code on machines without tmux (#28) — modified-key CSI u encoding is now enabled for all panes, not just tmux-backend panes; the setup assistant no longer claims to configure Claude Code keybindings
  tmux 未導入環境でも Claude Code の Shift+Enter 改行が効くように修正 (#28) — 修飾付きキーの CSI u 送出を tmux バックエンドペイン限定から全ペインに拡大。setup アシスタントが Claude Code 側キーバインドの設定を掲げる案内も廃止
- KV relay URL mismatch between daemon and PWA (unified to the live worker)
  デーモンと PWA で KV リレー URL が不一致だった問題を修正（稼働中の Worker に統一）
- Prompt delivery to claude TUI is now verified (#32): text is pasted via bracketed paste, the submitting Enter is sent as a separate delayed key event, and the input box is checked to be empty afterwards (with standalone Enter retries) — fixes multiline prompts stuck in the input box and intermittent Enter misses in `tako orchestrator spawn` / `tako send` / MCP `tako_send_input`
  claude TUI へのプロンプト送達を検証付きに（#32）: 本文は bracketed paste で貼り付け、送信の Enter は分離した単独キーとして遅延送信し、送信後に入力欄が空へ戻ったことを検証（残留時は Enter 単独再送）— `tako orchestrator spawn` / `tako send` / MCP `tako_send_input` のマルチライン残留・Enter 空振りを修正
- Trust dialog no longer consumes the spawn prompt (#32): the worker cwd is pre-trusted in `~/.claude.json` before launch, with on-screen dialog detection → auto-accept as fallback
  信頼確認ダイアログが spawn プロンプトを消費する問題を修正（#32）: 起動前に worker の cwd を `~/.claude.json` で事前信頼し、フォールバックとしてダイアログ検出 → 自動承諾も実装
- tmux session-targeted send/read fallback was broken on tmux 3.6 (`can't find pane: =<session>`) — target-pane commands now use the explicit `=<session>:` form
  tmux session 指定の send/read フォールバックが tmux 3.6 で壊れていた問題を修正（`can't find pane: =<session>`）— target-pane 系コマンドは `=<session>:` 形式に統一

## [0.2.2] - 2026-07-02

### Added

- Orchestrator profiles: `tako master -<name>` to launch with different configurations (model, effort, system prompt, project subset)
  オーケストレータープロファイル機能: `tako master -<名前>` で設定別のマスターを起動可能（モデル・effort・システムプロンプト・プロジェクトサブセット）
- Profile management: profiles stored in `~/Library/Application Support/tako/orchestrator/profiles/`
  プロファイル管理: `~/Library/Application Support/tako/orchestrator/profiles/` に YAML で保存
- `tako setup --check` now shows available profiles
  `tako setup --check` でプロファイル一覧を表示
- Default profile auto-created on first `tako master` run
  初回 `tako master` 実行時にデフォルトプロファイルを自動生成
- Backward compatible: `tako master dev` (old suffix form) still works
  後方互換: `tako master dev`（旧サフィックス形式）も引き続き動作

## [0.2.1] - 2026-07-02

### Added

- In-app auto-update notification: a status bar notification appears when a new stable release is available, with one-click update
  アプリ内自動更新通知機能を追加。新しい安定版がリリースされるとステータスバーに通知が表示され、ワンクリックで更新できます

## [0.2.0] - 2026-07-02

### Added

#### Interactive Setup / 対話式セットアップ

- `tako setup`: interactive setup command for Claude Code configuration (model selection, effort, CLAUDE.md backup)
  `tako setup`: Claude Code 設定の対話式セットアップコマンド（モデル選択・effort 設定・CLAUDE.md 自動バックアップ）
- `tako setup --reset`: reset and restart setup in one step
  `tako setup --reset`: リセット後にそのままセットアップを再開

#### Menu Bar & Window Management / メニューバー・ウィンドウ管理

- Menu bar: Open Directory, Open Repository, New Window; CLI `tako --dir` for launching with a specific directory
  メニューバー拡充: ディレクトリを開く・リポジトリを開く・新規ウィンドウ + CLI `tako --dir`

#### Media & File Preview / メディア・ファイルプレビュー

- mp4 preview: seek with arrow keys/click, keyboard shortcuts for playback control
  mp4 プレビュー: 矢印キー/クリックでシーク、キーボードショートカットで再生制御
- WebView pane: embedded Chrome-based web view within pane (headless mode, isolated profile)
  WebView ペイン: ペイン内の埋め込み Chrome ベース Web ビュー（headless モード、一時プロファイル）

#### Drag & Drop / ドラッグ＆ドロップ

- OS-level drag & drop: drop files/folders onto tako with context-aware behavior per drop target
  OS レベル D&D: ファイル/フォルダを tako にドロップ、ドロップ先に応じた挙動の出し分け

### Improved

#### Documentation Site / ドキュメントサイト

- Documentation site (tako-docs.pages.dev): Claude Design theme, improved content, sidebar widgets, mascot fix
  ドキュメントサイト（tako-docs.pages.dev）: Claude Design テーマ刷新・コンテンツ充実・サイドバーウィジェット・マスコット修正
- Distribution research and implementation draft (.pkg, Homebrew Cask)
  配布方法の調査結果と実装ドラフト（.pkg、Homebrew Cask）

#### Distribution / 配布

- Homebrew Cask support: `brew install --cask takushio2525/tako/tako`
  Homebrew Cask 対応: `brew install --cask takushio2525/tako/tako`

#### Project Infrastructure / プロジェクト基盤

- Issue templates for bug reports and feature requests
  Issue テンプレートの追加（バグ報告・機能リクエスト）
- .gitattributes for consistent line endings and binary detection
  .gitattributes 追加（改行コード統一・バイナリ判定）

### Fixed

- `orchestrator_spawn` CLI/MCP: pane/tab priority order was inverted
  `orchestrator_spawn` CLI/MCP: pane/tab 優先順位の逆転を修正
- `orchestrator_spawn`: pane/tab parameter now required to prevent ambiguous placement
  `orchestrator_spawn`: pane/tab 指定を必須化し配置の曖昧さを解消
- WebView Chrome launch: use temporary profile to avoid conflicts with existing Chrome sessions
  WebView Chrome 起動: 一時プロファイルで既存 Chrome との競合を回避
- `tako setup`: model suggestion based on user context + latest info; effort fixed to high; interactive mode
  `tako setup`: モデル提案をユーザー状況ベースに改善、effort を high 固定、対話モード修正

## [0.1.0] - 2026-06-26

### Added

#### Terminal Core / ターミナル基盤

- macOS terminal with tabs, pane split/resize/focus, 256-color/truecolor, copy-on-select, bracket paste, IME inline composition, .app bundle with code signing
  macOS ターミナル: タブ・ペイン分割/リサイズ/フォーカス・256色/truecolor・copy-on-select・ブラケットペースト・IMEインライン変換・コード署名付き.appバンドル

#### CLI & MCP / CLI・MCPサーバー

- `tako` CLI with subcommands: split, send, focus, list, read, close, title, resize, equalize, tab operations
  `tako` CLI サブコマンド: split/send/focus/list/read/close/title/resize/equalize/tab操作
- Built-in MCP server (stdio bridge + Streamable HTTP) with zero-config Claude Code connection (`tako setup-mcp`)
  内蔵MCPサーバー（stdioブリッジ + Streamable HTTP）、Claude Codeゼロ設定接続（`tako setup-mcp`）

#### Passive Detection / パッシブ検知

- Shell integration via OSC 7/133 (zsh/bash/fish auto-injection, cwd/state/exit_code tracking)
  シェル統合（OSC 7/133、zsh/bash/fish自動注入、cwd/状態/終了コード追跡）
- Listen port detection (macOS libproc) with inline suggestion chips
  listenポート検知（macOS libproc）+ インライン提案チップ
- AI auto-rename for tabs and panes (Claude Haiku + heuristic fallback)
  タブ・ペインのAI自動リネーム（Claude Haiku + ヒューリスティックフォールバック）
- tmux session visibility panel with tab-grouped display
  tmuxセッション可視化パネル（タブ別グルーピング表示）

#### Workspace / ワークスペース

- File tree sidebar with multi-root workspace display (per-tab cwd aggregation)
  ファイルツリーサイドバー（タブごとのcwd集約マルチルート表示）
- Code preview with syntax highlighting (syntect) and line numbers
  シンタックスハイライト付きコードプレビュー（syntect）+ 行番号
- Markdown preview with rendered/code toggle
  Markdownプレビュー（レンダリング/コード切替）
- Context menu (path copy, Finder reveal, cd, rename, new file/folder, trash) with inline editing
  コンテキストメニュー（パスコピー/Finder表示/cd/リネーム/新規ファイル・フォルダ/ゴミ箱）+ インライン編集
- Drag & drop path insertion from file tree to terminal pane
  ファイルツリーからターミナルペインへのD&Dパス挿入

#### Session Persistence / セッション永続化

- tmux backend: full session restore on restart (running processes + screen content)
  tmuxバックエンド永続化: 再起動時のセッション完全復元（実行中プロセス + 画面内容）
- Graceful fallback to direct spawn when tmux is unavailable; `tako persist` toggle
  tmux未使用環境では直接spawnへ劣化、`tako persist` でトグル可能

#### Shelving / たまり場

- Pane and tab shelving: hide from view while keeping processes alive
  ペイン/タブ退避: プロセスを維持したまま表示から除外
- Drawer UI with live terminal preview cards, horizontal scroll, and drag-and-drop restore
  ライブプレビューカード付きドロワーUI（横スクロール + D&D復帰）

#### Panel & UI / パネル・UI

- Status bar (Zed/VSCode style) with file tree, tmux, and git sidebar toggles
  ステータスバー（Zed/VSCode風）+ ファイルツリー/tmux/gitサイドバートグル
- Integrated tmux view with status badges, orphan detection, and one-click cleanup
  統合tmuxビュー（状態バッジ、orphan検出、ワンクリッククリーンアップ）
- Tab tree: hover preview, pin-to-float, collapse/expand
  タブツリー: ホバープレビュー・ピン留めフロート・折りたたみ
- git graph + diff viewer (sidebar accordion: branch/changes/commits/diff)
  git graph + diffビューア（サイドバーアコーディオン: ブランチ/変更/コミット/diff）

#### Orchestrator / オーケストレーター

- Built-in orchestrator: `tako master` for multi-agent coordination with worker spawn/watch/status
  内蔵オーケストレーター: `tako master` でマルチエージェント連携（worker spawn/watch/status）
- Project management via `tako orchestrator projects`
  `tako orchestrator projects` によるプロジェクト管理

#### Remote Access / リモートアクセス

- HTTP API + PWA for remote terminal access with cloudflared tunnel integration
  HTTP API + PWAリモートターミナル（cloudflaredトンネル統合）
- Daemon mode with QR code display (`tako remote start/stop/status`)
  QRコード表示付きデーモンモード（`tako remote start/stop/status`）

#### Reliability & Performance / 信頼性・パフォーマンス

- MCP/IPC restart resilience (fixed socket path + persistent token)
  MCP/IPC再起動耐性（固定ソケットパス + 永続トークン）
- Full-width character rendering fix, half-width character disappearance fix
  全角文字幅の根本修正、半角文字消失バグ根治
- Spawn reliability: TAKO_PANE_ID stale issue root cause fix
  spawn信頼性: TAKO_PANE_ID stale問題の根治
- UI rendering optimization (16ms debounce, event-driven file tree sync, cached style runs and rows)
  UI描画最適化（16msデバウンス、イベント駆動ファイルツリー同期、スタイルラン/行キャッシュ）
- main.rs modular decomposition (13,736 → 8,359 lines, 39% reduction)
  main.rsモジュール分割（13,736 → 8,359行、39%削減）

#### Distribution / 配布

- GitHub Releases distribution with `scripts/release.sh` (auto version + CHANGELOG extraction)
  GitHub Releases配布（`scripts/release.sh`、バージョン自動読み取り + CHANGELOG連携）
- Version management via `[workspace.package]` in Cargo.toml
  Cargo.toml `[workspace.package]` によるバージョン一元管理
