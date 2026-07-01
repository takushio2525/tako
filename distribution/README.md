# tako 配布方法 — 調査結果と実装ガイド

> 調査日: 2026-07-01
> 対象: macOS 向け GUI ターミナルアプリ `tako.app`（Rust + GPUI）

## 現状

- GitHub Releases から zip をダウンロード → 手動で `/Applications` に配置
- Apple Developer ID 署名は未実施（ad-hoc または Apple Development 証明書でローカル署名）
- Gatekeeper 警告が出る（「開発元を確認できないため開けません」）
- ビルドは `scripts/build-app.sh` → `dist/tako.app`
- リリースは `scripts/release.sh --publish` → GitHub Releases

---

## 1. Homebrew Cask（自前 tap）

### 概要

`brew install --cask takushio2525/tako/tako` で tako をインストールできるようにする。
自前 tap（`takushio2525/homebrew-tako`）に Cask formula を置き、GitHub Releases の zip を参照する。

### 要件

| 項目 | 状態 |
|------|------|
| GitHub リポジトリ `takushio2525/homebrew-tako` | 未作成 |
| GitHub Releases に zip アップロード | **済み**（`release.sh` で対応） |
| Cask formula（`Casks/tako.rb`） | ドラフト済み（本ディレクトリ） |
| Apple 署名 / notarization | **不要**（自前 tap では必須ではない） |

### 自前 tap の構成

```
takushio2525/homebrew-tako/
├── Casks/
│   └── tako.rb          ← Cask formula
└── README.md
```

### Apple 署名なしでの配布

**自前 tap では署名なしで配布可能**。ただし以下の制約がある:

