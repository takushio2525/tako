#!/usr/bin/env bash
# #1503 の実経路テスト: **無応答の claude を掴んでも `tako setup` が固まらない**。
#
# #1500 の棚卸し R4 = `claude mcp list` が返らない claude に対し `tako setup` が
# **無言で 6 分固まり続けた**。probe（`command_output` / `check_claude_mcp_health`）も
# dispatch の `SetupRun` も待ちに上限を持たなかったため。
#
# ここでは `claude mcp list` だけが返らないスタブ（= R4 と同じ形。`mcp list` は
# 登録済み MCP サーバへ 1 台ずつ繋ぐので、1 台無応答だとそこで返らなくなる）を作り、
# 上限で打ち切って「確認できません（N 秒応答なし）」を出し、**完走して残り一覧まで
# 出す**（#1501 の契約を壊さない）ことを実測する。A/B は `TAKO_1503_LEGACY=1`。
#
# 本番の ~/Library/Application Support/tako・~/.claude・~/.zprofile・実機の tmux には
# 一切触れない（`env -i` で環境ごと差し替え、claude / brew はスタブ）。
# 立てた人工プロセスは**自分が起こしたものだけ** pid 指定で片付ける
# （pkill / killall の名前一致は禁止）。
set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TAKO="${TAKO_BIN:-$ROOT/target/debug/tako}"
PASS=0; FAIL=0

ok()   { PASS=$((PASS+1)); echo "  PASS: $1"; }
ng()   { FAIL=$((FAIL+1)); echo "  FAIL: $1"; }
check(){ if [ "$2" = "$3" ]; then ok "$1"; else ng "$1 (期待 '$3' / 実際 '$2')"; fi; }
contains(){ if grep -qF -- "$2" "$3" 2>/dev/null; then ok "$1"; else ng "$1 ('$2' が出ていない)"; fi; }
absent(){ if grep -qF -- "$2" "$3" 2>/dev/null; then ng "$1 ('$2' が出ている)"; else ok "$1"; fi; }

[ -x "$TAKO" ] || { echo "tako が無い: ${TAKO}（cargo build -p tako-cli）"; exit 1; }
[ -x /usr/bin/python3 ] || { echo "/usr/bin/python3 が無い環境では走らない"; exit 1; }

SANDBOX="$(mktemp -d "${TMPDIR:-/tmp}/tako-1503-XXXXXX")"
BIN="$SANDBOX/bin"; HOMEDIR="$SANDBOX/home"; DATADIR="$SANDBOX/d"
MCPJSON="$HOMEDIR/.claude.json"

# 自分が起こした人工プロセスだけを片付ける（$SANDBOX を argv に持つものに限る）
sandbox_pids(){ /bin/ps -axo pid=,command= | grep -F "$SANDBOX" | grep -v grep | awk '{print $1}'; }
cleanup(){
  local pids; pids="$(sandbox_pids)"
  if [ -n "$pids" ]; then for p in $pids; do kill -9 "$p" 2>/dev/null; done; fi
  rm -rf "$SANDBOX"
}
trap cleanup EXIT

# --- スタブ ------------------------------------------------------------------
# `hang` は**自分の argv に $SANDBOX を持ったまま**眠る（`exec sleep 600` にすると
# argv が `sleep 600` へ化けて ps から自分のぶんを見分けられなくなり、「打ち切った子が
# 残っていないか」の検査が空振りする）。スタブ claude はこれへ `exec` で置き換わるので、
# tako が kill する相手 = この hang 自身
printf '#!/bin/sh\nwhile :; do /bin/sleep 1; done\n' > "$SANDBOX/hang"; chmod +x "$SANDBOX/hang"

