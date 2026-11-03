#!/usr/bin/env bash
# tako:run bash scripts/test-remote-resources-1999.sh
#
# test-remote-resources-1999.sh — スマホ（PWA）から PC の負荷と順番待ちを見る・優先度を変える・取り消す
# （#1999。エピック #1992 の S7）の実経路テスト
#
# 偽の tailscale CLI + 隔離した HOME / state / data で **実 daemon（tako remote start）+ 実 tako-app** を走らせる。
# 隔離 GUI は**明示の調停役**（`TAKO_QUEUE_SOCKET` + 注入 `TAKO_QUEUE_INJECT` = 同時数 1・メモリ圧 normal）で
# 起こし、本物の wrapper（`tako queue sleep 600`）を 3 本並べて、
#   ① 同じ時点の CLI `tako queue --json` と daemon の `GET /api/resources` が受付番号・並び・段で一致する
#   ② read（observe）/ interact の端末の変更は 403（文は daemon の表示言語の `RoleTooLow`）・CLI の答えは変わらない
#   ③ manage の端末が API で変えた段（受付番号 / ペインの既定）が CLI に見える・取り消した wrapper が 125 で終わる
#   ④ 表に無い操作（上限の書き換え）は manage でも 404・メソッド違いは 405
#   ⑤ PWA（Playwright の chromium。`/api/*` を実 daemon へ中継）の `#/` と `#/resources` が同じ並びを描き、
#      PWA の UI から上げた段が CLI に見える
#   ⑥ persist.log / 監査に表示名（`--label`）が出ない
# を実測する。GUI が居ないときの読み（`gui: false` + 機の状態）は ⑦ で daemon を残したまま GUI を落として見る。
#
# **本番の tailscale / serve 設定 / remote デーモン / tako 設定 / 順番待ちには一切触らない**
# （tailscale の呼び出しは偽物へ、HOME / state / data / discovery / 調停役のソケットは mktemp 配下）。
# 窓は常設の仮想ディスプレイ（tako-vd）へ出る（#1141）。**負荷は生成しない**: 並べる処理は眠るだけ。
# 落とすのは自分で起こした pid とソケットだけ（pkill / killall は使わない）。
#
# 使い方: bash scripts/test-remote-resources-1999.sh   （RES1999_DUMP_DIR=<dir> で画像の置き場を変える）
set -uo pipefail

# **本番 GUI と外側の順番待ちを指す env を最初に落とす**（#1449 / #1450 で本番にペインが漏れた実例が 2 件）
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_ORCHESTRATOR_ROLE TAKO_MCP_URL TAKO_CLI
unset TAKO_QUEUE_JOB TAKO_QUEUE_SOCKET TAKO_QUEUE_INJECT TAKO_QUEUE_PRIORITY TAKO_1994_INJECT
unset CLAUDE_CODE_CHILD_SESSION CLAUDE_CODE_SESSION_ID CLAUDECODE

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PASS=0
FAIL=0
UNMEASURED=""

pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
fail() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }
check_eq() {
  if [ "$2" = "$3" ]; then pass "$1"; else fail "$1（期待 '${2}' / 実際 '${3}'）"; fi
}
check_contains() {
  case "$2" in
    *"$3"*) pass "$1" ;;
    *) fail "$1（'${3}' を含まない: ${2}）" ;;
  esac
}

# `/tmp` 直下に取る（daemon の待ち受けと tmux が sun_path の上限に当たらない長さ。#1441）
TMP="$(mktemp -d /tmp/tk1999-XXXXXX)"
DUMP_DIR="${RES1999_DUMP_DIR:-$TMP/dump}"
APP_PID=""
TMUX_SOCKET="tk1999-$$"
JOB_A="" JOB_B="" JOB_C=""
MASK_USER="$(id -un)"
mask() { sed -e "s#${HOME}#~#g" -e "s#${MASK_USER}#<user>#g" -e "s#${TMP}#<TMP>#g"; }

