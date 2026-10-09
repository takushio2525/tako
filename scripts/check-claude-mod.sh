#!/usr/bin/env bash
# check-claude-mod.sh — tako mod（Claude Code の mod。crates/tako-core/claude-mod）を
# `claude plugin validate --strict` / `claude plugin test` にかける（Issue #1892）
#
# 使い方:
#   scripts/check-claude-mod.sh                    # 作業ツリーの mod を検査する
#   scripts/check-claude-mod.sh --ref origin/main  # その ref の mod を検査する（夜間リリースはこれ）
#   scripts/check-claude-mod.sh --dir <path>       # 任意の dir の mod を検査する（壊した写しを試す）
#   scripts/check-claude-mod.sh --last             # 記録した直近の結果を出す
#   --record を足すと結果を状態ファイルへ記録する（夜間リリースが付ける）
#
# 背景: mod の API は early access で、Claude Code の更新で mod が読まれなくなっても tako は
# 画面の読み取りへ落ちるだけで壊れはしない（設計書 .agent/plans/2026-10-tako-mod.md §6）。
# 気づけないと「いつの間にか一次ソースが消えていた」になるので、夜間リリース
# （scripts/nightly-release.sh）が毎晩これを回し、落ちたら通知する。
#
# 終了コード: 0 = 合格 / 1 = 不合格（validate・test の失敗、上限での打ち切り、mod を取り出せない）/
#             2 = 引数の誤り / 3 = 未実測（claude が無い。#1879 の受け入れ 8 と同じ扱い）
#
# 利用者の Claude Code の設定には触らない:
#   - 設定 dir（CLAUDE_CONFIG_DIR）は毎回作る使い捨て。claude が書く .claude.json もそこへ入る
#   - mod は一時 dir へ写してから検査する（validate / test が書く型定義をリポジトリへ落とさない）
#   - claude の作業 dir も一時 dir（プロジェクトの .claude/ を読ませない）
#   - 自動更新と不要な通信を止める（検査の途中で claude の実体が入れ替わらない）
#
# 上限: 各段（--version / validate / test）は TAKO_MOD_CHECK_TIMEOUT 秒（既定 60。実測は
# 1 段 1 秒前後）で打ち切り、プロセスグループごと止める（test は子プロセスでテストを走らせる）。
# 前の段で打ち切ったら後ろの段は走らせない。
#
# 文言一致の自己検査（#1903）: validate / test は終了コード 0 のまま問題を注記に出すことがあるので、
# 注記の文言でも落とす（`.catch` 抜け・テスト 0 本）。文言が Claude Code の更新で変わると
# 否定形の一致は黙って外れるので、**肯定形の行が出ていること**（validate の
# `gating hook with .catch:` = この mod のゲートになるフック・test の要約 `Ran N test(s)`）も
# 要求し、出ていなければ「文言が変わった」で落とす（追加の claude の呼び出しは要らない）。
#
# 記録（--record）: ~/.claude-orchestrator/state/tako-mod-check（夜間リリースの予約と同じ置き場）。
# 検査した claude の版・mod の出どころと、最後に合格した claude の版・mod の木を持つ。
# 不合格のときは前回の合格と比べて「Claude Code の更新で壊れた」「mod の変更で壊れた」を出し分ける。
#
# 使う claude は TAKO_CLAUDE_BIN（自動リネームと同じ名前・同じ意味）、無ければ PATH の claude。
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MOD_PATH="crates/tako-core/claude-mod"
TIMEOUT="${TAKO_MOD_CHECK_TIMEOUT:-60}"
STATE_FILE="${HOME}/.claude-orchestrator/state/tako-mod-check"

MODE="worktree"
SOURCE_ARG=""
RECORD=0

usage() {
  sed -n '5,10p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
}

while [ $# -gt 0 ]; do
  case "$1" in
    --ref | --dir)
      if [ "$MODE" != worktree ]; then
        echo "エラー: --ref と --dir は 1 つだけ指定する" >&2
        exit 2
      fi
      if [ $# -lt 2 ] || [ -z "$2" ]; then
        echo "エラー: $1 に値が無い" >&2
        exit 2
      fi
      MODE="${1#--}"
      SOURCE_ARG="$2"
      shift 2
      ;;
    --record) RECORD=1; shift ;;
    --last)
      if [ -f "$STATE_FILE" ]; then cat "$STATE_FILE"; else echo "記録なし（${STATE_FILE}）"; fi
      exit 0
      ;;
    -h | --help) usage; exit 0 ;;
    *)
      echo "不明な引数: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

