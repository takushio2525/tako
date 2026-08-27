#!/usr/bin/env bash
# nightly-release.sh — 夜間パッチリリースのローカル自動実行（macOS / launchd）
#
# 使い方:
#   scripts/nightly-release.sh                     # 実行（変更が無ければスキップ）
#   scripts/nightly-release.sh --dry-run           # 判定のみ（何も変更しない）
#   scripts/nightly-release.sh --reserve           # 次回バージョンの予約を表示
#   scripts/nightly-release.sh --reserve 0.8.0     # 次回バージョンを予約（1 回で消費）
#   scripts/nightly-release.sh --unreserve         # 予約を取消
#   scripts/nightly-release.sh --install-launchd   # launchd ジョブ（毎日 5:00）を登録
#   scripts/nightly-release.sh --uninstall-launchd # launchd ジョブを解除
#
# 背景（#166）:
#   クラウドルーチンによる夜間リリースは ①バージョン計算の不整合 ②クラウドから
#   main 直 push する設計 ③macOS バイナリを作れない、の三重苦で機能しなかったため、
#   self-improve と同じ launchd 方式のローカルジョブへ置き換えた。
#
# 動作（1 回の実行）:
#   1. 多重起動ロック（~/.claude-orchestrator/locks/）を取得。取れなければ即終了
#   2. worktree が clean か確認（dirty = 人間の作業中 → スキップ）
#   3. git fetch → 最新タグ vs origin/main。差分ゼロなら「変更なしスキップ」
#   4. Cargo.toml の version == 最新タグのときのみ bump する
#      （≠ は手動リリース進行中とみなしてスキップ。夜間ジョブは人間の作業に割り込まない）
#      版数は「次回バージョン予約があればその値、無ければ patch bump」（#1005）
#   5. origin/main へ detach → version bump + CHANGELOG 自動節 + Cargo.lock 同期をコミット
#   6. release.sh（ビルド + zip）→ 成功後にはじめて push（main → annotated tag）
#      → release.sh --skip-build --test（GitHub Release + Pages デプロイ）
#   7. ビルド失敗時はローカルコミットを破棄してロールバック（リモートは無傷）
#
# 両 OS 同時リリース（#965）:
#   タグ push が GitHub Actions（.github/workflows/release-windows.yml）を起こし、
#   windows ランナーが installer exe / ポータブル zip を同じ Release へ添付する。
#   release.sh はその添付を待ってからノートを作り直すので、夜間ジョブは
#   **両 OS が揃うまで（既定で最大 75 分）待って**から完了する。
#   片肺で終わった場合は release.sh が exit 3 を返し、ここで警告として通知する
#   （Release 自体は macOS 版で成立しているので、ロールバックはしない）。
#
# 次回バージョンの予約（#1005）:
#   節目のリリース（minor / major の繰り上げ）を夜間発火に乗せるための仕組み。
#   Cargo.toml を先に上げると「≠ 最新タグ = 手動リリース進行中」でスキップされるため、
#   版数の指定はリポジトリの外の状態ファイルで持つ（正本は scripts/lib/nightly-reserve.sh）。
#
#     予約する: scripts/nightly-release.sh --reserve 0.8.0
#     確認する: scripts/nightly-release.sh --reserve
#     取消する: scripts/nightly-release.sh --unreserve
#
#   - 予約は **成立したリリース 1 回で消費**される（タグを push した時点でクリア）
#   - 予約が使えない（semver 外 / 現行以下 / タグが既に在る）ときは**予約を無視**して
#     既定の patch bump へフォールバックし、警告ログ + 通知を出す
#   - **リリースに至らなかったときは予約を保持する**。「予約あり + 変更ゼロ」でも
#     消費しない（次に変更が入った夜へ持ち越す）。dry-run・worktree dirty・
#     手動リリース進行中・プレリリース版・ビルド失敗も同じく保持
#
# ログ: ~/.claude-orchestrator/logs/tako-nightly-release.log
# 注意: Homebrew cask（homebrew-tako）の更新は対象外（手動リリース時のみ）
set -euo pipefail

