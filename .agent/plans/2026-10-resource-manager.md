# tako 全体の PC リソース管理の設計（Issue #1992 S0）

> エピック #1992 の S0。**製品コードは 1 行も変えていない**。土台は main（`d4b83de` = v0.8.31）。
> 根拠の数値は §1 の手順でこの PC（36 GB / 18 論理 CPU）を読み取りだけで測ったもの。
> 子 Issue の一覧と依存は §12（正本は #1992 のコメント）。判断が要る点は §11 にまとめた。

## 0. 結論（先に要点）

- **計測は安い**（§1・§2）。PC 全体の指標（メモリの内訳・swap・メモリ圧・CPU・load）は 1 回 **0.008 ms**、
  全プロセス 1,320 本の走査（親子・名前・物理フットプリント・CPU 時間）は 1 回 **wall 11.0 ms / CPU 5.2 ms**。
  機全体は 2 秒ごと、プロセス走査は 5 秒ごと（見ている間と順番待ちが動いている間は 2 秒）で
  **1 コアの 0.10〜0.26%**。子プロセスは起こさず、背景スレッドだけで採る（NFR-8）
- **「重い処理」は 3 つの層で捉える**（§3）:
  1. **観測**（常時・全部）: プロセス走査で cargo / rustc / node のビルド系・Chromium・隔離 GUI などを
     分類し、ペイン（→ worker / master）か「tako の外」へ帰属させる。見える化と枠の勘定に使う
  2. **順番待ちの入口**（明示）: `tako queue <コマンド>`。**始まる前に待たせられるのはここだけ**。
     AI はこの repo の AGENTS.md のコマンド表と `scripts/` の入口が通すので、打つ言葉は増えない
  3. **cargo の全体並列の上限**（透過・後のスライス）: tako のペインへ共有 jobserver を
     `CARGO_MAKEFLAGS` で注入すると、**包まない cargo も札の数で絞れる**（cargo 1.95 で実測。§1.6）
- **順番待ちの本体は tako-control の調停役**で、IPC の受け口（`ipc::submit`）が UI スレッドへ渡す前に
  答える（§4）。UI が固まってもビルドの割り当ては止まらない。待ち行列の正本は各 `tako queue`
  （クライアント）が持ち、**tako の再起動をまたいで順番を保つ**。tako が居なければ待たずに走る（fail-open）
- **既定の閾値はメモリ優先**（§4.3）。同時数 = RAM 12 GB あたり 1（1〜4）、空きメモリの予約、
  **メモリ圧 warn では「1 件ずつ」に絞るだけで止めない**（この PC は普段の運転で 3 分中 42% が warn
  だった = 止めると何も走らない）、critical で新規の割り当てを止める
- **CLI / MCP は 1 語に寄せる**（§8）: CLI `tako queue`（引数なし = 状況、コマンド付き = 順番待ちで実行）、
  MCP は新設 1 本 `tako_queue`（約 1.8 KB。カタログは 192,549 / 204,800 B → 約 194.4 KB）
- **スライスは 8 本**（§12）: 計測と読み取り口 → 順番待ち → 見せ方 → 優先度と ETA → AI の運用 →
  cargo の jobserver → スマホ → Windows。最初の 2 本で「PC が落ちる」の主因（ビルドの同時多発）を止める
- **agent の起動も待たせる（#2413。2026-10-26 の判断で §11-7 を改めた）**: 10/25 夜の 3 回目の停止は
  ビルドではなく agent の常駐（claude 40 本 + MCP の子で 650 プロセス・約 40 GB）が原因で、再起動後の一斉の
  復元が 8 分で同じ水準へ戻した。メモリが逼迫しているあいだは spawn / run・再起動後の復元などの入口を
  **同じ `admit`** で待たせ、復元は 1 本ずつ時差で起こす。走っている agent は止めない（§9.6）

## 1. 実測（設計の根拠）

すべて**読み取りだけ**。本番の GUI・他のペインのプロセスには触らず、負荷も生成していない
（cargo の実験は scratchpad の 6 クレートの小さな workspace で、ビルドスクリプトが 0.7 秒眠るだけ）。

### 1.1 再現の手順

| 測ったもの | 手順 |
|---|---|
| PC 全体の指標のコスト | 付録 A の C（`clang -O2`）で `sysctlbyname`（`hw.memsize` / `vm.swapusage` / `kern.memorystatus_vm_pressure_level`）+ `getloadavg` + `host_statistics64(HOST_VM_INFO64)` + `host_statistics(HOST_CPU_LOAD_INFO)` を 1,000 回 |
| 全プロセス走査のコスト | 同じ C で `proc_listallpids` → 各 pid に `proc_pidinfo(PROC_PIDT_SHORTBSDINFO)` + `proc_pid_rusage(RUSAGE_INFO_V4)` を 20 回 |
| argv / env の読み取りのコスト | 自分の uid のプロセスへ `sysctl(KERN_PROCARGS2)`、`TAKO_PANE_ID=` を含む数も数える |
| CPU 時間の単位 | tako-app 1 本の `ri_user_time + ri_system_time` と `ps -o time=` を突き合わせ |
| メモリの推移 | 付録 A の内訳（Activity Monitor と同じ式）を 2 秒ごとに 85 回（約 3 分） |
| claude 1 本あたりの常駐 | `ps -axo pid=,ppid=,rss=,comm=` から `claude` の子孫の RSS を合計 |
| cargo の進み | 上記の小さな workspace を `CARGO_TERM_PROGRESS_WHEN=always` / `CARGO_TERM_PROGRESS_TERM_INTEGRATION=true` で、TTY 無し / `script` の疑似端末 / 隔離 tmux 越しにビルドし、出力のバイト列を見る |
| jobserver | 同じ workspace を、札を N 枚入れた fifo を `CARGO_MAKEFLAGS`（または `MAKEFLAGS`）の `--jobserver-auth=fifo:<path>` で渡してビルドし、所要時間と終了後の札の数を見る |
| MCP カタログ | 本番 GUI（v0.8.31 = このブランチの土台と同じコミット）の HTTP MCP へ `tools/list` を 1 回投げ、本体と各ツールの JSON のバイト数を数える |

### 1.2 PC 全体の指標

```
system_metrics: 0.0077 ms/回 (wall), 0.0068 ms/回 (cpu)  [1000 回平均]
  memsize=36.0 GB pressure_level=1 load=24.0/24.9/14.4 swap=0.00/0.00 GB
```

1 回の採取は 8 µs で、2 秒ごとに常時採っても **1 コアの 0.0004%**。ポーリングのコストは問題にならない。

### 1.3 プロセス単位

```
full_scan: 1320 プロセス (rusage 可 992)  10.97 ms/回 (wall), 5.20 ms/回 (cpu) [20 回平均]
procargs2(自分の uid): 994 本 / 10.92 ms（うち TAKO_PANE_ID を持つ 408 本）
tako-app ps time: 2:56.39
pid … raw=4233668561  raw/1e9=4.23 s  raw*timebase/1e9=176.40 s   (timebase 125/3)
```

- 走査 1 回の CPU は **5.2 ms**。5 秒ごとで 1 コアの 0.10%、2 秒ごとで 0.26%
- **`ri_user_time` / `ri_system_time` は Apple Silicon では mach tick**（timebase 125/3 を掛けて初めて
  `ps` の 2:56.39 = 176.39 秒と一致する）。秒として読むと CPU% が 41.7 分の 1 になる罠
- **root のプロセス（約 330 本）は rusage を読めない**（992 / 1,320）。ペインの子孫 = 自分のプロセスなので困らない
  （#1979 の `KERN_PROCARGS2` と同じ性質）
- argv / env の読み取り（`KERN_PROCARGS2`）は走査と同じだけ掛かる（10.9 ms）。**新しい pid だけ読む**
  （pid と開始時刻 `ri_proc_start_abstime` の組で覚える）。既存の `ports::process_table` は毎回全 pid の argv を
  読んでいる（`crates/tako-core/src/ports.rs:630`）

名前ごとの物理フットプリント（Activity Monitor の「メモリ」と同じ値）の上位:

```
  node             175 本  8.87 GB
  2.1.294           29 本  8.70 GB     ← claude（実行ファイル名が版数）
  python3.14        36 本  2.63 GB
  rustc              2 本  2.62 GB
  uv                36 本  0.93 GB
  chrome-headless    8 本  0.37 GB
claude 本体 28 本: RSS 中央値 298 MB / 合計 8.6 GB
claude + 子孫: RSS 中央値 596 MB / 最大 2695 MB / 合計 21.0 GB / 子孫の数 中央値 16
```

- **claude の実行ファイル名（`pbsi_comm`）は版数 `2.1.294`**。名前では claude と分からないので、
  既存の agent 判定（argv）を使う
- claude 1 本ごとに MCP サーバー（github / filesystem / sequential-thinking / playwright-mcp が各 21 本・
  `tako mcp serve` の橋渡しが 29 本・uv / python の MCP）がぶら下がる。**worker 1 本の常駐は子孫込みで約 0.6 GB**
  （RSS。共有ページを重複して数えるので上限寄りの値）。25 本で約 15 GB になる
- rustc は 2 本で 2.62 GB（1 本あたり約 1.3 GB。tako-app の本体や link はさらに大きい）

