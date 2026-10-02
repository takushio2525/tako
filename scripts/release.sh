#!/usr/bin/env bash
# release.sh — tako.app の zip を生成し、GitHub Releases へアップロードする（macOS 専用）
#
# 使い方:
#   scripts/release.sh              # ビルド → zip 生成まで（リリースは作成しない）
#   scripts/release.sh --publish    # zip 生成 + GitHub Release 作成・アップロード
#   scripts/release.sh --draft      # zip 生成 + ドラフトリリース作成
#   scripts/release.sh --skip-build # ビルド済み dist/tako.app を使って zip のみ再生成
#   scripts/release.sh --test       # テスト版（prerelease）としてリリース（#403）
#   scripts/release.sh --promote <tag>       # テスト版（-test.N）/ 夜間の素のタグを安定版に昇格し、
#                                            # Homebrew cask と docs の「最新の安定版」も追従させる（#403 / #1853）
#   scripts/release.sh --notes-only # リリースノートを生成して表示するだけ（ビルド・公開しない）
#   scripts/release.sh --update-notes [tag]  # 公開済みリリースのノートを実アセットから作り直す
#   scripts/release.sh --check-assets [tag]  # 両 OS の配布物が揃っているかを検査（#965）
#   scripts/release.sh --publish --no-wait-windows  # Windows 版を待たずに macOS 版だけ出す
#
# 前提:
#   - macOS（build-app.sh と同じ）
#   - --publish / --draft には gh CLI（`brew install gh`）+ 認証済み
#   - リポジトリのリモートが origin に設定されていること
#
# バージョンは Cargo.toml [workspace.package] から自動読み取り。
# リリースノートは CHANGELOG.md から該当バージョンのセクションを自動抽出。
#
# --- プラットフォーム対応（Issue #594 / #965）---
#
# リリースノートは Mac / Windows で分けず、単一ノート + プラットフォーム明示で運用する。
# ノートには実アセットから組み立てた**ダウンロード表**と**動作要件**が入り、Windows 版の
# 配布物が含まれるときだけ Windows のインストール手順と Known limitations 節が付く
# （Known limitations は #515 のサポートマトリクスから `tako platform` 経由で自動生成）。
#
# **リリースは両 OS の配布物が揃って初めて成立する（#965）**。
# 段取りはこうなっている:
#
#   1. タグを push する → GitHub Actions（.github/workflows/release-windows.yml）が
#      windows ランナーで installer exe / ポータブル zip を作り、同じ Release へ添付する
#   2. このスクリプトが macOS の zip を作って Release を作成し、
#      **Windows 側の添付が終わるまで待って**からノートを実アセットで作り直す
#   3. 最後に両 OS が揃ったかを検査する。揃っていなければ exit 3（片肺リリースの検出）
#
# 待ちを省く（緊急の macOS 先行公開）: --no-wait-windows。あとから揃えるには
#   installer/windows/release-windows.ps1 -Tag <tag> -Upload   # 実機でビルドする場合
#   gh run list --workflow release-windows.yml                  # CI の状態を見る
#   scripts/release.sh --update-notes <tag>                     # ノートを作り直す
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

