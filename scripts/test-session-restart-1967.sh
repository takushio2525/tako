#!/usr/bin/env bash
# #1967: 「セッションを引き継いで再起動」の後に会話が戻らない、を隔離した tako-app
# （仮想ディスプレイ `tako-vd`・tmux の器は隔離ソケット）+ 実 CLI で測る。
#
# 本番（10/9）の minige の master（35 桁のペイン）で起きたこと:
#   ① 再開した claude が終了時に出す `Resume this session with: claude --resume <id>` が
#      細いペインで**折り返し**、tako はその 1 行目（途中までの ID）を権威として差し替えた
#      → `--resume …-b897-065b9` が実行され、claude は会話が見つからずに終わった
#   ② 再開コマンドはカタログのメタだけで組まれ、model / effort / system prompt が落ちていた
#   ③ 送った後を誰も見ておらず、persist.log にも記録が無かった
#
# 腕（同じバイナリ）:
#   new    … 修正後（既定）
#   legacy … TAKO_1967_LEGACY=1 + TAKO_1940_LEGACY=1（旧挙動。①② が再現することを確かめる対照）
#
# claude は偽物（C の小さな実行ファイル。`claude` という名前のプロセスとして見え、受け取った
# 引数と role / 設定 dir を記録する。SIGTERM で本物と同じ再開の案内を出し、`--resume` の会話の
# 記録が設定 dir に無ければ本物と同じ文言で終わる）。実エージェントは起動しないので本番の
# 設定 dir に会話を書かない。シェルは検証用の zsh（ZDOTDIR を一時の置き場へ向ける =
# ユーザーの rc・履歴は読まない / 書かない）。本番の tako（GUI / 設定 / tmux / sessions /
# workers）にも触らない。
#
# 使い方: scripts/test-session-restart-1967.sh             # 両腕 × 両方の形
#         ARMS=new SHAPES=tall scripts/test-session-restart-1967.sh
set -uo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
ARMS="${ARMS:-new legacy}"
# ペインの形（窓の寸法）。本番の minige は 35 桁 × 43 行（細く高い）で、#1940 の worker は
# 2〜3 行（細く低い）だった。形ごとに旧挙動の壊れ方が違う:
#   short … 40 桁前後 × 2 行。旧挙動は起動コマンドを書き直し続けて化ける（#1940 型）
#   tall  … 40 桁前後 × 40 行前後。旧挙動は折り返した案内の途中までの ID で resume する（本番の ②）
# short では全項目、tall ではハーネス更新（項目 1）だけを測る
SHAPES="${SHAPES:-short tall}"
shape_bounds() {
  case "$1" in
    short) echo "0,0,340,170" ;;
    tall) echo "0,0,340,820" ;;
  esac
}
# 偽の会話 ID（UUID の形。本番と同じ長さ = 35 桁で案内が折り返す）
SID="1967aaaa-b67d-44ac-b897-065b9f0678f0"
WSID="1967bbbb-b67d-44ac-b897-065b9f0678f0"

PASS=0
FAIL=0
pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
bad()  { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }
check() { if [ "$1" = "1" ]; then pass "$2"; else bad "$2"; fi; }

for bin in tmux zsh python3 cc; do
  command -v "$bin" >/dev/null 2>&1 || { echo "未実測: ${bin} が無い"; exit 4; }
done
isolated_gui_bins || exit 1

# ペインの中から走らせても本番へ届かないように、継承した接続情報を落とす
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_MCP_URL TAKO_CLI TMUX \
  CLAUDE_CODE_PLUGIN_DIRS TAKO_ORCHESTRATOR_ROLE

TMP_ROOTS=""
SOCKETS=""
cleanup() {
  [ -n "${ISOLATED_GUI_PID:-}" ] && stop_isolated_gui "$ISOLATED_GUI_PID"
  local s d
  for s in $SOCKETS; do tmux -L "$s" kill-server >/dev/null 2>&1; done
  if [ -n "${KEEP:-}" ]; then
    echo "置き場を残した:${TMP_ROOTS}"
  else
    for d in $TMP_ROOTS; do rm -rf "$d"; done
  fi
}
trap cleanup EXIT

# ---- 小道具 ----

