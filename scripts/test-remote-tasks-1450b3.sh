#!/usr/bin/env bash
# test-remote-tasks-1450b3.sh — スマホからユーザー向けタスクを片付ける実経路テスト（#1450 B3）
#
# 偽の tailscale CLI + 隔離した HOME / state / data で **実 daemon（tako remote start）+
# 実 tako-app** を走らせ、HTTP を実際に叩いて
#   ① role の gate（observe は読めるが返答 / 完了 / 却下は 403・interact は通る）
#   ② 添付の解決が role で変わる（manage = `fs` ルート / interact = ツリーに出ているフォルダだけ）
#   ③ **添付が実際に落ちてくる**（数百 MB 相当の偽ファイルで Content-Length と実転送量）
#   ④ 返答が `tako todo show` の responses に載り、配送の顛末が残る（無言にならない）
#   ⑤ 完了 / 却下で一覧から消え、`tako todo list` と一致する
#   ⑥ エッジ（0 件 / 消えた添付 / 知らない id / 壊れた decision / 表に無い受け口）
#   ⑦ A/B（`TAKO_1450B3_LEGACY=1` で B3 以前 = 404 に戻る）
# を実測する。
#
# **本番の tailscale / serve 設定 / remote デーモン / tako 設定 / user-tasks.yaml には
# 一切触らない**（HOME / state / data / orchestrator は mktemp 配下、tailscale は偽物）。
# 添付も偽の木の中だけで、実ユーザー名・実ホームパスは出力に入らない（#927）。
#
# 使い方: bash scripts/test-remote-tasks-1450b3.sh
set -uo pipefail

# **本番 GUI を指す env を最初に落とす**。tako のペインの中から走らせると
# `TAKO_SOCKET` / `TAKO_TOKEN` / `TAKO_PANE_ID` が継承され、CLI が隔離インスタンスでは
# なく**ユーザーの本番 GUI** を触る（#1449 / #1450 B1 の検証で実際に起きた）。
# 下の「タブ 1 枚か」の確認が第 2 の網
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PASS=0
FAIL=0

pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
fail() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }
check_eq() {
  if [ "$2" = "$3" ]; then pass "$1"; else fail "$1（期待 '${2}' / 実際 '${3}'）"; fi
}
check_contains() {
  case "$2" in
    *"$3"*) pass "$1" ;;
    *) fail "$1（'${3}' を含まない）" ;;
  esac
}
check_one_of() {
  # $1=説明 $2=実際 $3...=許す値
  local what="$1" got="$2"; shift 2
  for allowed in "$@"; do
    if [ "$got" = "$allowed" ]; then pass "${what}（${got}）"; return; fi
  done
  fail "${what}（実際 '${got}' はどれにも当たらない: $*）"
}
# 「その値でない」ことを確かめる（空・null・なし は常に不合格）
check_is_set_and_ne() {
  if [ -z "$2" ] || [ "$2" = "null" ] || [ "$2" = "なし" ]; then
    fail "$1（値が無い: '${2}'）"
  elif [ "$2" = "$3" ]; then
    fail "$1（'${3}' であってはならない）"
  else
    pass "$1"
  fi
}

# `/tmp` 直下に取る（daemon の待ち受けと tmux が sun_path の上限に当たらない長さ。#1441）
TMP="$(mktemp -d /tmp/tako-1450b3-XXXXXX)"
APP_PID=""
TMUX_SOCKET="tako-1450b3-$$"
cleanup() {
  if [ -n "${TAKO_REMOTE_STATE_DIR:-}" ] && [ -s "$TAKO_REMOTE_STATE_DIR/tako-remote.pid" ]; then
    "$TAKO_BIN" remote stop >/dev/null 2>&1 || true
  fi
  # 明示 pid だけを落とす（pkill / killall は本番 GUI にも当たる）
  stop_isolated_gui "$APP_PID"
  tmux -L "$TMUX_SOCKET" kill-server >/dev/null 2>&1 || true
  rm -rf "$TMP"
}
trap cleanup EXIT

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
NODE="fake-1450b3.tailfake.ts.net"
case "${1:-}" in
  --version) echo "1.0.0-fake"; exit 0 ;;
  status)
    printf '{"BackendState":"Running","Version":"1.0.0-fake","CertDomains":["%s"],"Self":{"DNSName":"%s."}}\n' "$NODE" "$NODE"
    exit 0 ;;
  whois)
    printf '{"Node":{"StableID":"nFAKE1450","Name":"iphone.tailfake.ts.net.","Hostinfo":{"Hostname":"iPhone"}},"UserProfile":{"LoginName":"tester@example.com"}}\n'
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

