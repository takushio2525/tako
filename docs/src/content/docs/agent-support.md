---
title: エージェント別の対応状況
description: claude / codex / agy / ローカル LLM でどこまで同じことができるか。能力マトリクスから自動生成しています
---

tako は **Claude Code を基準に実装してきました**。ほかのエージェント CLI でも
worker を立てて使えますが、機能によっては落ちるか、まだ使えません。
このページは **tako 本体が持っている能力マトリクスから生成**しているので、
実装とずれません。手元で最新を引くには次を実行してください。

```sh
tako agent-support                        # 全系統の表
tako agent-support --agent codex          # codex の理由つき一覧
tako agent-support --agent agy --status pending   # まだ使えないものだけ
```

## 全体

能力 40 件の内訳です。

| エージェント | 対応 | 一部対応 | 未対応 | 対象外 |
| --- | --- | --- | --- | --- |
| Claude Code（基準） | 40 / 40 | 0 | 0 | 0 |
| OpenAI Codex CLI | 22 / 40 | 7 | 10 | 1 |
| Antigravity CLI | 9 / 40 | 6 | 20 | 5 |
| Local LLM | 0 / 40 | 0 | 35 | 5 |

### 状態の意味

| 状態 | 意味 |
| --- | --- |
| 対応 | Claude Code と同じように使えます |
| 一部対応 | 使えますが機能が落ちます。落ち方は各表の「差分」列 |
| 未対応 | tako 側の実装が無い、または**まだ調べていない**もの。追跡先の Issue 番号が付きます |
| 対象外 | そのエージェント CLI にその手段がそもそも無いもの |

**「未対応」と「対象外」を混ぜていません**。調べていないものは「対象外」ではなく
「未対応」に置いています。まだ道があるかもしれないものを「無理」と書くと、
この宣言を読んで動く AI エージェントがその道を永久に避けてしまうためです。

各表の「根拠」列が判定の裏づけです。

| 根拠 | 意味 |
| --- | --- |
| コード本文 | tako 自身の実装がそうなっていること（配線の有無）を引用したもの |
| 上流の仕様 | エージェント CLI 側の仕様・設計判断で、実測する対象がそもそも無いもの |
| 実測 | 実際に動かして結果を記録したもの |
| テスト | 緑のテストが担保しているもの |
| 未確認 | まだ確かめていないもの |

## ローカル LLM について

現時点では **1 つも成立していません**（表の値はほぼ「未対応」です）。
第一歩は codex CLI を Ollama へ向ける経路で、TUI 前提を外した一級対応は
その次の段階です。

## セットアップ