cleanup() {
  # 自分で起こした wrapper だけを止める（受付番号 = その pid）
  for pid in $JOB_A $JOB_B $JOB_C; do
    kill "$pid" 2>/dev/null || true
  done
  if [ -n "${TAKO_REMOTE_STATE_DIR:-}" ] && [ -s "$TAKO_REMOTE_STATE_DIR/tako-remote.pid" ]; then
    "$TAKO_BIN" remote stop >/dev/null 2>&1 || true
  fi
  stop_isolated_gui "$APP_PID"
  # ソケット名を明示した起動は自分で畳む（#1192）
  tmux -L "$TMUX_SOCKET" kill-server >/dev/null 2>&1 || true
  if [ -n "${KEEP:-}" ]; then echo "（KEEP: $TMP を残した）"; else rm -rf "${TMP:?}"; fi
}
# 途中で死んだテストを「緑」に化けさせない（bash 3.2 の set -e + EXIT trap。#1864）
# shellcheck source=lib/exit-guard.sh
. "$REPO_ROOT/scripts/lib/exit-guard.sh"
tako_exit_trap cleanup "PWA の resources（#1999）の実経路テスト"

# --- 偽 tailscale CLI ---------------------------------------------------------
FAKE_TS="$TMP/fake-tailscale"
echo '{}' > "$TMP/serve.json"
cat > "$FAKE_TS" <<'FAKE'
#!/usr/bin/env bash
set -u
STATE="$(dirname "$0")"
while [ $# -gt 0 ]; do
  case "$1" in
    --socket) shift 2 ;;
    *) break ;;
  esac
done
NODE="fake-1999.tailfake.ts.net"
case "${1:-}" in
  --version) echo "1.0.0-fake"; exit 0 ;;
  status)
    printf '{"BackendState":"Running","Version":"1.0.0-fake","CertDomains":["%s"],"Self":{"DNSName":"%s."}}\n' "$NODE" "$NODE"
    exit 0 ;;
  whois)
    printf '{"Node":{"StableID":"nFAKE1999","Name":"iphone.tailfake.ts.net.","Hostinfo":{"Hostname":"iPhone"}},"UserProfile":{"LoginName":"tester@example.com"}}\n'
    exit 0 ;;
  serve)
    shift
    case "${1:-}" in
      status) cat "$STATE/serve.json"; exit 0 ;;
      --bg)
        printf '{"TCP":{"443":{"HTTPS":true}},"Web":{"%s:443":{"Handlers":{"/":{"Proxy":"%s"}}}}}\n' "$NODE" "${3:-}" > "$STATE/serve.json"
        exit 0 ;;
      --https=443) echo '{}' > "$STATE/serve.json"; exit 0 ;;
      *) exit 1 ;;
    esac ;;
  *) exit 1 ;;
esac
FAKE
chmod +x "$FAKE_TS"

# --- 対象バイナリ -------------------------------------------------------------
# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
isolated_gui_bins || exit 1
mkdir -p "$DUMP_DIR"

# --- 隔離した環境 -------------------------------------------------------------
REAL_HOME="$HOME"
# Playwright（node）の置き場は PATH を絞る前に覚える
NODE_DIR="$(dirname "$(command -v node 2>/dev/null || echo /usr/local/bin/node)")"
export HOME="$TMP/home"
mkdir -p "$HOME" "$TMP/zdot"
# ペインのシェルが実ユーザー名・ホスト名を画面に出さない（#927）
printf "PROMPT='tako %%1~ %%%% '\nRPROMPT=''\n" > "$TMP/zdot/.zshrc"
export ZDOTDIR="$TMP/zdot"
mkdir -p "$TMP/bin"
ln -sf "$TAKO_BIN" "$TMP/bin/tako"
export PATH="$TMP/bin:/usr/bin:/bin:/usr/sbin:/sbin:/opt/homebrew/bin:/usr/local/bin"

