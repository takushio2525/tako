#!/usr/bin/env bash
# test-remote-fs-1451.sh — スマホからの Finder 風の全体閲覧とショートカットの実経路テスト（#1451）
#
# 偽の tailscale CLI + 隔離した HOME / state / data で **実 daemon（tako remote start）+
# 実 tako-app** を走らせ、HTTP を実際に叩いて
#   ① role の gate（observe / interact では全体閲覧が見えない / manage で見える）
#   ② `/` から偽のディレクトリ木を辿ってファイルの本文が読める
#   ③ 読めないディレクトリ・読めない 1 件が**理由つき**で返る（無言禁止）
#   ④ ショートカットの追加 / 削除が PWA・CLI・MCP の 3 口で同じ結果になり、
#      **daemon を再起動しても残る**
#   ⑤ エッジ（空フォルダ / 権限なし / symlink ループ / 巨大ディレクトリ /
#      重複登録 / 存在しないパスの登録 / 既定の削除）
#   ⑥ A/B（`TAKO_1451_LEGACY=1` で #1079 の見え方へ戻る）
# を実測する。
#
# **本番の tailscale / serve 設定 / remote デーモン / tako 設定 / ~/.ssh には一切触らない**
# （tailscale の呼び出しは偽物へ、HOME / state / data / orchestrator は mktemp 配下）。
# **実ファイルシステムを一度も一覧しない**のが #1451 の要点（#927）: 見るのは
# 隔離 HOME に作った偽の木だけで、実 `/` や実ユーザー名は出力にも入らない。
#
# 使い方: bash scripts/test-remote-fs-1451.sh
set -uo pipefail

# **本番 GUI を指す env を最初に落とす**。tako のペインの中から走らせると
# `TAKO_SOCKET` / `TAKO_TOKEN` / `TAKO_PANE_ID` が継承され、CLI が隔離インスタンスでは
# なく**ユーザーの本番 GUI** を触る（#1449 の検証で実際に起きた）。
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

# `/tmp` 直下に取る（daemon の待ち受けと tmux が sun_path の上限に当たらない長さ。#1441）
TMP="$(mktemp -d /tmp/tako-1451-XXXXXX)"
APP_PID=""
TMUX_SOCKET="tako-1451-$$"
cleanup() {
  if [ -n "${TAKO_REMOTE_STATE_DIR:-}" ] && [ -s "$TAKO_REMOTE_STATE_DIR/tako-remote.pid" ]; then
    "$TAKO_BIN" remote stop >/dev/null 2>&1 || true
  fi
  # 明示 pid だけを落とす（pkill / killall は本番 GUI にも当たる）
  if [ -n "$APP_PID" ]; then kill "$APP_PID" 2>/dev/null || true; fi
  # ソケット名を明示した起動は自分で畳む（#1192）
  tmux -L "$TMUX_SOCKET" kill-server >/dev/null 2>&1 || true
  # 権限 000 のフォルダを作っているので、消す前に戻す（残骸を残さない）。
  # **親から順に**辿れるようにしてから -R を掛ける（000 のままだと降りられず
  # `Directory not empty` で消し残る = 実測）
  chmod u+rwx "$TMP/home/locked" 2>/dev/null || true
  chmod -R u+rwX "$TMP" 2>/dev/null || true
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
NODE="fake-1451.tailfake.ts.net"
case "${1:-}" in
  --version) echo "1.0.0-fake"; exit 0 ;;
  status)
    printf '{"BackendState":"Running","Version":"1.0.0-fake","CertDomains":["%s"],"Self":{"DNSName":"%s."}}\n' "$NODE" "$NODE"
    exit 0 ;;
  whois)
    printf '{"Node":{"StableID":"nFAKE1451","Name":"iphone.tailfake.ts.net.","Hostinfo":{"Hostname":"iPhone"}},"UserProfile":{"LoginName":"tester@example.com"}}\n'
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
TAKO_BIN="${TAKO_BIN:-$REPO_ROOT/target/debug/tako}"
APP_BIN="${APP_BIN:-$REPO_ROOT/target/debug/tako-app}"
if [ ! -x "$TAKO_BIN" ] || [ ! -x "$APP_BIN" ]; then
  echo "バイナリをビルドします…"
  (cd "$REPO_ROOT" && cargo build -p tako-cli -p tako-app --quiet)