### 1.4 いまの機の状態（3 分の推移）

```
11:59:32 total=36.0 GB used=28.2 (app 8.6 + wired 4.8 + compressed 14.7) cached=5.4 available=7.8 swap=0.00/0.00 pressure=2 { 23.50 21.01 15.68 }
…
samples 85 pressure {'2': 36, '1': 49} available min/max 6.1 8.1 compressed 11.4 15.2 swap max 0.0 load1 min/max 20.73 35.66
```

**普段の運転（claude 28 本 + ビルド 1〜2 本）で、メモリ圧は 42% の時間 warn（2）**。swap は 0 のまま、
圧縮が 11〜15 GB を持っている。macOS は swap の前に圧縮で耐えるので、**warn は「危ない」ではなく
「余裕が無い」の合図**。事故（swap 6.8 / 8 GB・8.4 / 9.2 GB）はこの先で起きている。
→ 閾値は「warn で止める」ではなく「warn で 1 件ずつ・critical か swap の増加で止める」にする（§4.3）。

### 1.5 cargo の進み（ETA の材料）

| 条件 | 出たもの |
|---|---|
| TTY 無し・既定 | 進みの表示なし |
| TTY 無し・`CARGO_TERM_PROGRESS_WHEN=always` + `WIDTH=80` | stderr に `Building [=====>  ] N/16` が 0/16〜15/16 で全部出る |
| 疑似端末・`CARGO_TERM_PROGRESS_TERM_INTEGRATION=true` | OSC 9;4 の進捗（`ESC]9;4;1;6` … `9;4;1;94`、終わりに `9;4;0;0`） |
| 疑似端末 → 隔離 tmux 3.6b（`-f /dev/null`）→ attach の出力 | `hello` は届くが **OSC 9;4 は 1 つも届かない** |

- **AI の Bash ツールには TTY が無い**ので、worker のビルドは進みを一切出していない。
  強制すれば `N/M` が取れる（M = そのビルドのユニット総数）
- tako の本番ペインは器（tmux）越しなので、**OSC 9;4 は tako の OSC タップ（`osc_tap.rs` は 7 / 133 だけ）
  まで届かない**。OSC 9;4 には頼らない
- → ETA は「`tako queue` が非 TTY のときだけ `N/M` を強制して自分で読み、その行は素通しから外す」+
  「過去の所要時間」で組む（§6）

### 1.6 cargo の外からの並列制御（jobserver）

```
jobserver なし:                     2.91 s / 2.41 s
MAKEFLAGS fifo・札 0 枚:            6.36 s   札 前 0 → 後 0
MAKEFLAGS fifo・札 4 枚:            2.46 s   札 前 4 → 後 4
CARGO_MAKEFLAGS fifo・札 0 枚:      6.53 s
CARGO_MAKEFLAGS fifo・札 4 枚:      2.59 s
fifo が無い（パスだけ渡す）:        exit 0・警告なし（cargo は自前の並列へ黙って戻る）
```

- **cargo 1.95 は外から渡した fifo の jobserver に従う**（札 0 枚 = 暗黙の 1 枚だけで直列 → 6.5 秒）。
  札は終わると全部返る
- **`CARGO_MAKEFLAGS` で渡せば GNU make を巻き込まない**（`MAKEFLAGS` にすると、`-j` 無しの素の `make` まで
  並列で走り出し、並列に耐えない Makefile を壊す）
- fifo が無ければ黙って従来の挙動 = **tako が居なくても壊れない**
- **取り分（`CARGO_BUILD_JOBS`）と共有 jobserver は厳しい方が効く**: `CARGO_BUILD_JOBS=1` + 札 4 枚 = 5.60 s、
  `CARGO_BUILD_JOBS=8` + 札 0 枚 = 7.71 s（どちらも直列）。§4.3 の取り分と S6 の札は重ねて使える

### 1.7 予算の残り

| 対象 | いま | 上限 | 残り |
|---|---|---|---|
| MCP カタログ（160 本） | 192,549 B | 204,800 B | 約 12.0 KB |
| master system prompt（tako が作る部分） | 17,647 B | 18,944 B | 約 1.3 KB |
| AGENTS.md | 28,090 B | 30,720 B | 約 2.6 KB |

似た形のツール 1 本の大きさ: `tako_check_health` 511 / `tako_limit_service` 1,050 / `tako_scrollback` 1,059 /
`tako_platform` 1,291 / `tako_orchestrator_workers` 1,550 / `tako_panel` 1,705 / `tako_limit_resume` 1,831 /
`tako_sleep_guard` 2,234 / `tako_todo` 2,673（B。中央値は 1,050）。

## 2. 計測（論点 1）

### 2.1 何をどう採るか

| 指標 | macOS | Windows |
|---|---|---|
| メモリの内訳 | `host_statistics64(HOST_VM_INFO64)`。使用 = app（`internal − purgeable`）+ wired + 圧縮、空き = 全体 − 使用（Activity Monitor と同じ式） | `GlobalMemoryStatusEx`（`ullTotalPhys` / `ullAvailPhys` / `dwMemoryLoad`） |
| swap / commit | `sysctl vm.swapusage` | `GetPerformanceInfo` の `CommitTotal` / `CommitLimit`（commit の枯渇が swap の枯渇に当たる） |
| メモリ圧 | `kern.memorystatus_vm_pressure_level`（1 normal / 2 warn / 4 critical） | **OS に無い** → `dwMemoryLoad` と commit 比から 3 段を合成（境目は S8 で実測して決める） |
| CPU% | `HOST_CPU_LOAD_INFO` の差分 | `GetSystemTimes` の差分（B25 と同じ材料） |
| load | `getloadavg` | **無い。作らない**（B25 / #982 の「過大にも過小にも申告しない」） |
| プロセス一覧と親子 | `proc_listallpids` + `PROC_PIDT_SHORTBSDINFO`（#1979 で root でも取れると確認済み） | `NtQuerySystemInformation(SystemProcessInformation)` 1 回で pid・親・作業セット・private bytes・CPU 時間。落ち先は既存の Toolhelp（B5 `procinfo::snapshot`） |
| プロセスのメモリ / CPU | `proc_pid_rusage(RUSAGE_INFO_V4)` の `ri_phys_footprint` と CPU 時間（**× timebase**） | 上の 1 回に含まれる |
| argv / env | `KERN_PROCARGS2`（新しい pid だけ） | `NtQueryInformationProcess` の PEB 読み（新しい pid だけ。S8 で要否を判断） |
| 順番待ちの子の木 | 走査の親子表で子孫を合計（最大値 = 採取時点の最大） | **Job Object** に入れれば `PeakJobMemoryUsed` と CPU 合計が木ごと正確に取れる |

置き場は**新しい境界 B29 `tako_core::platform::resources`**（B28 まで使用済み）。B25 `sysload` は
「診断専用・製品の判断に使わない」と明記されているので流用せず、B25 はそのまま残す。
tako-core に置くのは、GUI が落ちているときに CLI が自分で機の状態を出せるようにするため（§4.6）。

### 2.2 採取の間隔とコスト

| 対象 | 間隔 | 1 コアに対するコスト |
|---|---|---|
| PC 全体 | 2 秒（常時。ステータスバーの区画が読む） | 0.0004% |
| プロセス走査 | 30 秒（誰も見ていない）/ 2 秒（resources ビューを表示中・順番待ちに実行中か待ちがある・CLI / MCP の status から 10 秒）。**S1 の実装で 5 秒 → 30 秒へ変えた**（隔離 GUI・debug の A/B で 5 秒ごとは 1 コアの +0.57% = 走査 1 回 22〜24 ms。見ていない間に要るのは CPU% の差分の基準だけ） | 0.02% / 0.26%（C の実測からの見込み） |
| argv / env | 新しい pid だけ | 1 回 約 11 µs × 新規数 |
| 器のペインの pid（`backend_pane_pids` = `tmux list-panes -a`） | 10 秒（既存の上限つきの子プロセス。背景で） | 子プロセス 1 本 / 10 秒 |

- **採るのは背景スレッドだけ**。UI はスナップショット（`Arc`）を読むだけで、走査も子プロセスも起こさない
  （NFR-8・#1968 / #1979 / #1984 / #1989 の番犬の対象に入れる）
- 既存の走査（worker の状態照会・stale binary・ssh の宛先）が持つ `capture_process_table` とは**当面は別**に
  持つ。寄せると既存の経路の時間の性質が変わるので、寄せるのは S1 の後の別 Issue（§13）
- S1 の受け入れで、tako-app のアイドル CPU が増えた量を `proc_pid_rusage` で前後比較して出す（#1001 の予算）

### 2.3 帰属（誰の処理か）

1. **ペインの祖先**: 直接ペインは `TerminalSession::child_pid`、器のペインは `backend_pane_pids` の `pane_pid`。
   親子表を上へ辿って当たったペイン（既存の `find_ancestor_pane` と同じ形。`agents.rs:74`）
2. **env の `TAKO_PANE_ID`**: launchd へ付け替わった子（`nohup` や dev サーバーのデーモン化）は祖先で辿れないので、
   新しい pid の `KERN_PROCARGS2` から読む（実測で自分のプロセス 994 本中 408 本が持っていた）
3. どちらも無ければ **「tako の外」**。祖先にあるアプリ名（Terminal / iTerm2 / Code など）を添える