# --- 隔離した環境 -------------------------------------------------------------
export HOME="$TMP/home"
mkdir -p "$HOME/dev/proj"
# 添付の置き場は **HOME の外**（tako のツリーには HOME 配下しか出ないので、
# ここに置くと「interact では解決できない添付」を実物で作れる）
MEDIA="$TMP/media"
mkdir -p "$MEDIA"
# 偽の動画。**数百 MB 相当**をスパースで作る（ストリーミングの実測用）
BIG_MB=300
dd if=/dev/zero of="$MEDIA/v6.mp4" bs=1 count=0 seek=$((BIG_MB * 1024 * 1024)) 2>/dev/null
BIG_BYTES=$((BIG_MB * 1024 * 1024))
# ツリーに出るフォルダの中の添付（interact 端末はこちらだけ落とせる）
echo "スクリーンショットの代わり" > "$HOME/dev/proj/shot.png"

# 起動される `tako master` / claude が本物を掴まないように PATH を絞る
mkdir -p "$TMP/bin"
ln -sf "$TAKO_BIN" "$TMP/bin/tako"
cat > "$TMP/bin/claude" <<'STUB'
#!/usr/bin/env bash
echo "fake claude (1450b3)"
exec cat
STUB
chmod +x "$TMP/bin/claude"
export PATH="$TMP/bin:/usr/bin:/bin:/usr/sbin:/sbin"

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
export TAKO_TAILSCALE_BIN="$FAKE_TS"
# #841: ループバック TCP では接続元プロセスを検証する。この隔離環境で serve の
# 代わりに繋いでくるのは curl だと名前で宣言する（ゲートは本番と同じ経路を通る）
export TAKO_REMOTE_TRUSTED_PEER_NAMES="curl"
# #1452: ペアリングの**承認**は「呼び出し元が tako-app か」も見る（管理トークンは
# 0600 = 同一ユーザーなら読めるので、トークンだけでは GUI と CLI / AI を区別できない）。
# このテストは curl で承認を撃つので、**この隔離環境では curl を GUI 側として名乗る**
export TAKO_REMOTE_TRUSTED_ADMIN_NAMES="curl"
mkdir -p "$TAKO_DATA_DIR" "$TAKO_DISCOVERY_DIR" "$TAKO_REMOTE_STATE_DIR" "$TAKO_ORCHESTRATOR_DIR"
# 本番へ書かない不変条件（env が効いていなければここで落とす）
for d in "$HOME" "$TAKO_DATA_DIR" "$TAKO_REMOTE_STATE_DIR" "$TAKO_ORCHESTRATOR_DIR"; do
  case "$d" in
    "$TMP"/*) : ;;
    *) echo "隔離されていないディレクトリ: $d"; exit 1 ;;
  esac
done

# --- 起動 ---------------------------------------------------------------------
echo "=== 準備: 隔離 tako-app と daemon を起動 ==="
launch_isolated_gui "$TMP/app.log"
APP_PID="$ISOLATED_GUI_PID"
wait_isolated_gui "$TMP/app.log" || exit 1
# **繋がった先が隔離インスタンスであることを確かめる**（外れたまま進むと本番を触る）
BOOT_TABS="$("$TAKO_BIN" list | python3 -c 'import json,sys; print(len(json.load(sys.stdin)["tabs"]))')"
if [ "$BOOT_TABS" != "1" ]; then
  echo "繋がった先が隔離インスタンスではない（タブ ${BOOT_TABS} 枚）。"
  echo "本番 GUI を触る恐れがあるので中止する（TAKO_SOCKET などが残っていないか確認）。"
  exit 1
fi
pass "隔離 tako-app が起動して CLI から見える（pid ${APP_PID}・タブ 1 枚）"

HOST_HDR="fake-1450b3.tailfake.ts.net"
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
body() { cat "$TMP/body"; }
jqf() { python3 -c 'import json,sys
d=json.load(open(sys.argv[1]))
cur=d
for k in sys.argv[2].split("."):
    if isinstance(cur,list):
        try: cur=cur[int(k)]
        except (ValueError,IndexError): cur=None; break
    elif isinstance(cur,dict) and k in cur: cur=cur[k]
    else: cur=None; break
print("null" if cur is None else (str(cur).lower() if isinstance(cur,bool) else cur))' "$TMP/body" "$1"; }
# タスク 1 件を id で引いて、その中のキーを読む（並び順に依存しない）
task_field() { python3 -c 'import json,sys
d=json.load(open(sys.argv[1]))
items=d.get("tasks") if isinstance(d,dict) and "tasks" in d else [d]
for t in items or []:
    if t.get("id")==sys.argv[2]:
        cur=t
        for k in sys.argv[3].split("."):
            if isinstance(cur,list):
                try: cur=cur[int(k)]
                except (ValueError,IndexError): cur=None; break
            elif isinstance(cur,dict) and k in cur: cur=cur[k]
            else: cur=None; break
        print("null" if cur is None else (str(cur).lower() if isinstance(cur,bool) else cur)); raise SystemExit
print("なし")' "$TMP/body" "$1" "$2"; }

start_daemon() {
  if ! "$TAKO_BIN" remote start > "$TMP/daemon.log" 2>&1; then
    echo "daemon を起動できなかった:"; cat "$TMP/daemon.log"; exit 1
  fi
  for _ in $(seq 1 200); do
    [ -s "$TAKO_REMOTE_STATE_DIR/tako-remote.port" ] && break
    sleep 0.1
  done
  PORT="$(cat "$TAKO_REMOTE_STATE_DIR/tako-remote.port" 2>/dev/null || true)"
  [ -n "$PORT" ] || { echo "daemon の port が読めない:"; cat "$TMP/daemon.log"; exit 1; }
  ADMIN_TOKEN="$(cat "$TAKO_REMOTE_STATE_DIR/tako-remote.token")"
  BASE="http://127.0.0.1:$PORT"
  for _ in $(seq 1 100); do
    if [ "$(api GET /api/health)" = "200" ]; then break; fi
    sleep 0.1
  done
}

# 端末の role を**その値に据える**。
# `POST /api/pair` + 承認は「上げる」向きしか効かない（降格は保留が作られない = 実測）ので、
# 下げるときは #1452 の管理経路（`/api/admin/devices/role`）を使う。
# どちらも本番と同じゲートを通る（この隔離環境では curl を GUI 側として名乗っている）
pair_as() {
  api POST /api/pair "{\"name\":\"iPhone\",\"role\":\"$1\"}" >/dev/null
  curl -s -o /dev/null -X POST "$BASE/api/admin/pair/approve" \
    -H "X-Tako-Admin: $ADMIN_TOKEN" -H 'Content-Type: application/json' \
    -d "{\"device_id\":\"nFAKE1450\",\"role\":\"$1\"}"
  curl -s -o /dev/null -X POST "$BASE/api/admin/devices/role" \
    -H "X-Tako-Admin: $ADMIN_TOKEN" -H 'Content-Type: application/json' \
    -d "{\"device_id\":\"nFAKE1450\",\"role\":\"$1\"}"
  # 据わったことを確かめてから先へ進む（外れたまま検査すると別の role を測る）
  api GET /api/me >/dev/null
  ACTUAL_ROLE="$(jqf role)"
  if [ "$ACTUAL_ROLE" != "$1" ]; then
    fail "role を ${1} に据えられなかった（実際 ${ACTUAL_ROLE}）"
  fi
}

start_daemon
pass "隔離 daemon が起動した（loopback port ${PORT}）"

# --- 起票（CLI = B1 の 1 実装）------------------------------------------------
#
# **注意**: #1452 の権限リクエストは `tako todo`（kind=permission）を 1 件起票する。
# このテストは role を何度も据え直すので、**全体の件数では検査せず id で見る**
# （他機能が起票したものを数えてしまうと、無関係な変更でこのテストが落ちる）。
echo
echo "=== 準備: タスクを起票する ==="
pair_as manage
add_task() { "$TAKO_BIN" todo add "$@" --json > "$TMP/add.log" 2>&1 || { echo "起票に失敗:"; cat "$TMP/add.log"; exit 1; }; }
add_task "解説動画 v6 を YouTube へ投稿" \
  --kind post --project tako \
  --body $'# 手順\n\n1. 動画を落とす\n2. 投稿文を貼る' \
  --attach "$MEDIA/v6.mp4" \
  --attach "$MEDIA/gone.png" \
  --copy-text "投稿文=tako v0.8 を出しました" \
  --copy-text "タグ=#tako #AI" \
  --link "https://example.com/tako"
POST_ID="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["id"])' "$TMP/add.log")"
add_task "PR のレビュー" --kind review --attach "$HOME/dev/proj/shot.png"
REVIEW_ID="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["id"])' "$TMP/add.log")"
# role ごとの「完了」を独立に試すための使い捨て（互いの前提を壊さない）
for role in observe interact manage; do
  add_task "使い捨て（${role}）" --kind other
  eval "SCRATCH_${role}=\"$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["id"])' "$TMP/add.log")\""
done
pass "CLI から起票した（投稿 ${POST_ID} / レビュー ${REVIEW_ID} + 使い捨て 3 件）"

echo
echo "=== Test 1: role の gate（読むのは observe・動かすのは interact 以上） ==="
for role in observe interact manage; do
  pair_as "$role"
  CODE="$(api GET /api/tasks)"
  check_eq "${role} は一覧を引ける（Issue #1450 の記載）" "200" "$CODE"
  check_eq "${role} の一覧に投稿タスクが載る" "$POST_ID" "$(task_field "$POST_ID" id)"
  # 返答（needs_change は open のままなので、後続の前提を壊さない）
  CODE="$(api POST "/api/tasks/$POST_ID/respond" '{"decision":"needs_change","comment":"gate"}')"
  eval "SCRATCH=\$SCRATCH_${role}"
  CODE2="$(api POST "/api/tasks/$SCRATCH/done")"
  if [ "$role" = "observe" ]; then
    check_eq "observe の返答は 403" "403" "$CODE"
    check_eq "observe の完了は 403" "403" "$CODE2"
    check_eq "observe が押しても状態は変わらない" "open" "$("$TAKO_BIN" todo show "$SCRATCH" --json | python3 -c 'import json,sys; print(json.load(sys.stdin)["status"])')"
  else
    check_eq "${role} の返答は通る" "200" "$CODE"
    check_eq "${role} の完了は通る" "200" "$CODE2"
    check_eq "${role} の完了が正本に効く" "done" "$("$TAKO_BIN" todo show "$SCRATCH" --json | python3 -c 'import json,sys; print(json.load(sys.stdin)["status"])')"
  fi
done

echo
echo "=== Test 2: 添付の解決が role で変わる（門は #1451 の 1 か所） ==="
pair_as interact
api GET /api/tasks >/dev/null
check_eq "interact はツリーの外の添付を解決できない" "false" "$(task_field "$POST_ID" attachments.0.available)"
check_eq "名前だけは出る（何が要るか分かる）" "v6.mp4" "$(task_field "$POST_ID" attachments.0.name)"
check_eq "宛先のルートは伏せられる" "null" "$(task_field "$POST_ID" attachments.0.root)"
check_eq "消えた添付は exists=false" "false" "$(task_field "$POST_ID" attachments.1.exists)"
check_eq "消えた添付は開けない" "false" "$(task_field "$POST_ID" attachments.1.available)"
check_eq "interact もツリー配下の添付は解決できる" "true" "$(task_field "$REVIEW_ID" attachments.0.available)"
check_is_set_and_ne "ツリー由来のルートが宛先（fs ではない）" "$(task_field "$REVIEW_ID" attachments.0.root)" "fs"

pair_as manage
api GET /api/tasks >/dev/null
check_eq "manage は fs ルートでツリーの外も解決できる" "true" "$(task_field "$POST_ID" attachments.0.available)"
check_eq "解決先のルートは fs" "fs" "$(task_field "$POST_ID" attachments.0.root)"
check_eq "サイズが載る（共有の可否を画面が決められる）" "$BIG_BYTES" "$(task_field "$POST_ID" attachments.0.size)"
REL_ATT="$(task_field "$POST_ID" attachments.0.path_rel)"
check_contains "相対パスで返る（URL に絶対パスを載せない）" "$REL_ATT" "media/v6.mp4"
case "$REL_ATT" in
  /*|なし|null) fail "path_rel が相対パスでない（'${REL_ATT}'）" ;;
  *) pass "path_rel は相対パス" ;;
esac

echo
echo "=== Test 3: 添付が実際に落ちてくる（数百 MB をストリーミング） ==="
ROOT_ID="$(task_field "$POST_ID" attachments.0.root)"
REL="$(task_field "$POST_ID" attachments.0.path_rel)"
DL_OUT="$(curl -s -o /dev/null -D "$TMP/headers" -w '%{http_code} %{size_download}' \
  -H "X-Forwarded-For: 100.64.0.7" -H "X-Forwarded-Host: $HOST_HDR" \
  "$BASE/api/files/download?root=$ROOT_ID&path=$(python3 -c 'import urllib.parse,sys; print(urllib.parse.quote(sys.argv[1]))' "$REL")")"
DL_CODE="${DL_OUT%% *}"
DL_SIZE="${DL_OUT##* }"
HEADERS="$(tr -d '\r' < "$TMP/headers")"
check_eq "ダウンロードが 200 で返る" "200" "$DL_CODE"
check_eq "実際に落ちたバイト数が添付と一致する（${BIG_MB} MB）" "$BIG_BYTES" "$DL_SIZE"
# 長さの伝え方は 2 通りある（Content-Length か chunked）。**どちらかであること**を見る
CL="$(printf '%s\n' "$HEADERS" | grep -i '^content-length:' | awk '{print $2}')"
TE="$(printf '%s\n' "$HEADERS" | grep -i '^transfer-encoding:' | awk '{print $2}')"
if [ "$CL" = "$BIG_BYTES" ]; then
  pass "Content-Length が添付と一致する（${CL}）"
elif [ "$TE" = "chunked" ]; then
  pass "分割応答（Transfer-Encoding: chunked）で丸ごと届く = メモリに載せていない"
else
  fail "長さの伝え方が無い（Content-Length='${CL}' / Transfer-Encoding='${TE}'）"
fi
check_contains "添付として保存させる（Content-Disposition）" "$HEADERS" "attachment"
check_contains "キャッシュさせない" "$HEADERS" "no-store"
# 転送中に daemon がメモリへ丸ごと載せていない（**RSS が添付より桁で小さい**）
DAEMON_PID="$(head -1 "$TAKO_REMOTE_STATE_DIR/tako-remote.pid" 2>/dev/null | awk '{print $1}' | tr -dc '0-9')"
RSS_KB="$(ps -o rss= -p "${DAEMON_PID:-0}" 2>/dev/null | tr -dc '0-9' || true)"
if [ -z "$RSS_KB" ]; then
  fail "daemon（pid '${DAEMON_PID}'）の常駐量を測れない"
elif [ "$RSS_KB" -lt $((BIG_MB * 1024 / 2)) ]; then
  pass "daemon の常駐量が添付より桁で小さい（${RSS_KB} KB < $((BIG_MB * 1024 / 2)) KB = ストリーミング）"
else
  fail "daemon が添付を丸ごとメモリへ載せている疑い（RSS ${RSS_KB} KB）"
fi
# 解決できない添付は**直接叩いても**落とせない（門は #1451 の 1 か所のまま）
pair_as interact
CODE="$(api GET "/api/files/download?root=fs&path=$(python3 -c 'import urllib.parse,sys; print(urllib.parse.quote(sys.argv[1]))' "$REL")")"
check_eq "interact が root=fs を直接叩いても 403" "403" "$CODE"

echo
echo "=== Test 4: 返答が正本に載り、配送の顛末が残る ==="
CODE="$(api POST "/api/tasks/$POST_ID/respond" '{"decision":"needs_change","comment":"サムネを差し替えてほしい"}')"
check_eq "返答が通る" "200" "$CODE"
check_eq "応答に一覧のキーが生えない（単体と一覧を見分けられる）" "False" \
  "$(python3 -c 'import json,sys; print("tasks" in json.load(open(sys.argv[1])))' "$TMP/body")"
LAST_IDX="$(python3 -c 'import json,sys; print(len(json.load(open(sys.argv[1]))["responses"]) - 1)' "$TMP/body")"
check_eq "応答に返答が載る" "needs_change" "$(task_field "$POST_ID" "responses.${LAST_IDX}.decision")"
check_eq "どこから返したかが残る（pwa）" "pwa" "$(task_field "$POST_ID" "responses.${LAST_IDX}.via")"
DELIVERY="$(task_field "$POST_ID" delivery.state)"
check_one_of "配送の顛末が残る" "$DELIVERY" "sent" "launched" "delivered" "failed"
if [ "$DELIVERY" = "failed" ]; then
  REASON="$(task_field "$POST_ID" delivery.reason)"
  if [ -z "$REASON" ] || [ "$REASON" = "null" ]; then
    fail "配送の失敗に理由が付いていない（無言になっている）"
  else
    pass "配送に失敗したら理由が付く（無言にしない）"
  fi
fi
SHOW="$("$TAKO_BIN" todo show "$POST_ID" --json 2>/dev/null)"
check_contains "tako todo show に返答が載っている" "$SHOW" "サムネを差し替えてほしい"
check_contains "via=pwa が正本に残っている" "$SHOW" "pwa"
check_contains "needs_change では閉じない" "$("$TAKO_BIN" todo list --json)" "$POST_ID"

echo
echo "=== Test 5: 完了 / 却下が一覧から消え、CLI と一致する ==="
CODE="$(api POST "/api/tasks/$POST_ID/done")"
check_eq "完了が通る" "200" "$CODE"
api GET /api/tasks >/dev/null
check_eq "一覧から消える" "なし" "$(task_field "$POST_ID" id)"
api GET "/api/tasks?all=1" >/dev/null
check_eq "all=1 なら片付いたものも読める" "done" "$(task_field "$POST_ID" status)"
CODE="$(api POST "/api/tasks/$REVIEW_ID/dismiss")"
check_eq "却下が通る" "200" "$CODE"
api GET /api/tasks >/dev/null
check_eq "却下しても一覧から消える" "なし" "$(task_field "$REVIEW_ID" id)"
# 画面が見ているものと CLI（正本）が一致する
API_OPEN="$(python3 -c 'import json,sys; d=json.load(open(sys.argv[1])); print(sorted(t["id"] for t in d["tasks"]))' "$TMP/body")"
CLI_OPEN="$("$TAKO_BIN" todo list --json | python3 -c 'import json,sys; print(sorted(t["id"] for t in json.load(sys.stdin)["tasks"]))')"
check_eq "未完了の一覧が CLI と完全に一致する" "$CLI_OPEN" "$API_OPEN"
check_eq "未完了件数も CLI と一致する" "$("$TAKO_BIN" todo list --json | python3 -c 'import json,sys; print(len(json.load(sys.stdin)["tasks"]))')" "$(jqf open_count)"

echo
echo "=== Test 6: エッジ ==="
CODE="$(api GET "/api/tasks?kind=post")"
check_eq "絞り込みの結果が 0 件でも 200（画面は「ありません」を出す）" "0" "$(jqf count)"
CODE="$(api POST "/api/tasks/u-999/done")"
check_eq "知らない id は 404" "404" "$CODE"
check_eq "理由の種別が付く" "not_found" "$(jqf kind)"
CODE="$(api POST "/api/tasks/$POST_ID/respond" '{"decision":"maybe"}')"
check_eq "壊れた decision は 400（頼み方が悪い）" "400" "$CODE"
# 表に無い受け口は**床の Manage**へ落ちる（未知の枝が弱い role へこぼれない）
CODE="$(api POST "/api/tasks/$POST_ID/purge")"
check_eq "interact は表に無い受け口で 403（床が効いている）" "403" "$CODE"
CODE="$(api GET "/api/tasks/$POST_ID/respond")"
check_eq "interact はメソッド違いでも 403（床が効いている）" "403" "$CODE"
pair_as manage
CODE="$(api POST "/api/tasks/$POST_ID/purge")"
check_eq "manage でも表に無い受け口は 404（受け口そのものが無い）" "404" "$CODE"
CODE="$(api GET "/api/tasks/$POST_ID/respond")"
check_eq "manage ならメソッド違いは 405" "405" "$CODE"
# 監査ログに何をしたかが残る（中身は残さない）
AUDIT="$TAKO_REMOTE_STATE_DIR/audit.log"
if [ -f "$AUDIT" ]; then
  check_contains "監査に task_respond が残る" "$(cat "$AUDIT")" "task_respond"
  check_contains "監査に task_done が残る" "$(cat "$AUDIT")" "task_done"
  check_contains "監査に task_dismiss が残る" "$(cat "$AUDIT")" "task_dismiss"
  if grep -q "サムネ" "$AUDIT"; then
    fail "監査ログにコメント本文が漏れている"
  else
    pass "監査ログに本文・コメントが漏れていない"
  fi
  if grep -q "media/v6.mp4" "$AUDIT"; then
    fail "監査ログに添付のパスが漏れている"
  else
    pass "監査ログに添付のパスが漏れていない"
  fi
else
  fail "監査ログが無い"
fi

echo
echo "=== Test 7: A/B（TAKO_1450B3_LEGACY=1 で B3 以前へ戻る） ==="
"$TAKO_BIN" remote stop >/dev/null 2>&1 || true
for _ in $(seq 1 50); do
  [ -s "$TAKO_REMOTE_STATE_DIR/tako-remote.pid" ] || break
  sleep 0.1
done
TAKO_1450B3_LEGACY=1 "$TAKO_BIN" remote start > "$TMP/daemon-legacy.log" 2>&1 || {
  echo "legacy 腕の daemon を起動できない:"; cat "$TMP/daemon-legacy.log"; }
for _ in $(seq 1 200); do
  [ -s "$TAKO_REMOTE_STATE_DIR/tako-remote.port" ] && break
  sleep 0.1
done
PORT="$(cat "$TAKO_REMOTE_STATE_DIR/tako-remote.port")"
ADMIN_TOKEN="$(cat "$TAKO_REMOTE_STATE_DIR/tako-remote.token")"
BASE="http://127.0.0.1:$PORT"
for _ in $(seq 1 100); do
  if [ "$(api GET /api/health)" = "200" ]; then break; fi
  sleep 0.1
done
pair_as manage
CODE="$(api GET /api/tasks)"
check_eq "legacy 腕では一覧が 404（スマホからは見えない）" "404" "$CODE"
CODE="$(api POST "/api/tasks/$POST_ID/done")"
check_eq "legacy 腕では操作も 404" "404" "$CODE"
check_contains "CLI からは今までどおり読める（正本は生きている）" "$("$TAKO_BIN" todo list --all --json)" "$POST_ID"

echo
echo "================================"
echo "  PASS: $PASS  /  FAIL: $FAIL"
echo "================================"
[ "$FAIL" -eq 0 ]
