# 北極星測定: 同一タスクを claude / codex / agy の worker で回した横断実測

- **対象**: #975（マルチエージェント化エピック）の北極星 =「agy / codex ともに claude と全く同じ使用感で使える」
- **実施日**: 2026-08-29
- **測定バイナリ**: `target/release/tako-app` / `target/release/tako` = **v0.8.1**（main `8ef90dd`。#982 / #983 変更 1〜3 / #984 / #985 が入った状態）
- **CLI 版**: claude 2.1.232 / codex-cli 0.150.1 / agy 1.1.22（3 系統とも実認証済み）
- **測定した観点**: ①完了検知までの遅延 ②誤検知の有無 ③送達の成否 ④報告取得の成否
- **結論**: **codex は claude と体感差なし**（4 観点すべてで同等以上）。**agy は①と④に差が残る**
  （完了検知が **claude の 2.6 倍・+24 秒**、報告は scrollback だけ）。差は
  `agent_support::MATRIX` の宣言（`degraded` / `pending(#984)`）と**完全に一致**しており、
  未申告の隠れた差は見つからなかった

## 判定

| 観点 | claude | codex | agy | 判定 |
|---|---|---|---|---|
| ① 完了検知の遅延（中央値） | 15.40s | **11.68s** | **39.38s** | codex = 同等（僅かに速い）/ **agy = 差あり（+24s / 2.6 倍）** |
| ② 誤検知（`watch` のイベント） | 偽イベント 0 | 偽イベント 0 | 偽イベント 0 | 3 系統とも同等 |
| ③ 送達 | delivered（**peer 直送**） | delivered（keys 経路） | delivered（keys 経路） | 結果は同等。経路は claude だけ第 1 層 |
| ④ 報告取得 | transcript（messages 1〜2） | transcript（messages 2） | **scrollback のみ（messages 0）** | codex = 同等 / **agy = 差あり** |

補足の差（体感に効く順）:

