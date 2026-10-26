#!/usr/bin/env bash
# test-check-health-mod-1891.sh — #1891: `tako check-health` の tako mod の項目の実経路テスト
#
# 隔離した data / discovery で**実 tako-app** を立て、次を実測する（番号は Issue #1891 の受け入れ条件）:
#   1. 報告のあるペイン / 無いペイン（`--safe-mode` / 版の下限未満 / `tako mod off`。ほかに注入前の claude と
#      注入は届いているのに報告が無い claude）それぞれで、`check-health` の mod の行が `tako mod` の出力に
#      **同じ字面で**在る
#   2. `--json` の `claude_mod` 節が `tako mod --json` と同じキー・同じ値。CLI と MCP（`tako_check_health` /
#      `tako_mod`）の応答が 1:1
#   3. GUI に繋がらない（止めた・SIGSTOP で固めた）ときも check-health の他の項目は出て、mod の項目は理由つきの不明
#   エッジ: 調べる上限（8 ペイン）を超えた分は調べず `over_limit` に数える / A/B（`TAKO_1891_LEGACY=1`）は
#          理由が候補の羅列のまま・check-health に節が無い（= 修正前）
#
# **本物の claude は起こさない**: 偽の claude を `cc` で作る（`claude --version` に版を返し、ふだんは
# claude の入力欄に似た画面を下端に出して止まるだけ。API も設定 dir も触らない）。シェルスクリプトにしないのは、
# Apple のプラットフォームバイナリ（`/bin/bash`）の起動時の env は他のプロセスから読めない（#1993 の実測）ため。
# 版は置き場（`…/versions/2.1.294`）で決まる（GUI は PATH の claude の実体のパスから版を読む）。
#
# **本番の tako にも利用者のエージェント CLI にも触らない**: data / discovery は mktemp 配下。GUI と CLI は
# 砂場（一時 HOME・偽の claude / codex / agy だけが居る PATH・ログインシェルにしない SHELL = #1505 の
# スクリプトと同じ作り）で動かす = 環境診断（`check-health` の `diagnostics`）が問い合わせるのも偽物だけ。
# 落とすのは自分で起こした pid だけ（偽の claude は起動時に自分の pid を書く）。
# 窓は常設の仮想ディスプレイ（tako-vd）へ出る（#1141）。描画は要らない（画面の判定は端末の格子を読む）。
#
# 選んで回す: ONLY=main|old|offline|limit|legacy（既定は全部）。CI には登録しない（実 GUI が要る）。
# CI 側の担保は単体テストと番犬（`crates/tako-control/tests/issue1891_check_health_mod_watchdog.rs`）。
#
# 使い方: bash scripts/test-check-health-mod-1891.sh
set -uo pipefail

# **本番 GUI を指す env を最初に落とす**（#1449 / #1450 で本番にペインが漏れた実例が 2 件）
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_ORCHESTRATOR_ROLE TAKO_MCP_URL \
  CLAUDE_CODE_PLUGIN_DIRS TAKO_CLI TAKO_1877_NO_MOD TAKO_1891_LEGACY TAKO_IPC_TIMEOUT_SECS
unset CLAUDE_CODE_CHILD_SESSION CLAUDE_CODE_SESSION_ID CLAUDECODE

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ONLY="${ONLY:-}"
PASS=0
FAIL=0
UNMEASURED=0

pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
fail() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }
unmeasured() { UNMEASURED=$((UNMEASURED + 1)); echo "  [未実測] $1"; }
check_eq() {
  if [ "$2" = "$3" ]; then pass "$1"; else fail "$1（期待 '${2}' / 実際 '${3}'）"; fi
}
want() { [ -z "$ONLY" ] || [ "$ONLY" = "$1" ]; }

TMP="$(mktemp -d /tmp/tako-1891-XXXXXX)"
APP_PID=""
FAKE_PIDS="$TMP/pids"

