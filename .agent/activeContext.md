# Active Context

> このファイルは AI が毎ターン上書きする現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。

## 現在の対象（2026-08-07、Issue #786 クローム・ペインのビュー単位キャッシュ = **実装完了・PR 待ち**）

- ブランチ `fix/786-view-cache`（worktree `~/dev/tako-wt-786`）にコミット 2 本。
  `crates/tako-app/src/view_cache.rs` を新設し、ペイン本体（`PaneBody`）とクローム 4 枚
  （`Chrome`: TabBar / Sidebar / Panel / StatusBar）を `AnyView::cached` の単位へ切り出した
- PTY 出力は `flush_term_redraw` で**そのペインのビューだけ**を notify。ペインの外にも出る変化
  （OSC / タイトル）とペイン本体の外にも映っている場合（たまり場サムネイル・ホバー /
  ピン留めプレビュー）は従来どおり全体を notify（`PaneVisibility` の 3 値）
- それ以外の状態変化は全部 `TakoApp` を notify し、子ビューが `cx.observe` で自分も汚す
  = 取りこぼしが構造的に起きない順序
- ペインの配置は cached ビューのスタイルへ移し、各 `render_*_pane` は `size_full` で描く

## 実測証拠（隔離・色付き 110 桁 200 行/秒・同一バイナリの A/B）

| 表示中 2 ペイン（22x21） | before（`TAKO_786_NO_VIEW_CACHE=1`） | after |
|---|---|---|
| 4 タブ + サイドバー + 右パネル | 25.30% CPU / 6.772M instr/frame | 18.04% / 5.016M |
| 17 タブ + サイドバー（実フォルダ）+ 右パネル | 36.65% / 9.693M | 8.94% / 5.574M |

- クロームを 4 → 17 タブへ増やしたときの 1 フレーム増分は **2.92M → 0.56M（−81%）**
  = 固定費（クローム）がほぼ消えた。before の 4.9M 前後は #782 の「固定 5.1M」とほぼ一致
- 1 ペイン（47x21）は before 22.77 / 22.79% → after 9.56〜13.43%
- 残る 5M 台/frame はペイングリッド自体の描画。専用 Element 化（#787）の担当

## 検証状況

- fmt / clippy(-D warnings) / test --workspace 全緑（1903 件）
- 隔離セルフテスト完走（`TAKO_APP_SELF_TEST_OK` / exit 0 / FAILED 0）。項目 108 を新設
- visual-test 全 23 節完走（FAILED 0）。`chat-select`(#725) / `md`(#680) / `indent-guide`(#589)
  を含み、`dark_roundtrip_diff` は cpu / python とも 0（キャッシュ無効時と一致）
- **visual-test は #749 以降ビルドできない状態だった**（feature 付きでしか通らないため
  `Request::OrchestratorProfiles` のフィールド追加の追従漏れが CI をすり抜けていた）。
  本 PR で復旧。併せてハーネス側の notify 抜け 1 箇所（編集モードの解除）を修正
- 本番 GUI（pid 47236）は全計測の前後で生存。本番 tmux socket `tako` には触っていない
  （隔離は `TAKO_ISOLATED=1` + socket `tako786` + data dir `/private/tmp/tk786`。
  socket パス長 104B 上限のため data dir だけ短いパスを使った）

## 不変条件

- 汚れ方の規約 2 つを崩さない: ①PTY 出力はそのペインだけ ②それ以外は全部 `TakoApp`
- `view_cache::cached_view` の `view.read(cx)` を外さない（外すとキャッシュしたビューが
  tracked から落ちて二度と描き直されない。実測で踏んだ）
- キャッシュビューへ渡すスタイルが大きさを確定させる（GPUI は中身を見ない）
- 状態を変えたら必ず notify する（毎フレーム全再構築の暗黙依存はもう無い）
- 検証は `TAKO_ISOLATED=1` + 専用 tmux socket + 専用 data / discovery dir。
  System Events のキーストローク送出は禁止（計測のためのウィンドウ前面化は
  PID 指定の `set frontmost` のみ）。隔離アプリは PID 指定で終了する
- 並走ビルドと同時に Cargo / app bundle ビルドを走らせない

## 次の手順

1. push → PR（`Closes #786` / `Refs #782`）→ macOS CI 全ジョブ緑を確認 → squash merge
2. `scripts/build-app.sh --install`（他 worker のビルドと同時に走らせない）
3. GUI 再起動は master 側。再起動後に本番でエージェント高出力時の体感を確認
4. 残りの Zed 同等化は #787（端末グリッドの専用 Element 化）

## 現フェーズで Read すべき設計書

- 描画・再描画まわりを触るとき: `.agent/architecture.md`「ビュー単位の描画キャッシュ」節
- ペイン矩形の会計: 同じファイルの #684 / #781 節