- **`WORKER_IDLE` に付く ctx%**: codex は `(ctx 8%)` が出るが **claude は出ない**（#1021 = `claude
  agents --json` に `contextPercentUsed` が無い）。この 1 点だけは **codex が claude より情報が多い**
- **codex の `rate_limits`** は実データが取れた（`plan_type: plus` / 5h 枠 + 週枠の `used_percent` /
  `resets_at`。#985）。claude は `rate_limits: null`、agy は枠を持たない（#985 で確定済み）

## 測定条件

### 隔離

本番インスタンス（別 pid）と他 worker の `tako-iso-*` には一切触れていない。

- 隔離 GUI: `TAKO_ISOLATED=1 TAKO_PERSIST=1`（**器あり = 本番と同じ構成**。tmux socket は
  `tako-iso-<pid>` へ自動隔離）。継承していた `TAKO_SOCKET` / `TAKO_TOKEN` / `TAKO_PANE_ID` と
  `CLAUDE_CODE_*` / `CLAUDECODE` を `env -u` で落として起動（後者を残すと worker の claude が
  transcript 保存を無効化し、④の測定が空振りする）
- CLI は専用ラッパ経由（隔離先の `control.json` から socket / token を読み、空なら停止。
  `TAKO_DATA_DIR` / `TAKO_SESSIONS_FILE` / `TAKO_PANE_LOG_DIR` / `TAKO_TMUX_SOCKET` を明示）
- 隔離先であることは毎回 `tako list` のペイン数で確認（起動直後 = 1 ペイン。本番は十数ペイン）
- 測定用プロジェクト `ns983` は**隔離側の projects.yaml にのみ**登録（本番 `projects.yaml` に
  `ns983` が無いことを実測で確認）

### タスク（3 系統に完全同一のプロンプト）

作業フォルダに **137 行**の `sample.txt`（`line %03d: north star measurement fixture`）を置き、
次のプロンプトを 1 文字も変えずに 3 系統へ渡した:

```
作業フォルダにある sample.txt の行数を数えてください。ファイルの作成や編集はしないでください。数え終わったら、最後に RESULT: に続けて行数の数字だけを 1 行で出力して終了してください。
```

- 正解は 137。プロンプト自身には `RESULT: 137` が現れないので、**画面に `RESULT:\s*137` が出た瞬間 =
  agent が実際に答えを出した時刻**として使える（読み取りのみのタスクにしたのは、#981 で codex worker の
  サンドボックスが既定 read-only になったため。書き込みを含めると codex だけ条件が変わる）
- spawn は 3 系統ともモデル / effort 明示（#1013 の「プロファイルの claude 用モデル名が codex へ渡る」を
  回避するため）: claude = `--model opus --effort high` / codex = `--model gpt-5.6-sol --effort high` /
  agy = `--model gemini-3.7-flash-high --effort high`
- ラウンドごとに worker ペインを close し、次のラウンドは常に同じ 1 ペイン状態から分割した
  （ペイン幅が変わると画面推定の条件が変わるため）

### 測り方

| 量 | 取り方 |
|---|---|
| `T0` | `orchestrator spawn` を叩く直前の時刻 |
| `answer` | 1 秒間隔で `tako read --pane N` を回し、`RESULT:\s*137` が**初めて**現れた時刻 |
| `IDLE` | `orchestrator watch --pane N` の出力行を受け取った時刻（= tako が完了と確定した瞬間） |
| **`lag`（①）** | `IDLE - answer` = **純粋な検知遅延**（タスク自体の実行時間を含まない） |
| `falseIdle` | 同じ 1 秒ポーリングで `orchestrator status` が **answer より前に** `idle` を返した最早時刻 |
| ③ | spawn 応答の `prompt_delivery` / 最終 `status` の `prompt_delivery` / `persist.log` の送達行 |
| ④ | `orchestrator report --pane N --messages 3` の `source` / `transcript_agent` / `messages` 件数と `RESULT: 137` の有無 |

## 測定結果（全 10 ラウンドの生値）

| agent | round | spawn(s) | answer(s) | IDLE(s) | **lag(s)** | falseIdle(s) | status_source | prompt_delivery | report.source | messages | RESULT 取得 |
|---|---|---|---|---|---|---|---|---|---|---|---|
| claude | r1 | 0.186 | 17.61 | 32.94 | **15.33** | 4.091 | `agents` | delivered | `transcript` | 2 | ○ |
| claude | r2 | 0.566 | 17.33 | 33.42 | **16.09** | 4.474 | `agents` | delivered | `transcript` | 1 | ○ |
| claude | r3 | 0.627 | 12.51 | 27.91 | **15.40** | 4.475 | `agents` | delivered | `transcript` | 1 | ○ |
| codex | r1 | 0.174 | 18.58 | 32.84 | **14.26** | 4.559 | `codex-session` | delivered | `transcript` | 2 | ○ |
| codex | r2 | 0.671 | 17.86 | 28.20 | **10.34** | 4.644 | `codex-session` | delivered | `transcript` | 2 | ○ |
| codex | r3 | 0.698 | 15.99 | 27.67 | **11.68** | 3.120 | `codex-session` | delivered | `transcript` | 2 | ○ |
| agy | r1 | 0.657 | — | 50.07 | — | 8.051 | `screen` | delivered | `scrollback` | 0 | × |
| agy | r2 | 0.642 | 34.18 | 73.56 | **39.38** | 21.135 | `screen` | delivered | `scrollback` | 0 | ○ |
| agy | r3 | 0.672 | 18.74 | 61.01 | **42.27** | 5.760 | `screen` | delivered | `scrollback` | 0 | ○ |
| agy | r4 | 0.658 | 17.25 | 56.12 | **38.87** | 5.698 | `screen` | delivered | `scrollback` | 0 | ○ |

agy の r1 は agent 側の事情でタスクを実行しなかった（後述）ため lag を測れていない。代わりに r4 を追加した。

```
=== 検知遅延（lag）の集計 ===
claude  n=3 値=[15.33, 16.09, 15.4]  中央値=15.40 平均=15.61 min=15.33 max=16.09
codex   n=3 値=[14.26, 10.34, 11.68] 中央値=11.68 平均=12.09 min=10.34 max=14.26
agy     n=3 値=[39.38, 42.27, 38.87] 中央値=39.38 平均=40.17 min=38.87 max=42.27
```

## ① 完了検知の遅延 — 実測が実装の理論値とぴたり合う

差の正体は `wait.rs` の 2 定数だけで説明できる。

```rust
// crates/tako-control/src/orchestrator/wait.rs:190-191
// agents 一次シグナル（明示 or 自動解決）は streak 3、画面推定は streak 8
let need_streak: u32 = if source == "screen" { 8 } else { 3 };
```

```rust
// crates/tako-cli/src/main.rs:3708（CLI watch のポーリング間隔）
interval: std::time::Duration::from_secs(5),
```

| source | need_streak | 理論値（streak × 5s） | 実測の中央値 |
|---|---|---|---|
| `agents`（claude） | 3 | 15s | **15.40s** |
| `codex-session`（codex） | 3 | 15s | **11.68s** |
| `screen`（agy） | 8 | 40s | **39.38s** |

- **codex が理論値より速いのは正常**: ポーリングの位相（最初の観測が偶然完了直後に当たる）で
  1 標本ぶん（5 秒）短くなることがある。3 標本すべて 10〜15s の帯に収まっている
- **agy の +24 秒は streak 5 回ぶん**（`(8-3) × 5s = 25s`）。実測差は
  `39.38 - 15.40 = 23.98s` で、**まさに 5 標本ぶん**
- `status_source` の推移も観測した（poll の生値）。3 系統とも最初の数秒は `screen` で、
  そのあと claude は `agents`、codex は `codex-session` へ移る。agy は最後まで `screen` のまま:

```
codex-r1 の source 推移（1 秒間隔・連続数）: screen×6 → codex-session×22
agy-r2   の source 推移: screen のみ（74 標本すべて）
```

## ② 誤検知 — `watch` は 3 系統とも clean、単発 `status` は 3 系統とも揺れる

`watch` が出したイベントは全 10 ラウンドで `WORKER_IDLE` **1 件だけ**。偽の `WORKER_ERROR` /
`WORKER_QUESTION` / `WORKER_PERMISSION` / `prompt_undelivered` は **1 件も出ていない**。

```
claude-r1: [[32.94, 'WORKER_IDLE: tako:2']]
codex-r1:  [[32.84, 'WORKER_IDLE: tako:3 (ctx 8%)']]
agy-r2:    [[73.56, 'WORKER_IDLE: tako:5']]
```

一方、**`orchestrator status` の単発呼び出しは 3 系統すべてで「回答が出る前に `idle`」を返した**
（`falseIdle` 列。claude 4.09〜4.48s / codex 3.12〜4.64s / agy 5.70〜21.14s）。これは
起動直後のまだプロンプトが届いていない画面や、ツール実行の合間の静止を 1 標本で見た結果であり、
**streak を持つ `watch` では起きない**。**claude 固有の優位ではなく 3 系統共通の性質**なので
体感差ではないが、「AI が `status` を 1 回叩いて完了と判断すると誤る」という運用上の罠として記録する
（既存の運用は `watch` 経由なので実害はない）。

## ③ 送達 — 結果は同等、経路は claude だけ第 1 層

全 10 ラウンドで `prompt_delivery = delivered`。spawn 応答時点では `null`（まだ送達中）で、
完了後の `status` で `delivered` になる。**#983 の変更 2 が入った状態で偽の `undelivered` /
`unverified` は 1 件も出ていない。**

`persist.log`（隔離側）の送達行がそのまま経路の記録になっている:

```
[2026-08-29T00:56:05Z] 送達: peer（session=tako-c341574664d5 pid=45276 状態=idle 確認=delivered）      ← claude r1
[2026-08-29T00:57:28Z] 送達: keys 経路（session=tako-4516b7019ce3 peer 不成立=no_claude_pid）          ← codex r1
[2026-08-29T00:58:32Z] 送達: keys 経路（session=tako-77099ddb5e89 peer 不成立=no_claude_pid）          ← agy r1
[2026-08-29T01:02:26Z] 送達: peer（session=tako-9ad549915bac pid=20527 状態=idle 確認=delivered）      ← claude r2
```

- claude は 2/2 で **peer**（#790 の Cross-Session Messaging = 受信箱へ直送）
- codex / agy は 5/5 で **keys 経路**（`peer 不成立=no_claude_pid` = claude ではないので当然）
- 今回のタスクは「idle な worker へ 1 回だけ送る」形なので**両経路とも結果は同じ**。差が出るのは
  「生成中に送る」「長文を送る」場合（#790 の設計どおり）で、今回の測定範囲では体感差にならない

## ④ 報告取得 — agy だけ構造化された発話が取れない

| agent | `report.source` | `transcript_agent` | `messages` 件数 | `RESULT: 137` |
|---|---|---|---|---|
| claude | `transcript` | `claude` | 1〜2 | ○ |
| codex | `transcript` | `codex` | 2 | ○（#984 のアダプタが効いている） |
| agy | `scrollback` | `null` | **0** | ○（`scrollback_text` から取れる） |

claude の最終メッセージ（実出力）:

```
ファイルは末尾に改行があり、`wc -l` の 137 がそのまま行数です（読み取りのみ、ファイルは一切変更していません）。