# 偽の claude を落とす（起動時に自分で書いた pid だけ）。GUI が起こした問い合わせは SIGTERM を
# 無視する設定を継いでいることがある（実測）ので SIGKILL で落とす
kill_fakes() {
  local f
  for f in "$FAKE_PIDS"/*; do
    [ -f "$f" ] || continue
    kill -KILL "${f##*/}" >/dev/null 2>&1 || true
    rm -f "$f"
  done
}
cleanup() {
  if [ -n "$APP_PID" ]; then kill -CONT "$APP_PID" >/dev/null 2>&1 || true; fi
  kill_fakes
  stop_isolated_gui "$APP_PID"
  if [ -n "${KEEP:-}" ]; then echo "（KEEP: $TMP を残した）"; else rm -rf "$TMP"; fi
}
# shellcheck source=lib/exit-guard.sh
. "$REPO_ROOT/scripts/lib/exit-guard.sh"
tako_exit_trap cleanup "check-health の mod の項目の実経路テスト"

# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
isolated_gui_bins || exit 1
caffeinate -d -u -w $$ >/dev/null 2>&1 &

# --- 偽の claude ----------------------------------------------------------------
if ! command -v cc >/dev/null 2>&1; then
  unmeasured "cc が無いので偽の claude を作れない（Xcode の Command Line Tools が要る）"
  exit 1
fi
mkdir -p "$TMP/claude/versions" "$TMP/newbin" "$TMP/oldbin" "$TMP/stubs" "$FAKE_PIDS" "$TMP/home"
cat > "$TMP/fake-claude.c" <<'C'
/* 偽の claude（#1891 の実測用）。API を呼ばず、設定 dir も読まない */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/ioctl.h>
#include <unistd.h>
int main(int argc, char **argv) {
  if (argc > 1 && strcmp(argv[1], "--version") == 0) {
    printf("%s (Claude Code)\n", FAKE_VERSION);
    return 0;
  }
  if (argc > 1 && strcmp(argv[1], "tako-fake-exec-warm") == 0) return 0;
  /* サブコマンド（GUI の診断の `auth status --json` / `mcp list`・監視の `agents --json` 等）と
     `-p` にはすぐ答えて抜ける（対話の画面で止まると、上限の無い問い合わせが診断ごと固まる） */
  if (argc > 1 && (argv[1][0] != '-' || strcmp(argv[1], "-p") == 0 ||
                   strcmp(argv[1], "--print") == 0)) {
    if (strcmp(argv[1], "auth") == 0) printf("{\"loggedIn\":false}\n");
    if (strcmp(argv[1], "agents") == 0) printf("[]\n");
    return 0;
  }
  const char *dir = getenv("FAKE_PID_DIR");
  if (dir) {
    char path[1024];
    snprintf(path, sizeof path, "%s/%d", dir, (int)getpid());
    FILE *f = fopen(path, "w");
    if (f) fclose(f);
  }
  /* claude の入力欄に似た画面を下端へ寄せて出す（tako の画面の判定 = claude_tui::detect が拾う形） */
  struct winsize ws;
  int rows = (ioctl(1, TIOCGWINSZ, &ws) == 0 && ws.ws_row > 0) ? ws.ws_row : 24;
  printf("\033[2J\033[H");
  for (int i = 0; i < rows - 6; i++) printf("\n");
  printf("* fake claude " FAKE_VERSION "\n\n----------------\n\xe2\x9d\xaf \n----------------");
  fflush(stdout);
  for (;;) pause();
}
C
cc -O1 -DFAKE_VERSION='"2.1.294"' -o "$TMP/claude/versions/2.1.294" "$TMP/fake-claude.c" || exit 1
cc -O1 -DFAKE_VERSION='"2.1.280"' -o "$TMP/claude/versions/2.1.280" "$TMP/fake-claude.c" || exit 1
ln -s ../claude/versions/2.1.294 "$TMP/newbin/claude"
ln -s ../claude/versions/2.1.280 "$TMP/oldbin/claude"
# 作りたての実行ファイルの初回 exec の検査を上限の外で済ませる（#2222 と同じ考え）
"$TMP/claude/versions/2.1.294" tako-fake-exec-warm
"$TMP/claude/versions/2.1.280" tako-fake-exec-warm
NEW_CLAUDE="$TMP/newbin/claude"
OLD_CLAUDE="$TMP/oldbin/claude"
# 診断が問い合わせるほかの CLI の偽物と、ログインシェルにしない SHELL（#1505 の砂場と同じ）
cat > "$TMP/stubs/codex" <<'SH'
#!/bin/sh
case "$*" in
  "login status") echo "Not logged in"; exit 1 ;;
  "mcp list"*) echo '[]' ;;
