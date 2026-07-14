# オーケストレーション設計改善メモ — LangGraph 概念の tako 翻訳

> Issue #161 の設計文書。LangGraph の 7 概念を tako の語彙で翻訳し、
> 現状の済み/残ギャップ/提案アーキテクチャを整理する。
> コード実装には触れない（crates/ 変更禁止）。

## 背景

2026-07-12〜13 の夜間バッチ無人運用で、master が手動介入を要した痛点 7 件を
LangGraph の設計概念と対応づけた。ライブラリ採用はしない（API 従量課金・不可視
実行が tako のサブスク CLI + ペイン可視の思想と非互換）が、設計概念の取り込みで
「夜間バッチの master 手動介入をゼロに近づける」ことを目指す。

## 全体方針

- 既存の dispatch / protocol / CLI / MCP 1:1 体系に乗せる（開発不変条件）
- tako-core にドメインモデル、tako-control に制御ロジック（既存の層分離を維持）
- LangGraph の用語をそのまま持ち込まず、tako の語彙（タスク・ペイン・dispatch）に翻訳する

---

## 1. チェックポイントと再開（LangGraph: checkpointer）— 優先度 1

### 痛点

アプリクラッシュ・codex 利用上限・API 切断のたびに、master が手動で「セーフティ
コミット → 状態要約 → 別モデルへ引き継ぎ」した（実運用で 1 夜 3 回）。

### 現状済み

| 機能 | 場所 | Issue |
|---|---|---|
| handoff ファイル + `tako orchestrator handoff` | `orchestrator/mod.rs`（`handoff_path` / `read_handoff`）、`dispatch.rs`（`OrchestratorHandoff`） | #123 / #193 |
| sessions カタログ（会話の発見・`claude --resume`） | `sessions.rs`（`SessionCatalog`） | #112 |
| ペイン平文ログ（ペイン死亡後も出力を遡る） | `tako-core::pane_log` | #112 |
| layout.json 永続化 + tmux バックエンド + `tako recover` | `layout.rs`、`tmux_backend.rs`、`dispatch.rs` | #30 / #113 / #177 |
| WORKER_ERROR 検知 + recommended_action | `wait.rs`（`WorkerErrorKind`） | #157 |
| WORKER_STALLED 検知 | `wait.rs`（`WatchOutcome::Stalled`） | #224 |

### 残ギャップ

1. **タスク進行状態の永続化がない**: worker が「どの Issue の何フェーズ（実装中/検証中/PR済み）にいるか」
   を構造化して保存していない。handoff は master 視点の自由形式テキストで、worker 1 体の状態を
   精密に resume する仕組みではない
2. **resume 操作がない**: クラッシュ後の復帰は `sessions resume`（`claude --resume`）で
   会話だけ復元するか、master の handoff で新 master が情報を引き継ぐかの二択。
   「同じ Issue のブランチ上で、直近のコミットから続きを実行する」明示操作がない
3. **自動引き継ぎがない**: usage_limit 到達時に別モデルへの切替を master が手動判断している
   （これは概念 5: フォールバックポリシーとも重複）

### 提案アーキテクチャ

#### データモデル: `TaskCheckpoint`（tako-core に新設）

```
TaskCheckpoint {
    task_id: String,         // "run-{N}" or Issue 番号ベースの識別子
    pane_id: u64,
    issue: Option<u32>,      // GitHub Issue 番号
    branch: Option<String>,  // 作業ブランチ
    phase: TaskPhase,        // Queued → Running → Verifying → Done / Failed / Suspended
    last_commit: Option<String>,  // 直近の git commit SHA
    agent: WorkerAgent,
    model: Option<String>,
    prompt_head: Option<String>,  // 再開時のコンテキスト復元用
    suspended_reason: Option<String>,  // usage_limit / api_error / crash 等
    updated_at: i64,         // Unix timestamp
}
```

永続化先: `<data_dir>/task_checkpoints.yaml`（config_io 経由、sessions.yaml と同パターン）。

#### コマンド体系

