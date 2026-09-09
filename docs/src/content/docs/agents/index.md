---
title: エージェントの選び方
description: tako で使えるエージェント CLI は 4 系統。どれを選ぶか、あとから切り替えるにはどうするかをまとめています
---

tako は特定のエージェント CLI に固定されていません。**エージェントは設定ファイルに書き込んだら終わりの属性ではなく、worker ごとに選び替えられるもの**として扱います。実装の基準は Claude Code ですが、ほかの系統でも同じ画面・同じ操作で worker を動かせます。

| 系統 | コマンド名 | master になれる | worker になれる | 個別ページ |
|---|---|---|---|---|
| Claude Code | `claude` | なれる | なれる | [Claude Code](/agents/claude/) |
| OpenAI Codex CLI | `codex` | なれる | なれる | [OpenAI Codex CLI](/agents/codex/) |
| Antigravity CLI | `agy` | なれない（worker 専用） | なれる | [Antigravity CLI](/agents/agy/) |
| ローカル LLM | （未定） | まだ成立していない | まだ成立していない | [ローカル LLM](/agents/local-llm/) |

どこまで同じことができるかは [エージェント別の対応状況](/agent-support/) にマトリクスがあります。**このページと個別ページの記述はそこから引いたもの**で、判定の正本は tako 本体が持っています。

## どれを選べばよいか

**迷ったら Claude Code** を選んでください。tako のすべての機能が最初にここへ実装され、ほかの系統はそこへ追いつく形で作られています。

- **すでに ChatGPT のプランを持っている** — [OpenAI Codex CLI](/agents/codex/)。master にもなれるので、tako の使い方はほぼ変わりません
- **Google のプランを持っている** — [Antigravity CLI](/agents/agy/)。worker 専用なので、master には claude か codex が別に要ります
- **手元のマシンだけで完結させたい** — [ローカル LLM](/agents/local-llm/)。**まだ成立していません**

## 選び替えの手段

いま tako に入っている手段は次のとおりです。設定ファイルをエディタで開く必要はありません。

| 場面 | コマンド |
|---|---|
| この worker だけ別の系統で立てる | `tako orchestrator spawn --agent codex --project app --prompt "..."` |
| worker の既定を変える | `tako orchestrator profiles set default --worker-agent codex` |
| master を別の系統にする | `tako orchestrator profiles set default --master-agent codex` |
| 系統ごとの既定モデルを決める | `tako orchestrator profiles set default --agent codex --agent-model <モデル名>` |
| git のコンフリクト解消だけ別の系統に任せる | `tako git resolve --agent codex` |

`spawn` の `--agent` を省くとプロファイルの `worker_agent`、それも無ければ claude が使われます。**プロファイルの設定は「既定」であって「固定」ではありません。**

worker はこの指定が次の spawn からすぐ効きます。master の系統を変えたときだけ、次に `tako master` を起動したときから切り替わります。

:::note[モデル名は系統をまたいで持ち越されません]
プロファイルに claude 用のモデル名（`claude-opus-5` など）が入っていても、codex / agy の worker にはそれを渡しません。存在しないモデル名で起動してしまうためです（#1013）。系統ごとのモデルは `--agent-model` で決めるか、指定せずに各 CLI の既定へ任せてください。
:::

## 切り替える前に要るもの

系統を増やすと、その系統ぶんの前提が要ります。1 と 3 は `tako setup` がまとめて面倒を見るので、通常は意識する必要はありません。

1. **CLI が入っていること** — `tako setup bootstrap install --agent codex`
2. **ログイン済みであること** — ブラウザ操作が要るので tako は代行しません。案内するコマンドは系統ごとに違います（[claude](/agents/claude/#ログインする) / [codex](/agents/codex/#ログインする) / [agy](/agents/agy/#ログインする)）
3. **tako の MCP が登録されていること** — `tako setup-mcp`。引数なしで、claude と導入済みの codex / agy にまとめて入ります

状態だけ見たいときは次を実行してください。3 系統それぞれについて「入っているか / PATH が通っているか / ログイン済みか」が並びます。

```bash
tako setup bootstrap status-all
```

## まだ固定になっているところ

- **master / solo の系統は起動時に選べません**。プロファイルの `master_agent` を変えてから `tako master` を起動する形です。起動のたびに選べるようにする導線は対応中です（[#988](https://github.com/takushio2525/tako/issues/988)）
- **アカウントの切り替えは claude だけに効きます**。codex / agy は設定ファイルの置き場が固定なので、tako 側からアカウントを差し替えられません（[#975](https://github.com/takushio2525/tako/issues/975)）
