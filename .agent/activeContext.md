# Active Context

> このファイルは AI が毎ターン上書きする現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。

## 現在の対象（2026-08-06、Issue #772 stale binary 検知の CPU 張り付き）

- 本番 GUI が CPU 40〜65% に張り付き、perf.log に `periodic_prep:stale_binary` が
  約 2.5 秒間隔で 392〜466ms（メインスレッド専有）と出ていた不具合の修正
  （worktree `../tako-wt-772`）
- 真因は「毎 tick × 対象ペインごと」に `find_claude_pid_for_backend` を呼んでいたこと。
  1 回あたり `tmux list-panes -a` + `ps -axo pid=,ppid=` の **2 プロセス**を起こすため、
  master / worker が 6 ペインで 12 プロセス / 2 秒。旧コメントの
  「pidpath は FFI 1 回で µs 級」はこの前段のサブプロセスを勘定していなかった
- 直した 3 点:
  1. **採取の束ね直し**: `ProcessSnapshot`（tmux + ps を 1 回ずつ）を新設し、
     ペイン数によらず 2 プロセスで全ペインを解決する
  2. **background 化**: UI スレッドは対象ペインの一覧作り（`collect_stale_binary_scan`）と
     結果反映（`apply_stale_binary_scan`）だけ。走査は background executor
  3. **頻度削減**: `should_rescan`（純関数）で、起動直後 / claude の指紋が変わった /
     対象ペインが増減した / 60 秒経った、のいずれかのときだけ重い走査を回す。
     それ以外の tick は **stat だけ**（`current_binary_fingerprint`）
- 副産物: `which claude` のサブプロセスを PATH 走査（stat のみ）へ置換。
  取りこぼし防止に `which` はフォールバックとして残す

## 検証状況

- 隔離インスタンス（6 worker ペイン・`TAKO_PERF_VERBOSE=1`）で before / after 実測:
  - `periodic_prep:stale_binary` p50 289〜323ms / max 478ms → **p50 0ms / max 0ms**
  - しきい値超えの perf.log 行 60 秒あたり 24〜25 行 → **0 行**
  - `ps` の起動回数（shim で実測）60 秒あたり 175 回 → **34 回**
    （残りは sleep guard 等の別系統。stale 検知ぶんは約 144 → 約 2）
- セルフテスト項目 103 を新設: 偽 claude（`versions/1.0.0` → `1.0.1` の symlink 差し替え）と
  実 tmux セッションで ①同版ならバナー無し ②変化無しの tick は走査を省く
  ③差し替えでバナーが出る、を通しで検証
- 単体 9 本（`should_rescan` / 指紋 / PATH 走査）追加
- `cargo test --workspace` / `fmt --check` / clippy（全 target・deny warnings）全緑

## 不変条件

- 本番 GUI・本番 tmux（ソケット `tako`）・本番 data dir に触れない
  （隔離は `TAKO_ISOLATED=1` + 明示 socket / discovery。CLI 側は呼び出し元の
  `TAKO_SOCKET` / `TAKO_TOKEN` / `TAKO_PANE_ID` を必ず unset する）
- stale 検知の**判定ロジック**（何を stale と見なすか・バナーの出し方）は変えない。
  変えたのは実行場所と頻度だけ
- 定期ループの UI スレッド部にサブプロセス実行を置かない（#168 / #212 / #340 と同じ原則）

## 未着手・持ち越し

- #691 GUI モードのクローズはユーザーの実使用確認待ち
- #658、#601 案 2、#632、#633、#638、#651 ほか既存キューは #772 の対象外
