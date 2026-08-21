# Active Context

> このファイルは AI が毎ターン上書きする現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。

## 現在の対象（2026-08-22）

- **#467 Windows 移植はスライス 1〜7c / 9 が main へ入り、残りはスライス 8（棚卸し）だけ**
- **#884（空白を含む cwd でペインが即死）を解消**（PR #887）。原因は psmux ではなく
  **tako の argv → Windows コマンドライン変換**（alacritty の `escape_args` が `false` のままで
  `program` と `args` が素の空白連結）。境界 **`platform::shell::apply_arg_escaping`（B1）** が
  `tty::new` の前に必ず通る（unix は恒等）。`-c <cwd>` と `-e KEY=<空白入り>` の両方が直る。
  詳細は plan の「#884 の記録」
- **#877 / #875 / #873 / #881 も main へ入っている**（agents 走査・実行ペイン・方言判定の
  一本化・器へ渡す第 1 語）
- **#766（器の中でシェル統合が届かない）を解消**（PR #891）。**器の側では直らない**
  （upstream に passthrough の実装が無く、psmux は parse → 再描画型なので**どのバイト列も
  素通りしない**）ため、抽象境界 **`tako_core::osc_sink`（側路）** で同じ OSC バイト列を
  ファイルで運び PTY 経路と同じ `osc_tap` へ通す。**`osc_passthrough` の申告は変えていない**
- **#870（`~/` のターミナルリンクが Windows で効かない）を解消**（PR #892）。
  ホーム解決が 2 か所にあり `links.rs` 側が `HOME` 決め打ちだった（Windows は `HOME` を
  持たないので必ず `None`）。**`paths::home_dir()` が唯一の入口**（番犬つき。`cfg` は
  持たない = `HOME` → `USERPROFILE` の順ならどちらの OS でも正しい）。
  **ホーム解決は他に 15 箇所ある**（分類つきで **#893** に起票。番犬の走査範囲を
  広げるのはそちらの作業）

## 実機で env 依存を測るときの作法（#877 / #870 で 2 回踏んだ）

- **GUI 起動時の env を再現してから測る。** `SHELL`（#877）も `HOME`（#870）も
  **SSH セッションの Process スコープにしか無い**（`User` / `Machine` は空）。
  GUI 起動の tako には渡らないので、`Remove-Item Env:SHELL` / `Remove-Item Env:HOME` を
  先に打たないと**壊れている経路が動いて見える**
- **A/B は「修正だけを戻す」形にする。** テストごと戻すと、失敗が
  「テスト自身の env 読み」なのか「製品の解決経路」なのか切り分けられない
- **誤読されやすい修正では「逆向きの固定」も置く**（#766 / #870）。既存テストを
  そのまま緑に保つことで「直っていないこと」と「別経路で届いていること」を同時に固定する

## #884 で分かった実機の作法（次の worker が踏みやすい）

- **同じ情報を 2 経路で渡している場所ではテストが空振りする**。cwd は器へ `-c` と
  `CreateProcessW` の `lpCurrentDirectory` の**両方**で渡っており、`-c` が割れても
  psmux はクライアントの cwd へ落ちるので **+600ms までは正常に見え、+1200ms で消える**。
  「一度でも正しく見えたら合格」のテストは**修正を戻しても通った**（実測）。
  出現を待ってから**生存を数秒見張る**こと
- **`git checkout <ref> -- <path>` は index を汚す**。あとで `git checkout -- <path>` すると
  「戻した版」が復活し、HEAD と working tree が食い違ったまま測ってしまう
  （このセッションで 1 回踏んだ。`git reset --hard <ref>` で確定させる）
- **fresh worktree は `web/tako-remote/dist/` を持たない**（.gitignore 済み）。
  `rust_embed` が埋め込むので **tako-control のコンパイルが即失敗する**（E0599 の連鎖）。
  `npm run build` するか既存 worktree からコピーしてから `cargo test --workspace` を回す
- **macOS のクロスチェックでは捕まらない**種類の失敗がまだある: 上記 dist はビルド環境の
  問題なので `check-windows.sh --all-targets` が緑でも実機で落ちる

## 次の一手

- **スライス 8（棚卸し）**: #865 の到達範囲表 + #875 / #877 / #884 の実測で
  `tako_run` / `tako_run_interactive` / `tako_show_command` /
  `tako_orchestrator_watch` / `tako_orchestrator_worker_status` を `Pending` から
  倒せる材料が揃った（作法 4 に従いマトリクスは #884 では触っていない）
- **同型の一族**（`$SHELL -l -c` の直書き）が残っている: `tako-core/src/lib.rs` /
  `tako-app/src/autorename.rs` / `tako-app/src/preview.rs` /
  `tako-control/src/config_share/env.rs` は
  **B21（`child_cmd::user_env_cli`）へそのまま寄せられる形**（「ユーザーの環境で CLI を
  1 回走らせる」）。
  **ただし `tako-control/src/setup_bootstrap.rs` の `login_shell_sees()` は例外**（#868 の
  担当者からの申し送り）: 意図は「ログインシェルの PATH にランチャーのディレクトリが
  載っているか」の確認で、`user_env_cli` の program / args に対応するものが無い
  （Windows の PATH はレジストリ由来でシェル profile 由来ではない）。現状
  `if cfg!(windows) { return false; }` で先に落とす**意図的な unix 専用**なので、
  機械的に寄せると意味が変わる。番犬の都合で境界の外に置けないなら、
  B21 側に「ログインシェルの PATH を読む」専用の口を足す（Windows は None）方が素直
- 項目 93 以降を Windows で通すには **#889** が要る（**#766 ではなかった**: セルフテストは
  `TAKO_PERSIST=0` で**器なしのペイン**を測っている。前提は `$PROFILE` 配置と `cat` 決め打ち）
- **#885**（tako-app の spawn が backend の `wrap_spawn` を通らない）は恒久課題として未着手
- 側路（#766）に対応しているのは **PowerShell の統合スクリプトだけ**
  （`side_channel_supported()`）。unix で素通ししない器を採るなら zsh / bash / fish の
  正本へ足してからフラグを立てる（黙って true にしない）
- 隣接の穴（別件・未起票）: alacritty の `cmdline()` は **`program` を一切
  エスケープしない**（`escape_args` の対象外）。空白入りのプログラムパスは
  `CreateProcessW` の「空白区切りを順に試す」探索に救われているだけ

## 現フェーズで Read すべき設計書

- `.agent/plans/2026-08-windows-main-merge-wip.md`（「#884 の記録」節に原因層の対照実測・
  機序・#881 非回帰の確認・テストの検出力で踏んだ罠。「#881 の記録」「agents 走査の
  Windows 対応（#877）」節に実機 A/B と作法。「スライス 8 の前提: 器の中のシェル統合（#766）」節に
  upstream の調査結果・側路の設計・製品経路の before/after。「8 の前提」節に到達範囲・実機レシピ）
- `.agent/plans/2026-07-windows-port-architecture.md`（境界の定義）