# launchd 環境は最小 PATH のため明示設定（cargo / gh / npm / node を通す）
export PATH="$HOME/.cargo/bin:$HOME/.local/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin"
# Node.js バージョンマネージャの標準所在を補う（Pages デプロイに npm/node が必要。#297）
for _p in "$HOME/.nodebrew/current/bin" "$HOME/.volta/bin" "$HOME/.fnm" "$HOME/n/bin"; do
  [[ -d "$_p" ]] && PATH="$_p:$PATH"
done
export PATH

cd "$(dirname "$0")/.."
REPO_ROOT=$PWD

LABEL="com.takushio.tako-nightly-release"
LOG_DIR="$HOME/.claude-orchestrator/logs"
LOG_FILE="$LOG_DIR/tako-nightly-release.log"
LOCK_DIR="$HOME/.claude-orchestrator/locks/tako-nightly-release.lock"
PLIST="$HOME/Library/LaunchAgents/$LABEL.plist"
DRY_RUN=0

mkdir -p "$LOG_DIR"

log() {
  echo "$(date '+%Y-%m-%d %H:%M:%S') $*" | tee -a "$LOG_FILE"
}

notify() {
  osascript -e "display notification \"$1\" with title \"tako 夜間リリース\"" 2>/dev/null || true
}

# 次回バージョン予約の正本（#1005。読み書き・検証・版種判定）
# shellcheck source=lib/nightly-reserve.sh
if [[ ! -f "$REPO_ROOT/scripts/lib/nightly-reserve.sh" ]]; then
  log "ERROR: scripts/lib/nightly-reserve.sh が見つからない（${REPO_ROOT}）。リポジトリが不完全"
  notify "失敗: nightly-reserve.sh が見つからない"
  exit 1
fi
source "$REPO_ROOT/scripts/lib/nightly-reserve.sh"

# ---- 次回バージョン予約の CLI（#1005）--------------------------------------

# 予約時点の「現行」は最新の v* タグ（オフラインで引ける・実際に出た版）。
# ローカルの取得状況次第で古いことがあるが、**発火時に origin/main の Cargo.toml で
# 再検証する**ので、抜けた予約はその場で無視される（二段の検証）
released_version() {
  local tag
  tag=$(git -C "$REPO_ROOT" tag --list 'v*' --sort=-v:refname 2>/dev/null | head -1 || true)
  printf '%s' "${tag#v}"
}

show_reservation() {
  local reserved current reason kind
  reserved=$(nightly_reserve_read)
  current=$(released_version)
  if [[ -z "$reserved" ]]; then
    echo "予約: なし（次回は patch bump: ${current:-?} → $(nightly_patch_bump "${current:-0.0.0}")）"
  elif reason=$(nightly_reserve_reject_reason "$reserved" "$current" "$REPO_ROOT"); then
    kind=$(nightly_bump_kind "$current" "$reserved")
    echo "予約: ${reserved}（現行 ${current} → v${reserved}・$(nightly_bump_label_ja "${kind}")繰り上げ）"
  else
    echo "予約: ${reserved}（**無効**: ${reason} → 次回は予約を無視して patch bump）"
  fi
  echo "予約ファイル: $(nightly_reserve_file)"
  echo "操作: --reserve <X.Y.Z> で予約 / --unreserve で取消"
}

reserve_version() {
  local version="$1"
  local current reason kind
  current=$(released_version)
  if ! reason=$(nightly_reserve_reject_reason "$version" "$current" "$REPO_ROOT"); then
    echo "ERROR: 予約できない: ${reason}" >&2
    echo "  安定版の semver（X.Y.Z）で、現行 ${current} より大きく、タグが未存在の版数を指定してください" >&2
    exit 2
  fi
  kind=$(nightly_bump_kind "$current" "$version")
  nightly_reserve_write "$version" "現行 ${current} からの$(nightly_bump_label_ja "${kind}")繰り上げ"
  echo "予約しました: ${version}（現行 ${current} → 次回の夜間リリースは v${version}・$(nightly_bump_label_ja "${kind}")繰り上げ）"
  echo "予約ファイル: $(nightly_reserve_file)"
  echo "取消: scripts/nightly-release.sh --unreserve"
}