esac
exit 0
SH
cat > "$TMP/stubs/agy" <<'SH'
#!/bin/sh
case "$*" in
  "mcp list"*) echo '{}' ;;
esac
exit 0
SH
printf '#!/bin/sh\nif [ "$1" = "-l" ]; then shift; fi\nexec /bin/sh "$@"\n' > "$TMP/stubs/isosh"
chmod +x "$TMP/stubs/codex" "$TMP/stubs/agy" "$TMP/stubs/isosh"
"$TMP/stubs/codex" >/dev/null 2>&1
"$TMP/stubs/agy" >/dev/null 2>&1
SANDBOX_PATH="$TMP/stubs:/usr/bin:/bin:/usr/sbin:/sbin"
# 砂場の env（GUI と CLI で同じ。PATH の先頭は段ごとに偽の claude の版を置く）
sandbox_env() {
  echo "HOME=$TMP/home" "SHELL=$TMP/stubs/isosh" "CODEX_HOME=$TMP/home/.codex" "TAKO_ISOLATED=1"
}
# ビルドした CLI（砂場の env で。PATH は対応する版の偽の claude）
tk() {
  # shellcheck disable=SC2046
  env $(sandbox_env) PATH="$TMP/newbin:$SANDBOX_PATH" "$TAKO_BIN" "$@"
}

# --- 隔離 GUI -------------------------------------------------------------------
# 段ごとに data / discovery を分ける（同じ dir を使い回すと前の段のレイアウトを拾う = #2157）
start_app() {
  local name="$1"
  shift
  export TAKO_DATA_DIR="$TMP/$name/data"
  export TAKO_DISCOVERY_DIR="$TMP/$name/disc"
  export TAKO_ORCHESTRATOR_DIR="$TMP/$name/orch"
  export TAKO_SESSIONS_FILE="$TMP/$name/sessions.yaml"
  export TAKO_PANE_LOG_DIR="$TMP/$name/panelogs"
  export TAKO_WORKERS_FILE="$TMP/$name/workers.yaml"
  # `tako mod --json` の理由は表示言語・check-health は日本語（#2365）。同じ字面で比べるので
  # GUI と CLI を日本語表示に固定する（OS のロケールに依らない）
  export TAKO_LANG=ja
  mkdir -p "$TAKO_DATA_DIR" "$TAKO_DISCOVERY_DIR" "$TAKO_ORCHESTRATOR_DIR"
  # shellcheck disable=SC2046
  launch_isolated_gui "$TMP/app-${name}.log" $(sandbox_env) TAKO_PERSIST=0 \
    FAKE_PID_DIR="$FAKE_PIDS" ${@+"$@"} || exit $?
  APP_PID="$ISOLATED_GUI_PID"
  wait_isolated_gui "$TMP/app-${name}.log" || exit 1
}
stop_app() {
  kill_fakes
  stop_isolated_gui "$APP_PID"
  APP_PID=""
}

# --- 観測の道具 -----------------------------------------------------------------
# JSON の 1 項目（ドット区切り。数字はリストの添字・`pane=N` はペインの行）
jf() {
  python3 -I -c '
import json, sys
path = [k for k in sys.argv[1].split(".") if k]
try:
    v = json.load(sys.stdin)
except Exception:
    print("absent"); sys.exit()
for k in path:
    if isinstance(v, list) and k.startswith("pane="):
        v = next((r for r in v if r.get("pane") == int(k[5:])), None)
    elif isinstance(v, dict):
        v = v.get(k)
    elif isinstance(v, list) and k.isdigit() and int(k) < len(v):
        v = v[int(k)]
    else:
        v = None
    if v is None:
        break
print("null" if v is None else (json.dumps(v, ensure_ascii=False) if isinstance(v, (dict, list)) else str(v).lower() if isinstance(v, bool) else v))' "$1"
}
mod_f() { tk mod --json 2>/dev/null | jf "$1"; }
health_f() { tk check-health --json 2>/dev/null | jf "$1"; }
mcp_call() {
  local sock="$TAKO_DATA_DIR/tako.sock"
  [ -f "$TAKO_DATA_DIR/tako.sock.path" ] && sock="$(cat "$TAKO_DATA_DIR/tako.sock.path")"
  printf '%s\n' \
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"t1891","version":"0"}}}' \
    "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"$1\",\"arguments\":$2}}" \
    | TAKO_SOCKET="$sock" TAKO_TOKEN="$(cat "$TAKO_DATA_DIR/token")" tk mcp serve 2>/dev/null \
    | python3 -I -c '