# 配布物の個人情報チェック（#1848）。build-app.sh の署名の後と同じ 1 実装を、zip の直前にも呼ぶ
# shellcheck source=lib/bundle-privacy.sh
source "$REPO_ROOT/scripts/lib/bundle-privacy.sh"

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
CHECK_ASSETS_TAG=""
# Windows 配布物を待つのが既定（#965。リリース = 両 OS が揃って出るのが正常）
WAIT_WINDOWS=1
while [[ $# -gt 0 ]]; do
  case "$1" in
    --publish)    PUBLISH=1; shift ;;
    --draft)      DRAFT=1; shift ;;
    --skip-build) SKIP_BUILD=1; shift ;;
    --test)       TEST_RELEASE=1; PUBLISH=1; shift ;;
    --notes-only) NOTES_ONLY=1; shift ;;
    --no-wait-windows) WAIT_WINDOWS=0; shift ;;
    --check-assets)
      shift
      # タグ省略時は Cargo.toml のバージョン
      if [[ $# -gt 0 && "$1" != --* ]]; then
        CHECK_ASSETS_TAG="$1"; shift
      else
        CHECK_ASSETS_TAG="$TAG"
      fi ;;
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
        echo "エラー: --promote には昇格するタグを指定してください（例: --promote v0.8.26）" >&2
        exit 2
      fi
      PROMOTE_TAG="$1"; shift ;;
    *) echo "不明な引数: $1（--publish / --draft / --skip-build / --test / --notes-only / --update-notes [tag] / --check-assets [tag] / --no-wait-windows / --promote <tag>）" >&2; exit 2 ;;
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
  for c in ${candidates[@]+"${candidates[@]}"}; do
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

# build_requirements_section <name...> — 動作要件（アセットがある OS の行だけ。#965）
# 文言の正は crates/tako-core/src/platform/release_assets.rs の os_requirement()
build_requirements_section() {
  local platform rows=""
  for platform in $TAKO_ASSET_PLATFORMS; do
    [[ -n "$(assets_for_platform "$platform" "$@")" ]] || continue
    rows+="- **$(tako_asset_label "$platform")**: $(tako_asset_requirement "$platform" ja)
  $(tako_asset_requirement "$platform" en)
"
  done
  [[ -n "$rows" ]] || return 0
  printf '### 動作要件 / Requirements\n\n%s' "$rows"
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

  # 動作要件（#965。配布物を落とす前に自分の環境で動くか判るように）
  local requirements
  requirements=$(build_requirements_section "${names[@]+"${names[@]}"}")
  if [[ -n "$requirements" ]]; then
    notes+="
${requirements}
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

# --- リリースの完全性と Windows 配布物の待ち合わせ（#965）-------------------
#
# リリースは macOS / Windows の配布物が**揃って初めて成立する**。macOS 側は
# このスクリプトが作り、Windows 側は tag push で起動する GitHub Actions
# （.github/workflows/release-windows.yml）が同じ Release へ添付する。
# ここはその待ち合わせと、揃ったかどうかの機械検査を担う。

# Windows 配布物を作るワークフローのファイル名（Actions 側の識別子）
WINDOWS_WORKFLOW="release-windows.yml"
# 待ち時間の上限と間隔（テストから短縮できるように env で上書き可）
WINDOWS_WAIT_MINUTES=${TAKO_WINDOWS_WAIT_MINUTES:-75}
WINDOWS_POLL_SECONDS=${TAKO_WINDOWS_POLL_SECONDS:-30}

require_gh() {
  if ! command -v gh >/dev/null; then
    echo "エラー: gh CLI が必要（brew install gh）" >&2
    exit 1
  fi
}

# gh_release_asset_names <tag> — 公開済みリリースのアセット名を 1 行ずつ
gh_release_asset_names() {
  gh release view "$1" --json assets -q '.assets[].name' 2>/dev/null || true
}

# report_release_completeness <tag> — 両 OS のアセットが揃っているかを表示。
# 揃っていなければ非 0（= 片肺リリースの検出。#965 受け入れ条件 3）
report_release_completeness() {
  local tag="$1" names=() platform missing found label n f
  while IFS= read -r n; do
    [[ -n "$n" ]] && names+=("$n")
  done < <(gh_release_asset_names "$tag")

  echo "==> リリース完全性の検査: $tag"
  for platform in $TAKO_ASSET_PLATFORMS; do
    label=$(tako_asset_label "$platform")
    found=$(assets_for_platform "$platform" "${names[@]+"${names[@]}"}")
    if [[ -n "$found" ]]; then
      while IFS= read -r f; do
        [[ -n "$f" ]] && echo "    [OK] ${label}: $f"
      done <<< "$found"
    else
      echo "    [NG] ${label}: 配布物が無い"
    fi
  done

  missing=$(tako_asset_missing_platforms "${names[@]+"${names[@]}"}")
  if [[ -z "$missing" ]]; then
    echo "    両 OS の配布物が揃っている"
    return 0
  fi
  echo "" >&2
  echo "警告: 片肺リリース（${tag}）— 配布物が無い OS: $(echo "$missing" | tr '\n' ' ')" >&2
  echo "  欠けた OS の利用者には更新が見えないままバージョンだけが進む（#595 / #965）" >&2
  local platform_missing
  for platform_missing in $missing; do
    case "$platform_missing" in
      windows)
        echo "  Windows: gh run list --workflow $WINDOWS_WORKFLOW でビルドの状態を確認し、" >&2
        echo "           成功していれば scripts/release.sh --check-assets $tag で再確認する。" >&2
        echo "           CI が使えないときは実機で installer/windows/release-windows.ps1 -Tag $tag -Upload" >&2
        ;;
      macos)
        echo "  macOS: scripts/release.sh --skip-build --publish（または --update-notes ${tag}）" >&2
        ;;
    esac
  done
  return 1
}

# refresh_release_notes <tag> — 実アセットを読み直してノートを作り直す
refresh_release_notes() {
  local tag="$1" version="${1#v}" names=() notes n
  while IFS= read -r n; do
    [[ -n "$n" ]] && names+=("$n")
  done < <(gh_release_asset_names "$tag")
  notes=$(build_release_notes "$tag" "$version" "${names[@]+"${names[@]}"}")
  gh release edit "$tag" --notes "$notes"
}

# windows_run_id <tag> — そのタグ向け Windows 配布物ワークフローの実行 ID（無ければ空）
# tag push 起動でも workflow_dispatch（--ref <tag>）起動でも headBranch はタグ名になる
windows_run_id() {
  local tag="$1"
  gh run list --workflow "$WINDOWS_WORKFLOW" --limit 50 \
    --json databaseId,headBranch \
    -q "[.[] | select(.headBranch == \"$tag\")] | first | .databaseId" 2>/dev/null || true
}