case "$TIMEOUT" in
  '' | *[!0-9]* | 0)
    echo "エラー: TAKO_MOD_CHECK_TIMEOUT は 1 以上の整数（秒）: ${TIMEOUT}" >&2
    exit 2
    ;;
esac
if [ "$MODE" = dir ] && [ ! -d "$SOURCE_ARG" ]; then
  echo "エラー: --dir の dir が無い: ${SOURCE_ARG}" >&2
  exit 2
fi

# --- 後始末 -------------------------------------------------------------------
WORK=""
JOB_PID=""

kill_job() {
  [ -n "$JOB_PID" ] || return 0
  { kill -TERM -- "-$JOB_PID" || true; sleep 1; kill -KILL -- "-$JOB_PID" || true; wait "$JOB_PID" || true; } 2>/dev/null
  JOB_PID=""
}

cleanup() {
  kill_job
  if [ -n "$WORK" ]; then rm -rf "$WORK"; fi
}
# shellcheck source=lib/exit-guard.sh
. "$REPO_ROOT/scripts/lib/exit-guard.sh"
tako_exit_trap cleanup "tako mod の検査"
trap 'exit 130' INT TERM

WORK="$(mktemp -d "${TMPDIR:-/tmp}/tako-mod-check-XXXXXX")"
# TMPDIR の末尾の / で `//` が混ざると、claude が出す正規化したパスと一致せず伏せられない
WORK="$(cd "$WORK" && pwd)"
WORK_REAL="$(cd "$WORK" && pwd -P)"
mkdir -p "$WORK/config" "$WORK/cwd"

# 出力に出るパスを短くする（一時 dir とホームを伏せる。ログに実ユーザー名を残さない）
mask() {
  sed -e "s#${WORK}/mod#<mod>#g" -e "s#${WORK_REAL}/mod#<mod>#g" \
    -e "s#${WORK}#<work>#g" -e "s#${WORK_REAL}#<work>#g" -e "s#${HOME}#~#g"
}

# run_step <秒> <出力ファイル> <コマンド...>
#   上限つきで走らせる。打ち切ったら 124。set -m で自分のプロセスグループに入れるので、
#   claude が起こした子（test のテストランナー）までまとめて止められる（launchd の
#   制御端末なしでも効くことを実測済み）
run_step() {
  local secs="$1" out="$2" i=0 rc=0
  shift 2
  set -m
  (cd "$WORK/cwd" && "$@") < /dev/null > "$out" 2>&1 &
  JOB_PID=$!
  set +m
  while kill -0 "$JOB_PID" 2>/dev/null; do
    if [ "$i" -ge $((secs * 5)) ]; then
      kill_job
      return 124
    fi
    sleep 0.2
    i=$((i + 1))
  done
  wait "$JOB_PID" || rc=$?
  JOB_PID=""
  return "$rc"
}

# 利用者の設定から切り離した claude（設定 dir は使い捨て。tako のペインの印も渡さない）
isolated_claude() {
  env -u CLAUDECODE -u CLAUDE_CODE_SESSION_ID -u CLAUDE_CODE_CHILD_SESSION \
    -u CLAUDE_CODE_ENTRYPOINT -u CLAUDE_CODE_PLUGIN_DIRS \
    -u TAKO_PANE_ID -u TAKO_CLI -u TAKO_SOCKET -u TAKO_TOKEN \
    CLAUDE_CONFIG_DIR="$WORK/config" DISABLE_AUTOUPDATER=1 CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC=1 \
    "$CLAUDE" "$@"
}

state_get() {
  [ -f "$STATE_FILE" ] || return 0
  sed -n "s/^$1=//p" "$STATE_FILE" | tail -1
}

