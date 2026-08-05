# Active Context

> このファイルは AI が**毎ターン上書き更新**する現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。ここには履歴を残さない。
> セッション開始時に AGENTS.md の直後に必ず読む。

## 現在の対象（2026-08-05・Windows 実機 — #521 動画プレビュー）

**#521 の動画側**を実装した（PDF は PR #704 で済み）。抽象境界 **B12** に
`crates/tako-app/src/platform/video/` を新設し、Windows は **Media Foundation の
`IMFMediaEngine`（フレームサーバーモード）+ `IWICBitmap`** で再生・シーク・音量・
フレーム表示まで通す。追加の再頒布物なし（pdfium を見送ったのと同じ判断）。

- `video_player.rs` は**純粋計算だけ**を残し `VideoPlayer` は境界から `pub use`。
  呼び出し側（main.rs / preview_render.rs / dispatch）は**コメント以外 1 行も変えていない**
- macOS 実装は無改変で移設（トークン列 2142 個が完全一致することを機械確認）
- マトリクスは `tako_video_playback` / `seek` / `volume` / `tako_open_file` を **対応済み**へ。
  縮退理由 `WIN_VIDEO_MACOS_ONLY` / `WIN_PREVIEW_NO_VIDEO` は実態と食い違うので削除
- 検証素材の mp4（H.264 + AAC）は **ffmpeg を使わず OS のエンコーダで生成**する
  （`platform/video/test_fixture.rs`）。CI の Windows ランナーでも同じ e2e が回る

### 実装で効いた Windows 固有の事情（再訪時にまず読む）

1. **初回ロードだけ 4 秒超**（デコーダ MFT 初期化）。`open()` は待たず、
   `needs_tick()` が「メタデータ + 最初のフレームが揃うまで」true を返してティッカーが埋める
2. **総尺不明のとき `clamp_time` がシークを全部 0.0 に潰す** → `seek_target()` で回避。
   これを踏むと「開いた直後の `tako video seek 4.0` が必ず先頭へ飛ぶ」
3. `OnVideoStreamTick` の S_OK / S_FALSE は `windows` crate だと**どちらも `Ok`**。
   vtable を直接呼んで HRESULT を見ないと新フレームの有無が判らない
4. PDF で必要だった終了時 ACCESS_VIOLATION の「番人」は**動画では不要**（実測で exit 0）

## 次の一手

1. **#521 に残るのは B11（Web ビュー）の実機目視**だけ（マトリクス上 `tako_web` は対応済み）
2. **GUI の再生ボタンが失敗を画面に出さない**（`start_video_player` の Err は eprintln のみ）。
   macOS でも同じ既存挙動だが、Windows では「OS が持たないコーデック」で踏みやすい。
   通知 UI を足す別タスクにしたい
3. **稼働中 GUI は旧バイナリ**なので動画も `tako account`（#709）もまだ使えない。
   次のインストーラー更新で反映
4. **main とのアカウントモデル統合（マージ時の宿題）**: main には CLI
   `tako orchestrator accounts`（#548/#556）が別途あるので、両方の CLI パスを同じ dispatch へ
   向ける（ロジックは二重化しない）。master への account 適用も本ブランチ #653 と main #555 が
   独立実装で衝突する既知の債務
5. **#623 の実機確認（未決）**: 日本語入力の打鍵消失は直った確証が無い。症状が出たら
   `TAKO_IME_DIAG=1 TAKO_PERF_LOG=<path>` で採取し #623 のコメント §5 で切り分ける

## mac 側 master への引き継ぎ（マージ後に要対応）

- **macOS ビルド + 目視確認**: Windows 側は cfg 構造保証のみで macOS 未コンパイル。
  特に #584（TRAFFIC_LIGHTS_SPACER の cfg 化）と #582（変換中も
  `invalidate_character_coordinates` が走る = mac でも挙動変化・改善方向）の目視を
- **tmux 経路の実機再確認**: M1 の配線変更（#519）は argv スナップショットで不変保証済みだが、
  再 attach と orphan 掃除の実測は mac でやること
- doc の getting-started は Tabs（mac/win 切替）化 + `windows-support.md` 新設（生成は
  `scripts/gen-windows-support-docs.mjs`、support.rs 変更時に再生成）
- **#525 で mac にも出る挙動差 3 点**（いずれも改善方向・要目視）: ①CLI 入口で表示言語を
  解決するようにしたので、日本語設定なら全サブコマンドの縮退理由が日本語になる
  ②`setup` の `completed_at` がローカル時刻 → UTC（`…Z`）表記へ ③MCP 自動登録の失敗が
  setup 全体を中断しなくなった（手動手順を出して続行）
- 夜間リリース（launchd）と Windows ローカルリリースの棲み分け: アセット名で OS 分離
  （`tako-setup-{tag}-x64.exe` / `tako-{tag}-windows-x64.zip`）。チャンネル軸とは直交

## 環境メモ（この Windows 機）

- 稼働中 GUI は**旧バイナリ**（今日の修正が未反映）。乗り換えはインストーラー経由で行う
- worker 検証の罠: ペイン注入の `TAKO_SOCKET`/`TAKO_TOKEN`/`TAKO_PANE_ID` を剥がさないと
  本番インスタンスへ誤接続する（今日 3 回発生）。隔離は `TAKO_ISOLATED=1` + env 剥がし + 接続先 assert
- psmux 検証資材: `~/dev/psmux-eval/`（REPORT.md + 生ログ + 検証済みバイナリ）
- **隔離の穴 2 つ（#525 で実測）**: ①`tako lang` 等の CLI は IPC で稼働中の本番へ届くので、
  `HOME`/`APPDATA` を隔離しても本番 `settings.json` が変わる ②`claude.exe` は
  `HOME`/`USERPROFILE` の上書きを無視するので `claude mcp add` 経路は隔離できない
  （`tako setup` 自身の設定生成は APPDATA 隔離が効く）

## 現フェーズで Read すべき設計書

- 永続化を触る: `.agent/plans/2026-07-windows-persistence-backend.md`（§11 psmux 採用）+
  `crates/tako-core/src/backend/psmux.rs` 冒頭 doc
- 配布・リリースを触る: `installer/windows/release-windows.ps1` ヘッダ + AGENTS.md リリース節
- 新機能を足す: `platform/support.rs` の MATRIX に分類必須（パリティテスト T1/T3 が落ちる）+
  `scripts/gen-windows-support-docs.mjs` で doc 再生成
- プレビュー（PDF / 動画）を触る: `.agent/plans/2026-07-windows-port-architecture.md` の B12 行 +
  `crates/tako-app/src/platform/{pdf,video}/mod.rs` 冒頭 doc（罠は各 `windows.rs` の冒頭に全部書いてある）
