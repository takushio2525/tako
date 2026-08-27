# Active Context

> このファイルは AI が毎ターン上書きする現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。

## 現在の対象（2026-08-24）

- **#932（ちらつき）第 2 ラウンド: タブ切り替えの「遅れリサイズ」を突き止めて根治した**。
  裏タブのペインは `render_pane` を通らないので、#647 が入ったあとも**幾何の変更**
  （ウィンドウ寸法・サイドバー幅・バナー）が届かず、表に出した瞬間に初めて
  リサイズ = SIGWINCH が飛んでいた（実測 裏 116x37 / 表 88x33 → 表に出した瞬間 88x33）。
  割り出しを表示中と同じ 1 本（`pane_text_area_of` → `grid_cells`）へ寄せて解消。
  A/B は `TAKO_932_NO_OFFSCREEN_GEOMETRY=1`。**実機で症状が消えたかはユーザー確認待ち**
- **#932 で潰した仮説（実測で否定。再調査の周回を避ける）**: 器（tmux）はリサイズで
  画面を消さない（`ED 2` 0 回・再描画は 0.1〜0.4ms で完了）/ 実 claude の TUI は
  SIGWINCH で消えない（4.7ms 刻みで一度も半分未満にならない）/ タブ切り替え・分割比変更・
  ウィンドウ寸法変更でグリッドが空になることは無い（1〜5ms 刻みで `grid_blackouts=0`）。
  詳細は `.agent/architecture.md`「裏タブのペインは「表に出たときの寸法」へ合わせる」
- **#467 Windows 移植はスライス 1〜9 がすべて main へ入り、最後の 8（棚卸し）も完了**。
  残りは「実機バグの消し込み」と「未実測項目の消し込み（#937）」だけ
- **#591（対応マトリクスの棚卸し + docs ページ）を完了**。判定は
  **supported 69 / degraded 13 / pending 56 / unsupported 2**（棚卸し前は 4 / 2 / 132 / 2）。
  `Feature::windows_evidence` を新設し **T7 が根拠なしの Supported を落とす**。
  docs は `docs/src/content/docs/windows-support.md`（生成物・CI で `--check`）
- **Issue の「完了（`cf7c9a4`）」は main に入っていなかった**（#658 と同じ型）。
  `windows/467-*` ブランチのコミットを「入っている」と読まないこと
- **#617（ゴミ箱が完全削除）は main へ移植して解消**（実装は win467 の `d528058` /
  `4752eee` に在り main には 1 行も入っていなかった = #658 / #591 と同じ型）。
  `SHFileOperationW` + `FOF_ALLOWUNDO` へ差し替え、その他 unix は**削除へ劣化させずエラー**。
  表記は `os_integration::file_manager()` 1 か所で決めて `FileManager` を値で配る。
  **実機は offline なので #617 は open 維持**（実機確認項目は Issue コメント）
- **#722（AI タブ命名が Windows で一度も走らない）も main へ移植して解消**。
  `autorename::detect_claude()` だけが B16（`platform::exe::find`）へ寄せられておらず、
  `$SHELL -l -c "command -v claude"` が Windows で必ず失敗 → `.ok()?` で `None` →
  `OnceLock` なので永久に無効、という**黙って死ぬ**形だった。判断部分を純粋関数
  `resolve_claude` に切り出し、`TAKO_AUTORENAME_DIAG=1` で理由を出せるようにした。
  マトリクスは **`Supported` へは倒さず `Degraded` のまま**理由文を #760 の実態
  （素材が不変なので命名はタブごとに 1 回だけ）へ差し替えた
- **棚卸しで確認した残りの製品バグ（未着手）**: **#935**（受け入れゲートが `sh -c`）/
  **#936**（古い claude の警告が出ない。#726 の続き）
- **#937**: 未実測 46 件の消し込み（手順は Issue 本文）。実機復帰後の作業
- **解消済み（詳細は plan の各記録節と progress.md）**: #898 / #927 / #920 / #913 / #907 /
  #906 / #903 / #866 / #897 / #889 / #872 / #727 / #905 / #766 / #870 / #884 / #881 / #877 / #875 / #873
- **A/B の env（同一バイナリで旧挙動へ戻せる）**: `TAKO_920_LEGACY` / `TAKO_913_LEGACY` /
  `TAKO_906_NO_PAD` / `TAKO_907_NO_INJECT` / `TAKO_903_LEGACY` / `TAKO_866_KEEP_EXACT_TARGET`

## 対応マトリクスを触るときの規約（#591）

- **根拠なしに `Supported` / `Degraded` / `Unsupported` へ倒さない**。`windows_evidence` へ
  「実機セルフテストの項目 / 実機で緑のテスト名 / 実測の記録」のどれかを書く。
  書けないなら `Pending` + `notes::WIN_UNVERIFIED` + 追跡 #937 のまま置く（T7 が落とす）
- 宣言は `PlatformFacts` 経由で master / solo / setup の system prompt へ流れる（#516）。
  **過大申告はエージェントを誤らせ、過小申告は使える機能を回避させる**