# --- 結果の組み立てと記録 ---------------------------------------------------------
CLAUDE=""
CLAUDE_VERSION=""
MOD_SOURCE=""
MOD_TREE=""
VALIDATE="skipped"
TEST="skipped"
PREV_PASS_VERSION="$(state_get last_pass_claude_version)"
PREV_PASS_TREE="$(state_get last_pass_mod_tree)"
PREV_PASS_AT="$(state_get last_pass_at)"

# 不合格のとき、前回の合格と比べて何が変わったかを 1 句で言う
cause_hint() {
  [ -n "$PREV_PASS_VERSION" ] || return 0
  local tree_changed=unknown
  if [ -n "$MOD_TREE" ] && [ -n "$PREV_PASS_TREE" ]; then
    if [ "$MOD_TREE" = "$PREV_PASS_TREE" ]; then tree_changed=no; else tree_changed=yes; fi
  fi
  if [ "$CLAUDE_VERSION" != "$PREV_PASS_VERSION" ]; then
    if [ "$tree_changed" = yes ]; then
      printf '%s' "前回合格は claude ${PREV_PASS_VERSION}（${PREV_PASS_AT}）。Claude Code の更新と mod の変更が重なった"
    else
      printf '%s' "前回合格は claude ${PREV_PASS_VERSION}（${PREV_PASS_AT}）= Claude Code の更新で壊れた可能性"
    fi
  elif [ "$tree_changed" = yes ]; then
    printf '%s' "claude は前回合格と同じ ${CLAUDE_VERSION} = mod の変更で壊れた可能性"
  elif [ "$tree_changed" = no ]; then
    printf '%s' "前回合格と同じ claude・同じ mod（検査環境の揺れの可能性）"
  fi
}

# finish <result: pass|fail|unmeasured> <結果の本文>
finish() {
  local result="$1" summary="$2" now pass_at pass_ver pass_tree tmp
  echo "結果: ${summary}"
  if [ "$RECORD" = 1 ]; then
    now="$(date '+%Y-%m-%dT%H:%M:%S%z')"
    pass_at="$PREV_PASS_AT"
    pass_ver="$PREV_PASS_VERSION"
    pass_tree="$PREV_PASS_TREE"
    if [ "$result" = pass ]; then
      pass_at="$now"
      pass_ver="$CLAUDE_VERSION"
      pass_tree="$MOD_TREE"
    fi
    mkdir -p "$(dirname "$STATE_FILE")"
    tmp="${STATE_FILE}.tmp.$$"
    {
      echo "# tako mod の検査（scripts/check-claude-mod.sh --record。Issue #1892）の直近の結果。手で書き換えない"
      echo "checked_at=${now}"
      echo "result=${result}"
      echo "claude_version=${CLAUDE_VERSION}"
      echo "claude_path=$(printf '%s' "$CLAUDE" | mask)"
      echo "mod_source=${MOD_SOURCE}"
      echo "mod_tree=${MOD_TREE}"
      echo "validate=${VALIDATE}"
      echo "test=${TEST}"
      echo "summary=${summary}"
      echo "last_pass_at=${pass_at}"
      echo "last_pass_claude_version=${pass_ver}"
      echo "last_pass_mod_tree=${pass_tree}"
    } > "$tmp"
    mv -f "$tmp" "$STATE_FILE"
  fi
  case "$result" in
    pass) tako_exit 0 ;;
    unmeasured) exit 3 ;;
    *) exit 1 ;;
  esac
}

# 失敗した段の出力の末尾を字下げして出す
show_tail() {
  mask < "$1" | grep -v '^[[:space:]]*$' | tail -n "${2:-20}" | sed 's/^/    /' || true
}

# --- 1. claude を探す（無ければ未実測）---------------------------------------------
if [ -n "${TAKO_CLAUDE_BIN:-}" ]; then
  if [ -x "$TAKO_CLAUDE_BIN" ] && [ ! -d "$TAKO_CLAUDE_BIN" ]; then CLAUDE="$TAKO_CLAUDE_BIN"; fi
  WHERE="TAKO_CLAUDE_BIN=$(printf '%s' "$TAKO_CLAUDE_BIN" | mask)"
else
  CLAUDE="$(command -v claude 2>/dev/null || true)"
  WHERE="PATH に claude が無い"
fi