import json, sys
for line in sys.stdin:
    try:
        msg = json.loads(line)
    except Exception:
        continue
    if msg.get("id") == 2:
        for c in msg.get("result", {}).get("content", []):
            if c.get("type") == "text":
                print(c["text"])'
}
wait_for() {
  local limit="$1" i
  shift
  for i in $(seq 1 "$limit"); do
    if "$@"; then return 0; fi
    sleep 1
  done
  return 1
}
row_is() { [ "$(mod_f "panes.pane=$1.$2")" = "$3" ]; }
root_pane() {
  tk list 2>/dev/null | python3 -I -c '
import json, sys
print(json.load(sys.stdin)["tabs"][0]["panes"][0]["id"])'
}
split_new() { tk split --pane "$1" "--$2" 2>/dev/null | tr -dc '0-9'; }
new_tab_pane() {
  tk tab new 2>/dev/null >/dev/null
  tk list 2>/dev/null | python3 -I -c '
import json, sys
print(json.load(sys.stdin)["tabs"][-1]["panes"][0]["id"])'
}
# ペインのシェルに 1 行打たせる（clear してから偽の claude を前面で動かす）
run_in() { tk send --pane "$1" "clear; $2" >/dev/null 2>&1; }
# 疑似の報告（mod と同じ口 = `tako mod report` の stdin。送り主は TAKO_PANE_ID。設定 dir は載せない =
# GUI が skills/tako を置きに行かない）
send_report() {
  printf '%s' '{"schema":1,"mod_version":"t1891","claude_version":"2.1.294","session_id":"s-1891","at":1,"model":"claude-haiku-5-5","effort":"medium","context":{"tokens":10,"window":100,"percent":10},"rate_limits":[],"turn":"idle","classic_events":true,"ended":false}' \
    | TAKO_PANE_ID="$1" tk mod report >/dev/null 2>&1
}

# check-health の mod の行（`tako mod:` の行から指摘の `[` の手前まで）が、すべて `tako mod` の出力に
# 同じ字面で在るか。足りない行を出す（空なら一致）
lines_missing() {
  python3 -I -c '
import sys
health = open(sys.argv[1], encoding="utf-8").read().splitlines()
mod = set(open(sys.argv[2], encoding="utf-8").read().splitlines())
section, inside = [], False
for line in health:
    if line.startswith("tako mod:"):
        inside = True
    elif inside and line.startswith("["):
        break
    if inside:
        section.append(line)
if not section:
    print("（check-health に mod の行が無い）")
for line in section:
    if line not in mod:
        print(line)' "$1" "$2"
}
# check-health の mod の行に、指定の字面の行が在るか
health_has_line() {
  python3 -I -c '
import sys
lines = open(sys.argv[1], encoding="utf-8").read().splitlines()
sys.exit(0 if sys.argv[2] in lines else 1)' "$1" "$2"
}
# `--json` の `claude_mod` 節が `tako mod --json` と同じキー・同じ値か（行は check-health のキーだけを比べる）
json_diff() {
  python3 -I -c '
import json, sys
health = json.load(open(sys.argv[1]))["claude_mod"]
status = json.load(open(sys.argv[2]))
keys = ["enabled", "injecting", "reason", "claude_version", "min_claude_version", "summary", "no_report_probe", "skills"]
out = []
for k in keys:
    if health.get(k) != status.get(k):
        out.append(f"{k}: {health.get(k)!r} != {status.get(k)!r}")
rows = {r["pane"]: r for r in status["panes"]}
have = health["panes"]
if len(have) != len(rows):
    out.append("panes の数 %d != %d" % (len(have), len(rows)))
for r in have:
    pane = r["pane"]
    full = rows.get(pane, {})
    for k, v in r.items():
        if full.get(k) != v:
            out.append("pane %s.%s: %r != %r" % (pane, k, v, full.get(k)))
    if "report" in r:
        out.append("pane %s に報告の中身が載っている" % pane)
print("\n".join(out))' "$1" "$2"
}
# 2 つの JSON 文書の同じ道の値が等しいか
same_json() {
  python3 -I -c '
import json, sys
a = json.load(open(sys.argv[1]))
b = json.load(open(sys.argv[2]))
for k in sys.argv[3].split("."):
    a = a.get(k) if isinstance(a, dict) else None
    b = b.get(k) if isinstance(b, dict) else None
sys.exit(0 if a == b and a is not None else 1)' "$1" "$2" "$3"
}

