#!/usr/bin/env bash
# #1132: worker ペイン 1 枚の最小幅の保証を**実 GUI + 実 dispatch**で測る。
#
# 隔離した tako-app（実ウィンドウ）へ実 CLI で master + worker 8 体を spawn し、
# `tako list` の cols を実測する。下限を割る spawn が別のタブへ出て、
# どの worker ペインも下限以上の幅になっていることを確かめる。
# 同じ手順を `TAKO_1132_LEGACY=1` でも走らせ、#1132 前の挙動（狭いまま同じタブ）
# へ戻ることを対照として見る。
#
# claude は同梱のスタブへ向くので**実エージェントは起動しない**。
# 本番の tako（GUI / 設定 / tmux / projects / workers）には触らない。
set -uo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TMPDIR_ROOT="$(mktemp -d /tmp/tk1132-shared-XXXXXX)"
trap 'rm -rf "$TMPDIR_ROOT"' EXIT
TAKO_BIN="${TAKO_BIN:-$REPO_ROOT/target/debug/tako}"
APP_BIN="${APP_BIN:-$REPO_ROOT/target/debug/tako-app}"
WORKERS="${WORKERS:-8}"

PASS=0
FAIL=0
pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
bad()  { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }
check() { if [ "$1" = "1" ]; then pass "$2"; else bad "$2"; fi; }

if [ ! -x "$TAKO_BIN" ] || [ ! -x "$APP_BIN" ]; then
  echo "バイナリをビルドします…"
  (cd "$REPO_ROOT" && cargo build -p tako-cli -p tako-app --quiet)
fi
for b in "$TAKO_BIN" "$APP_BIN"; do
  [ -x "$b" ] || { echo "バイナリが見つからない: $b"; exit 1; }
done

# ペインの中から走らせても本番へ届かないように、継承した接続情報を落とす
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_MCP_URL

# GPUI のウィンドウは他のウィンドウに**完全に隠れると描画を止める**（#838 / #470）。
# 描かれないと PTY のセル数が確定せず（既定 80x24 のまま）幅を実測できない。
# ただしユーザーのメイン画面に隔離 tako の窓を出さない（2026-09-06 の恒久指示）ので、
# **仮想ディスプレイ `tako-vd` へ移してから**描かせる。あちらでは何にも覆われないので
# 前面化（= ユーザーのキーボード focus を奪う）は要らない。
# tako-vd が無ければ NEED VD を出して先へ進む（幅を実測できないぶんは skip になる）
VD_ORIGIN=""
vd_origin() {
  if [ -n "$VD_ORIGIN" ]; then printf '%s' "$VD_ORIGIN"; return 0; fi
  local src="$TMPDIR_ROOT/vd-origin.swift"
  cat > "$src" <<'SW'
import AppKit
guard let main = NSScreen.screens.first(where: { $0.frame.origin == .zero })
    ?? NSScreen.screens.first else { exit(1) }
let topY = main.frame.origin.y + main.frame.size.height
guard let vd = NSScreen.screens.first(where: { $0.localizedName == "tako-vd" }) else { exit(2) }
let f = vd.frame
// System Events が使う「主画面の左上を原点に下向き y」へ変換した左上
print("\(Int(f.origin.x)) \(Int(topY - (f.origin.y + f.size.height)))")
SW
  VD_ORIGIN="$(swift "$src" 2>/dev/null || true)"
  printf '%s' "$VD_ORIGIN"
}

place_on_vd() {
  local pid="$1" origin x y
  origin="$(vd_origin)"
  if [ -z "$origin" ]; then
    echo "  NEED VD: 仮想ディスプレイ tako-vd が見つからない（幅の実測は skip になる）"
    return 1
  fi
  x="${origin%% *}"
  y="${origin##* }"
  osascript <<OSA >/dev/null 2>&1
tell application "System Events"
  set p to first process whose unix id is ${pid}
  repeat with w in windows of p
    set position of w to {${x} + 40, ${y} + 40}
  end repeat
end tell
OSA
}

# 各ペインの cols と role・所属タブを取り出す
panes_json() {
  "$TAKO_BIN" list 2>/dev/null | python3 -c '
import json, sys
d = json.load(sys.stdin)
for tab in d.get("tabs", []):
    for p in tab.get("panes", []):
        print(tab.get("id"), p.get("id"), p.get("cols"), (p.get("role") or "-"))
'
}

