#!/usr/bin/env bash
# test-reattach-1857.sh — tmux の attach クライアントだけが外から終わってもペインを閉じない、の実経路テスト（#1857）
#
# 隔離した data / tmux で**実 tako-app** を立て、
#   ① attach クライアントだけを kill -TERM すると、ペインは同じ ID のまま再 attach し、
#      中のプログラム（tick-N を出し続ける sh）の出力が続けて見える。persist.log に
#      `attach クライアント異常終了: … 理由=exit 1 → 再 attach（1 回目）` が 1 行出て、
#      ペインの内容（tick-N）は含まない。状態は CLI（tako list）と MCP（tako_list_panes）から読める
#   ② backend セッションを kill した場合は従来どおりペインが閉じる（記録は書かない）
#   ③ クライアントを連続で殺すと上限（60 秒に 5 回）で止まり、ペインは閉じずに
#      「再接続できない」を画面に出す。persist.log に上限の行が残る
#   ④ 止まったペインは `tako persist reattach --pane N`（CLI）/ MCP `tako_persist` の
#      `reattach` で繋ぎ直せる
#   ⑤ クライアントとセッションがほぼ同時に死ぬと、ペインは閉じ、新しいセッションを作らない
#   ⑥ 再 attach を待っている間にペインを閉じると、蘇らない（セッションも残らない）
#   ⑦ A/B: `TAKO_1857_LEGACY=1`（同じバイナリで旧挙動）では ① でペインが閉じる
# を実測する。
#
# **kill するのは隔離ソケットの `list-clients` で引いた自分のクライアントの pid だけ**。
# 名前・パターン指定の kill（pkill / killall）は使わない（#1857 の事故の原因そのもの）。
# **本番の tako / 設定には一切触らない**（data / HOME は mktemp 配下、tmux は専用ソケット、
# 落とすのは自分で起こした pid だけ）。窓は常設の仮想ディスプレイ（tako-vd）へ出る（#1141）。
#
# 修正前のバイナリで ① が落ちることを見るときは `APP_BIN=<修正前の tako-app> ONLY=1` で回す。
# **CI には登録しない**（実 GUI を立てるので表示のある実機でだけ回る）。CI 側の担保は
# 単体テスト（`tako_core::backend_reattach` / `tmux_backend` の実 tmux テスト）と番犬
# （`crates/tako-control/tests/issue1857_backend_reattach_watchdog.rs`）。
#
# 使い方: bash scripts/test-reattach-1857.sh
set -uo pipefail

# **本番 GUI を指す env を最初に落とす**（#1449 / #1450 で本番にペインが漏れた実例が 2 件）
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_ORCHESTRATOR_ROLE TAKO_MCP_URL

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ONLY="${ONLY:-}"
PASS=0
FAIL=0

pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
fail() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }
check_eq() {
  if [ "$2" = "$3" ]; then pass "$1"; else fail "$1（期待 '${2}' / 実際 '${3}'）"; fi
}

TMP="$(mktemp -d /tmp/tako-1857-XXXXXX)"
APP_PID=""
TMUX_SOCKET="tako-1857-$$"
cleanup() {
  stop_isolated_gui "$APP_PID"
  tmux -L "$TMUX_SOCKET" kill-server >/dev/null 2>&1 || true
  # tmux は kill-server の後もソケットファイルを残すことがある（自分の名前のものだけ消す）
  rm -f "${TMUX_TMPDIR:-/tmp}/tmux-$(id -u)/$TMUX_SOCKET"
  if [ -n "${KEEP:-}" ]; then echo "（KEEP: $TMP を残した）"; else rm -rf "$TMP"; fi
}
trap cleanup EXIT

# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
isolated_gui_bins || exit 1
# 走っているあいだだけディスプレイを眠らせない（眠ると tako から面が 1 枚も見えず、
# 検証用 GUI は窓を開かずに終わる = #1160。規約: 長い検証は caffeinate -d -w）
caffeinate -d -u -w $$ >/dev/null 2>&1 &

