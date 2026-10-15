# PowerShell の構文定義（同梱物の出所）

two-face（bat 由来の構文セット）は fancy-regex で組めない正規表現を含む構文を外していて、
PowerShell はそのひとつ（bat 同梱の定義はサブルーチン呼び出し `\g<1>` を使う）。tako は
fancy-regex で全部の正規表現が組める Sublime Text 公式の PowerShell パッケージの定義を
**改変せずそのまま**同梱し、ビルド時に `crates/tako-app/build.rs` が読む（Issue #1954）。
tako 本体（GPL-3.0-or-later）とは別ライセンス（MIT）なので、更新するときは
`LICENSE` と本ファイルも必ず一緒に更新すること。

| 項目 | 値 |
|---|---|
| 名前 | PowerShell（Sublime Text パッケージ） |
| バージョン | `st4107-5.1.2`（タグ） |
| コミット | `45b7abf95ec46a371a244c6778c7b09fe195264a` |
| 取得元 | `https://github.com/SublimeText/PowerShell/blob/st4107-5.1.2/PowerShell.sublime-syntax` |
| 上流リポジトリ | https://github.com/SublimeText/PowerShell |
| ライセンス | MIT（`LICENSE` = 上流の `LICENSE.txt`。改行だけ CRLF → LF） |
| 著作権 | Copyright (c) 2011-2015 Guillermo López-Anglada / 2020-2025 Michael Lyons |
| `PowerShell.sublime-syntax` SHA-256 | `33815ed0094261dd4074b5af7edbbe21e55df3ea6456212cd3fd3d9704494f28` |
| 同梱日 | 2026-10-16（Issue #1954） |

## 読み込むときの置き換え（ファイルは改変しない）

`build.rs` の `compile_extra_syntaxes` が読む前に次を置き換える。置き換え元が見つからなければ
ビルドが落ちる（上流が変わったら、ここと置き換えを見直す）。

| 元 | 後 | 理由 |
|---|---|---|
| `scope:source.cs.embedded.powershell` | `scope:source.cs` | ヒアドキュメントの中の C#（`Add-Type -TypeDefinition @' … '@`）は上流の別ファイル `syntax/embed/C# (for PowerShell).sublime-syntax` を名指す。そのファイルは Sublime Text 4 の `extends` で書かれていて syntect は読めないので、two-face の C# へ向ける（結べない参照は塗る途中で panic する） |

ほかに `first_line_match` の変数（`{{shebang_language}}`）を構文の変数で展開する（syntect は
文脈の正規表現の変数は展開するが `first_line_match` は展開しない。展開しないと 1 行目での
構文の判定が組めない正規表現で panic する）。

## 更新手順

```sh
TAG=st4107-5.1.2   # 新しいタグ
curl -sL -o PowerShell.sublime-syntax \
  "https://raw.githubusercontent.com/SublimeText/PowerShell/$TAG/PowerShell.sublime-syntax"
curl -sL "https://raw.githubusercontent.com/SublimeText/PowerShell/$TAG/LICENSE.txt" \
  | tr -d '\r' > LICENSE
shasum -a 256 PowerShell.sublime-syntax   # 本ファイルの SHA-256 を更新
```

更新後は `cargo test -p tako-app syntax_extra` を通すこと（全部の正規表現が fancy-regex で
組めること・既存の 213 構文が 1 バイトも変わらないこと・syntect の builder で全体を作り直した
結果と一致することを単体テストが見る）。比較と採否の経緯は
`.agent/plans/2026-10-language-coverage.md` §6.2。
