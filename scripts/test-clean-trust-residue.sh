#!/usr/bin/env bash
# test-clean-trust-residue.sh — 掃除スクリプトのモックテスト（#1030 / #1022）
#
# 偽の `HOME` に作った `.claude.json` を相手に、
# 「テスト由来だけを消す / 実プロジェクトは残す / dry-run は 1 バイトも書かない」を実測する。
# **本番の ~/.claude.json には一切触れない**（HOME を差し替えるので参照すらしない）。
#
# 使い方: bash scripts/test-clean-trust-residue.sh
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PASS=0
FAIL=0
pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
fail() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }
check_eq() {
  if [ "$2" = "$3" ]; then pass "$1"; else fail "$1（期待 '${2}' / 実際 '${3}'）"; fi
}

FAKE_HOME="$(mktemp -d "${TMPDIR:-/tmp}/tako-trust-residue-test.XXXXXX")"
trap 'rm -rf "$FAKE_HOME"' EXIT
mkdir -p "$FAKE_HOME/.claude"

# 実プロジェクト 2 件 + 一時ディレクトリの「テスト名でない」もの 1 件 + テスト残骸 4 件
cat > "$FAKE_HOME/.claude.json" <<'JSON'
{
  "installMethod": "brew",
  "projects": {
    "/Users/testuser/dev/real-project": {"hasTrustDialogAccepted": true},
    "/Users/testuser/Documents/tako-wt-944": {"hasTrustDialogAccepted": true},
    "/private/tmp/some-other-tool/workspace": {"hasTrustDialogAccepted": true},
    "/private/tmp/tako-selftest-165-4242": {"hasTrustDialogAccepted": true},
    "/private/tmp/tako-e2e-571-4242/work": {"hasTrustDialogAccepted": true},
    "/private/tmp/tako-1055-prof-4242/want": {"hasTrustDialogAccepted": true},
    "/private/tmp/tako-t558-4242/cfg": {"hasTrustDialogAccepted": true}
  }
}
JSON
cp "$FAKE_HOME/.claude.json" "$FAKE_HOME/.claude/.claude.json"
BEFORE_HASH="$(shasum -a 256 "$FAKE_HOME/.claude.json" | cut -d' ' -f1)"

echo "1. dry-run は何も書かない"
OUT="$(HOME="$FAKE_HOME" bash "$REPO_ROOT/scripts/clean-trust-residue.sh" --json)"
check_eq "掃除対象は 2 ファイル合計 8 件" "8" "$(echo "$OUT" | /usr/bin/env python3 -c 'import json,sys; print(json.load(sys.stdin)["removable_total"])')"
check_eq "要確認は 2 件（テスト名でない一時 dir）" "2" "$(echo "$OUT" | /usr/bin/env python3 -c 'import json,sys; print(json.load(sys.stdin)["review_total"])')"
check_eq "dry-run でファイルが変わっていない" "$BEFORE_HASH" "$(shasum -a 256 "$FAKE_HOME/.claude.json" | cut -d' ' -f1)"

echo "2. --apply はテスト由来だけを消す"
HOME="$FAKE_HOME" bash "$REPO_ROOT/scripts/clean-trust-residue.sh" --apply > /dev/null
REMAIN="$(/usr/bin/env python3 -c '
import json, sys
d = json.load(open(sys.argv[1]))
print("\n".join(sorted(d["projects"])))
' "$FAKE_HOME/.claude.json")"
check_eq "残るのは 3 件" "3" "$(echo "$REMAIN" | grep -c .)"
case "$REMAIN" in
  *"/Users/testuser/dev/real-project"*) pass "実プロジェクトを残した" ;;
  *) fail "実プロジェクトを消した" ;;
esac
case "$REMAIN" in
  *"/private/tmp/some-other-tool/workspace"*) pass "テスト名でない一時 dir を残した" ;;
  *) fail "テスト名でない一時 dir を消した" ;;
esac
case "$REMAIN" in
  *"tako-selftest"* | *"tako-e2e"* | *"tako-1055"* | *"tako-t558"*) fail "テスト残骸が残っている" ;;
  *) pass "テスト残骸を全部消した" ;;
esac
check_eq "無関係なキーを保持している" "brew" "$(/usr/bin/env python3 -c '
import json, sys; print(json.load(open(sys.argv[1])).get("installMethod"))' "$FAKE_HOME/.claude.json")"
check_eq "退避ファイルが 1 つ残る" "1" "$(ls -1 "$FAKE_HOME"/.claude.json.residue-backup.* 2>/dev/null | wc -l | tr -d ' ')"

echo "3. 2 回目は何も残っていない（冪等）"
OUT2="$(HOME="$FAKE_HOME" bash "$REPO_ROOT/scripts/clean-trust-residue.sh" --json)"
check_eq "掃除対象 0 件" "0" "$(echo "$OUT2" | /usr/bin/env python3 -c 'import json,sys; print(json.load(sys.stdin)["removable_total"])')"

echo
echo "PASS=$PASS FAIL=$FAIL"
[ "$FAIL" -eq 0 ]