ペインからは既存の情報で「誰」を組む: タブ名・ペイン名・役割（`orchestrator-master` / `orchestrator-worker:` /
solo / 素のシェル）・worker なら project と label（`TAKO_ORCHESTRATOR_ROLE` と同じ材料）。

### 2.4 分類（何の処理か）

表は tako-core の純関数 1 つ（両 OS で単体テストできる）:

| 区分 | 判定 |
|---|---|
| agent（常駐） | 既存の agent 判定（claude / codex / agy）と、その子の MCP サーバー・`tako mcp serve` |
| build（重い） | `cargo` / `rustc` / `clippy-driver` / `rustdoc` / `ld` 系 / `target/**/deps/*-<hash>`（テストの実行体）/ `node` の argv に npm・npx・pnpm・yarn・vite・webpack・tsc・esbuild・jest・vitest / `xcodebuild` / `swift-frontend` |
| browser（重い） | playwright が起こす `chrome-headless-shell` / Chromium |
| gui（重い） | 隔離の `tako-app`（env `TAKO_ISOLATED=1`） |
| その他 | 上に当たらないもの（表示はするが「重い処理」として数えない） |

「重い処理 1 件」は**分類に当たったプロセスのうち一番上の祖先**（例: `cargo` の下の `rustc` 群はまとめて 1 件）。
メモリはその木の合計、CPU% は木の CPU 時間の差分 ÷ 経過。

## 3. 「重い処理」の捉え方（論点 2）

| 案 | 待たせられるか | 漏れ | AI / 人の手間 | 評価 |
|---|---|---|---|---|
| A. 明示の入口 `tako queue <cmd>` | **始まる前に待たせられる** | 入口を通らない処理 | 打つ言葉が 1 語増える（AGENTS.md と scripts が通すので AI の手間は無い） | **採る**（順番待ちの唯一の入口） |
| B. 自動検知（観測だけ） | 待たせられない（もう走っている） | 無い（tako の外も見える） | 無し | **採る**（見える化と枠の勘定） |
| C. ペイン単位 | 待たせられない | 粗すぎる（worker のペイン = claude + 数十のコマンド） | 無し | 採らない |
| D. jobserver の注入（cargo の札） | 並列度だけ絞れる | cargo 以外・メモリ | 無し | **採る**（S6。§1.6 で実測） |
| E. Claude Code の PreToolUse フックで重いコマンドを書き換える | 待たせられる | claude 以外 | 無し | 判断点（§11-6）。既定では採らない |
| F. PATH の shim（`cargo` を差し替える） | 待たせられる | rc が `~/.cargo/bin` を前へ足すと素通り・Claude の shell スナップショット・rustup の代理 | 無し | 採らない（壊れ方が見えない） |
| G. 走り出した後の一時停止（SIGSTOP / SIGCONT） | 後から止められる | — | 無し | 採らない（メモリは減らない・Bash の時間切れで AI が再試行して悪化・cargo のロックを持ったまま止めると優先度の逆転・Windows に無い） |

推奨は **B（常時）+ A（入口）+ D（S6）**。A を通らない処理も B で見えて枠に数えるので、
「入口を通らなかった分だけ他が待つ」になり、全体の上限は守られる（§4.3 の「外の重い処理も 1 枠」）。

**人の自然な経路**: `tako queue cargo build`（最簡。オプション無しで役割と優先度は自動で決まる）。
Code Runner（`tako run`）のビルドを自動で通すのは S5 の後の検討に回す（実行中のプログラムまで枠を占めるため）。

## 4. ゲート = 順番待ち（論点 3）

### 4.1 形

```
tako queue cargo test -p tako-core
  │  ① Enqueue（誰・何・優先度・pid・cwd の target dir・区分）→ 受付番号と順位
  │  ② Poll を 1 秒ごと（心拍を兼ねる）→ waiting{順位, 見込み, 理由} / granted{env}
  │  ③ 子を起こす（env: TAKO_QUEUE_JOB=<id>、CARGO_BUILD_JOBS / RUST_TEST_THREADS が未設定なら取り分）
  │     子は ① の前に PATH で引いた絶対パスで起こす（argv[0] は打った形のまま。#2538）
  │  ④ Started{子の pid} → 以後 2 秒ごとに心拍
  │  ⑤ Done{終了コード, 経過, 最大メモリ} → 過去の記録へ。自分の終了コードは子と同じ
  ▼
tako-control の調停役（シングルトン。tako-app の起動時に立つ）
  - ipc::submit が UI へ渡す前に Request::Queue を答える（先例: 非同期 run の分岐 = ipc.rs:76）
  - HTTP MCP も同じ submit を通るので同じ答え
  - 判断は tako-core の純関数 admit(機の状態, 実行中, 待ち, 上限) → 割り当て
```

- **UI スレッドを通らない**ので、#1979 型の UI の固まりがビルドの割り当てを止めない
- 調停役は**本番の 1 インスタンスだけ**。`TAKO_ISOLATED=1` の隔離 GUI は調停しない
  （隔離ごとに別の待ち行列ができると「全体で絞る」にならない）。テストは `TAKO_QUEUE_SOCKET` で
  隔離の調停役を明示して立てる（`TAKO_DATA_DIR` だけでは本番に当たる罠 = 既存の隔離検証の教訓と同じ）
- **入れ子は待たない**: `TAKO_QUEUE_JOB` がある中での `tako queue` はその枠の中で即実行
  （`tako queue bash scripts/test-x.sh` の中の `tako queue cargo build` が自分を待って固まるのを防ぐ）

### 4.2 待ち行列

| 項目 | 推奨 |
|---|---|
| 並び | 実効優先度（§5）の高い順 → 受付の古い順。枠・メモリで入れない件があれば後ろも待つ（資源の追い越し無し = 予測しやすい）。小さい処理の追い越しは S4 の後に要否を判断。**target dir だけで入れない件は後ろに追い越させる**（#2243。§4.8） |
| 待ちの上限 | 既定 30 分（`--max-wait`）。切れたら終了コード 75（EX_TEMPFAIL）と「N 件待ち・空いたらもう一度」の 1 行 |
| 待ちの表示 | 受付直後に 1 行（順位・見込み・待つ理由）。TTY なら同じ行を書き換え、非 TTY は 60 秒ごとに 1 行（AI の背景実行の出力で読める） |
| Ctrl+C / 親が殺した | 待ちなら外れて 130 で終わる。走っていたら子へ同じシグナル。wrapper が消えたら次の tick で枠を返す（pid の生存で判定） |
| tako の再起動 | **またぐ**。正本は各 wrapper。調停役へ繋がらなくなったら 1 → 5 秒で繋ぎ直し、待ちは元の受付時刻で、実行中は子の pid と開始時刻で登録し直す。調停役は心拍から表を組み直す。ディスクには持たない |
| tako が居ない | 受付の時点で繋がらなければ**待たずに走る**（1 行だけ知らせる。終了コードは変えない）。待ちの途中で切れたら 60 秒だけ繋ぎ直しを待ってから走る |
| 止まった処理 | 実行中で木の CPU が 10 分続けて 1% 未満なら「停滞」。**枠は返す**（同時数に数えない。メモリは機の値に効くので勝手に増えない）。殺さない。CPU が戻れば数え直す |
| 取り消し | 待ち = 外して wrapper は 125 で終わる。実行中 = 子のプロセスグループへ SIGTERM → 10 秒で SIGKILL（Windows は `TerminateJobObject`）。GUI は確認つき、CLI / MCP は呼んだこと自体を同意とみなす（`tako_background_kill` と同じ） |
| 同じ target dir | **同時に 1 件**。cargo は build directory のロックで待つので、2 件目が枠を持ったまま `Blocking waiting for file lock` で寝るのを防ぐ |

### 4.3 割り当ての条件（純関数 `admit`）

既定値（`settings.json` の `resources` 節と `tako queue limits` で変えられる。serde の default で旧ファイルはそのまま読める）:

| 条件 | 既定 | 根拠 |
|---|---|---|
| 同時に走る重い処理（`max_jobs`） | `clamp(floor(RAM_GB / 12), 1, 4)` → 36 GB で 3・16 GB で 1・64 GB で 4 | **粗い上限**で、実際の歯止めは次の 3 行のメモリの条件。RAM 12 GB あたり 1 件は「build の見込み 3 GB + 予約 + worker の常駐（25 本で約 15 GB。§1.3）」を 36 GB に収める目安 |
| 空きメモリの予約（`reserve_gb`） | `max(2, RAM × 10%)` → 36 GB で 3.6 GB | 空き 6.1〜8.1 GB の運転で、もう 1 件入れて予約が残るか |
| 1 件の見込みメモリ | 過去の記録（同じコマンドの直近 5 回の最大 → 同じ形の直近 10 回の最大 = #2254 の §4.9）。無ければ区分の既定（build 3.5 GB・test 7 GB・browser 2 GB・node 1 GB・gui 1 GB・その他 1 GB） | 区分の既定は S2 で tako 自身の `cargo build` / `cargo test --workspace` の木の最大メモリを実測して決め直した（§4.7）。記録は #2254 で入った |
| メモリ圧 | normal = 上の条件どおり / **warn = 実行中が 0 件のときだけ 1 件** / critical = 新規を止める（走っているものは止めない） | §1.4: warn は普段の 42%。止めると何も走らない |
| swap | 使用 2 GB 以上で warn 扱い | 事故は swap 6.8 GB・8.4 GB。普段は 0 |
| tako の外の重い処理 | **1 件 = 1 枠として数える**（判断点 §11-4） | 数えないと外のビルド + 3 件で溢れる |
| 最低限の前進 | 管理下の実行中が 0 件で critical でなければ、メモリの条件が欠けても先頭を入れる | 外の負荷だけで永久に待たないため |