| 操作 | CLI | MCP | dispatch |
|---|---|---|---|
| チェックポイント記録 | `tako task checkpoint --pane N [--phase ...]` | `tako_task_checkpoint` | `TaskCheckpoint` |
| 一覧 | `tako task list [--phase running]` | `tako_task_list` | `TaskList` |
| 再開 | `tako task resume <task_id> [--model ...]` | `tako_task_resume` | `TaskResume` |

`resume` は: ① checkpoint から branch / cwd / issue を復元 → ② 新ペインを spawn →
③ system prompt に「前回 checkpoint の要約 + "continue from commit {sha}"」を注入。
master prompt の recovery セクションに resume コマンドの使い方を追記する。

#### イベント

- `TASK_CHECKPOINT`: watch がフェーズ遷移を検出したとき（git commit / PR 作成 / テスト通過）
  → master の watch ループに自動チェックポイント更新を追加
- `TASK_SUSPENDED`: usage_limit / crash でワーカーが停止 → checkpoint.phase = Suspended +
  reason 記録。概念 5（フォールバック）と連動して自動 resume 可能にする

---

## 2. 異常検知とイベント化（LangGraph: interrupt / エラーハンドリング）— 優先度 2

### 痛点

WORKER_IDLE だけでは「完了・質問・API エラー・リミット・モデル勝手に切替」を
区別できず、master が毎回 `read_pane` で画面判別していた。

### 現状済み

| 機能 | 場所 | Issue |
|---|---|---|
| WorkerErrorKind（api_error / usage_limit / limit_dialog） | `wait.rs:73-113` | #157 |
| recommended_action（resume / wait_reset / respond_dialog） | `wait.rs:102-113` | #157 |
| WatchOutcome::Stalled | `wait.rs:63-67` | #224 |
| detect_worker_error（画面パターンマッチ）| `wait.rs:811-858` | #157 |
| screen_looks_busy / screen_looks_idle（claude/codex/agy 3 種対応） | `wait.rs:764-791` | #120 |
| screen_is_collapsed（折りたたみ検出）| `wait.rs:796-800` | #224 |
| worker_status の status_source（agents / agents-auto / screen） | `dispatch.rs`（`finish_worker_status`） | #181 |
| master prompt の Recovery & Error Handling セクション | `default_system_prompt.md:284-313` | #157 |

### 残ギャップ

1. **「完了」と「質問あり」の区別がない**: idle は「入力待ち」だが、それが「作業完了」なのか
   「ユーザーに質問している」のかを区別できない。master は read_pane して自分で判断している
2. **model_switched の検知がない**: `limit reached, now using ...` の自動切替は除外されるが、
   「今 mini に落ちている」ことを master が能動的に知る手段がない
3. **イベントの構造化ストリームがない**: watch は同期ポーリングで、イベントはログ行
   （`WORKER_IDLE` / `WORKER_ERROR`）として出るだけ。MCP からは worker_status の
   応答 JSON を見るしかない

### 提案アーキテクチャ

#### WorkerEventKind の拡張（wait.rs に追加）

```rust
enum WorkerEventKind {
    // 既存
    Idle,         // 作業完了（既存の WatchOutcome::Idle）
    Error(WorkerErrorKind),  // 既存の api_error / usage_limit / limit_dialog
    Stalled,      // 既存の WatchOutcome::Stalled

    // 新設
    Question,     // worker が質問している（idle + 画面末尾に ? / 選択肢パターン）
    ModelSwitched { from: String, to: String },  // 自動モデル切替検出
    ContextHigh { percent: u32 },  // ctx 使用率が閾値（60%）を超えた
    Done,         // worker が明示的に「完了」と報告（セルフテストマーカー等の積極検出）
}
```

#### 検出メカニズム

- **Question 検出**: `detect_worker_question(output)` を新設。idle 確定後の画面に
  `?` 終端行 / 選択番号（`1.` `2.`）/ `Which` / `Should I` 等の質問パターンがあれば
  Question。既存の `detect_worker_error` と同じ tail_lines 方式