# wait_for_windows_assets <tag> — Windows 配布物が Release へ載るのを待つ。
# 既にあれば即成功。ワークフローが走っていなければ workflow_dispatch で起こす
wait_for_windows_assets() {
  local tag="$1" run_id="" status conclusion deadline now names=() n

  if tako_asset_is_complete $(gh_release_asset_names "$tag"); then
    echo "==> Windows 配布物は既に添付済み"
    return 0
  fi

  echo "==> Windows 配布物を待つ（$WINDOWS_WORKFLOW / 上限 ${WINDOWS_WAIT_MINUTES} 分）"
  run_id=$(windows_run_id "$tag")
  if [[ -z "$run_id" || "$run_id" == "null" ]]; then
    echo "    タグ $tag のワークフロー実行が見つからない → workflow_dispatch で起動"
    if ! gh workflow run "$WINDOWS_WORKFLOW" --ref "$tag" >/dev/null 2>&1; then
      echo "警告: $WINDOWS_WORKFLOW の起動に失敗（タグ $tag はリモートにある？）" >&2
      return 1
    fi
    # 起動直後は run 一覧に出るまで数秒かかる
    local tries=0
    while [[ -z "$run_id" || "$run_id" == "null" ]] && [[ $tries -lt 10 ]]; do
      sleep 5
      run_id=$(windows_run_id "$tag")
      tries=$((tries + 1))
    done
  fi
  if [[ -z "$run_id" || "$run_id" == "null" ]]; then
    echo "警告: $WINDOWS_WORKFLOW の実行を特定できなかった" >&2
    return 1
  fi
  echo "    実行 ID: ${run_id}（https://github.com/takushio2525/tako/actions/runs/${run_id}）"

  deadline=$(( $(date +%s) + WINDOWS_WAIT_MINUTES * 60 ))
  while :; do
    status=$(gh run view "$run_id" --json status -q '.status' 2>/dev/null || echo "")
    conclusion=$(gh run view "$run_id" --json conclusion -q '.conclusion' 2>/dev/null || echo "")
    if [[ "$status" == "completed" ]]; then
      echo "    ワークフロー完了: ${conclusion:-unknown}"
      break
    fi
    now=$(date +%s)
    if [[ $now -ge $deadline ]]; then
      echo "警告: ${WINDOWS_WAIT_MINUTES} 分待っても完了しなかった（status=${status:-unknown}）" >&2
      return 1
    fi
    echo "    ${status:-unknown}... （$(( (deadline - now) / 60 )) 分まで待つ）"
    sleep "$WINDOWS_POLL_SECONDS"
  done

  if [[ "$conclusion" != "success" ]]; then
    echo "警告: Windows 配布物のビルドが success ではない（${conclusion}）" >&2
    echo "  ログ: gh run view $run_id --log-failed" >&2
    return 1
  fi

  # 成功していればワークフローが添付済み。念のため実アセットで確認する
  while IFS= read -r n; do
    [[ -n "$n" ]] && names+=("$n")
  done < <(gh_release_asset_names "$tag")
  if [[ -z "$(assets_for_platform windows "${names[@]+"${names[@]}"}")" ]]; then
    echo "警告: ワークフローは成功したが Windows アセットが Release に無い" >&2
    echo "  gh run download $run_id で成果物を取り、gh release upload $tag <file> --clobber で添付する" >&2
    return 1
  fi
  return 0
}

# --- 公開済みリリースのアセットが両 OS 揃っているかの検査（--check-assets）---
if [[ -n "$CHECK_ASSETS_TAG" ]]; then
  require_gh
  if ! gh release view "$CHECK_ASSETS_TAG" >/dev/null 2>&1; then
    echo "エラー: リリース $CHECK_ASSETS_TAG が見つからない" >&2
    exit 1
  fi
  report_release_completeness "$CHECK_ASSETS_TAG"
  exit $?