取り分: 割り当てた処理には `CARGO_BUILD_JOBS = max(1, 論理 CPU ÷ 実行中の数)` と同じ値の `RUST_TEST_THREADS` を、
**利用者が既に設定していなければ**渡す（手で頼んでいた「`CARGO_BUILD_JOBS=2 … --test-threads=2`」の置き換え）。

### 4.4 tako の外から起きた重い処理

別の端末・launchd（夜間リリース）・CI のローカル実行など。§2.3 で「tako の外」に帰属し、
**見える・枠に数える・操作はしない**（S1〜S5 では kill も出さない。要るなら FR-2.19.2 と同じ確認つきの kill を後で）。
`scripts/` の入口は tako が居れば `tako queue` を通る（§9.3）ので、夜間リリースも `low` で並ぶ。

### 4.5 失敗時

| 事象 | 振る舞い |
|---|---|
| 子が起動できない | wrapper は 127（`sh` と同じ）と理由を出し、枠を返す |
| 調停役の判断が例外 | その回は割り当てない（待ちのまま）。次の tick で再判断。persist.log へ 1 行 |
| 機の値が採れない | 同時数の条件だけで判断（メモリの条件を飛ばす）。status に `machine: unavailable` と理由 |
| 札の数え違い（S6） | 管理下の cargo が 0 本になった時点で札を N 枚へ戻す（§9.4） |

### 4.6 GUI が居ないときの読み取り

`tako queue`（status）は GUI に繋がらなければ、CLI 自身が B29 で機の状態だけ採って出し、
「順番待ちは無効（tako が起動していない）」と 1 行添える。

### 4.7 S2（#1994）の実装で決めたこと

設計どおりに入れたうえで、実装の途中で決めた差分と細部（正は FR-2.43.6〜.10 とコード）:

| 項目 | 決めたこと | 理由 |
|---|---|---|
| 受付番号 | **wrapper の pid** | 調停役の世代をまたいでも番号が衝突しない（調停役が振る連番だと、立て直した後の新しい受付と、戻ってきた古い待ちが同じ番号を持ちうる）。`ps` で見える |
| Enqueue / Poll / Started | **1 つの upsert（poll）に寄せた**（action の名前は 3 つとも受ける）。毎回 1 件の全体（受付時刻・区分・target dir・子の pid）を送る | 正本を wrapper に置く形がそのまま書ける。調停役の立て直しは「知らない件の poll を受けたら登録する」だけ |
| 立て直しの順番 | 立ち上げ直後 **6 秒は誰も入れない**（`regather`）。wrapper の繋ぎ直しは 1 → 5 秒 | 後から戻ってきた古い待ちが、先に戻った新しい待ちに追い越されない |
| tako の外の重い処理 | **木の CPU が 10% 以上のときだけ 1 枠**（差分がまだ無い件は数える） | 休んでいる dev サーバー（vite / next）や watch（tsc --watch）が常に枠を食う。管理下の処理の「停滞は枠を返す」と同じ考え方 |
| 木の集計 | wrapper の pid の子孫 + env の `TAKO_QUEUE_JOB` が同じプロセス（とその子孫） | launchd へ付け替わった子は env で拾う。`/bin/sleep` などのプラットフォームバイナリは env が読めない（§1.3 の S1 の実測）ので子孫でも拾う |
| 調停役の隔離 | `TAKO_QUEUE_SOCKET` を明示したら、wrapper は**そこだけ**へ繋ぐ（本番へ漏れない）。GUI 無しの調停役は隠しコマンド `tako queue serve`（`TAKO_QUEUE_SOCKET` と `TAKO_TOKEN` が要る） | e2e を GUI 無しで回せる（落として立て直すのが軽い）。GUI に付けても同じ（その GUI が隔離の調停役になる） |
| 機の値の注入 | `TAKO_QUEUE_INJECT=<JSON>`（全体・空き・メモリ圧・swap・コア数・外の件数・停滞の窓・立ち上げ直後の窓）。**`TAKO_QUEUE_SOCKET` で立てた調停役だけ**が読む | 負荷を生成せずに条件の境界を e2e で踏める。本番は読まない |
| UI が固まった状態 | GUI 無しの調停役の `TAKO_1994_INJECT=ui-stall`（UI の代役が答えない）+ 統合テスト `issue1994_queue_ui_stall`（本物の IPC サーバーで受け口を誰も読まない） | 「`ipc::submit` の手前で答える」が効いていることを、`tako list` が返らないのに割り当てが進むことで見る |
| 取り分 | 割り当てた時点の「実行中（停滞を除く）+ 今回の分」で割る。走り出した後は変えない | 設計どおり。走っている cargo の並列は外から変えられない（S6 の jobserver で全体を絞る） |
| 待ちの表示 | 即座に入れたときは**何も出さない**。待ったときだけ開始の 1 行（待ち時間・取り分） | AI の出力に余計な行を混ぜない |
| 夜間リリース | env `TAKO_QUEUE_PRIORITY=low` と `TAKO_QUEUE_MAX_WAIT=3h`（中の build-app の run_heavy が拾う） | 夜は worker が走っていて 30 分を超えて待ちうる。超えると夜間リリースが 75 で落ちる |
| 区分の見込みメモリ | **build 3.5 GB・test 7 GB**（test を区分に足した）・browser 2 GB・その他 1 GB | 下の実測。テストは実行体の並走でビルドの約 2 倍食う |
| 旧版の tako | 順番待ちを知らない GUI は poll を「未知の action」で断る → wrapper は**待たずに走る**。`run_heavy` は `tako queue --help` に `--max-wait` が無ければ素の実行 | 着地から本番の入れ替えまでの間も、新しい CLI / scripts が壊れない |

**区分の見込みメモリの実測**（2026-10-15。§4.3 の仮置き build 3 GB の決め直し）: GUI 無しの隔離の調停役
（`TAKO_QUEUE_SOCKET`。機の値は実機・注入はコア数 6 だけ = 取り分 6 は 18 コアで 3 件並走のときと同じ）越しに走らせ、
調停役の走査（2 秒ごと）が採った木の物理フットプリントの最大（`recent[].peak_mb`）を読んだ。

```
tako-core を touch した後の cargo build --workspace         236 s  peak 3,430 MB  → build 3.5 GB
cargo test --workspace --no-fail-fast -- --test-threads=4    862 s  peak 6,834 MB  → test 7 GB
```

2 秒ごとの採取なので、それより短い山は取りこぼす（見込みは 0.5 GB 単位へ切り上げた）。冷えた target からの
フルビルドは測っていない（取り分 6 で rustc の同時数は抑えられるので、山を決めるのは tako-app の本体と link）。

### 4.8 target dir 待ちの追い越し（#2243）

S2 の「先頭が入れなければ後ろも待つ」は、先頭が**同じ target dir の処理が走っている**で待つ間、別の target dir の後ろの処理まで
空き枠の前で待たせた（2026-10-17: master のレビューの `cargo test` が「待ち 2 番目（実行 1/3）— 前に 1 件（先頭: 同じ target dir を
使う処理が走っている）」で約 5 分、#2230 の worker が約 6 分）。target dir 待ちの件はその dir が空くまでどのみち走れないので、
その間の空き枠は後ろに使わせる。

| 項目 | 決めたこと | 理由 |
|---|---|---|
| 追い越してよい相手 | **§4.3 の 1（target dir）だけで入れない件**。2〜6（メモリ圧・同時数・空きメモリ）で入れない件で並びは止まる | 資源の追い越しは S4 の後に判断（§4.2）。target dir 待ちは資源を待っていない |
| 追い越す側 | 自分で §4.3 の 1〜6 をすべて通る（同じ回に入れた分も実行中に数える） | 枠・メモリの約束は追い越しでも崩さない |
| 同じ target dir | 追い越された件と同じ target dir の後ろの件は追い越さない | 同じ dir の中は受付の順に 1 件ずつ（cargo のロック待ちを避ける直列を保つ） |
| 上限 | 1 件の待ちを追い越せるのは**累計 同時数 × 2 件**（`max_overtaken`。36 GB の機で 6 件）。達したらその件で止まる（= S2 と同じく後ろも待つ） | 下の「飢えない」で上限が無くても飢えはしないが、メモリの山が先頭の前に積み上がり続ける形を数で打ち切る。2 巡は「長い cargo test（約 14 分）の間に 8 本前後の worker の短い処理が流れる」を止めない数 |
| 飢えない | dir が空いた時点で先頭は入るか 2〜6 で止まる → 後ろはもう追い越せないので、空いた後に先頭より先に入る処理は無い。先頭は dir の持ち主の枠を引き継げる（持ち主が終われば同時数は 1 空く） | 追い越しで走り出した処理は、先頭が走れない間の空き枠を使うだけ |
| 数の持ち方 | 追い越された件数は調停役のメモリ（立て直すと 0 から）。`JobWire` は変えない | 立て直しは 6 秒の regather で並びを組み直す。数え直しで先頭が損をするのは立て直しの直後だけ |
| 待ちの表示 | 理由は件ごと: status の `waiting[]` に `reason` / `text` / `overtaken`。wrapper の 1 行は自分の理由（枠・メモリ…）、並びが止まって調べなかった件だけ「前に N 件（先頭: 止めている理由）」。上限で止まったら `overtake_limit` | 「先頭: 同じ target dir …」だけでは、後ろが追い越せる先頭の後ろで待っているのか、枠を待っているのか分からない |
| A/B | 調停役のプロセスの env `TAKO_2243_LEGACY=1` で S2 の形（`AdmitInput.overtake = false`） | 同じバイナリで修正前を再現（`scripts/test-queue-head-of-line-2243.sh`） |

