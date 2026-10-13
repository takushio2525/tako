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

## 2026-10-09（#1877 S7 設計 + 試作: 描画の主を mod へ・setup での導入・定型の UI 設定）
- 設計書 `.agent/plans/2026-10-tako-mod.md` §9。本物の認証を写さず偽 Messages API + 一時の設定 dir の実物 2.1.294 で試作: カード（ToolGroup を展開して ToolUse を描き替え・Copy は改行なしの論理文字列）・`/compact` のワンボタン・PromptHint の 5h / 7d / ctx バー・吹き出し。tako はクリックを TUI へ渡さない・S3 の帯が他の mod の帯を消すのを実測
- 導入は `<設定 dir>/skills/tako/` への管理印つきの写し（settings 系は書かない・ホットリロード）+ env 注入を残す（同名は env だけ読まれる）。marketplace は settings.json を書き直し外しても残骸 = 不採用。子 Issue #1958〜#1965

## 2026-10-10（#1962: tako mod S7-3 = 帯を next で包む 1 行・カスタムボタン・使用制限 / ctx のバーを mod で描き、描いている間はステータスバーの claude の区画を止める）
- 帯は他の mod の行を包み tako の行はボタン込み 1 行（`register.ts` の `bandRow` / `stackBand`）、バーは PromptHint の tail（描くかは `claude_mod::usage_bar_decision` = statusLine の有無 × 画面の数値・mod 自身のバーは数えない）、ボタンの argv は tako が組む（`TakoOp::cli_args`）。報告に `renders` / `status_line` / `last_press`、フォーカス中のペインの mod がバーを描けば `claude_bar_owner` で区画を外す（`tako mod` / `tako limit-service --refresh` の `status_bar`）。A/B `TAKO_1877_S7_LEGACY=1`
- 実測: `scripts/test-mod-band-buttons-1962.sh` 61 PASS 0 FAIL（実 claude 2.1.294・一時の設定 dir + 偽 Messages API・本物の ~/.claude* は前後で同じ）・`claude plugin test` 44 本・visual-test `mod-bar`（旧は ① で FAILED）・番犬 `issue1962_mod_band_buttons_watchdog`。罠: `kill -STOP` した claude 2.1.294 はすぐ起き直す

## 2026-10-09（#1933: 夜間の mod 検査のモックテストの間欠的な赤 = 落ちた段の手掛かりを出し、手掛かりで見つけた 2 つの赤を直した。CI の 1 回の原因は未確定）
- CI の 5 件は「Test 2 の注入前の回で検査本体が 0 / 3 以外で終わった」ときだけ揃う（注入で一致。記録・通知の漏れ / 未実測 / ロックは不一致）。`stage` / `dump_stages` が段の出力・記録・通知・スタブの呼び出し・温めを出す（`TAKO_1933_INJECT`）。手元で再現せず（夜間 300・単体 500・`run_step` 1500・git 並走 60 回で 0）
- 直した赤: ① `printf | grep -q` の assert が pipefail 下で取りこぼす（64 KB 超の注入で旧 5/5・新 0/5、Test 0）② 作りたてのスタブの初回起動（実測 1 回目 219 ms・2 回目 9 ms）が負荷下で Test 5 の上限 2 秒を超える → make_env で温める（初回 3 秒の注入で旧 4 FAIL・新 0）

## 2026-10-09（#1921: release-windows の rust-cache を外し、タグごとに一度も当たらないキャッシュを毎晩約 1.6 GB 保存して main / PR のキャッシュを押し出すのを止めた）
- 公式文書「タグの run は別のタグのキャッシュを読めない（読めるのは自分の ref と main）」と、v0.8.16〜v0.8.28 の 13 回すべての「No cache found.」+ 保存 1.60〜1.61 GB を実測。main は debug のキャッシュだけで release の target に効かない（依存の取得 約 60 秒 / その復元 約 55 秒）ので (a) save-if: false ではなく (b) 外す
- `scripts/test-cargo-root-manifest-key-1915.sh` に「タグで起動しうる workflow の rust-cache は 0 本」（判定の自己検査 8 通り）。34 PASS・注入 2 通り（修正前の yml / ci.yml にタグ起動）で名指しの FAIL・actionlint 0 件。所要は rust-cache の前後ステップ平均 108 秒ぶん縮む見込み

