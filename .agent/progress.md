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

## 2026-09-11（#1309: 新規 worktree で PWA の dist が無くてもビルドが通るようにした）
- 真因は #574 の手当てが CI にしか無かったこと。`crates/tako-control/build.rs` を新設し、`dist/index.html` が無ければ npm でビルドする（**既にある dist は触らない**。rust_embed の埋め込み元へ `rerun-if-changed` も張った）。npm 無し / npm 失敗は**1 行目に手順が出る**エラーで止める（空埋め込みで通す案は不採用 = 製品バイナリに PWA が入らない事故の余地を作る）
- PWA ビルドの正本を `scripts/build-pwa.sh` へ 1 本化（`build-app.sh` / `check-windows.sh` が呼ぶ。既定は毎回作り直す = #60、`--if-missing` は dist があれば何もしない）
- 実測: クリーン worktree で修正前 = `PwaAssets::get` 未定義 4 件で失敗 → 修正後は成功（npm ci + build が自動で走り dist 生成）。npm を PATH から外すと `scripts/build-pwa.sh` の 1 行案内で停止。no-op 再ビルド 3 回 = before 0.17/0.15/0.15s → after 0.16/0.17/0.17s。release rlib に dist のハッシュ付きアセット名を確認。テスト 10 本（偽 npm + 一時 dir、実 npm は起こさない）

## 2026-09-11（#1296: テストの pid ごとの data dir が消えずに溜まるのを直した）
- #944 の隔離は「本番の外へ倒す」までで消す仕掛けが無く、再起動でも消えない macOS の `TMPDIR` に積もっていた（実測 2,188 件 + `tako-agent-config-*` 784 件 = `du` で 115 MB）。`tako_core::test_residue` に後始末を 1 実装し、作る経路（`paths::test_data_dir`）が `arm_self_cleanup`（`libc::atexit`）+ `sweep_stale_on_start`（SIGKILL 分を次回起動で回収）を持つ形へ
- 消すのは**自分の pid か pid が生きていないもの**だけ。pid 再利用は「dir の作成時刻より後に始まったプロセス」として見分けて見送る（実測 19 件）。消す直前に生死と作成時刻を取り直して、列挙後に作り直された置き場を巻き込まない
- 既存残骸の口は `tako test-residue`（既定 dry-run・`--apply` で実削除・MCP `tako_test_residue`）。番犬 2 本（修正前ソースで 2 件 FAILED）+ 子プロセス実測 2 本 + 単体 12 本。A/B は `TAKO_1296_LEGACY=1`

## 2026-09-11（#1295: 実行拒否ゲートが claude では常に真だったのを締めた）
- 真因は `None`（観測ゼロ）を無条件に作業ゼロと数えていたこと。claude の腕（`query_agent_status`）は `agent_work_started` を一度も代入しないので `!= Some(true)` が常に真で、`execution_refused_patterns(Claude)` に文言を 1 つ足した瞬間に正常完了した worker が `error` / `retry_spawn` へ落ちる形だった。判定を `dispatch::refusal_gate_open` の 1 実装へ閉じ、`None` を通せる系統は `agent_support::MATRIX` の新マス `worker_refusal_work_proof`（agy だけ対象外 = 代理の証拠を使う）が宣言する
- A/B（`TAKO_1295_LEGACY=1`）: 旧アーム = 正常完了の claude worker が `status=error` / `kind=execution_refused` / `retry_spawn`、新アーム = `error` なし。#1034 の agy 経路は両アームとも分類されたまま（既存テスト緑）
- 番犬は 4 → 7 本で doc と assert を機械で結んだ（doc の「ゲートの形」1 行を assert の入力にする）。ゲートを旧形へ戻す A/B で 4 本が `dispatch.rs:9343` 等を名指し FAILED

## 2026-09-11（#1320 / #1324 / #1319: AI 向け規約ファイルの取り残し 3 件）
- `AGENTS.md` の「状況」行（Phase 5 中断中のまま = 3 か月前）と push 運用（「公開まで main 直 push 可」）を実態へ。フェーズ詳細は `.agent/roadmap.md` 参照に寄せ、Phase 7 見出しも「✅ 公開済み・残は README 図版と CONTRIBUTING.md」へ棚卸し（#1320）
- `.agent/commands.md` に `tako file open-in-tako`（#1182）の行を追加。コマンド名で正規化した `comm` の差分は設計上の畳み込み `tako setup bootstrap` 1 件のみ（#1324）
- `.agent/requirements.md` の重複 FR を解消（#1319）。コードが 12 か所参照する **FR-3.18 = Code Runner は不動**、#496 の 2 行を FR-3.25 / 3.26 へ、参照ゼロの #1067 セクションを FR-2.38 へ（`:1653` の相互参照も追従）

