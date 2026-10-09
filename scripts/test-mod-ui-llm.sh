#!/bin/bash
# #1960: 軽いモデル（Sonnet / Haiku）が `tako mod ui` の口だけで Claude Code の画面の UI を調整しても
# ui.json を壊さないか（スキーマ違反 0 件）と、依頼どおりになったか（一致率）を測る。**CI 外**。
#
# tako:run: bash scripts/test-mod-ui-llm.sh
#
#   bash scripts/test-mod-ui-llm.sh            # 採点器の検証だけ（oracle = 20/20・null = 1/20 のはず。claude 不要）
#   ANTHROPIC_API_KEY=<key> MODELS="sonnet haiku" bash scripts/test-mod-ui-llm.sh --claude
#
# 依頼 20 件と判定の正本は scripts/lib/mod-ui-llm.py の CASES。
#
# **本物の認証・設定 dir を写さない**: claude は一時の HOME / CLAUDE_CONFIG_DIR で動かし、認証は呼び手が
# 渡した ANTHROPIC_API_KEY だけ（ファイルへは書かない）。無ければ claude の段は「未実測」で終了コード 4。
# tako は一時の TAKO_DATA_DIR（`TAKO_ISOLATED=1`）。使ってよい道具は `Bash(tako mod ui:*)` だけ。
set -uo pipefail

unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_ORCHESTRATOR_ROLE TAKO_MCP_URL \
  TAKO_CLI CLAUDE_CODE_PLUGIN_DIRS CLAUDE_CONFIG_DIR TAKO_DATA_DIR

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT" || exit 1
WITH_CLAUDE=0
[ "${1:-}" = "--claude" ] && WITH_CLAUDE=1
MODELS="${MODELS:-sonnet haiku}"
# claude の場所は env を空にする前に決める（一時の PATH には本物の claude を置かない）
CLAUDE_BIN="$(command -v claude 2>/dev/null || true)"

TMP="$(mktemp -d "${TMPDIR:-/tmp}/tako-1960-llm-XXXXXX")"
TMP="$(cd "$TMP" && pwd -P)"
cleanup() { rm -rf "$TMP"; }
# shellcheck source=lib/exit-guard.sh
. "$REPO_ROOT/scripts/lib/exit-guard.sh"
tako_exit_trap cleanup "test-mod-ui-llm"

cargo build -q -p tako-cli || exit 1
TAKO_BIN="$REPO_ROOT/target/debug/tako"
mkdir -p "$TMP/bin" "$TMP/h" "$TMP/cc"
ln -s "$TAKO_BIN" "$TMP/bin/tako"
cat > "$TMP/env" <<ENV
HOME=$TMP/h
CLAUDE_CONFIG_DIR=$TMP/cc
TAKO_ISOLATED=1
TAKO_LANG=ja
PATH=$TMP/bin:/usr/bin:/bin:/usr/sbin:/sbin
LANG=ja_JP.UTF-8
TERM=dumb
ENV
DRIVER="$REPO_ROOT/scripts/lib/mod-ui-llm.py"
FAIL=0
run_mode() { # run_mode <mode> [追加の引数...]
  local mode="$1"
  shift
  rm -rf "$TMP/work"
  mkdir -p "$TMP/work"
  /usr/bin/python3 -I "$DRIVER" --tako "$TAKO_BIN" --env-file "$TMP/env" --work "$TMP/work" \
    --mode "$mode" ${1+"$@"}
}

echo "== 採点器の検証: 正解の口（oracle）は 20 / 20 一致・違反 0 =="
OUT="$(run_mode oracle)"
printf '%s\n' "$OUT" | sed -e "s#${TMP}#<TMP>#g"
case "$OUT" in
  *"RESULT mode=oracle model=- cases=20 matched=20 violations=0"*) echo "  [OK] oracle" ;;
  *) echo "  [NG] oracle が 20 / 20 にならない（判定か正解の口が壊れている）"; FAIL=1 ;;
esac
echo
echo "== 検出力: 何もしない（null）なら一致は「何も変えない」が正解の 1 件だけ =="
OUT="$(run_mode null)"
printf '%s\n' "$OUT" | grep '^RESULT'
case "$OUT" in
  *"cases=20 matched=1 violations=0"*) echo "  [OK] null" ;;
  *) echo "  [NG] null の一致が 1 件ではない（判定が既定値で通ってしまう依頼がある）"; FAIL=1 ;;
esac

if [ "$WITH_CLAUDE" -eq 1 ]; then
  echo
  if [ -z "${ANTHROPIC_API_KEY:-}" ] || [ -z "$CLAUDE_BIN" ]; then
    echo "未実測: claude の段には ANTHROPIC_API_KEY と claude CLI が要る（本物の設定 dir の認証は写さない）"
    exit 4
  fi
  for model in $MODELS; do
    echo "== claude -p --model ${model}（道具は Bash(tako mod ui:*) だけ） =="
    run_mode claude --claude "$CLAUDE_BIN" --model "$model" | sed -e "s#${TMP}#<TMP>#g"
  done
fi

if [ "$FAIL" -eq 0 ]; then tako_exit 0; fi
exit 1
