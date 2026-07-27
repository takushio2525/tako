# Windows 実機セットアップ手順（Issue #467）

Windows 機で tako を初めてビルドするための手順。**上から順にそのまま実行できる粒度**で書いてある。

- 対象: Windows 11（x64）。Windows 10 でも 1809 以降なら ConPTY は動く見込み
- CI（GitHub Actions）は使わない方針のため、Windows の検証はこの手順で各自のローカルに行う
- 設計の正: `.agent/plans/2026-07-windows-port-architecture.md`
- 到達目標: `cargo build --workspace` が成功する（**起動して動くのは次のフェーズ**）

---

## 1. Visual Studio Build Tools（MSVC）

Rust の `x86_64-pc-windows-msvc` ターゲットは MSVC のリンカと Windows SDK を使う。

PowerShell（管理者）で:

```powershell
winget install --id Microsoft.VisualStudio.2022.BuildTools --override `
  "--quiet --wait --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"
```

GUI で入れる場合は「Visual Studio Installer」→ **C++ によるデスクトップ開発** を選択し、
右側のオプションで次の 2 つが入っていることを確認する。

- MSVC v143 ビルドツール（x64/x86）
- Windows 11 SDK

### Spectre 緩和ライブラリ

一部の依存クレートは Spectre 緩和版のランタイムを要求する。個別コンポーネントから
**「MSVC v143 - VS 2022 C++ x64/x86 Spectre 緩和ライブラリ (最新)」** も追加する。

（リポジトリの旧 CI 設定でも同じものを追加していた）

インストール後は**一度サインアウトまたは再起動**する（環境変数を反映させるため）。

## 2. Rust ツールチェーン

```powershell
winget install --id Rustlang.Rustup
```

インストール後、新しい PowerShell を開いて確認する。
このリポジトリは `rust-toolchain.toml` でバージョンを固定しているため、
リポジトリ内で `cargo` を叩けば自動的に正しいツールチェーンが入る。

```powershell
rustup show
rustc --version
```

## 3. Git と Node.js

```powershell
winget install --id Git.Git
winget install --id OpenJS.NodeJS.LTS
```

Node.js が要るのは、リモート用 PWA（`web/tako-remote`）を `rust_embed` でバイナリに
埋め込むため。**`dist` が無いと `tako-control` がコンパイルできない。**

## 3.5. psmux（セッション永続化の器。任意だが強く推奨。#519 M2）

```powershell
winget install --id marlocarlo.psmux   # ID は marlocarlo.psmux（psmux.psmux は存在しない）
psmux -V                               # 2 行目に psmux <version> が出れば OK
```

Windows の永続バックエンド（tako を閉じても実行中プロセスと画面が生き残る器）は
psmux が担う。**入れなくても tako は動く**が、その場合はタブ・ペイン構成と cwd だけの
復元になり、実行中のエージェントは tako 終了時に止まる。

- scoop で入れるなら**先にバケット追加が要る**（upstream README。素の `scoop install psmux` は
  マニフェストが見つからず失敗する）:
  `scoop bucket add psmux https://github.com/psmux/scoop-psmux` → `scoop install psmux`
- 適合検証済みバージョンは `tako-core::backend::psmux::VERIFIED_VERSION`。
  違うバージョンでも起動時プローブ（器を作る → 見つける → 壊す）が通れば使う。
  通らなければ警告を出して構成のみ復元へ落ちる
- 導入したか・器として使われているかは `tako persist` の `backend.label` で分かる
  （`psmux` なら器あり、`none` なら構成のみ）
- PATH に置かず試すなら `TAKO_PSMUX_BIN=<psmux.exe のパス>` で明示指定できる
- psmux は `tmux.exe` も PATH に置くが、tako は `-V` の 2 行目で正体を判別するので
  本物の tmux と取り違えない。**素の `tmux kill-server` を打つと `-L` を越えて
  tako の器まで全滅する**（psmux の実測挙動）ので、器を掃除したいときは
  `tako recover` か `psmux -L tako kill-server` を使うこと

## 4. リポジトリの取得

```powershell
cd $HOME
git clone https://github.com/takushio2525/tako.git
cd tako
```

改行コードについて: リポジトリには `.gitattributes` がある。
`core.autocrlf` を勝手に変更しないこと（シェル統合スクリプトが LF 必須のため）。

## 5. PWA のビルド（Rust ビルドの前に必須）

```powershell
cd web\tako-remote
npm ci
npm run build
cd ..\..
```

`web\tako-remote\dist` が生成されていることを確認する。

## 6. ビルド

```powershell
cargo build --workspace
```

### 期待される結果（2026-07-25 時点）

- **コンパイルエラーは出ない見込み**。macOS からのクロス検査
  （`cargo check --workspace --target x86_64-pc-windows-msvc`）はエラーゼロで通っている
- **警告は出る**。macOS 専用実装に対する `dead_code` 警告が十数件出るが、これは想定内。
  各抽象境界の Windows 実装が入るにつれて自然に消える
- **リンク段は実機が初出**。クロス検査はリンクを行わないため、
  ここで初めて出るエラー（シンボル解決・import ライブラリ不足）はあり得る。
  出たら Issue #467 にログ全文を貼ること

### つまずいたときの確認順

| 症状 | 確認 |
|---|---|
| `link.exe` が見つからない | 手順 1 の Build Tools。サインアウトして環境変数を反映したか |
| `assert.h` などの C ヘッダが無い | Windows SDK が入っていない（手順 1 の SDK チェック） |
| Spectre 関連のリンクエラー | 手順 1 の Spectre 緩和ライブラリ |
| `RustEmbed folder ... does not exist` | 手順 5 の PWA ビルド未実施 |
| ビルドが極端に遅い | Defender の除外に `%USERPROFILE%\.cargo` と リポジトリの `target` を追加する |

## 7. 起動を試す（このフェーズでは失敗して構わない）

```powershell
cargo run -p tako-app
```

現時点では**ウィンドウが出ない・ペインが起動しないのが想定どおり**。
`terminal.rs` の既定シェル解決が Windows で未実装（`None` を返す）ためペインを spawn できない。
これは P1（最小 GUI 起動）の担当範囲。

観測した内容（ウィンドウが出たか、パニックしたか、ログに何が出たか）を Issue に記録すると、
P1 の着手材料になる。

## 8. 開発中の tako 本体を触る場合

Windows 側で機能実装に入るときは、次を読む。

- `.agent/plans/2026-07-windows-port-architecture.md` — 抽象境界のカタログ。
  **プラットフォーム分岐は境界の内側にだけ書く**
- `.agent/plans/2026-07-windows-port-survey.md` — 何が動かないかの全数調査

macOS 側で開発している人は、コミット前に `scripts/check-windows.sh` を回すと
Windows のコンパイル崩れを持ち込まずに済む。
