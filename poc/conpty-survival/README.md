# poc/conpty-survival — ConPTY 生存セマンティクスの実測スパイク（M0）

Issue #518 の案 **B-1（自前 ConPTY セッションホスト）** が成立するかを実機で確定させるための
使い捨て検証コード。品質基準の対象外（`poc/` は Phase 0 の使い捨て置き場）。

- 実測日: 2026-07-27
- 環境: Windows 11 Home 10.0.26200 / rustc 1.95.0 / PowerShell 7.6.4 / pwsh 7
- ルートの Cargo workspace には参加しない（ルート側 `exclude = ["poc"]` + 本 Cargo.toml の空 `[workspace]`）

## 結論

**B-1 は成立する。ただしセッションホストは `DETACHED_PROCESS` で起動しなければならない。**

| 実測 | 結果 |
|---|---|
| ① ConPTY を所有する host を kill | 中のシェルは **1 秒以内に道連れで死ぬ**（conhost も消える） |
| ② 中継 client だけを kill | シェルは **生存し、実行も継続**。再 attach で出力・セッション状態とも回復 |
| ③ host を tako のペイン内から**コンソール継承**で起動 → tako 死亡 | host もシェルも**道連れで死ぬ** |
| ③' host を **DETACHED_PROCESS** で起動 → tako 死亡 | host もシェルも**生存**し、再 attach 可能 |

①は「ConPTY の寿命は所有プロセスに縛られる」ことの確認であり、B-1 の前提そのもの
（だから tako 本体ではなく常駐 host が所有する）。②③' が B-1 の成立条件を満たす。

## 使い方

```powershell
cargo build --release

# 常駐セッションホスト（ConPTY を所有）。実運用では DETACHED_PROCESS で起動すること
.\target\release\poc-conpty.exe launch --detached --cmd '"<exe>" host --pipe tako1 --status s.json --log s.log --marker m1'

# 薄い中継クライアント（tako の PTY の中で動く想定）。これを kill する = tako が死ぬ
.\target\release\poc-conpty.exe client --pipe tako1 --out c.out --send 'echo hi\r' --exit-after-ms 3000
```

`host` は `--status` に `{"host_pid":..,"shell_pid":..}` を書く。`--log` には ConPTY 出力の
生バイトと進行ログの両方が落ちる。

## 実装上の注意（M2 へ引き継ぐ）

実装中に踏んだ罠。どれも「無音で失敗する」ため、本実装で再度踏むと原因特定に時間を溶かす。

1. **`UpdateProcThreadAttribute` の `lpValue` には HPCON の値そのものを渡す**（`&hpc` ではない）。
   ポインタを渡すと子はデタラメなコンソールに紐付き、`CreateProcessW` は成功するのに
   ConPTY 出力が 1 バイトも来ない。ホストにコンソールがあれば「シェルは生きているのに無音」、
   無ければ「シェルが即死」という別々の症状に化けるので原因が分かりにくい
2. **セッションホストは `DETACHED_PROCESS` で起動する（必須）**。tako のペインから
   コンソールを継承して起動すると、tako 終了時にそのコンソールの終了イベントが
   継承プロセス全体に伝播して道連れになる（実測③）。`CREATE_NEW_PROCESS_GROUP` も併せて付け、
   Ctrl+C がペイン経由でホストへ飛ばないようにする
3. **中継パイプは一方向 2 本にする**。1 本の duplex パイプを同期ハンドルで読み書き兼用すると、
   常時 pending の `ReadFile` の後ろで `WriteFile` が詰まって両側デッドロックする
   （同期ハンドルの I/O はカーネルが直列化するため）。overlapped I/O にするなら 1 本でもよい
4. **client 不在でも ConPTY 出力を吸い続ける**。誰も読まないとパイプが詰まってシェルが
   write でブロックし、「生きているのに動かない」状態になる。ホスト側にリングバッファを持たせ、
   attach 時にリプレイする（②の「再 attach で出力を再取得」はこれで実現している）
5. **client の切断検知は書き込み失敗だけに頼らない**。シェルが無出力の間は書き込みが発生せず
   切断に気づけないので、再 attach が `ERROR_PIPE_BUSY` で弾かれる。`PeekNamedPipe` の
   失敗を併用する
6. **host が落ちると配下のシェルは全部死ぬ**（①）。裏を返すと孤児シェルは溜まらない
   （macOS の tmux orphan 問題 #177 / #191 のクラスは発生しない）が、
   **ホストは単一障害点**になる。1 ホストが N セッションを持つか 1 セッションごとに
   1 ホストを立てるかは M2 で決める必要がある（tmux は前者と同じトレードオフ）
7. **未検証: Job object**。親が `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` 付きの Job に居ると
   `DETACHED_PROCESS` でも道連れになる。tako を Explorer から起動する通常経路では
   問題にならないはずだが、必要なら `CREATE_BREAKAWAY_FROM_JOB` を検討する

## サブコマンド

| | |
|---|---|
| `host --pipe N --status F --log F [--marker S] [--shell EXE] [--cmdline S]` | ConPTY を所有する常駐プロセス（B-1 の `tako session-host` 相当） |
| `client --pipe N --out F [--log F] [--send TEXT]... [--send-delay-ms N] [--exit-after-ms N]` | 中継プロセス（`session-client` 相当）。`--send` は `\r` `\n` `\t` `\\` のみ解釈 |
| `launch --cmd "CMDLINE" [--detached]` | 生成フラグを変えて子を起こす補助（③の A/B 比較用） |