pane_dims() {
  "$TAKO_BIN" list 2>/dev/null | python3 -c '
import json, sys
want = int(sys.argv[1])
for tab in json.load(sys.stdin).get("tabs", []):
    for p in tab.get("panes", []):
        if p.get("id") == want:
            print(p.get("cols"), p.get("rows"))
' "$1"
}

# role が一致するペインの ID（新しいものを最後に）
panes_with_role() {
  "$TAKO_BIN" list 2>/dev/null | python3 -c '
import json, sys
want = sys.argv[1]
for tab in json.load(sys.stdin).get("tabs", []):
    for p in tab.get("panes", []):
        if p.get("role") == want:
            print(p.get("id"))
' "$1"
}

# `tako session-restart --pane N`（下見）の値。パスは a.b 形式
restart_field() {
  "$TAKO_BIN" session-restart --pane "$1" 2>/dev/null | python3 -c '
import json, sys
v = json.load(sys.stdin)
for k in sys.argv[1].split("."):
    v = v.get(k) if isinstance(v, dict) else None
print("null" if v is None else (json.dumps(v) if isinstance(v, (dict, list)) else v))
' "$2"
}

pane_state() {
  "$TAKO_BIN" list 2>/dev/null | python3 -c '
import json, sys
want = int(sys.argv[1])
for tab in json.load(sys.stdin).get("tabs", []):
    for p in tab.get("panes", []):
        if p.get("id") == want:
            print(p.get("state"))
' "$1"
}

json_field() {
  printf '%s' "$1" | python3 -c '
import json, sys
try:
    v = json.load(sys.stdin)
except Exception:
    print("<not-json>"); sys.exit()
for k in sys.argv[1].split("."):
    v = v.get(k) if isinstance(v, dict) else None
print("null" if v is None else v)
' "$2"
}

launch_count() {
  if [ -f "$1" ]; then wc -l < "$1" | tr -d ' '; else echo 0; fi
}

# 記録の n 行目（1 始まり）
launch_line() { sed -n "${2}p" "$1" 2>/dev/null; }

# 建て直しが決着する（phase = started / failed）まで待つ。決着した phase を出す
wait_restart_settled() {
  local pane="$1" i phase=""
  for i in $(seq 1 120); do
    phase="$(restart_field "$pane" last_restart.phase)"
    case "$phase" in started|failed) break ;; esac
    sleep 0.5
  done
  echo "$phase"
}

wait_launches() {
  local file="$1" want="$2" i
  for i in $(seq 1 120); do
    [ "$(launch_count "$file")" -ge "$want" ] && return 0
    sleep 0.5
  done
  return 1
}

build_fake_claude() {
  cat > "$1/fake_claude.c" <<'CSRC'
#include <glob.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

static char sid[256];

static void on_term(int s) {
  (void)s;
  char buf[512];
  int n = snprintf(buf, sizeof buf, "\nResume this session with:\nclaude --resume %s\n", sid);
  (void)!write(1, buf, (size_t)n);
  _exit(0);
}

int main(int argc, char **argv) {
  if (argc > 1 && !strcmp(argv[1], "--version")) { puts("2.1.294 (Claude Code)"); return 0; }
  if (argc > 1 && !strcmp(argv[1], "agents")) { puts("[]"); return 0; }
  if (argc > 1 && (!strcmp(argv[1], "-p") || !strcmp(argv[1], "mcp") ||
                   !strcmp(argv[1], "auth") || !strcmp(argv[1], "plugin"))) return 0;
  const char *log = getenv("FAKE_LOG");
  const char *role = getenv("TAKO_ORCHESTRATOR_ROLE");
  const char *cfg = getenv("CLAUDE_CONFIG_DIR");
  const char *resume = NULL;
  for (int i = 1; i < argc - 1; i++) if (!strcmp(argv[i], "--resume")) resume = argv[i + 1];
  FILE *f = log ? fopen(log, "a") : NULL;
  if (f) {
    fprintf(f, "ROLE=%s CFG=%s ARGS=", role ? role : "<unset>", cfg ? cfg : "<unset>");
    for (int i = 1; i < argc; i++) fprintf(f, "%s%s", i > 1 ? " " : "", argv[i]);
    fputc('\n', f);
    fclose(f);
  }
  /* 会話 ID は役割ごと（本物は自分の会話の ID を案内する。worker が master の ID を言わない） */
  const char *fsid = (role && !strncmp(role, "worker", 6)) ? getenv("FAKE_WORKER_SESSION_ID")
                                                           : getenv("FAKE_SESSION_ID");
  snprintf(sid, sizeof sid, "%s", resume ? resume : (fsid ? fsid : "none"));
  if (resume) {
    char base[1024], pat[1400];
    if (cfg) snprintf(base, sizeof base, "%s", cfg);
    else snprintf(base, sizeof base, "%s/.claude", getenv("HOME"));
    snprintf(pat, sizeof pat, "%s/projects/*/%s.jsonl", base, resume);
    glob_t g;
    int found = glob(pat, 0, NULL, &g) == 0 && g.gl_pathc > 0;
    globfree(&g);
    const char *failf = getenv("FAKE_FAIL_FILE");
    if (!found || (failf && access(failf, F_OK) == 0)) {
      printf("No conversation found with session ID: %s\n", resume);
      fflush(stdout);
      return 1;
    }
  }
  signal(SIGTERM, on_term);
  printf("FAKECLAUDE-STARTED\n");
  fflush(stdout);
  for (;;) pause();
}
CSRC
  cc -O0 -o "$1/claude" "$1/fake_claude.c"
}