# claude スタブ。$1 = mcp list の振る舞い（hang / ok / partial / slow:<秒>）
write_claude(){
  cat > "$BIN/claude" <<EOF
#!/bin/sh
LIST_MODE="$1"
HANG="$SANDBOX/hang"
EOF
  cat >> "$BIN/claude" <<'EOF'
if [ "${1:-}" = "auth" ] && [ "${2:-}" = "status" ]; then
  echo '{"loggedIn":true,"authMethod":"claude.ai","subscriptionType":"Max"}'; exit 0
fi
if [ "${1:-}" = "--version" ]; then echo "2.1.258 (Claude Code)"; exit 0; fi
if [ "${1:-}" = "mcp" ]; then
  shift; sub="${1:-}"; [ $# -gt 0 ] && shift
  case "$sub" in
    list)
      case "$LIST_MODE" in
        hang)    exec "$HANG";;
        partial) echo "tako: /stub/tako mcp serve - connected"; exec "$HANG";;
        slow:*)  sleep "${LIST_MODE#slow:}";;
      esac
      /usr/bin/python3 -c '
import json,sys
try: d=json.load(open(sys.argv[1]))
except Exception: d={}
s=d.get("mcpServers") or {}
if not s: print("No MCP servers configured")
for k,v in s.items():
    print("%s: %s - connected" % (k, " ".join([v.get("command","")]+list(v.get("args") or []))))
' "$HOME/.claude.json"
      exit 0;;
    add)
      name=""; cmd=""
      while [ $# -gt 0 ]; do
        case "$1" in
          --scope|--transport) shift 2;;
          --) shift; cmd="$*"; break;;
          *) name="$1"; shift;;
        esac
      done
      /usr/bin/python3 -c '
import json,sys
p,name,cmd=sys.argv[1],sys.argv[2],sys.argv[3].split(" ")
try: d=json.load(open(p))
except Exception: d={}
d.setdefault("mcpServers",{})[name]={"type":"stdio","command":cmd[0],"args":cmd[1:],"env":{}}
json.dump(d,open(p,"w"),ensure_ascii=False)
' "$HOME/.claude.json" "$name" "$cmd"
      echo "Added stdio MCP server $name"; exit 0;;
    remove)
      name=""
      while [ $# -gt 0 ]; do
        case "$1" in --scope) shift 2;; *) name="$1"; shift;; esac
      done
      /usr/bin/python3 -c '
import json,sys
p,name=sys.argv[1],sys.argv[2]
try: d=json.load(open(p))
except Exception: sys.exit(1)
if name in (d.get("mcpServers") or {}):
    del d["mcpServers"][name]; json.dump(d,open(p,"w"),ensure_ascii=False)
else: sys.exit(1)
' "$HOME/.claude.json" "$name" || exit 1
      exit 0;;
  esac
  exit 0
fi
exit 0
EOF
  chmod +x "$BIN/claude"
}

