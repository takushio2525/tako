#!/usr/bin/env bash
# test-remote-attachment-preview-1472.sh — 添付をその場で見せる実経路テスト（#1472 B）
#
# 偽の tailscale CLI + 隔離した HOME / state / data で **実 daemon（tako remote start）+
# 実 tako-app** を走らせ、HTTP を実際に叩いて
#   ① 添付の応答に種別（`preview`）が載る（画像 / 動画 / それ以外）
#   ② `?disposition=inline` で **`Content-Type` が実体の型**・`Content-Disposition: inline`
#      （既定は今までどおり `application/octet-stream` + `attachment`）
#   ③ `Range` 付き GET が **206 + Content-Range**（先頭 2 バイト / 途中 / 末尾チャンク）で、
#      **中身が実体のその位置と一致する**
#   ④ 満たせない範囲は 416・単位違い / 複数範囲は 200（全体）へ倒れる
#   ⑤ **300 MB を Range で取っても daemon の常駐量が添付より桁で小さい**（ストリーミング）
#   ⑥ 認可は 1 実装のまま（observe は inline も Range も 403・interact はツリー外を落とせない）
#   ⑦ 空白 / 日本語のファイル名が往復する
# を実測する。
#
# **本番の tailscale / serve 設定 / remote デーモン / tako 設定 / user-tasks.yaml には
# 一切触らない**（HOME / state / data / orchestrator は mktemp 配下、tailscale は偽物）。
# 添付も偽の木の中だけで、実ユーザー名・実ホームパスは出力に入らない（#927）。
#
# 使い方: bash scripts/test-remote-attachment-preview-1472.sh
set -uo pipefail

# **本番 GUI を指す env を最初に落とす**（tako のペインの中から走らせると継承される）
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
check_not_contains() {
  case "$2" in
    *"$3"*) fail "$1（'${3}' を含んでいる）" ;;
    *) pass "$1" ;;
  esac
}

# `/tmp` 直下に取る（daemon の待ち受けと tmux が sun_path の上限に当たらない長さ。#1441）
TMP="$(mktemp -d /tmp/tako-1472-XXXXXX)"
APP_PID=""
TMUX_SOCKET="tako-1472-$$"
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
NODE="fake-1472.tailfake.ts.net"
case "${1:-}" in
  --version) echo "1.0.0-fake"; exit 0 ;;
  status)
    printf '{"BackendState":"Running","Version":"1.0.0-fake","CertDomains":["%s"],"Self":{"DNSName":"%s."}}\n' "$NODE" "$NODE"
    exit 0 ;;
  whois)
    printf '{"Node":{"StableID":"nFAKE1472","Name":"iphone.tailfake.ts.net.","Hostinfo":{"Hostname":"iPhone"}},"UserProfile":{"LoginName":"tester@example.com"}}\n'
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

# --- バイナリ -----------------------------------------------------------------
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
# 偽の動画。**300 MB 相当**をスパースで作り、**末尾 16 バイトに目印**を書く
# （Range の末尾チャンクが「実体のその位置」を返しているかを中身で確かめるため）
BIG_MB=300
BIG_BYTES=$((BIG_MB * 1024 * 1024))
TAIL_MARK="TAKO-1472-TAILXX"   # ちょうど 16 バイト
dd if=/dev/zero of="$MEDIA/v6.mp4" bs=1 count=0 seek="$BIG_BYTES" 2>/dev/null
printf '%s' "$TAIL_MARK" | dd of="$MEDIA/v6.mp4" bs=1 seek=$((BIG_BYTES - 16)) conv=notrunc 2>/dev/null
# 先頭にも目印（先頭 Range の中身を確かめる）
printf 'HEAD0123' | dd of="$MEDIA/v6.mp4" bs=1 seek=0 conv=notrunc 2>/dev/null
ACTUAL_BYTES="$(wc -c < "$MEDIA/v6.mp4" | tr -d ' ')"

# ツリーに出るフォルダの中の添付（interact 端末はこちらだけ落とせる）
# 320x180 の PNG（e2e と同じフィクスチャ = 実体のある画像）
cp "$REPO_ROOT/web/tako-remote/e2e/fixtures/thumb.png" "$HOME/dev/proj/thumb.png"
# 空白と日本語を含む名前（URL の往復を実測する）
cp "$REPO_ROOT/web/tako-remote/e2e/fixtures/thumb.png" "$HOME/dev/proj/サムネ 01.png"
# プレビューを名乗らない種別（従来どおりの見え方が残る）
echo "# メモ" > "$HOME/dev/proj/notes.md"

