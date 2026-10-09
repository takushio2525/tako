#!/usr/bin/env bash
# #1940: spawn の起動コマンドが「行数の少ないペイン + 準備の遅いシェル」で化けないこと、
# 起動に失敗した spawn が `workers` に理由つきで出ることを、隔離した tako-app
# （実ウィンドウ・tmux の器は隔離ソケット）+ 実 CLI で測る。
#
# 本番（10/9）の型 2: worker ペインが 2 行まで潰れていると zsh は入力行を 1 行の横スクロールに
# 畳むので、起動コマンドの全文が画面に出ない。旧実装は書き直しを使い切った後、消していない行へ
# 本文 + Enter を書き足して `--permission-mode autoexport` に化けた。
#
# 腕（同じバイナリ）:
#   new    … 修正後（既定）
#   legacy … TAKO_1940_LEGACY=1（旧挙動。化けが再現することを確かめる対照）
#
# claude は偽物（受け取った引数を記録し、不正な `--permission-mode` なら本物と同じ文言で落ちる）
# なので実エージェントは起動しない。シェルは検証用の zsh（ZDOTDIR を一時の置き場へ向ける =
# ユーザーの rc・履歴は読まない / 書かない）で、direnv の実物を一時の XDG で使う
# （ユーザーの許可リストに触れない）。本番の tako（GUI / 設定 / tmux / projects / workers）にも触らない。
#
# 使い方: scripts/test-spawn-launch-1940.sh            # 両腕
#         ARMS=new SLOW=10 scripts/test-spawn-launch-1940.sh
set -uo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
ARMS="${ARMS:-new legacy}"
# .envrc の sleep 秒（シェルの準備の遅さ）
SLOW="${SLOW:-3}"
# 窓の寸法（低くして worker ペインを 2〜3 行へ寄せる。本番 10/9 の形）
BOUNDS="${BOUNDS:-0,0,1400,150}"

PASS=0
FAIL=0
pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
bad()  { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }
check() { if [ "$1" = "1" ]; then pass "$2"; else bad "$2"; fi; }

for bin in tmux zsh direnv python3; do
  command -v "$bin" >/dev/null 2>&1 || { echo "未実測: ${bin} が無い"; exit 4; }
done
isolated_gui_bins || exit 1

# ペインの中から走らせても本番へ届かないように、継承した接続情報を落とす
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_MCP_URL TMUX

TMP_ROOTS=""
SOCKETS=""
cleanup() {
  [ -n "${ISOLATED_GUI_PID:-}" ] && stop_isolated_gui "$ISOLATED_GUI_PID"
  local s d
  for s in $SOCKETS; do tmux -L "$s" kill-server >/dev/null 2>&1; done
  # KEEP=1 なら置き場を残す（persist.log / perf.log / app.log を後から読むため）
  if [ -n "${KEEP:-}" ]; then
    echo "置き場を残した:${TMP_ROOTS}"
  else
    for d in $TMP_ROOTS; do rm -rf "$d"; done
  fi
}
trap cleanup EXIT

pane_rows() {
  "$TAKO_BIN" list 2>/dev/null | python3 -c '
import json, sys
want = int(sys.argv[1])
d = json.load(sys.stdin)
for tab in d.get("tabs", []):
    for p in tab.get("panes", []):
        if p.get("id") == want:
            print(p.get("cols"), p.get("rows"))
' "$1"
}

worker_field() {
  # worker_field <pane> <field>
  "$TAKO_BIN" orchestrator workers 2>/dev/null | python3 -c '
import json, sys
pane, field = int(sys.argv[1]), sys.argv[2]
for w in json.load(sys.stdin).get("workers", []):
    if w.get("pane") == pane:
        v = w.get(field)
        print("null" if v is None else v)
        break
' "$1" "$2"
}

# 偽 claude が記録した起動の行数（記録ファイルが無ければ 0）
launch_count() {
  if [ -f "$1" ]; then wc -l < "$1" | tr -d ' '; else echo 0; fi
}

run_arm() {
  local arm="$1" TMP sock master resp pane i n rows
  TMP="$(mktemp -d /tmp/t1940-XXXXXX)"
  TMP_ROOTS="$TMP_ROOTS $TMP"
  sock="t1940-${arm}-$$"
  SOCKETS="$SOCKETS $sock"
  echo
  echo "=== 腕: ${arm}（.envrc の準備 ${SLOW} 秒・窓 ${BOUNDS}）==="

  mkdir -p "$TMP/bin" "$TMP/work" "$TMP/zdot" "$TMP/xd" "$TMP/xc"
  # 偽 claude: 引数を 1 行ずつ記録。fail の印があれば本物と同じく即終了（起動失敗の再現）
  # GUI 自身の問い合わせ（`--version` / `agents --json`）には答えるだけで記録しない
  cat > "$TMP/bin/claude" <<STUB
#!/bin/sh
case "\$1" in
  --version) echo "2.1.294 (Claude Code)"; exit 0 ;;
  agents) echo "[]"; exit 0 ;;
  -p) exit 0 ;;