- **ModelSwitched 検出**: `limit reached, now using {model}` 行のパース。
  現在は除外（誤検知防止）しているが、情報として記録する価値がある
- **ContextHigh**: worker_status の ctx_percent が 60% 超のとき。既存フィールド

#### コマンド体系

worker_status の応答 JSON に `events` 配列を追加（直近のイベント履歴を返す）:
```json
{
    "status": "idle",
    "events": [
        {"kind": "context_high", "percent": 65, "at": 1752540000},
        {"kind": "model_switched", "from": "opus-4.6", "to": "sonnet-4.5", "at": 1752540100}
    ]
}
```

master prompt に各イベントの推奨リカバリを追記する（既存 Recovery セクションの拡張）。

---

## 3. 受け入れゲートの遷移条件化（LangGraph: 条件付きエッジ）— 優先度 3

### 痛点

「worker が done と言った」と「実際に検証が通った」は別物で、master が毎回
diff・マージ・実機を検査している。

### 現状済み

| 機能 | 場所 | Issue |
|---|---|---|
| master prompt の Acceptance Inspection セクション（5 ステップ検収） | `default_system_prompt.md:315-344` | #100 |
| worker prompt template の受け入れ条件・検証手順・証拠つき報告 | `default_system_prompt.md` | #100 |
| WatchOutcome の Error / Stalled 時は auto_close しない | `wait.rs:362-374` | #157 |

### 残ギャップ

1. **受け入れ条件が構造化されていない**: master prompt に「検証せよ」と書いてあるが、
   「何を検証するか」はタスクごとに master が会話文脈で保持するだけ。コード側に
   述語の定義と判定結果の記録がない
2. **検証結果が永続化されない**: 検収の合否は master の会話に閉じており、同じ Issue を
   再スポーンしたとき「前回どこで落ちたか」がわからない
3. **機械検証可能な述語の自動実行がない**: `cargo test` の緑 / `git diff` の空 /
   PR merged 等は master の手動実行に頼っている

### 提案アーキテクチャ

#### データモデル: `AcceptanceGate`（tako-core に新設）

```
AcceptanceGate {
    task_id: String,
    criteria: Vec<AcceptanceCriterion>,
    overall: GateStatus,  // Pending → Passed / Failed
}

AcceptanceCriterion {
    id: String,           // "tests_green", "pr_merged", "install_done" 等
    kind: CriterionKind,  // Command(cmd) / PrMerged / Custom(description)
    status: CriterionStatus,  // Pending / Passed / Failed(reason)
    evidence: Option<String>, // 実行結果の要約
    checked_at: Option<i64>,
}

enum CriterionKind {
    Command { cmd: String, expect_exit_0: bool },  // `cargo test --workspace`
    PrMerged { pr_number: u32 },
    Custom { description: String },  // 人間判断が必要なもの
}
```

#### コマンド体系

| 操作 | CLI | MCP | dispatch |
|---|---|---|---|
| ゲート定義 | `tako task gate set <task_id> --criterion "..."` | `tako_task_gate` | `TaskGateSet` |
| 述語チェック | `tako task gate check <task_id>` | `tako_task_gate_check` | `TaskGateCheck` |
| 結果参照 | `tako task gate show <task_id>` | `tako_task_gate_show` | `TaskGateShow` |

`gate check` は Command 種別の述語を worker の cwd で実行し、結果を記録する。
master の acceptance セクションに「gate check を使え」と追記する。

#### タスク状態機械との統合

概念 1 の `TaskCheckpoint.phase` と連動:
- worker が idle → master が `gate check` → 全 Passed → phase = Done
- 1 つでも Failed → phase = Verifying のまま → master が worker に具体的欠陥を送信

---

## 4. タスクキューの永続化（LangGraph: StateGraph の状態）— 優先度 4

### 痛点

Issue 消化キューの順序・依存が master の会話文脈にしかなく、master が落ちると失われる。

### 現状済み