### 4.9 実行中の予約と過去の記録の見込み（#2254）

2026-10-17 の昼、先頭が「空きメモリ不足（空き 7〜10 GB・必要 13.7〜13.9 GB・予約 3.6 GB）」で 30 分待ち、実行 1/3 のまま
待ちの 7 件が順に 30 分で諦めた（終了コード 75。全 worker のビルド・テストが止まった）。「必要」は**走っている
`cargo test --workspace` のまだ使っていない見込み 6.9 GB + 先頭の見込み 7 GB**。§4.3 の 6 は「見込み − 今の使用量」を
実行中の最後まで押さえていたので、ピーク（最大 7.3 GB）を過ぎて数百 MB しか使っていない処理が 7 GB 近くを抱え続け、
先頭の小さい `cargo test --test …` にも test の既定 7 GB が当てはめられていた（同種の実測の最大は 1.1〜7.5 GB）。

| 項目 | 決めたこと | 理由 |
|---|---|---|
| まだ使っていない見込み | **走り出してからピークまで**: ピーク前は「見込み − 今の使用量」、ピークを過ぎたら 0（`RunningJob::pending_mb`） | §4.3 の 6 の狙いは「走り出したばかりでまだ食っていない分」。空きは実測なので、ピーク後に実際に使っている分は空きから既に減っている |
| ピークを過ぎた | ①木の最大が見込みに届いた、または ②過去の記録の山の時刻（同じコマンド / 形の記録の中の最大）+ 60 秒を過ぎ、自分の最大の更新からも 60 秒たった（`peak_passed`） | ①は本番の形（7.3 GB ≥ 7 GB）。②は今回の山が記録より小さく終わった回（差分ビルドの `cargo test --workspace`）。記録が無ければ時間では過ぎない（初回は安全側）。60 秒は 2 秒ごとの走査の取りこぼしと回ごとの山の時刻の揺れを覆う |
| 見込み | 同じコマンドの直近 5 回の最大 → 無ければ同じ形の直近 10 回の最大（2 回以上）→ 区分の既定。128 MB 刻みへ切り上げ・下限 256 MB（`queue::estimate`） | 最大を採るのは安全側（失敗で早く終わった回が混ざっても下がらない）。形は 1 回だけだと形の中の軽い 1 本に引きずられる |
| 鍵 | 同じコマンド = 「repo の根 + 正規化した argv（前置きの env / nice / time を飛ばし、プログラムは basename）」の FNV-1a 64bit。git の worktree（`.git` がファイルで `gitdir: <本体>/.git/worktrees/<名前>`）は**本体の根へ寄せる**。同じ形 = cargo だけ、`--test` / `--bench` / `--example` / `--bin` の値・`--target-dir` / `--manifest-path` / `--config` の値・2 語目以降の位置引数（テストの絞り込み）・`--` の後ろの位置引数を `*` にした argv の同じハッシュ。**`-p` の値は伏せない** | worker は Issue ごとに worktree を作るので、根のパスのままだと記録が引き継がれない。Issue ごとに変わる `--test <名前>` は形でまとめ、パッケージで重さが大きく違う `-p tako-app` と `-p tako-cli` は分ける。シェルスクリプト・node は中身で重さが違うので形でまとめない |
| 記録 | `<data_dir>/resources/history.jsonl`（§6 と同じ置き場。#1996 の所要時間もここへ足す）。終わった処理 1 件 = 1 行（`at` / `key` / `shape` / `name` / `kind` / `peak_mb` / `peak_at_s` / `ran_s` / `exit_code`）。512 KB で `history.1.jsonl` へ回す。読めない行は飛ばす。共有しない（`config_share::catalog` の `resources/` = Local） | **生の argv とパスは書かない**（ハッシュと表示名 `cargo test` だけ。番犬で検査）。追記だけのファイルなので版数の番地は持たず、行の欄は `migration_registry` の指紋で固定（欄を足すなら serde の default） |
| 諦めた件の表示 | wrapper は待ちの上限で外れるときに `done` へ 75 を載せ、最後の poll の `holders`（実行中・割り当て済みの使用・最大・まだ使っていない見込み・見込みとその出どころ・ピーク後・停滞）を stderr に出す。調停役は `recent` に `gave_up` / `reason` / `text` / `holders` を残す（CLI `tako queue` の「直近の諦め」・MCP `tako_queue`） | 「何がどれだけ押さえていたか」が分からないと、30 分の詰まりの原因を追えない（今回も #2243 の worker の実測まで分からなかった） |
| 歯止め | §4.3 の 2・4・6（メモリ圧 warn / critical・swap・予約・空きの実測）は変えない | 安全側を崩さない。本当に空きが無いときは今まで通り待つ |
| A/B | 調停役のプロセスの env `TAKO_2254_LEGACY=1` で修正前（区分の既定だけ・最後まで押さえる）。記録は修正前でも足す | 同じバイナリで修正前を再現（`scripts/test-queue-reservation-2254.sh`。注入 `TAKO_QUEUE_INJECT` の `jobs` で実行中の木の使用量・最大を与え、負荷は生成しない） |

## 5. 優先度（論点 4）

| 段 | 既定で付くもの |
|---|---|
| `top` | 人が直接（agent の居ない素のシェルのペインで、TTY から打った） |
| `high` | master / solo |
| `normal` | worker |
| `low` | ペインの外（launchd の夜間リリース・別の端末・CI スクリプト） |

- **付ける場所**: 処理（`--priority` / 後からの変更）> ペイン（そのペインから出る処理の既定。layout.json の
  ペインに serde default のフィールドで持つ = 器のペインが GUI の再起動をまたいでも残る）> 役割の既定
- **変え方**: `tako queue priority <受付番号 | --pane N> <top|high|normal|low>`・MCP `tako_queue` の `priority`・
  resources ビューの行のチップ。master は「merge を塞いでいる worker」を上げられる
- **エイジング**: 待ち 15 分ごとに 1 段上げる（`high` まで。`top` へは自動で上げない）= 飢餓を防ぐ
- **横取りはしない**: 走っている処理を止めて上位を入れることはしない（§3 の G を採らない理由と同じ）。
  優先度が効くのは「次に誰を入れるか」だけ

## 6. 終わりの見込み（論点 5）

| 材料 | 取り方 | 使い方 |
|---|---|---|
| cargo の `N/M` | wrapper の stderr が TTY でないときだけ `CARGO_TERM_PROGRESS_WHEN=always` と `WIDTH` を足し、`Building [...] N/M` の行を**自分で読んで素通しから外す**（AI の出力に混ぜない） | 進みのバー。履歴が無く `N/M ≥ 0.2` のときだけ線形で見込みを出す |
| 過去の所要時間 | `<data_dir>/resources/history.jsonl`（512 KB で回す）。鍵は「repo の根 + 正規化した argv」のハッシュ、表示名はプログラム + サブコマンド（`cargo test`）。経過・最大メモリ・`M`・終了コード | 同じ鍵が 2 回以上あれば「残り = 中央値 − 経過」 |
| TTY のとき | cargo 自身のバーを人がそのまま見る（wrapper は横取りしない） | 履歴だけで見込み |

- **cargo のユニットは大きさが偏る**（最後の tako-app 本体と link が重い）ので、`N/M` だけの線形は楽観に倒れる。
  履歴を優先する
- 表示: 「あと約 3 分（前回 7 分）」/ 履歴を超えたら「前回より長くかかっている（経過 9 分）」/
  材料が無ければ「経過 4 分（見込みなし）」
- 待ちの見込み: 前に並ぶ全件に見込みがあるときだけ「開始まで約 N 分」、無ければ順位だけ
- 履歴に**生の argv とパスは書かない**（ハッシュと表示名だけ）。ペインの内容も持たない。置き場と行（最大メモリ・
  山の時刻・所要時間・終了コード）は #2254 で入った（§4.9）。追記だけの共有しない Local なので、`usage/` の行（#2200）と
  同じく版数の番地は持たず、行の欄を `migration_registry` の指紋で固定する（#1996 で欄を足すなら serde の default）

## 7. 見せ方（論点 6）