- 理由文は「〜が前提」ではなく**実際に何ができないか**を書く（回避行動が取れる形）
- docs は生成物。`cargo build -p tako-cli && node scripts/gen-windows-support-docs.mjs`。
  新機能はスクリプトの `CATEGORIES` へ 1 行足す（足さないと生成が落ちる）
- **テストに理由文を直書きしない**。期待値はマトリクスから作る（#920 / #591 の両方で踏んだ）

## 実機セルフテストの到達範囲（#920 後の実測）

**完走している**（`TAKO_APP_SELF_TEST_OK` / exit 0 / FAILED 0 / skip 19）。
skip 19 は全部理由つきの既知で、内訳がそのまま「Windows で動かないもの」の一覧になる:
psmux が本物の tmux でない系（#519）/ PDF の text_layer 不在（#693）/ WebView2 の panic（#724）/
macOS 固有の項目 79（#872）/ POSIX 専用の道具（nc・ジョブ制御・`/dev/fd`・ECHOCTL）/
links の POSIX 前提（#522）/ 蓋閉じで未描画になる項目。

**実機テストのベースラインは 22 件**（21 + #930）。失敗名まで照合する（全数は plan の
「#906 の記録」節）。**製品の縮退を指すもの**（acceptance_gates 5 = #935 / stale_binary 2 = #936 /
remote 2）と**テスト側の POSIX 前提**（`/tmp` 直書き・区切り決め打ち・symlink）は別物。

## 実機テストの読み方（要点。全文は plan の各記録節）

- **psmux の e2e / GUI セルフテストは `schtasks /it`（session 1）で回す**。SSH（session 0）で
  作った psmux の detached セッションは約 1 秒で自然死する
- **孤児は run のたびに掃除する**。「tako-app が 1 つも居ない」を確かめてから
  `-L tako-iso-*` を**明示 pid で**落とす（`-L tako` は本番）
- **GUI 起動時の env を再現してから測る**（`SHELL` / `HOME` は SSH セッションの Process
  スコープにしか無い）。長い処理は `schtasks` か `Invoke-CimMethod` で投げ、
  **ログの `EXITCODE=` 行で完了を待つ**
- **測定側も UTF-8 にする**（`[Console]::OutputEncoding`）。ログは `-Encoding UTF8` で読む
- **`git stash` を A/B に使わない**。`git checkout <sha> -- <path>`。ただし**未コミットのまま
  `git checkout HEAD -- <path>` は自分の変更を全部捨てる**
- **fresh worktree は `web/tako-remote/dist/` を持たない**（`rust_embed` が埋め込むので即失敗）。
  既存 worktree からコピーする。**docs も `npm ci` が要る**
- **`cp` が `-i` の別名かもしれない**: スクリプトでは `command cp -f` を使う

## 測り方の落とし穴（#932 で踏んだ。他の検証にも効く）

- **セルフテストは `-u TERM -u COLORTERM` で起動する**。tako のペインへ渡る TERM は
  親から継承されるので、tako のペインの中（`TERM=tmux-256color`）から起こすと
  項目 1b（TERM / COLORTERM 注入）が**決定的に落ちる**（main でも 3/3）。GUI 起動には
  親の TERM が無いので、外して測るのが本番と同じ条件
- **`cargo test` は本番 data dir へ書く**（#944）。`TAKO_DATA_DIR` を渡さないと本番
  `perf.log` へ入り、しかもテストプロセスは `mark_main_thread()` を呼ばないので
  **全部「メインスレッド専有」と誤記録**される（本番ログの 643 行の正体）
- **`visual-test` の全節は現状 main でも `term-grid attrs-underline` で止まる**（#943。
  `ul_strip=32` が期待の 40 未満。e703e40 で同じ数値を実測）

## 次の一手

- **#935 / #936**: どちらも「境界へ寄せる」既存の型がある（#875 = B1 / #898 = B16 /
  スライス 9 = procinfo）ので、寄せ先は決まっている
- **#937**: 実機が戻ったら未実測 46 件を消し込む。GUI が要るものは `schtasks /it` で
  セルフテストを回し、CLI で足りるものは `ssh win` から直接叩く
- **#967（新規）**: セルフテスト項目 97 (d) が Windows で必ず止まる。判定が画面のリテラル
  `"tako setup"` を見ているが、実行ファイル名が `tako.exe` なので `…\tako.exe setup` になる。
  **#898 が実体パスを返すようにした時点**で壊れており 98 以降が一切走らない（製品は正しい）
- **#467 のエピック完了判断**: スライスは全部入った。残るのは上の実機バグと未実測の消し込みなので、
  「移植は完了・品質の詰めが残る」という状態

## 現フェーズで Read すべき設計書

- `.agent/plans/2026-08-windows-main-merge-wip.md`（「8 の記録」節に棚卸しの作法と根拠の在庫表・
  ベースライン失敗名の切り分け。「#920 の記録」節に完走までの経緯と skip 19 の内訳。
  各 Issue の記録節に実機の測り方）
- `.agent/plans/2026-07-windows-port-architecture.md`（境界の定義）
- `crates/tako-core/src/platform/support.rs`（対応マトリクスの正本。判定を触るなら必ず読む）