| 機能 | 場所 |
|---|---|
| projects.yaml（プロジェクト登録・cwd 解決） | `orchestrator/mod.rs`（`ProjectsConfig`） |
| config_io（排他 flock + アトミック書き込み） | `config_io.rs`（#169） |
| handoff ファイル（master 交代時の状態引き継ぎ） | `orchestrator/mod.rs`（`handoff_path`） |

### 残ギャップ

- **タスクキュー自体が存在しない**: master が「次に何をするか」は会話文脈に閉じている。
  master のクラッシュ・ctx 上限でのハンドオフ時に「残タスクリスト」が失われる

### 提案（子 Issue 化は優先度 4 のため #161 コメントに留める）

`<config_dir>/task_queue.yaml` に `{task_id, issue, project, prompt, depends_on, status}` のキューを永続化。
`tako task queue add / list / next / done` + MCP。master は queue から取り出して spawn するだけになり、
master 交代・再起動に耐える。依存関係（`depends_on: [task-1]`）で順序制約。
概念 1 の TaskCheckpoint と統合し、queue のエントリが checkpoint の task_id になる。

---

## 5. モデル/エージェントのフォールバックポリシー（LangGraph: リトライポリシー）— 優先度 5

### 痛点

sol → mini 勝手切替の検知、sol リミット時の opus/fable 切替をすべて master が手動判断。

### 現状済み

| 機能 | 場所 | Issue |
|---|---|---|
| WorkerModelPolicy（inherit / fixed / delegate） | `orchestrator/mod.rs:211-221` | — |
| worker_agents（エージェント別の model/effort/skip_permissions） | `orchestrator/mod.rs:247-262` | #120 |
| ResolvedWorkerLaunch（agent + model + effort の解決） | `orchestrator/mod.rs:265-272` | #120 |
| WorkerErrorKind::UsageLimit + recommended_action "wait_reset" | `wait.rs:77-78` | #157 |
| master prompt の Recovery セクション（手動判断のガイダンス） | `default_system_prompt.md:284-313` | #157 |

### 残ギャップ

- **宣言的フォールバック定義がない**: 「sol のリミット時は opus に切替」をプロファイルに
  書けない。master が watch イベントを見て手動で判断 → resume する必要がある
- **自動引き継ぎがない**: 概念 1 の resume と連動して「usage_limit → checkpoint →
  別モデルで resume」を自動実行する仕組みがない

### 提案（子 Issue 化は優先度 5 のため #161 コメントに留める）

Profile に `fallback_chain` を追加:
```yaml
fallback_chain:
  - trigger: usage_limit
    action: resume_with
    model: claude-fable-5
    effort: max
  - trigger: model_switched_to_mini
    action: suspend  # master に判断を委ねる
```
概念 1 の `TaskResume` と連動し、checkpoint.suspended_reason が trigger にマッチしたら
自動で resume_with を実行する。

---

## 6. 並列実行と合流の一級市民化（LangGraph: Send / fan-out）— 優先度 6

### 痛点

同一リポ並列のための worktree 作成・projects 登録・撤去がすべて手動儀式。

### 現状済み

| 機能 | 場所 | Issue |
|---|---|---|
| master-reserved レイアウトエンジン（worker 領域の grid/spiral 配置） | `dispatch.rs`（`layout_engine`）、`tako-core`（`spawn_layout`） | #165 |
| spawned_by チェーン（worker 領域判定） | `PaneTree`（`spawned_by`）、`dispatch.rs` | #165 |
| projects 動的追加・削除 | `orchestrator/mod.rs`（`ProjectsConfig::mutate`） | — |

### 残ギャップ

- **worktree の自動作成・撤去がない**: `git worktree add` / `git worktree remove` を
  master が手動で行い、projects に登録してから spawn する必要がある
- **合流（join）操作がない**: 複数 worker の完了を待って集約する操作がない。
  master が個別に watch → read → 手動集約している

### 提案（子 Issue 化は優先度 6 のため #161 コメントに留める）