## 2026-09-11（#1313: config_io の tmp 名を書き込み 1 回ごとに分けた）
- #638 の同型を `config_io::atomic_write` へ。ロック無しで呼ぶ経路は 11 か所（`with_backup` 経由込み。Issue にコメントで列挙）で、legacy 実測は 3 形 = `len=0`（空 = #169 の全消失の入口）/ 短い本文が長い本文の先頭を潰した `len=5610` / rename の ENOENT
- tmp suffix を `.tmp.{pid}.{seq}` へ（`AtomicU64`）。`.tmp.` を含む形は保つ（共有カタログが `contains(".tmp.")` で派生を外すため。catalog のテストへ新形を追加）
- A/B `TAKO_1313_LEGACY=1`: 8 スレッド × 100 書き込みの再現は旧 **50/50 FAILED** → 新 **0/100**（負荷 81〜99 下でも 0/50）。並行テストは**統合テスト側**へ置く（同一プロセスの `ipc::tests::連続接続でfdが漏れない` の fd 計測を押し上げるため）

## 2026-09-11（#1297: 入力欄の AI ゴースト提案を「人の下書き」と読まないようにした）
- #1273 の最後の関門（入力欄が空か）が**文字列だけ**だったので、claude が空欄へ dim で描く AI ゴースト提案が下書きに見えて覆せなかった（本番 pane 1636 / 1761 / 1775 / 1784 が永久 busy）。判定へ `read_pane` の `input_status.style` と同じ 1 実装から採った属性を渡し、ghost / none = 下書きなし・user / mixed = 下書きあり（#1273 の安全側は維持）。述語の正本は `InputStyle::is_user_draft`
- 属性の取れない素の tmux capture は従来の文字列判定へ落とし、`worker_status` の新フィールド `idle_override_blocked=input_draft_unreadable` に理由を残す（MCP / CLI 1:1）
- 実測: 実 PTY の dim 提案で `InputStyle::Ghost` → `status=idle` / 修正前ソースでは単体 5 本が `busy` で FAILED。A/B は `TAKO_1297_LEGACY=1`。番犬 4 本が 5 種の注入（文字列へ戻す / ghost を下書き / mixed を空 / 渡さない / 採らない）を file:line 名指しで落とす。実 claude 2.1.258 の入力欄プレースホルダが `ESC[2m` であることも隔離 tmux で確認

## 2026-09-11（#1314: psmux の conf を tmp → rename で差し替えるようにした）
- `backend::psmux::ensure_conf` が `new-session -f` の直前に最終パスへ直書きしていた（#625 の機序③が psmux 側に残存）。`write_conf_in`（pid + 連番の tmp → rename・rename 失敗時は tmp を掃除）へ寄せ、tmux 側（#625）と同じ作法に揃えた。**内容が同じなら書かない**ので、2 回目以降の spawn は最終パスに触らない（Windows の `FILE_SHARE_DELETE` 依存の置換そのものを避ける）
- A/B `TAKO_1314_LEGACY=1`（修正前の直書き）: 8 スレッド × 200 書き込みの再現テストが旧アーム **60/60 FAILED**（`len=0` / 短い本文が長い本文の先頭を潰した `len=55296`）→ 新アーム 0/60・負荷下（load 8 / 55〜60）0/50 × 2
- **Windows CI で tako-core のテストが 1 件も走っていなかった**（`cargo test --workspace` は非ブロッキング + #583 で打ち切り）ので、#1282 と同じ形で `backend::psmux` の実行検査を blocking ステップとして追加。実機確認は Windows 機が offline のため Issue に手順を残した

## 2026-09-11（#1333: PR の「CI 3 本緑を待って merge」を共通の待ちスクリプトへ寄せた）
- 「出ているチェックが全部非 pending = 完了」は push 直後に Cloudflare Pages しか登録されていない瞬間に通る（#1313 = PR #1328 が CI 完了前 merge・過去に #1253 / #1282）。判定を `scripts/wait-pr-checks.sh` の 1 実装へ寄せ、**期待名が全部そろって全部 completed** を **2 回連続**観測してから確定する形にした（期待名は `.github/workflows/*.yml` の pull_request job から導出・外部連携の Cloudflare Pages だけ定数）
- merge は `scripts/merge-pr.sh`（揃わなければ merge しない / CONFLICTING・BEHIND は待たずに拒否）。モックテスト 14 ケース（偽 gh + 偽リポジトリ）は CI の macOS ジョブで走り、A/B `TAKO_1333_LEGACY=1` の腕が「1 本だけで完了」「揺れの 1 回目で確定」を再現して Test 13 / 14 が固定する
- この PR 自身をこの経路で merge する（初回の実運用で CI 赤を 2 回検出して拒否 = #1313 の予算超過と #1343 のコンパイル破損）。**merge 直前に「緑を出した run の後に main が進んだか」を警告する**（PR の CI は merge 結果を検査するが merge base は run 開始時点で凍る = #1343 の事故クラス）

## 2026-09-11（#1318 #1321: README のリモート transport と `tako --help` の説明を実態へ）
- README の日英を #1038 後の実態（Tailscale `serve` → ループバック TCP `127.0.0.1` のエフェメラルポート・LAN / 外部から到達不可）へ。事実に反する「TCP ポートを一切開きません」を削除し、`tako remote --help` の `Tailscale Serve + UDS` も同じ形へ
- #982 で `AgentSupport` が `Platform` の doc の上へ挿し込まれ 3 行すべてが agent-support の説明になっていたのを分離。`platform` は `PlatformArgs` の「参照引数」露出をやめて自前の doc を持つ（`--help` 実出力で確認）
- 挙動は不変（doc コメント / README のみ）。`remote.rs:45` / `:1473` の同じ #1038 前の記述は #1332 へ切り出した
