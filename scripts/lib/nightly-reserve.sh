#!/usr/bin/env bash
# nightly-reserve.sh — 夜間リリースの「次回バージョン予約」の正本（#1005）
#
# 夜間リリース（scripts/nightly-release.sh）は既定で patch bump するが、節目の
# リリース（minor / major の繰り上げ）を夜間発火に乗せる手段が無かった:
# Cargo.toml を先に上げると「Cargo.toml version ≠ 最新タグ = 手動リリース進行中」で
# スキップされてしまう。そこで「次の 1 回だけ版数を指定する」予約を別に持つ。
#
# 予約の読み書き・検証・版種（patch / minor / major）判定はここが 1 実装。
# nightly-release.sh と scripts/test-nightly-reserve.sh の両方がこれを source する。
#
# 予約ファイル: $HOME/.claude-orchestrator/state/tako-nightly-next-version
#   **リポジトリの外**に置く。理由は 3 つ:
#     - worktree を dirty にしない（dirty = 人間の作業中とみなしてスキップされる）
#     - git checkout --detach / reset --hard（ロールバック）で消えない
#     - 誤コミットの余地が無い（public リポなので状態ファイルを混ぜたくない）
#   ログ / ロックと同じ ~/.claude-orchestrator/ 配下に揃える。
#   書式: `#` から行末はコメント。最初の非空行が版数（前後の空白は無視）。
#
# 版数は **bash 3.2 で動く**必要がある（launchd は /bin/bash = 3.2 で起動する）。
# 連想配列・${var,,}・mapfile 等の bash 4 以降の機能は使わない。

# 予約ファイルの置き場（HOME 基準。テストは HOME を差し替えて隔離する）
nightly_reserve_file() {
  printf '%s\n' "${HOME}/.claude-orchestrator/state/tako-nightly-next-version"
}

# 予約値を読む。無ければ何も出さない（正常終了）
nightly_reserve_read() {
  local file
  file=$(nightly_reserve_file)
  if [[ ! -f "$file" ]]; then
    return 0
  fi
  # コメントと空行を落として最初の 1 行だけを返す。
  # awk 1 プロセスで済ませる（grep をパイプに挟むと pipefail で無マッチが失敗になる）
  awk '{ gsub(/\r/, ""); sub(/#.*/, ""); gsub(/^[ \t]+|[ \t]+$/, ""); if ($0 != "") { print; exit } }' "$file"
}

# 予約を書く（原子的に差し替える）。$2 はメモ（任意）
nightly_reserve_write() {
  local version="$1"
  local note="${2:-}"
  local file dir tmp
  file=$(nightly_reserve_file)
  dir=$(dirname "$file")
  mkdir -p "$dir"
  tmp="${file}.tmp.$$"
  {
    echo "# tako 夜間リリース: 次回バージョン予約（#1005）"
    echo "# 予約日時: $(date '+%Y-%m-%d %H:%M:%S')"
    if [[ -n "$note" ]]; then
      echo "# メモ: ${note}"
    fi
    echo "# 次に成立したリリース 1 回で消費される。取消: scripts/nightly-release.sh --unreserve"
    echo "$version"
  } > "$tmp"
  mv "$tmp" "$file"
}

# 予約を消す（無くてもエラーにしない）
nightly_reserve_clear() {
  local file
  file=$(nightly_reserve_file)
  rm -f "$file"
}

# 安定版 semver（X.Y.Z）か。プレリリース / ビルドメタデータ付きは通さない
nightly_version_is_stable() {
  local re='^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$'
  [[ "$1" =~ $re ]]
}

# $1 > $2 か（数値比較）。判定不能なら false（呼び出し側の検証漏れで落ちないように）
nightly_version_gt() {
  local -a a b
  if ! nightly_version_is_stable "$1" || ! nightly_version_is_stable "$2"; then
    return 1
  fi
  IFS=. read -r -a a <<< "$1"
  IFS=. read -r -a b <<< "$2"
  local i
  for i in 0 1 2; do
    if (( ${a[i]} > ${b[i]} )); then
      return 0
    fi
    if (( ${a[i]} < ${b[i]} )); then
      return 1
    fi
  done
  return 1
}

# 現行 $1 から新 $2 への繰り上げ種別を返す（major / minor / patch）。
# 判定不能なら patch（既定の夜間リリースと同じ扱い）
nightly_bump_kind() {
  local -a cur new
  if ! nightly_version_is_stable "$1" || ! nightly_version_is_stable "$2"; then
    echo "patch"
    return 0
  fi
  IFS=. read -r -a cur <<< "$1"
  IFS=. read -r -a new <<< "$2"
  if [[ "${new[0]}" != "${cur[0]}" ]]; then
    echo "major"
  elif [[ "${new[1]}" != "${cur[1]}" ]]; then
    echo "minor"
  else
    echo "patch"
  fi
}

# 版種の表示名（リリースノート / コミット件名 / 通知で使う）
nightly_bump_label_ja() {
  case "$1" in
    major) echo "メジャー" ;;
    minor) echo "マイナー" ;;
    *)     echo "パッチ" ;;
  esac
}

nightly_bump_label_en() {
  case "$1" in
    major) echo "major" ;;
    minor) echo "minor" ;;
    *)     echo "patch" ;;
  esac
}

# 現行版数から patch を 1 つ上げた版数（予約が無いときの既定）。
# 判定不能なら入力をそのまま返す（表示用の経路で落ちないように）
nightly_patch_bump() {
  local -a cur
  if ! nightly_version_is_stable "$1"; then
    echo "$1"
    return 0
  fi
  IFS=. read -r -a cur <<< "$1"
  echo "${cur[0]}.${cur[1]}.$(( cur[2] + 1 ))"
}

# 予約値が使えない理由を返す（使えるなら何も出さず 0）。
#   $1 = 予約値 / $2 = 現行版数（省略可）/ $3 = リポジトリのパス（省略可・タグ衝突検査用）
#
# 「使えないなら予約を無視して patch bump へフォールバックする」判断はここ 1 箇所。
nightly_reserve_reject_reason() {
  local version="$1"
  local current="${2:-}"
  local repo="${3:-}"

  if [[ -z "$version" ]]; then
    echo "版数が空"
    return 1
  fi
  if ! nightly_version_is_stable "$version"; then
    echo "semver（X.Y.Z）ではない: ${version}"
    return 1
  fi
  if [[ -n "$current" ]]; then
    if ! nightly_version_is_stable "$current"; then
      echo "現行版数が安定版ではない: ${current}"
      return 1
    fi
    if ! nightly_version_gt "$version" "$current"; then
      echo "現行 ${current} より大きくない: ${version}"
      return 1
    fi
  fi
  if [[ -n "$repo" ]]; then
    if git -C "$repo" rev-parse -q --verify "refs/tags/v${version}" >/dev/null 2>&1; then
      echo "タグ v${version} が既に存在する"
      return 1
    fi
  fi
  return 0
}
