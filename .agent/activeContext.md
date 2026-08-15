# Active Context

> このファイルは AI が毎ターン上書きする現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。

## 現在の対象（2026-08-15、#828 = window close 後の OS ウィンドウ残留）

- ブランチ `fix/828-window-close`（worktree `~/dev/tako-wt-828`、base = `9484e15`）
- #819 の調査から分離した子 Issue。#814 の削減シリーズの一部

## わかったこと（Issue の目星は反証された）

`sync_viewports` の `let _ = handle.update(...)` は**毎回 `Ok`** を返していて、gpui の
ウィンドウ登録も毎回減っている（計装実測）。AppKit 側も `MacWindow::drop` が走り
（`delegate=nil`）ウィンドウは order out されている（`isVisible=false`）。
残るのは **NSWindow オブジェクトが解放されない**ことだけで、`retainCount` は 24 → 8 の
まま減衰せず、CAMetalLayer の drawable（既定窓 9.1MB）が返らない。

**tako の欠陥ではない**: tako のコードを 1 行も含まない素の gpui アプリで同じ状態になり、
**赤ボタン相当（NSWindow へ直接 close）でも完全に同じ**。閉じ方を変えても結果が同じなので
tako 側に「正しく解放される閉じ方」は無い。`leaks` も到達不能リークを報告しない
（= AppKit が意図的に保持している）。

## やったこと（挙動は変えていない）

close 失敗を握り潰していた `let _ =` を、発生源つきの persist.log 記録へ。
`sync_viewports(origin, cx)` の `origin` は `render` / `dispatch` / `selftest`。
**再試行はしない**ので挙動は従来どおり（#169 以来の fail-loud 規約に合わせただけ）。
対応表（`drop_viewport`）を先に落とす設計なので、失敗すると記録も無く孤児化する
（今回それを判定するのに計装ビルドを 1 本起こす必要があった）のを塞ぐのが目的。

## この環境で検証できなかったこと

`AppleClamshellState = Yes` / `Display Asleep: Yes`（外部ディスプレイ無し）のままで
蓋を開けられず、**「閉じた窓が画面から消える」の実ピクセル確認は不可能**
（`screencapture` は成功するが画像は全面黒）。CGWindowList が `onscreen=true` を
返し続けるのと AppKit の `isVisible=false` の食い違いが画面 OFF 由来かは未判定。

## 次の一手

- 蓋を開けて `~/dev/tako-evidence/828/repro828.sh` を再実行（5 分）。drawable が解放
  されるなら #828 は環境由来としてクローズ
- 残るなら上流（zed）へ報告するかは **master 判断**（`win828probe.rs` が最小再現）
- tako 側の緩和は「GPUI ウィンドウを close せず再利用」しか効かないが、
  `cx.windows()` が空になることを Dock 復帰の起点にしている **#381 と衝突**する

## 現フェーズで Read すべき設計書

- `.agent/architecture.md`「複数ウィンドウ（ビューポート方式）」= #339 / #380 / #381 の
  不変条件（同一 entity 共有・タブ排他・最後の 1 枚は entity を殺さない）
- 証拠一式は `~/dev/tako-evidence/828/`（再現ハーネス `repro828.sh` / 素の gpui プローブ
  `win828probe.rs` / CGWindowList 計測 `winlist.swift` / 計装パッチ / 全ログ）
