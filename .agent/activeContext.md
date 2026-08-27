# Active Context

> このファイルは AI が毎ターン上書きする現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。

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
  `TAKO_932_NO_OFFSCREEN_GEOMETRY` / `TAKO_961_LEGACY` / `TAKO_966_LEGACY`

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

## 次の一手

- **v0.7.9 の初回同時リリース**（#965。PR merge 後）: main を pull → `git tag -a v0.7.9` を
  push（ここで Windows のワークフローが走る）→ `scripts/release.sh --test`。
  release.sh が Windows の添付を待ってノートを作り直し、揃わなければ exit 3 を返す
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