`tako orchestrator spawn --isolated` で:
1. `git worktree add <temp_dir> -b <branch>` を自動実行
2. worktree のパスを projects に一時登録（`_isolated: true` フラグ）
3. worker がそのディレクトリで作業
4. worker 完了 + acceptance gate passed → worktree のブランチを main に merge →
   `git worktree remove` + projects から削除

合流は `tako task wait --all <task_id_1> <task_id_2>` で複数タスクの全完了をブロック待ち。
概念 1 の TaskCheckpoint と統合し、全タスクが Done になったら master に通知。

---

## 7. 実行履歴の可観測性（LangSmith 相当）— 優先度 7

### 痛点

worker の各フェーズ・判定・証拠が pane のスクロールバックにしかない
（alt screen だと消える）。

### 現状済み

| 機能 | 場所 | Issue |
|---|---|---|
| ペイン平文ログ（確定行の増分保存） | `tako-core::pane_log`（`PaneLogWriter` / `PaneLogConfig`） | #112 |
| sessions カタログ（claude の session_id → transcript 参照） | `sessions.rs`（`SessionCatalog`） | #112 |
| perf.log（UI ストール・dispatch 遅延の診断） | `tako-app`（`perf_span` / `watchdog`） | #168 |
| persist.log（復元成否・理由・明示削除） | `layout.rs` | #30 |
| master prompt の evidence-per-criterion ルール | `default_system_prompt.md:322-325` | #100 |

### 残ギャップ

- **構造化されたタスク実行履歴がない**: ペイン平文ログと claude transcript は「生の出力」で、
  「いつどのフェーズに遷移し、どの検証がどういう結果だったか」の時系列が取れない
- **pane_log と sessions の統合 UI がない**: CLI で個別に引けるが、1 つのタスクの
  全経緯（spawn → checkpoint → error → resume → done + acceptance）を俯瞰できない

### 提案（子 Issue 化は優先度 7 のため #161 コメントに留める）

概念 1 の TaskCheckpoint にイベントログを追加:
```
TaskEvent {
    task_id: String,
    kind: String,        // "phase_change" / "error" / "checkpoint" / "acceptance" / "resume"
    detail: Value,       // 種別ごとの構造化データ
    at: i64,             // Unix timestamp
}
```
永続化先: `<data_dir>/task_events.jsonl`（追記のみ、ローテート）。
`tako task log <task_id>` で時系列表示。sessions / pane_log へのリンクを含める。

---

## 実装順序と依存関係

```
概念 1（checkpoint/resume）  ← 他の全概念の基盤
    ↓
概念 2（イベント種別化）     ← 概念 5（フォールバック）の判断材料
    ↓
概念 3（受け入れゲート）     ← 概念 1 の phase 遷移の判定条件
    ↓
概念 4（タスクキュー）       ← 概念 1 の task_id 体系に乗る
概念 5（フォールバック）     ← 概念 1 の resume + 概念 2 のイベント
概念 6（fan-out）            ← 概念 1 の checkpoint + 概念 3 のゲート
概念 7（可観測性）           ← 概念 1 のイベントログ
```

**最初の 3 つ（1→2→3）が費用対効果最大**。夜間バッチの無人化に直結する:
- 概念 1 でクラッシュ復帰が自動化
- 概念 2 で異常の種別が master に構造化データで届く
- 概念 3 で「完了」の判定が master の主観から機械検証に移る

---

## 既存機能との境界（what's already done）

この設計が「新規」とする箇所は、上記の「残ギャップ」にまとめたもののみ。
以下は明確に「済み」であり、再実装しない:

- 画面ベースの busy/idle/error/stalled 検出（wait.rs。精度改善は別 Issue）
- handoff ファイルの読み書き + master 交代（dispatch OrchestratorHandoff）
- sessions カタログの CRUD + resume（sessions.rs。claude 限定）
- ペイン平文ログの記録・参照（pane_log.rs）
- 非同期 run レジストリ（wait.rs の RunRegistry）
- レイアウトエンジンの worker 領域配置（#165）
- 排他 flock + アトミック書き込み（config_io.rs。#169）
