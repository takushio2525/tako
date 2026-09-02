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

能力 43 件の内訳です。

| エージェント | 対応 | 一部対応 | 未対応 | 対象外 |
| --- | --- | --- | --- | --- |
| Claude Code（基準） | 43 / 43 | 0 | 0 | 0 |
| OpenAI Codex CLI | 27 / 43 | 5 | 10 | 1 |
| Antigravity CLI | 12 / 43 | 7 | 18 | 6 |
| Local LLM | 0 / 43 | 0 | 38 | 5 |

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
| **setup が tako の MCP サーバーをこの CLI へ恒久登録する**<br />`setup_mcp_register` | 対応 | 対応 | 対応 | 未対応 [#990](https://github.com/takushio2525/tako/issues/990)<br />ローカル LLM の系統がまだ成立していない（リポジトリに Ollama への参照が 1 件も無い） | 実測: #979（main の 63a7c26）で `tako setup-mcp` が 3 系統へ登録するようになった。書き先は claude = ~/.claude.json / codex = ~/.codex/config.toml の [mcp_servers.tako] / agy = ~/.gemini/config/mcp_config.json で、codex は env_vars 許可リストまで足して実セッションから tako_list_panes が通ることを実測。正本は tako-control::agent_mcp |
| **setup でモデルを選んでプロファイルへ反映できる（一覧は CLI から実取得し、一覧コマンドを持たない系統は同梱の既知リスト + 取得不可の明示。#1002）**<br />`setup_model_picker` | 対応 | 対応 | 対応 | 未対応 [#990](https://github.com/takushio2525/tako/issues/990)<br />ローカル LLM の系統がまだ成立していない（リポジトリに Ollama への参照が 1 件も無い） | 実測: #1002 の実測（2026-08-27）: codex 0.150.1 は `codex debug models` が `Render the raw model catalog as JSON` で slug / display_name / supported_reasoning_levels / context_window を返す（未認証でも既定カタログ、 認証すると内容が変わる）。agy 1.1.22 は `agy models` が `id<TAB>表示名` の TSV を stdout へ返し未認証は exit 1 + `Please sign in to view available models.`。 claude 2.1.232 は該当サブコマンドが無く `claude models` は**プロンプトとして 解釈される**（一覧はセッション内の /model のみ） |

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
| **thinking / reasoning effort を tako から指定する**<br />`effort_control` | 対応 | 対応 | 対応 | 未対応 [#990](https://github.com/takushio2525/tako/issues/990)<br />ローカル LLM の系統がまだ成立していない（リポジトリに Ollama への参照が 1 件も無い） | 実測: #1002 の実測（agy 1.1.22）: `--effort（low\|medium\|high）` が --help に実在し、`agy models` が挙げる 6 モデルすべてで不正値が `invalid --effort "bogus" (valid: low, medium, high)` として咎められる = 表示名に "(High)" 等を含むモデルでも --effort の検証が走る。正しい組み合わせは 検証を通り API 呼び出しへ進む。**未知のモデル名のときだけ** `--effort is not supported for model "…"` になる（この文言を「agy は effort 非対応」と 読み違えないこと）。orchestrator/agent.rs は claude = --effort / codex = -c model_reasoning_effort= / agy = --effort へ写像する（旧挙動は TAKO_1002_LEGACY=1） |
| **アカウント（資格情報）の切替に追従する**<br />`account_switch` | 対応 | 未対応 [#975](https://github.com/takushio2525/tako/issues/975)<br />設定ファイルの場所が固定なので、tako のアカウント切替がこの系統には効かない | 未対応 [#975](https://github.com/takushio2525/tako/issues/975)<br />設定ファイルの場所が固定なので、tako のアカウント切替がこの系統には効かない | 未対応 [#990](https://github.com/takushio2525/tako/issues/990)<br />ローカル LLM の系統がまだ成立していない（リポジトリに Ollama への参照が 1 件も無い） | コード本文: orchestrator/agent.rs の事前信頼は claude だけ CLAUDE_CONFIG_DIR（#512 / #558）を 見て書き先を決め、codex は ~/.codex/config.toml、agy は ~/.gemini/antigravity-cli/settings.json を固定で開く（同ファイルのコメントが明示） |

## worker の監視

| 能力 | Claude Code | OpenAI Codex CLI | Antigravity CLI | Local LLM | 根拠 |
| --- | --- | --- | --- | --- | --- |
| **作業中か終わったかを判定する**<br />`worker_status_detect` | 対応 | 対応 | 一部対応<br />状態が画面推定のみなので完了の確定が遅い（同じ判定を 8 回続けて見る必要がある。claude は 3 回） | 未対応 [#991](https://github.com/takushio2525/tako/issues/991)<br />ローカル LLM の系統がまだ成立していない（リポジトリに Ollama への参照が 1 件も無い） | 実測: #984: codex は構造化ソース（codex-session）を得たので need_streak が 8 → 3 に なり claude と同じ確定速度になる。同一タスクの A/B 実測（primes 25 個）で before = source=screen / ctx=None / **開始前の t=3s・6s に idle を出す**、after = t=9s から source=codex-session で busy を 2 標本とも捉え t=15s から idle + ctx=8。agy は画面推定のままだが、弱マーカーを agent 別に分離したので (Thinking) 型の誤爆は構造的に起こらない（残る差は確定までの回数だけ） |
| **画面に依らない一次シグナルで状態を取れる（`claude agents --json` 相当）**<br />`worker_status_structured` | 対応 | 対応 | 未対応 [#984](https://github.com/takushio2525/tako/issues/984)<br />agy は会話を SQLite で持つため（~/.gemini/antigravity-cli/conversations/）読むには新しい依存が要る。生存は presence のロックで分かるがターンの開始・完了は取れない | 未対応 [#991](https://github.com/takushio2525/tako/issues/991)<br />ローカル LLM の系統がまだ成立していない（リポジトリに Ollama への参照が 1 件も無い） | 実測: #984 で codex-cli 0.150.1 を実物調査: $CODEX_HOME/sessions/ の rollout JSONL に task_started / task_complete が**逐次**書かれる（250 語生成を 1 秒刻みで観測: t=1s 開始 → t=27s 完了）。tako は status_source=codex-session として読む。agy は会話が SQLite（~/.gemini/antigravity-cli/conversations/<id>.db）で、生存は presence/<id>.lock で分かるがターンの開始・完了は取れない |
| **プロンプトが届かなかったことを検知して再送手段を出す（#390 / #530）**<br />`worker_prompt_undelivered` | 対応 | 対応 | 一部対応<br />送達を裏づける一次シグナルが無いので、猶予を過ぎても「未達」と断定せず「未確認」を返す（自動再送を撃つと二重指示になる） | 未対応 [#991](https://github.com/takushio2525/tako/issues/991)<br />ローカル LLM の系統がまだ成立していない（リポジトリに Ollama への参照が 1 件も無い） | テスト: #983 の変更 2 で prompt_delivery_assessment の判断を delivery_observation （このマトリクスの WORKER_STATUS_STRUCTURED）から引く形にした。codex は rollout の task_started を送達の証拠にできるので claude と同じく未達を断定し、agy は画面確認しか 無いので未達ではなく unverified（+ verify_then_resend）を返す。緑のテスト: registry の「一次シグナルの無い系統は未達と断定せず未確認を返す」「送達の観測手段はマトリクスから引く」「ターンが走った証拠は画面検証の失敗より強い」/ dispatch の「issue983_観測手段の無い系統でも送達判定が黙らない」 |
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
| **会話を保ったまま CLI プロセスだけ建て直す（#1067。CLI の自動更新に追いつく手段）**<br />`session_restart_harness` | 対応 | 未対応 [#984](https://github.com/takushio2525/tako/issues/984)<br />tako の実装が claude 専用で、この系統への配線がまだ無い | 未対応 [#984](https://github.com/takushio2525/tako/issues/984)<br />tako の実装が claude 専用で、この系統への配線がまだ無い | 未対応 [#991](https://github.com/takushio2525/tako/issues/991)<br />ローカル LLM の系統がまだ成立していない（リポジトリに Ollama への参照が 1 件も無い） | コード本文: session_restart の harness は sessions::resume_command（claude --resume）を              組んで送るので、resume を配線していない系統では成立しない              （手段自体は上流にある: codex resume / agy --conversation） |
| **引き継ぎを書かせてセッションを交代する（#1067。ペインの右クリック / `tako session-restart --mode handoff`）**<br />`session_restart_handoff` | 対応 | 未対応 [#1067](https://github.com/takushio2525/tako/issues/1067)<br />手段は揃っているが実機で確かめていない（claude で先行実装した） | 未対応 [#987](https://github.com/takushio2525/tako/issues/987)<br />agy は worker 専用で、master / solo としては起動前にエラーになる（#127） | 未対応 [#991](https://github.com/takushio2525/tako/issues/991)<br />ローカル LLM の系統がまだ成立していない（リポジトリに Ollama への参照が 1 件も無い） | コード本文: 引き継ぎ再起動は master ペインへ定型文を送り、エージェント自身が              tako_orchestrator_handoff を呼ぶ形（handoff.rs の restart_prompt）。             codex master は #979 で MCP が届くので成立しうるが未実測。             agy は master になれない（#987）ので対象そのものが無い |

## 利用制限

| 能力 | Claude Code | OpenAI Codex CLI | Antigravity CLI | Local LLM | 根拠 |
| --- | --- | --- | --- | --- | --- |
| **利用上限で止まったことを検知する**<br />`worker_limit_detect` | 対応 | 対応 | 対象外<br />agy の残量は前払いの AI クレジット残高で、5h / 週のような枠とリセット時刻が無い。残高も対話の /credits モーダルの中にしか出ないので、worker の画面を乱さずに読む口が無い（#985 で agy 1.1.22 を再調査） | 対象外<br />自分のマシンで動かすモデルなので利用上限という概念が無い | 実測: #985 実測（2026-08-27）: codex 0.150.1 の停止文言 `You've hit your usage limit.` と 接近ダイアログ `Approaching rate limits` をバイナリ内文字列で確認し、`limit_stop.rs` の実採取 fixture が両方を検知することをテストで固定した。agy 1.1.22 は**窓つきの利用上限を持たない**（`agy --help` に usage / quota 系の サブコマンドが無く、バイナリの `RateLimit` は全部 PR レビュー設定と Go / sentry の内部名。残量は `/credits` = 前払いクレジット）ので、検知すべき「上限で止まった状態」自体が存在しない |
| **利用上限の解除後に自分で再開する（#813）**<br />`worker_limit_autoresume` | 対応 | 一部対応<br />5h / 週の枠は解除を待って自分で再開するが、ワークスペースのクレジットが尽きた場合は「待つ」出口が無い（増枠申請・購入・獲得済みリセットの引き換えしか無いので、tako は何も選ばずに止まる） | 対象外<br />agy はクレジットを使い切っても「解除を待つ」出口が無い（買い足す導線しか無い）ので、待って再開するという動作が成立しない（#985） | 対象外<br />自分のマシンで動かすモデルなので利用上限という概念が無い | 実測: #985 実測（2026-08-27 / codex-cli 0.150.1）: codex の解除時刻は 2 つの経路で 取れる。① 画面の `Try again at Aug 28th, 2026 4:24 AM.`（バイナリ内書式 `" Try again at "` + `", %Y %-I:%M %p"`。日付を挟む形は #985 前は読めず、不明の猶予 900 秒で早撃ちして 3 回で諦めていた）② rollout の `rate_limits.<枠>.resets_at`（epoch 秒。書式にもタイムゾーンにも依存しない）。セルフテスト項目 111 の codex 節が解除前は撃たず解除後に再開するところまで見る （`TAKO_985_LEGACY=1` へ戻すと reset_at=None で FAILED になることを実測）。agy 1.1.22 は `/credits` に「待つ」出口が無く（Get More AI Credits / See Activity）、待って再開する動作そのものが成立しない |
| **利用制限の残量（%）を取り出す（#357）**<br />`worker_limit_metrics` | 対応 | 対応 | 対象外<br />agy の残量は前払いの AI クレジット残高で、5h / 週のような枠とリセット時刻が無い。残高も対話の /credits モーダルの中にしか出ないので、worker の画面を乱さずに読む口が無い（#985 で agy 1.1.22 を再調査） | 対象外<br />自分のマシンで動かすモデルなので利用上限という概念が無い | 実測: #985 実測（2026-08-27 / codex-cli 0.150.1 / plan_type = plus = **有料プラン**）: rollout の `token_count` に `rate_limits.primary`（`window_minutes: 300` = 5h）と `.secondary`（`10080` = 週）が数値で載る。**#357 の画面スクレイピングは 0.150.1 では成立しない**（実測: TUI のフッターはモデル名と cwd だけで、`5h limit: [██…] 90% left (resets 23:23)` は `/status` のモーダルの中にしか 出ない = 常時見えるところに `primary NN%` は無い）ので、構造化ソースが正になった。両者の解除時刻が一致することも確認（rollout の 1787840583 = 画面の 23:23）。agy 1.1.22 は前払いクレジットで枠が無い（`/credits` を実行して確認） |
| **ステータスバーの利用制限表示をこの系統へ切り替えられる（#217 / #357）**<br />`limit_service_switch` | 対応 | 対応 | 対象外<br />agy の残量は前払いの AI クレジット残高で、5h / 週のような枠とリセット時刻が無い。残高も対話の /credits モーダルの中にしか出ないので、worker の画面を乱さずに読む口が無い（#985 で agy 1.1.22 を再調査） | 未対応 [#990](https://github.com/takushio2525/tako/issues/990)<br />ローカル LLM の系統がまだ成立していない（リポジトリに Ollama への参照が 1 件も無い） | 実測: #985: ステータスバーの codex 表示は rollout の構造化データ（`rate_limits`）を 読む形になり、有料プランの実データが出る。agy は取得不能を再確認して unsupported の明示表示のまま（#357 の判断は理由を差し替えて維持） |

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
