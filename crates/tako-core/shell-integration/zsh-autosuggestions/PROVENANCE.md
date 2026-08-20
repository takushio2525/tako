# zsh-autosuggestions（同梱物の出所）

このディレクトリは第三者製の zsh プラグインを**改変せずそのまま**同梱している。
tako 本体（GPL-3.0-or-later）とは別ライセンス（MIT）なので、更新するときは
`LICENSE` と本ファイルも必ず一緒に更新すること。

| 項目 | 値 |
|---|---|
| 名前 | zsh-autosuggestions |
| バージョン | v0.7.1 |
| 取得元 | `https://github.com/zsh-users/zsh-autosuggestions/archive/refs/tags/v0.7.1.tar.gz` |
| 上流リポジトリ | https://github.com/zsh-users/zsh-autosuggestions |
| ライセンス | MIT（`LICENSE`） |
| 著作権 | Copyright (c) 2013 Thiago de Arruda / Copyright (c) 2016-2021 Eric Freese |
| tarball SHA-256 | `0df7affff21cd87ed298e6a3970ed08a1dd66a6efa676454ee5b091ad503badf` |
| `zsh-autosuggestions.zsh` SHA-256 | `eec7ba8f7a71414ace0ea0fab0908d005b24cf65d83b169c0ff97815d3cfc51a` |
| 同梱日 | 2026-07-27（Issue #600） |

同梱しているのは配布物のうち次の 2 ファイルだけ。

- `zsh-autosuggestions.zsh` — 単一ファイルで完結する本体（上流の `make` 済み成果物）
- `LICENSE` — MIT ライセンス全文

## 改変ポリシー

**このファイルには手を入れない**（差分が生まれると上流追従の検証コストが跳ね上がる）。
tako 固有の挙動（読み込みタイミング・二重注入ガード・ON/OFF）はすべて
`../zshenv.zsh` 側に置く。

## 更新手順

```sh
V=0.7.2   # 新しいタグ
curl -sL -o /tmp/zsh-as.tar.gz \
  "https://github.com/zsh-users/zsh-autosuggestions/archive/refs/tags/v$V.tar.gz"
shasum -a 256 /tmp/zsh-as.tar.gz          # 本ファイルの tarball SHA-256 を更新
tar xzf /tmp/zsh-as.tar.gz -C /tmp
cp /tmp/zsh-autosuggestions-$V/zsh-autosuggestions.zsh \
   /tmp/zsh-autosuggestions-$V/LICENSE .
shasum -a 256 zsh-autosuggestions.zsh     # 本ファイルの SHA-256 を更新
```

更新後は `crates/tako-core/src/shell_integration.rs` の `AUTOSUGGEST_VERSION` を
合わせ、`cargo test -p tako-core shell_integration` を通すこと
（同梱物の実在とバージョン表記の一致をテストが検査する）。
