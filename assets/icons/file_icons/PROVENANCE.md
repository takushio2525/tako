# ファイル種別アイコン（同梱物の出所）

このディレクトリの SVG 61 個は、Zed のリポジトリの `assets/icons/file_icons/` から
**改変せずそのまま**取り込んだもの（2026-07-01 のコミット `19c8657` で追加）。
tako はこれを `crates/tako-app/src/file_icons.rs` から名前指定でバイナリへ埋め込み、
ファイルツリー・タブ・プレビューのファイル種別の印に使う。

| 項目 | 値 |
|---|---|
| 上流リポジトリ | https://github.com/zed-industries/zed |
| 上流の置き場 | `assets/icons/file_icons/` |
| 突き合わせた上流の版 | rev `cafbf4b5df7fedb67fc0f248850a5654efcec5d9`（tako が GPUI を取り込んでいる版。61 個すべてバイト一致を確認。Issue #1709） |
| ライセンス | GPL-3.0-or-later（tako 本体と同じ。下記） |
| 一部の由来 | Lucide（ISC。上流の `assets/icons/LICENSES` の告知。全文は `THIRD-PARTY-NOTICES.md`） |

## ライセンスの根拠

Zed のリポジトリはファイルごとの著作権表示を持たず、README の Licensing 節で
「Zed のソースは主に GPL-3.0-or-later で、Apache-2.0 の部分はその旨を明示している」と宣言している。
このアイコン群には Apache-2.0 の明示が無いので、GPL-3.0-or-later として扱う。
tako 本体も GPL-3.0-or-later なので、そのまま同梱してよい。
上流の `assets/icons/LICENSES` は、アイコンの一部が Lucide（ISC ライセンス）に由来すると告知している。
ISC は「著作権表示と許諾表示をすべての複製に含める」ことを求めるので、その告知を
リポジトリ直下の `THIRD-PARTY-NOTICES.md` に写してある。

## 更新するとき

- 上流から取り直す・足すときは、同じ版の `assets/icons/LICENSES` に変化が無いかを確かめ、
  変わっていれば `THIRD-PARTY-NOTICES.md` の該当節も直す
- tako 独自に描いたアイコンはここへ混ぜず、`assets/icons/ui/` に置く
- アイコンの中には各言語・ツールのロゴを図案化したもの（`rust.svg` `python.svg` `docker.svg` 等）がある。
  **ファイルの種類を示す目的だけ**に使い、tako の宣伝素材やアプリアイコンには使わない
  （ロゴは各権利者の商標であり、種類の表示を超える使い方は各社の商標ガイドラインの確認が要る）