| 能力 | Claude Code | OpenAI Codex CLI | Antigravity CLI | Local LLM | 根拠 |
| --- | --- | --- | --- | --- | --- |
| **setup がこの CLI の導入を検出する**<br />`setup_detect` | 対応 | 対応 | 対応 | 未対応 [#990](https://github.com/takushio2525/tako/issues/990)<br />ローカル LLM の系統がまだ成立していない（リポジトリに Ollama への参照が 1 件も無い） | コード本文: setup.rs の SetupAgent が 3 系統を列挙し、platform::exe::find（B16）で解決する |
| **setup が認証済みかどうかを判定できる**<br />`setup_auth_check` | 対応 | 対応 | 一部対応<br />認証の有無は分かるがプランを取れないので、推奨プロファイルの規模を決められない | 未対応 [#990](https://github.com/takushio2525/tako/issues/990)<br />ローカル LLM の系統がまだ成立していない（リポジトリに Ollama への参照が 1 件も無い） | コード本文: tako-cli/src/setup.rs のプラン解決は認証済み・導入済みの provider だけを巡る （#262）。agy は provider としてプランを返さない |
| **未認証なら setup がログインまで案内・代行する**<br />`setup_auth_launch` | 対応 | 未対応 [#989](https://github.com/takushio2525/tako/issues/989)<br />tako の実装が claude 専用で、この系統への配線がまだ無い | 未対応 [#989](https://github.com/takushio2525/tako/issues/989)<br />tako の実装が claude 専用で、この系統への配線がまだ無い | 未対応 [#990](https://github.com/takushio2525/tako/issues/990)<br />ローカル LLM の系統がまだ成立していない（リポジトリに Ollama への参照が 1 件も無い） | コード本文: setup.rs の認証誘導は claude の導線しか持たない（#868 のゼロスタートも claude 限定） |
| **setup が契約プランを検出して推奨規模を決める**<br />`setup_plan_detect` | 対応 | 対応 | 対象外<br />agy はプラン情報を出さないので検出できない | 対象外<br />ローカルモデルに契約プランという概念が無い | コード本文: setup.rs の Provider は Claude / Gpt / Google の 3 値だが、プラン取得は claude / gpt の 2 経路しか実装が無い（#226） |
| **CLI 自体が入っていない環境へ setup が導入する（#868）**<br />`setup_cli_install` | 対応 | 未対応 [#989](https://github.com/takushio2525/tako/issues/989)<br />tako の実装が claude 専用で、この系統への配線がまだ無い | 未対応 [#989](https://github.com/takushio2525/tako/issues/989)<br />tako の実装が claude 専用で、この系統への配線がまだ無い | 未対応 [#990](https://github.com/takushio2525/tako/issues/990)<br />ローカル LLM の系統がまだ成立していない（リポジトリに Ollama への参照が 1 件も無い） | コード本文: platform/agent_install.rs の AgentKind が Claude 1 値しか持たず、recipe() も claude ぶんしか無い（#868 の Out of scope。拡張は #989） |
| **setup が起動プロファイルを組み立てる**<br />`setup_profile_recommend` | 対応 | 対応 | 一部対応<br />worker としてのプロファイルは作れるが、master には別系統が自動で選ばれる | 未対応 [#990](https://github.com/takushio2525/tako/issues/990)<br />ローカル LLM の系統がまだ成立していない（リポジトリに Ollama への参照が 1 件も無い） | コード本文: setup.rs は選択した agent を worker_agent へ書くが、master_agent は claude / codex しか受け付けない（agy は起動前エラーになるため） |
| **共通ルールをこの CLI のグローバル指示ファイルへ同期する（#136）**<br />`setup_rules_sync` | 対応 | 対応 | 対応 | 未対応 [#991](https://github.com/takushio2525/tako/issues/991)<br />ローカル LLM の系統がまだ成立していない（リポジトリに Ollama への参照が 1 件も無い） | コード本文: agents_sync.rs の AgentKind が 3 系統ぶんの書き先を持つ （~/.claude/CLAUDE.md / ~/.codex/AGENTS.md / ~/.gemini/GEMINI.md） |
| **setup が tako の MCP サーバーをこの CLI へ恒久登録する**<br />`setup_mcp_register` | 対応 | 未対応 [#979](https://github.com/takushio2525/tako/issues/979)<br />tako の実装が claude 専用で、この系統への配線がまだ無い | 未対応 [#979](https://github.com/takushio2525/tako/issues/979)<br />tako の実装が claude 専用で、この系統への配線がまだ無い | 未対応 [#990](https://github.com/takushio2525/tako/issues/990)<br />ローカル LLM の系統がまだ成立していない（リポジトリに Ollama への参照が 1 件も無い） | コード本文: setup-mcp の書き先は ~/.claude.json と .mcp.json だけ（棚卸し §5）。codex / agy の設定ファイルへ書く経路が無い |

## オーケストレーター（master / solo）

| 能力 | Claude Code | OpenAI Codex CLI | Antigravity CLI | Local LLM | 根拠 |
| --- | --- | --- | --- | --- | --- |
| **master オーケストレーターとして起動する（`tako master`）**<br />`master_launch` | 対応 | 対応 | 未対応 [#987](https://github.com/takushio2525/tako/issues/987)<br />agy は worker 専用で、master / solo としては起動前にエラーになる（#127） | 未対応 [#991](https://github.com/takushio2525/tako/issues/991)<br />ローカル LLM の系統がまだ成立していない（リポジトリに Ollama への参照が 1 件も無い） | コード本文: orchestrator/mod.rs の build_master_cmd_in は claude / codex で分岐し、agy は unreachable!() で到達しない（#127 の設計判断。前提の再評価は #987） |
| **1 対 1 対話の solo として起動する（`tako solo`）**<br />`solo_launch` | 対応 | 対応 | 未対応 [#987](https://github.com/takushio2525/tako/issues/987)<br />agy は worker 専用で、master / solo としては起動前にエラーになる（#127） | 未対応 [#991](https://github.com/takushio2525/tako/issues/991)<br />ローカル LLM の系統がまだ成立していない（リポジトリに Ollama への参照が 1 件も無い） | コード本文: solo は master と同じ build_master_cmd_in を通るので、agy は同じ理由で 起動前にエラーになる（#111 / #127） |
| **master の system prompt がモデルへ届く**<br />`master_system_prompt` | 対応 | 対応 | 未対応 [#987](https://github.com/takushio2525/tako/issues/987)<br />agy は worker 専用で、master / solo としては起動前にエラーになる（#127） | 未対応 [#991](https://github.com/takushio2525/tako/issues/991)<br />ローカル LLM の系統がまだ成立していない（リポジトリに Ollama への参照が 1 件も無い） | コード本文: claude は --append-system-prompt-file、codex は developer_instructions で注入する （orchestrator/mod.rs）。agy の注入手段（custom agent 定義の起動時選択）は 公式ドキュメントに記載が無く #987 で実機確認する |
| **master が tako の MCP ツール群を呼べる**<br />`master_mcp` | 対応 | 対応 | 未対応 [#987](https://github.com/takushio2525/tako/issues/987)<br />agy は worker 専用で、master / solo としては起動前にエラーになる（#127） | 未対応 [#991](https://github.com/takushio2525/tako/issues/991)<br />ローカル LLM の系統がまだ成立していない（リポジトリに Ollama への参照が 1 件も無い） | コード本文: orchestrator/mod.rs が codex へ -c mcp_servers.tako.* を起動時に一時注入する （恒久登録はしない = tako 外の codex にツールを出さない。FR-2.3.2） |
| **master の引き継ぎ（後任の spawn と管轄の受け渡し）が通る**<br />`master_handoff` | 対応 | 対応 | 未対応 [#987](https://github.com/takushio2525/tako/issues/987)<br />agy は worker 専用で、master / solo としては起動前にエラーになる（#127） | 未対応 [#991](https://github.com/takushio2525/tako/issues/991)<br />ローカル LLM の系統がまだ成立していない（リポジトリに Ollama への参照が 1 件も無い） | コード本文: handoff の後任は build_master_cmd_in（orchestrator/mod.rs）を通るので master が起動できる系統では通る。agy はその関数に到達しない |
| **ctx% が閾値を超えたら自分で引き継ぐ（#749）**<br />`master_auto_handoff` | 対応 | 未対応 [#984](https://github.com/takushio2525/tako/issues/984)<br />この系統に同等の手段があるかを実物で調べていない（無いと確定したわけではない） | 未対応 [#987](https://github.com/takushio2525/tako/issues/987)<br />agy は worker 専用で、master / solo としては起動前にエラーになる（#127） | 未対応 [#991](https://github.com/takushio2525/tako/issues/991)<br />ローカル LLM の系統がまだ成立していない（リポジトリに Ollama への参照が 1 件も無い） | コード本文: #749 の発火材料は画面由来の ctx%。codex 側のパターンは terminal.rs にあるが 採取 fixture が無く、実際に描画されるかは未確認（棚卸し §10 の 2 番） |
| **コンテキスト残量を画面から読み取れる**<br />`master_ctx_percent` | 対応 | 一部対応<br />worker の状態照会では構造化ソースから ctx% が取れるが、master の自動ハンドオフは画面のパターンを見るので master 経路では未確認 | 未対応 [#984](https://github.com/takushio2525/tako/issues/984)<br />agy は会話を SQLite で持つため（~/.gemini/antigravity-cli/conversations/）読むには新しい依存が要る。生存は presence のロックで分かるがターンの開始・完了は取れない | 未対応 [#991](https://github.com/takushio2525/tako/issues/991)<br />ローカル LLM の系統がまだ成立していない（リポジトリに Ollama への参照が 1 件も無い） | 実測: #984: rollout の token_count に last_token_usage.total_tokens と model_context_window があり、worker_status の ctx_percent へ載せた（実測で 8%）。master の #749 は terminal.rs の画面パターンを見る別経路なのでそこは未確認 |

## worker の起動

| 能力 | Claude Code | OpenAI Codex CLI | Antigravity CLI | Local LLM | 根拠 |
| --- | --- | --- | --- | --- | --- |
| **worker として起動できる**<br />`worker_spawn` | 対応 | 対応 | 対応 | 未対応 [#990](https://github.com/takushio2525/tako/issues/990)<br />ローカル LLM の系統がまだ成立していない（リポジトリに Ollama への参照が 1 件も無い） | コード本文: orchestrator/agent.rs の build_worker_cmd_in が唯一の組み立て口で、effort / 権限スキップ / role 注入の 3 点だけを系統別に分岐する |
| **worker を立てるときに系統を選べる（設定の書き換えや再起動なしで）**<br />`agent_select_at_spawn` | 対応 | 対応 | 対応 | 未対応 [#990](https://github.com/takushio2525/tako/issues/990)<br />ローカル LLM の系統がまだ成立していない（リポジトリに Ollama への参照が 1 件も無い） | コード本文: orchestrator/agent.rs の WorkerAgent が spawn 引数・プロファイルの両方から 解決され、build_worker_cmd_in が 3 系統ぶんのコマンドを組む。ペイン単位・タスク単位の切替導線は #988 |
| **作業フォルダを起動前に信頼済みにしておく（信頼ダイアログで止まらない）**<br />`worker_trust` | 対応 | 一部対応<br />設定ファイルの場所が固定なので、tako のアカウント切替がこの系統には効かない | 一部対応<br />設定ファイルの場所が固定なので、tako のアカウント切替がこの系統には効かない | 未対応 [#990](https://github.com/takushio2525/tako/issues/990)<br />ローカル LLM のハーネスが決まっていないので可否が定まらない（codex TUI を借りる #990 なら在り、非 TUI 経路の #991 なら無い） | コード本文: orchestrator/agent.rs の ensure_trusted_in が 3 系統ぶん書き分けてある （claude = <config dir>/.claude.json / codex = ~/.codex/config.toml / agy = ~/.gemini/antigravity-cli/settings.json）。claude 以外は固定パス |
| **起動直後の Bypass 確認ダイアログを事前に承諾しておく（#407）**<br />`worker_bypass_preaccept` | 対応 | 未対応 [#983](https://github.com/takushio2525/tako/issues/983)<br />tako の実装が claude 専用で、この系統への配線がまだ無い | 未対応 [#983](https://github.com/takushio2525/tako/issues/983)<br />tako の実装が claude 専用で、この系統への配線がまだ無い | 未対応 [#991](https://github.com/takushio2525/tako/issues/991)<br />ローカル LLM のハーネスが決まっていないので可否が定まらない（codex TUI を借りる #990 なら在り、非 TUI 経路の #991 なら無い） | コード本文: dispatch.rs の事前承諾は 2 箇所とも WorkerAgent::Claude を条件にしている。codex / agy は default_skip_permissions() が true なので常に skip 側なのに 事前承諾が無い（棚卸し §1.3(c)） |
| **thinking / reasoning effort を tako から指定する**<br />`effort_control` | 対応 | 対応 | 対象外<br />agy は effort を CLI から指定できない（モデル名の "(High)" 等に組み込まれている） | 未対応 [#990](https://github.com/takushio2525/tako/issues/990)<br />ローカル LLM の系統がまだ成立していない（リポジトリに Ollama への参照が 1 件も無い） | コード本文: orchestrator/agent.rs: claude は --effort、codex は -c model_reasoning_effort= へ 写像する。agy は effort_options() が空で、コマンド組み立ても何も付けない |
| **アカウント（資格情報）の切替に追従する**<br />`account_switch` | 対応 | 未対応 [#975](https://github.com/takushio2525/tako/issues/975)<br />設定ファイルの場所が固定なので、tako のアカウント切替がこの系統には効かない | 未対応 [#975](https://github.com/takushio2525/tako/issues/975)<br />設定ファイルの場所が固定なので、tako のアカウント切替がこの系統には効かない | 未対応 [#990](https://github.com/takushio2525/tako/issues/990)<br />ローカル LLM の系統がまだ成立していない（リポジトリに Ollama への参照が 1 件も無い） | コード本文: orchestrator/agent.rs の事前信頼は claude だけ CLAUDE_CONFIG_DIR（#512 / #558）を 見て書き先を決め、codex は ~/.codex/config.toml、agy は ~/.gemini/antigravity-cli/settings.json を固定で開く（同ファイルのコメントが明示） |

## worker の監視

| 能力 | Claude Code | OpenAI Codex CLI | Antigravity CLI | Local LLM | 根拠 |
| --- | --- | --- | --- | --- | --- |
| **作業中か終わったかを判定する**<br />`worker_status_detect` | 対応 | 対応 | 一部対応<br />状態が画面推定のみなので完了の確定が遅い（同じ判定を 8 回続けて見る必要がある。claude は 3 回） | 未対応 [#991](https://github.com/takushio2525/tako/issues/991)<br />ローカル LLM の系統がまだ成立していない（リポジトリに Ollama への参照が 1 件も無い） | 実測: #984: codex は構造化ソース（codex-session）を得たので need_streak が 8 → 3 に なり claude と同じ確定速度になる。同一タスクの A/B 実測（primes 25 個）で before = source=screen / ctx=None / **開始前の t=3s・6s に idle を出す**、after = t=9s から source=codex-session で busy を 2 標本とも捉え t=15s から idle + ctx=8。agy は画面推定のままだが、弱マーカーを agent 別に分離したので (Thinking) 型の誤爆は構造的に起こらない（残る差は確定までの回数だけ） |
| **画面に依らない一次シグナルで状態を取れる（`claude agents --json` 相当）**<br />`worker_status_structured` | 対応 | 対応 | 未対応 [#984](https://github.com/takushio2525/tako/issues/984)<br />agy は会話を SQLite で持つため（~/.gemini/antigravity-cli/conversations/）読むには新しい依存が要る。生存は presence のロックで分かるがターンの開始・完了は取れない | 未対応 [#991](https://github.com/takushio2525/tako/issues/991)<br />ローカル LLM の系統がまだ成立していない（リポジトリに Ollama への参照が 1 件も無い） | 実測: #984 で codex-cli 0.150.1 を実物調査: $CODEX_HOME/sessions/ の rollout JSONL に task_started / task_complete が**逐次**書かれる（250 語生成を 1 秒刻みで観測: t=1s 開始 → t=27s 完了）。tako は status_source=codex-session として読む。agy は会話が SQLite（~/.gemini/antigravity-cli/conversations/<id>.db）で、生存は presence/<id>.lock で分かるがターンの開始・完了は取れない |
| **プロンプトが届かなかったことを検知して再送手段を出す（#390 / #530）**<br />`worker_prompt_undelivered` | 対応 | 未対応 [#983](https://github.com/takushio2525/tako/issues/983)<br />tako の実装が claude 専用で、この系統への配線がまだ無い | 未対応 [#983](https://github.com/takushio2525/tako/issues/983)<br />tako の実装が claude 専用で、この系統への配線がまだ無い | 未対応 [#991](https://github.com/takushio2525/tako/issues/991)<br />ローカル LLM の系統がまだ成立していない（リポジトリに Ollama への参照が 1 件も無い） | コード本文: orchestrator/registry.rs の prompt_delivery_assessment は agent が claude でなければ NotApplicable で即返るので、非 claude の未達は観測されない |
| **突然死を検知して復旧コマンドを提示する（#390）**<br />`worker_death_resume` | 対応 | 未対応 [#984](https://github.com/takushio2525/tako/issues/984)<br />tako の実装が claude 専用で、この系統への配線がまだ無い | 未対応 [#984](https://github.com/takushio2525/tako/issues/984)<br />tako の実装が claude 専用で、この系統への配線がまだ無い | 未対応 [#991](https://github.com/takushio2525/tako/issues/991)<br />ローカル LLM の系統がまだ成立していない（リポジトリに Ollama への参照が 1 件も無い） | コード本文: dispatch.rs のレジストリの resume_command はコメントどおり claude のみ （session ID から claude --resume を組む） |

## worker への指示と応答

| 能力 | Claude Code | OpenAI Codex CLI | Antigravity CLI | Local LLM | 根拠 |
| --- | --- | --- | --- | --- | --- |
| **初期プロンプトが送達確認つきで届く（#32 / #530）**<br />`worker_prompt_delivery` | 対応 | 対応 | 対応 | 未対応 [#991](https://github.com/takushio2525/tako/issues/991)<br />ローカル LLM の系統がまだ成立していない（リポジトリに Ollama への参照が 1 件も無い） | コード本文: 第 2 層のキー操作経路（claude_tui::deliver_via_tmux）は 3 系統の入力欄 （❯ / › / >）を見分けるので agent 非依存に動く |
| **画面を介さずに指示を直送する（生成中でも取りこぼさない。#790）**<br />`worker_delivery_peer` | 対応 | 対象外<br />画面を介さない直送は claude の受信箱（Cross-Session Messaging）に固有の仕組みで、他系統には相当物が無い | 対象外<br />画面を介さない直送は claude の受信箱（Cross-Session Messaging）に固有の仕組みで、他系統には相当物が無い | 対象外<br />画面を介さない直送は claude の受信箱（Cross-Session Messaging）に固有の仕組みで、他系統には相当物が無い | 上流の仕様: 第 1 層は claude の Cross-Session Messaging（受信箱の socket へ直送）に固有。AGENTS.md「worker への指示送達（#790）」も codex / agy / Windows は常に 第 2 層と明記している |
| **permission ダイアログを検知して応答する（#319 / #577）**<br />`worker_permission_dialog` | 対応 | 対応 | 対応 | 未対応 [#991](https://github.com/takushio2525/tako/issues/991)<br />ローカル LLM のハーネスが決まっていないので可否が定まらない（codex TUI を借りる #990 なら在り、非 TUI 経路の #991 なら無い） | コード本文: claude_tui.rs の detect_permission_dialog は 3 系統のパターンを持ち、agy の「Do you want to proceed?」も対象に入っている |
| **選択肢ダイアログを構造として読み、番号やラベルで応答する（#748）**<br />`worker_choice_dialog` | 対応 | 対応 | 対応 | 未対応 [#991](https://github.com/takushio2525/tako/issues/991)<br />ローカル LLM のハーネスが決まっていないので可否が定まらない（codex TUI を借りる #990 なら在り、非 TUI 経路の #991 なら無い） | コード本文: claude_tui.rs は claude v2.1.198 / codex 0.144.1 / agy 1.1.0 の実採取画面の 和集合として実装され、CODEX_TRUST_DIALOG / AGY_PERMISSION_DIALOG 等の fixture が同ファイルに在る |
| **worker ペインの中から tako CLI で tako を操作できる**<br />`worker_cli_control` | 対応 | 対応 | 対応 | 未対応 [#991](https://github.com/takushio2525/tako/issues/991)<br />ローカル LLM の系統がまだ成立していない（リポジトリに Ollama への参照が 1 件も無い） | コード本文: TAKO_PANE_ID / TAKO_SOCKET / TAKO_TOKEN の注入と PATH 注入（#601）は ペイン単位で agent に依らない |
| **worker が tako の MCP ツール群を呼べる**<br />`worker_mcp` | 対応 | 未対応 [#986](https://github.com/takushio2525/tako/issues/986)<br />tako の実装が claude 専用で、この系統への配線がまだ無い | 未対応 [#986](https://github.com/takushio2525/tako/issues/986)<br />tako の実装が claude 専用で、この系統への配線がまだ無い | 未対応 [#991](https://github.com/takushio2525/tako/issues/991)<br />ローカル LLM の系統がまだ成立していない（リポジトリに Ollama への参照が 1 件も無い） | コード本文: mcp_servers を組む非テストコードは orchestrator/mod.rs（master 経路）だけで、WorkerLaunch には tako_bin も MCP 引数も無い（棚卸し §5.3 = 最大の穴） |

## 報告と会話ログ

| 能力 | Claude Code | OpenAI Codex CLI | Antigravity CLI | Local LLM | 根拠 |
| --- | --- | --- | --- | --- | --- |
| **画面の履歴から報告を取れる（#364 の第 1 層）**<br />`worker_report_scrollback` | 対応 | 対応 | 対応 | 未対応 [#991](https://github.com/takushio2525/tako/issues/991)<br />ローカル LLM の系統がまだ成立していない（リポジトリに Ollama への参照が 1 件も無い） | コード本文: 第 1 層は器の capture（capture-pane -p -J -S）なので agent に依らない （dispatch.rs の report が明記） |
| **構造化された会話ログから報告を取れる（#364 の第 2 層 / `--messages`）**<br />`worker_report_transcript` | 対応 | 対応 | 未対応 [#984](https://github.com/takushio2525/tako/issues/984)<br />agy は会話を SQLite で持つため（~/.gemini/antigravity-cli/conversations/）読むには新しい依存が要る。生存は presence のロックで分かるがターンの開始・完了は取れない | 未対応 [#991](https://github.com/takushio2525/tako/issues/991)<br />ローカル LLM の系統がまだ成立していない（リポジトリに Ollama への参照が 1 件も無い） | 実測: #984 で codex アダプタを実装。rollout JSONL の response_item（role=assistant）を 読むので `report --messages N` が codex でも実データを返す。応答の transcript_agent でどちらを読んだか分かる。agy は会話が SQLite なので未対応 |
| **会話がセッションカタログに索引される（#112）**<br />`sessions_catalog` | 対応 | 一部対応<br />spawn の記録は残るが、会話の実体を索引できないので pending のまま期限切れで消える | 一部対応<br />spawn の記録は残るが、会話の実体を索引できないので pending のまま期限切れで消える | 未対応 [#991](https://github.com/takushio2525/tako/issues/991)<br />ローカル LLM の系統がまだ成立していない（リポジトリに Ollama への参照が 1 件も無い） | コード本文: sessions.rs の昇格は claude のセッション検出（transcript）に依存する。3 系統とも spawn 時に pending 記録は作られるが、claude 以外は昇格しない |
| **過去の会話を復元して続ける（`tako sessions resume`）**<br />`sessions_resume` | 対応 | 未対応 [#984](https://github.com/takushio2525/tako/issues/984)<br />tako の実装が claude 専用で、この系統への配線がまだ無い | 未対応 [#984](https://github.com/takushio2525/tako/issues/984)<br />tako の実装が claude 専用で、この系統への配線がまだ無い | 未対応 [#991](https://github.com/takushio2525/tako/issues/991)<br />ローカル LLM の系統がまだ成立していない（リポジトリに Ollama への参照が 1 件も無い） | コード本文: dispatch.rs の resume は claude --resume <session_id> を組み、~/.claude/projects の transcript を前提にする（claude 以外は分類済みエラーで 手動の代替を案内する） |

## 利用制限

| 能力 | Claude Code | OpenAI Codex CLI | Antigravity CLI | Local LLM | 根拠 |
| --- | --- | --- | --- | --- | --- |
| **利用上限で止まったことを検知する**<br />`worker_limit_detect` | 対応 | 対応 | 未対応 [#985](https://github.com/takushio2525/tako/issues/985)<br />この系統に同等の手段があるかを実物で調べていない（無いと確定したわけではない） | 対象外<br />自分のマシンで動かすモデルなので利用上限という概念が無い | コード本文: claude_tui.rs の usage limit 分類は claude の「What do you want to do?」と codex の「Approaching rate limits」を実採取して持つ。agy の記述は無い （#357 が調べたのはメトリクス表示で、停止ダイアログの有無は別問題） |
| **利用上限の解除後に自分で再開する（#813）**<br />`worker_limit_autoresume` | 対応 | 一部対応<br />上限で止まったことは検知できるが、ダイアログ型のため自動復帰まで到達していない（#985） | 未対応 [#985](https://github.com/takushio2525/tako/issues/985)<br />この系統に同等の手段があるかを実物で調べていない（無いと確定したわけではない） | 対象外<br />自分のマシンで動かすモデルなので利用上限という概念が無い | コード本文: limit_stop.rs は claude の idle 型と codex のダイアログ型を分類するが、自動復帰まで到達するのは claude 経路だけ。agy のパターンは 1 件も無い |
| **利用制限の残量（%）を取り出す（#357）**<br />`worker_limit_metrics` | 対応 | 一部対応<br />抽出パターンはあるが、実データが出るのは有料プランのみで未実測（#357 の残課題） | 対象外<br />agy は利用制限の残量を表示・出力しないため取得できない（#357 で実地確認） | 対象外<br />自分のマシンで動かすモデルなので利用上限という概念が無い | 実測: #357: codex の primary / secondary NN% はスクレイピング実装済みだが有料プラン 限定で未実測、agy は v1.1.4 の実地調査で取得不能と確定（再確認は #985）。**#984 の副産物**: codex の rollout JSONL の token_count イベントに rate_limits.primary.used_percent / window_minutes / resets_at が構造化されて 入っている（実測）ので、画面スクレイピングに頼らず取れる。配線は #985 |
| **ステータスバーの利用制限表示をこの系統へ切り替えられる（#217 / #357）**<br />`limit_service_switch` | 対応 | 一部対応<br />抽出パターンはあるが、実データが出るのは有料プランのみで未実測（#357 の残課題） | 対象外<br />agy は利用制限の残量を表示・出力しないため取得できない（#357 で実地確認） | 未対応 [#990](https://github.com/takushio2525/tako/issues/990)<br />ローカル LLM の系統がまだ成立していない（リポジトリに Ollama への参照が 1 件も無い） | 実測: #357 の完了報告: codex は TUI の primary / secondary NN% を抽出できるが実データは 有料プラン限定で未実測、agy は取得不能を実地確認して unsupported を明示表示にした |

## その他

| 能力 | Claude Code | OpenAI Codex CLI | Antigravity CLI | Local LLM | 根拠 |
| --- | --- | --- | --- | --- | --- |
| **コンフリクト解消エージェントとして起動する（#496）**<br />`git_resolve_agent` | 対応 | 一部対応<br />起動はできるが tako の MCP ツールを呼べない（#986） | 一部対応<br />起動はできるが tako の MCP ツールを呼べない（#986） | 未対応 [#990](https://github.com/takushio2525/tako/issues/990)<br />ローカル LLM の系統がまだ成立していない（リポジトリに Ollama への参照が 1 件も無い） | コード本文: dispatch.rs の git resolve は 3 系統とも起動できるが、worker と同じ経路なので MCP の一時注入が無い（mcp_servers を組むのは orchestrator/mod.rs の master 側だけ） |

## この表の作り方

正本は `crates/tako-core/src/agent_support.rs` の能力マトリクスです。
Claude Code 以外について「使える」「落ちる」「使えない」と書くときは根拠
（コード本文の引用・上流の仕様・実測の記録）を同時に書く必要があり、
書かずに倒すとテストが落ちます。

このページの再生成と同期検査は次のとおりです。

```sh
cargo build -p tako-cli
node scripts/gen-agent-support-docs.mjs          # 再生成
node scripts/gen-agent-support-docs.mjs --check   # 同期検査
```