# 起動の記録の ARGS 部分
args_of() { printf '%s' "$1" | sed 's/^.* ARGS=//'; }
env_of() { printf '%s' "$1" | sed 's/ ARGS=.*$//'; }

write_catalog() {
  # write_catalog <file> <id> <kind> <pane> [<extra yaml lines>]
  local file="$1" id="$2" kind="$3" pane="$4" extra="${5:-}"
  python3 - "$file" "$id" "$kind" "$pane" "$extra" <<'PY'
import os, sys
path, sid, kind, pane, extra = sys.argv[1:6]
body = ""
if os.path.exists(path):
    body = open(path).read()
entry = f"  {sid}:\n    kind: {kind}\n    agent: claude\n    pane: {pane}\n" \
        f"    started_at: 2026-10-09T00:00:00Z\n    last_seen_at: 2026-10-09T00:00:00Z\n"
for line in filter(None, extra.split("|")):
    entry += f"    {line}\n"
if "entries:" not in body:
    body = "entries:\n" + entry + body
else:
    body = body.replace("entries:\n", "entries:\n" + entry, 1)
open(path, "w").write(body)
PY
}

run_arm() {
  local arm="$1" shape="$2" TMP sock resp master cols rows n line1 line2 phase BOUNDS
  BOUNDS="$(shape_bounds "$shape")"
  TMP="$(mktemp -d /tmp/t1967-XXXXXX)"
  TMP_ROOTS="$TMP_ROOTS $TMP"
  sock="t1967-${arm}-${shape}-$$"
  SOCKETS="$SOCKETS $sock"
  echo
  echo "=== 腕: ${arm} / 形: ${shape}（窓 ${BOUNDS}）==="

  mkdir -p "$TMP/bin" "$TMP/cfg/projects/-tmp-st1967" "$TMP/work" "$TMP/zdot"
  build_fake_claude "$TMP/bin" || { bad "偽 claude をビルドできない"; return; }
  printf '{"type":"user","message":{"content":"#1967 の検証"}}\n' \
    > "$TMP/cfg/projects/-tmp-st1967/$SID.jsonl"
  printf '{"type":"user","message":{"content":"#1967 の worker"}}\n' \
    > "$TMP/cfg/projects/-tmp-st1967/$WSID.jsonl"
  # 検証用の zsh: 本番と同じ形のプロンプトだけ（ユーザーの rc は読まない）
  printf '%s\n' 'PS1="[testuser@host:%1~]\$ "' > "$TMP/zdot/.zshrc"

  export TAKO_DATA_DIR="$TMP/data"
  export TAKO_DISCOVERY_DIR="$TMP/disc"
  export TAKO_ORCHESTRATOR_DIR="$TMP/orch"
  export TAKO_SESSIONS_FILE="$TMP/sessions.yaml"
  export TAKO_PANE_LOG_DIR="$TMP/panelogs"
  export TAKO_WORKERS_FILE="$TMP/workers.yaml"
  export TAKO_TMUX_SOCKET="$sock"
  mkdir -p "$TAKO_DATA_DIR" "$TAKO_DISCOVERY_DIR" "$TAKO_ORCHESTRATOR_DIR/profiles" \
    "$TAKO_ORCHESTRATOR_DIR/handoff"
  for d in "$TAKO_DATA_DIR" "$TAKO_ORCHESTRATOR_DIR" "$TAKO_DISCOVERY_DIR"; do
    case "$d" in "$TMP"/*) : ;; *) echo "隔離されていない: $d"; exit 1 ;; esac
  done
  # 本番の minige と同じ形のプロファイル（model / effort / master_account）。
  # アカウントの設定 dir は一時の置き場（偽の会話の記録はここにだけある）
  cat > "$TAKO_ORCHESTRATOR_DIR/accounts.yaml" <<YAML
accounts:
  st1967acct:
    config_dir: $TMP/cfg
YAML
  cat > "$TAKO_ORCHESTRATOR_DIR/profiles/st1967.yaml" <<YAML
model: claude-opus-5-5
effort: xhigh
master_account: st1967acct
worker_agents:
  claude:
    model: claude-opus-5-5
    args: ["--permission-mode", "auto"]
YAML
  printf '## 決定事項\n- #1967 の検証（自動削除される）\n' \
    > "$TAKO_ORCHESTRATOR_DIR/handoff/st1967.md"
  PATH="$TMP/bin:$PATH" "$TAKO_BIN" orchestrator projects add --key st1967p --cwd "$TMP/work" \
    --description "#1967 の検証（自動削除される）" >/dev/null 2>&1

  local legacy_env="TAKO_1967_LEGACY=" legacy_1940="TAKO_1940_LEGACY="
  if [ "$arm" = legacy ]; then
    legacy_env="TAKO_1967_LEGACY=1"
    legacy_1940="TAKO_1940_LEGACY=1"
  fi
  launch_isolated_gui "$TMP/app.log" "$legacy_env" "$legacy_1940" "TAKO_PERSIST=1" \
    "TAKO_WINDOW_BOUNDS=$BOUNDS" "SHELL=/bin/zsh" "ZDOTDIR=$TMP/zdot" \
    "FAKE_LOG=$TMP/launches.log" "FAKE_SESSION_ID=$SID" "FAKE_WORKER_SESSION_ID=$WSID" \
    "FAKE_FAIL_FILE=$TMP/fail-resume" \
    "TAKO_AUTO_RENAME=0" "PATH=$TMP/bin:$PATH" || exit $?
  if ! wait_isolated_gui "$TMP/app.log"; then
    stop_isolated_gui "$ISOLATED_GUI_PID"; bad "隔離 GUI が起動しない"; return
  fi
  pass "隔離 tako-app が起動して CLI から見える（pid ${ISOLATED_GUI_PID}・tmux -L ${sock}）"
  sleep 3

  # ---- 元の起動: `tako master -st1967`（本番の minige と同じ組み立て）----
  PATH="$TMP/bin:$PATH" "$TAKO_BIN" master -st1967 >/dev/null 2>&1
  master=""
  for i in $(seq 1 40); do
    master="$(panes_with_role orchestrator-master:st1967 | tail -1)"
    [ -n "$master" ] && break
    sleep 0.5
  done
  if [ -z "$master" ] || ! wait_launches "$TMP/launches.log" 1; then
    bad "master が立たない（pane=${master:-なし}）"
    stop_isolated_gui "$ISOLATED_GUI_PID"; return
  fi
  sleep 2
  read -r cols rows < <(pane_dims "$master")
  echo "  master pane=${master} の寸法: ${cols} 桁 × ${rows} 行"
  if [ "$shape" = short ]; then
    check "$([ "${cols:-99}" -le 40 ] && [ "${rows:-99}" -le 3 ] && echo 1 || echo 0)" \
      "master のペインが細く低い（40 桁前後 × 2〜3 行 = #1940 の worker の形）"
  else
    check "$([ "${cols:-99}" -le 40 ] && [ "${rows:-0}" -ge 20 ] && echo 1 || echo 0)" \
      "master のペインが細く高い（40 桁前後 × 20 行以上 = 本番の minige の形）"
  fi
  write_catalog "$TAKO_SESSIONS_FILE" "$SID" master "$master" "profile: st1967|label: master-st1967"
  line1="$(launch_line "$TMP/launches.log" 1)"
  echo "  元の起動: $(env_of "$line1") ARGS=…$(args_of "$line1" | tail -c 90)"

  # ---- 1. ハーネス更新（会話を保って再起動）----
  # system prompt のファイルが消えていても、再開は `tako master` と同じく書き直してから起動する
  [ "$arm" = new ] && rm -f "$TAKO_ORCHESTRATOR_DIR/_system_prompt_st1967.md"
  n="$(launch_count "$TMP/launches.log")"
  resp="$("$TAKO_BIN" session-restart --mode harness --pane "$master" 2>&1)"
  echo "  harness: applied=$(json_field "$resp" applied) recipe=$(json_field "$resp" recipe) profile=$(json_field "$resp" profile)"
  if [ "$arm" = new ]; then
    # 重ねて押す（本番では 2 回目の押下が立ち上がったばかりの claude を終了させた疑い）
    local again
    again="$("$TAKO_BIN" session-restart --mode harness --pane "$master" 2>&1)"
    check "$(printf '%s' "$again" | grep -q '建て直しがまだ終わっていない' && echo 1 || echo 0)" \
      "建て直しの最中に重ねた再起動は理由つきで断る"
  fi
  phase="$(wait_restart_settled "$master")"
  sleep 1
  echo "  last_restart: $(restart_field "$master" last_restart)"
  echo "  起動の記録（再起動の後 $(( $(launch_count "$TMP/launches.log") - n )) 回）:"
  sed -n "$((n + 1)),\$p" "$TMP/launches.log" | sed 's/^/    /'
  line2="$(launch_line "$TMP/launches.log" $((n + 1)))"
  if [ "$arm" = new ]; then
    check "$([ "$phase" = started ] && echo 1 || echo 0)" "last_restart.phase=started（実測 ${phase}）"
    check "$([ "$(launch_count "$TMP/launches.log")" = $((n + 1)) ] && echo 1 || echo 0)" \
      "再開は 1 回だけ起動する"
    check "$([ "$(args_of "$line2")" = "$(args_of "$line1") --resume $SID" ] && echo 1 || echo 0)" \
      "再開の引数 = 元の起動の引数 + --resume <全長の ID>（model / effort / system prompt が落ちない）"
    check "$([ "$(env_of "$line2")" = "$(env_of "$line1")" ] && echo 1 || echo 0)" \
      "role（master:st1967）と設定 dir が元の起動と同じ"
    grep -q "セッション再起動: 開始 pane=${master} mode=harness .*組み立て=master_profile プロファイル=st1967" \
      "$TAKO_DATA_DIR/persist.log"
    check "$([ $? -eq 0 ] && echo 1 || echo 0)" "persist.log に開始（モード・組み立て・プロファイル）が残る"
    grep -q "セッション再起動: 結果=起動 pane=${master}" "$TAKO_DATA_DIR/persist.log"
    check "$([ $? -eq 0 ] && echo 1 || echo 0)" "persist.log に結果が残る"
    check "$(grep -q -- "--resume" "$TAKO_DATA_DIR/persist.log" && echo 0 || echo 1)" \
      "persist.log にコマンドの本文を出さない"
    check "$([ -s "$TAKO_ORCHESTRATOR_DIR/_system_prompt_st1967.md" ] && echo 1 || echo 0)" \
      "消えていた system prompt のファイルを書き直してから起動した"
  else
    check "$(printf '%s' "$line2" | grep -q -- '--append-system-prompt-file' && echo 0 || echo 1)" \
      "旧挙動で system prompt が落ちる（対照）"
    check "$(printf '%s' "$line2" | grep -q -- '--effort' && echo 0 || echo 1)" \
      "旧挙動で effort が落ちる（対照）"
    if [ "$shape" = short ]; then
      check "$([ "$(launch_count "$TMP/launches.log")" -ge $((n + 2)) ] && grep -q -- "--resume ${SID}export" "$TMP/launches.log" && echo 1 || echo 0)" \
        "旧挙動で 2 行のペインの再開コマンドが化けて 2 回起動する（#1940 型。対照）"
    else
      local token
      token="$(args_of "$line2" | sed -n 's/.*--resume \([^ ]*\).*/\1/p')"
      check "$([ -n "$token" ] && [ "$token" != "$SID" ] && [ "${SID#"$token"}" != "$SID" ] && echo 1 || echo 0)" \
        "旧挙動で折り返した案内の途中までの ID（${token:-なし}）で resume する（本番の ②。対照）"
      check "$([ "$phase" = started ] && echo 1 || echo 0)" \
        "旧挙動は会話が見つからずに落ちても started と言う（本番の ③ = 黙る。対照）"
    fi
    echo "  （旧挙動の persist.log の再起動の行: $(grep -c 'セッション再起動:' "$TAKO_DATA_DIR/persist.log" 2>/dev/null || echo 0) 行）"
    stop_isolated_gui "$ISOLATED_GUI_PID"
    tmux -L "$sock" kill-server >/dev/null 2>&1
    return
  fi
  if [ "$shape" = tall ]; then
    stop_isolated_gui "$ISOLATED_GUI_PID"
    tmux -L "$sock" kill-server >/dev/null 2>&1
    return
  fi

  # ---- 2. 会話の記録が無い: 終了させずに理由を返す ----
  mv "$TMP/cfg/projects/-tmp-st1967/$SID.jsonl" "$TMP/moved.jsonl"
  n="$(launch_count "$TMP/launches.log")"
  resp="$("$TAKO_BIN" session-restart --mode harness --pane "$master" 2>&1)"
  check "$(printf '%s' "$resp" | grep -q '記録' && echo 1 || echo 0)" \
    "記録が無い会話は応答に理由が出る: $(printf '%s' "$resp" | head -c 160)"
  grep -q "セッション再起動: 断った pane=${master} mode=harness 理由=conversation_missing" \
    "$TAKO_DATA_DIR/persist.log"
  check "$([ $? -eq 0 ] && echo 1 || echo 0)" "persist.log に断った理由が残る"
  check "$([ "$(launch_count "$TMP/launches.log")" = "$n" ] && echo 1 || echo 0)" \
    "記録が無いときはエージェントに触らない（起動もしない）"
  mv "$TMP/moved.jsonl" "$TMP/cfg/projects/-tmp-st1967/$SID.jsonl"

  # ---- 3. 送った後にすぐ落ちる: 1 回打ち直し、だめなら理由を残す ----
  # （偽 claude を「記録はあるのにすぐ終わる」にする = 理由は agent_exited。記録の無い ID で
  #   落ちたときの conversation_not_found は単体テストと旧挙動の腕が見る）
  : > "$TMP/fail-resume"
  n="$(launch_count "$TMP/launches.log")"
  resp="$("$TAKO_BIN" session-restart --mode harness --pane "$master" 2>&1)"
  phase="$(wait_restart_settled "$master")"
  echo "  last_restart: $(restart_field "$master" last_restart)"
  check "$([ "$phase" = failed ] && echo 1 || echo 0)" "すぐ落ちた再開は phase=failed（実測 ${phase}）"
  check "$([ "$(restart_field "$master" last_restart.reason)" = agent_exited ] && [ "$(restart_field "$master" last_restart.exit_code)" = 1 ] && echo 1 || echo 0)" \
    "理由は agent_exited・終了コード 1"
  check "$([ "$(restart_field "$master" last_restart.attempts)" = 2 ] && echo 1 || echo 0)" \
    "1 回だけ打ち直した（attempts=2）"
  check "$([ "$(launch_count "$TMP/launches.log")" = $((n + 2)) ] && echo 1 || echo 0)" \
    "起動は 2 回（最初 + 打ち直し）"
  grep -q "セッション再起動: 打ち直し pane=${master}" "$TAKO_DATA_DIR/persist.log" &&
    grep -q "セッション再起動: 結果=失敗 pane=${master} 理由=agent_exited 終了コード=1 試行=2" "$TAKO_DATA_DIR/persist.log"
  check "$([ $? -eq 0 ] && echo 1 || echo 0)" "persist.log に打ち直しと失敗の理由が残る"
  check "$(restart_field "$master" last_restart.message | grep -q 'tako session-restart --mode harness' && echo 1 || echo 0)" \
    "応答（last_restart.message）に次の一手が出る"

  # ---- 4. エージェントが居ないペインの引き継ぎは断り、harness で戻せる ----
  resp="$("$TAKO_BIN" session-restart --mode handoff --pane "$master" 2>&1)"
  check "$(printf '%s' "$resp" | grep -q -- '--mode harness' && echo 1 || echo 0)" \
    "エージェントが居ないペインの handoff は断り、harness を案内する"
  rm -f "$TMP/fail-resume"
  check "$([ "$(restart_field "$master" shell_idle)" = True ] && echo 1 || echo 0)" \
    "シェルだけが残ったペインは shell_idle=true"
  n="$(launch_count "$TMP/launches.log")"
  resp="$("$TAKO_BIN" session-restart --mode harness --pane "$master" 2>&1)"
  check "$([ "$(json_field "$resp" terminated_pid)" = null ] && echo 1 || echo 0)" \
    "シェルだけのペインは終了させずに打つ"
  phase="$(wait_restart_settled "$master")"
  line2="$(launch_line "$TMP/launches.log" $((n + 1)))"
  check "$([ "$phase" = started ] && [ "$(args_of "$line2")" = "$(args_of "$line1") --resume $SID" ] && echo 1 || echo 0)" \
    "シェルだけのペインも同じ引数で戻る（phase=${phase}）"

  # ---- 4b. claude ではないプログラムが前景で動く master ペイン（セルフテスト項目 140 の (e) と同じ条件）----
  # エージェントのプロセスは見つからないが、シェルは入力待ちではない = 引き継ぎの依頼は従来どおり積む
  local other
  other="$("$TAKO_BIN" split --pane "$master" 2>/dev/null | python3 -c 'import re, sys; m = re.search(r"[0-9]+", sys.stdin.read()); print(m.group(0) if m else "")')"
  "$TAKO_BIN" title --pane "$other" --role orchestrator-master:st1967 >/dev/null 2>&1
  sleep 2
  "$TAKO_BIN" send --pane "$other" "sleep 600" >/dev/null 2>&1
  for i in $(seq 1 40); do
    [ "$(pane_state "$other")" = running ] && break
    sleep 0.5
  done
  resp="$("$TAKO_BIN" session-restart --mode handoff --pane "$other" 2>&1)"
  check "$([ "$(json_field "$resp" applied)" = True ] && echo 1 || echo 0)" \
    "前景で claude 以外が動く master ペインの handoff は従来どおり依頼を積む（pane=${other} state=$(pane_state "$other")）"
  resp="$("$TAKO_BIN" session-restart --mode harness --pane "$other" 2>&1)"
  check "$(printf '%s' "$resp" | grep -q 'ハーネス更新できない' && echo 1 || echo 0)" \
    "同じペインの harness は終了させる相手も会話も無いので理由つきで断る"
  "$TAKO_BIN" close --pane "$other" >/dev/null 2>&1

  # ---- 5. 引き継ぎの後任: 呼び出し元の名乗りが無くても元の profile・設定 dir で立つ ----
  n="$(launch_count "$TMP/launches.log")"
  resp="$("$TAKO_BIN" orchestrator handoff --pane "$master" 2>&1)"
  local succ
  succ="$(json_field "$resp" new_master_pane_id)"
  echo "  handoff: profile=$(json_field "$resp" profile) profile_source=$(json_field "$resp" profile_source) 後任=${succ}"
  wait_launches "$TMP/launches.log" $((n + 1))
  sleep 4
  read -r cols rows < <(pane_dims "$succ")
  echo "  後任 pane=${succ} の寸法: ${cols} 桁 × ${rows} 行"
  line2="$(launch_line "$TMP/launches.log" $((n + 1)))"
  echo "  後任の起動: $(env_of "$line2")"
  check "$([ "$(launch_count "$TMP/launches.log")" = $((n + 1)) ] && echo 1 || echo 0)" \
    "後任は 1 回だけ起動する"
  check "$([ "$(args_of "$line2")" = "$(args_of "$line1")" ] && [ "$(env_of "$line2")" = "$(env_of "$line1")" ] && echo 1 || echo 0)" \
    "後任は元の profile（master:st1967）・設定 dir・model・effort・system prompt で立つ（default に化けない）"

  # ---- 6. `tako sessions resume` も同じ組み立て ----
  n="$(launch_count "$TMP/launches.log")"
  resp="$("$TAKO_BIN" sessions resume "$SID" --pane "$master" 2>&1)"
  echo "  sessions resume: $(printf '%s' "$resp" | head -2 | tr '\n' ' ')"
  wait_launches "$TMP/launches.log" $((n + 1))
  sleep 3
  line2="$(launch_line "$TMP/launches.log" $((n + 1)))"
  check "$([ "$(args_of "$line2")" = "$(args_of "$line1") --resume $SID" ] && [ "$(env_of "$line2")" = "$(env_of "$line1")" ] && echo 1 || echo 0)" \
    "tako sessions resume も元の起動の引数 + --resume で立つ"

  # ---- 7. worker: spawn と同じ引数（許可モード・model / effort）で戻る ----
  n="$(launch_count "$TMP/launches.log")"
  resp="$("$TAKO_BIN" orchestrator spawn --project st1967p --prompt "#1967 の worker" \
    --pane "$master" --label w 2>&1)"
  local worker
  worker="$(json_field "$resp" pane_id)"
  wait_launches "$TMP/launches.log" $((n + 1))
  sleep 3
  local wline1
  wline1="$(launch_line "$TMP/launches.log" $((n + 1)))"
  echo "  worker pane=${worker} の起動: $(env_of "$wline1")"
  write_catalog "$TAKO_SESSIONS_FILE" "$WSID" worker "$worker" "project: st1967p|label: w"
  n="$(launch_count "$TMP/launches.log")"
  resp="$("$TAKO_BIN" session-restart --mode harness --pane "$worker" 2>&1)"
  echo "  worker harness: recipe=$(json_field "$resp" recipe) profile=$(json_field "$resp" profile)"
  phase="$(wait_restart_settled "$worker")"
  line2="$(launch_line "$TMP/launches.log" $((n + 1)))"
  check "$([ "$phase" = started ] && [ "$(args_of "$line2")" = "$(args_of "$wline1") --resume $WSID" ] && [ "$(env_of "$line2")" = "$(env_of "$wline1")" ] && echo 1 || echo 0)" \
    "worker の再開 = spawn の引数（--permission-mode auto 等）+ --resume（phase=${phase}）"

  # ---- 8. solo: `tako solo -<名前>` と同じ引数（solo の system prompt）で戻る ----
  mkdir -p "$TAKO_ORCHESTRATOR_DIR/solo-profiles"
  printf 'model: claude-opus-5-5\neffort: high\nmaster_account: st1967acct\n' \
    > "$TAKO_ORCHESTRATOR_DIR/solo-profiles/st1967s.yaml"
  local solo ssid sline1
  ssid="1967cccc-b67d-44ac-b897-065b9f0678f0"
  printf '{"type":"user","message":{"content":"#1967 の solo"}}\n' \
    > "$TMP/cfg/projects/-tmp-st1967/$ssid.jsonl"
  n="$(launch_count "$TMP/launches.log")"
  PATH="$TMP/bin:$PATH" "$TAKO_BIN" solo -st1967s >/dev/null 2>&1
  solo=""
  for i in $(seq 1 40); do
    solo="$(panes_with_role solo:st1967s | tail -1)"
    [ -n "$solo" ] && break
    sleep 0.5
  done
  wait_launches "$TMP/launches.log" $((n + 1))
  sleep 2
  sline1="$(launch_line "$TMP/launches.log" $((n + 1)))"
  echo "  solo pane=${solo:-なし} の起動: $(env_of "$sline1")"
  write_catalog "$TAKO_SESSIONS_FILE" "$ssid" solo "$solo" "profile: st1967s"
  n="$(launch_count "$TMP/launches.log")"
  resp="$("$TAKO_BIN" session-restart --mode harness --pane "$solo" 2>&1)"
  echo "  solo harness: recipe=$(json_field "$resp" recipe) profile=$(json_field "$resp" profile)"
  phase="$(wait_restart_settled "$solo")"
  line2="$(launch_line "$TMP/launches.log" $((n + 1)))"
  check "$([ "$phase" = started ] && [ "$(args_of "$line2")" = "$(args_of "$sline1") --resume $ssid" ] && [ "$(env_of "$line2")" = "$(env_of "$sline1")" ] && echo 1 || echo 0)" \
    "solo の再開 = tako solo の引数（solo の system prompt）+ --resume（phase=${phase}）"

  stop_isolated_gui "$ISOLATED_GUI_PID"
  tmux -L "$sock" kill-server >/dev/null 2>&1
}

for shape in $SHAPES; do
  for arm in $ARMS; do
    run_arm "$arm" "$shape"
  done
done

echo
echo "PASS ${PASS} / FAIL ${FAIL}"
[ "$FAIL" -eq 0 ]
