#!/bin/bash
# docs と能力マトリクスが「追跡先」として貼る Issue が open かを検査する（Issue #1547）
#
# **追跡先と根拠を分ける**のがこの検査の前提:
#
#   追跡先 = まだ残っている仕事の在り処。**open でなければならない**
#            （`pending(note, N)` の N / docs の「追跡: [#N]」）
#   根拠   = 済んだ調査・実装・実測の引用。**closed でよい**
#            （Note 本文や evidence の中の `#N`、手書きページの経緯の引用）
#
# 閉じた Issue を追跡先に置いたままにすると、docs には「追跡: #N」と出るのに
# N を開くと完了していて、読む側（利用者・AI エージェント）が残りの仕事を辿れなくなる。
# #1547 の棚卸しでは #757 / #983 / #984 / #1033 / #1067 が閉じたあとも
# 13 マスが指し続け、docs の 17 件の Issue 参照のうち 10 件が closed だった。
#
#   bash scripts/check-docs-issue-refs.sh
#
# gh が無い / 未認証の環境では**検査せずに 0 で抜ける**（open かどうかは
# GitHub に問う以外に知る方法が無く、ローカル作業を止める理由にはならない）。
# CI では gh と GITHUB_TOKEN が必ず在るので、そこが実効的な関門になる。
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

MATRIX_SRC="crates/tako-core/src/agent_support.rs"
DOCS_DIR="docs/src/content/docs"

# ── 追跡先の収集 ────────────────────────────────────────────
# (1) 能力マトリクスの pending(<note>, <番号>)
matrix_refs="$(grep -oE 'pending\([A-Za-z_:]+[A-Za-z_]*, *[0-9]+\)' "$MATRIX_SRC" \
  | grep -oE '[0-9]+' || true)"
matrix_sites="$(printf '%s\n' "$matrix_refs" | grep -c '[0-9]' || true)"
if [ "$matrix_sites" -lt 20 ]; then
  echo "[NG] $MATRIX_SRC から追跡先を $matrix_sites 件しか拾えていない。" >&2
  echo "     pending(...) の書き方が変わったので、この検査の走査も直すこと。" >&2
  exit 1
fi

# (2) docs の「追跡: [#番号]」（生成ページ・手書きページの両方）
docs_refs="$(grep -rhoE '追跡: \[#[0-9]+\]' "$DOCS_DIR" | grep -oE '[0-9]+' || true)"

numbers="$(printf '%s\n%s\n' "$matrix_refs" "$docs_refs" | grep -E '^[0-9]+$' | sort -un)"
count="$(printf '%s\n' "$numbers" | grep -c '[0-9]' || true)"
echo "追跡先として貼られている Issue: $count 件: $(printf '%s' "$numbers" | tr '\n' ' ')"

# ── gh が使えないなら検査せず抜ける ──────────────────────────
if ! command -v gh >/dev/null 2>&1; then
  echo "[SKIP] gh が無いので open / closed を確認できない（CI では検査される）"
  exit 0
fi
if ! gh auth status >/dev/null 2>&1; then
  echo "[SKIP] gh が未認証なので open / closed を確認できない（CI では検査される）"
  exit 0
fi

# ── open かを問う ──────────────────────────────────────────
closed=""
for n in $numbers; do
  state="$(gh issue view "$n" --json state -q .state 2>/dev/null || echo UNKNOWN)"
  case "$state" in
    OPEN) printf '  #%-6s OPEN\n' "$n" ;;
    UNKNOWN)
      echo "[NG] #$n の状態を取得できなかった（存在しない番号か、権限が無い）" >&2
      closed="$closed $n"
      ;;
    *)
      printf '  #%-6s %s  <- 追跡先が閉じている\n' "$n" "$state"
      closed="$closed $n"
      ;;
  esac
done

if [ -n "$closed" ]; then
  cat >&2 <<MSG

[NG] 閉じた Issue を追跡先に貼っている:$closed

直し方:
  - $MATRIX_SRC の pending(<note>, <番号>) を、まだ open な
    追跡先（残りの仕事を持つ Issue / 親エピック）へ付け替える
  - 済んだ調査・実装を引きたいだけなら、番号は Note 本文か evidence 側へ移す
    （そちらは closed でよい。むしろ確定した事実として強い）
  - 生成ページは node scripts/gen-agent-support-docs.mjs で作り直す
MSG
  exit 1
fi

echo "[OK] 追跡先の Issue はすべて open"
