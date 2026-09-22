# Progress Log

> AI が作業完了時に**末尾へ追記**する時系列ログ。新しいものほど下。
> **1 エントリは 1〜3 行**（何を / どこを / 結果）。詳細は git log・Issue・PR・`.agent/plans/` に委ねる。

このファイルは `AGENTS.md` から `@import` されるので**毎ターン全文が読み込まれる**。
予算（直近 5 作業日 / 20 エントリ / 12 KB）を超えたぶんは `progress-archive.md` へ
1 行で移る。移送は `tako context-budget fix` が行う（冪等・本文は改変しない・
全文は git 履歴に残る）。規約の全文は `AGENTS.md`「起動時ロードの予算」節。

## 追記フォーマット

```markdown

## YYYY-MM-DD（#Issue 一言）
- {何を / どこを / 結果}
- 関連コミット: `{shortsha}` `[種別] 概要`
- 次: {次にやることがあれば 1 行}
```

---

## 2026-09-22（#1502: tako CLI を外部ターミナルからも打てるようにした）
- `tako setup` の段として `$HOME/.local/bin/tako` へ symlink を張り、**その置き場所だけ**を `~/.zprofile` のマーカーブロック（1 組のまま）へ通す。**実体のディレクトリは PATH へ入れない**（`.app` の `Contents/MacOS` は tako-app ごと / dev の `target/debug` は依存クレートごと PATH へ出るうえ、`.app` を動かすと黙って切れる）。`$HOME/.local/bin` は 3 系統のランチャーと同じ置き場所なので macOS ではブロックの中身は 1 ディレクトリのまま = 既存ユーザーの profile の形が変わらない
- 途中で踏んだ真因 2 つ: ①claude が ready だとエージェントの PATH 段は走らないので「ついで」に載せると設置されない（= `run_setup` の独立した段にした）②「もう通っているか」を**プロセスの PATH** で測ると #601 の注入で必ず「通っている」に見える。`$SHELL -l -c` も**継承した PATH を `path_helper` が引き継ぐ**ので、launchd の既定 PATH へ戻してから起こす必要があった
- 実測: `scripts/test-tako-cli-path-1502.sh` **49 PASS 0 FAIL**（隔離 HOME + 隔離 GUI）。A/B `TAKO_1502_LEGACY=1` は 23 FAIL（B7 / B8「新しいログインシェルで tako が見つかる・動く」= Issue の症状）。番犬 `issue1502_tako_cli_path_watchdog` 6 本（注入 8 通りすべて FAILED → 戻して緑）

## 2026-09-22（#1516: `orchestrator self --pane N` が名指しどおりそのペインを答えるようにした）
- 真因は**入口が 2 種類の問いを同じ `pane` 欄へ混ぜていた**こと。受け手の解決順は「確かな順」= pid 祖先辿りが最優先で `pane` は stale になりうる env 由来として扱う契約（#288 / #210）なので、`--pane` は毎回黙って負けていた（pane 1954 から `--pane 1964` → `pane_id: 1954`）。MCP は caller_pid を持たないので pane_id は動くが `caller_role` が残り profile だけ呼び出し元のものになる
- 組み立てを `Request::orchestrator_self`（protocol.rs）の 1 本へ寄せ、**名指しなら呼び出し元の手掛かりを 1 つも載せない**。自分を名指しした場合は「私についての問い」に倒す（solo の profile は env の `solo:<名前>` にしか無い）。受け手は `named_pane` で名指しを見分け、解けなければ role 検索へ落とさず `PaneNotFound`（#1466）。応答の `role` は名乗り → 対象ペインのラベル、master / solo でなければ理由を `warnings` へ。`adopt` / `guide` / `handoff` の `--pane` は同型のまま（寄せるのは入口 1 か所で済む）
- 実測: 本番 GUI（旧バイナリ）に対して修正 CLI が `pane_id 1964 / profile <名指し先のもの> / profile_source pane_role`（出荷 CLI と `TAKO_1516_LEGACY=1` は 1967 = 症状）。`scripts/test-self-pane-1516.sh` **28 PASS 0 FAIL**（隔離 GUI・CLI / MCP 両経路 / A/B / エッジ 3 種）。実注入 6 通りすべて FAILED + file:line 名指し（CLI へ戻すと e2e が 11 NG）。番犬 `issue1516_named_pane_watchdog` 3 本・workspace 5047 passed 0 failed・clippy 3 宇宙 0・check-windows error 0