# 起動される `tako master` / claude が本物を掴まないように PATH を絞る
mkdir -p "$TMP/bin"
ln -sf "$TAKO_BIN" "$TMP/bin/tako"
cat > "$TMP/bin/claude" <<'STUB'
#!/usr/bin/env bash
echo "fake claude (1472)"
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
export TAKO_REMOTE_TRUSTED_PEER_NAMES="curl"
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
check_eq "偽の動画が ${BIG_MB} MB ある" "$BIG_BYTES" "$ACTUAL_BYTES"
launch_isolated_gui "$TMP/app.log"
APP_PID="$ISOLATED_GUI_PID"
wait_isolated_gui "$TMP/app.log" || exit 1
# **繋がった先が隔離インスタンスであることを確かめる**（外れたまま進むと本番を触る）
BOOT_TABS="$("$TAKO_BIN" list | python3 -c 'import json,sys; print(len(json.load(sys.stdin)["tabs"]))')"
if [ "$BOOT_TABS" != "1" ]; then
  echo "繋がった先が隔離インスタンスではない（タブ ${BOOT_TABS} 枚）。中止する。"
  exit 1
fi
pass "隔離 tako-app が起動して CLI から見える（pid ${APP_PID}・タブ 1 枚）"

HOST_HDR="fake-1472.tailfake.ts.net"
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
urlq() { python3 -c 'import urllib.parse,sys; print(urllib.parse.quote(sys.argv[1]))' "$1"; }

# 添付を 1 回取りに行く。`$1` = URL 追加分 / `$2` = Range ヘッダ（空なら付けない）。
# ヘッダは `$TMP/headers`・本文は `$TMP/dl` に残る
dl() {
  local extra="$1" range="${2:-}"
  local -a args=(-s -o "$TMP/dl" -D "$TMP/headers" -w '%{http_code} %{size_download}'
    -H "X-Forwarded-For: 100.64.0.7" -H "X-Forwarded-Host: $HOST_HDR")
  if [ -n "$range" ]; then args+=(-H "Range: $range"); fi
  curl ${args[@]+"${args[@]}"} "$BASE/api/files/download?root=$ROOT_ID&path=$(urlq "$REL")$extra"
}
headers() { tr -d '\r' < "$TMP/headers"; }
hval() { headers | grep -i "^$1:" | head -1 | cut -d' ' -f2- | tr -d ' '; }

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

pair_as() {
  api POST /api/pair "{\"name\":\"iPhone\",\"role\":\"$1\"}" >/dev/null
  curl -s -o /dev/null -X POST "$BASE/api/admin/pair/approve" \
    -H "X-Tako-Admin: $ADMIN_TOKEN" -H 'Content-Type: application/json' \
    -d "{\"device_id\":\"nFAKE1472\",\"role\":\"$1\"}"
  curl -s -o /dev/null -X POST "$BASE/api/admin/devices/role" \
    -H "X-Tako-Admin: $ADMIN_TOKEN" -H 'Content-Type: application/json' \
    -d "{\"device_id\":\"nFAKE1472\",\"role\":\"$1\"}"
  api GET /api/me >/dev/null
  ACTUAL_ROLE="$(jqf role)"
  if [ "$ACTUAL_ROLE" != "$1" ]; then
    fail "role を ${1} に据えられなかった（実際 ${ACTUAL_ROLE}）"
  fi
}

start_daemon
pass "隔離 daemon が起動した（loopback port ${PORT}）"

echo
echo "=== 準備: 添付つきのタスクを起票する ==="
pair_as manage
add_task() { "$TAKO_BIN" todo add "$@" --json > "$TMP/add.log" 2>&1 || { echo "起票に失敗:"; cat "$TMP/add.log"; exit 1; }; }
add_task "解説動画 v6 を YouTube へ投稿" \
  --kind post --project tako \
  --body "試聴して OK なら投稿する" \
  --attach "$MEDIA/v6.mp4" \
  --attach "$HOME/dev/proj/thumb.png" \
  --attach "$HOME/dev/proj/サムネ 01.png" \
  --attach "$HOME/dev/proj/notes.md"
POST_ID="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["id"])' "$TMP/add.log")"
pass "添付 4 件のタスクを起票した（${POST_ID}）"

echo
echo "=== Test 1: 応答に「その場で見せる種別」が載る ==="
api GET /api/tasks >/dev/null
check_eq "動画は video" "video" "$(task_field "$POST_ID" attachments.0.preview)"
check_eq "画像は image" "image" "$(task_field "$POST_ID" attachments.1.preview)"
check_eq "日本語名の画像も image" "image" "$(task_field "$POST_ID" attachments.2.preview)"
check_eq "ビューアの無い種別は名乗らない（md）" "null" "$(task_field "$POST_ID" attachments.3.preview)"
check_eq "動画は manage の fs ルートで解決できる" "true" "$(task_field "$POST_ID" attachments.0.available)"
ROOT_ID="$(task_field "$POST_ID" attachments.0.root)"
REL="$(task_field "$POST_ID" attachments.0.path_rel)"
check_eq "サイズが載る" "$BIG_BYTES" "$(task_field "$POST_ID" attachments.0.size)"