esac
printf '%s\n' "\$*" >> "$TMP/launches.log"
[ -f "$TMP/fail" ] && { echo "error: simulated launch failure"; exit 1; }
mode=""; prev=""
for a in "\$@"; do
  if [ "\$prev" = "--permission-mode" ]; then mode="\$a"; fi
  prev="\$a"
done
case "\$mode" in
  auto|default|plan|acceptEdits|bypassPermissions) echo "FAKECLAUDE-STARTED"; exec sleep 600 ;;
  *) echo "error: option '--permission-mode <mode>' argument '\$mode' is invalid."; exit 1 ;;
esac
STUB
  chmod +x "$TMP/bin/claude"
  # 検証用の zsh: 本番と同じ形のプロンプト + direnv の hook だけ（ユーザーの rc は読まない）
  printf '%s\n' 'PS1="[test@host:%1~]\$ "' 'eval "$(direnv hook zsh)"' > "$TMP/zdot/.zshrc"
  printf 'sleep %s\nexport CLAUDE_CONFIG_DIR="/tmp/fake-config"\n' "$SLOW" > "$TMP/work/.envrc"
  XDG_DATA_HOME="$TMP/xd" XDG_CONFIG_HOME="$TMP/xc" direnv allow "$TMP/work" >/dev/null 2>&1

  export TAKO_DATA_DIR="$TMP/data"
  export TAKO_DISCOVERY_DIR="$TMP/disc"
  export TAKO_ORCHESTRATOR_DIR="$TMP/orch"
  export TAKO_SESSIONS_FILE="$TMP/sessions.yaml"
  export TAKO_PANE_LOG_DIR="$TMP/panelogs"
  export TAKO_WORKERS_FILE="$TMP/workers.yaml"
  export TAKO_TMUX_SOCKET="$sock"
  mkdir -p "$TAKO_DATA_DIR" "$TAKO_DISCOVERY_DIR" "$TAKO_ORCHESTRATOR_DIR/profiles"
  for d in "$TAKO_DATA_DIR" "$TAKO_ORCHESTRATOR_DIR" "$TAKO_DISCOVERY_DIR"; do
    case "$d" in "$TMP"/*) : ;; *) echo "隔離されていない: $d"; exit 1 ;; esac
  done
  # 本番と同じ形・同じくらいの長さ（10/9 は 204 桁）の起動コマンドにする
  # （export の前置き + --model + --remote-control + --permission-mode auto）。
  # 短いと direnv 中の kernel のエコーが 2 行に収まり、旧挙動でも全文一致が成立してしまう
  cat > "$TAKO_ORCHESTRATOR_DIR/profiles/default.yaml" <<YAML
effort: xhigh
env:
  PATH: "$TMP/bin:/usr/bin:/bin:/opt/homebrew/bin"
worker_agents:
  claude:
    model: claude-opus-5-5
    args: ["--remote-control", "--permission-mode", "auto"]
YAML
  "$TAKO_BIN" orchestrator projects add --key t1940 --cwd "$TMP/work" \
    --description "#1940 の検証（自動削除される）" >/dev/null 2>&1

  local legacy_env="TAKO_1940_LEGACY="
  [ "$arm" = legacy ] && legacy_env="TAKO_1940_LEGACY=1"
  launch_isolated_gui "$TMP/app.log" "$legacy_env" "TAKO_PERSIST=1" "TAKO_FLOW_DIAG=1" \
    "TAKO_WINDOW_BOUNDS=$BOUNDS" "SHELL=/bin/zsh" "ZDOTDIR=$TMP/zdot" \
    "XDG_DATA_HOME=$TMP/xd" "XDG_CONFIG_HOME=$TMP/xc" "PATH=$TMP/bin:$PATH" || exit $?
  if ! wait_isolated_gui "$TMP/app.log"; then
    stop_isolated_gui "$ISOLATED_GUI_PID"; bad "隔離 GUI が起動しない"; return
  fi
  pass "隔離 tako-app が起動して CLI から見える（pid ${ISOLATED_GUI_PID}・tmux -L ${sock}）"
  sleep 3
  master="$("$TAKO_BIN" list | python3 -c 'import json,sys; print(json.load(sys.stdin)["tabs"][0]["panes"][0]["id"])')"

  # ---- 1. 正常な起動: 1 回だけ・正しい引数 ----
  resp="$("$TAKO_BIN" orchestrator spawn --project t1940 --prompt "#1940 の検証" --pane "$master" --label ok 2>&1)"
  pane="$(printf '%s' "$resp" | python3 -c 'import json,sys; print(json.load(sys.stdin)["pane_id"])' 2>/dev/null)"
  if [ -z "$pane" ]; then
    bad "spawn が通らない: $(printf '%s' "$resp" | head -c 300)"
    stop_isolated_gui "$ISOLATED_GUI_PID"; return
  fi
  read -r cols rows < <(pane_rows "$pane")
  echo "  worker pane=${pane} の寸法: ${cols} 桁 × ${rows} 行"
  # 起動の記録を待つ（旧挙動は書き直し 10 回 ≒ 45 秒かかる）
  for i in $(seq 1 150); do
    [ "$(launch_count "$TMP/launches.log")" -ge 1 ] && break
    sleep 0.5
  done
  sleep 3
  n="$(launch_count "$TMP/launches.log")"
  echo "  起動の記録（${n} 回）:"
  [ -f "$TMP/launches.log" ] && sed 's/^/    /' "$TMP/launches.log"
  if [ "$arm" = new ]; then
    check "$([ "$n" = 1 ] && echo 1 || echo 0)" "起動先は 1 回だけ起動する（実測 ${n} 回）"
    check "$(grep -c -- '--permission-mode auto$' "$TMP/launches.log" 2>/dev/null | grep -qx 1 && echo 1 || echo 0)" \
      "正しい引数（--permission-mode auto）で起動する"
    check "$(grep -q 'autoexport' "$TMP/launches.log" 2>/dev/null && echo 0 || echo 1)" \
      "化けた引数（autoexport）が無い"
  else
    check "$(grep -q 'autoexport' "$TMP/launches.log" 2>/dev/null && echo 1 || echo 0)" \
      "旧挙動で化け（autoexport）が再現する（対照）"
  fi
  echo "  workers: launch=$(worker_field "$pane" launch) prompt_delivery=$(worker_field "$pane" prompt_delivery)"
  if [ "$arm" = new ]; then
    check "$([ "$(worker_field "$pane" prompt_delivery)" != delivered ] && echo 1 || echo 0)" \
      "依頼文が届いていない（偽 claude）のに delivered と言わない"
  fi

  # ---- 2. 起動の失敗: workers に理由つきで出る（新しい腕だけ）----
  if [ "$arm" = new ]; then
    : > "$TMP/fail"
    resp="$("$TAKO_BIN" orchestrator spawn --project t1940 --prompt "#1940 の失敗検証" --pane "$master" --label ng 2>&1)"
    pane="$(printf '%s' "$resp" | python3 -c 'import json,sys; print(json.load(sys.stdin)["pane_id"])' 2>/dev/null)"
    local state=""
    for i in $(seq 1 120); do
      state="$(worker_field "$pane" launch)"
      [ "$state" = failed ] && break
      sleep 0.5
    done
    echo "  失敗した spawn pane=${pane}: launch=${state} launch_failure=$(worker_field "$pane" launch_failure)" \
      "launch_exit_code=$(worker_field "$pane" launch_exit_code)" \
      "prompt_delivery=$(worker_field "$pane" prompt_delivery) 内訳=$(worker_field "$pane" prompt_delivery_failure)" \
      "resend_command=$(worker_field "$pane" resend_command)"
    if [ "$state" != failed ] && [ -n "${DEBUG_1940:-}" ]; then
      "$TAKO_BIN" list | python3 -c '
import json, sys
want = int(sys.argv[1])
for t in json.load(sys.stdin)["tabs"]:
    for p in t["panes"]:
        if p["id"] == want:
            print("  [debug] state=", p.get("state"), "exit=", p.get("exit_code"), "scroll=", p.get("scroll"))
' "$pane"
      "$TAKO_BIN" read --pane "$pane" 2>/dev/null | tail -5 | sed 's/^/  [debug] /'
    fi
    check "$([ "$state" = failed ] && echo 1 || echo 0)" "起動の失敗が 60 秒以内に workers の launch=failed へ出る"
    check "$([ "$(worker_field "$pane" launch_failure)" = agent_exited ] && echo 1 || echo 0)" \
      "理由コードは agent_exited"
    check "$([ "$(worker_field "$pane" launch_exit_code)" = 1 ] && echo 1 || echo 0)" "終了コード 1 が載る"
    check "$([ "$(worker_field "$pane" resend_command)" = null ] && echo 1 || echo 0)" \
      "起動に失敗した worker へ依頼文の再送を案内しない"
    grep -q "起動失敗: pane=${pane} 理由=agent_exited" "$TAKO_DATA_DIR/persist.log" 2>/dev/null
    check "$([ $? -eq 0 ] && echo 1 || echo 0)" "persist.log に起動失敗の 1 行が残る"
    rm -f "$TMP/fail"
  fi

  stop_isolated_gui "$ISOLATED_GUI_PID"
  tmux -L "$sock" kill-server >/dev/null 2>&1
}

for arm in $ARMS; do
  run_arm "$arm"
done

echo
echo "PASS ${PASS} / FAIL ${FAIL}"
[ "$FAIL" -eq 0 ]
