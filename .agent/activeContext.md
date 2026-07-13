# Active Context

> このファイルは AI が**毎ターン上書き更新**する現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。ここには履歴を残さない。
> セッション開始時に AGENTS.md の直後に必ず読む。

## 現在の対象（2026-07-13・#177 全ペイン消失の根治。#159 / #165 も同日完了）

**#177（UI から全ターミナルペイン消失）を根本修正**。根本原因は多重起動ガード（#113）の
構造的な穴: ガードの判定材料が discovery（control.json）だけなのに、守るべき資源
（layout.json = HOME 固定 / tmux セッション = TAKO_TMUX_SOCKET）は別の環境変数で
差し替わる。`TAKO_DISCOVERY_DIR` だけ隔離した dev 検証起動（#165 検証中の実験、16:53:47）が
プライマリ判定 → 本番 layout.json を復元 → `new-session -A -D` が本番 GUI の
クライアント 13 本を強奪 → PTY 一斉死亡 → 定期保存が縮退 layout を上書き、の連鎖。
**#178（同事故の起票）はこの修正が本質対応**（討議の案 1 プロセスベース判定は
セカンダリ残存で本物の再起動を阻害するため、資源ベースのクライアントガードを採用）。

対策（三層防御 + 復旧）:
- **復元強奪ガード**（FR-5.10）: 復元前に `tmux list-clients` を走査し、生きた別
  tako-app 配下のクライアントが attach 中ならセカンダリ降格
- **縮退保存ガード**（FR-5.11）: ペイン数半減の保存前に `.bak.1`〜`.bak.3` 世代退避
  （bak.1 が 10 分以内は回転しない = 連鎖縮退での押し出し防止）
- **`TAKO_ISOLATED=1`**: discovery / persist / tmux socket の一括隔離（片脚隔離の根絶。
  実験起動はこれを必須とする。AGENTS.md コマンド表に明記）
- **`tako recover`**: バックアップ一覧 / `--apply <世代>` 復元（稼働中は拒否、--force あり）
- persist.log 全行に `[pid N]` 付与

詳細は `.agent/architecture.md`「多重インスタンスの資源保護」節。

main 側の同日完了分: #159 スクロール大幅改善（ピクセル単位化 + ローカル履歴ミラー）、
#165 spawn レイアウトエンジン（master-reserved + grid/spiral。59 ツール）。

## 検証済み（#177）

- workspace build / test（521 passed）/ fmt / clippy(-D warnings) 全緑 + 隔離セルフテスト完走
- 実機 e2e（隔離 HOME + 専用 socket、本番不接触）: 修正後 = 事故条件（discovery のみ隔離）の
  後発インスタンスがセカンダリ降格し先発の 4 クライアント無傷 / FORCE_PRIMARY バイパスで
  事故経路を再現（クライアント全交代 = 強奪）→ 縮退保存直前に bak.1（5 ペイン版）自動退避 →
  `tako recover --apply 1` → 再起動で「2 タブ / 5 ペイン（tmux 再 attach 4）」完全復旧

## 次の一手

- fix/177 の PR #180 を squash merge（#177 / #178 クローズ）→ `build-app.sh --install`
- tako 再起動後の GUI 確認（manual-checks.md）: 「ターミナルスクロールの大幅改善」節
  （#159）+「Web ビューペイン」節（#155）+ #153/#152 節 + Cmd-Q 経過観察（#103）
- 明朝 5:00 の夜間ジョブ初回実行を監視（v0.4.1 自動リリース見込み。#166）
- Phase 5 の次候補は FR-2.19 localhost ポートパネル・FR-3.10 画像プレビュー等

## 現フェーズで Read すべき設計書

- 多重インスタンスの資源保護（#177）: `.agent/architecture.md` 該当節
- spawn レイアウトの設計（#165）: `.agent/architecture.md`「spawn レイアウトエンジン」節
- スクロールの要件（#159 で全面改稿）: `.agent/requirements.md` FR-2.5.13 +
  手動確認 `.agent/manual-checks.md`「ターミナルスクロールの大幅改善」
- 設定ファイル I/O の安全化（#169）: `.agent/architecture.md` 該当節