# 3 つの写し（tako mod / check-health の人が読む形と JSON）を同じ瞬間に近く採り、字面とキーを比べる
compare_now() {
  local label="$1"
  tk mod > "$TMP/mod.txt" 2>&1
  tk check-health > "$TMP/health.txt" 2>&1
  tk mod --json > "$TMP/mod.json" 2>/dev/null
  tk check-health --json > "$TMP/health.json" 2>/dev/null
  local missing diff
  missing="$(lines_missing "$TMP/health.txt" "$TMP/mod.txt")"
  if [ -z "$missing" ]; then
    pass "$label: check-health の mod の行はすべて tako mod と同じ字面（$(awk '/^tako mod:/{f=1} f && /^\[/{exit} f' "$TMP/health.txt" | wc -l | tr -d ' ') 行）"
  else
    fail "$label: tako mod に無い行が check-health にある: $missing"
  fi
  diff="$(json_diff "$TMP/health.json" "$TMP/mod.json")"
  if [ -z "$diff" ]; then
    pass "$label: --json の claude_mod 節は tako mod --json と同じキー・同じ値"
  else
    fail "$label: --json の食い違い: $diff"
  fi
}

# --- 1 / 2: 報告のあるペイン・無いペイン（--safe-mode / 注入前 / 注入は届いている）・tako mod off ---------
phase_main() {
  echo "== main: 対応する版（2.1.294）の偽の claude で 5 種類のペイン"
  start_app main "PATH=$TMP/newbin:$SANDBOX_PATH"
  check_eq "GUI が偽の claude の版を控えた" "2.1.294" "$(mod_f claude_version)"
  check_eq "新しいペインへ注入する" "true" "$(mod_f injecting)"
  local p1 p2 p3 p4 p5
  p1="$(root_pane)"
  p2="$(split_new "$p1" right)"
  p3="$(split_new "$p1" down)"
  p4="$(split_new "$p2" down)"
  if [ -z "$p2" ] || [ -z "$p3" ] || [ -z "$p4" ]; then
    fail "ペインを分割できない（${p1} / ${p2} / ${p3} / ${p4}）"
    stop_app
    return
  fi
  run_in "$p1" "'$NEW_CLAUDE' --safe-mode"
  run_in "$p2" "env -u CLAUDE_CODE_PLUGIN_DIRS -u TAKO_CLI '$NEW_CLAUDE'"
  run_in "$p3" "'$NEW_CLAUDE' --model haiku"
  run_in "$p4" "'$NEW_CLAUDE'"
  if wait_for 30 row_is "$p1" reason.cause safe_mode; then
    pass "--safe-mode の claude: no_report・理由は safe_mode（1 つ）"
  else
    fail "--safe-mode の行（state=$(mod_f "panes.pane=$p1.state") cause=$(mod_f "panes.pane=$p1.reason.cause")）"
  fi
  wait_for 15 row_is "$p2" reason.cause before_injection \
    && pass "起動時の env に注入の印が無い claude: 理由は before_injection" \
    || fail "注入前の claude の行（cause=$(mod_f "panes.pane=$p2.reason.cause")）"
  wait_for 15 row_is "$p4" reason.cause not_loaded \
    && pass "注入は届いているのに報告の無い claude: 理由は not_loaded" \
    || fail "not_loaded の行（cause=$(mod_f "panes.pane=$p4.reason.cause")）"
  wait_for 15 row_is "$p3" state no_report || true
  send_report "$p3"
  wait_for 10 row_is "$p3" state reporting \
    && pass "報告のあるペイン: reporting" \
    || fail "報告のあるペインが reporting にならない（$(mod_f "panes.pane=$p3.state")）"
  check_eq "調べたペインの数（報告の無い 3 つ。上限 8）" "3" "$(mod_f no_report_probe.panes)"

  send_report "$p3"
  compare_now "main"
  health_has_line "$TMP/health.txt" "  pane $p1  no_report  $(mod_f "panes.pane=$p1.reason.message")" \
    && pass "check-health に --safe-mode のペインの理由の行がある" \
    || fail "check-health に --safe-mode のペインの理由の行が無い"
  if grep -q "pane $p3 " "$TMP/health.txt"; then
    fail "報告のあるペインが check-health の理由の行に出た"
  else
    pass "報告のあるペインは check-health の理由の行に出ない（tako mod では reporting の行）"
  fi
  check_eq "指摘: 報告の無いペインが 3 個" "true" "$(python3 -I -c '
import json, sys
issues = json.load(open(sys.argv[1]))["issues"]
print(str(any(i["check"] == "claude_mod_no_report" and "3 個" in i["message"] for i in issues)).lower())' "$TMP/health.json")"
  check_eq "--safe-mode の行の理由は判別した 1 つ（候補の羅列ではない）" "false" \
    "$( [ "$(mod_f "panes.pane=$p1.reason.message")" = "$(mod_f "panes.pane=$p2.reason.message")" ] && echo true || echo false)"

  # MCP と CLI の 1:1
  send_report "$p3"
  mcp_call tako_check_health '{}' > "$TMP/mcp-health.json"
  tk check-health --json > "$TMP/health.json" 2>/dev/null
  same_json "$TMP/mcp-health.json" "$TMP/health.json" claude_mod \
    && pass "MCP tako_check_health の claude_mod 節は CLI の --json と同じ" \
    || fail "MCP tako_check_health の claude_mod 節が CLI と違う"
  mcp_call tako_mod '{"action":"status"}' > "$TMP/mcp-mod.json"
  tk mod --json > "$TMP/mod.json" 2>/dev/null
  local key differ=""
  for key in injecting summary no_report_probe skills; do
    same_json "$TMP/mcp-mod.json" "$TMP/mod.json" "$key" || differ="$differ $key"
  done
  if [ -z "$differ" ]; then
    pass "MCP tako_mod（status）と CLI の --json が同じ（injecting / summary / no_report_probe / skills）"
  else
    fail "MCP tako_mod と CLI の --json が違う:$differ"
  fi
  check_eq "MCP tako_mod の --safe-mode の行の理由" "safe_mode" \
    "$(jf "panes.pane=$p1.reason.cause" < "$TMP/mcp-mod.json")"

  # tako mod off: 次に作るペインは注入されない（理由は disabled）
  tk mod off >/dev/null 2>&1
  p5="$(split_new "$p3" right)"
  run_in "$p5" "'$NEW_CLAUDE'"
  wait_for 15 row_is "$p5" state not_injected \
    && pass "tako mod off のペイン: not_injected（理由 $(mod_f "panes.pane=$p5.reason.code")）" \
    || fail "tako mod off のペインの行（$(mod_f "panes.pane=$p5.state")）"
  send_report "$p3"
  compare_now "mod off"
  health_has_line "$TMP/health.txt" "$(python3 -I -c '
import json, sys
print("tako mod: 注入しない（" + json.load(open(sys.argv[1]))["reason"]["message"] + "）")' "$TMP/mod.json")" \
    && pass "mod off の 1 行目（注入しない理由）が check-health にも同じ字面で出る" \
    || fail "mod off の 1 行目が check-health に無い"
  check_eq "利用者の選択（off）は指摘に出さない" "false" "$(python3 -I -c '
import json, sys
issues = json.load(open(sys.argv[1]))["issues"]
print(str(any(i["check"] == "claude_mod" for i in issues)).lower())' "$TMP/health.json")"
  tk mod on >/dev/null 2>&1

  # 3: 固まった GUI（自分の pid を SIGSTOP）でも他の項目は出る
  echo "== offline: 固まった GUI（SIGSTOP）"
  kill -STOP "$APP_PID"
  local t0 t1 t2
  t0="$(date +%s)"
  TAKO_IPC_TIMEOUT_SECS=5 tk check-health --json > "$TMP/frozen.json" 2>/dev/null
  t1="$(date +%s)"
  TAKO_IPC_TIMEOUT_SECS=5 tk check-health > "$TMP/frozen.txt" 2>/dev/null
  t2="$(date +%s)"
  kill -CONT "$APP_PID"
  check_eq "固まった GUI: mod の項目は不明（available=false）" "false" "$(jf claude_mod.available < "$TMP/frozen.json")"
  case "$(jf claude_mod.reason.message < "$TMP/frozen.json")" in
    "tako アプリへ届かない（"*) pass "固まった GUI: 不明の理由が付く（$(jf claude_mod.reason.message < "$TMP/frozen.json" | cut -c1-60)…）" ;;
    *) fail "固まった GUI: 不明の理由が無い" ;;
  esac
  check_eq "固まった GUI: 環境診断の項目は出る" "true" \
    "$(python3 -I -c 'import json,sys; d=json.load(open(sys.argv[1])); print(str(len(d["diagnostics"]["items"]) > 0 and d["ipc"] is not None).lower())' "$TMP/frozen.json")"
  grep -q '^tako mod: 不明（tako アプリへ届かない' "$TMP/frozen.txt" \
    && pass "固まった GUI: 人が読む形にも「tako mod: 不明（理由）」" \
    || fail "固まった GUI: 人が読む形に mod の不明の行が無い"
  # 応答の上限 5 秒 + 手元の環境診断（偽の CLI だけ）。mod の項目のせいで待ちが延びないこと
  if [ $((t1 - t0)) -le 15 ] && [ $((t2 - t1)) -le 15 ]; then
    pass "固まった GUI: check-health は応答の上限で打ち切って返る（$((t1 - t0)) 秒 / $((t2 - t1)) 秒。上限 5 秒）"
  else
    fail "固まった GUI: check-health が遅い（$((t1 - t0)) 秒 / $((t2 - t1)) 秒）"
  fi
  stop_app
}

