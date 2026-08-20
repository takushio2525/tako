#!/usr/bin/env bash
# release.sh — tako.app の zip を生成し、GitHub Releases へアップロードする（macOS 専用）
#
# 使い方:
#   scripts/release.sh              # ビルド → zip 生成まで（リリースは作成しない）
#   scripts/release.sh --publish    # zip 生成 + GitHub Release 作成・アップロード
#   scripts/release.sh --draft      # zip 生成 + ドラフトリリース作成
#   scripts/release.sh --skip-build # ビルド済み dist/tako.app を使って zip のみ再生成
#   scripts/release.sh --test       # テスト版（prerelease）としてリリース（#403）
#   scripts/release.sh --promote <test-tag>  # テスト版を安定版に昇格（#403）
#   scripts/release.sh --notes-only # リリースノートを生成して表示するだけ（ビルド・公開しない）
#   scripts/release.sh --update-notes [tag]  # 公開済みリリースのノートを実アセットから作り直す
#
# 前提:
#   - macOS（build-app.sh と同じ）
#   - --publish / --draft には gh CLI（`brew install gh`）+ 認証済み
#   - リポジトリのリモートが origin に設定されていること
#
# バージョンは Cargo.toml [workspace.package] から自動読み取り。
# リリースノートは CHANGELOG.md から該当バージョンのセクションを自動抽出。
#
# --- プラットフォーム対応（Issue #594）---
#
# リリースノートは Mac / Windows で分けず、単一ノート + プラットフォーム明示で運用する。
# ノートには実アセットから組み立てた**ダウンロード表**が入り、Windows 版の配布物が
# 含まれるときだけ Windows のインストール手順と Known limitations 節が付く
# （Known limitations は #515 のサポートマトリクスから `tako platform` 経由で自動生成）。
#
# macOS 先行リリース → 後から Windows 版を同じタグに足す運用:
#
#   1. Windows 版をビルドし、命名規則どおりの名前で dist/ に置く
#      （例: dist/tako-v0.6.0-windows-x86_64.exe。規則は scripts/lib/release-assets.sh）
#   2. gh release upload v0.6.0 dist/tako-v0.6.0-windows-x86_64.exe --clobber
#   3. scripts/release.sh --update-notes v0.6.0
#      （実アセットを読み直してノートを作り直し、gh release edit --notes で差し替える）
#
# アセットを足した時点で Windows クライアントの更新チェックにも初めて見えるようになる
# （#595。自 OS 用アセットが無いリリースは更新候補にならない）。
set -euo pipefail

cd "$(dirname "$0")/.."
REPO_ROOT=$PWD
DIST="$REPO_ROOT/dist"
APP="$DIST/tako.app"
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
TAG="v${VERSION}"

# アセット命名規則の写し（正は crates/tako-core/src/platform/release_assets.rs。
# 一致は cargo test -p tako-core release_assets が機械検証する）
# shellcheck source=lib/release-assets.sh
source "$REPO_ROOT/scripts/lib/release-assets.sh"

# Launch Services の登録ヘルパ（#837）。リリースが成立したらビルド出力を片付ける
# shellcheck source=lib/launch-services.sh
source "$REPO_ROOT/scripts/lib/launch-services.sh"

ARCH=$(uname -m)  # arm64 / x86_64
ZIP_NAME=$(tako_asset_name "$TAG" macos "$ARCH")
ZIP_PATH="$DIST/$ZIP_NAME"

