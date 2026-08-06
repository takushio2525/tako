# Active Context

> このファイルは AI が毎ターン上書きする現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。

## 現在の対象（2026-08-06、Issue #770「再起動でタブが消えた」の調査と根治）

- worktree `../tako-wt-770` / ブランチ `fix/770-restart-tab-loss`
- **根因は再起動ではなかった**: tab 152 は 2026-08-05 10:13:27 JST に GUI のタブ ×
  で close されていた（報告された 13:57 の再起動の 2 時間 44 分前）。確定材料は
  ペインログのクローズマーカー `close:gui-tab`、sessions.yaml の last_seen、
  persist.log の復元数推移（12:24 の再起動時点で既に 6 タブ 10 ペイン）
- 直したのは「失ったことに気づけない / 取り戻せない」側:
  1. **監査記録**（FR-5.15 新設）: セッション kill とタブ close を発生源つきで
     persist.log へ。従来の痕跡はペインログのマーカーだけで、これは `pane_logs`
     設定で OFF にできるため事故調査が原理的に不可能になりうる状態だった
  2. **バックアップ回転条件**（FR-5.11 拡張）: 「ペイン半減」に加えて
     「**セッションを持つペインが消える保存**」で `.bak.1` を回す。実機は 12→10 で
     素通りし、close が 1 ペインずつ届くのでどの段階も半減にならず 16 日間 0 世代だった
  3. 検証用の graceful quit トリガ（`platform::quit_signal`）: **隔離モード限定**で
     SIGTERM を正規の quit（`on_app_quit` を通る）へ読み替える

## 検証の安全規約（2026-08-06 の実害を反映。最重要）

- **System Events のキーストローク送出は禁止**。`keystroke "q" using command down` は
  グローバル送出で、frontmost 切替とのレースで**本番 tako に Cmd+Q が着弾して終了させた**。
  pid を狙えない手段は検証に使わない
- GUI インスタンスの graceful quit は**対象 pid への SIGTERM**（上記 3 の仕掛け）で撃つ
- e2e は各位相の前後で**本番 tako-app の pid 不変**をアサートし、変化したら即 FATAL
- 自分が起動した隔離インスタンスは終了時に必ず回収する（漏れると次の検証を汚す。
  実際に漏れた 1 プロセスがセルフテストの連続失敗を招いた）

## 検証状況

- 隔離 e2e（persist ON・専用 data dir / tmux socket、pid 指定 SIGTERM の graceful quit）:
  - 判定 0: `on_app_quit` を通った = 本番と同じ終了経路
  - 判定 A: **プレビュー混在タブ（ターミナル 2 + PDF 1）は quit → 再起動で
    tmux セッション集合が完全一致 = 喪失ゼロ**（復元成功 2 タブ / 4 ペイン・再 attach 3）
  - close の before/after: persist.log の監査行 0 → `セッション kill: pane=… （発生源 …）`、
    `.bak.1` 無し → 喪失直前の 2 タブ 4 ペイン版が残り `tako recover` に出る
- 単体: layout の回転条件（旧条件へ戻すと新規 2 本が FAILED を実測）
- 番犬 3 本（tako-app）: kill が明示 close 経路の外に無い / `on_app_quit` が kill しない /
  `close:gui-tab` を GUI タブ × 以外が名乗らない。いずれも違反注入で FAILED を実測
- `cargo test --workspace` 1881 件緑 / fmt --check / clippy(-D warnings) 緑
- 隔離セルフテスト `TAKO_APP_SELF_TEST_OK`（exit 0）。項目 104 のマーカー検査は
  「ウィンドウが前面でないと新ペインが描画されずペインログが作られない」ため
  76d と同じ条件で SKIPPED になることがある

## 不変条件

- 本番 GUI・本番 tmux（socket `tako`）・本番 data dir に触れない
- セッション kill は**明示 close のときだけ**。quit / PTY 死亡では kill しない（#30）
- 縮退の連鎖で健全世代を押し出さない 10 分ガードは維持（#177）

## 未着手・持ち越し

- 別 Issue 化候補: GUI 経路の close が `workers.yaml` に closed を記録しない
  （`mark_closed_by_pane` は dispatch 経路のみ）/ `cargo test` が本番 data dir へ
  supervisor.log・recent.json を書く / 本番でも SIGTERM で layout を保存したい
- #691 GUI モードのクローズはユーザーの実使用確認待ち
- #658、#601 案 2、#632、#633、#638、#651 ほか既存キューは #770 の対象外
