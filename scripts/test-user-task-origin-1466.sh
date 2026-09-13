#!/usr/bin/env bash
# test-user-task-origin-1466.sh — worker が起票したユーザータスクの戻り先の実経路テスト（#1466）
#
# 隔離した data / orchestrator / tmux で**実 tako-app** を立て、CLI と MCP から実際に
# `tako todo add` / `respond` を撃って
#   ① worker（spawn 元が生きている）の起票が **spawn 元の master のプロファイル**へ戻る
#   ② CLI と MCP で `origin.profile` が同じ（設計原則 5 の 1:1）
#   ③ 返答が **spawn 元の master のペイン**へ届く（無関係な default の master には行かない）
#   ④ 解けない起票（role なし / solo / spawn 元が閉じた worker）は `default` へ落ちず、
#      配送が `failed` + 宛先不明の理由になり `tako todo show` から読める
#   ⑤ spawn 元の枝が無くても、**一意な管轄プロファイル**があればそこへ戻る
#   ⑥ A/B（`TAKO_1466_LEGACY=1`）で修正前を再現 = 同じ worker の返答が
#      **無関係な default の master のペイン**へ届く
# を実測する。
#
# **本番の tako / 設定 / user-tasks.yaml には一切触らない**（data / orchestrator / HOME は
# mktemp 配下、tmux は専用ソケット、落とすのは自分で起こした pid だけ）。
# 窓は常設の仮想ディスプレイ（tako-vd）へ出る（#1141）。
#
# 使い方: bash scripts/test-user-task-origin-1466.sh
set -euo pipefail

# **本番 GUI を指す env を最初に落とす**（#1449 / #1450 で本番にペインが漏れた実例が 2 件）
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_ORCHESTRATOR_ROLE

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
    *) fail "$1（'${3}' を含まない: ${2}）" ;;
  esac
}

# `/tmp` 直下に取る（tmux と IPC が sun_path の上限に当たらない長さ。#1441）
TMP="$(mktemp -d /tmp/tako-1466-XXXXXX)"
APP_PID=""
TMUX_SOCKET="tako-1466-$$"
cleanup() {
  # 明示 pid だけを落とす（pkill / killall は本番 GUI にも当たる）。
  # **終わるのを待ってから消す**（死に際に書くペインログで置き場が生え直す = 実測）
  if [ -n "$APP_PID" ]; then
    kill "$APP_PID" 2>/dev/null || true
    wait "$APP_PID" 2>/dev/null || true
  fi
  tmux -L "$TMUX_SOCKET" kill-server >/dev/null 2>&1 || true
  rm -rf "$TMP"
}
trap cleanup EXIT

TAKO_BIN="${TAKO_BIN:-$REPO_ROOT/target/debug/tako}"
APP_BIN="${APP_BIN:-$REPO_ROOT/target/debug/tako-app}"
if [ ! -x "$TAKO_BIN" ] || [ ! -x "$APP_BIN" ]; then
  echo "バイナリをビルドします…"
  (cd "$REPO_ROOT" && cargo build -p tako-cli -p tako-app --quiet)
fi
for b in "$TAKO_BIN" "$APP_BIN"; do
  [ -x "$b" ] || { echo "バイナリが見つからない: $b"; exit 1; }
done

bash "$REPO_ROOT/scripts/lib/virtual-display.sh" ensure >/dev/null 2>&1 || \
  echo "  (注) 仮想ディスプレイを用意できなかった: 既定の面で続行する"