## 2026-09-22（#1518: bash 3.2 で落ちる空配列展開を 41 箇所直し、番犬で縛った）
- 走査は `set -u` 宣言だけでは足りなかった: `scripts/lib/*.sh` / `promo/lib.sh` は宣言側から source されて継ぐので 4 箇所が隠れ、逆にクォート付きヒアドキュメント（`promo/lib.sh` が書き出すデモ用スクリプト）の 3 箇所は囲む側が展開しないので対象外。**41 箇所 / 12 ファイル**へ `${arr[@]+"${arr[@]}"}` を適用（挙動は不変）
- 判定は実測で 1 点に絞った（`/bin/bash` 3.2.57・空配列・`set -u` で落ちるのは**演算子の無い** `${a[@]}` / `${a[*]}` だけ。`${#a[@]}` / `${!a[@]}` / `:-` / `:+` / `:1` / `#pat` / `/pat/rep` は通る）。番犬 `空配列の展開はbash32の慣用句で守られている` はリポジトリ全体の `.sh` を走査し、例外は `# tako:bash32-ok <理由>`（理由なしは無効）。走査を `scripts/` から広げた初回に #837 の実在バグ `distribution/build-pkg.sh:30` が出たので同時に直した
- 実測: 全 41 箇所を実ファイル本文から抽出して A/B（空 → 旧形は `unbound variable`・新形は通る / 非空 3 要素は 1 バイト同一）= 54/54。触った 13 本を `/bin/bash` 3.2 で副作用の無い経路から実走 27/27（`test-release-retry` 55/0・`test-wait-pr-checks` 137/0・`test-launch-services` 17/0 を含む）。注入 11 通りすべて file:line 名指し。実際に空が渡る経路は 4 系統（`check-windows.sh --all-targets` = 即死・`promo/lib.sh` の `PROMO_ENV_CLEAN` = 即死・`release.sh --promote` の `ASSETS`・`wait-pr-checks.sh:378` は `$( )` の中で死ぬので案内の中身だけ消える）。workspace 5045 passed 0 failed・clippy 3 宇宙 0

## 2026-09-22（#1501: 未認証・未導入でも setup が全段やって完走するようにした）
- 真因は **3 か所の早期 return**（`run_bootstrap_stage` の `?` / `missing_required` / 選択系統の未認証）で、一番手前が認証段（#1129 で代行を禁じた所）なので新品環境では `profiles/default.yaml`・`~/.claude/CLAUDE.md`・テンプレ・MCP 登録が**全部未作成**のまま exit 1（#1500 の R1 / R2 / R3）。判断を `tako_control::setup_remaining`（`summarize` / `render` / `for_step` / `legacy_stop` = 理由つき純粋関数）へ寄せ、段は `Vec<Remaining>` を返し、CLI は積んで最後に「残り N 件 + 次に打つ 1 行」を出すだけ。`--check` も同じ 1 実装・`setup.completed` は残りがあっても記録（再実行は残りから再開）
- 要確認だった点を実測で決着: **未認証の実 claude 2.1.258 でも `claude mcp add --scope user` は通る**（隔離 HOME / `loggedIn:false` → rc 0・`mcpServers.tako` が載る）ので MCP は持ち越さず登録する。ついでに判明 = **`scripts/verify-setup-multiagent.sh` は main で既に赤**（未認証の想定が #868 の段順より古く Z2 が先に出る + codex / agy スタブが #979 の `mcp add` を「予期しない起動」と数える）ので #1493 の規約どおり同じコミットで現行契約へ直して全緑
- 実測: `scripts/test-setup-continue-1501.sh` **81 PASS 0 FAIL**（CI 登録。非 TTY / `--yes` / TTY / dispatch 同条件 / `--check` / 冪等 / 認証済み A/B / エージェントゼロ / 必須依存 / Z3 / #1499・#1502 との同居）。A/B `TAKO_1501_LEGACY=1` は exit 1 で何も整わない = 症状。番犬 7 本・注入 12 通りすべて FAILED → 戻して緑。workspace 5087 passed 0 failed・clippy 3 宇宙 0・check-windows error 0

