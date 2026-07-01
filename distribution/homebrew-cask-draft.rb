# Homebrew Cask formula ドラフト — tako
#
# 配置先: takushio2525/homebrew-tako リポジトリの Casks/tako.rb
#
# インストール:
#   brew tap takushio2525/tako
#   brew tap --trust takushio2525/tako   # Homebrew 6.0+ の Tap Trust
#   brew install --cask tako
#
# アンインストール:
#   brew uninstall --cask tako
#
# リリース時の更新手順:
#   1. scripts/release.sh --publish で GitHub Release + zip アップロード
#   2. shasum -a 256 dist/tako-v*.zip で SHA-256 を取得
#   3. 下記の version / sha256 / url を更新
#   4. homebrew-tako リポに commit & push
#
# TODO: arm64 / x86_64 の両アーキテクチャ対応時は
#       on_arm / on_intel ブロックで url / sha256 を分岐させる

cask "tako" do
  version "0.1.0"
  sha256 "REPLACE_WITH_ACTUAL_SHA256"

  url "https://github.com/takushio2525/tako/releases/download/v#{version}/tako-v#{version}-macos-arm64.zip"
  name "tako"
  desc "AI-driven terminal for agent-intensive monitoring"
  homepage "https://github.com/takushio2525/tako"

  # macOS 11.0 (Big Sur) 以降
  depends_on macos: ">= :big_sur"

  # tako.app を /Applications に配置
  app "tako.app"

  # tako CLI を PATH に追加（/usr/local/bin/tako → /Applications/tako.app/Contents/MacOS/tako）
  binary "#{appdir}/tako.app/Contents/MacOS/tako"

  # アンインストール時のクリーンアップ
  zap trash: [
    "~/Library/Application Support/dev.takushio.tako",
  ]

  caveats <<~EOS
    tako CLI が #{HOMEBREW_PREFIX}/bin/tako にリンクされました。

    Claude Code 連携（初回 1 回）:
      claude mcp add --scope user tako -- #{appdir}/tako.app/Contents/MacOS/tako mcp serve

    初回起動時に Gatekeeper の警告が出た場合:
      システム設定 → プライバシーとセキュリティ → 「tako」のブロック解除 → このまま開く
  EOS
end