fi
for b in "$TAKO_BIN" "$APP_BIN"; do
  [ -x "$b" ] || { echo "バイナリが見つからない: $b"; exit 1; }
done

# 検証用の窓はユーザーの画面に出さない（#1141。常設 tako-vd・冪等）
bash "$REPO_ROOT/scripts/lib/virtual-display.sh" ensure >/dev/null 2>&1 || \
  echo "  (注) 仮想ディスプレイを用意できなかった: 既定の面で続行する"

# --- 隔離した環境 -------------------------------------------------------------
# **HOME ごと隔離する**: 既定ショートカット（`~` / Desktop / Downloads）はホームを
# 見るので、本物のホーム（= 実ユーザー名。#927）を読ませない
export HOME="$TMP/home"
mkdir -p "$HOME/Desktop" "$HOME/Downloads" "$HOME/dev/proj" "$HOME/empty"
echo "# 偽のメモ" > "$HOME/dev/proj/notes.md"
echo "hello from the fake tree" > "$HOME/dev/proj/hello.txt"
# 読めないディレクトリ（実 `/private/var/db` を触らずに同じ状況を作る）
mkdir -p "$HOME/locked/inside"
chmod 000 "$HOME/locked"
# 読めないエントリ（リンク先が消えている）
ln -s "$HOME/dev/nowhere" "$HOME/dev/broken-link"
# symlink のループ
ln -s "$HOME/dev/loop-b" "$HOME/dev/loop-a"
ln -s "$HOME/dev/loop-a" "$HOME/dev/loop-b"
# 巨大ディレクトリ（一覧の上限 1000 を超える）
mkdir -p "$HOME/many"
python3 -c "
import pathlib,sys
d = pathlib.Path(sys.argv[1])
for i in range(1500):
    (d / f'f{i:05d}.txt').write_text('x')
" "$HOME/many"

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
# #841: ループバック TCP では XFF を読む前に**接続元プロセス**を検証する。
# 切る（`TAKO_841_LEGACY`）のではなく「この隔離環境で serve の代わりに繋いでくるのは
# curl」だと名前で宣言する（所有者ゲートは本番と同じ経路を通る）
export TAKO_REMOTE_TRUSTED_PEER_NAMES="curl"
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
"$APP_BIN" > "$TMP/app.log" 2>&1 &
APP_PID=$!
for _ in $(seq 1 200); do
  if "$TAKO_BIN" list >/dev/null 2>&1; then break; fi
  sleep 0.1
done
"$TAKO_BIN" list >/dev/null 2>&1 || { echo "tako-app へ接続できない:"; tail -20 "$TMP/app.log"; exit 1; }
# **繋がった先が隔離インスタンスであることを確かめる**（外れたまま進むと本番を触る）
BOOT_TABS="$("$TAKO_BIN" list | python3 -c 'import json,sys; print(len(json.load(sys.stdin)["tabs"]))')"
if [ "$BOOT_TABS" != "1" ]; then
  echo "繋がった先が隔離インスタンスではない（タブ ${BOOT_TABS} 枚）。"
  echo "本番 GUI を触る恐れがあるので中止する（TAKO_SOCKET などが残っていないか確認）。"
  exit 1
fi
pass "隔離 tako-app が起動して CLI から見える（pid ${APP_PID}・タブ 1 枚）"

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

