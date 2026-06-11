# Active Context

> このファイルは AI が**毎ターン上書き更新**する現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。ここには履歴を残さない。
> セッション開始時に AGENTS.md の直後に必ず読む。

## 現在の対象

- 何を / どこを: **Phase 3.5（日常使い品質）を実施中。IME 変換中表示（FR-1.9）の実装が途中**
  （利用枠の都合で WIP コミットして中断。下記「再開手順」から続行すること）
- ステータス: `crates/tako-app/src/main.rs` に IME の土台（状態・ヘルパー・二重入力防止）まで
  入れた状態。コンパイルは通る（未使用警告あり = 未配線のため想定どおり）
- 最終更新: 2026-06-11

## 再開手順（次セッションはここから）

Phase 3.5 の全体スコープ: (1) IME 変換中表示（FR-1.9）、(2) .app バンドル化
（`scripts/build-app.sh` + icns + Info.plist + release profile + README 手順）、
(3) アイコン選定結果の記録（A 案採用、B〜D 没を `assets/icon/README.md` へ）。
完了時に roadmap / activeContext / progress 更新、fmt / clippy -D warnings / test 維持、
機能単位コミット + push + CI 両 OS 緑確認。

### (1) IME — 実装済み（main.rs に入っている）

- `ImeComposition` 構造体（pane / text / selected_utf16）+ `TakoApp.ime` フィールド
- `utf16_to_byte_offset` / `utf16_len`（NSTextInputClient は範囲をすべて UTF-16 で渡す）
- ヘルパー `ime_target` / `pane_cursor_origin` / `ime_prefix_width`（shape_line で幅算出）
- `handle_key` に `cx.stop_propagation()` 追加 —— **これが二重入力防止の要**。
  macOS は KeyDown コールバックが未処理だと inputContext へ回送し insertText →
  `replace_text_in_range` で同じ文字が二重に PTY へ入る
- gpui の import 追加済み: `canvas, ElementInputHandler, EntityInputHandler, TextRun,
  UTF16Selection, Range`

### (1) IME — 残作業（設計確定済み、書くだけ）

1. **`impl EntityInputHandler for TakoApp`**（main.rs、ControlHost impl の近くへ）。
   擬似ドキュメント = 未確定文字列のみ、という設計：
   - `text_for_range`: ime.text の UTF-16 範囲を substring して返す
   - `selected_text_range`: 変換中 = `selected_utf16`（無ければ末尾キャレット end..end）、
     非変換中 = `0..0`。`UTF16Selection { range, reversed: false }`
   - `marked_text_range`: `ime.as_ref().map(|i| 0..utf16_len(&i.text))`
   - `unmark_text`: **そのまま挿入してクリア**（NSTextInputClient の規約。ime.pane の
     session へ `write(text.into_bytes())`）
   - `replace_text_in_range(_range, text)`: 確定。`ime.take()` の pane（無ければ focused）の
     session へ `clear_selection()` + `write(...)`、`cx.notify()`
   - `replace_and_mark_text_in_range(_range, new_text, sel)`: `new_text` 空なら ime = None
     （変換キャンセル）、それ以外は ime を丸ごと差し替え（pane は既存変換の pane を引き継ぐ）
   - `bounds_for_range(range, _element_bounds, window)`: `pane_cursor_origin(ime_target())` +
     `ime_prefix_width(text[..range.start のバイト位置])` を x に加算、サイズはセル寸法。
     カーソル非表示なら None（候補ウィンドウの位置出し用）
   - `character_index_for_point`: None でよい
2. **render() に 2 つ追加**:
   - IME オーバーレイ: `self.ime` があり `pane_cursor_origin(ime.pane)` が取れたら、
     ペインコンテナ（`div().flex_1().relative()`）の子に
     `div().absolute().left(anchor.x - content_origin.x).top(anchor.y - content_origin.y)
     .h(px(theme.line_height)).bg(rgba(theme.background))` + `StyledText` で未確定文字列を描画。
     ハイライト: 全体に細下線（thickness 1）、`selected_utf16`（バイト範囲へ変換）に
     太下線（thickness 2, accent 色）+ `selection_background`
   - 入力ハンドラ登録: `Window::handle_input` は **paint フェーズ限定 API** のため
     `canvas(|_,_,_| (), move |_,_,window,cx| window.handle_input(&focus,
     ElementInputHandler::new(target_bounds, entity), cx)).absolute().size_full()` を
     ルート div の子として置く。`entity = cx.entity()`、`focus = self.focus_handle.clone()`、
     `target_bounds` = フォーカスペインの text area（無ければコンテンツ領域）