run_arm() {
  local legacy="$1" label="$2" floor="${3:-}"
  local TMP APP_PID
  TMP="$(mktemp -d "/tmp/tk1132-${legacy}-XXXXXX")"
  echo
  echo "=== ${label}（TAKO_1132_LEGACY=${legacy:-未設定}）==="

  # claude のスタブ（起動したまま居座る = 実物と同じ形。実 API は呼ばない）
  mkdir -p "$TMP/bin"
  cat > "$TMP/bin/claude" <<'STUB'
#!/usr/bin/env bash
echo "STUB_CLAUDE args=$*"
sleep 600
STUB
  chmod +x "$TMP/bin/claude"

  export TAKO_ISOLATED=1
  export TAKO_DATA_DIR="$TMP/data"
  export TAKO_DISCOVERY_DIR="$TMP/disc"
  export TAKO_ORCHESTRATOR_DIR="$TMP/orch"
  export TAKO_SESSIONS_FILE="$TMP/sessions.yaml"
  export TAKO_PANE_LOG_DIR="$TMP/panelogs"
  export TAKO_WORKERS_FILE="$TMP/workers.yaml"
  export TAKO_PERSIST=0
  if [ -n "$legacy" ]; then export TAKO_1132_LEGACY=1; else unset TAKO_1132_LEGACY; fi
  mkdir -p "$TAKO_DATA_DIR" "$TAKO_DISCOVERY_DIR" "$TAKO_ORCHESTRATOR_DIR/profiles"
  # 本番へ書かない不変条件（env が効いていなければここで落とす）
  for d in "$TAKO_DATA_DIR" "$TAKO_ORCHESTRATOR_DIR" "$TAKO_DISCOVERY_DIR"; do
    case "$d" in "$TMP"/*) : ;; *) echo "隔離されていない: $d"; exit 1 ;; esac
  done

  # 既定プロファイル: PATH をスタブへ差し替える（spawn は role 無しペインから呼ぶので default が効く）
  cat > "$TAKO_ORCHESTRATOR_DIR/profiles/default.yaml" <<YAML
model: null
effort: high
env:
  PATH: "$TMP/bin:/usr/bin:/bin"
YAML
  "$TAKO_BIN" orchestrator projects add --key t1132 --cwd "$TMP" \
    --description "#1132 の検証（自動削除される）" >/dev/null 2>&1

  "$APP_BIN" > "$TMP/app.log" 2>&1 &
  APP_PID=$!
  for _ in $(seq 1 200); do "$TAKO_BIN" list >/dev/null 2>&1 && break; sleep 0.1; done
  if ! "$TAKO_BIN" list >/dev/null 2>&1; then
    echo "tako-app へ接続できない:"; tail -20 "$TMP/app.log"
    kill "$APP_PID" 2>/dev/null; return 1
  fi
  pass "隔離 tako-app が起動して CLI から見える（pid ${APP_PID}）"
  # 実ウィンドウを前面へ出して描かせ、PTY のセル数が確定するのを待つ
  place_on_vd "$APP_PID"
  sleep 3

  local min_cols master tab0
  if [ -n "$floor" ]; then
    "$TAKO_BIN" orchestrator layout --min-worker-cols "$floor" >/dev/null 2>&1
  fi
  min_cols="$("$TAKO_BIN" orchestrator layout | python3 -c 'import json,sys; print(json.load(sys.stdin)["min_worker_cols"])')"
  echo "  下限幅: ${min_cols} 桁"
  read -r tab0 master _ _ < <(panes_json | head -1)
  echo "  master: tab=${tab0} pane=${master}"

  # worker を 8 体 spawn（配置先と見積もりを記録）
  local i placement out_tab cols placements=""
  for i in $(seq 1 "$WORKERS"); do
    local resp
    resp="$("$TAKO_BIN" orchestrator spawn --project t1132 --prompt "" \
      --pane "$master" --label "w$i" 2>&1)"
    read -r placement out_tab cols < <(printf '%s' "$resp" | python3 -c '
import json, sys
try:
    d = json.load(sys.stdin)
except Exception:
    print("ERR - -"); raise SystemExit
print(d.get("placement"), d.get("tab"), d.get("pane_cols"))
' 2>/dev/null)
    printf '  spawn w%-2s → placement=%-12s tab=%-4s 見積もり=%s 桁\n' \
      "$i" "${placement:-?}" "${out_tab:-?}" "${cols:-?}"
    placements="$placements ${placement:-?}"
    # 新しいタブのペインが描かれて実寸になるのを待つ（隠れると描画が止まるので毎回前面へ）
    place_on_vd "$APP_PID"
    sleep 1
    if [ "${placement:-ERR}" = "ERR" ]; then
      echo "$resp" | head -3
      bad "w$i の spawn が失敗した"
    fi
  done
  place_on_vd "$APP_PID"
  sleep 3

  echo "  --- tako list の実測 ---"
  panes_json | while read -r t p c r; do
    printf '    tab=%-4s pane=%-4s cols=%-5s role=%s\n' "$t" "$p" "$c" "$r"
  done

  # 判定: worker ペイン（role が orchestrator-worker）の cols が全部下限以上か
  local narrow tabs_used live
  narrow="$(panes_json | awk -v m="$min_cols" '$4 ~ /^orchestrator-worker/ && $3 != "None" && $3+0 < m {n++} END {print n+0}')"
  tabs_used="$(panes_json | awk '$4 ~ /^orchestrator-worker/ {print $1}' | sort -u | wc -l | tr -d ' ')"
  # PTY の桁数は**描かれたフレーム**でしか確定しない（GPUI は窓が隠れると描画を止める。
  # #838 / #470）。worker ペインが全部 spawn 直後の既定 80x24 のままなら「実測できて
  # いない」ので、桁数に依る判定は理由つきで飛ばす（配置の判定は材料が別なので残る）
  live="$(panes_json | awk '$4 ~ /^orchestrator-worker/ && $3 != "None" && $3+0 != 80 {n++} END {print n+0}')"
  echo "  下限を割った worker ペイン: ${narrow} 枚 / 使ったタブ: ${tabs_used} / 実寸が載ったペイン: ${live}"
  if [ -n "$floor" ]; then
    if [ "$live" -eq 0 ]; then
      echo "  SKIP: ウィンドウが描かれず PTY の桁数を実測できないため cols の判定を飛ばす"
    else
      check "$([ "$narrow" -eq 0 ] && echo 1 || echo 0)" \
        "どの worker ペインも下限（${min_cols} 桁）以上（割った枚数=${narrow}）"
    fi
    # 下限を下げると 3 通りの配置が全部出る（同タブ → 新タブ → あふれ先へ詰める）
    for kind in same_tab new_tab overflow_tab; do
      case " $placements " in
        *" $kind "*) pass "配置 ${kind} が実際に選ばれた" ;;
        *) bad "配置 ${kind} が一度も選ばれなかった（実測: ${placements}）" ;;
      esac
    done
  elif [ -z "$legacy" ]; then
    if [ "$live" -eq 0 ]; then
      echo "  SKIP: ウィンドウが描かれず PTY の桁数を実測できないため cols の判定を飛ばす"
    else
      check "$([ "$narrow" -eq 0 ] && echo 1 || echo 0)" \
        "どの worker ペインも下限（${min_cols} 桁）以上（割った枚数=${narrow}）"
    fi
    check "$([ "$tabs_used" -gt 1 ] && echo 1 || echo 0)" \
      "下限を割る spawn が別のタブへ分散した（使ったタブ=${tabs_used}）"
  else
    if [ "$live" -eq 0 ]; then
      echo "  SKIP: ウィンドウが描かれず PTY の桁数を実測できないため cols の判定を飛ばす"
    else
      check "$([ "$narrow" -gt 0 ] && echo 1 || echo 0)" \
        "旧挙動: 下限を割る worker ペインが出る（割った枚数=${narrow}）"
    fi
    check "$([ "$tabs_used" -eq 1 ] && echo 1 || echo 0)" \
      "旧挙動: 全部同じタブへ割る（使ったタブ=${tabs_used}）"
  fi

  # 後始末: 明示 pid だけを落とす
  kill "$APP_PID" 2>/dev/null
  for _ in $(seq 1 50); do kill -0 "$APP_PID" 2>/dev/null || break; sleep 0.1; done
  kill -9 "$APP_PID" 2>/dev/null
  pkill -f "$TMP/bin/claude" 2>/dev/null
  rm -rf "$TMP"
}

run_arm "" "新挙動（既定 60 桁）"
run_arm "" "新挙動（下限 40 桁 = 同タブ / 新タブ / あふれ先の 3 通りが出る）" "40"
run_arm "1" "対照（#1132 前）"

echo
echo "==================================="
echo "PASS: $PASS  FAIL: $FAIL"
[ "$FAIL" -eq 0 ] || exit 1
