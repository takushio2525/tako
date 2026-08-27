# agent 種別を表す enum の対応表（#982）

> **正本は `crates/tako-core/src/agent_support.rs` の `Agent`**。
> このファイルは「同じ概念の enum が並存している現状」と「なぜ 1 つに統合していないか」を
> 記録する。機械検証は `crates/tako-control/tests/agent_parity.rs`。

## いま並存している 5 つ

棚卸し（#975・2026-08-27）で、agent 種別を表す enum が**正本の他に 5 つ**あることが分かった。
どれも「claude / codex / agy」を並べているが、**目的と値の集合が違う**。

| enum | 置き場 | クレート | 値 | 目的 |
|---|---|---|---|---|
| **`agent_support::Agent`** | `tako-core/src/agent_support.rs` | tako-core | Claude / Codex / Agy / **Local** | **能力マトリクスの正本**。何がどこまで使えるか |
| `orchestrator::agent::WorkerAgent` | `tako-control/src/orchestrator/agent.rs` | tako-control | Claude / Codex / Agy | worker として起動できる系統（コマンド組み立て・事前信頼） |
| `setup::SetupAgent` | `tako-cli/src/setup.rs` | tako-cli | Claude / Codex / Agy | setup を進行できる系統（**非公開 enum**） |
| `agents_sync::AgentKind` | `tako-control/src/agents_sync.rs` | tako-control | Claude / Codex / Agy | 共通ルールの同期先（#136） |
| `platform::agent_install::AgentKind` | `tako-core/src/platform/agent_install.rs` | tako-core | **Claude のみ** | 自動インストールに対応する系統（#868） |
| `terminal::LimitService` | `tako-core/src/terminal.rs` | tako-core | Claude / Codex / Agy | 利用制限の表示対象（#357） |

## なぜ 1 つに統合していないか

**値の集合が本当に違う**ので、機械的にまとめると嘘になる。

- `Agent::Local`（ローカル LLM）は **`WorkerAgent` に無い**のが正しい。
  `WorkerAgent` は「TUI をキー操作で駆動する」前提の型で、その前提を外すのが #991
- `agent_install::AgentKind` が Claude 1 値なのは **実装がそうだから**（#868 の Out of scope）。
  ここに codex / agy を足すのは #989 の仕事で、足した瞬間に能力マトリクスの
  `setup_cli_install` 行も動く
- `LimitService` に Local が無いのは、ローカルモデルに利用制限の概念が無いから

つまり **5 つは「正本の部分集合」であって重複ではない**。統合ではなく
「正本へ写せること」と「値が勝手に増減しないこと」を縛るのが正しい形と判断した。

## 縛り方

### 1. ソース走査（5 つとも同じ規則で見張る）

`agent_parity.rs` の `WATCHED` が 6 つ（正本 + 5 つ）の**期待する値の集合**を持ち、
ソースから実際のバリアント名を拾って突き合わせる。どれかに値が増減すると落ちる。

**なぜソース走査か**: `SetupAgent` が**非公開 enum** で型として見えない。
5 つを別々の方法で見張ると、見張り方の穴が enum ごとに違ってしまう。

落ちたときにやることは 3 つ:

1. `agent_parity.rs` の `WATCHED` の期待値を直す
2. `agent_support::MATRIX` の列と根拠を見直す（値が増えたなら列が要る）
3. このファイルの対応表を直す

### 2. 型変換（見える 4 つは網羅 match でも押さえる）

ソース走査が壊れても（書式の変化・抽出の穴）残るように二重化してある。

| 変換 | 置き場 |
|---|---|
| `LimitService` → `Agent` | `tako-core/src/agent_support.rs`（`From`） |
| `agent_install::AgentKind` → `Agent` | 同上 |
| `WorkerAgent` ⇄ `Agent` | `tako-control/src/orchestrator/agent.rs`（`From` / `TryFrom`） |
| `agents_sync::AgentKind` → `Agent` | 変換は持たず `label()` の表記一致で縛る |
| `SetupAgent` → `WorkerAgent` | `tako-cli/src/setup.rs`（`worker_agent_of`。**非公開 enum なので `From` を持てない**ので、`as_str()` 一致を単体テスト `系統の写しは新しいenumを作らずに済んでいる` で縛る。#1002） |

**`Agent` → `WorkerAgent` は `TryFrom`**（`Local` を落とす部分写像）。
ローカル LLM を worker として起動できるようになったら、ここが変換できるようになる時点で
`WorkerAgent` へ値を足す必要がある = テストがそれを教える。

変換を **`agent_support.rs` 側に置いた**のは、既存 enum のファイルを 1 行も触らずに
正本へ寄せられるから（並行作業との衝突を避ける意味もある）。

## 段階的に寄せる（一気に置換しない）

`WorkerAgent::has_agents_api()` は #982 で**マトリクスへ吸収済み**
（`keys::WORKER_STATUS_STRUCTURED` を引く。`TAKO_982_LEGACY=1` で吸収前へ戻せる）。

残りの能力判断は各スライスが**その機能を実装するときに**マトリクス経由へ寄せる。
先に全部の呼び出し側を書き換えると、実装が無いまま「使える / 使えない」の分岐だけが
増えて、どのマスが本当に効いているのか分からなくなる。

| これから寄せる先 | 現状 | 担当スライス |
|---|---|---|
| `dispatch.rs` の Bypass 事前承諾（`WorkerAgent::Claude` 直比較 2 箇所） | claude 限定の `if` | #983 |
| `registry.rs` の `prompt_delivery_assessment`（`entry.agent != "claude"`） | 文字列比較で `NotApplicable` | #983 |
| `dispatch.rs` の worker_status の `status_source` 分岐 | 実行時の session_id 解決結果で決める | #984 |
| `dispatch.rs` の transcript アダプタ | claude 固定（拡張点のコメントあり） | #984 |
| `WorkerLaunch` の MCP 注入 | 配線が無い | #986 |
| `agent_install::AgentKind` の拡張 | Claude 1 値 | #989 |
| モデル一覧の取得手段（`agent_models::catalog_argv`） | 系統ごとの `match`。**能力マトリクスには「ピッカーが使えるか」だけ**が載る（claude = 基準系 = 全 Supported の不変条件があるため、取得手段の差はマトリクスの 1 マスでは表せない） | #1002（済） |

## 新しい系統を足すとき

1. `agent_support::Agent` へ値を足す（`ALL` / `as_str` / `parse` / `label`）
2. `MATRIX` の全行にその列の値を書く（**根拠なしに `Supported` へ倒すとテストが落ちる**）
3. `agent_parity.rs` の `WATCHED` の期待値を直す
4. `scripts/gen-agent-support-docs.mjs` を走らせて docs を再生成（CI が `--check` する）
5. このファイルの対応表を直す
