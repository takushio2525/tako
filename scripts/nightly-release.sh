#!/usr/bin/env bash
# nightly-release.sh — 夜間パッチリリースのローカル自動実行（macOS / launchd）
#
# 使い方:
#   scripts/nightly-release.sh                     # 実行（変更が無ければスキップ）
#   scripts/nightly-release.sh --dry-run           # 判定のみ（何も変更しない）
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
#   4. Cargo.toml の version == 最新タグのときのみパッチ bump
#      （≠ は手動リリース進行中とみなしてスキップ。夜間ジョブは人間の作業に割り込まない）
#   5. origin/main へ detach → version bump + CHANGELOG 自動節 + Cargo.lock 同期をコミット
#   6. release.sh（ビルド + zip）→ 成功後にはじめて push（main → annotated tag）
#      → release.sh --skip-build --publish（GitHub Release + Pages デプロイ）
#   7. ビルド失敗時はローカルコミットを破棄してロールバック（リモートは無傷）
#
# ログ: ~/.claude-orchestrator/logs/tako-nightly-release.log
# 注意: Homebrew cask（homebrew-tako）の更新は対象外（手動リリース時のみ）
set -euo pipefail

# launchd 環境は最小 PATH のため明示設定（cargo / gh / npm / node を通す）
export PATH="$HOME/.cargo/bin:$HOME/.local/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin"

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

for arg in "$@"; do
  case "$arg" in
    --dry-run)            DRY_RUN=1 ;;
    --install-launchd)    install_launchd; exit 0 ;;
    --uninstall-launchd)  uninstall_launchd; exit 0 ;;
    *) echo "不明な引数: ${arg}（--dry-run / --install-launchd / --uninstall-launchd）" >&2; exit 2 ;;
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

# ---- 変更検知 --------------------------------------------------------------

# untracked はビルド残骸の可能性が高いので無視し、tracked の変更のみを作業中とみなす
# （リリースコミットは明示 add の 3 ファイルのみのため untracked が混入する余地はない）
if [[ -n "$(git status --porcelain --untracked-files=no)" ]]; then
  log "SKIP: worktree が dirty（人間の作業中と判断）: $REPO_ROOT"
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
  log "SKIP: 変更なし（$LATEST_TAG == origin/main）"
  exit 0
fi

CUR_VERSION=$(git show origin/main:Cargo.toml | sed -n 's/^version = "\(.*\)"/\1/p' | head -1)
TAG_VERSION="${LATEST_TAG#v}"
if [[ "$CUR_VERSION" != "$TAG_VERSION" ]]; then
  log "SKIP: Cargo.toml version ($CUR_VERSION) ≠ 最新タグ ($TAG_VERSION)。手動リリース進行中とみなす"
  notify "スキップ: 手動リリース進行中（${CUR_VERSION}）"
  exit 0
fi

IFS=. read -r major minor patch <<< "$CUR_VERSION"
NEW_VERSION="$major.$minor.$((patch + 1))"
NEW_TAG="v$NEW_VERSION"
TODAY=$(date '+%Y-%m-%d')

log "変更 $COMMITS 件（$LATEST_TAG..origin/main）→ $NEW_TAG としてリリースする"

if [[ $DRY_RUN -eq 1 ]]; then
  log "DRY-RUN: ここで終了（bump: $CUR_VERSION → ${NEW_VERSION}、コミット一覧は下記）"
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
  echo "Nightly patch release (automated). Changes since $LATEST_TAG:"
  echo "夜間パッチリリース（自動）。$LATEST_TAG 以降の変更:"
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
git commit --quiet -m "[リリース] $NEW_TAG: 夜間パッチリリース（自動）

$LATEST_TAG 以降の変更 $COMMITS 件を自動リリース。scripts/nightly-release.sh による。

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

git tag -a "$NEW_TAG" -m "tako $NEW_TAG — 夜間パッチリリース（自動）

$LATEST_TAG 以降の変更:
$(git log --format='- %s' "$LATEST_TAG..HEAD~1")"
git push origin "$NEW_TAG" --quiet

if ! "$REPO_ROOT/scripts/release.sh" --skip-build --publish >> "$LOG_FILE" 2>&1; then
  log "ERROR: GitHub Release 作成に失敗（tag $NEW_TAG は push 済み）。手動リカバリ: scripts/release.sh --skip-build --publish"
  notify "失敗: Release 作成（tag は push 済み、要手動リカバリ）"
  exit 1
fi

log "完了: ${NEW_TAG}（$COMMITS 件、https://github.com/takushio2525/tako/releases/tag/${NEW_TAG}）"
notify "リリース完了: ${NEW_TAG}（$COMMITS 件の変更）"