# --- 1: 版の下限未満 ----------------------------------------------------------------
phase_old() {
  echo "== old: 版の下限未満（2.1.280）の偽の claude"
  start_app old "PATH=$TMP/oldbin:$SANDBOX_PATH"
  check_eq "下限未満の版を控えた" "2.1.280" "$(mod_f claude_version)"
  check_eq "下限未満では注入しない（理由 claude_too_old）" "claude_too_old" "$(mod_f reason.code)"
  local p
  p="$(root_pane)"
  run_in "$p" "'$OLD_CLAUDE'"
  wait_for 15 row_is "$p" reason.code claude_too_old \
    && pass "下限未満のペイン: not_injected・理由 claude_too_old" \
    || fail "下限未満のペインの行（$(mod_f "panes.pane=$p.state") / $(mod_f "panes.pane=$p.reason.code")）"
  compare_now "old"
  health_has_line "$TMP/health.txt" "  pane $p  not_injected  $(mod_f "panes.pane=$p.reason.message")" \
    && pass "check-health に下限未満のペインの理由の行がある" \
    || fail "check-health に下限未満のペインの理由の行が無い"
  check_eq "直す手がある「注入しない」は指摘に出る（check=claude_mod）" "true" "$(python3 -I -c '
import json, sys
issues = json.load(open(sys.argv[1]))["issues"]
print(str(any(i["check"] == "claude_mod" and "2.1.280" in i["message"] for i in issues)).lower())' "$TMP/health.json")"
  stop_app
}