3. **セルフテスト 37〜39 追加**（36 の後。実 IME イベントは合成できないため
   EntityInputHandler のメソッドを `window.update(cx, |app, window, cx| ...)` から直接呼ぶ）:
   - 37: `replace_and_mark_text_in_range(None, "にほんご", Some(0..4))` → marked_text_range が
     `Some(0..4)`（"にほんご" = UTF-16 で 4）、`bounds_for_range` が Some、
     PTY に「にほんご」が**現れない**こと
   - 38: `replace_text_in_range(None, "echo IME-$((40+2))-にほんご")` → ime クリア、
     enter 後に画面へ `IME-42-にほんご`
   - 39: mark → `unmark_text` → ime クリア + 「そのまま挿入」で画面に現れる
   - 冒頭 doc コメントのセルフテスト説明と AGENTS/roadmap の「36 項目」表記を 39 へ更新
4. **手動チェックリスト** `.agent/manual-checks.md` を新規作成（変換中表示は機械検証不能のため）:
   日本語 IME で「にほんご」入力 → 下線付きで変換前表示／スペースで変換 → 文節下線／
   候補ウィンドウがカーソル直下に出る／enter 確定で 1 回だけ入力される／esc キャンセル／
   英数直打ちで二重入力しない（stop_propagation の回帰確認）／dead key（option-e 等）。
   AGENTS.md の詳細仕様リストにバックティック参照を 1 行追加
5. fmt / clippy / test → `TAKO_SELF_TEST=1 cargo run -p tako-app` 緑 → コミット
   `[機能追加] IME 変換中表示（FR-1.9）...`

### (2) .app バンドル — 未着手（方針決定済み）

- **cargo-bundle は不採用**（メンテ停滞・icns 生成も結局別途必要・macOS 専用なら素のスクリプトで
  十分）→ `scripts/build-app.sh` を新規作成。冒頭で比較理由を 1 行コメント
- 手順: release ビルド（tako-app + tako-cli）→ icns 生成（rsvg-convert があれば
  `assets/icon/icon-a.svg` から各サイズ、無ければ `assets/icon/preview/icon-a-1024.png` から
  sips で縮小）→ iconset（16/32/128/256/512 の @1x/@2x）→ `iconutil -c icns` →
  `dist/tako.app/Contents/{MacOS,Resources}` 組み立て（**tako CLI も MacOS/ に同梱**。
  `claude mcp add` の登録先パスを安定させるため）→ Info.plist（CFBundleExecutable=tako-app,
  CFBundleIconFile, LSMinimumSystemVersion 11.0, NSHighResolutionCapable）→ ad-hoc codesign
  （nested の tako を先に個別署名 → バンドル署名）
- `--verify` フラグで `TAKO_SELF_TEST=1 dist/tako.app/Contents/MacOS/tako-app` を実行し
  `TAKO_APP_SELF_TEST_OK` を grep（バンドル版でも TAKO_*/IPC/MCP が生きる確認。
  セルフテスト step 14 は cargo build を叩くためリポジトリ内から実行すること）
- ルート Cargo.toml に `[profile.release] lto = "thin", codegen-units = 1, strip = "symbols"`
- README.md に「.app のビルドと /Applications への配置」節、AGENTS.md コマンド表に 1 行追加

### (3) アイコン記録 — 未着手

- `assets/icon/README.md` に「A 案採用（2026-06-11）、B〜D は没として保存」を追記し、
  「今後のタスク（選定後）」節を build-app.sh への参照に書き換え

### その他の整合性メモ

- FR-1.9 は「AI フルコントロール不変条件」の例外説明を requirements.md に 1 行足す
  （IME は OS 入力メソッド統合であり、AI からの等価操作は `tako_send_input` で提供済み）
- gpui ソース参照は `~/.cargo/git/checkouts/zed-a70e2ad075855582/cafbf4b/crates/gpui*` のみ
  （Apache-2.0。zed の editor/terminal クレートは GPL 系なので読まない）

## 現フェーズで Read すべき設計書

- IME 続行時: このファイルの「再開手順」+ `requirements.md` FR-1.9。gpui の参照は
  `crates/gpui/src/input.rs`（EntityInputHandler）と `crates/gpui_macos/src/window.rs` の
  keyDown 分岐（2020 行目あたり）
- Phase 4 着手時: `.agent/architecture.md`「Layer 3」節 + `requirements.md` FR-2.4 / FR-2.10 /
  FR-2.1.3〜2.1.4

## 未解決・次の一手

- [ ] Phase 3.5 (1): IME 残作業（上記 1〜5。設計確定済み、書くだけ）
- [ ] Phase 3.5 (2): .app バンドル化（build-app.sh）
- [ ] Phase 3.5 (3): アイコン選定記録
- [ ] Phase 3 残: role ラベル / 状態表示 UI（Phase 4 集約センターと併せて）
- [ ] Phase 4: パッシブ検知
- [ ] Phase 1 残骨格: ドラッグでのペイン境界リサイズ

## 関連ファイル / リンク

- リポジトリ: https://github.com/takushio2525/tako（private）
- 仕様一式: `.agent/concept.md` / `requirements.md` / `architecture.md` / `roadmap.md`
- MCP 実機検証: `scripts/verify-claude-mcp.sh`