## 2026-10-09（#1918: セルフテストの器にも -f・tk-coretest の残骸を製品の掃除で回収・psmux の e2e も -f NUL）
- セルフテストの `-f` なし 6 か所（字面 48 / 68 / 73 / 103 + シェルへ打ち込む 1d / 61f。1d は固定名 `takoST` → pid 入り。103 / 61f は backend の conf）を直し、番犬を `mod self_test` とシェル文字列の `-L … new-session` へ広げた。`is_killable_socket_name` に `tk-coretest-` だけ足した（`tk*` は手書きなので据え置き）。psmux は CI で v3.3.7 / v3.3.8 を実測し `-f NUL` で既定の探索をしない → e2e を `Fixture::new_session` へ寄せた
- 実測: 偽の HOME の `.tmux.conf` がセルフテストで 5 → 0 回・番犬は土台の main.rs で 6 か所を file:line で名指し・回収の注入で新テスト 3 本が落ちる・psmux の `-f /dev/null` は v3.3.8 で設定警告

## 2026-10-10（#1946: たまり場を画面全体の縮小・由来タブ → 親 master の見出しで描き、器の無い幽霊を見分けて安全に外せるようにした）
- 真因: ドロワーが開くたびに退避ペインをカード寸法（35×8）へ resize → claude は下端だけ描き直し入力欄と帯しか残らない。正本 `tako_core::background_groups`（器の判定・親 master・まとまり・閉じ方）を GUI と `tako backgrounded` / MCP `tako_background_list` の `groups` / `vessel` / `parent_master` が引く。`spawned_by` を layout.json へ（serde default）。A/B `TAKO_1946_LEGACY=1`
- 実測: `scripts/test-background-drawer-1946.sh` 3 PASS（隔離 GUI・器あり・退避 19 本）: 寸法不変（旧 104×25 → 35×8）・出力起因の全体再描画 43.3 → 0.0 回/秒（間引き 3.2 回/秒）・CPU 101.8 → 14.9%・幽霊を実マウスで閉じても tmux 20 → 20 本。番犬 6 本（注入 6 通りを file:line で名指し）

## 2026-10-10（#1958: pane を省いた `tako_show_command` / `sessions link` を呼び出し元ペインで埋める）
- 真因: MCP の変換（`mcp/request.rs`）と CLI が pane 省略時に呼び出し元（`TAKO_PANE_ID` / `X-Tako-Pane`）で埋めず、show は「対象ペインが未指定」・link はフォーカスペインの会話を返していた。show は `target_pane`、card 指定は埋めない。番犬 `issue1958_caller_pane_watchdog` がカタログで約束する 58 本 × 全 action を公開の入口で検査。`tako_open_remote` は説明を挙動（アクティブタブ）へ寄せ、「混ぜない」側として縛る
- 実測: `scripts/test-show-command-caller-1958.sh`（隔離 GUI・呼び出し元とフォーカスを分ける・HTTP / stdio / CLI + visual `show-command-caller` の実ピクセル）新 22 PASS / 旧 9 PASS 13 FAIL・注入 7 通りで単体と番犬が名指し

## 2026-10-11（#1984: UI スレッドの同期の dispatch の子プロセス待ちの残りを background へ = RemoteAgents・TmuxList・呼び出し元の pid 祖先辿り・spawn / handoff の CLI の所在確認・再起動の計画・check_qos の ps）
- `RemoteAgents` / `TmuxList` は background のジョブ、残りは**先取り**（`OffloadJob::Probe` → `with_ui_probes` の内側で同じ dispatch。handoff は答えが足りなければもう一巡 = `land_offload`）。読む側は `caller_pane_by_pid` / `agent_preflight` / `tako_binary` / `restart_scan` だけ。右パネルの取り直し・再起動のボタンも background。A/B `TAKO_1984_LEGACY=1`・番犬 `issue1984_*`（注入 10 通り）・カタログ増分 0
- 実測: `scripts/test-ui-thread-offload-1984.sh`（偽の SHELL / tmux が武装中は戻らない）修正後 17 PASS（別の要求 0.03〜0.05 秒）/ 修正前 6 FAIL（4.03 秒〜打ち切り・メインスレッドに `caller_pane_by_pid`・`preflight` → `locate` 等）。再描画上限の CPU は蓋閉じで `body_renders` 0 = 未実測のまま