mkenv(){ # $1 = mcp list の振る舞い
  rm -rf "$BIN" "$HOMEDIR" "$DATADIR"
  mkdir -p "$BIN" "$HOMEDIR" "$DATADIR"
  # exe::find（境界 B16）のログインシェル経由の解決を隔離 PATH に閉じ込める
  printf '#!/bin/sh\nif [ "$1" = "-l" ]; then shift; fi\nexec /bin/sh "$@"\n' > "$BIN/isosh"
  printf '#!/bin/sh\necho "==> Pouring $2"\nexit 0\n' > "$BIN/brew"
  write_claude "$1"
  chmod +x "$BIN"/* 2>/dev/null
}

# 隔離した tako を**外側の締め切りつき**で走らせる。
# 締め切りで殺されたら「固まった」と読める（所要そのものは assert しない = 証拠として出す）
BUDGET=3       # TAKO_SETUP_PROBE_TIMEOUT_SECS
DEADLINE=40    # 外側の締め切り（秒）
LEGACY=""
# 結果は**グローバルへ置く**（`$( )` で呼ぶと副シェルなので所要も締め切りの成否も捨てられる）
RC=0; ELAPSED=0; KILLED=0; LEFTOVERS=""
run(){ # run <出力先> <stdin の渡し方: tty|pipe> -- <tako の引数...>
  local out="$1"; local mode="$2"; shift 3
  local start end pid
  start="$(date +%s)"
  if [ "$mode" = pipe ]; then
    printf '%s' '{}' | env -i HOME="$HOMEDIR" TAKO_DATA_DIR="$DATADIR" TAKO_ISOLATED=1 \
        SHELL="$BIN/isosh" TAKO_SETUP_PROBE_TIMEOUT_SECS="$BUDGET" \
        ${LEGACY:+TAKO_1503_LEGACY=1} \
        PATH="$BIN:/usr/bin:/bin:/usr/sbin:/sbin" TERM=dumb LANG=ja_JP.UTF-8 \
        "$TAKO" "$@" > "$out" 2>&1 &
  else
    env -i HOME="$HOMEDIR" TAKO_DATA_DIR="$DATADIR" TAKO_ISOLATED=1 \
        SHELL="$BIN/isosh" TAKO_SETUP_PROBE_TIMEOUT_SECS="$BUDGET" \
        ${LEGACY:+TAKO_1503_LEGACY=1} \
        PATH="$BIN:/usr/bin:/bin:/usr/sbin:/sbin" TERM=dumb LANG=ja_JP.UTF-8 \
        "$TAKO" "$@" < /dev/null > "$out" 2>&1 &
  fi
  pid=$!
  KILLED=0
  ( sleep "$DEADLINE"; kill -9 "$pid" 2>/dev/null ) & local killer=$!
  # 締め切りで殺したときの `Killed: 9` はシェルのジョブ報告なので黙らせる（rc は取る）
  exec 3>&2 2>/dev/null
  wait "$pid"; RC=$?
  kill "$killer" 2>/dev/null; wait "$killer" 2>/dev/null
  exec 2>&3 3>&-
  end="$(date +%s)"; ELAPSED=$((end-start))
  # 128+9（SIGKILL）で落ちたら締め切りに掛かった = 固まっていた
  if [ "$RC" -eq 137 ]; then KILLED=1; fi
  # **回収する前に控える**（tako が打ち切った子を自分で始末したかを見るため。
  # ここで先に消すと「残らない」が自作自演になる）
  LEFTOVERS="$(sandbox_pids)"
  if [ -n "$LEFTOVERS" ]; then for p in $LEFTOVERS; do kill -9 "$p" 2>/dev/null; done; fi
}
mcp_registered(){ /usr/bin/python3 -c '
import json,sys
try: d=json.load(open(sys.argv[1]))
except Exception: sys.exit(1)
sys.exit(0 if "tako" in (d.get("mcpServers") or {}) else 1)
' "$MCPJSON"; }

echo "== 1) claude mcp list が返らない: 上限で打ち切り、完走する（R4）=="
mkenv hang
run "$SANDBOX/1.log" tty -- setup
echo "  [実測] 所要 ${ELAPSED} 秒（上限 ${BUDGET} 秒 × probe 2 回 + 残りの段）"
check    "締め切りに掛からず終わる"     "$KILLED" "0"
check    "終了コード 0（完走する）"     "$RC" "0"
contains "何を何秒待ったかを出す"       "[確認できません] claude mcp list（3 秒応答なし）" "$SANDBOX/1.log"
contains "打ち切って次へ進むと言う"     "打ち切って次へ進みます" "$SANDBOX/1.log"
contains "完走して結果を出す"           "セットアップ" "$SANDBOX/1.log"
if mcp_registered; then ok "確認できなくても MCP 登録まで進む"; else ng "MCP 登録が載っていない"; fi
if [ -n "$LEFTOVERS" ]; then ng "打ち切った子が残っている（${LEFTOVERS}）"; else ok "打ち切った子は tako が始末する"; fi

echo "== 2) A/B: TAKO_1503_LEGACY=1 は #1503 前の待ち = 固まる =="
mkenv hang
LEGACY=1
run "$SANDBOX/2.log" tty -- setup
echo "  [実測] 締め切り ${DEADLINE} 秒まで走らせて所要 ${ELAPSED} 秒 / rc=${RC}"
check    "締め切りに掛かる（= 固まっていた）" "$KILLED" "1"
absent   "知らせも出ない（無言）"        "確認できません" "$SANDBOX/2.log"
absent   "完走していない"                "残り" "$SANDBOX/2.log"
if [ -n "$LEFTOVERS" ]; then ok "上限が無いので無応答の子も残ったまま（$(echo "$LEFTOVERS" | wc -l | tr -d ' ') 件）"; else ng "旧挙動なら無応答の子が残るはず"; fi
LEGACY=""

echo "== 3) 応答のある claude では従来どおり（知らせを出さない）=="
mkenv ok
run "$SANDBOX/3.log" tty -- setup
echo "  [実測] 所要 ${ELAPSED} 秒"
check    "終了コード 0"                 "$RC" "0"
check    "締め切りに掛からない"         "$KILLED" "0"
absent   "打ち切りの知らせは出ない"     "確認できません" "$SANDBOX/3.log"
if mcp_registered; then ok "MCP 登録が載る"; else ng "MCP 登録が載っていない"; fi

echo "== 4) エッジ: 上限直前に返る（正常扱い）=="
mkenv "slow:1"
run "$SANDBOX/4.log" tty -- setup
echo "  [実測] 所要 ${ELAPSED} 秒（応答 1 秒 / 上限 ${BUDGET} 秒）"
check    "終了コード 0"                 "$RC" "0"
absent   "打ち切らない"                 "確認できません" "$SANDBOX/4.log"
if mcp_registered; then ok "MCP 登録が載る"; else ng "MCP 登録が載っていない"; fi

echo "== 5) エッジ: 少しだけ出してから固まる（パイプが詰まる形）=="
mkenv partial
run "$SANDBOX/5.log" tty -- setup
echo "  [実測] 所要 ${ELAPSED} 秒"
check    "締め切りに掛からず終わる"     "$KILLED" "0"
check    "終了コード 0"                 "$RC" "0"
contains "打ち切りを知らせる"           "[確認できません] claude mcp list（3 秒応答なし）" "$SANDBOX/5.log"
if [ -n "$LEFTOVERS" ]; then ng "打ち切った子が残っている（${LEFTOVERS}）"; else ok "打ち切った子は tako が始末する"; fi

echo "== 6) エッジ: env で上限を上書きできる（既定で済む形は保つ）=="
mkenv hang
BUDGET=2
run "$SANDBOX/6.log" tty -- setup
echo "  [実測] 所要 ${ELAPSED} 秒（上限 2 秒）"
contains "上書きした秒数で知らせる"     "（2 秒応答なし）" "$SANDBOX/6.log"
absent   "既定の 15 秒では待たない"     "（15 秒応答なし）" "$SANDBOX/6.log"
BUDGET=3

echo "== 7) dispatch SetupRun / MCP tako_setup と同条件（--yes --answers - をパイプ）=="
mkenv hang
run "$SANDBOX/7.log" pipe -- setup --yes --answers -
echo "  [実測] 所要 ${ELAPSED} 秒"
check    "締め切りに掛からず終わる"     "$KILLED" "0"
check    "終了コード 0（dispatch は非 0 を失敗として返す）" "$RC" "0"
contains "知らせが stderr に載る（dispatch が拾う行）" "[確認できません] claude mcp list（3 秒応答なし）" "$SANDBOX/7.log"
if mcp_registered; then ok "MCP 登録まで進む"; else ng "MCP 登録が載っていない"; fi

echo
echo "PASS=$PASS FAIL=$FAIL"
[ "$FAIL" -eq 0 ]