fi

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
  echo "==> $UPDATE_NOTES_TAG のノートを実アセットから再生成"
  UPLOADED_NAMES=()
  while IFS= read -r n; do
    [[ -n "$n" ]] && UPLOADED_NAMES+=("$n")
  done < <(gh release view "$UPDATE_NOTES_TAG" --json assets -q '.assets[].name')
  if [[ ${#UPLOADED_NAMES[@]} -eq 0 ]]; then
    echo "警告: $UPDATE_NOTES_TAG にアセットが 1 つも無い（表は空になる）" >&2
  else
    printf '    アセット: %s\n' ${UPLOADED_NAMES[@]+"${UPLOADED_NAMES[@]}"}
  fi
  refresh_release_notes "$UPDATE_NOTES_TAG"
  echo "==> ノートを更新した: $UPDATE_NOTES_TAG"
  # 片肺のまま気付かず終わらないよう、更新のついでに完全性も報告する（#965。exit は変えない）
  report_release_completeness "$UPDATE_NOTES_TAG" || true
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

# --- 昇格（--promote）: テスト版 / 夜間の素のタグを安定版にする（#403 / #1853）----------
#
# 受け付けるタグは 2 つの形:
#   - テスト版タグ（v0.6.0-test.1）… 同じコミットに安定版タグ（v0.6.0）を打ち、アセットの
#     名前を付け替えて添付した安定版 Release を作る（#403 以来の経路）
#   - 夜間の素のタグ（v0.8.26）… 夜間リリースは常に prerelease で出る（#403）ので、
#     その Release の prerelease を外して Latest にする（タグ・アセットはそのまま）
#
# どちらも最後に同じ後続を走らせる。Release が Latest になっても、Homebrew の cask と
# docs の「最新の安定版」が古いままだと、そこから入れる・読む利用者には昇格していないのと
# 同じ（v0.8.0 の昇格後も cask が 1 か月半 v0.7.0 のままだった = #1853 / #1592）:
#   1. tap（HOMEBREW_TAP_REPO）の cask の version / sha256 を、公開アセットを実際に落として
#      算出した値へ書き換え、PR → squash merge（tap に CI は無い）。成否は tap の main を
#      読み直して決める
#   2. docs/src/content/docs/releases.md の「最新の安定版」を書き換えて PR を出し、
#      scripts/merge-pr.sh で CI が緑で揃ってから merge する（main へ直接は push しない）
# 後続のどれかが終わらなければ exit 4（= Release は昇格済み）。同じ昇格を打ち直せば、
# 済んだ段は飛ばして残りだけをやる（冪等）

# 昇格できるタグの形
PROMOTE_PLAIN_TAG_RE='^v[0-9]+\.[0-9]+\.[0-9]+$'
PROMOTE_TEST_TAG_RE='^v[0-9]+\.[0-9]+\.[0-9]+-test\.[0-9]+$'
SHA256_RE='^[0-9a-f]{64}$'
# 後続の行き先。tap の clone 元だけはテストから一時ディレクトリの bare リポへ差し替える
TAKO_RELEASE_REPO="takushio2525/tako"
HOMEBREW_TAP_REPO="takushio2525/homebrew-tako"
HOMEBREW_TAP_REMOTE=${TAKO_HOMEBREW_TAP_REMOTE:-"https://github.com/${HOMEBREW_TAP_REPO}.git"}
HOMEBREW_CASK_PATH="Casks/tako.rb"
# cask の url が指す配布物の arch（url の雛形は arm64 の zip）
HOMEBREW_CASK_ARCH="arm64"
RELEASES_PAGE="docs/src/content/docs/releases.md"
# 「Release は安定版（Latest）になったが、cask / docs の追従が終わっていない」。
# 1（昇格そのものの失敗）・3（片肺）と区別できるようにする
PROMOTE_FOLLOWUP_EXIT=4

# version_lt <a> <b> — x.y.z 同士で a < b なら 0
version_lt() {
  local a1 a2 a3 b1 b2 b3
  IFS=. read -r a1 a2 a3 <<< "$1"
  IFS=. read -r b1 b2 b3 <<< "$2"
  if (( a1 != b1 )); then (( a1 < b1 )); return; fi
  if (( a2 != b2 )); then (( a2 < b2 )); return; fi
  (( a3 < b3 ))
}

# release_field <tag> <field> — Release の 1 項目（isPrerelease 等）を読む。読めなければ空
release_field() {
  gh release view "$1" --json "$2" -q ".$2" 2>/dev/null || true
}

# mark_release_latest <tag> — prerelease を外して Latest にし、外れたことを読み直して確かめる
mark_release_latest() {
  local tag="$1"
  if ! gh release edit "$tag" --prerelease=false --latest >/dev/null; then
    echo "エラー: gh release edit $tag --prerelease=false --latest が失敗した" >&2
    return 1
  fi
  if [[ "$(release_field "$tag" isPrerelease)" != "false" ]]; then
    echo "エラー: $tag の prerelease が外れていない（gh release view $tag で確かめる）" >&2
    return 1
  fi
}

# print_indented <file> <prefix> — 失敗したコマンドの stderr を字下げして出す
print_indented() {
  [[ -s "$1" ]] || return 0
  sed "s/^/    $2/" "$1" >&2
}

# copy_commit_identity <dir> — 一時 clone でも、このリポと同じ作者名・メールで commit させる
# （一時 clone はリポジトリのローカル設定を持たないので、素のままだと作者がグローバル設定になる）
copy_commit_identity() {
  local key val
  for key in user.name user.email; do
    val=$(git -C "$REPO_ROOT" config --get "$key" 2>/dev/null || true)
    if [[ -n "$val" ]]; then
      git -C "$1" config "$key" "$val"
    fi
  done
}

# cask_field <file> <version|sha256|url> — cask の `version "…"` 等の値（無ければ空）
cask_field() {
  sed -n 's/^[[:space:]]*'"$2"' "\(.*\)"[[:space:]]*$/\1/p' "$1" 2>/dev/null | head -1
}

# cask_rewrite <file> <version> <sha256> — version / sha256 の行だけを書き換える（字下げは保つ）
cask_rewrite() {
  sed -e 's/^\([[:space:]]*version \)".*"/\1"'"$2"'"/' \
      -e 's/^\([[:space:]]*sha256 \)".*"/\1"'"$3"'"/' "$1" > "$1.new" && mv -f "$1.new" "$1"
}

# update_homebrew_cask <stable-tag> — 後続 1: tap の cask を公開アセットに合わせる
update_homebrew_cask() {
  local tag="$1" version="${1#v}" work asset file sha digest clone cask url expected_url
  local cur_ver cur_sha branch pr_url merge_rc main_cask
  work="$PROMOTE_TMPDIR/cask"
  asset=$(tako_asset_name "$tag" macos "$HOMEBREW_CASK_ARCH")
  echo "==> Homebrew cask を $tag へ（$HOMEBREW_TAP_REPO の ${HOMEBREW_CASK_PATH}）"
  mkdir -p "$work/asset"

  # 公開アセットを実際に落として sha256 を出す（brew が落とすのと同じファイル）
  if ! gh release download "$tag" --pattern "$asset" --dir "$work/asset" --clobber \
      >/dev/null 2>"$work/download.err"; then
    echo "エラー: $tag の公開アセット $asset を落とせない（Release に無い / 通信の失敗）" >&2
    print_indented "$work/download.err" "gh: "
    return 1
  fi
  file="$work/asset/$asset"
  if [[ ! -s "$file" ]]; then
    echo "エラー: $tag から落とした $asset が無いか空（gh release download $tag --pattern ${asset}）" >&2
    return 1
  fi
  sha=$(shasum -a 256 "$file" 2>/dev/null | awk '{print $1}') || sha=""
  if [[ ! "$sha" =~ $SHA256_RE ]]; then
    echo "エラー: $asset の sha256 を算出できない（shasum -a 256 の結果: ${sha:-空}）" >&2
    return 1
  fi
  # GitHub がアセットごとに持つ digest と食い違えば、落とす途中で壊れている
  digest=$(gh release view "$tag" --json assets \
    -q ".assets[] | select(.name == \"$asset\") | .digest" 2>/dev/null || true)
  digest=${digest#sha256:}
  if [[ -n "$digest" && "$digest" != "null" && "$digest" != "$sha" ]]; then
    echo "エラー: 落とした $asset の sha256 が GitHub の digest と一致しない" >&2
    echo "    算出:   $sha" >&2
    echo "    GitHub: $digest" >&2
    return 1
  fi
  echo "    sha256: ${sha}（${asset}）"

  clone="$work/tap"
  if ! git clone --quiet "$HOMEBREW_TAP_REMOTE" "$clone" 2>"$work/clone.err"; then
    echo "エラー: tap を clone できない（${HOMEBREW_TAP_REMOTE}）" >&2
    print_indented "$work/clone.err" "git: "
    return 1
  fi
  cask="$clone/$HOMEBREW_CASK_PATH"
  cur_ver=$(cask_field "$cask" version)
  cur_sha=$(cask_field "$cask" sha256)
  url=$(cask_field "$cask" url)
  if [[ -z "$cur_ver" || -z "$cur_sha" || -z "$url" ]]; then
    echo "エラー: tap の $HOMEBREW_CASK_PATH に version / sha256 / url の行が見つからない（形が変わった？）" >&2
    return 1
  fi
  # url の雛形が、sha256 を出したのと同じアセットを指していること
  # （別の名前を指していれば、brew が落とす物と sha256 が合わず入れられなくなる）
  expected_url="https://github.com/${TAKO_RELEASE_REPO}/releases/download/${tag}/${asset}"
  if [[ "${url//'#{version}'/$version}" != "$expected_url" ]]; then
    echo "エラー: cask の url が $asset を指していない" >&2
    echo "    cask: $url" >&2
    echo "    期待: $expected_url" >&2
    return 1
  fi
  if [[ "$cur_ver" == "$version" && "$cur_sha" == "$sha" ]]; then
    echo "    tap の cask は既に ${version}（sha256 も一致）。何もしない"
    return 0
  fi
  echo "    cask: $cur_ver → $version"

  cask_rewrite "$cask" "$version" "$sha"
  if [[ "$(cask_field "$cask" version)" != "$version" || "$(cask_field "$cask" sha256)" != "$sha" ]]; then
    echo "エラー: $HOMEBREW_CASK_PATH の version / sha256 を書き換えられなかった" >&2
    return 1
  fi
  branch="update-$version"
  copy_commit_identity "$clone"
  if ! git -C "$clone" checkout --quiet -b "$branch" ||
     ! git -C "$clone" commit --quiet -a -m "[改善] cask を tako $tag に更新" \
         -m "sha256 は公開アセット $asset を実際にダウンロードして算出した（scripts/release.sh --promote）。"; then
    echo "エラー: tap の clone で commit できなかった" >&2
    return 1
  fi
  # 前回の打ち直しで同名のブランチが残っていても、中身は今回の値で上書きしてよい
  if ! git -C "$clone" push --quiet --force origin "$branch" 2>"$work/push.err"; then
    echo "エラー: tap へ push できない（$HOMEBREW_TAP_REMOTE の ${branch}）" >&2
    print_indented "$work/push.err" "git: "
    return 1
  fi
  pr_url=$(cd "$clone" && gh pr list --repo "$HOMEBREW_TAP_REPO" --head "$branch" --state open \
    --json url -q '.[0].url' 2>/dev/null || true)
  if [[ -z "$pr_url" || "$pr_url" == "null" ]]; then
    if ! pr_url=$(cd "$clone" && gh pr create --repo "$HOMEBREW_TAP_REPO" --base main --head "$branch" \
        --title "[改善] cask を tako $tag に更新" \
        --body "$tag を安定版（Latest）へ昇格したのに合わせて cask を更新する。sha256 は公開アセット \`$asset\` を実際にダウンロードして算出した（\`scripts/release.sh --promote\` が作成）。" \
        2>"$work/pr.err"); then
      echo "エラー: tap の PR を作れない（ブランチ $branch は push 済み）" >&2
      print_indented "$work/pr.err" "gh: "
      return 1
    fi
  fi
  echo "    PR: $pr_url"
  merge_rc=0
  (cd "$clone" && gh pr merge "$pr_url" --repo "$HOMEBREW_TAP_REPO" --squash --delete-branch) \
    >/dev/null 2>"$work/merge.err" || merge_rc=$?

  # 成否は gh の終了コードではなく tap の main の実物で決める（merge 後の後始末だけ
  # 失敗しても gh は非 0 で終わる = #1430 と同じ）
  main_cask="$work/main-cask.rb"
  if ! git -C "$clone" fetch --quiet origin main 2>"$work/fetch.err"; then
    echo "エラー: merge 後の tap の main を読み直せない" >&2
    print_indented "$work/fetch.err" "git: "
    return 1
  fi
  git -C "$clone" show "origin/main:$HOMEBREW_CASK_PATH" > "$main_cask" 2>/dev/null || : > "$main_cask"
  if [[ "$(cask_field "$main_cask" version)" != "$version" || "$(cask_field "$main_cask" sha256)" != "$sha" ]]; then
    echo "エラー: tap の main の cask が $version になっていない（merge されていない。gh pr merge の終了コード ${merge_rc}）" >&2
    print_indented "$work/merge.err" "gh: "
    echo "    PR は残してある: $pr_url" >&2
    return 1
  fi
  echo "    tap の main を読み直して確認した: version $version / sha256 一致"
}

# update_docs_stable_label <stable-tag> — 後続 2: docs の「最新の安定版」を書き換えて PR → merge
update_docs_stable_label() {
  local tag="$1" version="${1#v}" wt date branch pr_url pr
  wt="$PROMOTE_TMPDIR/docs"
  branch="docs/promote-$tag"
  echo "==> docs の「最新の安定版」を $tag へ（${RELEASES_PAGE}）"
  if ! command -v node >/dev/null; then
    echo "エラー: node が無い（$RELEASES_PAGE の書き換えに使う）" >&2
    return 1
  fi
  # 呼び出し元のツリー（共有ツリーのこともある）は動かさない（#1136）。最新の origin/main を
  # 使い捨ての worktree に出して、そこで書き換える
  if ! git -C "$REPO_ROOT" fetch --quiet origin main 2>"$PROMOTE_TMPDIR/docs-fetch.err" ||
     ! git -C "$REPO_ROOT" worktree add --quiet --detach "$wt" origin/main 2>>"$PROMOTE_TMPDIR/docs-fetch.err"; then
    echo "エラー: origin/main を作業用の worktree に出せない" >&2
    print_indented "$PROMOTE_TMPDIR/docs-fetch.err" "git: "
    return 1
  fi
  # 日付はその版の CHANGELOG の節の日付（無ければ今日）
  date=$(sed -n "s/^## \[${version//./\\.}\] - \([0-9]\{4\}-[0-9]\{2\}-[0-9]\{2\}\).*/\1/p" \
    "$wt/CHANGELOG.md" 2>/dev/null | head -1)
  [[ -n "$date" ]] || date=$(date +%F)
  # 書き換えと検査は番犬（docs の CI と同じスクリプト）の 1 実装に任せる
  if ! node "$wt/docs/scripts/check-releases-page.mjs" --set-stable="$version" --date="$date"; then
    echo "エラー: $RELEASES_PAGE を $tag へ書き換えられない（上の理由を直してから scripts/release.sh --promote ${tag}）" >&2
    return 1
  fi
  if git -C "$wt" diff --quiet -- "$RELEASES_PAGE"; then
    echo "    origin/main の $RELEASES_PAGE は既に ${tag}。何もしない"
    return 0
  fi
  if ! git -C "$wt" commit --quiet -m "[ドキュメント] リリースノートのページの「最新の安定版」を $tag にした" \
      -m "scripts/release.sh --promote $tag が、安定版（Latest）への昇格に合わせて書き換えた。" \
      -- "$RELEASES_PAGE"; then
    echo "エラー: $RELEASES_PAGE の書き換えを commit できなかった" >&2
    return 1
  fi
  if ! git -C "$wt" push --quiet --force origin "HEAD:refs/heads/$branch" 2>"$PROMOTE_TMPDIR/docs-push.err"; then
    echo "エラー: docs のブランチ $branch を push できない" >&2
    print_indented "$PROMOTE_TMPDIR/docs-push.err" "git: "
    return 1
  fi
  pr_url=$(gh pr list --repo "$TAKO_RELEASE_REPO" --head "$branch" --state open \
    --json url -q '.[0].url' 2>/dev/null || true)
  if [[ -z "$pr_url" || "$pr_url" == "null" ]]; then
    if ! pr_url=$(gh pr create --repo "$TAKO_RELEASE_REPO" --base main --head "$branch" \
        --title "[ドキュメント] リリースノートのページの「最新の安定版」を $tag にした" \
        --body "\`scripts/release.sh --promote $tag\` が自動で作った PR。$tag を安定版（Latest）へ昇格したのに合わせて \`$RELEASES_PAGE\` の「最新の安定版」を書き換えた（#1853 / #1592）。CI が緑で揃ったら同じスクリプトが \`scripts/merge-pr.sh\` で merge する。" \
        2>"$PROMOTE_TMPDIR/docs-pr.err"); then
      echo "エラー: docs の PR を作れない（ブランチ $branch は push 済み）" >&2
      print_indented "$PROMOTE_TMPDIR/docs-pr.err" "gh: "
      return 1
    fi
  fi
  pr=${pr_url##*/}
  echo "    PR: ${pr_url}（CI が緑で揃ってから merge する）"
  if ! (cd "$REPO_ROOT" && "$REPO_ROOT/scripts/merge-pr.sh" "$pr"); then
    echo "エラー: docs の PR #$pr を merge できなかった（PR は残してある）。CI が緑になってから scripts/merge-pr.sh $pr" >&2
    return 1
  fi
}

# 後始末: 一時ファイルと docs の worktree（成功・失敗のどちらでも残さない）。
# EXIT の trap なので、最後に元の終了コードで抜け直す。ただし bash 3.2 は set -u
# （unbound variable）で死んだときだけ trap の中の $? が 0 になる（trap が無ければ 1。
# bash 5 は trap の中でも 1）。そのままだと途中で死んだ昇格が exit 0 に見えるので、
# 成功の exit の直前に立てる印（PROMOTE_SUCCEEDED）が無い 0 は失敗として 1 にする
promote_cleanup() {
  local rc=$?
  if [[ $rc -eq 0 && "${PROMOTE_SUCCEEDED:-0}" != 1 ]]; then
    echo "エラー: 昇格が途中で止まった（終了コード 0 のまま抜けかけた。上の出力の最後の行を見る）" >&2
    rc=1
  fi
  if [[ -n "${PROMOTE_TMPDIR:-}" && -d "$PROMOTE_TMPDIR/docs" ]]; then
    git -C "$REPO_ROOT" worktree remove --force "$PROMOTE_TMPDIR/docs" >/dev/null 2>&1 || true
  fi
  if [[ -n "${PROMOTE_TMPDIR:-}" ]]; then
    rm -rf "$PROMOTE_TMPDIR"
  fi
  git -C "$REPO_ROOT" worktree prune >/dev/null 2>&1 || true
  exit "$rc"
}

if [[ -n "$PROMOTE_TAG" ]]; then
  require_gh
  if [[ "$PROMOTE_TAG" =~ $PROMOTE_PLAIN_TAG_RE ]]; then
    PROMOTE_KIND="nightly"
    STABLE_TAG="$PROMOTE_TAG"
  elif [[ "$PROMOTE_TAG" =~ $PROMOTE_TEST_TAG_RE ]]; then
    PROMOTE_KIND="test"
    # v0.6.0-test.1 → v0.6.0
    STABLE_TAG="${PROMOTE_TAG%-test.*}"
  else
    echo "エラー: $PROMOTE_TAG は昇格できるタグの形ではない（テスト版 v0.6.0-test.1 / 夜間の素のタグ v0.8.26）" >&2
    exit 1
  fi
  STABLE_VERSION="${STABLE_TAG#v}"

  if ! gh release view "$PROMOTE_TAG" >/dev/null 2>&1; then
    echo "エラー: リリース $PROMOTE_TAG が見つからない" >&2
    exit 1
  fi
  if [[ "$(release_field "$PROMOTE_TAG" isDraft)" == "true" ]]; then
    echo "エラー: $PROMOTE_TAG はドラフト。公開してから昇格させる" >&2
    exit 1
  fi
  # 今の安定版より古い版は昇格させない。打ち間違えると Homebrew の cask と docs が
  # 古い版へ戻る（同じ版の打ち直しは後続のやり直しとして通す）
  CURRENT_LATEST=$(gh release view --json tagName -q .tagName 2>/dev/null || true)
  if [[ "$CURRENT_LATEST" =~ $PROMOTE_PLAIN_TAG_RE ]] && version_lt "$STABLE_VERSION" "${CURRENT_LATEST#v}"; then
    echo "エラー: $STABLE_TAG は今の安定版 $CURRENT_LATEST より古い（昇格すると Homebrew の cask と docs が古い版へ戻る）" >&2
    exit 1
  fi

  PROMOTE_TMPDIR=$(mktemp -d)
  trap promote_cleanup EXIT

  if [[ "$PROMOTE_KIND" == "test" ]]; then
    echo "==> テスト版 $PROMOTE_TAG を安定版に昇格"

    # テスト版タグのコミットを取得
    PROMOTE_COMMIT=$(git rev-list -n1 "$PROMOTE_TAG" 2>/dev/null || true)
    if [[ -z "$PROMOTE_COMMIT" ]]; then
      echo "エラー: タグ $PROMOTE_TAG のコミットが見つからない（git fetch --tags してください）" >&2
      exit 1
    fi

    echo "  テスト版: $PROMOTE_TAG (commit: ${PROMOTE_COMMIT:0:7})"
    echo "  安定版:   $STABLE_TAG"

    # テスト版のアセットをダウンロードして安定版に添付
    mkdir -p "$PROMOTE_TMPDIR/assets"
    echo "  アセットをダウンロード..."
    gh release download "$PROMOTE_TAG" --dir "$PROMOTE_TMPDIR/assets" 2>/dev/null || true

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
    for f in "$PROMOTE_TMPDIR/assets"/*; do
      [[ -f "$f" ]] || continue
      BASENAME=$(basename "$f")
      # tako-v0.6.0-test.1-macos-arm64.zip → tako-v0.6.0-macos-arm64.zip
      NEWNAME=$(echo "$BASENAME" | sed "s/${PROMOTE_TAG#v}/$STABLE_VERSION/g")
      if [[ "$NEWNAME" != "$BASENAME" ]]; then
        mv "$f" "$PROMOTE_TMPDIR/assets/$NEWNAME"
      fi
      ASSETS+=("$PROMOTE_TMPDIR/assets/$NEWNAME")
      ASSET_NAMES+=("$NEWNAME")
    done

    # 安定版のリリースノート（昇格の一文 + 通常と同じ構成 = ダウンロード表・OS 別手順）
    PROMOTE_NOTES="Promoted from test release $PROMOTE_TAG.
テスト版 $PROMOTE_TAG からの昇格リリース。

$(build_release_notes "$STABLE_TAG" "$STABLE_VERSION" "${ASSET_NAMES[@]+"${ASSET_NAMES[@]}"}")"

    if gh release view "$STABLE_TAG" >/dev/null 2>&1; then
      echo "  安定版 Release $STABLE_TAG は既に存在。アセットのみアップロード"
      for a in ${ASSETS[@]+"${ASSETS[@]}"}; do
        gh release upload "$STABLE_TAG" "$a" --clobber
      done
    else
      gh release create "$STABLE_TAG" \
        --title "tako $STABLE_TAG" \
        --notes "$PROMOTE_NOTES" \
        --generate-notes \
        ${ASSETS[@]+"${ASSETS[@]}"}
      echo "  安定版 Release $STABLE_TAG を作成"
    fi
    # テスト版リリースの prerelease フラグ維持（昇格しても消さない。履歴として残す）
  else
    echo "==> 夜間リリース $PROMOTE_TAG を安定版に昇格（prerelease を外して Latest にする）"
    if [[ "$(release_field "$PROMOTE_TAG" isPrerelease)" == "false" ]]; then
      echo "  $PROMOTE_TAG は既に安定版。Latest の印と後続（cask / docs）だけをやり直す"
    fi
  fi

  # 既にあった安定版 Release（夜間の素のタグと同じ版）が prerelease のまま残らないよう、
  # どちらの経路もここで Latest にする
  mark_release_latest "$STABLE_TAG" || exit 1
  echo "==> 昇格完了: $PROMOTE_TAG → ${STABLE_TAG}（Latest）"
  # 昇格元が片肺なら昇格先も片肺になる。気付けるように報告する（#965。exit は変えない）
  report_release_completeness "$STABLE_TAG" || true

  # 終わらなかった段の名前（" / " 区切り）
  PROMOTE_FOLLOWUP_FAILED=""
  update_homebrew_cask "$STABLE_TAG" ||
    PROMOTE_FOLLOWUP_FAILED="${PROMOTE_FOLLOWUP_FAILED:+$PROMOTE_FOLLOWUP_FAILED / }Homebrew cask"
  update_docs_stable_label "$STABLE_TAG" ||
    PROMOTE_FOLLOWUP_FAILED="${PROMOTE_FOLLOWUP_FAILED:+$PROMOTE_FOLLOWUP_FAILED / }docs の「最新の安定版」"
  if [[ -n "$PROMOTE_FOLLOWUP_FAILED" ]]; then
    echo "" >&2
    echo "ERROR: $STABLE_TAG は安定版（Latest）になったが、後続が終わっていない: $PROMOTE_FOLLOWUP_FAILED" >&2
    echo "  上の理由を直してから打ち直す（済んだ段は飛ばす）: scripts/release.sh --promote $STABLE_TAG" >&2
    exit "$PROMOTE_FOLLOWUP_EXIT"
  fi
  echo "==> Homebrew cask と docs の「最新の安定版」も $STABLE_TAG に揃った"
  PROMOTE_SUCCEEDED=1
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
  echo "  dist が stale です。scripts/build-pwa.sh を実行してから再試行してください" >&2
  exit 1
fi
echo "    OK: dist の JS にソース由来マーカーを確認"

# --- 配布物の個人情報チェック（Issue #1848）---
# build-app.sh も署名の後に同じ検査をするが、--skip-build は既存の dist/tako.app をそのまま
# 包むので、修正前のスクリプトで作った .app（ホームパス・個人名の署名入り）も通ってしまう。
# 公開物になる zip の手前でもう一度だけ見る
echo "==> 配布物の個人情報チェック（ビルド機のパス・署名の名義。#1848）"
check_bundle_privacy "$APP" || exit 1

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
    printf '  同梱アセット: %s\n' ${ASSET_NAMES[@]+"${ASSET_NAMES[@]}"}
  fi

  # CHANGELOG + 実アセットからリリースノートを組み立て（ダウンロード表・OS 別手順・
  # Windows 版があれば Known limitations も。組み立ては build_release_notes が正）
  RELEASE_NOTES=$(build_release_notes "$TAG" "$VERSION" ${ASSET_NAMES[@]+"${ASSET_NAMES[@]}"})

  echo "==> GitHub Release 作成: $TAG"

  # 冪等性: Release が既に存在する場合はアセット追加のみ（#256）
  if gh release view "$TAG" >/dev/null 2>&1; then
    echo "    Release $TAG は既に存在。アセットのアップロードのみ実行"
    gh release upload "$TAG" ${UPLOAD_PATHS[@]+"${UPLOAD_PATHS[@]}"} --clobber
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
          ${UPLOAD_PATHS[@]+"${UPLOAD_PATHS[@]}"} 2>"$GH_STDERR_FILE" || GH_EXIT=$?

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
          gh release upload "$TAG" ${UPLOAD_PATHS[@]+"${UPLOAD_PATHS[@]}"} --clobber
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

  # --- Windows 配布物の待ち合わせ（#965）---
  # Windows 版は tag push で起動する GitHub Actions
  # （.github/workflows/release-windows.yml）が同じ Release へ添付する。
  # 揃ってからノートを作り直す（ダウンロード表・Windows 手順・動作要件・
  # Known limitations はどれも実アセットから組み立てるため）
  if [[ $DRAFT -eq 1 ]]; then
    echo "==> ドラフトのため Windows 配布物の待ち合わせは省略"
  elif [[ $WAIT_WINDOWS -eq 0 ]]; then
    echo "==> --no-wait-windows: Windows 配布物を待たない（片肺のまま公開される）"
  elif wait_for_windows_assets "$TAG"; then
    echo "==> ノートを実アセットから作り直す（Windows の手順・要件・Known limitations を反映）"
    refresh_release_notes "$TAG"
  else
    echo "警告: Windows 配布物が揃わなかった。macOS 版のみのリリースになっている" >&2
  fi

  echo "==> リリース完了"

  # 片肺リリースの検出（#965 受け入れ条件 3）。ドラフトと明示的な --no-wait-windows は
  # 「今は揃っていない」ことを承知の上なので報告だけにする
  RELEASE_INCOMPLETE=0
  if [[ $DRAFT -eq 0 ]]; then
    report_release_completeness "$TAG" || RELEASE_INCOMPLETE=1
  fi
  if [[ $RELEASE_INCOMPLETE -eq 1 && $WAIT_WINDOWS -eq 1 ]]; then
    echo "" >&2
    echo "ERROR: リリース $TAG は片肺（macOS 版のみ）。上の手順で Windows 版を添付してから" >&2
    echo "       scripts/release.sh --update-notes $TAG でノートを作り直す" >&2
    # 3 = 「Release は作られたが両 OS が揃っていない」。1（Release 作成そのものの失敗）と
    # 区別できるようにする（nightly-release.sh がこの値で通知を出し分ける）
    exit 3
  fi

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