| 場所 | 中身 | スライス |
|---|---|---|
| ステータスバーの 1 区画 | メモリの使用バー（色はメモリ圧: normal = 通常 / warn = 黄 / critical = 赤。GPUI の矩形で描く・絵文字なし）+ 実行中・待ちが 1 件以上あるときだけ「実行 2 · 待ち 3」。押すと右パネルの resources ビュー | S3 |
| 右パネルの新ビュー `resources` | ① 機: メモリ（使用 / 全体・圧縮・swap・メモリ圧）・CPU%・load（macOS のみ）② 実行中: 誰（タブ · ペイン · 役割）/ 何（`cargo test -p tako-core`）/ CPU% / メモリ / 進み / 見込み / 優先度チップ / 取り消し（確認つき）③ 待ち: 順位 / 誰 / 何 / 待ち時間 / 優先度 / 取り消し ④ 常駐の内訳: ペインごと（agent 本体 + MCP サーバー + dev サーバー）の上位と「tako の外」のまとめ | S3（②③の進み・見込み・チップは S4） |
| fleet ビューの worker の行 | その worker の常駐メモリ（子孫込み）を 1 つ | S5 |
| スマホ（PWA） | `#/` に機の状態のカード、`#/resources` に実行中と待ち。優先度の変更と取り消しは manage 以上 | S7 |

- 既存のビュー（fleet / orch / git / tasks / diagnostics）を拡張せず新ビューにする理由: fleet は「タブとペインの入れ子」、
  diagnostics は LSP の診断（名前が埋まっている）で、機の状態と待ち行列はどちらの軸にも乗らない
- UI は**スナップショットを読むだけ**（§2.2）。行を押したら該当ペインへフォーカス（dispatch の `Focus`）
- `tako check-health` には足さない（環境の健全性 = IPC の受け口・PATH・tmux の診断で、時々刻々の負荷とは別の軸）

## 8. CLI / MCP（論点 7）

### 8.1 CLI

| 操作 | コマンド |
|---|---|
| 状況（機 + 実行中 + 待ち + 常駐の上位） | `tako queue`（`--json`） |
| 順番待ちで走らせる | `tako queue cargo build --release`（clap の external subcommand。走らせるコマンドの名前が `priority` / `cancel` / `limits` と同じときだけ `tako queue -- <cmd>`） |
| 優先度 | `tako queue priority <id \| --pane N> <top\|high\|normal\|low>` |
| 取り消し | `tako queue cancel <id>` |
| 上限 | `tako queue limits [--max-jobs N] [--reserve-gb X] [--swap-guard-gb Y] [--max-wait-min M] [--reset]` |

順番待ちで走らせるときのオプション（`--priority` / `--max-wait` / `--label`）は**普段は付けない**（#322）。

### 8.2 MCP

**新設 1 本 `tako_queue`**（`action`: `status`（既定）/ `priority` / `cancel` / `limits`）。

- 大きさの見込み 約 1.8 KB（似た `tako_limit_resume` 1,831 B・`tako_sleep_guard` 2,234 B）。**上限 2.0 KB を S1 の受け入れに入れる**。
  カタログは 192,549 → 約 194.4 KB（残り 約 10.4 KB）
- `tako_panel` の `view` に `resources` を 1 つ足す（+ 十数 B）
- 既存ツールの**応答**に足すもの（カタログは増えない）: `tako_orchestrator_spawn` / `tako_orchestrator_run` に
  `machine` と `advice`（S5）、`tako_orchestrator_workers` に worker ごとの常駐メモリ（S5）
- **順番待ちで走らせる操作は CLI だけ**: 待ち行列に入るのは呼び手のシェルの子プロセスで、MCP の呼び出しには
  包む対象が無い。UI にも「走らせる」操作は無いので開発不変条件（UI でできることは AI からも）には反しない。
  この例外と理由を requirements の FR へ書く（S2）
- 既存ツールへの相乗り（`tako_check_health` に `resources` を足す）は採らない: 健全性の診断と負荷の操作は軸が違い、
  `priority` / `cancel` の行き場が無い（判断点 §11-2）

### 8.3 dispatch

`Request::Queue { action }`。`Status` / `Priority` / `Cancel` / `Limits` は CLI / MCP / GUI の 3 経路が同じ口、
`Enqueue` / `Poll` / `Started` / `Done` は `tako queue` の wrapper 専用（MCP のカタログには出さない）。
すべて `ipc::submit` の手前で答える（§4.1）。

## 9. AI の運用への組み込み（論点 8）

### 9.1 master の spawn 判断

- **#2413 で改めた**: 逼迫しているあいだの spawn / run は**待たせる**（§9.6）。以前の判断（spawn は止めない・助言だけ）は
  10/25 の停止で効かないと分かった（助言 #1997 も無いまま agent が 40 本まで増えた）。「止めると master が新しい失敗の形を
  覚える」懸念は、**失敗にせず待ちとして返す**（spawn は成功し、応答の `launch_gate` に理由と順番。圧が戻ると tako が起こす）
  ことで小さくした。master が覚えるのは「`launch_gate.state=waiting` は待っている・急ぐなら `tako queue start <ペイン>`」の 1 つ
- `machine` / `advice` を応答に載せる部分・fleet の常駐メモリ・guide の追記のうち、#2413 に含めなかったものは #1997 に残す
  （#2413 の着地後に #1997 の本文を整理する）

### 9.2 worker への指示

- guide `worker-prompt` の Constraints の雛形に 1 行:「ビルド・テスト・隔離 GUI は `tako queue` を通す（AGENTS.md の表どおり）。
  待ちが長くなりうるときは Bash の背景実行で」
- 引き継ぎの運用メモにある「`CARGO_BUILD_JOBS=2 … --test-threads=2` を毎回指示」は S2 の着地後に外す（取り分は tako が渡す）
- Claude Code の Bash ツールは既定 2 分・最大 10 分で切れる。待ちが長いと切られて再試行 = 悪化するので、
  背景実行（`run_in_background`）を推す。wrapper は受付直後に順位と見込みを 1 行出すので、切られても理由が transcript に残る

### 9.3 この repo の入口

- AGENTS.md のコマンド表の build / test / lint を `tako queue cargo …` へ（1 行 +11 B 程度。残り 2.6 KB）。`.agent/commands.md` の同じ行に理由
- `scripts/lib/queue.sh`（新設）の `run_heavy` を `scripts/build-app.sh`・`scripts/release.sh`・`scripts/nightly-release.sh`（`--priority low`）・
  `scripts/lib/isolated-gui.sh` のビルド・cargo を呼ぶ `scripts/test-*.sh` が通す。`tako` が無い（CI）か調停役が居なければそのまま走る
- 隔離 GUI を立てる検証スクリプトは**スクリプト全体を 1 件**として並ぶ（中の cargo は入れ子で即実行 = §4.1）

### 9.4 cargo の全体並列（S6）

- tako-app が `<data_dir>/resources/jobserver.fifo` を作って開いたまま持ち、札を `論理 CPU 数` 枚入れる。
  `spawn_session`（`crates/tako-app/src/main.rs:9377`）の env に `CARGO_MAKEFLAGS=-j<N> --jobserver-auth=fifo:<path>` を足す
- fifo の中身は**誰かが開いている間だけ**残る。GUI の再起動の間は走っている cargo が開いているので札は残り、
  起動時に札を足すと二重になる → 「管理下の cargo が 0 本のときに N 枚へ揃える」で数え違いを直す
- 器のペインは作った時の env を持ち続けるので、**パスは固定**（世代ごとに変えない）
- 順番待ちの取り分（`CARGO_BUILD_JOBS`）と重なっても厳しい方が効く（§1.6 で実測）ので、両方を入れて矛盾しない
- 既定 on（判断点 §11-5）・`settings.json` の `resources.jobserver` と A/B の `TAKO_1992_NO_JOBSERVER=1` で切れる

### 9.5 #1945（上限後の自動復帰）との関係

一括復帰で worker が一斉に動き出すと、その直後のビルドがまとめて来る。**復帰そのものは軽い**ので変えず、
ビルドは順番待ちが吸収する。S5 で「一括復帰の直後の待ち件数」を実測し、足りなければ復帰の時差を別 Issue にする。
#2413 でも一括復帰は待たせない（新しいプロセスを起こさない = 常駐の本数が増えない。§9.6 の表）。

### 9.6 agent の起動待ち（#2413）

**判断（2026-10-26 ユーザー）**: 逼迫時は起動を待たせる。メモリ圧が warn / critical のときは spawn・run・再起動後の復元を
待たせ、復元は時差で行う。止まっている worker は殺さない。

#### 形