unreserve_version() {
  local reserved
  reserved=$(nightly_reserve_read)
  nightly_reserve_clear
  if [[ -n "$reserved" ]]; then
    echo "予約を取消しました: ${reserved}（次回は patch bump）"
  else
    echo "予約はありません（変更なし）"
  fi
}

# ---- launchd 登録 / 解除 ------------------------------------------------

# worktree 検出: 一時 worktree なら本体リポのパスを返す（#205 再発防止）
resolve_main_repo() {
  local git_dir git_common_dir
  git_dir=$(git rev-parse --git-dir 2>/dev/null) || { echo "$REPO_ROOT"; return; }
  git_common_dir=$(git rev-parse --git-common-dir 2>/dev/null) || { echo "$REPO_ROOT"; return; }
  [[ "$git_dir" = /* ]] || git_dir="$REPO_ROOT/$git_dir"
  [[ "$git_common_dir" = /* ]] || git_common_dir="$REPO_ROOT/$git_common_dir"
  git_dir=$(cd "$git_dir" && pwd -P)
  git_common_dir=$(cd "$git_common_dir" && pwd -P)
  if [[ "$git_dir" != "$git_common_dir" ]]; then
    dirname "$git_common_dir"
  else
    echo "$REPO_ROOT"
  fi
}

install_launchd() {
  local install_root
  install_root=$(resolve_main_repo)

  if [[ "$install_root" != "$REPO_ROOT" ]]; then
    if [[ ! -x "$install_root/scripts/nightly-release.sh" ]]; then
      echo "ERROR: 一時 worktree から実行されましたが、本体リポ ($install_root) に scripts/nightly-release.sh が見つかりません" >&2
      exit 1
    fi
    echo "NOTE: 一時 worktree を検出。本体リポのパスで登録します: $install_root"
  fi

  mkdir -p "$(dirname "$PLIST")"
  cat > "$PLIST" <<PLIST_END
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>Label</key>
	<string>$LABEL</string>
	<key>ProgramArguments</key>
	<array>
		<string>/bin/bash</string>
		<string>$install_root/scripts/nightly-release.sh</string>
	</array>
	<key>StartCalendarInterval</key>
	<dict>
		<key>Hour</key>
		<integer>5</integer>
		<key>Minute</key>
		<integer>0</integer>
	</dict>
	<key>WorkingDirectory</key>
	<string>$install_root</string>
	<key>StandardOutPath</key>
	<string>$LOG_DIR/launchd-tako-nightly-release.log</string>
	<key>StandardErrorPath</key>
	<string>$LOG_DIR/launchd-tako-nightly-release.log</string>
	<key>EnvironmentVariables</key>
	<dict>
		<key>HOME</key>
		<string>$HOME</string>
	</dict>
</dict>
</plist>
PLIST_END
  launchctl unload "$PLIST" 2>/dev/null || true
  launchctl load "$PLIST"
  echo "登録完了: ${LABEL}（毎日 5:00、対象リポ: ${install_root}）"
  echo "確認: launchctl list | grep tako-nightly"
}

uninstall_launchd() {
  launchctl unload "$PLIST" 2>/dev/null || true
  rm -f "$PLIST"
  echo "解除完了: $LABEL"
}

ARG_HINT="--dry-run / --reserve [<X.Y.Z>] / --unreserve / --install-launchd / --uninstall-launchd"
while [[ $# -gt 0 ]]; do
  case "$1" in
    --dry-run)            DRY_RUN=1; shift ;;
    --install-launchd)    install_launchd; exit 0 ;;
    --uninstall-launchd)  uninstall_launchd; exit 0 ;;
    # 引数なしで現在値（#322 の「最も簡単なコマンド」原則）
    --reserve)
      shift
      if [[ $# -gt 0 && "$1" != --* ]]; then
        reserve_version "$1"
      else
        show_reservation
      fi
      exit 0
      ;;
    --unreserve)          unreserve_version; exit 0 ;;
    *) echo "不明な引数: ${1}（${ARG_HINT}）" >&2; exit 2 ;;
  esac
done

# ---- パス妥当性チェック（#205: worktree 撤去で launchd 参照先が消失した場合の早期検出）
if [[ ! -f "$REPO_ROOT/scripts/nightly-release.sh" ]]; then
  log "ERROR: スクリプトパスが無効 ($REPO_ROOT/scripts/nightly-release.sh)。launchd の参照先が消えた可能性。本体リポから --install-launchd を再実行してください"
  notify "失敗: スクリプトパスが無効"
  exit 1
fi
if ! git -C "$REPO_ROOT" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  log "ERROR: 有効な git リポジトリではない ($REPO_ROOT)。本体リポから --install-launchd を再実行してください"
  notify "失敗: git リポジトリが無効"
  exit 1
fi

# ---- 前提チェック --------------------------------------------------------

if [[ "$(uname)" != "Darwin" ]]; then
  log "SKIP: macOS 専用（バイナリビルドが不能）"
  exit 0
fi
for tool in git gh cargo; do
  if ! command -v "$tool" >/dev/null; then
    log "ERROR: $tool が見つからない（PATH: ${PATH}）"
    notify "失敗: $tool が見つからない"
    exit 1
  fi
done

# ---- 多重起動ロック --------------------------------------------------------
# mkdir はアトミック。stale ロック（前回実行の異常死）は記録 PID の生存で判定する

mkdir -p "$(dirname "$LOCK_DIR")"
if ! mkdir "$LOCK_DIR" 2>/dev/null; then
  old_pid=$(cat "$LOCK_DIR/pid" 2>/dev/null || echo "")
  if [[ -n "$old_pid" ]] && kill -0 "$old_pid" 2>/dev/null; then
    log "SKIP: 多重起動（実行中 PID: ${old_pid}）"
    exit 0
  fi
  log "WARN: stale ロックを回収（旧 PID: ${old_pid:-不明}）"
  rm -rf "$LOCK_DIR"
  mkdir "$LOCK_DIR"
fi
echo $$ > "$LOCK_DIR/pid"
trap 'rm -rf "$LOCK_DIR"' EXIT

# ---- 次回バージョン予約の読み取り（この時点では消費しない）------------------
# 「リリースに至ったときだけ消費する」ため、以降のスキップ経路では保持したまま抜ける

RESERVED=$(nightly_reserve_read)
RESERVE_KEPT=""
if [[ -n "$RESERVED" ]]; then
  RESERVE_KEPT="（予約 ${RESERVED} は保持）"
  log "予約あり: ${RESERVED}（$(nightly_reserve_file)）"
fi

# ---- 変更検知 --------------------------------------------------------------

# untracked はビルド残骸の可能性が高いので無視し、tracked の変更のみを作業中とみなす
# （リリースコミットは明示 add の 3 ファイルのみのため untracked が混入する余地はない）
if [[ -n "$(git status --porcelain --untracked-files=no)" ]]; then
  log "SKIP: worktree が dirty（人間の作業中と判断）: ${REPO_ROOT}${RESERVE_KEPT}"
  notify "スキップ: worktree が dirty"
  exit 0
fi

git fetch origin --tags --quiet

LATEST_TAG=$(git tag --list 'v*' --sort=-v:refname | head -1)
if [[ -z "$LATEST_TAG" ]]; then
  log "ERROR: v* タグが 1 つも見つからない"
  exit 1
fi

COMMITS=$(git rev-list --count "$LATEST_TAG..origin/main")
if [[ "$COMMITS" -eq 0 ]]; then
  # 予約は消費しない。リリースが 1 回も成立していないので次の夜へ持ち越す（#1005）
  log "SKIP: 変更なし（${LATEST_TAG} == origin/main）${RESERVE_KEPT}"
  exit 0
fi

CUR_VERSION=$(git show origin/main:Cargo.toml | sed -n 's/^version = "\(.*\)"/\1/p' | head -1)
TAG_VERSION="${LATEST_TAG#v}"
if [[ "$CUR_VERSION" != "$TAG_VERSION" ]]; then
  log "SKIP: Cargo.toml version (${CUR_VERSION}) ≠ 最新タグ (${TAG_VERSION})。手動リリース進行中とみなす${RESERVE_KEPT}"
  notify "スキップ: 手動リリース進行中（${CUR_VERSION}）"
  exit 0
fi

# テスト版バージョン（-test.N 等のプレリリースサフィックス付き）は bump の対象外
if [[ "$CUR_VERSION" == *-* ]]; then
  log "SKIP: プレリリース版 (${CUR_VERSION})。夜間 bump は安定版のみ対象${RESERVE_KEPT}"
  notify "スキップ: プレリリース版（${CUR_VERSION}）"
  exit 0
fi

# ---- 次回バージョンの決定（予約 > 既定の patch bump。#1005）------------------

PATCH_VERSION=$(nightly_patch_bump "$CUR_VERSION")
NEW_VERSION="$PATCH_VERSION"
VERSION_SOURCE="既定の patch bump"
if [[ -n "$RESERVED" ]]; then
  if REJECT=$(nightly_reserve_reject_reason "$RESERVED" "$CUR_VERSION" "$REPO_ROOT"); then
    NEW_VERSION="$RESERVED"
    VERSION_SOURCE="予約（--reserve ${RESERVED}）"
  else
    # 予約は無視して従来どおり patch bump（予約自体はリリース成立時にまとめて消費する）
    log "WARN: 予約 ${RESERVED} は使えないので無視する（${REJECT}）→ 既定の patch bump (${PATCH_VERSION}) で続行"
    notify "予約を無視: ${RESERVED}（${REJECT}）"
    VERSION_SOURCE="既定の patch bump（予約 ${RESERVED} は無効: ${REJECT}）"
  fi
fi
NEW_TAG="v$NEW_VERSION"
BUMP_KIND=$(nightly_bump_kind "$CUR_VERSION" "$NEW_VERSION")
BUMP_JA=$(nightly_bump_label_ja "$BUMP_KIND")
BUMP_EN=$(nightly_bump_label_en "$BUMP_KIND")
TODAY=$(date '+%Y-%m-%d')

log "変更 ${COMMITS} 件（${LATEST_TAG}..origin/main）→ ${NEW_TAG} としてリリースする（${BUMP_KIND} / ${VERSION_SOURCE}）"

if [[ $DRY_RUN -eq 1 ]]; then
  # dry-run は何も変更しない。予約もクリアしない
  log "DRY-RUN: ここで終了（bump: ${CUR_VERSION} → ${NEW_VERSION}、種別: ${BUMP_KIND}、由来: ${VERSION_SOURCE}）"
  if [[ -n "$RESERVED" ]]; then
    log "DRY-RUN: 予約 ${RESERVED} はクリアしない（消費は実リリース時のみ）"
  fi
  log "DRY-RUN: コミット一覧は下記"
  git log --format='  - %s' "$LATEST_TAG..origin/main" | tee -a "$LOG_FILE"
  exit 0
fi

# ---- バージョン bump + CHANGELOG 自動節 -------------------------------------

git checkout --detach origin/main --quiet

rollback() {
  log "ROLLBACK: ローカル変更を破棄して origin/main へ戻す"
  git checkout --detach origin/main --quiet || true
  git reset --hard origin/main --quiet || true
}

# Cargo.toml: [workspace.package] の version 行（最初の完全一致行のみ）を書き換え
awk -v old="version = \"$CUR_VERSION\"" -v new="version = \"$NEW_VERSION\"" \
  '!done && $0 == old { print new; done = 1; next } { print }' \
  Cargo.toml > Cargo.toml.tmp && mv Cargo.toml.tmp Cargo.toml

# CHANGELOG.md: 最新バージョン節の直前に自動生成節を挿入
SECTION_FILE=$(mktemp)
{
  echo "## [$NEW_VERSION] - $TODAY"
  echo ""
  echo "Nightly ${BUMP_EN} release (automated). Changes since ${LATEST_TAG}:"
  echo "夜間${BUMP_JA}リリース（自動）。${LATEST_TAG} 以降の変更:"
  echo ""
  git log --format='- %s' "$LATEST_TAG..origin/main"
  echo ""
} > "$SECTION_FILE"
awk -v secfile="$SECTION_FILE" '
  !ins && /^## \[/ { while ((getline line < secfile) > 0) print line; close(secfile); ins = 1 }
  { print }
' CHANGELOG.md > CHANGELOG.md.tmp && mv CHANGELOG.md.tmp CHANGELOG.md
rm -f "$SECTION_FILE"

# Cargo.lock の workspace メンバー版数を同期
if ! cargo update --workspace --quiet; then
  log "ERROR: cargo update --workspace が失敗"
  rollback
  notify "失敗: Cargo.lock 同期（詳細はログ）"
  exit 1
fi

git add Cargo.toml Cargo.lock CHANGELOG.md
git commit --quiet -m "[リリース] ${NEW_TAG}: 夜間${BUMP_JA}リリース（自動）

${LATEST_TAG} 以降の変更 ${COMMITS} 件を自動リリース。scripts/nightly-release.sh による。
版数の由来: ${VERSION_SOURCE}

$(git log --format='- %s' "$LATEST_TAG..origin/main")"

# ---- ビルド（失敗したらリモートに触れる前にロールバック） --------------------

log "ビルド開始（release.sh: build + zip）"
if ! "$REPO_ROOT/scripts/release.sh" >> "$LOG_FILE" 2>&1; then
  log "ERROR: ビルド失敗。リリースを中止しロールバックする"
  rollback
  notify "失敗: ビルド（$NEW_TAG は作られていない）"
  exit 1
fi

# ---- push + タグ + GitHub Release -------------------------------------------

log "ビルド成功 → push + タグ + GitHub Release"
git push origin HEAD:main --quiet

git tag -a "$NEW_TAG" -m "tako ${NEW_TAG} — 夜間${BUMP_JA}リリース（自動）

$LATEST_TAG 以降の変更:
$(git log --format='- %s' "$LATEST_TAG..HEAD~1")"
git push origin "$NEW_TAG" --quiet

# 予約は「次の 1 回」ぶん。**版数が確定した（タグを push した）時点で消費**する。
# ここへ到達しなかった場合（スキップ / ビルド失敗 / dry-run）は予約を保持する（#1005）
if [[ -n "$RESERVED" ]]; then
  nightly_reserve_clear
  log "予約 ${RESERVED} を消費した（予約ファイルをクリア。次回からは patch bump）"
fi

# 夜間リリースはテスト版（prerelease）として配布する（#403）。
# release.sh は Windows 配布物（tag push で起動する GitHub Actions）を待ってから
# ノートを作り直す。終了コードは 0 = 両 OS 揃った / 3 = 片肺 / それ以外 = 作成失敗（#965）
RELEASE_RC=0
"$REPO_ROOT/scripts/release.sh" --skip-build --test >> "$LOG_FILE" 2>&1 || RELEASE_RC=$?

RELEASE_URL="https://github.com/takushio2525/tako/releases/tag/${NEW_TAG}"
case "$RELEASE_RC" in
  0)
    log "完了: ${NEW_TAG}（テスト版・両 OS の配布物あり、${COMMITS} 件、${BUMP_KIND}、${RELEASE_URL}）"
    notify "テスト版リリース完了: ${NEW_TAG}（${COMMITS} 件、mac/win 両方）"
    ;;
  3)
    # Release 自体は macOS 版で成立している。ロールバックはしない（tag も push 済み）
    log "WARN: ${NEW_TAG} は片肺リリース（Windows 配布物が未添付）。${RELEASE_URL}"
    log "  確認: gh run list --workflow release-windows.yml"
    log "  回収: gh run download <run-id> → gh release upload ${NEW_TAG} <file> --clobber"
    log "  実機で作る場合: pwsh -File installer/windows/release-windows.ps1 -Tag ${NEW_TAG} -Upload"
    log "  添付後: scripts/release.sh --update-notes ${NEW_TAG}"
    notify "片肺リリース: ${NEW_TAG}（Windows 版が未添付・要対応）"
    exit 1
    ;;
  *)
    log "ERROR: GitHub Release 作成に失敗（exit ${RELEASE_RC}、tag $NEW_TAG は push 済み）。手動リカバリ: scripts/release.sh --skip-build --test"
    notify "失敗: Release 作成（tag は push 済み、要手動リカバリ）"
    exit 1
    ;;
esac