# --- 2. mod を一時 dir へ取り出す ---------------------------------------------------
case "$MODE" in
  ref)
    if ! git -C "$REPO_ROOT" rev-parse --verify --quiet "${SOURCE_ARG}^{commit}" > /dev/null; then
      MOD_SOURCE="$SOURCE_ARG"
      echo "tako mod の検査: ${SOURCE_ARG} が解決できない"
      finish fail "不合格 — ${SOURCE_ARG} が解決できない（mod を取り出せない）"
    fi
    MOD_SOURCE="${SOURCE_ARG}@$(git -C "$REPO_ROOT" rev-parse --short "${SOURCE_ARG}^{commit}")"
    if ! MOD_TREE="$(git -C "$REPO_ROOT" rev-parse --verify --quiet "${SOURCE_ARG}:${MOD_PATH}")"; then
      MOD_TREE=""
      echo "tako mod の検査: ${MOD_SOURCE} に ${MOD_PATH} が無い"
      finish fail "不合格 — ${MOD_SOURCE} に ${MOD_PATH} が無い（置き場が変わったならこのスクリプトの MOD_PATH を直す）"
    fi
    mkdir -p "$WORK/mod"
    if ! git -C "$REPO_ROOT" archive --format=tar "$MOD_TREE" | tar -x -C "$WORK/mod"; then
      echo "tako mod の検査: ${MOD_SOURCE} の mod を取り出せない（git archive / tar が失敗）"
      finish fail "不合格 — ${MOD_SOURCE} の mod を取り出せない"
    fi
    ;;
  dir)
    MOD_SOURCE="$(cd "$SOURCE_ARG" && pwd | mask)"
    cp -R "$SOURCE_ARG" "$WORK/mod"
    ;;
  *)
    MOD_SOURCE="作業ツリー（HEAD $(git -C "$REPO_ROOT" rev-parse --short HEAD 2>/dev/null || echo '?')）"
    if [ ! -d "$REPO_ROOT/$MOD_PATH" ]; then
      echo "tako mod の検査: ${REPO_ROOT}/${MOD_PATH} が無い" | mask
      finish fail "不合格 — 作業ツリーに ${MOD_PATH} が無い"
    fi
    cp -R "$REPO_ROOT/$MOD_PATH" "$WORK/mod"
    ;;
esac

if [ -z "$CLAUDE" ]; then
  echo "tako mod の検査: mod = ${MOD_SOURCE}"
  echo "  claude: 見つからない（${WHERE}）"
  finish unmeasured "未実測 — claude が無いので validate / test を飛ばした（${WHERE}）"
fi

# --- 3. 版を控える ------------------------------------------------------------------
rc=0
run_step "$TIMEOUT" "$WORK/version.out" isolated_claude --version || rc=$?
if [ "$rc" -eq 124 ]; then
  CLAUDE_VERSION="不明"
  echo "tako mod の検査: mod = ${MOD_SOURCE}"
  echo "  claude --version: ${TIMEOUT} 秒で返らないので打ち切った（validate / test は飛ばした）"
  finish fail "不合格 — claude --version が ${TIMEOUT} 秒で返らない（打ち切り）"
fi
CLAUDE_VERSION="$(grep -Eo '[0-9]+\.[0-9]+\.[0-9]+' "$WORK/version.out" | head -1 || true)"
if [ "$rc" -ne 0 ] || [ -z "$CLAUDE_VERSION" ]; then
  CLAUDE_VERSION="${CLAUDE_VERSION:-不明}"
  echo "tako mod の検査: mod = ${MOD_SOURCE}"
  echo "  claude --version: 失敗（exit ${rc}）"
  show_tail "$WORK/version.out" 5
  finish fail "不合格 — claude --version が失敗した（exit ${rc}。claude 自体が壊れている）"
fi
echo "tako mod の検査: claude ${CLAUDE_VERSION}（$(printf '%s' "$CLAUDE" | mask)）/ mod = ${MOD_SOURCE}"