PUBLISH=0
DRAFT=0
SKIP_BUILD=0
TEST_RELEASE=0
PROMOTE_TAG=""
NOTES_ONLY=0
UPDATE_NOTES_TAG=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --publish)    PUBLISH=1; shift ;;
    --draft)      DRAFT=1; shift ;;
    --skip-build) SKIP_BUILD=1; shift ;;
    --test)       TEST_RELEASE=1; PUBLISH=1; shift ;;
    --notes-only) NOTES_ONLY=1; shift ;;
    --update-notes)
      shift
      # タグ省略時は Cargo.toml のバージョン
      if [[ $# -gt 0 && "$1" != --* ]]; then
        UPDATE_NOTES_TAG="$1"; shift
      else
        UPDATE_NOTES_TAG="$TAG"
      fi ;;
    --promote)
      shift
      if [[ $# -eq 0 ]]; then
        echo "エラー: --promote にはテスト版タグを指定してください（例: --promote v0.6.0-test.1）" >&2
        exit 2
      fi
      PROMOTE_TAG="$1"; shift ;;
    *) echo "不明な引数: $1（--publish / --draft / --skip-build / --test / --notes-only / --update-notes [tag] / --promote <tag>）" >&2; exit 2 ;;
  esac
done

# --- CHANGELOG.md から該当バージョンのセクションを抽出 ---
# 注意: --promote / --update-notes からも呼ぶので、**必ず全処理より前に定義する**
extract_changelog() {
  local ver="$1"
  local file="$REPO_ROOT/CHANGELOG.md"
  if [[ ! -f "$file" ]]; then
    return
  fi
  local escaped_ver="${ver//./\\.}"
  sed -n "/^## \\[${escaped_ver}\\]/,/^## \\[/{
    /^## \\[${escaped_ver}\\]/d
    /^## \\[/d
    p
  }" "$file"
}

# --- リリースノートの組み立て（Issue #594）---

# tako CLI の場所を解決する（Known limitations 生成に使う）。
# ビルド済み .app 内 > release > debug の順。どれも無ければ空文字（節を省略する）
resolve_tako_bin() {
  local candidates=(
    "$APP/Contents/MacOS/tako"
    "$REPO_ROOT/target/release/tako"
    "$REPO_ROOT/target/debug/tako"
  )
  local c
  for c in "${candidates[@]}"; do
    if [[ -x "$c" ]]; then
      printf '%s' "$c"
      return 0
    fi
  done
  printf ''
}

# assets_for_platform <platform> <name...> — 指定 OS 向けのアセット名だけを出力
assets_for_platform() {
  local platform="$1"; shift
  local name
  for name in "$@"; do
    if tako_asset_is_for "$name" "$platform"; then
      echo "$name"
    fi
  done
}

# build_download_table <name...> — ダウンロード表（アセットがある OS の行だけ）
build_download_table() {
  local platform matched rows=""
  for platform in $TAKO_ASSET_PLATFORMS; do
    matched=$(assets_for_platform "$platform" "$@")
    [[ -n "$matched" ]] || continue
    local label file
    label=$(tako_asset_label "$platform")
    while IFS= read -r file; do
      [[ -n "$file" ]] || continue
      rows+="| ${label} | \`${file}\` |
"
    done <<< "$matched"
  done
  [[ -n "$rows" ]] || return 0
  printf '### ダウンロード / Download\n\n| OS | ファイル / File |\n|---|---|\n%s\n' "$rows"
}

# build_release_notes <tag> <version> <asset-name...>
# ダウンロード表・OS 別インストール手順・Known limitations を実アセットに応じて出し分ける
build_release_notes() {
  local tag="$1" version="$2"; shift 2
  local names=("$@")
  local body mac_assets win_assets notes

  notes="## tako ${tag}
"
  body=$(extract_changelog "$version")
  if [[ -n "$body" ]]; then
    notes+="
${body}
---
"
  fi

  local table
  table=$(build_download_table "${names[@]+"${names[@]}"}")
  if [[ -n "$table" ]]; then
    # $(...) は末尾改行を落とすので、次の節との間の空行はここで足す
    notes+="
${table}
"
  fi

  mac_assets=$(assets_for_platform macos "${names[@]+"${names[@]}"}")
  win_assets=$(assets_for_platform windows "${names[@]+"${names[@]}"}")

  if [[ -n "$mac_assets" ]]; then
    notes+="
### インストール（macOS） / Install (macOS)

1. 上の表の macOS 用 zip をダウンロード / Download the macOS zip from the table above
2. zip を展開（ダブルクリック） / Extract the zip
3. \`tako.app\` を \`/Applications\` フォルダへドラッグ / Drag \`tako.app\` to \`/Applications\`
4. 初回起動時に Gatekeeper の警告が出たら:
   **システム設定 → プライバシーとセキュリティ → 「tako」のブロック解除 → このまま開く**
   If Gatekeeper warns on first launch:
   **System Settings → Privacy & Security → Unblock \"tako\" → Open Anyway**
"
  fi

  if [[ -n "$win_assets" ]]; then
    notes+="
### インストール（Windows） / Install (Windows)

1. 上の表の Windows 用インストーラーをダウンロード / Download the Windows installer from the table above
2. インストーラーを実行 / Run the installer
   （SmartScreen の警告が出たら **詳細情報 → 実行** / If SmartScreen warns: **More info → Run anyway**）
"
    local tako_bin limits
    tako_bin=$(resolve_tako_bin)
    if [[ -n "$tako_bin" ]]; then
      limits=$("$tako_bin" platform --platform windows --known-limitations 2>/dev/null || true)
      if [[ -n "$limits" ]]; then
        notes+="
${limits}
"
      fi
    else
      echo "警告: tako バイナリが見つからないため Known limitations 節を省略しました" >&2
    fi
  fi

  notes+="
### Claude Code 連携（初回 1 回） / Claude Code Setup (one-time)

\`\`\`sh
claude mcp add --scope user tako -- /Applications/tako.app/Contents/MacOS/tako mcp serve
\`\`\`
"
  printf '%s' "$notes"
}

# dist/ にある、このタグ向けの配布物の**ファイル名**を列挙する。
# macOS の zip 以外（後から置いた Windows 版など）も拾う
collect_dist_asset_names() {
  local tag="$1" platform ext f
  for platform in $TAKO_ASSET_PLATFORMS; do
    for ext in $(tako_asset_ext_list "$platform"); do
      for f in "$DIST/${TAKO_ASSET_PREFIX}${tag}-${platform}-"*".${ext}"; do
        # nullglob 未設定なので未マッチ時はパターン文字列がそのまま入る → 実在チェックで弾く
        [[ -f "$f" ]] && basename -- "$f"
      done
    done
  done
  # 1 件も無いときに最後の [[ -f ]] の非 0 を漏らさない（set -e 対策）
  return 0
}

# --- 公開済みリリースのノートを実アセットから作り直す（--update-notes）---
if [[ -n "$UPDATE_NOTES_TAG" ]]; then
  if ! command -v gh >/dev/null; then
    echo "エラー: gh CLI が必要（brew install gh）" >&2
    exit 1
  fi
  if ! gh release view "$UPDATE_NOTES_TAG" >/dev/null 2>&1; then
    echo "エラー: リリース $UPDATE_NOTES_TAG が見つからない" >&2
    exit 1
  fi
  UPDATE_VERSION="${UPDATE_NOTES_TAG#v}"
  echo "==> $UPDATE_NOTES_TAG のノートを実アセットから再生成"
  UPLOADED_NAMES=()
  while IFS= read -r n; do
    [[ -n "$n" ]] && UPLOADED_NAMES+=("$n")
  done < <(gh release view "$UPDATE_NOTES_TAG" --json assets -q '.assets[].name')
  if [[ ${#UPLOADED_NAMES[@]} -eq 0 ]]; then
    echo "警告: $UPDATE_NOTES_TAG にアセットが 1 つも無い（表は空になる）" >&2
  else
    printf '    アセット: %s\n' "${UPLOADED_NAMES[@]}"
  fi
  NEW_NOTES=$(build_release_notes "$UPDATE_NOTES_TAG" "$UPDATE_VERSION" "${UPLOADED_NAMES[@]+"${UPLOADED_NAMES[@]}"}")
  gh release edit "$UPDATE_NOTES_TAG" --notes "$NEW_NOTES"
  echo "==> ノートを更新した: $UPDATE_NOTES_TAG"
  exit 0
fi

# --- ノート生成のドライラン（--notes-only）---
if [[ $NOTES_ONLY -eq 1 ]]; then
  DRY_NAMES=()
  while IFS= read -r n; do
    [[ -n "$n" ]] && DRY_NAMES+=("$n")
  done < <(collect_dist_asset_names "$TAG")
  # まだビルドしていなくても、これから作る macOS zip は必ず含まれる
  if ! printf '%s\n' "${DRY_NAMES[@]+"${DRY_NAMES[@]}"}" | grep -qx "$ZIP_NAME"; then
    DRY_NAMES+=("$ZIP_NAME")
  fi
  build_release_notes "$TAG" "$VERSION" "${DRY_NAMES[@]+"${DRY_NAMES[@]}"}"
  exit 0
fi

# --- 昇格（--promote）処理: テスト版と同一コミットに安定版リリースを作成 ---
if [[ -n "$PROMOTE_TAG" ]]; then
  if ! command -v gh >/dev/null; then
    echo "エラー: gh CLI が必要（brew install gh）" >&2
    exit 1
  fi
  echo "==> テスト版 $PROMOTE_TAG を安定版に昇格"

  # テスト版リリースの存在確認
  if ! gh release view "$PROMOTE_TAG" >/dev/null 2>&1; then
    echo "エラー: テスト版リリース $PROMOTE_TAG が見つからない" >&2
    exit 1
  fi

  # テスト版タグのコミットを取得
  PROMOTE_COMMIT=$(git rev-list -n1 "$PROMOTE_TAG" 2>/dev/null || true)
  if [[ -z "$PROMOTE_COMMIT" ]]; then
    echo "エラー: タグ $PROMOTE_TAG のコミットが見つからない（git fetch --tags してください）" >&2
    exit 1
  fi

  # 安定版タグを生成（v0.6.0-test.1 → v0.6.0）
  STABLE_TAG=$(echo "$PROMOTE_TAG" | sed 's/-test\.[0-9]*$//')
  STABLE_VERSION="${STABLE_TAG#v}"
  if [[ "$STABLE_TAG" == "$PROMOTE_TAG" ]]; then
    echo "エラー: $PROMOTE_TAG はテスト版タグ（-test.N サフィックス）ではない" >&2
    exit 1
  fi

  echo "  テスト版: $PROMOTE_TAG (commit: ${PROMOTE_COMMIT:0:7})"
  echo "  安定版:   $STABLE_TAG"

  # テスト版のアセットをダウンロードして安定版に添付
  PROMOTE_TMPDIR=$(mktemp -d)
  trap 'rm -rf "$PROMOTE_TMPDIR"' EXIT
  echo "  アセットをダウンロード..."
  gh release download "$PROMOTE_TAG" --dir "$PROMOTE_TMPDIR" 2>/dev/null || true

  # 安定版タグを同コミットに作成
  if git rev-parse "$STABLE_TAG" >/dev/null 2>&1; then
    echo "  安定版タグ $STABLE_TAG は既に存在。スキップ"
  else
    git tag -a "$STABLE_TAG" "$PROMOTE_COMMIT" -m "tako $STABLE_TAG — promoted from $PROMOTE_TAG"
    git push origin "$STABLE_TAG"
    echo "  安定版タグ $STABLE_TAG を作成・push"
  fi

  # アセットをリネーム（-test.N を除去）してリリース作成
  ASSETS=()
  ASSET_NAMES=()
  for f in "$PROMOTE_TMPDIR"/*; do
    [[ -f "$f" ]] || continue
    BASENAME=$(basename "$f")
    # tako-v0.6.0-test.1-macos-arm64.zip → tako-v0.6.0-macos-arm64.zip
    NEWNAME=$(echo "$BASENAME" | sed "s/${PROMOTE_TAG#v}/$STABLE_VERSION/g")
    if [[ "$NEWNAME" != "$BASENAME" ]]; then
      mv "$f" "$PROMOTE_TMPDIR/$NEWNAME"
    fi
    ASSETS+=("$PROMOTE_TMPDIR/$NEWNAME")
    ASSET_NAMES+=("$NEWNAME")
  done

  # 安定版のリリースノート（昇格の一文 + 通常と同じ構成 = ダウンロード表・OS 別手順）
  PROMOTE_NOTES="Promoted from test release $PROMOTE_TAG.
テスト版 $PROMOTE_TAG からの昇格リリース。

$(build_release_notes "$STABLE_TAG" "$STABLE_VERSION" "${ASSET_NAMES[@]+"${ASSET_NAMES[@]}"}")"

  if gh release view "$STABLE_TAG" >/dev/null 2>&1; then
    echo "  安定版 Release $STABLE_TAG は既に存在。アセットのみアップロード"
    for a in "${ASSETS[@]}"; do
      gh release upload "$STABLE_TAG" "$a" --clobber
    done
  else
    gh release create "$STABLE_TAG" \
      --title "tako $STABLE_TAG" \
      --notes "$PROMOTE_NOTES" \
      --generate-notes \
      "${ASSETS[@]}"
    echo "  安定版 Release $STABLE_TAG を作成"
  fi

  # テスト版リリースの prerelease フラグ維持（昇格しても消さない。履歴として残す）
  echo "==> 昇格完了: $PROMOTE_TAG → $STABLE_TAG"
  exit 0
fi

if [[ "$(uname)" != "Darwin" ]]; then
  echo "エラー: macOS 専用" >&2
  exit 1
fi

# --- ビルド ---
if [[ $SKIP_BUILD -eq 0 ]]; then
  echo "==> build-app.sh を実行"
  "$REPO_ROOT/scripts/build-app.sh"
else
  if [[ ! -d "$APP" ]]; then
    echo "エラー: $APP が見つからない（--skip-build には事前ビルドが必要）" >&2
    exit 1
  fi
  echo "==> ビルドをスキップ（既存の $APP を使用）"
fi

# --- PWA dist 鮮度検証（Issue #60 再発防止）---
# ビルド後の dist の JS にソース由来のマーカーが含まれることを確認する。
# stale な dist が同梱されるとリモート PWA の機能が欠落する。
echo "==> PWA dist 鮮度検証"
PWA_DIST="$REPO_ROOT/web/tako-remote/dist"
if [[ ! -d "$PWA_DIST/assets" ]]; then
  echo "エラー: PWA dist が存在しない（$PWA_DIST/assets）" >&2
  echo "  build-app.sh が npm build を実行したか確認してください" >&2
  exit 1
fi
PWA_MARKER_FOUND=0
for jsfile in "$PWA_DIST"/assets/*.js; do
  if grep -q "ペイン" "$jsfile" 2>/dev/null; then
    PWA_MARKER_FOUND=1
    break
  fi
done
if [[ $PWA_MARKER_FOUND -eq 0 ]]; then
  echo "エラー: PWA dist の JS に「ペイン」マーカーが見つからない" >&2
  echo "  dist が stale です。npm run build を実行してから再試行してください" >&2
  exit 1
fi
echo "    OK: dist の JS にソース由来マーカーを確認"

# --- zip 生成 ---
echo "==> zip 生成: $ZIP_NAME"
rm -f "$ZIP_PATH"
# ditto はリソースフォーク・拡張属性を保持する macOS 推奨のアーカイバ
ditto -c -k --keepParent "$APP" "$ZIP_PATH"
ZIP_SIZE=$(du -h "$ZIP_PATH" | cut -f1 | xargs)
echo "    生成完了: $ZIP_PATH ($ZIP_SIZE)"

# --- リリース作成 ---
if [[ $PUBLISH -eq 1 ]] || [[ $DRAFT -eq 1 ]]; then
  if ! command -v gh >/dev/null; then
    echo "エラー: gh CLI が必要（brew install gh）" >&2
    exit 1
  fi

  DRAFT_FLAG=""
  if [[ $DRAFT -eq 1 ]]; then
    DRAFT_FLAG="--draft"
  fi

  PRERELEASE_FLAG=""
  if [[ $TEST_RELEASE -eq 1 ]]; then
    PRERELEASE_FLAG="--prerelease"
    echo "  [テスト版] prerelease フラグ付きでリリース"
  fi

  # 添付するアセット: 生成した macOS zip + dist に置かれた他 OS の配布物（#594）
  UPLOAD_PATHS=("$ZIP_PATH")
  ASSET_NAMES=("$ZIP_NAME")
  while IFS= read -r n; do
    [[ -n "$n" && "$n" != "$ZIP_NAME" ]] || continue
    UPLOAD_PATHS+=("$DIST/$n")
    ASSET_NAMES+=("$n")
  done < <(collect_dist_asset_names "$TAG")
  if [[ ${#ASSET_NAMES[@]} -gt 1 ]]; then
    printf '  同梱アセット: %s\n' "${ASSET_NAMES[@]}"
  fi

  # CHANGELOG + 実アセットからリリースノートを組み立て（ダウンロード表・OS 別手順・
  # Windows 版があれば Known limitations も。組み立ては build_release_notes が正）
  RELEASE_NOTES=$(build_release_notes "$TAG" "$VERSION" "${ASSET_NAMES[@]}")

  echo "==> GitHub Release 作成: $TAG"

  # 冪等性: Release が既に存在する場合はアセット追加のみ（#256）
  if gh release view "$TAG" >/dev/null 2>&1; then
    echo "    Release $TAG は既に存在。アセットのアップロードのみ実行"
    gh release upload "$TAG" "${UPLOAD_PATHS[@]}" --clobber
  else
    # タグ push 直後は GitHub 側の伝播ラグで gh release create が失敗する
    # ことがあるため、指数バックオフ付きリトライで吸収する（#256）
    MAX_RETRIES=3
    RETRY_WAIT=${TAKO_RELEASE_RETRY_WAIT:-10}
    ATTEMPT=0
    RELEASE_CREATED=0

    while [[ $ATTEMPT -lt $MAX_RETRIES ]]; do
      ATTEMPT=$((ATTEMPT + 1))
      echo "    gh release create: 試行 ${ATTEMPT}/${MAX_RETRIES}"

      GH_STDERR_FILE=$(mktemp)
      GH_EXIT=0
      gh release create "$TAG" \
          --title "tako $TAG" \
          --notes "$RELEASE_NOTES" \
          --generate-notes \
          $DRAFT_FLAG \
          $PRERELEASE_FLAG \
          "${UPLOAD_PATHS[@]}" 2>"$GH_STDERR_FILE" || GH_EXIT=$?

      if [[ $GH_EXIT -eq 0 ]]; then
        rm -f "$GH_STDERR_FILE"
        RELEASE_CREATED=1
        break
      fi

      echo "    gh release create 失敗（exit ${GH_EXIT}）。gh stderr:" >&2
      cat "$GH_STDERR_FILE" >&2
      rm -f "$GH_STDERR_FILE"

      if [[ $ATTEMPT -lt $MAX_RETRIES ]]; then
        # 部分成功（Release は作られたがアセット添付で失敗等）への対処
        if gh release view "$TAG" >/dev/null 2>&1; then
          echo "    Release $TAG が前回の試行で作成された。アセットをアップロード"
          gh release upload "$TAG" "${UPLOAD_PATHS[@]}" --clobber
          RELEASE_CREATED=1
          break
        fi
        echo "    ${RETRY_WAIT} 秒後にリトライ..."
        sleep "$RETRY_WAIT"
        RETRY_WAIT=$((RETRY_WAIT * 2))
      fi
    done

    if [[ $RELEASE_CREATED -eq 0 ]]; then
      echo "" >&2
      echo "ERROR: GitHub Release の作成に ${MAX_RETRIES} 回失敗（tag $TAG は push 済み）" >&2
      echo "手動リカバリ: scripts/release.sh --skip-build --publish" >&2
      exit 1
    fi
  fi

  echo "==> リリース完了"

  # リリースが成立した時点で、展開済みの tako.app（zip の材料）は用済み。
  # 置いたままにすると LS が拾って Finder の候補に tako が 2 つ並ぶ（#837）。
  # 夜間リリース（#166）は「release.sh（build + zip）→ release.sh --skip-build（公開）」の
  # 2 段なので、片付けるのは公開まで済んだこの分岐だけにする（失敗時は残るので
  # --skip-build での再試行がそのまま効く）
  ls_drop_build_output "$APP" "$DIST"
else
  echo ""
  echo "================================================"
  echo "  zip 生成完了（リリースは未作成）"
  echo "================================================"
  echo "  バージョン : $VERSION"
  echo "  タグ       : $TAG"
  echo "  zip        : $ZIP_PATH"
  echo "  サイズ     : $ZIP_SIZE"
  echo "  アーキテクチャ: $ARCH"
  echo ""
  echo "  リリースを作成するには:"
  echo "    scripts/release.sh --publish     # 公開リリース"
  echo "    scripts/release.sh --draft       # ドラフト（非公開）"
  echo "================================================"
fi