## 2026-10-11（#1989: GitResolveAgent・StaleBinary status・tmux の書き込み系を background へ・agent CLI の所在確認を上限つきへ）
- 解消エージェントは git の走査と CLI の所在を先取り、`StaleBinary status` / `TmuxKill` / `TmuxResize` / `TmuxSelectWindow` は background のジョブ、`TmuxOpen` は socket・存在・グループを先取り、`TmuxCleanup` は host が守るセッションだけ集めて回収の本体は background。`locate` は `find_with_timeout` + 置き場所だけ（打ち切りは `LookupTimedOut`）。GUI のボタン 5 つも `dispatch_off_ui`。A/B `TAKO_1989_LEGACY=1`・番犬は #1984 のものへ注入 17 通り
- 実測: `scripts/test-ui-thread-offload-1989.sh` 修正後 ⑦〜⑭・E5 通過（別の要求 0.03〜0.05 秒・spawn は上限 2 秒で 2.07 秒に理由つき）/ 修正前 8 FAIL（4.0 秒〜打ち切り・spawn は返らない）。再描画上限の CPU は蓋閉じのまま未実測

## 2026-10-12（#1992 S0: PC リソース管理の設計 = 観測・順番待ち `tako queue`・優先度・ETA・見せ方・CLI / MCP・AI の運用・Windows と子 Issue 8 本）
- `.agent/plans/2026-10-resource-manager.md`: 観測（全プロセス走査）+ 明示の入口 `tako queue <cmd>` + 透過の cargo jobserver（`CARGO_MAKEFLAGS` の fifo）の 3 層。調停役は `ipc::submit` の手前で答え UI スレッドを通らない・再起動をまたぐ・tako 不在は待たずに走る。メモリ圧 warn は 1 件ずつ・critical で止める。MCP は `tako_queue` 1 本（約 1.8 KB）。判断点 10 個（§11）
- 実測（読み取りのみ）: 機全体 0.008 ms・全 1,320 プロセス走査 CPU 5.2 ms（5 秒間隔で 1 コアの 0.10%）・`ri_user_time` は mach tick・warn は普段の 42%・claude 1 本は子孫込み約 0.6 GB・cargo 1.95 は外の fifo jobserver に従う・tmux 3.6b は OSC 9;4 を通さない。子 Issue #1993〜#2000

## 2026-10-12（#1963: tako mod S7-4 = コマンドカードを mod で描く。会話の行にカード・帯の 1 行に card N と Copy / Run・描いたカードだけ GPUI のカード帯から外す）
- 応答の `view.cards`（store の論理文字列 + tako が組んだ CLI の引数 + ホットキー）で mod が `tako_show_command` の行と帯に描き、`renders.cards` で返す。GPUI の帯は `claude_mod::card_owner` の 1 本（新鮮な報告 ∧ renders.cards ∧ `cards.place`。拾う前の新しいカードは 5 秒まで保留）で外し、2 秒 tick で戻す。`show-command --list` の `owner`・A/B `TAKO_1963_LEGACY=1`・MATRIX `claude_mod_cards`
- 実測: `scripts/test-mod-command-cards-1963.sh` 48 PASS（実 claude 2.1.294・偽 Messages API が MCP を呼ぶ・行数 44 → 44 を 0.1 秒おきに採って一度も縮まない・全画面 / main screen のキーで Run・報告停止 45 秒 / claude 終了 0.2 秒で GPUI の帯へ・Copy 157 字改行 0）・visual-test `mod-card`（A/B で ① FAILED）・番犬 `issue1963_*`。罠: GUI ペインの claude の `$.ui.copy` は OSC 52 で本物のクリップボードへ届く
