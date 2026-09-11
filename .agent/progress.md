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

## 2026-09-11（#758: AI 自動命名の claude 呼び出しから MCP サーバーを外した）
- `autorename` の起動を `spawn_claude` 1 本へ寄せ `--strict-mcp-config` を付ける。**再試行の引き金は非ゼロ終了だけ**で、フラグ無しの再試行が通ったときにだけ「使えない」と学習する（打ち切り・起動失敗では再試行しない）。A/B は `TAKO_758_LEGACY=1`
- macOS 実測（順序交互 10 ラウンド。順序固定の初回 15 ラウンドは 2 番目の腕が系統的に遅く測り直した）: 命名プロンプト p50 18.4→16.9s（対応差の中央値 -3.8s・9/10）/ 最小プロンプト 5.8→3.7s / **子孫プロセス 17→2**。隔離 GUI の 1 対 1 は 32.8s→16.6s。`CLAUDE_TIMEOUT` は縮めない（最大 43.6s）
- 単体 6 本（フォールバックを外すと 3 本 FAILED = 検出力）。上限テストが**上限が効いていない**のを発見（kill しても stdout の `join` は孫 = MCP サーバーが握る限り返らない。実測 上限 0.6s が 30.3s）→ 読み出しを `mpsc` の期限付き受信へ

## 2026-09-11（#946: ペインの TERM 注入を「最後に無条件上書き」へ寄せ、1b の失敗理由を名指しできるようにした）
- Issue の見立て（親の TERM がペインへ素通し）は**実測で否定**。修正前バイナリ（`f1af0cc`）を tako のペインの中（親 `TERM=tmux-256color`）から起こしても項目 1b は **3/3 通過**（`ok=true waited=0.1s`）。alacritty は `Options.env` を親 env の上に当てる（`tty/unix.rs:235`）ので注入は元から勝っていた。報告時点（`e703e40`）の 3/3 失敗は**同じ項目を落とす #1165 の固定 6.4 秒窓**で、9/8（`4d84695`）に解消済み
- 残っていた穴は「注入が既定でしかない」こと（`env.extend(options.env)` が後 = 呼び出し側の env 1 つで無言で消える）。`finalize_pane_env` で TERM / COLORTERM を**最後に上書き**へ寄せ、番犬 `端末申告の注入はoptions_envより後にある` が修正前ソースで FAILED。項目 1b の失敗時だけ `TAKO_SELF_TEST_946: cause=injected|inherited|unexpected|no-echo` を出す（SKIP にはしない）
- 実測: 修正後 × 継承あり / `-u TERM` とも `TAKO_APP_SELF_TEST_OK`（316s / 294s）。注入口 `TAKO_946_INJECT=inherit` では 1b が `cause=inherited`（観測 `tmux-256color,truecolor`）で FAILED。`cargo test -p tako-core --test pane_term_env` が親 TERM 6 系統 + `options.env` の `TERM=dumb` でも `xterm-256color` を実シェルで固定

## 2026-09-11（#1309: 新規 worktree で PWA の dist が無くてもビルドが通るようにした）
- 真因は #574 の手当てが CI にしか無かったこと。`crates/tako-control/build.rs` を新設し、`dist/index.html` が無ければ npm でビルドする（**既にある dist は触らない**。rust_embed の埋め込み元へ `rerun-if-changed` も張った）。npm 無し / npm 失敗は**1 行目に手順が出る**エラーで止める（空埋め込みで通す案は不採用 = 製品バイナリに PWA が入らない事故の余地を作る）
- PWA ビルドの正本を `scripts/build-pwa.sh` へ 1 本化（`build-app.sh` / `check-windows.sh` が呼ぶ。既定は毎回作り直す = #60、`--if-missing` は dist があれば何もしない）
- 実測: クリーン worktree で修正前 = `PwaAssets::get` 未定義 4 件で失敗 → 修正後は成功（npm ci + build が自動で走り dist 生成）。npm を PATH から外すと `scripts/build-pwa.sh` の 1 行案内で停止。no-op 再ビルド 3 回 = before 0.17/0.15/0.15s → after 0.16/0.17/0.17s。release rlib に dist のハッシュ付きアセット名を確認。テスト 10 本（偽 npm + 一時 dir、実 npm は起こさない）

## 2026-09-11（#1296: テストの pid ごとの data dir が消えずに溜まるのを直した）
- #944 の隔離は「本番の外へ倒す」までで消す仕掛けが無く、再起動でも消えない macOS の `TMPDIR` に積もっていた（実測 2,188 件 + `tako-agent-config-*` 784 件 = `du` で 115 MB）。`tako_core::test_residue` に後始末を 1 実装し、作る経路（`paths::test_data_dir`）が `arm_self_cleanup`（`libc::atexit`）+ `sweep_stale_on_start`（SIGKILL 分を次回起動で回収）を持つ形へ
- 消すのは**自分の pid か pid が生きていないもの**だけ。pid 再利用は「dir の作成時刻より後に始まったプロセス」として見分けて見送る（実測 19 件）。消す直前に生死と作成時刻を取り直して、列挙後に作り直された置き場を巻き込まない
- 既存残骸の口は `tako test-residue`（既定 dry-run・`--apply` で実削除・MCP `tako_test_residue`）。番犬 2 本（修正前ソースで 2 件 FAILED）+ 子プロセス実測 2 本 + 単体 12 本。A/B は `TAKO_1296_LEGACY=1`

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

## 2026-09-11（#1312: テスト本体が作る使い捨て dir を「スコープで消える器」へ寄せた）
- #1296 の射程は**置き場を決める側**だけで、テストが個別に `temp_dir().join(…)` で作る使い捨ては残っていた（実測: `cargo test -p tako-core --lib` で 14 件 / `-p tako-cli -p tako-control --lib` で 31 件）。`tako_core::test_residue` に `ScratchDir`（スコープで消える）と `process_scratch`（プロセス寿命）を 1 実装し、親を `<TMPDIR>/tako-test-scratch-<pid>` 1 つへ畳んで #1296 の 2 段構え（atexit + 次回起動の pid 回収 + `tako test-residue`）にそのまま乗せた
- 実測（空 TMPDIR の前後）: tako-core `--lib` **14 → 0**・全 target **16 → 0**・tako-cli + tako-control `--lib` **31 → 7**（残 7 は `dispatch.rs` の `tako-mcp-test-*` = 別 worker が編集中で射程外・#1312 にコメント）。libtest が `process::exit` する失敗回でも 0、SIGKILL 相当の残骸は次回起動で掃かれ、生きている pid の置き場は残る
- 番犬は動的 1 本（このテストバイナリを使い捨て TMPDIR で回して**実際に数える**。修正前の作り手で 15 件を名指し FAILED）+ 静的 8 本（修正前ソースで 4 本 FAILED）。A/B は `TAKO_1312_LEGACY=1`（器を作りっぱなしへ戻す = 19 件残る）