echo
echo "=== Test 2: 既定（保存）と inline（その場表示）の出し分け ==="
OUT="$(dl "")"
check_eq "既定のダウンロードは 200" "200" "${OUT%% *}"
check_eq "全体が落ちる" "$BIG_BYTES" "${OUT##* }"
check_contains "既定は添付として保存させる" "$(headers)" "attachment"
check_contains "既定の型は octet-stream" "$(headers)" "application/octet-stream"
check_contains "部分取得ができると名乗る（<video> がシークを諦めない）" "$(headers)" "Accept-Ranges: bytes"
check_contains "キャッシュさせない" "$(headers)" "no-store"

OUT="$(dl "&disposition=inline")"
check_eq "inline も 200" "200" "${OUT%% *}"
check_eq "inline でも全体が落ちる" "$BIG_BYTES" "${OUT##* }"
check_contains "inline は実体の型を名乗る（mp4）" "$(headers)" "video/mp4"
check_contains "inline は inline で返る" "$(headers)" "Content-Disposition: inline"
check_not_contains "inline では attachment を名乗らない" "$(hval Content-Disposition)" "attachment"

echo
echo "=== Test 3: Range 付き GET が 206 + Content-Range を返し、中身が一致する ==="
OUT="$(dl "&disposition=inline" "bytes=0-1")"
check_eq "先頭 2 バイトの要求が 206" "206" "${OUT%% *}"
check_eq "落ちたのは 2 バイト" "2" "${OUT##* }"
check_eq "Content-Range が正しい" "bytes0-1/$BIG_BYTES" "$(hval Content-Range)"
check_eq "先頭 2 バイトの中身が実体と一致" "HE" "$(cat "$TMP/dl")"

OUT="$(dl "&disposition=inline" "bytes=4-7")"
check_eq "途中の 4 バイトが 206" "206" "${OUT%% *}"
check_eq "落ちたのは 4 バイト" "4" "${OUT##* }"
check_eq "途中の中身が実体と一致" "0123" "$(cat "$TMP/dl")"

# **末尾チャンクだけ取れる**（動画の末尾へシークしたときの形）
OUT="$(dl "&disposition=inline" "bytes=-16")"
check_eq "末尾 16 バイトの要求が 206" "206" "${OUT%% *}"
check_eq "落ちたのは 16 バイト" "16" "${OUT##* }"
check_eq "末尾の Content-Range が正しい" "bytes$((BIG_BYTES - 16))-$((BIG_BYTES - 1))/$BIG_BYTES" "$(hval Content-Range)"
check_eq "末尾の中身が実体と一致" "$TAIL_MARK" "$(cat "$TMP/dl")"

OUT="$(dl "&disposition=inline" "bytes=$((BIG_BYTES - 16))-")"
check_eq "開いた範囲（末尾まで）が 206" "206" "${OUT%% *}"
check_eq "開いた範囲も 16 バイト" "16" "${OUT##* }"
check_eq "開いた範囲の中身が実体と一致" "$TAIL_MARK" "$(cat "$TMP/dl")"

echo
echo "=== Test 4: 満たせない範囲は 416・解釈できない範囲は全体 ==="
OUT="$(dl "&disposition=inline" "bytes=$BIG_BYTES-")"
check_eq "実体の外を要求すると 416" "416" "${OUT%% *}"
check_eq "416 に長さの目安が載る" "bytes*/$BIG_BYTES" "$(hval Content-Range)"
OUT="$(dl "&disposition=inline" "items=0-9")"
check_eq "知らない単位は無視して全体（200）" "200" "${OUT%% *}"
check_eq "全体が落ちる" "$BIG_BYTES" "${OUT##* }"
OUT="$(dl "&disposition=inline" "bytes=0-9,20-29")"
check_eq "複数範囲は受けずに全体（200）" "200" "${OUT%% *}"
check_eq "複数範囲でも全体が落ちる" "$BIG_BYTES" "${OUT##* }"

echo
echo "=== Test 5: 300 MB を Range で取っても常駐量が桁で小さい ==="
DAEMON_PID="$(head -1 "$TAKO_REMOTE_STATE_DIR/tako-remote.pid" 2>/dev/null | awk '{print $1}' | tr -dc '0-9')"
# 半分より後ろを丸ごと（150 MB）要求する = 一番メモリへ載せたくなる形
HALF=$((BIG_BYTES / 2))
OUT="$(dl "&disposition=inline" "bytes=$HALF-")"
check_eq "150 MB の部分取得が 206" "206" "${OUT%% *}"
check_eq "150 MB がそのまま落ちる" "$((BIG_BYTES - HALF))" "${OUT##* }"
RSS_KB="$(ps -o rss= -p "${DAEMON_PID:-0}" 2>/dev/null | tr -dc '0-9' || true)"
if [ -z "$RSS_KB" ]; then
  fail "daemon（pid '${DAEMON_PID}'）の常駐量を測れない"