## 2026-09-22（#1509: remote setup の Tailscale 導入を setup_deps の 1 実装へ寄せた）
- #1499 は判断（`offer_for`）と実行（`install`）を共有しただけで**表示と確認は呼び手に残していた**ので、`remote setup` の [1/5] は `brew install tailscale` を自前で組み直したままだった。案内 → `[y/N]` → 導入 → 再検出のひと続きを `setup_deps::offer_and_install`（字下げだけ呼び手が渡す）へ移し、1 件だけ引く `status_of` を追加。`remote_setup.rs` から素の `Command::new("brew")` が消えたので **Windows のコンソール窓を出す起動も 1 件減った**（`platform_parity` の表を更新）
- 割れていた 4 点を出荷版との A/B で実測: brew が無い機は**聞いてから起動に失敗して中断**（新: 聞かずに `要 Homebrew`）／非 TTY は EOF を N と読むだけ（新: 理由つき案内）／「入れたのに引けない」は案内なしで中断／どこへ入るかを出さない。MCP の非対話経路は従来どおり導入しない
- 実測: `scripts/test-remote-setup-deps-1509.sh` **49 PASS 0 FAIL**（CI 登録。隔離 HOME + brew スタブ + `TAKO_TAILSCALE_BIN` で未導入を偽装。[2/5] 未ログインで止まるので serve に触れない）・#1499 回帰 52 PASS・番犬 3 本で注入 5 通りすべて FAILED → 戻して緑・workspace 5081 passed 0 failed・clippy 3 宇宙 0・check-windows error 0

## 2026-09-22（#1524: 標準 tako setup の依存 [y/N] を setup_deps::offer_and_install の 1 呼び出しへ寄せた）
- #1499 は判断 / 実行 / 再検出を寄せたが**表示と入力は呼び手に残し**、#1509 が束ねた `offer_and_install` も #1501 と同時進行だったため標準 setup は差分ゼロのままだった（体験を組む層が 2 か所）。`setup.rs` の `offer_dep_install` / `print_dep_install_plan` / `print_dep_manual_hint` を消し、`DepPromptIo`（stderr + stdin + 字下げ 6 マス）を渡す 1 呼び出しへ。再検出も中で済むので `resolve` の 2 度引き（ログインシェル起動）と `find_command("brew")` の先引きが各 1 回消えた
- 同時に判明 = `[警告] … インストール後も検出できません` は**到達しない分岐**だった（導入器が成功して引けないときは `install` が `Err` を返し `[警告] {e}` 側へ落ちる）。実測でも before / after とも 0 件なので削除した
- 実測: 隔離実走 13 本の出力が**バイト単位で一致**（文面の差分 0）。`test-setup-deps-prompt-1499.sh` 52 PASS / `test-remote-setup-deps-1509.sh` 49 PASS / `test-setup-continue-1501.sh` 81 PASS / `test-tako-cli-path-1502.sh` 49 OK。番犬 `issue1524_setup_prompt_single_impl_watchdog` 4 本・注入 10 通りすべて FAILED → 戻して緑。workspace 5107 passed 0 failed・clippy 3 宇宙 0・check-windows error 0

## 2026-09-22（#1525: AGENTS.md の予算の余地を作り、activeContext を現在状態へ戻した）
- 予算の主因はコマンド表ではなく**リリース運用の本文**（71 行 6121 バイト）だった。両 OS 同時 / 夜間リリース / 版数の予約を `.agent/release.md`（新設）へ移し、AGENTS.md には不変条件 2 行 + バックティック参照だけを残した（`@import` にはしない）。表からは実測値（45c / 95c）と診断オプション列挙の 2 行ぶんを `commands.md` の同じ行へ寄せた
- 実測: **30657 → 25309 バイト**（上限 30720 の 99.8% → 82.4%）。消えた実質 65 行のうち 63 行は release.md に全文一致で残り、残る 2 行は commands.md 側にセル単位で全文あり = **消えた情報 0**。`context_budget` 11 passed / `no_personal_data` 6 passed / fmt 差分なし / docs 生成 2 本とも同期
- `.agent/activeContext.md` は 9/22 の状態（main = `7bfeb6c` / 着地 8 件 / #1500 のレーン A・C / 判断待ち 4 件）へ 77 行で書き直した