| 項目 | 決めたこと | 理由 |
|---|---|---|
| 判断 | **重い処理と同じ `admit`**（`tako_core::queue::admit_launches` が入力を組んで呼ぶ）。違いは入力だけ: ①**最低限の前進なし**（`AdmitInput::min_progress = false`）②同時数の上限なし ③実行中 = 立ち上がり中の agent（まだ使っていない見込みを空きから引く）④復元は列（`RESTORE_LANE`）で 1 本ずつ | 方針を 2 か所に書かない。①は重い処理との唯一の本質的な違い: 重い処理は終われば空くが、常駐は終わらないので逼迫中に 1 本ずつでも足すと戻らない |
| 待つ条件 | メモリ圧 warn / critical・swap が警戒線（既定 2 GB）以上・空き − 立ち上がり中のまだ使っていない見込み − この agent の見込み < 予約（既定 max(2 GB, RAM の 10%)） | §4.3 の 2・4・6 と同じ段・同じ設定（`settings.json` の `resources`）。warn は普段の運転の 42%（§1.4）なので待ちは頻繁に起きうる = ユーザーの判断で受け入れた。#2412（worker を tako の MCP だけで起動）で 1 本の常駐が減ると warn の時間も減る |
| 置き場 | `tako_control::launch_gate`（プロセスに 1 つ）。入口は `request` で問い、待つなら**起こし方（起動コマンドと依頼文 / 復元の resume の行）を預けてすぐ答える**。GUI の 500 ms tick（`tick`）が覚え（機の値・ペインのメモリ）を読んで判断し、入れた件を**待たずに起こすときと同じ手順**で起こす | MCP の呼び出しを固めない（spawn は 1 秒以内に返る）。UI スレッドは覚えを読むだけ（番犬 `issue1993_resources_ui_thread_watchdog` の ⑨⑩） |
| 見せ方 | spawn / `sessions resume` / `git resolve` の応答に `launch_gate`（`state` waiting / starting・`code` / `reason`・`position` / `waiting`・`waited_s`・`start_now`）。worker の状態は `launch_waiting`（素のシェルのプロンプトを idle = 完了と読ませない・未達 / 突然死 / 起動失敗 / 停滞の事象を出さない）。`workers` の `launch` は `waiting` + `launch_gate`。`run_status` は `launch_waiting`、起こした後は `starting`（完了待ちのスレッドが読み直すまでの間も、正本の起動待ちが立ち上がり中と言えば起動待ちを残さない = #2426）。`tako queue` の `launches` 節（待ち・立ち上がり中）。persist.log に待ち・起動・上書き・閉じた件の 1 行 | master が待ちを追える（spawn の応答 → `workers` / `worker_status` / `run_status` / `tako_queue`）。新しい MCP ツールは足さない |
| 完了待ち | watch / run / supervisor の共通の `wait_for_worker` は `launch_waiting` の間は完了と読まず、**時間切れの時計を止める**（起きてから `timeout` を数える）。起きた直後は spawn 直後と同じ初期待ちを置く。spawn が待ちを返した run は初期待ちを**先に寝ず**、待っている間も間隔ごとに読み、起きたのを見たら進捗を `starting` へ直してから初期待ちを置く。待っている間に閉じられた（gone）は起きたと読まない（#2426） | 待った時間で run が timeout にならない。待ちは失敗ではない。先に寝ると、起きた後も閉じた後も初期待ちの 20 秒は進捗が起動待ちのまま残り、起きた直後の待ちも初期待ちの残りへ縮む（#2426 の e2e ④） |
| 送達の猶予 | 待っている間の `prompt_delivery` は Pending。起こしたら worker レジストリの `spawned_at` を起こした時刻へ直す | 猶予（240 秒）を待ちに使うと「依頼文が届いていない」→ supervisor の自動再送がシェルへ依頼文を撃つ |
| 時差 | 復元は列で 1 本ずつ。起こしてから**立ち上がり**（20 秒、または見込みに届くまで）を終えたら次。並びは master / solo（high）が先 → 受付の古い順（復元は全部を預けてから最初の tick で並べる = layout の並びの先頭が先に起きない）。**spawn は列の待ちを追い越せる**（列の待ちは段 1 だけで入れない件 = #2243 の追い越しの規則） | 10/26 9:12 は全セッションの一斉の復元で 8 分後に 29.6 / 36 GB・load 272。1 本ずつなら圧の変化が次の判断に間に合う。master が先なら人がすぐ指示を出せる。20 秒の根拠は下の実測（山は 2 秒。送達と、再起動直後のログインシェルの一斉の立ち上がりの遅れを見込んだ余裕）。30 本なら最長 10 分（見込みに届けば早く明ける・逼迫すればそこで止まる） |
| 見込み | 立ち上がりの最大（ペインの木のメモリ）を過去の記録（`<data_dir>/resources/history.jsonl` の区分 `agent`。鍵は系統の名前のハッシュ = 系統をまたいで借りない）へ足し、`queue::estimate` で直近 5 回の最大。無ければ 640 MB（§1.3 の claude + 子孫の RSS の中央値 596 MB を切り上げ） | 「agent 1 本の見込みは過去の実測から」（#2413）。#2412 で常駐が減れば記録が追従する |
| 上書き | CLI `tako queue start <ペイン>` / MCP `tako_queue` の `action: start`（`id` = ペイン）。次の tick（500 ms 以内）で起こす。人の GUI 操作で起こす入口（`PaneOrigin::User`）はそもそも待たせない | 新しいオプションを増やさず、既存の順番待ちの口へ 1 語足すだけ（#322）。spawn の応答の `start_now` がそのまま打てる形 |
| 有効な tako | 本番の GUI とセカンダリ（機の値はマシン全体なので同じ判断になる）。隔離 / セルフテストは注入 `TAKO_2413_INJECT`（機の値と立ち上がりの長さ。1 秒ごとに読み直す）があるときだけ。A/B は `TAKO_2413_LEGACY=1` | 機の実際の圧で検証用の GUI を固めない。逼迫は注入で再現する（負荷を生成しない） |
| Windows | 機の値が採れない（#2000 まで）ので逼迫の段は飛び、**時差（復元の列）だけ**が効く | `admit` の「機の値が採れなければ同時数だけ」と同じ扱い |
| 殺さない | 走っている agent・立ち上がり中の agent は止めない・殺さない。起こすと決めた件は、その後に圧が戻っても取り消さない | ユーザーの判断。決めた起動を取り消すと待ちの順番が揺れる |

#### 立ち上がりの実測（2026-10-26）

本物の claude 1 本を疑似端末で起こし（依頼文は送らない・tako の env は落とす）、子孫の RSS を 1 秒ごとに 45 秒:

```
stdio の MCP サーバー 7 本が子として起きる構成:  0.0s 1 本 0 MB → 1.1s 8 本 639 MB → 2.1s 714 MB → 以後 694〜718 MB で平ら
MCP の子が起きない構成:                          1.1s 2 本 353 MB → 以後 291〜357 MB で平ら
```

既定の見込み 640 MB（§1.3 の RSS の中央値 596 MB を切り上げ）は 2 つの間。見込みは過去の記録（立ち上がりの最大）で
系統ごとに置き換わる。#2412 で worker の MCP が tako だけになれば記録が下がる。

#### 入口の棚卸し

| 入口 | 待たせるか | 理由 |
|---|---|---|
| `tako_orchestrator_spawn` / `tako orchestrator spawn` | **待たせる** | 常駐が 1 本増える。10/25 の 40 本の主な増え方 |
| `tako_orchestrator_run`（MCP の非同期）/ `tako orchestrator run` | **待たせる**（中で spawn を通る） | 同上。run の時計は起きてから数える |
| GUI の起動時の会話の復元（#1076 / #1238） | **待たせる + 時差** | 再起動後の一斉の復元が 10/26 の再逼迫の原因 |
| `tako sessions resume`（#112 / #1069） | **待たせる** | 会話が 1 本増える（CLI / MCP だけの入口） |
| `tako git resolve`（#496）の CLI / MCP | **待たせる** | agent が 1 本増える |
| `tako git resolve` の GUI の操作 | 待たせない | 人が押した = 明示の起動（上書きと同じ扱い） |
| 一括の自動復帰（#1945）・上限後の自動復帰（#813）・ネット復帰の再開（#2112） | 待たせない | 新しいプロセスを起こさない（生きている agent へ続きを送るだけ = 常駐の本数は増えない） |
| 会話を引き継いだ再起動（#1067）・supervisor の突然死の resume（#390。既定は通知だけ） | 待たせない | 置き換え（前の agent が落ちてから起こす = 本数は増えない） |
| master の引き継ぎ（#193 / #749） | 待たせない | 置き換え（後任が立てば前任は役目を終える）。待たせると master が居ない時間ができる |
| `tako master` / `tako solo`（人が端末で打つ）・スマホの「+」（#1078 / #1449）・人の返答で起票 master を起こす（#2182） | 待たせない | 人が直接起こす = 明示の起動 |
| orphan の自動復帰（#191）・`tako tmux open` | 待たせない | 走っている器へ付け直すだけ（新しいプロセスを起こさない） |
| 自動命名（`claude -p`） | 待たせない | 常駐しない短命の 1 回 |

## 10. Windows と MATRIX（論点 9）

- **platform の対応マトリクス**（`crates/tako-core/src/platform/support.rs`。キーは MCP ツール名と 1:1）: `tako_queue` を足す。
  macOS = Supported（S1 / S2 の着地で）、Windows = Pending（S8 の Issue を追跡先に）。理由の文は日英で
  「Windows の計測（B29 の Windows 側）と Job Object での木の集計が未実装・未実測」
- 各スライスは Windows でもビルドが通る形で入れる（B29 の Windows 側は `None` を返し、`tako queue` は fail-open で
  そのまま走る = 壊れはしないが絞らない）。S8 で実装と実機の実測をまとめて行う
- Windows で変わるもの: load が無い（出さない）/ メモリ圧の合成（§2.1）/ 子の木は Job Object / 取り消しは `TerminateJobObject` /
  jobserver は fifo ではなく名前付きセマフォ（jobserver クレートの Windows 側。cargo が従うかは未実測 = S8）