# --- 4. validate --strict -----------------------------------------------------------
# --strict は警告も落とす（Claude Code が非推奨にした書き方を、読まれなくなる前に拾う）。
# ゲートになるフックの .catch 抜け（validate は注記に出すだけで合格させる）は設計書 §5 の
# 規約違反（mod が壊れると tool call と権限ダイアログを止める）なので不合格にする
FAILED=""
rc=0
run_step "$TIMEOUT" "$WORK/validate.out" isolated_claude plugin validate --strict "$WORK/mod" || rc=$?
if [ "$rc" -eq 124 ]; then
  VALIDATE="timeout"
  echo "  validate --strict: ${TIMEOUT} 秒で返らないので打ち切った（test は飛ばした）"
  finish fail "不合格 — claude ${CLAUDE_VERSION} の validate が ${TIMEOUT} 秒で返らない（打ち切り）"
elif [ "$rc" -ne 0 ]; then
  VALIDATE="fail"
  FAILED="validate"
  echo "  validate --strict: 不合格（exit ${rc}）"
  show_tail "$WORK/validate.out"
elif grep -q 'gating hook without .catch' "$WORK/validate.out"; then
  VALIDATE="fail"
  FAILED="validate"
  echo "  validate --strict: 不合格（ゲートになるフックに .catch の無いものがある = 設計書 §5）"
  grep 'gating hook without .catch' "$WORK/validate.out" | mask | sed 's/^/    /'
elif ! grep -q 'gating hook with .catch: ' "$WORK/validate.out"; then
  # この mod にはゲートになるフック（tool.call 等）が必ずあるので、肯定形の行が 1 つも無いのは
  # 文言が変わった（= 上の .catch 抜けの一致が黙って外れている）しるし
  VALIDATE="fail"
  FAILED="validate（注記の文言が変わった）"
  echo "  validate --strict: 不合格（注記に『gating hook with .catch:』の行が無い = Claude Code の出力の"
  echo "    文言が変わり、.catch 抜けの検出が効いていない可能性。scripts/check-claude-mod.sh の文言一致を直す）"
  grep -i 'gating\|catch' "$WORK/validate.out" | mask | sed 's/^/    /' || true
else
  VALIDATE="pass"
  echo "  validate --strict: 合格"
fi

# --- 5. test ------------------------------------------------------------------------
rc=0
run_step "$TIMEOUT" "$WORK/test.out" isolated_claude plugin test "$WORK/mod" || rc=$?
COUNTS="$(grep -Eo '^ *[0-9]+ (pass|fail)' "$WORK/test.out" | tr -s ' ' | sed 's/^ //' | paste -sd '/' - || true)"
# 要約行 `Ran N tests across M files.`（1 本なら `Ran 1 test`）の N。読めなければ空 = 文言が変わった
RAN="$(grep -Eo '^Ran [0-9]+ test' "$WORK/test.out" | grep -Eo '[0-9]+' | head -1 || true)"
if [ "$rc" -eq 124 ]; then
  TEST="timeout"
  FAILED="${FAILED:+${FAILED}・}test（${TIMEOUT} 秒で打ち切り）"
  echo "  test: ${TIMEOUT} 秒で返らないので打ち切った"
elif [ "$rc" -ne 0 ]; then
  TEST="fail"
  FAILED="${FAILED:+${FAILED}・}test"
  echo "  test: 不合格（exit ${rc}${COUNTS:+・${COUNTS}}）"
  show_tail "$WORK/test.out"
elif [ -z "$RAN" ]; then
  TEST="fail"
  FAILED="${FAILED:+${FAILED}・}test（要約行の文言が変わった）"
  echo "  test: 不合格（要約行『Ran N tests』が読めない = Claude Code の出力の文言が変わり、"
  echo "    0 本の検出が効いていない可能性。scripts/check-claude-mod.sh の文言一致を直す）"
  show_tail "$WORK/test.out" 5
elif [ "$RAN" -eq 0 ]; then
  TEST="fail"
  FAILED="${FAILED:+${FAILED}・}test（1 本も走らなかった）"
  echo "  test: 不合格（テストが 1 本も走らなかった）"
else
  TEST="pass"
  echo "  test: 合格${COUNTS:+（${COUNTS}）}"
fi

if [ -z "$FAILED" ]; then
  finish pass "合格 — claude ${CLAUDE_VERSION}（validate --strict・test${COUNTS:+ ${COUNTS}}）"
fi
HINT="$(cause_hint)"
finish fail "不合格 — claude ${CLAUDE_VERSION} で ${FAILED} が落ちた${HINT:+。${HINT}}"
