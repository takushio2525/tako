---
title: OpenAI Codex CLI
description: codex を tako で使う手順（導入・ログイン・MCP 接続・master / worker としての使い方）と、Claude Code との差分
---

`codex`（OpenAI Codex CLI）は **master にも worker にもなれる**系統です。ChatGPT のプランを持っているなら、tako の使い方はほとんど変わりません。

## 入れる

```bash
tako setup bootstrap install --agent codex
```

何をどこに入れるかは実行前に表示されます。先に見るだけなら `--dry-run`、いま入っているかどうかだけなら `tako setup bootstrap status --agent codex` です。

公式の手順を直接使う場合は次のとおりです。

```bash
curl -fsSL https://chatgpt.com/codex/install.sh | sh
```

Windows の公式手順は `irm https://chatgpt.com/codex/install.ps1 | iex` ですが、**Windows で tako が導入を代行できるのは claude だけ**です。codex は状態の確認と手順の案内までなので、表示されたコマンドをご自身で実行してください。

本体は macOS では `~/.local/bin/codex` に置かれます（Windows は `%LOCALAPPDATA%\Programs\OpenAI\Codex\bin`）。claude と違って背景更新はしません。新しい版が出ると codex 自身が知らせるので、`codex update` で更新してください。

## ログインする

```bash
codex login
```

ブラウザでの操作が要るため tako は代行しません。ログインが済んだら `tako setup` をやり直してください。

## tako に繋ぐ（MCP）

```bash
tako setup-mcp --agent codex
```

引数なしの `tako setup-mcp` でも、導入済みなら codex は対象に入ります。書き込み先は `~/.codex/config.toml` の `[mcp_servers.tako]` です。

codex は MCP サーバーへ環境変数をそのまま渡さないので、**転送する変数名の一覧**（`TAKO_SOCKET` / `TAKO_TOKEN` / `TAKO_PANE_ID` / `TAKO_ORCHESTRATOR_ROLE`）も一緒に書き込みます。**書くのは変数名だけで、トークンの値は設定ファイルに残りません。**

ツールが見えないときは `codex mcp list` を確認してください。`tako` の `env_vars` に `TAKO_SOCKET` / `TAKO_TOKEN` が並んでいないとツールが 0 個になります。その場合は `tako setup-mcp --agent codex` で入れ直せます。

## master として使う

```bash
tako orchestrator profiles set default --master-agent codex
tako master
```

master として起動するときは、tako が MCP の設定を**起動コマンドへその場で渡します**（`~/.codex/config.toml` の恒久登録とは別経路です）。tako の外で立ち上げた codex に tako の操作ツールが見えないようにするための線引きです。

## worker として使う

```bash
tako orchestrator spawn --agent codex --project app --prompt "テストを通してください"
```

毎回 codex にしたいなら `tako orchestrator profiles set default --worker-agent codex` で既定を変えられます。

worker からも tako の MCP ツールを呼べます。`tako_list_panes` や、ペインを省略した `tako_set_title` が自分のペインに当たります。

:::caution[モデル名は指定するか、指定しないか]
プロファイルに claude 用のモデル名が入っていても codex には渡しません（渡すと存在しないモデル名で起動してしまうため。[#1013](https://github.com/takushio2525/tako/issues/1013)）。codex のモデルを決めたいときは次のどちらかにしてください。

```bash
tako orchestrator profiles set default --agent codex --agent-model <モデル名>   # 既定にする
tako orchestrator spawn --agent codex --model <モデル名> --project app --prompt "..."   # この worker だけ
```

指定しなければ codex CLI の既定モデルが使われます。選べるモデルは `tako setup models --agent codex` で実際の一覧を引けます。
:::

## Claude Code との差分

現時点で 47 件中 32 件が「対応」です。全件の内訳と理由は [OpenAI Codex CLI を選ぶと落ちるもの](/agent-support/#openai-codex-cli-を選ぶと落ちるもの) にあります。手元で最新を引くなら次を実行してください。

```bash
tako agent-support --agent codex
```

とくに効くのは次の 3 点です。

- **会話の復元がまだ配線されていません**。`tako sessions resume` と、PC 再起動後にペインを会話ごと戻す復元は claude 専用です。codex のペインは再起動後に新しいシェルになります（[#984](https://github.com/takushio2525/tako/issues/984) / [#1238](https://github.com/takushio2525/tako/issues/1238)。手段自体は上流にあり、`codex resume` を手で実行すれば戻せます）
- **tako 側からアカウントを切り替えられません**。設定ファイルの置き場が固定のためです（[#975](https://github.com/takushio2525/tako/issues/975)）
- **ワークスペースのクレジットが尽きたときは自動で再開しません**。5 時間 / 週の枠は解除を待って自分で再開しますが、クレジット切れには「待つ」出口が無いので tako は何も選ばずに止まります

利用制限の残量はステータスバーに表示されます（[#985](https://github.com/takushio2525/tako/issues/985) で 5 時間枠と週枠の実データを読むようになりました）。