- **agent 能力マトリクス**（`tako_core::agent_support::MATRIX`）: 1 行 `worker_heavy_queue_wait`
  「重い処理の順番待ちを、シェルの時間切れで切られずに待てる」を S5 で足す。claude = Supported（Bash の背景実行と最大 10 分）、
  codex / agy = Pending（シェルの時間切れと背景実行の有無が未実測）、local = 既存の `local_pending_first_class`。
  順番待ちそのもの（env と CLI）は `worker_cli_control` と同じ根拠で agent に依らないので、別の行は作らない

## 11. 判断点（master が決める。推奨を先頭に）

1. **CLI / MCP の名前**: `tako queue` / `tako_queue`（推奨。いちばん多く打つ「順番待ちで走らせる」が最短で読める）／
   `tako resources`（状況）+ `tako queue`（実行）の 2 語（利用者の言葉「リソース管理」に近いが入口が 2 つになる）
2. **MCP は新設 1 本**（推奨。約 1.8 KB）／ `tako_check_health` に相乗り（+ 数百 B で済むが優先度・取り消しの行き場が無い）
3. **メモリ圧 warn の扱い**: 1 件ずつに絞る（推奨。§1.4 の実測で普段の 42% が warn）／ 止める（何も走らなくなる）／ 無視（事故の手前で効かない）
4. **tako の外の重い処理を枠に数える**（推奨）／ 数えない（外のビルド中に tako 側が上限まで入れて溢れる）
5. **jobserver の注入（S6）を既定 on**（推奨。包まない cargo も絞れる。切り替えあり）／ opt-in（安全だが効く範囲が入口だけになる）。
   S6 は S1 だけに依存するので**前倒しもできる**
6. **Claude Code のフックで重いコマンドを自動で包む**: 採らない（推奨。AGENTS.md と guide で通る・黙ってコマンドを書き換えない）／
   tako mod の opt-in として後で足す
7. ~~**spawn は助言だけ**（推奨）／ 生きている agent の上限で止める（9/25 の 25 本は止められるが、master が新しい失敗の形を覚える）~~
   → **#2413 で改めた（2026-10-26 ユーザー判断）**: 逼迫しているあいだは spawn / run・再起動後の復元を待たせ、復元は時差にする。
   止めるのではなく**待ちとして成功を返す**（失敗の形は増えない）。上限は本数ではなくメモリ（§9.6）
8. **待ちの上限 30 分 + AI は背景実行**（推奨）／ AI のときだけ 8 分（Bash の 10 分に収まるが、ビルド自体が長いと結局切られる）
9. **停滞は枠を返して殺さない**（推奨）／ 一定時間で殺す（デバッガ待ち・入力待ちを壊す）
10. **tako が居なければ待たずに走る**（推奨）／ 待つ（tako の外・CI・GUI の再起動中にビルドが止まる）

## 12. スライスと子 Issue（論点 10）

正本は #1992 のコメント。並べる順は「落ちる原因を先に止める」: 見える（読み取り口）→ 順番待ち → 見せ方 → 細かい制御 → 運用 → 透過 → 他の画面 → Windows。

| 順 | 子 Issue | 中身 | 依存 | 規模 |
|---|---|---|---|---|
| S1 | #1993 | 計測の土台（B29・背景の採取・分類・帰属）と読み取り口（`tako queue` の状況 / MCP `tako_queue` の `status`） | なし | M |
| S2 | #1994 | 順番待ち本体（`tako queue <cmd>`・調停役・割り当ての条件・再起動をまたぐ・fail-open・取り分）+ この repo の入口の採用（✅ 実装の差分は §4.7） | S1 | L |
| S3 | #1995 | 右パネルの `resources` ビューとステータスバーの 1 区画 | S1（待ち行列の欄は S2） | M |
| S4 | #1996 | 優先度（4 段・変更・エイジング）と終わりの見込み（`N/M`・過去の所要時間） | S2 | M |
| S5 | #1997 | AI の運用（spawn の `machine` / `advice`・guide・fleet の列・agent 能力マトリクスの 1 行・一括復帰の実測） | S2 | M |
| S6 | #1998 | cargo の全体並列の上限（共有 jobserver の注入と札の数え直し） | S1 | M |
| S7 | #1999 | スマホ（PWA）の状況カードと `#/resources` | S3・S4 | S〜M |
| S8 | #2000 | Windows の実装と実機の実測（B29 の Windows 側・Job Object・メモリ圧の合成・jobserver のセマフォ）・MATRIX を Supported へ | S1・S2（S6） | M |

`progress.md` の衝突（#1228）があるので着地は 1 本ずつ直列。

## 13. 既存機能との関係（矛盾の確認）

| 既存 | 関係 |
|---|---|
| `tako task gate`（受け入れゲート #244） | 名前が紛れるので、この機能の CLI / 文書では「ゲート」と呼ばず「順番待ち」と呼ぶ |
| `tako run`（Code Runner）・`tako_orchestrator_run` | `queue` とは衝突しない。Code Runner のビルドを自動で通すかは S5 の後に検討（§3） |
| 右パネル（fleet / orch / git / tasks / diagnostics） | `resources` を 1 つ足す。fleet には worker の常駐メモリを 1 つ足すだけ（S5） |
| ステータスバー | 区画を 1 つ足す。既存の fleet（master + N workers）・ctx / 5h / 7d・スリープ防止とは別 |
| `tako check-health` | 足さない（§7） |
| #1968 / #1979 / #1984 / #1989（UI スレッドの待ち） | 採取は背景スレッドで libproc / sysctl だけ。tmux の pane pid は既存の上限つきの子プロセスを背景で。番犬の対象に入れる |
| `with_shared_scan` / `capture_process_table` | 当面は別に持つ。寄せる（worker の状態照会などが resources のスナップショットを読む）のは別 Issue（時間の性質が変わるため実測してから） |
| #1945（上限後の自動復帰） | 変えない。ビルドは順番待ちが吸収（§9.5）。起動待ち（#2413）でも待たせない（§9.6） |
| `tako orchestrator` の spawn / レジストリ | #2413: 逼迫中は起動を待たせる（ペイン・レジストリは今作り、起動コマンドと依頼文を預ける。§9.6）。`machine` / `advice` は #1997 |
| 再起動後の復元（#1076 / #1238） | #2413: 復元の列で 1 本ずつ・master が先・逼迫中は待つ（§9.6）。内訳の数え方（#1554）は変えない |
| `tako todo`（人がやること） | 使わない（負荷は一過性で、人に残すタスクではない） |
| #1001（tako 自身の軽量化） | 採取のコスト（§2.2）を S1 の受け入れで前後比較 |
| #990（ローカル LLM のリソース警告） | B29 のスナップショットを使い回せる |
| #1666（並行する cargo test の一時 dir の衝突） | 同じ target dir を 1 件にする条件（§4.2）で起きにくくはなるが、直すのは #1666 側 |
| 個人情報（#927） | 履歴は argv とパスをハッシュにする（§6）。画面と CLI に出す argv は走っているプロセスの生の値でディスクに残さない |

## 付録 A 計測に使ったコード（抜粋）

```c
// clang -O2 -o probe probe.c && ./probe 20   （macOS。読み取りのみ）
#include <libproc.h>
#include <sys/proc_info.h>
#include <sys/sysctl.h>
#include <mach/mach.h>
// PC 全体: 1,000 回の平均
size_t len = sizeof(memsize); sysctlbyname("hw.memsize", &memsize, &len, NULL, 0);
len = sizeof(sw);  sysctlbyname("vm.swapusage", &sw, &len, NULL, 0);
len = sizeof(lvl); sysctlbyname("kern.memorystatus_vm_pressure_level", &lvl, &len, NULL, 0);
getloadavg(la, 3);
cnt = HOST_VM_INFO64_COUNT;  host_statistics64(mach_host_self(), HOST_VM_INFO64, (host_info64_t)&vs, &cnt);
cnt = HOST_CPU_LOAD_INFO_COUNT; host_statistics(mach_host_self(), HOST_CPU_LOAD_INFO, (host_info_t)&cl, &cnt);
// 内訳（Activity Monitor と同じ式）
app = (vs.internal_page_count - vs.purgeable_count) * page;  wired = vs.wire_count * page;
compressed = vs.compressor_page_count * page;  available = memsize - (app + wired + compressed);
// 全プロセス: 20 回の平均
n = proc_listallpids(pids, sizeof(pids));
for (i = 0; i < n; i++) {
  proc_pidinfo(pids[i], PROC_PIDT_SHORTBSDINFO, 0, &sb, sizeof(sb));      // 親・名前・uid
  proc_pid_rusage(pids[i], RUSAGE_INFO_V4, (rusage_info_t *)&ri);           // ri_phys_footprint・CPU 時間
}
// CPU 時間は mach tick: 秒 = (ri_user_time + ri_system_time) * timebase.numer / timebase.denom / 1e9
```

cargo の jobserver の実験（scratchpad の 6 クレートの workspace。各ビルドスクリプトが 0.7 秒眠るだけ）:

```sh
mkfifo js.fifo; exec 9<>js.fifo; python3 -c "import os; os.write(9, b'+'*4)"   # 札 4 枚
CARGO_MAKEFLAGS="-j5 --jobserver-auth=fifo:$PWD/js.fifo" cargo build --offline -q
```
