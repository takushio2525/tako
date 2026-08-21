# Active Context

> このファイルは AI が毎ターン上書きする現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。

## 現在の対象（2026-08-21）

- **#467 Windows 移植はスライス 1〜7c / 9 が main へ入り、残りはスライス 8（棚卸し）だけ**
- **#884（空白を含む cwd でペインが即死）を解消**（PR #887）。原因は psmux ではなく
  **tako の argv → Windows コマンドライン変換**。`TerminalSession::spawn` が
  `tty::Options` を既定で組んでいたため alacritty の **`escape_args` が `false`** のままで、
  Windows の `cmdline()` は `program` と `args` を**素の空白で連結するだけ**だった
  = tako の argv 形の `SpawnCommand.args` が Windows でだけ
  「生のコマンドライン断片」に意味が変わっていた。境界
  **`platform::shell::apply_arg_escaping`（B1）** を新設して `tty::new` の前に通す
  （unix は恒等）。`-c <cwd>` だけでなく **`-e KEY=<空白入りの値>`** も同時に直る
- **#877 / #875 / #873 / #881 も main へ入っている**（agents 走査・実行ペイン・方言判定の
  一本化・器へ渡す第 1 語）

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
- 項目 93 以降を Windows で通すには **#766**（psmux 越しに OSC が届かない）が要る
- **#885**（tako-app の spawn が backend の `wrap_spawn` を通らない）は恒久課題として未着手
- 隣接の穴（別件・未起票）: alacritty の `cmdline()` は **`program` を一切
  エスケープしない**（`escape_args` の対象外）。空白入りのプログラムパスは
  `CreateProcessW` の「空白区切りを順に試す」探索に救われているだけ

## 現フェーズで Read すべき設計書

- `.agent/plans/2026-08-windows-main-merge-wip.md`（「#884 の記録」節に原因層の対照実測・
  機序・#881 非回帰の確認・テストの検出力で踏んだ罠。「#881 の記録」「agents 走査の
  Windows 対応（#877）」節に実機 A/B と作法。「8 の前提」節に到達範囲・実機レシピ）
- `.agent/plans/2026-07-windows-port-architecture.md`（境界の定義）