HOST_HDR="fake-1451.tailfake.ts.net"
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
# 一覧の中から条件に合う 1 件を数える（実パスを出力しないための補助）
count_where() { python3 -c 'import json,sys
d=json.load(open(sys.argv[1]))
items=d.get(sys.argv[2]) or []
k,v=sys.argv[3],sys.argv[4]
print(sum(1 for i in items if str(i.get(k)).lower()==v))' "$TMP/body" "$1" "$2" "$3"; }
entry_meta() { python3 -c 'import json,sys
d=json.load(open(sys.argv[1]))
for e in d.get("entries") or []:
    if e.get("name")==sys.argv[2]:
        print(json.dumps({k:e.get(k) for k in ("dir","unreadable","symlink","size")},ensure_ascii=False)); raise SystemExit
print("なし")' "$TMP/body" "$1"; }

pair_as() {
  api POST /api/pair "{\"name\":\"iPhone\",\"role\":\"$1\"}" >/dev/null
  curl -s -o /dev/null -X POST "$BASE/api/admin/pair/approve" \
    -H "X-Tako-Admin: $ADMIN_TOKEN" -H 'Content-Type: application/json' \
    -d "{\"device_id\":\"nFAKE1451\",\"role\":\"$1\"}"
}

start_daemon
pass "隔離 daemon が起動した（loopback port ${PORT}）"

echo
echo "=== Test 1: role の gate（全体閲覧は manage 以上） ==="
for role in observe interact manage; do
  pair_as "$role"
  CODE="$(api GET /api/me)"
  check_eq "${role} 端末として登録された" "$role" "$(jqf role)"
  CODE="$(api GET /api/files)"
  if [ "$role" = "observe" ]; then
    check_eq "observe は一覧そのものが 403" "403" "$CODE"
  else
    check_eq "${role} は一覧を引ける" "200" "$CODE"
    FS_COUNT="$(count_where roots kind fs)"
    if [ "$role" = "interact" ]; then
      check_eq "interact に全体閲覧の入口が出ない（#1079 のまま）" "0" "$FS_COUNT"
    else
      check_eq "manage に全体閲覧の入口が出る" "1" "$FS_COUNT"
    fi
  fi
  # ショートカット API は読み書きとも manage
  CODE="$(api GET /api/files/shortcuts)"
  if [ "$role" = "manage" ]; then
    check_eq "manage はショートカットを引ける" "200" "$CODE"
  else
    check_eq "${role} のショートカット取得は 403" "403" "$CODE"
  fi
  CODE="$(api POST /api/files/shortcuts "{\"path\":\"$HOME/dev\"}")"
  if [ "$role" = "manage" ]; then
    check_eq "manage はショートカットを足せる" "200" "$CODE"
  else
    check_eq "${role} のショートカット追加は 403" "403" "$CODE"
  fi
done

echo
echo "=== Test 2: interact は id を直接叩いても全体閲覧できない ==="
pair_as interact
CODE="$(api GET "/api/files?root=fs&path=")"
check_eq "interact が root=fs を直接叩いても 403" "403" "$CODE"
check_eq "理由は unknown_root（存在の有無を漏らさない）" "unknown_root" "$(jqf kind)"
CODE="$(api GET "/api/files/content?root=fs&path=etc/hosts")"
check_eq "本文の直接取得も 403" "403" "$CODE"

echo
echo "=== Test 3: manage は / から偽の木を辿れる ==="
pair_as manage
CODE="$(api GET "/api/files?root=fs&path=")"
check_eq "/ の一覧が引ける" "200" "$CODE"
check_eq "ルートの種別が fs" "fs" "$(jqf root_kind)"
check_eq "パンくず用の絶対パスが載る" "/" "$(jqf abs)"
# 隔離 HOME の相対パス（`/` を落とす）で辿る
REL_HOME="${HOME#/}"
CODE="$(api GET "/api/files?root=fs&path=$REL_HOME/dev/proj")"
check_eq "偽の木の奥まで辿れる" "200" "$CODE"
check_contains "ファイルが一覧に出る" "$(body)" "hello.txt"
CODE="$(api GET "/api/files/content?root=fs&path=$REL_HOME/dev/proj/hello.txt")"
check_eq "本文が読める" "200" "$CODE"
check_contains "本文の中身が返る" "$(body)" "hello from the fake tree"
check_eq "書き込み用の検証子が付く" "false" "$(python3 -c 'import json;print(str(json.load(open("'"$TMP"'/body"))["etag"] is None).lower())')"

echo
echo "=== Test 4: 読めないものは理由を返す（無言禁止） ==="
CODE="$(api GET "/api/files?root=fs&path=$REL_HOME/locked")"
check_eq "権限の無いフォルダは 500" "500" "$CODE"
check_eq "理由の種別が unreadable" "unreadable" "$(jqf kind)"
check_contains "日本語の理由が付く" "$(body)" "読み取れませんでした"
check_contains "英語の理由も付く" "$(body)" "Could not read"
CODE="$(api GET "/api/files?root=fs&path=$REL_HOME/dev")"
check_eq "壊れたリンクを含むフォルダは開ける" "200" "$CODE"
check_contains "壊れたリンクに unreadable の印が付く" "$(entry_meta broken-link)" '"unreadable": true'
check_contains "リンク自身の種別は残る" "$(entry_meta broken-link)" '"symlink": true'
check_contains "読めるファイルには印が付かない" "$(entry_meta loop-a)" '"unreadable": true'

echo
echo "=== Test 5: エッジ ==="
CODE="$(api GET "/api/files?root=fs&path=$REL_HOME/empty")"
check_eq "空フォルダは 200" "200" "$CODE"
check_eq "空フォルダの件数は 0" "0" "$(python3 -c 'import json;print(len(json.load(open("'"$TMP"'/body"))["entries"]))')"
CODE="$(api GET "/api/files?root=fs&path=$REL_HOME/dev/loop-a")"
check_eq "symlink ループは 404（無限に辿らない）" "404" "$CODE"
CODE="$(api GET "/api/files?root=fs&path=$REL_HOME/many")"
check_eq "巨大フォルダも 200 で返る" "200" "$CODE"
check_eq "上限で切られる" "1000" "$(python3 -c 'import json;print(len(json.load(open("'"$TMP"'/body"))["entries"]))')"
check_eq "切ったことが伝わる" "true" "$(jqf truncated)"
CODE="$(api GET "/api/files?root=fs&path=../etc")"
check_eq '.. は形の段階で 403' "403" "$CODE"
check_eq "理由は traversal" "traversal" "$(jqf kind)"
CODE="$(api GET "/api/files?root=fs&path=/etc")"
check_eq "絶対パスは 403" "403" "$CODE"

echo
echo "=== Test 6: ショートカットの 3 口（PWA / CLI / MCP）と永続 ==="
api DELETE "/api/files/shortcuts?id=$HOME/dev" >/dev/null   # Test 1 の残りを消す
CODE="$(api POST /api/files/shortcuts "{\"path\":\"$HOME/dev/proj\",\"name\":\"仕事\"}")"
check_eq "HTTP で足せる" "200" "$CODE"
SC_ID="$(jqf shortcut.id)"
CODE="$(api POST /api/files/shortcuts "{\"path\":\"$HOME/dev/proj\"}")"
check_eq "同じパスの二重登録も 200" "200" "$CODE"
check_eq "id は変わらない（冪等）" "$SC_ID" "$(jqf shortcut.id)"
CODE="$(api GET /api/files/shortcuts)"
check_eq "一覧に 1 件だけ（件数が増えない）" "1" "$(count_where shortcuts builtin false)"
check_eq "既定（ホーム / デスクトップ / ダウンロード）が 3 件出る" "3" "$(count_where shortcuts builtin true)"
SC_ROOT="$(python3 -c 'import json
d=json.load(open("'"$TMP"'/body"))
print(next(s["root"] for s in d["shortcuts"] if not s["builtin"]))')"
if [ "$SC_ROOT" != "fs" ]; then
  echo "    （診断）一覧に出ているルート:"
  api GET /api/files >/dev/null
  python3 -c 'import json
d=json.load(open("'"$TMP"'/body"))
for r in d.get("roots") or []:
    print("      id=%s kind=%s name=%s" % (r["id"], r.get("kind"), r["name"]))'
fi
check_eq "登録分には飛び先が付く" "fs" "$SC_ROOT"

CLI_COUNT="$("$TAKO_BIN" remote shortcuts | python3 -c 'import json,sys
d=json.load(sys.stdin); print(sum(1 for s in d["shortcuts"] if not s["builtin"]))')"
check_eq "CLI（tako remote shortcuts）が同じ件数を返す" "1" "$CLI_COUNT"
CLI_NAME="$("$TAKO_BIN" remote shortcuts | python3 -c 'import json,sys
d=json.load(sys.stdin); print(next(s["name"] for s in d["shortcuts"] if not s["builtin"]))')"
check_eq "CLI が同じ表示名を返す" "仕事" "$CLI_NAME"
"$TAKO_BIN" remote shortcuts add "$HOME/Downloads" --name "落とし物" >/dev/null 2>&1
CODE="$(api GET /api/files/shortcuts)"
check_eq "CLI で足したものが HTTP にも出る" "2" "$(count_where shortcuts builtin false)"
# 既定と同じパス（Downloads）を登録すると、既定は落ちて登録分が勝つ（二重に出さない）
check_eq "既定と重なったぶんは二重に出ない" "2" "$(count_where shortcuts builtin true)"

# MCP（stdio ブリッジ）からも同じ答えが返る。
# **隔離インスタンスは discovery に出ない（#177）**ので、接続先を env で明示する
# （`TAKO_SOCKET` を渡さないと「接続情報が無い」で落ちる = 実測）
MCP_SOCK="$(python3 -c 'import json,sys
print(json.load(open(sys.argv[1]))["socket"])' "$TAKO_DISCOVERY_DIR/control.json" 2>/dev/null || true)"
MCP_TOKEN="$(python3 -c 'import json,sys
print(json.load(open(sys.argv[1]))["token"])' "$TAKO_DISCOVERY_DIR/control.json" 2>/dev/null || true)"
if [ -z "$MCP_SOCK" ] || [ -z "$MCP_TOKEN" ]; then
  fail "隔離 app の接続情報（$TAKO_DISCOVERY_DIR/control.json）を読めない"
fi
MCP_OUT="$(TAKO_SOCKET="$MCP_SOCK" TAKO_TOKEN="$MCP_TOKEN" printf '%s\n%s\n' \
  '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"t","version":"1"}}}' \
  '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"tako_remote_shortcuts","arguments":{"action":"list"}}}' \
  | TAKO_SOCKET="$MCP_SOCK" TAKO_TOKEN="$MCP_TOKEN" "$TAKO_BIN" mcp serve 2>"$TMP/mcp.err")"
printf '%s' "$MCP_OUT" > "$TMP/mcp.out"
MCP_COUNT="$(printf '%s' "$MCP_OUT" | python3 -c 'import json,sys
for line in sys.stdin:
    line=line.strip()
    if not line: continue
    try: m=json.loads(line)
    except ValueError: continue
    if m.get("id")==2:
        try:
            payload=json.loads(m["result"]["content"][0]["text"])
            print(sum(1 for s in payload["shortcuts"] if not s["builtin"]))
        except (KeyError, ValueError, TypeError): print("解釈できない")
        break
else: print("応答なし")')"
if [ "$MCP_COUNT" != "2" ]; then
  echo "    （診断）MCP の応答: $(tail -1 "$TMP/mcp.out" | cut -c1-300)"
  echo "    （診断）MCP の stderr: $(tail -2 "$TMP/mcp.err" | cut -c1-200)"
fi
check_eq "MCP（tako_remote_shortcuts）も同じ件数" "2" "$MCP_COUNT"

echo
echo "=== Test 7: 断る理由（重複以外） ==="
CODE="$(api POST /api/files/shortcuts "{\"path\":\"$HOME/いない\"}")"
check_eq "存在しないパスは 404" "404" "$CODE"
check_eq "理由は not_found" "not_found" "$(jqf kind)"
CODE="$(api POST /api/files/shortcuts '{"path":"relative/dir"}')"
check_eq "相対パスは 400" "400" "$CODE"
check_eq "理由は not_absolute" "not_absolute" "$(jqf kind)"
CODE="$(api POST /api/files/shortcuts "{\"path\":\"$HOME/dev/proj/notes.md\"}")"
check_eq "ファイルは 400" "400" "$CODE"
check_eq "理由は not_a_directory" "not_a_directory" "$(jqf kind)"
HOME_ID="$(api GET /api/files/shortcuts >/dev/null; python3 -c 'import json
d=json.load(open("'"$TMP"'/body"))
print(next((s["id"] for s in d["shortcuts"] if s["builtin"]), "なし"))')"
CODE="$(api DELETE "/api/files/shortcuts?id=$HOME_ID")"
check_eq "既定のショートカットは消せない" "400" "$CODE"
check_eq "理由は builtin（「無い」と言わない）" "builtin" "$(jqf kind)"

echo
echo "=== Test 8: daemon を再起動しても残る ==="
"$TAKO_BIN" remote stop >/dev/null 2>&1
start_daemon
pair_as manage
CODE="$(api GET /api/files/shortcuts)"
check_eq "再起動後も一覧が引ける" "200" "$CODE"
check_eq "登録分が残っている" "2" "$(count_where shortcuts builtin false)"
check_eq "永続ファイルが 1 つできている" "1" "$(ls "$TAKO_DATA_DIR/remote/shortcuts.json" 2>/dev/null | wc -l | tr -d ' ')"
if [ "$(uname)" != "Darwin" ] || [ "$(stat -f '%Lp' "$TAKO_DATA_DIR/remote/shortcuts.json")" = "600" ]; then
  pass "永続ファイルの権限が 0600"
else
  fail "永続ファイルの権限が 0600 でない（$(stat -f '%Lp' "$TAKO_DATA_DIR/remote/shortcuts.json")）"
fi
# 削除も残る
api DELETE "/api/files/shortcuts?id=$SC_ID" >/dev/null
"$TAKO_BIN" remote stop >/dev/null 2>&1
start_daemon
pair_as manage
api GET /api/files/shortcuts >/dev/null
check_eq "削除も再起動を跨いで残る" "1" "$(count_where shortcuts builtin false)"

echo
echo "=== Test 9: A/B（TAKO_1451_LEGACY=1 で #1079 の見え方へ戻る） ==="
"$TAKO_BIN" remote stop >/dev/null 2>&1
TAKO_1451_LEGACY=1 start_daemon
pair_as manage
CODE="$(api GET /api/files)"
check_eq "legacy 腕でも一覧は引ける" "200" "$CODE"
check_eq "legacy 腕では全体閲覧の入口が出ない" "0" "$(count_where roots kind fs)"
CODE="$(api GET "/api/files?root=fs&path=")"
check_eq "legacy 腕では root=fs が 403" "403" "$CODE"

echo
echo "=== Test 10: 監査ログにパスが載らない（#287 P2-2） ==="
AUDIT="$TAKO_REMOTE_STATE_DIR/audit.log"
if [ -s "$AUDIT" ]; then
  LEAK="$(grep -c "$HOME" "$AUDIT" || true)"
  check_eq "監査ログに隔離 HOME のパスが 1 件も載らない" "0" "$LEAK"
  check_contains "ショートカットの操作は種別だけ残る" "$(cat "$AUDIT")" "shortcut_add"
else
  fail "監査ログが空（記録されていない）"
fi

echo
echo "================================"
echo "PASS: $PASS / FAIL: $FAIL"
[ "$FAIL" -eq 0 ] || exit 1