# --- 3: GUI が居ない ----------------------------------------------------------------
phase_offline() {
  echo "== offline: GUI が居ない"
  export TAKO_DATA_DIR="$TMP/none/data"
  export TAKO_DISCOVERY_DIR="$TMP/none/disc"
  mkdir -p "$TAKO_DATA_DIR" "$TAKO_DISCOVERY_DIR"
  tk check-health --json > "$TMP/none.json" 2>/dev/null
  local rc=$?
  tk check-health > "$TMP/none.txt" 2>/dev/null
  check_eq "GUI が居ない: 終了コードは非 0" "1" "$rc"
  check_eq "GUI が居ない: connected=false" "false" "$(jf connected < "$TMP/none.json")"
  check_eq "GUI が居ない: mod の項目は不明" "false" "$(jf claude_mod.available < "$TMP/none.json")"
  check_eq "GUI が居ない: 環境診断と IPC の項目は出る" "true" \
    "$(python3 -I -c 'import json,sys; d=json.load(open(sys.argv[1])); print(str(len(d["diagnostics"]["items"]) > 0 and d["ipc"] is not None).lower())' "$TMP/none.json")"
  grep -q '^tako mod: 不明（tako アプリへ届かない' "$TMP/none.txt" \
    && pass "GUI が居ない: 人が読む形にも「tako mod: 不明（理由）」" \
    || fail "GUI が居ない: 人が読む形に mod の不明の行が無い"
}

