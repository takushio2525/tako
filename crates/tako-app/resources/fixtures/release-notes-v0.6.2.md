## tako v0.6.2


Nightly patch release (automated). Changes since v0.6.1:
夜間パッチリリース（自動）。v0.6.1 以降の変更:

- [修正] 縦に積む UI（バナー等）表示中にペインの PTY 行数が可視行数を超える問題を根治 (#684) (#689)
- [ドキュメント] #680 完了を progress / activeContext へ反映
- [機能追加] Markdown プレビュー: リンクの ⌘+クリックでブラウザ起動 + コードブロックのコピーボタン (#680) (#685)
- [改善] コマンド提案カードを会話内容にアンカーするインライン表示へ (#681) (#683)
- [ドキュメント] activeContext を 07-30 バッチ完了状態へ更新
- [修正] Code Runner の tako run が focus 未指定でも新ペインへフォーカスを奪う問題を根治 (#676) (#678)
- [修正] コードプレビュー（非 md）の構文色がライトテーマで読めない問題を根治 (#669) (#677)
- [機能追加] AI コマンド提案カード: AI が提示するコマンドをワンクリックコピー / 新規ペイン実行できる (#666) (#675)
- [修正] visual-test のインデントガイド節が main で失敗し以降の全節が止まる問題を根治 (#668) (#673)
- [改善] Markdown プレビューの高品質化: GFM テーブル対応 + 配色・タイポグラフィ全面改善 (#656) (#667)
- [修正] PC 再起動後に master ペインだけ claude 会話が resume されない問題を根治 (#652) (#661)
---

### ダウンロード / Download

| OS | ファイル / File |
|---|---|
| macOS | `tako-v0.6.2-macos-arm64.zip` |

### インストール（macOS） / Install (macOS)

1. 上の表の macOS 用 zip をダウンロード / Download the macOS zip from the table above
2. zip を展開（ダブルクリック） / Extract the zip
3. `tako.app` を `/Applications` フォルダへドラッグ / Drag `tako.app` to `/Applications`
4. 初回起動時に Gatekeeper の警告が出たら:
   **システム設定 → プライバシーとセキュリティ → 「tako」のブロック解除 → このまま開く**
   If Gatekeeper warns on first launch:
   **System Settings → Privacy & Security → Unblock "tako" → Open Anyway**

### Claude Code 連携（初回 1 回） / Claude Code Setup (one-time)

```sh
claude mcp add --scope user tako -- /Applications/tako.app/Contents/MacOS/tako mcp serve
```

## What's Changed
* [修正] PC 再起動後に master ペインだけ claude 会話が resume されない問題を根治 (#652) by @takushio2525 in https://github.com/takushio2525/tako/pull/661
* [改善] Markdown プレビューの高品質化: GFM テーブル対応 + 配色・タイポグラフィ全面改善 (#656) by @takushio2525 in https://github.com/takushio2525/tako/pull/667
* [修正] visual-test のインデントガイド節が main で失敗し以降の全節が止まる問題を根治 (#668) by @takushio2525 in https://github.com/takushio2525/tako/pull/673
* [機能追加] AI コマンド提案カード: AI が提示するコマンドをワンクリックコピー / 新規ペイン実行できる (#666) by @takushio2525 in https://github.com/takushio2525/tako/pull/675
* [修正] コードプレビュー（非 md）の構文色がライトテーマで読めない問題を根治 (#669) by @takushio2525 in https://github.com/takushio2525/tako/pull/677
* [修正] Code Runner の tako run が focus 未指定でも新ペインへフォーカスを奪う問題を根治 (#676) by @takushio2525 in https://github.com/takushio2525/tako/pull/678
* [改善] コマンド提案カードを会話内容にアンカーするインライン表示へ (#681) by @takushio2525 in https://github.com/takushio2525/tako/pull/683
* [機能追加] Markdown プレビュー: リンクの ⌘+クリックでブラウザ起動 + コードブロックのコピーボタン (#680) by @takushio2525 in https://github.com/takushio2525/tako/pull/685
* [修正] 縦に積む UI（バナー等）表示中にペインの PTY 行数が可視行数を超える問題を根治 (#684) by @takushio2525 in https://github.com/takushio2525/tako/pull/689


**Full Changelog**: https://github.com/takushio2525/tako/compare/v0.6.1...v0.6.2