RESULT: 137
```

**agy でも答えは取れる**（画面テキスト全体が返る）が、`--messages N` で「直近 N 件の assistant 発話」
を取る経路が無いので、**呼び出し側が画面テキストから報告部分を切り出す負担を負う**。長い作業ログが
流れた worker では体感差になる。

## `agent_support::MATRIX` の宣言と実測の突き合わせ

**未申告の差は見つからなかった**（`tako agent-support --agent <系統> --json` の実出力と本実測の対応）。

| 能力キー | claude | codex | agy | 実測との一致 |
|---|---|---|---|---|
| `worker_status_structured` | supported | supported | **pending (#984)** | ○（agy は 74 標本すべて `screen`） |
| `worker_status_detect` | supported | supported | **degraded** | ○（宣言の理由文「8 回続けて見る必要がある。claude は 3 回」= 実測 +24s） |
| `worker_report_transcript` | supported | supported | **pending (#984)** | ○（agy は `messages` 0 件） |
| `worker_report_scrollback` | supported | supported | supported | ○（3 系統とも `RESULT` が取れる） |
| `worker_prompt_delivery` | supported | supported | supported | ○（3 系統とも delivered） |
| `worker_delivery_peer` | supported | **unsupported** | **unsupported** | ○（peer は claude のみ） |

マトリクスは agy の一次シグナル不在の理由と置き場まで書いており、**実在も確認した**（読み取りのみ）:

```
~/.gemini/antigravity-cli/conversations/<id>.db   ← 会話は SQLite（測定中に更新されていた）
~/.gemini/antigravity-cli/presence/<id>.lock      ← 生存は分かるがターンの開始・完了は取れない
```

`agy help` の全サブコマンド（`agent(s)` / `changelog` / `help` / `install` / `mcp` / `mic-serve` /
`models` / `plugin(s)` / `update`）に**状態照会は無い**ことも確認した。`--output-format stream-json`
は print モード専用なので TUI worker には使えない。

## 新しい発見（起票した分）

### 1. agy worker が「アカウント検証待ち」で 1 文字も作業していないのに `WORKER_IDLE` + `delivered`（→ #1034）

agy の r1 で、CLI が起動直後にこう表示してタスクを実行しなかった（実画面。個人情報は #927 に従い置換）:

```
[testuser@host:proj]$ TAKO_ORCHESTRATOR_ROLE='worker:ns983:ns983-agy' agy --model gemini-3.7-flash-high --effort high --dangerously-skip-permissions
  Antigravity CLI 1.1.22
  <account> (Google AI Pro)
  Gemini 3.7 Flash (High)