export TAKO_ISOLATED=1
export TAKO_DATA_DIR="$TMP/data"
export TAKO_DISCOVERY_DIR="$TMP/disc"
export TAKO_REMOTE_STATE_DIR="$TMP/remote"
export TAKO_ORCHESTRATOR_DIR="$TMP/orch"
export TAKO_SESSIONS_FILE="$TMP/sessions.yaml"
export TAKO_PANE_LOG_DIR="$TMP/panelogs"
export TAKO_WORKERS_FILE="$TMP/workers.yaml"
export TAKO_TMUX_SOCKET="$TMUX_SOCKET"
export TAKO_PERSIST=0
export TAKO_AUTORENAME=0
export TAKO_TAILSCALE_BIN="$FAKE_TS"
# #841: ループバック TCP では接続元プロセスを検証する。serve の代わりに繋ぐのは curl と
# Playwright の中継（node）
export TAKO_REMOTE_TRUSTED_PEER_NAMES="curl,node"
# #1452: 承認は「呼び出し元が tako-app か」も見る。この隔離環境では curl を GUI 側として名乗る
export TAKO_REMOTE_TRUSTED_ADMIN_NAMES="curl"
mkdir -p "$TAKO_DATA_DIR" "$TAKO_DISCOVERY_DIR" "$TAKO_REMOTE_STATE_DIR" "$TAKO_ORCHESTRATOR_DIR"
for d in "$HOME" "$TAKO_DATA_DIR" "$TAKO_REMOTE_STATE_DIR" "$TAKO_ORCHESTRATOR_DIR" "$TAKO_DISCOVERY_DIR"; do
  case "$d" in
    "$TMP"/*) : ;;
    *) echo "隔離されていないディレクトリ: $d"; exit 1 ;;
  esac
done
# 画面の文は日本語（英語は daemon の断りの単体テストと網羅テストが縛る）
printf '{"language":"ja"}' > "$TAKO_DATA_DIR/settings.json"
# 調停役は隔離 GUI 自身（本番の順番待ちへは並ばない）。同時数 1（12 GB あたり 1 = total 12 GB）・空き 10 GB・normal
QSOCK="$TMP/q.sock"
INJECT="$TMP/inject.json"
printf '{"total_mb":12288,"available_mb":10000,"pressure":"normal","swap_used_mb":0,"cpus":8,"unmanaged":0,"regather_secs":0}\n' > "$INJECT"
# 調停役の受け口は隔離 GUI のトークンで認証する（GUI が起きた後に `$TAKO_DATA_DIR/token` から読む）。
# 渡すのは調停役へ繋ぐ CLI と wrapper だけ（daemon や他の CLI へは継がない）
ISO_TOKEN=""
q() { TAKO_TOKEN="$ISO_TOKEN" TAKO_QUEUE_SOCKET="$QSOCK" "$TAKO_BIN" queue "$@"; }

# --- 起動 ---------------------------------------------------------------------
echo "=== 準備: 隔離 tako-app（明示の調停役）と daemon を起動 ==="
rc=0
launch_isolated_gui "$TMP/app.log" TAKO_QUEUE_SOCKET="$QSOCK" TAKO_QUEUE_INJECT="$INJECT" || rc=$?
if [ "$rc" -eq 4 ]; then
  echo "tako-vd を用意できない（蓋が閉じている等）。実 GUI が要るので未実測で止める"
  echo "PASS=$PASS FAIL=$FAIL 未実測: 隔離 GUI"
  exit 4
elif [ "$rc" -ne 0 ]; then
  echo "隔離 GUI を起こせない（${rc}）"; exit 1
fi
APP_PID="$ISOLATED_GUI_PID"
wait_isolated_gui "$TMP/app.log" || exit 1
ISO_TOKEN="$(cat "$TAKO_DATA_DIR/token" 2>/dev/null)"
[ -n "$ISO_TOKEN" ] || { echo "隔離 GUI のトークンが読めない（$TAKO_DATA_DIR/token）"; exit 1; }
BOOT_TABS="$("$TAKO_BIN" list | python3 -c 'import json,sys; print(len(json.load(sys.stdin)["tabs"]))')"
if [ "$BOOT_TABS" != "1" ]; then
  echo "繋がった先が隔離インスタンスではない（タブ ${BOOT_TABS} 枚）。本番 GUI を触る恐れがあるので中止する"
  exit 1
fi
PANE="$("$TAKO_BIN" list | python3 -c 'import json,sys
d=json.load(sys.stdin)
def walk(o):
    if isinstance(o, dict):
        if "surface" in o and "id" in o: return o["id"]
        for v in o.values():
            r=walk(v)
            if r is not None: return r
    elif isinstance(o, list):
        for v in o:
            r=walk(v)
            if r is not None: return r
print(walk(d))')"
case "$PANE" in
  ''|None|*[!0-9]*) echo "隔離 GUI のペインが読めない（'${PANE}'）"; exit 1 ;;
esac
pass "隔離 tako-app が起動して CLI から見える（pid ${APP_PID}・ペイン ${PANE}）"
for _ in $(seq 1 100); do
  if q --json 2>/dev/null | grep -q '"arbiter": true'; then break; fi
  sleep 0.1
done
check_contains "隔離 GUI が明示の調停役になっている" "$(q --json 2>/dev/null | tr -d '\n ')" '"role":"explicit"'

HOST_HDR="fake-1999.tailfake.ts.net"
api() {
  local method="$1" path="$2" body="${3:-}"
  if [ -n "$body" ]; then
    curl -s -o "$TMP/body" -w '%{http_code}' -X "$method" "$BASE$path" \
      -H "X-Forwarded-For: 100.64.0.7" -H "X-Forwarded-Host: $HOST_HDR" \
      -H 'Content-Type: application/json' -d "$body"
  else
    curl -s -o "$TMP/body" -w '%{http_code}' -X "$method" "$BASE$path" \
      -H "X-Forwarded-For: 100.64.0.7" -H "X-Forwarded-Host: $HOST_HDR"
  fi
}
# JSON のキーを辿る（`a.0.b`）。$1 = ファイル、$2 = 経路
jqf_in() { python3 -c 'import json,sys
d=json.load(open(sys.argv[1]))
cur=d
for k in sys.argv[2].split("."):
    if isinstance(cur,list):
        try: cur=cur[int(k)]
        except (ValueError,IndexError): cur=None; break
    elif isinstance(cur,dict) and k in cur: cur=cur[k]
    else: cur=None; break
print("null" if cur is None else (str(cur).lower() if isinstance(cur,bool) else cur))' "$1" "$2"; }
jqf() { jqf_in "$TMP/body" "$1"; }
# 順番待ちの行の要約（実行中 → 待ちの順に 受付番号:段:番目:段の出どころ）。$1 = status の JSON（CLI / daemon の queue 節）
digest() { python3 -c 'import json,sys
d=json.load(open(sys.argv[1]))
q=d.get("queue") or {}
out=[]
for r in q.get("running") or []: out.append("R%s:%s:-:%s" % (r["id"], r.get("priority"), r.get("priority_from")))
for r in q.get("waiting") or []: out.append("W%s:%s:%s:%s" % (r["id"], r.get("priority"), r.get("position"), r.get("priority_from")))
print(" ".join(out))' "$1"; }

start_daemon() {
  if ! "$TAKO_BIN" remote start > "$TMP/daemon.log" 2>&1; then
    echo "daemon を起動できなかった:"; mask < "$TMP/daemon.log"; exit 1
  fi
  for _ in $(seq 1 200); do
    [ -s "$TAKO_REMOTE_STATE_DIR/tako-remote.port" ] && break
    sleep 0.1
  done
  PORT="$(cat "$TAKO_REMOTE_STATE_DIR/tako-remote.port" 2>/dev/null || true)"
  [ -n "$PORT" ] || { echo "daemon の port が読めない:"; mask < "$TMP/daemon.log"; exit 1; }
  ADMIN_TOKEN="$(cat "$TAKO_REMOTE_STATE_DIR/tako-remote.token")"
  BASE="http://127.0.0.1:$PORT"
  for _ in $(seq 1 100); do
    if [ "$(api GET /api/health)" = "200" ]; then break; fi
    sleep 0.1
  done
}

# 端末の role をその値に据える（上げる = pair + 承認 / 下げる = 管理経路。#1452 と同じゲート）
pair_as() {
  api POST /api/pair "{\"name\":\"iPhone\",\"role\":\"$1\"}" >/dev/null
  curl -s -o /dev/null -X POST "$BASE/api/admin/pair/approve" \
    -H "X-Tako-Admin: $ADMIN_TOKEN" -H 'Content-Type: application/json' \
    -d "{\"device_id\":\"nFAKE1999\",\"role\":\"$1\"}"
  curl -s -o /dev/null -X POST "$BASE/api/admin/devices/role" \
    -H "X-Tako-Admin: $ADMIN_TOKEN" -H 'Content-Type: application/json' \
    -d "{\"device_id\":\"nFAKE1999\",\"role\":\"$1\"}"
  api GET /api/me >/dev/null
  ACTUAL_ROLE="$(jqf role)"
  if [ "$ACTUAL_ROLE" != "$1" ]; then
    fail "role を ${1} に据えられなかった（実際 ${ACTUAL_ROLE}）"
  fi
}

start_daemon
pass "隔離 daemon が起動した（loopback port ${PORT}）"

echo
echo "=== 準備: 本物の wrapper を 3 本並べる（同時数 1 = 1 本が走り 2 本が待つ）==="
# A / B は隔離 GUI のペインの処理、C はペインの外（launchd・別の端末と同じ扱い = low）。
# stdin は端末でない（`</dev/null`）= 人の打鍵の top にはならない
TAKO_PANE_ID="$PANE" TAKO_TOKEN="$ISO_TOKEN" TAKO_QUEUE_SOCKET="$QSOCK" "$TAKO_BIN" queue --label res1999-a sleep 600 </dev/null >/dev/null 2>>"$TMP/wrapper.err" &
JOB_A=$!
for _ in $(seq 1 100); do
  q --json 2>/dev/null | grep -q '"phase": "running"' && break
  sleep 0.1
done
TAKO_PANE_ID="$PANE" TAKO_TOKEN="$ISO_TOKEN" TAKO_QUEUE_SOCKET="$QSOCK" "$TAKO_BIN" queue --label res1999-b sleep 600 </dev/null >/dev/null 2>>"$TMP/wrapper.err" &
JOB_B=$!
TAKO_TOKEN="$ISO_TOKEN" TAKO_QUEUE_SOCKET="$QSOCK" "$TAKO_BIN" queue --label res1999-c sleep 600 </dev/null >/dev/null 2>>"$TMP/wrapper.err" &
JOB_C=$!
for _ in $(seq 1 150); do
  n="$(q --json 2>/dev/null | python3 -c 'import json,sys
try:
    q=json.load(sys.stdin)["queue"]; print(len(q["running"]), len(q["waiting"]))
except Exception: print("0 0")')"
  [ "$n" = "1 2" ] && break
  sleep 0.1
done
check_eq "実行 1・待ち 2 で並んだ（A=${JOB_A} B=${JOB_B} C=${JOB_C}）" "1 2" "$n"
if [ "$n" != "1 2" ]; then
  echo "    wrapper の stderr:"; mask < "$TMP/wrapper.err" | sed -n '1,5p' | sed 's/^/      /'
  echo "並ばなかったので以降の比較は意味を持たない。止める"
  exit 1
fi

echo
echo "=== Test 1: 同じ時点の CLI と daemon の答えが一致する（observe でも読める）==="
pair_as observe
q --json > "$TMP/cli-1.json" 2>&1
CODE="$(api GET /api/resources)"
q --json > "$TMP/cli-1b.json" 2>&1
command cp -f "$TMP/body" "$TMP/api-1.json"
check_eq "observe は PC の負荷を読める" "200" "$CODE"
CLI1="$(digest "$TMP/cli-1.json")"
echo "    CLI : $CLI1"
echo "    API : $(digest "$TMP/api-1.json")"
case "$CLI1" in
  R*W*W*) pass "CLI の答えに実行 1・待ち 2 の行がある（空の答え同士を比べていない）" ;;
  *) fail "CLI の答えが空か形が違う（'${CLI1}'）" ;;
esac
check_eq "読みの間に CLI の答えが動いていない（同じ時点の比較になっている）" "$CLI1" "$(digest "$TMP/cli-1b.json")"
check_eq "受付番号・並び・段・段の出どころが CLI と一致" "$CLI1" "$(digest "$TMP/api-1.json")"
check_eq "実行中は A" "$JOB_A" "$(jqf queue.running.0.id)"
check_eq "待ちの 1 番目は B（ペインの既定 normal）" "$JOB_B:normal" "$(jqf queue.waiting.0.id):$(jqf queue.waiting.0.priority)"
check_eq "待ちの 2 番目は C（ペインの外 = low）" "$JOB_C:low" "$(jqf queue.waiting.1.id):$(jqf queue.waiting.1.priority)"
check_eq "誰の欄（ペイン）が載る" "$PANE" "$(jqf queue.running.0.pane)"
# 注入は調停役の割り当ての判断にだけ効く（status の machine は実測）。CLI と同じ値かを見る
check_eq "機の値（メモリの全体・メモリ圧）が CLI と同じ" \
  "$(jqf_in "$TMP/cli-1.json" machine.memory_mb.total):$(jqf_in "$TMP/cli-1.json" machine.pressure)" \
  "$(jqf machine.memory_mb.total):$(jqf machine.pressure)"
check_eq "札の fifo のパスを端末へ配らない" "null" "$(jqf jobserver)"
python3 -c 'import json,sys
a=json.load(open(sys.argv[1]))["queue"]; c=json.load(open(sys.argv[2]))["queue"]
# 時刻とともに動く欄（経過・待ち・心拍の覚え）を除いて節ごと一致するか
def strip(rows):
    out=[]
    for r in rows:
        r=dict(r)
        for k in ("ran_s","waited_s","cpu_percent","memory_mb","peak_mb","pending_mb","eta","start_s","start_text","text","reason"):
            r.pop(k,None)
        out.append(r)
    return out
ok = strip(a["running"])==strip(c["running"]) and strip(a["waiting"])==strip(c["waiting"]) and a["limits"]==c["limits"]
sys.exit(0 if ok else 1)' "$TMP/api-1.json" "$TMP/cli-1.json" \
  && pass "行の中身（ラベル・ペイン・タブ・役割・段の出どころ・上限）も CLI と一致" \
  || fail "行の中身が CLI とずれている"

echo
echo "=== Test 2: read / interact の端末の変更は 403・PC の答えは変わらない ==="
CODE="$(api POST "/api/resources/jobs/$JOB_C/priority" '{"priority":"top"}')"
check_eq "observe の段の変更は 403" "403" "$CODE"
check_contains "足りない role を名指す（表示言語の RoleTooLow）" "$(jqf error)" "manage"
CODE="$(api POST "/api/resources/jobs/$JOB_C/cancel" '{}')"
check_eq "observe の取り消しは 403" "403" "$CODE"
pair_as interact
CODE="$(api POST "/api/resources/panes/$PANE/priority" '{"priority":"low"}')"
check_eq "interact のペインの既定の変更は 403" "403" "$CODE"
CODE="$(api POST "/api/resources/jobs/$JOB_B/cancel" '{}')"
check_eq "interact の取り消しは 403" "403" "$CODE"
q --json > "$TMP/cli-2.json" 2>&1
check_eq "CLI の並びと段は変わっていない" "$CLI1" "$(digest "$TMP/cli-2.json")"
kill -0 "$JOB_B" 2>/dev/null && kill -0 "$JOB_C" 2>/dev/null \
  && pass "断った取り消しで wrapper は止まっていない" || fail "断ったはずの取り消しで wrapper が止まった"

echo
echo "=== Test 3: manage が API で変えた段が CLI に見える ==="
pair_as manage
CODE="$(api POST "/api/resources/jobs/$JOB_C/priority" '{"priority":"high"}')"
check_eq "manage の段の変更は通る" "200" "$CODE"
check_eq "応答に待ちの番目が載る（上げた C が先頭）" "1" "$(jqf position)"
q --json > "$TMP/cli-3.json" 2>&1
check_eq "CLI でも C が待ちの先頭・段 high・出どころ job" "$JOB_C:high:1:job" \
  "$(jqf_in "$TMP/cli-3.json" queue.waiting.0.id):$(jqf_in "$TMP/cli-3.json" queue.waiting.0.priority):$(jqf_in "$TMP/cli-3.json" queue.waiting.0.position):$(jqf_in "$TMP/cli-3.json" queue.waiting.0.priority_from)"
CODE="$(api POST "/api/resources/panes/$PANE/priority" '{"priority":"low"}')"
check_eq "manage のペインの既定の変更は通る" "200" "$CODE"
q --json > "$TMP/cli-3b.json" 2>&1
check_eq "CLI でもペインの既定が low（B の pane_priority）" "low" \
  "$(python3 -c 'import json,sys
q=json.load(open(sys.argv[1]))["queue"]
print(next(r.get("pane_priority") for r in q["waiting"] if r["id"]==int(sys.argv[2])))' "$TMP/cli-3b.json" "$JOB_B")"
# 戻す（以降の PWA の確かめを素の並びで始める）
api POST "/api/resources/jobs/$JOB_C/priority" '{"priority":"default"}' >/dev/null
api POST "/api/resources/panes/$PANE/priority" '{"priority":"default"}' >/dev/null
q --json > "$TMP/cli-3c.json" 2>&1
check_eq "default で既定の並びへ戻る" "$CLI1" "$(digest "$TMP/cli-3c.json")"
CODE="$(api POST "/api/resources/jobs/$JOB_C/priority" '{"priority":"urgent"}')"
check_eq "読めない段は 400（読みは調停役の 1 実装）" "400" "$CODE"
CODE="$(api POST "/api/resources/jobs/$JOB_C/priority" '{}')"
check_eq "段の無い変更は 400" "400" "$CODE"
CODE="$(api POST "/api/resources/jobs/999999/cancel" '{}')"
check_eq "居ない受付番号は 404" "404" "$CODE"
check_eq "理由の種別は gone" "gone" "$(jqf kind)"

echo
echo "=== Test 4: 表に無い操作は manage でも 404・メソッド違いは 405 ==="
CODE="$(api POST /api/resources/limits '{"max_jobs":9}')"
check_eq "上限の書き換えは受け付けない" "404" "$CODE"
check_eq "上限は変わっていない" "1" "$(q --json 2>/dev/null | python3 -c 'import json,sys; print(json.load(sys.stdin)["queue"]["limits"]["max_jobs"])')"
CODE="$(api GET "/api/resources/jobs/$JOB_C/cancel")"
check_eq "メソッド違いは 405" "405" "$CODE"

echo
echo "=== Test 5: PWA（chromium）が同じ並びを描き、UI から上げた段が CLI に見える ==="
q --json > "$TMP/cli-5a.json" 2>&1
PW_LOG="$TMP/pw.log"
pw_rc=0
(
  cd "$REPO_ROOT/web/tako-remote" &&
  HOME="$REAL_HOME" PATH="$NODE_DIR:/usr/bin:/bin:/opt/homebrew/bin:/usr/local/bin" \
  TAKO_PWA_PORT="${TAKO_PWA_PORT:-5199}" TAKO_EVIDENCE_DIR="$DUMP_DIR" \
  TAKO_1999_LIVE_BASE="$BASE" TAKO_1999_LIVE_HOST="$HOST_HDR" TAKO_1999_LIVE_RAISE="$JOB_C" \
  npx playwright test e2e/resources-1999-live.spec.js --reporter=line --output="$TMP/pw-out"
) > "$PW_LOG" 2>&1 || pw_rc=$?
q --json > "$TMP/cli-5b.json" 2>&1
grep -E "passed|failed|skipped|Error" "$PW_LOG" | mask | sed 's/^/    /' | sed -n '1,8p'
check_eq "Playwright（chromium）の live spec が通る" "0" "$pw_rc"
pick() { grep -o "TAKO_1999_ROWS $1 .*" "$PW_LOG" | head -1 | sed "s/^TAKO_1999_ROWS $1 //"; }
pick before > "$TMP/pwa-before.json"
pick after > "$TMP/pwa-after.json"
pwa_digest() { python3 -c 'import json,sys
try: rows=json.load(open(sys.argv[1]))
except Exception: print("(読めない)"); sys.exit(0)
print(" ".join(("R%s:%s" % (r["id"], r["priority"])) if r["phase"]=="running" else ("W%s:%s:%s" % (r["id"], r["priority"], r["position"])) for r in rows))' "$1"; }
cli_short() { python3 -c 'import json,sys
q=json.load(open(sys.argv[1]))["queue"]
print(" ".join(["R%s:%s" % (r["id"], r["priority"]) for r in q["running"]] + ["W%s:%s:%s" % (r["id"], r["priority"], r["position"]) for r in q["waiting"]]))' "$1"; }
echo "    CLI（PWA の前）: $(cli_short "$TMP/cli-5a.json")"
echo "    PWA（上げる前）: $(pwa_digest "$TMP/pwa-before.json")"
echo "    PWA（上げた後）: $(pwa_digest "$TMP/pwa-after.json")"
echo "    CLI（PWA の後）: $(cli_short "$TMP/cli-5b.json")"
check_eq "PWA の #/resources の並びと段が CLI と一致（上げる前）" "$(cli_short "$TMP/cli-5a.json")" "$(pwa_digest "$TMP/pwa-before.json")"
check_eq "PWA の UI から上げた段が CLI に見える（C が top で待ちの先頭）" "$JOB_C:top:1:job" \
  "$(jqf_in "$TMP/cli-5b.json" queue.waiting.0.id):$(jqf_in "$TMP/cli-5b.json" queue.waiting.0.priority):$(jqf_in "$TMP/cli-5b.json" queue.waiting.0.position):$(jqf_in "$TMP/cli-5b.json" queue.waiting.0.priority_from)"
check_eq "PWA の上げた後の並びも CLI と一致" "$(cli_short "$TMP/cli-5b.json")" "$(pwa_digest "$TMP/pwa-after.json")"
check_eq "#/ のカードの件数" "実行 1 · 待ち 2" "$(pick home | python3 -c 'import json,sys; print(json.loads(sys.stdin.read() or "\"\""))')"
check_contains "PWA の誰の欄にペインが出る" "$(cat "$TMP/pwa-before.json")" "ペイン $PANE"

echo
echo "=== Test 6: manage の取り消しで wrapper が 125 で終わる ==="
CODE="$(api POST "/api/resources/jobs/$JOB_B/cancel" '{}')"
check_eq "manage の取り消しは通る" "200" "$CODE"
check_eq "応答は取り消しを言う" "true" "$(jqf cancelled)"
b_rc=""
for _ in $(seq 1 150); do
  if ! kill -0 "$JOB_B" 2>/dev/null; then wait "$JOB_B"; b_rc=$?; break; fi
  sleep 0.1
done
check_eq "取り消した待ちの wrapper の終了コードは 125" "125" "$b_rc"
# 止まったのを見たときだけ後片付けの対象から外す（見ていなければ cleanup が止める）
[ -n "$b_rc" ] && JOB_B=""
q --json > "$TMP/cli-6.json" 2>&1
check_eq "CLI の待ちから B が消えた" "1" "$(python3 -c 'import json,sys; print(len(json.load(open(sys.argv[1]))["queue"]["waiting"]))' "$TMP/cli-6.json")"

echo
echo "=== Test 7: 診断ログと監査に表示名を出さない ==="
PERSIST="$TAKO_DATA_DIR/persist.log"
AUDIT="$TAKO_REMOTE_STATE_DIR/audit.log"
check_contains "persist.log に端末・操作・受付番号・段・結果が残る" "$(cat "$PERSIST" 2>/dev/null)" \
  "リモートから順番待ちを操作: 端末=nFAKE1999 操作=job_priority 受付番号=$JOB_C 段=high 結果=ok"
check_contains "居ない受付番号の取り消しも残る（gone）" "$(cat "$PERSIST" 2>/dev/null)" "受付番号=999999 結果=gone"
check_contains "監査に queue_cancel が残る" "$(cat "$AUDIT" 2>/dev/null)" "queue_cancel"
for f in "$PERSIST" "$AUDIT"; do
  if grep -q "res1999-" "$f" 2>/dev/null || grep -q "sleep 600" "$f" 2>/dev/null; then
    fail "$(basename "$f") に表示名が漏れている"
  else
    pass "$(basename "$f") に表示名が漏れていない"
  fi
done

echo
echo "=== Test 8: GUI が居ないとき、読む経路は daemon が機の状態だけ返す ==="
stop_isolated_gui "$APP_PID"
APP_PID=""
CODE=""
for _ in $(seq 1 50); do
  CODE="$(api GET /api/resources)"
  [ "$CODE" = "200" ] && [ "$(jqf gui)" = "false" ] && break
  sleep 0.2
done
check_eq "GUI 不在でも読む経路は 200" "200" "$CODE"
check_eq "gui: false" "false" "$(jqf gui)"
check_contains "注記が載る（daemon の表示言語）" "$(jqf note)" "順番待ちは無効"
case "$(jqf machine.memory_mb.total)" in
  ''|null|*[!0-9]*) fail "機の状態を daemon が自分で採れていない" ;;
  *) pass "機の状態を daemon が自分で採った（total $(jqf machine.memory_mb.total) MB）" ;;
esac
CODE="$(api POST "/api/resources/jobs/$JOB_C/cancel" '{}')"
check_eq "GUI 不在の変更は 503" "503" "$CODE"
check_eq "理由の種別は app_unreachable" "app_unreachable" "$(jqf kind)"

echo "== 画像の個人情報（ユーザー名・ホームパス）=="
if command -v tesseract >/dev/null 2>&1 && find "$DUMP_DIR" -name '*.png' | grep -q .; then
  leak=0
  while IFS= read -r f; do
    ocr="$(tesseract "$f" - 2>/dev/null)"
    if grep -qiE "${MASK_USER}|/Users/" <<<"$ocr"; then
      leak=1
      echo "    写り込み: $(basename "$f")"
    fi
  done < <(find "$DUMP_DIR" -name '*.png')
  [ "$leak" = 0 ] && pass "画像の文字（OCR）にユーザー名・ホームパスが無い" || fail "画像にユーザー名かホームパスが写っている"
else
  echo "    （tesseract が無い・画像が無い = 目視で確かめる）"
fi

echo
if [ -n "${KEEP:-}" ] || [ -n "${RES1999_DUMP_DIR:-}" ]; then
  echo "画像: $DUMP_DIR" | mask
fi
echo "PASS=$PASS FAIL=$FAIL${UNMEASURED:+ 未実測:$UNMEASURED}"
[ "$FAIL" -eq 0 ] || exit 1
tako_exit 0
