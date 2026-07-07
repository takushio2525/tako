# Active Context

> このファイルは AI が**毎ターン上書き更新**する現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。ここには履歴を残さない。
> セッション開始時に AGENTS.md の直後に必ず読む。

## 現在の対象（2026-07-07・#104 remote セキュリティ強化をマージ・実機反映）

直近作業 = #104（tako remote 監査 + 推奨6件実装、PR #105 = `5782367` マージ済み、
`build-app.sh --install` で 0.3.0 反映済み）。**要 tako 再起動**で新 CLI 経路
（`remote start` の暗号化トンネル必須化 / `--insecure` opt-in / `--show-token`）が GUI に反映される。
remote は secure-by-default（トンネル不成立で起動拒否）に変わった。secure 拒否の runtime 観測は
cloudflared を隠せず未実施（コード+build 担保。監査レポート `reviews/2026-07-07_takoremote再監査.md`）。
Unreleased に #95 / #100 / #104 が溜まっている（次リリース 0.3.1 未実施）。

---

## 参考: 直前フェーズ（2026-07-07・#95 + #100 マージ済み、実機反映完了）

main = `4c62786`（#95 Enter 空振り修正 = PR #101、#100 品質パイプライン = PR #102 の両方入り）。
tako 再起動済み（2026-07-07 14:08、新プロセス確認済み）。**#95 は実機検証まで完了**:
Enter 代行が括りなし CR 即発火（旧: 空括り+13 秒をプローブのバイト観測で確認）/
残留テキストの Enter 代行 4 連続成功 / busy（生成）中の Enter 送達が queue 成立 →
タスク完了後の自動送達まで実 claude で確認。
副産物: Cmd-Q で終了しない事象を発見 → #103 起票（Dock 右クリック終了は正常。未修正）。

- #100 品質パイプライン: master 用 default prompt に task-intake / worker-prompt-template /
  acceptance を新設（既存ブロック名は prompt_blocks 互換のため維持）。setup 配布物に
  CLAUDE.md セクション `06-completion-verification` 新設 + changes.yaml rev 5（guided）。
  設計意図 = `reviews/2026-07-07_オーケストレーション品質設計.md`
- ローカル環境の移行済み: 旧カスタム `orchestrator/master-system.md` は
  `master-system.md.bak-20260707` へ退避（シャドウ解除）。個人ルールは
  `orchestrator/local-rules.md` に集約し、profiles/{default,fable}.yaml の
  `prompt_blocks.append` で注入する構成へ（default の今後の更新に自動追従）
- この環境の `setup.applied_revision` は 0（フル `tako setup` 対話を v0.3.0 以降未完走）。
  次回 `tako setup` で rev 1〜5 の追従案内が出るのは正常（rev 2/5 の guided は
  master-system.md 退避済みなので「デフォルト使用中」で即通過するはず）
- setup 関連の変更を入れたら `resources/setup/changes.yaml` に revision を 1 増やして追記する
  （運用ルール。記入方法はファイル冒頭コメント。連番・非空はテストで機械検証）
- 残 Issue: #84（MCP HTTP 直列処理）/ #85（タブ退避の CLI/MCP 対応）/ #86（ControlHost 分割）。
  将来候補: worker への直接 system prompt 注入（`build_worker_claude_cmd` に
  --append-system-prompt-file。reviews/2026-07-07 の「今後の候補」参照）
- リモート接続バグ #89 残り: lan_ip の en0 固定解消・cask への cloudflared 依存追加など
- 公開監査は全条件クリア（判定 OK）。次リリース（0.3.1 tag / Release / cask）は未実施 —
  Unreleased に #95 / #100 が溜まっている

## 未検証（スマホ実機テスト — #63 リーダービュー）

- [ ] タッチでの連続スクロール(上下)が滑らかに動作するか
- [ ] 下端追従: 新しい出力が来たとき自動スクロールするか
- [ ] 「↓最新へ」ボタン: 過去を見た後に押すと最下部に戻り追従再開するか
- [ ] ソフトキーボード入力: 文字入力 + Enter 送信が機能するか
- [ ] #64 PC 側確認: 日本語混在行で半角文字が消えないこと

## 残作業・既知の制約

- main.rs は 9,800 行前後。さらなる分割は別タスク
- MCP HTTP ポートのランダム問題は未解決（stdio ブリッジ経由なら影響なし）
- セルフテストの既知失敗は PDF（項目 70、CoreGraphics 環境依存）のみ
- CI（GitHub Actions）が 6/12 以降トリガーされていない — Actions 無料枠逼迫で停止中。
  品質保証はローカル全緑で代替

## 未着手タスク（優先順はユーザーと相談）

- [ ] **Phase 5 続き**: FR-3.5 軽い編集
- [ ] **FR-2.19 localhost ポートパネル**
- [ ] **FR-2.18 未表示の子の自動サーフェス**
- [ ] **FR-2.14 MCP ゼロコンフィグオンボーディング**（配布前必須）

## 現フェーズで Read すべき設計書

- ターミナル描画修正時: `crates/tako-app/src/main.rs` の `chunk_line_chars` / `terminal_screen_lines` 周辺
- リモート API 修正時: `crates/tako-control/src/remote.rs` モジュールコメント
- リモート PWA 修正時: `web/tako-remote/src/pages/terminal.jsx` 冒頭コメント
- オーケストレーター修正時: `.agent/orchestrator.md`（品質パイプラインの表）+
  `crates/tako-control/src/orchestrator/default_system_prompt.md`