⚠ Verifying your account...
  ⎿  We're finishing verifying your account eligibility.
     This usually takes a moment. Please try again shortly.
```

このとき tako は `status: idle` / `prompt_delivery: delivered` / `WORKER_IDLE`（50.07s）を報告した。
**master から見ると「worker が仕事を終えた」ように見えるが、実際は 1 文字も進んでいない。**

- 直後に `agy -p` を単体で叩くと `PING_OK` が返り（推論は通る）、次のラウンド（r2）は正常に完走した
  ので、**一時的な検証待ちが TUI 起動時に当たった**ケース
- #983 の `detect_launch_failure` は「送達の証拠がまだ無い worker」に限るゲートなので、
  送達が成立していたこの事象は**設計どおりゲートの外**（誤分類を防ぐための正しい設計）
- ただし `not_authenticated` と同型（起動した・認証も通った・だが実行が拒否された）なので、
  分類できれば master が「再試行すればよい」と分かる → **#1034 として起票**

### 2. `WORKER_IDLE` の ctx% が codex には出て claude には出ない

`WORKER_IDLE: tako:3 (ctx 8%)`（codex）に対し claude は `WORKER_IDLE: tako:2`。原因は #1021
（`claude agents --json` に `contextPercentUsed` が無い）で**既知**。北極星の観点では
「claude 側が劣る唯一の点」として記録するだけで、新規起票はしない。

### 3. `no_personal_data` 番犬が gitignore 済みの未追跡ファイルで落ちる（→ #1035）

本レポートの個人情報チェックで `cargo test -p tako-control --test no_personal_data` を回したところ
2 件 FAILED した。原因は `.claude/settings.local.json:4`（`.gitignore` 済みの未追跡ファイル）で、
**main でも同じ 1 ファイル・同じ 1 行が落ちる**ことを HEAD の A/B で確認した（= 本作業とは無関係）。
本レポート自身は検査を通っている（失敗一覧に 1 件も現れない）。

## 起票した Issue

| # | 内容 |
|---|---|
| **#1033** | agy worker の完了検知が claude の 2.6 倍遅く（+24 秒）報告も scrollback だけ（#984 の agy 分の残り） |
| **#1034** | agent が起動後に実行を拒否されて作業ゼロでも `WORKER_IDLE` + `delivered` になる（#983 の分類の穴） |
| **#1035** | `no_personal_data` 番犬が gitignore 済みの未追跡ファイルで落ちる（手元のテストが恒久的に赤） |

## 測定できなかったこと / リスク

- **agy の r1 は lag を測れていない**（agent 側の検証待ちでタスクを実行しなかった）。r2〜r4 の
  3 標本で判定した
- **生成中の送達（#790 の第 1 層の真価）は測っていない**。今回のタスクは「idle な worker へ 1 回送る」
  形なので、claude の peer 直送と keys 経路の差が結果に出ない条件だった。生成中の送達・長文送達の
  比較は別測定が要る
- **ダイアログ応答（#985）・limit 復帰（#813 / #985）は今回の射程外**。3 系統ともダイアログが
  1 度も出ないタスクを選んだ（承認が挟まると実行時間が人の操作に依存して lag の測定が壊れる）
- **1 タスク・軽量な読み取りのみ**での測定。長時間タスク・多ツールタスクでの検知精度は別途
- **`falseIdle` は 1 秒ポーリングの単発 `status` で観測した値**。tako の運用経路（`watch`）は
  streak を持つので、この値がそのまま製品の誤検知率を意味するわけではない
- 測定は macOS のみ（Windows 実機はこのセッションでは触っていない）

## 生データ

隔離インスタンスの data dir（`$TMPDIR/tako-iso-data-<pid>`）は測定後に解放したため、**数値は
すべて本レポートに埋め込んである**（上の表が `logs/aggregate.json` の全カラム、`watch` の
イベント行・`persist.log` の送達行・実画面は該当節に原文で引用）。採取に使ったのは
`orchestrator spawn` / `watch` / `status` / `report` / `workers` と `tako read` だけで、
すべて製品の公開経路（CLI = MCP と 1:1）である。

## 参照

#975（エピック・北極星）/ #982（能力マトリクス）/ #983（無言死・送達判定）/ #984（監視の同等化。
codex は closed 済み・agy は残る）/ #985（limit・ダイアログ）/ #1013（codex のモデル名）/
#1015（codex の `Waiting for background terminal` 誤検知）/ #1021（claude の ctx% 欠落）/
#790（送達の 2 層）/ #981（codex のサンドボックス既定）/ 本測定で起票した #1033 / #1034 / #1035