# --- エッジ: 調べる上限 -------------------------------------------------------------
phase_limit() {
  echo "== limit: 報告の無い claude が上限（8）を超える"
  start_app limit "PATH=$TMP/newbin:$SANDBOX_PATH"
  local i p panes=""
  for i in 1 2 3 4 5 6 7 8 9; do
    if [ "$i" = 1 ]; then p="$(root_pane)"; else p="$(new_tab_pane)"; fi
    [ -n "$p" ] || { fail "タブを作れない（$i 本目）"; stop_app; return; }
    run_in "$p" "'$NEW_CLAUDE' --safe-mode"
    panes="$panes $p"
  done
  local last="${p}"
  wait_for 30 row_is "$last" state no_report || true
  check_eq "上限の外の数（9 ペインのうち 1）" "1" "$(mod_f no_report_probe.over_limit)"
  check_eq "調べたのは上限まで" "8" "$(mod_f no_report_probe.panes)"
  check_eq "上限の外のペインは調べない（undetermined）" "undetermined" "$(mod_f "panes.pane=$last.reason.cause")"
  local first="${panes# }"
  first="${first%% *}"
  check_eq "上限の内のペインは判別する" "safe_mode" "$(mod_f "panes.pane=$first.reason.cause")"
  compare_now "limit"
  stop_app
}

# --- エッジ: A/B（修正前） -----------------------------------------------------------
phase_legacy() {
  echo "== legacy: TAKO_1891_LEGACY=1（修正前 = 調べない・check-health に節が無い）"
  start_app legacy "PATH=$TMP/newbin:$SANDBOX_PATH" TAKO_1891_LEGACY=1
  local p
  p="$(root_pane)"
  run_in "$p" "'$NEW_CLAUDE' --safe-mode"
  wait_for 30 row_is "$p" state no_report || true
  check_eq "A/B: 理由は候補の羅列のまま" "undetermined" "$(mod_f "panes.pane=$p.reason.cause")"
  check_eq "A/B: 調べていない" "false" "$(mod_f no_report_probe.ran)"
  check_eq "A/B: check-health に mod の節が無い" "null" "$(health_f claude_mod)"
  tk check-health > "$TMP/legacy.txt" 2>&1
  if grep -q '^tako mod:' "$TMP/legacy.txt"; then
    fail "A/B: check-health に mod の行が出た"
  else
    pass "A/B: check-health に mod の行が無い（修正前の形）"
  fi
  stop_app
}

want main && phase_main
want old && phase_old
want offline && phase_offline
want limit && phase_limit
want legacy && phase_legacy

echo
echo "結果: PASS $PASS / FAIL $FAIL / 未実測 $UNMEASURED"
[ "$FAIL" -eq 0 ] || exit 1
tako_exit 0