- **公式 homebrew-cask（本家）への掲載は不可**: 2026年9月以降、署名・notarization のない
  Cask は公式 tap から削除される方針（[Discussion #6482](https://github.com/orgs/Homebrew/discussions/6482)）
- ユーザーは初回起動時に Gatekeeper の手動解除が必要
  （システム設定 → プライバシーとセキュリティ → ブロック解除）
- Homebrew 6.0.0 の **Tap Trust** 機能により、サードパーティ tap は
  `brew tap --trust takushio2525/tako` での明示的な信頼が必要になった

### インストール手順（ユーザー向け）

```sh
# tap を追加して信頼（初回のみ）
brew tap takushio2525/tako
brew tap --trust takushio2525/tako

# インストール
brew install --cask tako

# 初回起動時に Gatekeeper 警告が出たら:
# システム設定 → プライバシーとセキュリティ → 「tako」のブロック解除
```

### CLI の PATH 設置

Cask formula の `binary` stanza で `/Applications/tako.app/Contents/MacOS/tako` を
`/usr/local/bin/tako`（または `$(brew --prefix)/bin/tako`）にシンボリックリンクする。
Homebrew がリンク管理するため、`brew uninstall` で自動削除される。

### リリース時のワークフロー

1. `scripts/release.sh --publish` で GitHub Release + zip アップロード
2. zip の SHA-256 を取得: `shasum -a 256 dist/tako-v*.zip`
3. `homebrew-tako` リポの `Casks/tako.rb` を更新（version + sha256 + url）
4. commit & push

このステップ 2〜4 は `scripts/update-homebrew.sh` で自動化可能（将来タスク）。

---

## 2. .pkg インストーラー

### 概要

macOS 標準の `.pkg` 形式で配布する。ダブルクリックでインストーラー UI が起動し、
`/Applications` に `tako.app` を配置する。

### ビルド方法

```sh
# 1. pkgbuild でコンポーネントパッケージを作成
pkgbuild \
  --root ./payload \
  --identifier dev.takushio.tako \
  --version 0.1.0 \
  --install-location /Applications \
  tako-component.pkg

# 2. productbuild で配布パッケージ（UI 付き）を作成
productbuild \
  --distribution distribution.xml \
  --package-path . \
  --resources ./resources \
  tako-installer.pkg
```

ドラフトスクリプト `build-pkg.sh` を本ディレクトリに用意済み。

### 署名なしの場合

- **配布自体は可能**だが、ダブルクリック時に Gatekeeper が「開発元不明」として**ブロックする**
- ユーザーは右クリック → 「開く」 または システム設定 → ブロック解除が必要
- zip 配布と比べて UX の改善が小さい（どちらも Gatekeeper 警告が出る）

### 署名ありの場合

| 必要なもの | 費用 | 用途 |
|-----------|------|------|
| Apple Developer Program | $99/年 | Developer ID 証明書の取得 |
| Developer ID Installer 証明書 | 無料（Program 内） | .pkg の署名 |
| Developer ID Application 証明書 | 無料（Program 内） | .app の署名 |
| `xcrun notarytool` | 無料 | Apple への notarization 送信 |

```sh
# .pkg の署名
productsign --sign "Developer ID Installer: Your Name (TEAMID)" \
  tako-unsigned.pkg tako-signed.pkg

# notarization
xcrun notarytool submit tako-signed.pkg \
  --apple-id "your@email.com" \
  --team-id "TEAMID" \
  --password "@keychain:AC_PASSWORD" \
  --wait

# staple（オフライン検証用）
xcrun stapler staple tako-signed.pkg
```

### macOS 26.3（Tahoe）の .pkg 問題 ⚠️

**2026年7月時点で、macOS 26.3 において Developer ID Installer で署名・notarization 済みの
.pkg が Gatekeeper に拒否されるバグが報告されている**。
.app バンドル（Developer ID Application 署名）は問題ない。
Apple Developer Forums で議論中だが、修正時期は未定。

この問題があるため、**現時点では .pkg への投資は時期尚早**。

---

## 3. 優先順位の提案

### 推奨ロードマップ

| 順番 | 施策 | 工数 | 前提条件 | 効果 |
|------|------|------|----------|------|
| **1（推奨）** | **Homebrew Cask（自前 tap）** | 小（1-2時間） | なし | `brew install` でインストール可能。CLI も PATH に入る |
| 2 | Apple Developer Program 加入 | - | $99/年 | 署名 + notarization の前提 |
| 3 | .app の署名 + notarization | 中（半日） | Developer Program | Gatekeeper 警告の完全除去 |
| 4 | 公式 homebrew-cask への掲載 | 小 | 署名 + notarization | `brew install --cask tako` で入る（tap 不要） |
| 5 | .pkg インストーラー | 中 | Developer Program + macOS バグ修正 | DMG 代替（現状は非推奨） |

### Homebrew Cask を先にやるべき理由

1. **コストゼロ**: Apple Developer Program 不要、GitHub リポ 1 個作るだけ
2. **CLI の PATH 問題を解決**: `binary` stanza で `/usr/local/bin/tako` にリンクされる
3. **アップデートが楽**: `brew upgrade tako` で更新可能
4. **開発者なら Homebrew は入っている**: ターゲットユーザー（AI で開発する開発者）にとって
   最も自然なインストール方法

### .pkg を後回しにすべき理由

1. **macOS 26.3 のバグ**: 署名 + notarization しても Gatekeeper に拒否される（.app は問題なし）
2. **署名なしでは zip と同じ UX**: Gatekeeper 警告が出るので zip 配布との差分が小さい
3. **Homebrew で十分**: ターゲットユーザーは開発者であり、`brew install` で事足りる
4. **投資対効果が低い**: pkgbuild + productbuild + 署名 + notarization の手順は重い

### Apple Developer Program（$99/年）の必要性

**Homebrew 自前 tap だけなら不要**。ただし以下を実現するには必須:

- Gatekeeper 警告の完全除去（= 非エンジニアでも安心してインストール）
- 公式 homebrew-cask への掲載（2026年9月以降は署名必須）
- 将来の .pkg / DMG 配布
- 自動アップデート（Sparkle は notarization 済みアプリが前提）

Phase 7（公開準備）で自動アップデートや非エンジニア向け配布を目指すなら、
そのタイミングで加入するのが合理的。

---

## ファイル一覧

| ファイル | 説明 |
|---------|------|
| `distribution/README.md` | 本ファイル（調査結果まとめ） |
| `distribution/homebrew-cask-draft.rb` | Cask formula ドラフト |
| `distribution/build-pkg.sh` | .pkg ビルドスクリプト ドラフト |
| `distribution/distribution.xml` | productbuild 用 distribution 定義 |