elif [ "$RSS_KB" -lt $((BIG_MB * 1024 / 4)) ]; then
  pass "Range でも daemon の常駐量が桁で小さい（${RSS_KB} KB < $((BIG_MB * 1024 / 4)) KB = ストリーミング）"
else
  fail "Range で daemon が実体をメモリへ載せている疑い（RSS ${RSS_KB} KB）"
fi

echo
echo "=== Test 6: 認可は 1 実装のまま（見せ方を足しても緩まない） ==="
# 画像（ツリー配下）へ切り替える
REL_IMG="$(task_field "$POST_ID" attachments.1.path_rel)"
ROOT_IMG="$(task_field "$POST_ID" attachments.1.root)"
SAVED_ROOT="$ROOT_ID"; SAVED_REL="$REL"
pair_as observe
ROOT_ID="$SAVED_ROOT"; REL="$SAVED_REL"
check_eq "observe は既定のダウンロードが 403" "403" "$(dl "" | cut -d' ' -f1)"
check_eq "observe は inline でも 403" "403" "$(dl "&disposition=inline" | cut -d' ' -f1)"
check_eq "observe は Range 付きでも 403" "403" "$(dl "&disposition=inline" "bytes=0-1" | cut -d' ' -f1)"

pair_as interact
check_eq "interact は fs ルート（ツリー外）を inline でも落とせない" "403" "$(dl "&disposition=inline" | cut -d' ' -f1)"
# ツリー配下の画像なら interact でも見られる
api GET /api/tasks >/dev/null
ROOT_ID="$(task_field "$POST_ID" attachments.1.root)"
REL="$(task_field "$POST_ID" attachments.1.path_rel)"
check_eq "interact はツリー配下の画像を inline で取れる" "200" "$(dl "&disposition=inline" | cut -d' ' -f1)"
check_contains "画像の型を名乗る" "$(headers)" "image/png"
check_eq "画像の中身が実体と一致（PNG の署名）" "PNG" "$(head -c 4 "$TMP/dl" | tail -c 3)"
check_eq "画像も部分取得できる" "206" "$(dl "&disposition=inline" "bytes=0-3" | cut -d' ' -f1)"

echo
echo "=== Test 7: 空白 / 日本語のファイル名が往復する ==="
pair_as manage
api GET /api/tasks >/dev/null
ROOT_ID="$(task_field "$POST_ID" attachments.2.root)"
REL="$(task_field "$POST_ID" attachments.2.path_rel)"
check_contains "相対パスに空白と日本語が残っている" "$REL" "サムネ 01.png"
OUT="$(dl "&disposition=inline")"
check_eq "%20 で符号化しても届く" "200" "${OUT%% *}"
check_contains "画像として名乗る" "$(headers)" "image/png"
# PWA（`URLSearchParams`）は空白を `+` で送る。daemon が同じものとして解けることを見る
PLUS_REL="$(python3 -c 'import urllib.parse,sys; print(urllib.parse.quote(sys.argv[1]).replace("%20","+"))' "$REL")"
CODE="$(curl -s -o "$TMP/dl" -w '%{http_code}' \
  -H "X-Forwarded-For: 100.64.0.7" -H "X-Forwarded-Host: $HOST_HDR" \
  "$BASE/api/files/download?root=$ROOT_ID&path=$PLUS_REL&disposition=inline")"
check_eq "空白を + で送っても同じファイルに届く（PWA の符号化）" "200" "$CODE"
check_contains "ヘッダの名前は UTF-8 で逃がす" \
  "$(curl -s -D - -o /dev/null -H "X-Forwarded-For: 100.64.0.7" -H "X-Forwarded-Host: $HOST_HDR" \
     "$BASE/api/files/download?root=$ROOT_ID&path=$(urlq "$REL")&disposition=inline" | tr -d '\r')" \
  "filename*=UTF-8''"

echo
echo "=== Test 8: 監査ログに残る（見せ方が変わっても記録は同じ経路） ==="
AUDIT="$TAKO_REMOTE_STATE_DIR/audit.log"
if [ -s "$AUDIT" ]; then
  check_contains "ダウンロードが監査に残る" "$(cat "$AUDIT")" '"download"'
  check_not_contains "監査に絶対パスを書かない（#927）" "$(cat "$AUDIT")" "$MEDIA"
else
  fail "監査ログが無い（${AUDIT}）"
fi

echo
echo "================================"
echo "  PASS: $PASS  /  FAIL: $FAIL"
echo "================================"
[ "$FAIL" -eq 0 ]