# --- 隔離した環境 -------------------------------------------------------------
export HOME="$TMP/home"
mkdir -p "$HOME"
export TAKO_ISOLATED=1
export TAKO_DATA_DIR="$TMP/data"
export TAKO_DISCOVERY_DIR="$TMP/disc"
export TAKO_ORCHESTRATOR_DIR="$TMP/orch"
export TAKO_USER_TASKS_FILE="$TMP/orch/user-tasks.yaml"
export TAKO_SESSIONS_FILE="$TMP/sessions.yaml"
export TAKO_PANE_LOG_DIR="$TMP/panelogs"
export TAKO_WORKERS_FILE="$TMP/workers.yaml"
export TAKO_TMUX_SOCKET="$TMUX_SOCKET"
export TAKO_PERSIST=0
mkdir -p "$TAKO_DATA_DIR" "$TAKO_DISCOVERY_DIR" "$TAKO_ORCHESTRATOR_DIR"
for d in "$HOME" "$TAKO_DATA_DIR" "$TAKO_ORCHESTRATOR_DIR"; do
  case "$d" in
    "$TMP"/*) : ;;
    *) echo "隔離されていないディレクトリ: $d"; exit 1 ;;
  esac
done

# --- 偽エージェント CLI（spawn を実経路で通すため。実 claude は起こさない） --------
FAKE_BIN="$TMP/bin"
mkdir -p "$FAKE_BIN"
cat > "$FAKE_BIN/claude" <<'FAKE'
#!/usr/bin/env bash
# 起動しただけの器（TUI の入力欄だけ出して待つ）。渡された引数は使わない
printf '\xe2\x9d\xaf '
while IFS= read -r _line; do printf '\n\xe2\x9d\xaf '; done
FAKE
chmod +x "$FAKE_BIN/claude"
export PATH="$FAKE_BIN:$PATH"

MASTER_PROFILE="_iso1466_"
GOV_PROFILE="_iso1466gov_"
PROJECT="_iso1466proj_"

jsonf() { python3 -c 'import json,sys
d = json.load(sys.stdin)
for k in sys.argv[1:]:
    if d is None:
        break
    d = d[int(k)] if isinstance(d, list) else d.get(k)
print("" if d is None else d)' "$@"; }

# `tako split` の出力（JSON でも素の数字でも）から新ペイン ID を読む
pane_id() { python3 -c 'import json,sys,re
t = sys.stdin.read().strip()
try:
    d = json.loads(t)
    print(d["pane"] if isinstance(d, dict) and "pane" in d else d.get("pane_id"))
except Exception:
    m = re.search(r"\d+", t); print(m.group(0) if m else "")'; }

# 隔離 GUI を 1 つ起こす（$1 が空でなければ A/B の legacy アーム）
start_app() {
  local legacy="${1:-}"
  rm -f "$TAKO_USER_TASKS_FILE"
  # 仮想ディスプレイは検証の合間に眠る（眠ると列挙から落ちて起動が中止される。#1160）
  bash "$REPO_ROOT/scripts/lib/virtual-display.sh" ensure >/dev/null 2>&1 || true
  if [ -n "$legacy" ]; then
    TAKO_1466_LEGACY=1 "$APP_BIN" > "$TMP/app.log" 2>&1 &
  else
    "$APP_BIN" > "$TMP/app.log" 2>&1 &
  fi
  APP_PID=$!
  for _ in $(seq 1 200); do
    if "$TAKO_BIN" list >/dev/null 2>&1; then break; fi
    sleep 0.1
  done
  "$TAKO_BIN" list >/dev/null 2>&1 || { echo "tako-app へ接続できない:"; tail -20 "$TMP/app.log"; exit 1; }
  local tabs
  tabs="$("$TAKO_BIN" list | python3 -c 'import json,sys; print(len(json.load(sys.stdin)["tabs"]))')"
  if [ "$tabs" != "1" ]; then
    echo "繋がった先が隔離インスタンスではない（タブ ${tabs} 枚）。中止する。"
    exit 1
  fi
}

stop_app() {
  if [ -n "$APP_PID" ]; then kill "$APP_PID" 2>/dev/null || true; wait "$APP_PID" 2>/dev/null || true; fi
  APP_PID=""
  tmux -L "$TMUX_SOCKET" kill-server >/dev/null 2>&1 || true
}

# master 1 枚 + その worker 1 枚 + 無関係な default master 1 枚 + 素のシェル 1 枚を組む。
# worker は **master のペインを split して**作る（spawn と同じく `spawned_by` が張られる）
build_layout() {
  # 管轄（projects.yaml + プロファイル）を用意する。**実 spawn を通す**ために要る
  mkdir -p "$TAKO_ORCHESTRATOR_DIR/profiles" "$TMP/repo"
  printf 'projects:\n  %s:\n    cwd: %s\n' "$PROJECT" "$TMP/repo" \
    > "$TAKO_ORCHESTRATOR_DIR/projects.yaml"
  printf 'master_agent: claude\nworker_agent: claude\nprojects:\n  - %s\ncwd: %s\n' \
    "$PROJECT" "$TMP/repo" > "$TAKO_ORCHESTRATOR_DIR/profiles/$MASTER_PROFILE.yaml"

  # master は**新しいタブ**に作る（`tab new` が {"tab":N,"pane":M} を返すので
  # ツリーの形に依存せずペイン ID が採れる）
  ROOT_PANE="$("$TAKO_BIN" tab new --title master | jsonf pane)"
  "$TAKO_BIN" title --pane "$ROOT_PANE" --role "orchestrator-master:$MASTER_PROFILE" >/dev/null
  # worker は **実 spawn** で作る（`spawned_by` が張られるのはこの経路だけ）
  WORKER_PANE="$(TAKO_PANE_ID="$ROOT_PANE" TAKO_ORCHESTRATOR_ROLE="master:$MASTER_PROFILE" \
    "$TAKO_BIN" orchestrator spawn --project "$PROJECT" --label 1466 \
    --prompt "#1466 の実測用" | jsonf pane_id)"
  OTHER_MASTER_PANE="$("$TAKO_BIN" tab new --title other | jsonf pane)"
  "$TAKO_BIN" title --pane "$OTHER_MASTER_PANE" --role "orchestrator-master" >/dev/null
  PLAIN_PANE="$("$TAKO_BIN" tab new --title plain | jsonf pane)"
}

# 呼び出し元（ペイン + role）を名乗って CLI から起票する
todo_add_cli() { # pane role title
  TAKO_PANE_ID="$1" TAKO_ORCHESTRATOR_ROLE="$2" "$TAKO_BIN" todo add "$3" >/dev/null
}

# 同じことを MCP（stdio ブリッジ）から撃つ
todo_add_mcp() { # pane role title
  local socket token
  read -r socket token <<<"$(python3 - "$TAKO_DISCOVERY_DIR" <<'PYD'
import json, os, sys
d = sys.argv[1]
for name in sorted(os.listdir(d)):
    try:
        info = json.load(open(os.path.join(d, name)))
    except Exception:
        continue
    if info.get("socket") and info.get("token"):
        print(info["socket"], info["token"]); break
PYD
)"
  printf '%s\n%s\n%s\n' \
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"t","version":"0"}}}' \
    '{"jsonrpc":"2.0","method":"notifications/initialized"}' \
    "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"tako_todo\",\"arguments\":{\"action\":\"add\",\"title\":\"$3\"}}}" \
    | TAKO_SOCKET="$socket" TAKO_TOKEN="$token" TAKO_PANE_ID="$1" TAKO_ORCHESTRATOR_ROLE="$2" \
      "$TAKO_BIN" mcp serve 2>/dev/null | tail -1 > "$TMP/mcp.json"
}

show_json() { "$TAKO_BIN" todo show "$1" --json > "$TMP/show.json"; }
shown() { jsonf "$@" < "$TMP/show.json"; }

echo "=== 準備: 隔離 tako-app（修正後アーム）を起動 ==="
start_app
build_layout
pass "隔離 tako-app 起動（pid ${APP_PID}）: master=${ROOT_PANE} worker=${WORKER_PANE} 無関係な default master=${OTHER_MASTER_PANE} 素のシェル=${PLAIN_PANE}"

echo
echo "=== Test 1: worker の起票は spawn 元の master へ戻る（CLI / MCP で同じ） ==="
todo_add_cli "$WORKER_PANE" "worker:$PROJECT:1466" "CLI から worker が起票"
show_json u-1
check_eq "CLI: origin.profile が spawn 元の master" "$MASTER_PROFILE" "$(shown origin profile)"
check_eq "CLI: origin.pane が起票した worker" "$WORKER_PANE" "$(shown origin pane)"
check_eq "CLI: created_by は worker のまま（master を騙らない）" \
  "worker:$PROJECT:1466" "$(shown created_by)"

todo_add_mcp "$WORKER_PANE" "worker:$PROJECT:1466" "MCP から worker が起票"
show_json u-2
check_eq "MCP: origin.profile が CLI と同じ" "$MASTER_PROFILE" "$(shown origin profile)"
check_eq "MCP: created_by も同じ" "worker:$PROJECT:1466" "$(shown created_by)"

echo
echo "=== Test 2: 返答は spawn 元の master のペインへ届く ==="
"$TAKO_BIN" todo respond u-1 --decision answered --comment "1466 の実測" >/dev/null
show_json u-1
check_eq "配送先のプロファイル" "$MASTER_PROFILE" "$(shown delivery profile)"
check_eq "配送先のペイン = spawn 元の master" "$ROOT_PANE" "$(shown delivery pane)"
if [ "$(shown delivery pane)" = "$OTHER_MASTER_PANE" ]; then
  fail "無関係な default の master へ届いている（#1466 の症状）"
else
  pass "無関係な default の master（pane ${OTHER_MASTER_PANE}）へは届いていない"
fi

echo
echo "=== Test 3: 解けない起票は default へ落ちず、宛先不明で失敗する ==="
todo_add_cli "$PLAIN_PANE" "" "role なしの素のシェルから起票"
show_json u-3
check_eq "role なし: origin.profile は空のまま" "" "$(shown origin profile)"
"$TAKO_BIN" todo respond u-3 --decision answered --comment "宛先不明の確認" >/dev/null
show_json u-3
check_eq "role なし: 配送は失敗" "failed" "$(shown delivery state)"
check_eq "role なし: 宛先を選んでいない" "" "$(shown delivery pane)"
check_contains "role なし: 理由が読める" "$(shown delivery reason)" "宛先不明"
check_contains "tako todo show に理由が出る" "$("$TAKO_BIN" todo show u-3)" "宛先不明"

"$TAKO_BIN" title --pane "$PLAIN_PANE" --role "solo" >/dev/null
todo_add_cli "$PLAIN_PANE" "solo" "solo から起票"
show_json u-4
check_eq "solo: origin.profile は空のまま" "" "$(shown origin profile)"
"$TAKO_BIN" todo respond u-4 --decision answered --comment "solo" >/dev/null
show_json u-4
check_eq "solo: 配送は失敗" "failed" "$(shown delivery state)"

echo
echo "=== Test 4: spawn 元の master を閉じても、返答は同じプロファイルの master へ行く ==="
"$TAKO_BIN" close --pane "$ROOT_PANE" --force >/dev/null
todo_add_cli "$WORKER_PANE" "worker:$PROJECT:1466" "spawn 元を閉じた後の起票"
show_json u-5
check_eq "枝が切れても管轄から同じプロファイルへ戻る" "$MASTER_PROFILE" "$(shown origin profile)"
"$TAKO_BIN" todo respond u-5 --decision answered --comment "閉じた後" >/dev/null
show_json u-5
check_eq "同じプロファイルの master を起こして渡す" "launched" "$(shown delivery state)"
check_eq "起こした先のプロファイル" "$MASTER_PROFILE" "$(shown delivery profile)"
if [ "$(shown delivery pane)" = "$OTHER_MASTER_PANE" ]; then
  fail "無関係な default の master へ届いている（#1466 の症状）"
else
  pass "無関係な default の master（pane ${OTHER_MASTER_PANE}）へは届いていない"
fi

echo
echo "=== Test 5: 管轄も引けない worker の起票は宛先不明のまま ==="
ORPHAN_PANE="$("$TAKO_BIN" tab new --title orphan | jsonf pane)"
"$TAKO_BIN" title --pane "$ORPHAN_PANE" --role "orchestrator-worker:_iso1466none_:x" >/dev/null
todo_add_cli "$ORPHAN_PANE" "worker:_iso1466none_:x" "管轄の無い worker が起票"
show_json u-6
check_eq "管轄が無い: origin.profile は空のまま" "" "$(shown origin profile)"
"$TAKO_BIN" todo respond u-6 --decision answered --comment "管轄なし" >/dev/null
show_json u-6
check_eq "管轄が無い: 配送は失敗（default の master へ行かない）" "failed" "$(shown delivery state)"
check_contains "管轄が無い: 理由が読める" "$(shown delivery reason)" "宛先不明"
# 管轄が 2 本あるときも「どれか 1 つ」を選ばない
printf 'projects:\n  - %s\n' "_iso1466none_" > "$TAKO_ORCHESTRATOR_DIR/profiles/${GOV_PROFILE}a.yaml"
printf 'projects:\n  - %s\n' "_iso1466none_" > "$TAKO_ORCHESTRATOR_DIR/profiles/${GOV_PROFILE}b.yaml"
todo_add_cli "$ORPHAN_PANE" "worker:_iso1466none_:x" "管轄が曖昧なときの起票"
show_json u-7
check_eq "管轄が曖昧なら宛先不明のまま" "" "$(shown origin profile)"
rm -f "$TAKO_ORCHESTRATOR_DIR/profiles/${GOV_PROFILE}b.yaml"
todo_add_cli "$ORPHAN_PANE" "worker:_iso1466none_:x" "管轄が一意になった後の起票"
show_json u-8
check_eq "管轄が一意になれば解ける" "${GOV_PROFILE}a" "$(shown origin profile)"
rm -f "$TAKO_ORCHESTRATOR_DIR/profiles/${GOV_PROFILE}a.yaml"

echo "=== Test 6: A/B（TAKO_1466_LEGACY=1）で修正前を再現する ==="
stop_app
start_app legacy
build_layout
todo_add_cli "$WORKER_PANE" "worker:$PROJECT:1466" "legacy アームで worker が起票"
show_json u-1
check_eq "legacy: origin.profile は空（spawn 元を解かない）" "" "$(shown origin profile)"
"$TAKO_BIN" todo respond u-1 --decision answered --comment "legacy" >/dev/null
show_json u-1
check_eq "legacy: default へ落ちる" "default" "$(shown delivery profile)"
check_eq "legacy: 無関係な default の master へ届く（= #1466 の症状）" \
  "$OTHER_MASTER_PANE" "$(shown delivery pane)"

echo
echo "=== 結果: ${PASS} PASS / ${FAIL} FAIL ==="
[ "$FAIL" -eq 0 ]
