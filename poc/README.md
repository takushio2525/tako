# poc/ — Phase 0 技術検証スパイク

本実装（将来の `crates/`）とは**完全に分離**された使い捨て検証コード。
ここのコードは品質基準の対象外。検証結果は `.agent/architecture.md` と `.agent/roadmap.md` に反映済み。

## 構成

| ディレクトリ | 検証内容 | 結果 |
|---|---|---|
| `01-gpui-window-crates/` | crates.io 版 gpui 0.2.2 で最小ウィンドウ（macOS） | ✅ rustc 1.89 でビルド・起動・文字描画 OK |
| `02-gpui-window-git/` | zed リポ git 版 gpui（rev 固定）で最小ウィンドウ（macOS） | ✅ rustc 1.95 + `gpui_platform` + `font-kit` feature で OK |
| `03-term-poc/` | alacritty_terminal + PTY + GPUI の最小ターミナル | ✅ シェル起動・出力描画・キー入力 OK |

## 実行方法

```sh
cd poc/03-term-poc && cargo run
```

03 は起動 2 秒後にセルフテスト（GPUI キーディスパッチで `echo TAKO-INPUT-OK` を注入）が走り、
入力 → PTY → シェル実行 → グリッド反映まで通れば stdout に `TAKO_POC_INPUT_ECHO_VERIFIED` が出る。

## ハマりどころ（本実装でも踏む）

- **git 版 gpui は `gpui_platform` の `font-kit` feature を有効にしないと文字が一切描画されない**
  （テキストシステムがスタブになる。エラーも警告も出ない）
- git 版は `Application::new()` が廃止され `gpui_platform::application()` に分離（2026 年前半の破壊的変更）
- git 版は最新 stable Rust が必要（1.89 不可 → `rust-toolchain.toml` で 1.95.0 をピン）
- ウィンドウがオクルージョン状態（別 Space・完全に隠れている）だと display link が止まり
  再描画されない。「描画されない」デバッグの前にウィンドウの可視状態を疑うこと
- `WindowHandle<V>::update` 中に `dispatch_keystroke` するとルートビューの二重借用でパニック。
  `AnyWindowHandle::update` を使う