# --- 隔離した環境 -------------------------------------------------------------
export HOME="$TMP/home"
mkdir -p "$HOME"
export TAKO_ISOLATED=1
export TAKO_DATA_DIR="$TMP/data"
export TAKO_DISCOVERY_DIR="$TMP/disc"
export TAKO_ORCHESTRATOR_DIR="$TMP/orch"
export TAKO_SESSIONS_FILE="$TMP/sessions.yaml"
export TAKO_PANE_LOG_DIR="$TMP/panelogs"
export TAKO_WORKERS_FILE="$TMP/workers.yaml"
export TAKO_TMUX_SOCKET="$TMUX_SOCKET"
# tmux バックエンドの経路そのものを見るので永続は ON（TAKO_ISOLATED は既定で OFF にする）
export TAKO_PERSIST=1
mkdir -p "$TAKO_DATA_DIR" "$TAKO_DISCOVERY_DIR" "$TAKO_ORCHESTRATOR_DIR"
for d in "$HOME" "$TAKO_DATA_DIR" "$TAKO_ORCHESTRATOR_DIR"; do
  case "$d" in
    "$TMP"/*) : ;;
    *) echo "隔離されていないディレクトリ: $d"; exit 1 ;;
  esac
done
PLOG="$TAKO_DATA_DIR/persist.log"

#   start_app <ログの名前> [VAR=VAL …]
start_app() {
  local name="$1"
  shift
  launch_isolated_gui "$TMP/app-${name}.log" ${@+"$@"} || exit $?
  APP_PID="$ISOLATED_GUI_PID"
  wait_isolated_gui "$TMP/app-${name}.log" || exit 1
}
stop_app() {
  stop_isolated_gui "$APP_PID"
  APP_PID=""
  tmux -L "$TMUX_SOCKET" kill-server >/dev/null 2>&1 || true
  sleep 1
}

# --- 観測の道具 -----------------------------------------------------------------
# ペインの 1 項目（無ければ "absent"、値が null なら "null"）
pane_field() {
  "$TAKO_BIN" list 2>/dev/null | python3 -c '
import json, sys
pane, path = int(sys.argv[1]), sys.argv[2].split(".")
try:
    data = json.load(sys.stdin)
except Exception:
    print("absent"); sys.exit()
for tab in data.get("tabs", []):
    for p in tab.get("panes", []):
        if p.get("id") == pane:
            v = p
            for k in path:
                v = v.get(k) if isinstance(v, dict) else None
            print("null" if v is None else v)
            sys.exit()
print("absent")' "$1" "$2"
}
# 自分の隔離ソケットで、そのセッションに attach しているクライアントの pid
client_of() {
  tmux -L "$TMUX_SOCKET" list-clients -F '#{client_pid} #{session_name}' 2>/dev/null \
    | awk -v s="$1" '$2 == s { print $1; exit }'
}
has_session() { tmux -L "$TMUX_SOCKET" has-session -t "=$1" 2>/dev/null; }
# 画面に出ている tick-N の最大値（無ければ 0）
last_tick() {
  "$TAKO_BIN" read --pane "$1" 2>/dev/null | python3 -c '
import json, re, sys
raw = sys.stdin.read()
try:
    text = json.loads(raw).get("text", "")
except Exception:
    text = raw
nums = [int(n) for n in re.findall(r"tick-(\d+)", text)]
print(max(nums) if nums else 0)'
}
screen_has() {
  "$TAKO_BIN" read --pane "$1" 2>/dev/null | python3 -c '
import json, sys
raw = sys.stdin.read()
try:
    text = json.loads(raw).get("text", "")
except Exception:
    text = raw
# 折り返し（長い行が幅で割れる）で照合が外れないよう、空白と改行を落として比べる
squash = lambda t: "".join(t.split())
sys.exit(0 if squash(sys.argv[1]) in squash(text) else 1)' "$2"
}
# 0.1 秒刻みで最大 $1 回、条件（残りの引数）が成り立つまで待つ（**状態で待つ**）
wait_for() {
  local tries="$1" i
  shift
  for i in $(seq 1 "$tries"); do
    if "$@"; then return 0; fi
    sleep 0.1
  done
  return 1
}
status_is() { [ "$(pane_field "$1" backend_reattach.status)" = "$2" ]; }
pane_gone() { [ "$(pane_field "$1" id)" = "absent" ]; }
new_client() { local c; c="$(client_of "$1")"; [ -n "$c" ] && [ "$c" != "$2" ]; }
tick_beyond() { [ "$(last_tick "$1")" -gt "$2" ]; }
# tick-N を出し続けるペインを立てて ID を返す
spawn_ticker() {
  "$TAKO_BIN" split --pane "$1" --down -- /bin/sh -c \
    'i=0; while :; do i=$((i+1)); echo tick-$i; sleep 0.3; done' | tr -dc '0-9'
}
# MCP stdio ブリッジは接続情報を env から取る（隔離 GUI の data dir の socket とトークン。
# export しない = CLI の接続は従来どおり discovery で決まる）
mcp_call() {
  local sock="$TAKO_DATA_DIR/tako.sock"
  [ -f "$TAKO_DATA_DIR/tako.sock.path" ] && sock="$(cat "$TAKO_DATA_DIR/tako.sock.path")"
  printf '%s\n' \
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"t1857","version":"0"}}}' \
    "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"$1\",\"arguments\":$2}}" \
    | TAKO_SOCKET="$sock" TAKO_TOKEN="$(cat "$TAKO_DATA_DIR/token")" "$TAKO_BIN" mcp serve 2>/dev/null \
    | python3 -c '
import json, sys
for line in sys.stdin:
    try:
        msg = json.loads(line)
    except Exception:
        continue
    if msg.get("id") == 2:
        if msg.get("error"):
            print(msg["error"].get("message", ""))
            sys.exit(1)
        res = msg.get("result", {})
        for c in res.get("content", []):
            if c.get("type") == "text":
                print(c["text"])
        if res.get("isError"):
            sys.exit(1)'
}

# --- ① クライアントだけ殺すと同じペインのまま戻る --------------------------------
scenario_client_only() {
  local root p s c1 before line
  echo "== ① attach クライアントだけを kill -TERM する =="
  root="$(pane_field_root)"
  p="$(spawn_ticker "$root")"
  wait_for 100 tick_beyond "$p" 3 || { fail "ticker が動き出さない（ペイン ${p}）"; return; }
  s="$(pane_field "$p" tmux_session)"
  wait_for 50 new_client "$s" "" || { fail "クライアントが見つからない（${s}）"; return; }
  c1="$(client_of "$s")"
  before="$(last_tick "$p")"
  echo "  ペイン ${p} / セッション ${s} / クライアント pid ${c1} / kill 前の tick ${before}"
  kill -TERM "$c1"
  if wait_for 100 status_is "$p" attached && wait_for 50 new_client "$s" "$c1"; then
    pass "ペイン ${p} は閉じずに同じ ID のまま再 attach した（新しいクライアント pid $(client_of "$s")）"
  else
    fail "ペイン ${p} が再 attach しない（状態 $(pane_field "$p" backend_reattach.status) / 存在 $(pane_field "$p" id)）"
    return
  fi
  if wait_for 100 tick_beyond "$p" "$((before + 2))"; then
    pass "中のプログラムの出力が続けて見える（tick ${before} → $(last_tick "$p")）"
  else
    fail "再 attach 後に出力が進まない（tick $(last_tick "$p")）"
  fi
  line="$(grep "attach クライアント異常終了: pane=${p} session=${s}" "$PLOG" | tail -1)"
  echo "  persist.log: ${line}"
  case "$line" in
    *"理由=exit 1 → 再 attach（1 回目）"*) pass "persist.log に理由（exit 1）と判断の 1 行が残る" ;;
    *) fail "persist.log の行が期待と違う（${line}）" ;;
  esac
  case "$line" in
    *tick-*) fail "persist.log にペインの内容が混ざった" ;;
    *) pass "persist.log の行にペインの内容（tick-N）が含まれない" ;;
  esac
  check_eq "CLI（tako list）から回数が読める" "1" "$(pane_field "$p" backend_reattach.total)"
  check_eq "CLI（tako list）から直近の理由が読める" "exit 1" "$(pane_field "$p" backend_reattach.last_reason)"
  local mcp
  mcp="$(mcp_call tako_list_panes '{}' | python3 -c '
import json, sys
pane = int(sys.argv[1])
data = json.loads(sys.stdin.read())
for tab in data.get("tabs", []):
    for p in tab.get("panes", []):
        if p.get("id") == pane:
            r = p.get("backend_reattach") or {}
            print(r.get("status"), r.get("total"), r.get("last_reason"))' "$p")"
  check_eq "MCP（tako_list_panes）から状態が読める" "attached 1 exit 1" "$mcp"
  "$TAKO_BIN" close --pane "$p" >/dev/null 2>&1
}
pane_field_root() {
  "$TAKO_BIN" list | python3 -c 'import json,sys; print(json.load(sys.stdin)["tabs"][0]["panes"][0]["id"])'
}

# --- ② セッションを kill したら従来どおり閉じる ------------------------------------
scenario_session_killed() {
  local root p s
  echo "== ② backend セッションを kill する =="
  root="$(pane_field_root)"
  p="$(spawn_ticker "$root")"
  wait_for 100 tick_beyond "$p" 1 || { fail "ticker が動き出さない"; return; }
  s="$(pane_field "$p" tmux_session)"
  tmux -L "$TMUX_SOCKET" kill-session -t "=$s"
  if wait_for 100 pane_gone "$p"; then
    pass "セッションが終わったペイン ${p} は従来どおり閉じる"
  else
    fail "セッションが終わったのにペイン ${p} が残っている（状態 $(pane_field "$p" backend_reattach.status)）"
  fi
  if grep -q "attach クライアント異常終了: pane=${p} " "$PLOG"; then
    fail "普通の終わり方（セッション終了）まで persist.log に書いている"
  else
    pass "セッションごと終わった普通の終わり方は persist.log に書かない"
  fi
}

# --- ③④ 連続で殺すと止まり、手動で繋ぎ直せる -------------------------------------
scenario_give_up_and_manual() {
  local root p s c prev n out before
  echo "== ③ クライアントを連続で殺す（上限 60 秒に 5 回） =="
  root="$(pane_field_root)"
  p="$(spawn_ticker "$root")"
  wait_for 100 tick_beyond "$p" 1 || { fail "ticker が動き出さない"; return; }
  s="$(pane_field "$p" tmux_session)"
  prev=""
  for n in 1 2 3 4 5 6; do
    if ! wait_for 100 new_client "$s" "$prev"; then
      fail "${n} 回目: 殺すクライアントが現れない（状態 $(pane_field "$p" backend_reattach.status)）"
      return
    fi
    c="$(client_of "$s")"
    kill -TERM "$c"
    prev="$c"
  done
  if wait_for 100 status_is "$p" gave_up; then
    pass "6 回目で止まった（状態 gave_up / 直近の回数 $(pane_field "$p" backend_reattach.attempts)）"
  else
    fail "上限で止まらない（状態 $(pane_field "$p" backend_reattach.status)）"
  fi
  check_eq "止まってもペイン ${p} は閉じない" "$p" "$(pane_field "$p" id)"
  if wait_for 50 screen_has "$p" "tako persist reattach --pane ${p}"; then
    pass "画面に「再接続できない」と次の一手のコマンドが出る"
  else
    fail "画面に再接続できない旨が出ていない（画面の末尾: $("$TAKO_BIN" read --pane "$p" --lines 6 2>/dev/null | tr '\n' ' ')）"
  fi
  check_eq "応答に次の一手が載る" "tako persist reattach --pane ${p}" "$(pane_field "$p" backend_reattach.next_step)"
  sleep 1.5
  if [ -z "$(client_of "$s")" ]; then
    pass "止まった後は再 attach を撃たない（クライアント 0 本）"
  else
    fail "止まった後もクライアントが立っている"
  fi
  if grep -q "attach クライアント異常終了: pane=${p} session=${s} .*上限" "$PLOG"; then
    pass "persist.log に上限の行が残る"
    grep "attach クライアント異常終了: pane=${p} session=${s} .*上限" "$PLOG" | tail -1 | sed 's/^/  persist.log: /'
  else
    fail "persist.log に上限の行が無い"
  fi

  echo "== ④ 止まったペインを CLI で繋ぎ直す =="
  before="$(last_tick "$p")"
  out="$("$TAKO_BIN" persist reattach --pane "$p" 2>&1)"
  echo "  応答: ${out}"
  case "$out" in
    *'"status":"attached"'*) pass "tako persist reattach の応答が attached" ;;
    *) fail "tako persist reattach の応答が attached でない" ;;
  esac
  if wait_for 50 new_client "$s" "" && wait_for 100 tick_beyond "$p" "$((before + 2))"; then
    pass "手動の再 attach で出力が続けて見える（tick ${before} → $(last_tick "$p")）"
  else
    fail "手動の再 attach の後に出力が進まない"
  fi
  out="$("$TAKO_BIN" persist reattach --pane "$p" 2>&1)"
  case "$out" in
    *"動いている"*) pass "クライアントが動いているペインの再 attach は断る" ;;
    *) fail "動いているペインの再 attach を断らない（${out}）" ;;
  esac

  echo "== ④' もう一度止めて MCP（tako_persist の reattach）で繋ぎ直す =="
  # 手動の再 attach は上限を数え直すので、止まるまで殺し直す（手動 1 回 + 自動 4 回で 5 回目に止まる）
  prev=""
  for n in 1 2 3 4 5 6 7 8; do
    status_is "$p" gave_up && break
    if ! wait_for 100 new_client "$s" "$prev"; then
      status_is "$p" gave_up && break
      fail "${n} 回目: 殺すクライアントが現れない"; return
    fi
    c="$(client_of "$s")"
    kill -TERM "$c"
    prev="$c"
    wait_for 30 status_is "$p" gave_up && break
  done
  wait_for 100 status_is "$p" gave_up || { fail "2 度目の上限で止まらない"; return; }
  out="$(mcp_call tako_persist "{\"reattach\":${p},\"enabled\":true}" 2>&1)"
  case "$out" in
    *"併用できない"*) pass "MCP: reattach と enabled の併用は断る" ;;
    *) fail "MCP: reattach と enabled の併用を断らない（${out}）" ;;
  esac
  before="$(last_tick "$p")"
  out="$(mcp_call tako_persist "{\"reattach\":${p}}")"
  echo "  MCP 応答: ${out}"
  case "$out" in
    *'"status":"attached"'*) pass "MCP（tako_persist の reattach）の応答が attached" ;;
    *) fail "MCP の応答が attached でない" ;;
  esac
  if wait_for 50 new_client "$s" "" && wait_for 100 tick_beyond "$p" "$((before + 2))"; then
    pass "MCP からの再 attach で出力が続けて見える（tick ${before} → $(last_tick "$p")）"
  else
    fail "MCP からの再 attach の後に出力が進まない"
  fi
  local nclients
  nclients="$(tmux -L "$TMUX_SOCKET" list-clients -F '#{session_name}' 2>/dev/null | grep -cx "$s")"
  check_eq "繋ぎ直した後のクライアントは 1 本" "1" "$nclients"
  "$TAKO_BIN" close --pane "$p" >/dev/null 2>&1
}

# --- ⑤ クライアントとセッションがほぼ同時に死ぬ ------------------------------------
scenario_both_die() {
  local root p s c
  echo "== ⑤ クライアントとセッションをほぼ同時に殺す =="
  root="$(pane_field_root)"
  p="$(spawn_ticker "$root")"
  wait_for 100 tick_beyond "$p" 1 || { fail "ticker が動き出さない"; return; }
  s="$(pane_field "$p" tmux_session)"
  wait_for 50 new_client "$s" "" || { fail "クライアントが見つからない"; return; }
  c="$(client_of "$s")"
  kill -TERM "$c"
  tmux -L "$TMUX_SOCKET" kill-session -t "=$s"
  if wait_for 100 pane_gone "$p"; then
    pass "ペイン ${p} は閉じる（固まらない）"
  else
    fail "ペイン ${p} が残っている（状態 $(pane_field "$p" backend_reattach.status)）"
  fi
  sleep 1
  if has_session "$s"; then
    fail "attach 専用のはずの再 attach が新しいセッション ${s} を作った"
  else
    pass "新しいセッションを作らない（${s} は無いまま）"
  fi
}

# --- ⑥ 再 attach を待っている間にペインを閉じる ------------------------------------
scenario_close_while_waiting() {
  local root p s c prev n
  echo "== ⑥ 再 attach を待っている間 / 問い合わせ中にペインを閉じる =="
  root="$(pane_field_root)"
  p="$(spawn_ticker "$root")"
  wait_for 100 tick_beyond "$p" 1 || { fail "ticker が動き出さない"; return; }
  s="$(pane_field "$p" tmux_session)"
  # 2 回殺して、3 回目の待ち（1 秒）を作る。その待ちの間に閉じる
  prev=""
  for n in 1 2 3; do
    wait_for 100 new_client "$s" "$prev" || { fail "${n} 回目: クライアントが現れない"; return; }
    c="$(client_of "$s")"
    kill -TERM "$c"
    prev="$c"
  done
  if wait_for 50 status_is "$p" waiting; then
    echo "  3 回目の再 attach を待っている（直近の回数 $(pane_field "$p" backend_reattach.attempts)）間に閉じる"
  else
    fail "3 回目の待ちを捉えられない（状態 $(pane_field "$p" backend_reattach.status)）"
  fi
  "$TAKO_BIN" close --pane "$p" >/dev/null 2>&1
  sleep 2
  check_eq "待ちの間に閉じたペイン ${p} は蘇らない" "absent" "$(pane_field "$p" id)"
  if [ -z "$(client_of "$s")" ] && ! has_session "$s"; then
    pass "明示 close どおりセッションも残らない（再 attach のクライアントも立たない）"
  else
    fail "閉じた後にクライアント / セッションが残っている"
  fi
  # 問い合わせ中（kill の直後）に閉じる
  p="$(spawn_ticker "$root")"
  wait_for 100 tick_beyond "$p" 1 || { fail "ticker が動き出さない"; return; }
  s="$(pane_field "$p" tmux_session)"
  wait_for 50 new_client "$s" "" || { fail "クライアントが見つからない"; return; }
  c="$(client_of "$s")"
  kill -TERM "$c"
  "$TAKO_BIN" close --pane "$p" >/dev/null 2>&1
  sleep 2
  check_eq "kill の直後に閉じたペイン ${p} も蘇らない" "absent" "$(pane_field "$p" id)"
  if [ -z "$(client_of "$s")" ] && ! has_session "$s"; then
    pass "kill の直後に閉じてもクライアント / セッションは残らない"
  else
    fail "kill の直後に閉じた後にクライアント / セッションが残っている"
  fi
}

# --- ⑦ A/B: 旧挙動ではペインが閉じる ----------------------------------------------
scenario_legacy() {
  local root p s c
  echo "== ⑦ A/B: TAKO_1857_LEGACY=1（旧挙動） =="
  root="$(pane_field_root)"
  p="$(spawn_ticker "$root")"
  wait_for 100 tick_beyond "$p" 1 || { fail "ticker が動き出さない"; return; }
  s="$(pane_field "$p" tmux_session)"
  wait_for 50 new_client "$s" "" || { fail "クライアントが見つからない"; return; }
  c="$(client_of "$s")"
  kill -TERM "$c"
  if wait_for 50 pane_gone "$p"; then
    pass "旧挙動ではクライアントだけ殺されてもペイン ${p} が閉じる（= 症状の再現）"
  else
    fail "旧挙動なのにペインが残る（A/B の入口が効いていない）"
  fi
  if has_session "$s"; then
    pass "そのときセッション ${s} は生きている（閉じたのは tako 側だけ）"
  else
    fail "セッションまで消えている"
  fi
}

echo "== 隔離 GUI を起こす =="
start_app main
TABS="$("$TAKO_BIN" list | python3 -c 'import json,sys; print(len(json.load(sys.stdin)["tabs"]))')"
if [ "$TABS" != "1" ]; then
  echo "繋がった先が隔離インスタンスではない（タブ ${TABS} 枚）。中止する。"; exit 1
fi
echo "  pid=$APP_PID socket=$TMUX_SOCKET"

scenario_client_only
if [ -z "$ONLY" ]; then
  scenario_session_killed
  scenario_give_up_and_manual
  scenario_both_die
  scenario_close_while_waiting
  stop_app
  start_app legacy TAKO_1857_LEGACY=1
  scenario_legacy
fi

echo
echo "結果: ${PASS} PASS / ${FAIL} FAIL"
[ "$FAIL" -eq 0 ]
