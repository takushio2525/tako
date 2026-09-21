#!/usr/bin/env bash
# #1439: worker ペインの下限桁数を**同じタブのまま**確保していることを
# 実 GUI + 実 dispatch で測る（#1132 の別タブ配置からの方針変更）。
#
# 隔離した tako-app（実ウィンドウ）へ実 CLI で master + worker 8 体を spawn し、
# `tako list` のタブ数と cols を実測する。既定では**タブは 1 枚のまま**で、
# 足りない桁数は worker ペインのフォントを縮めて確保する（床でも届かなければ
# `cols_short=true` と実桁数が応答に載る）。
#
# 対照は 2 腕:
#   - TAKO_1439_LEGACY=1 … #1132 の挙動（下限を割ると別のタブへ出る）
#   - TAKO_1132_LEGACY=1 … #1132 前の挙動（狭いまま同じタブ = 17 桁まで潰れる）
#
# claude は同梱のスタブへ向くので**実エージェントは起動しない**。
# 本番の tako（GUI / 設定 / tmux / projects / workers）には触らない。
set -uo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TMPDIR_ROOT="$(mktemp -d /tmp/tk1439-shared-XXXXXX)"
trap 'rm -rf "$TMPDIR_ROOT"' EXIT
# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
WORKERS="${WORKERS:-8}"

PASS=0
FAIL=0
pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
bad()  { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }
check() { if [ "$1" = "1" ]; then pass "$2"; else bad "$2"; fi; }

isolated_gui_bins || exit 1

# ペインの中から走らせても本番へ届かないように、継承した接続情報を落とす
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_MCP_URL

# 隔離 tako は `TAKO_ISOLATED=1` だけで**常設の仮想ディスプレイ `tako-vd` に開く**
# （#1141 / #1150 / #1160。既定の面がそれ）。あちらでは何にも覆われないので、
# GPUI の「完全に隠れると描画を止める」（#838 / #470）にも当たらず、
# 窓を動かす必要が無い。
#
# **AX（System Events）で窓を動かしてはいけない（#1442）**: プロセス名がどちらも
# `tako-app` なので、複数の tako-app が動いていると**本番 /Applications の窓に当たる**
# （#782 の計測で実測）。ユーザーの窓を動かす事故になるので、この器では一切使わない。
# 面を用意する（眠っていれば起こす）のは launch_isolated_gui の中（#1490）。
# `tako-vd` が無い環境では既定の面へ落ちて続行し、面が 1 枚も見えないときは
# tako 側が窓を開かずに終わる（#1160）。どちらも幅を実測できないぶんは skip になる。

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
  local arm="$1" label="$2"
  local TMP APP_PID
  TMP="$(mktemp -d "/tmp/tk1439-${arm}-XXXXXX")"
  echo
  echo "=== ${label}（腕: ${arm}）==="

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
  # A/B の腕は 1 つずつ独立に効く（両方立てない）
  unset TAKO_1132_LEGACY TAKO_1439_LEGACY
  case "$arm" in
    split) export TAKO_1439_LEGACY=1 ;;
    narrow) export TAKO_1132_LEGACY=1 ;;
  esac
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
  "$TAKO_BIN" orchestrator projects add --key t1439 --cwd "$TMP" \
    --description "#1439 の検証（自動削除される）" >/dev/null 2>&1

  launch_isolated_gui "$TMP/app.log"
  APP_PID="$ISOLATED_GUI_PID"
  if ! wait_isolated_gui "$TMP/app.log"; then
    stop_isolated_gui "$APP_PID"; return 1
  fi
  pass "隔離 tako-app が起動して CLI から見える（pid ${APP_PID}）"
  # tako-vd 上で描かれて PTY のセル数が確定するのを待つ
  sleep 3

  local min_cols master tab0 policy
  min_cols="$("$TAKO_BIN" orchestrator layout | python3 -c 'import json,sys; print(json.load(sys.stdin)["min_worker_cols"])')"
  policy="$("$TAKO_BIN" orchestrator layout | python3 -c 'import json,sys; d=json.load(sys.stdin); print(d.get("placement"), d.get("auto_shrink_font"), d.get("min_worker_font_scale"))')"
  echo "  下限幅: ${min_cols} 桁 / 方針: ${policy}"
  if [ "$arm" = "default" ]; then
    # #1439: layout 応答に有効な方針が出る（CLI / MCP で同じ 1 実装）
    check "$([ "$policy" = "same_tab True 0.6" ] && echo 1 || echo 0)" \
      "layout 応答が「同じタブ + 自動縮小 + 床 0.6」を返す（実測: ${policy}）"
  fi
  read -r tab0 master _ _ < <(panes_json | head -1)
  echo "  master: tab=${tab0} pane=${master}"

  # worker を 8 体 spawn（配置・倍率・桁数を記録）
  local i placement out_tab cols scale short placements="" shorts="" scales=""
  for i in $(seq 1 "$WORKERS"); do
    local resp
    resp="$("$TAKO_BIN" orchestrator spawn --project t1439 --prompt "" \
      --pane "$master" --label "w$i" 2>&1)"
    read -r placement out_tab cols scale short < <(printf '%s' "$resp" | python3 -c '
import json, sys
try:
    d = json.load(sys.stdin)
except Exception:
    print("ERR - - - -"); raise SystemExit
print(d.get("placement"), d.get("tab"), d.get("pane_cols"), d.get("font_scale"), d.get("cols_short"))
' 2>/dev/null)
    printf '  spawn w%-2s → placement=%-12s tab=%-4s cols=%-5s font_scale=%-6s cols_short=%s\n' \
      "$i" "${placement:-?}" "${out_tab:-?}" "${cols:-?}" "${scale:-?}" "${short:-?}"
    placements="$placements ${placement:-?}"
    shorts="$shorts ${short:-?}"
    scales="$scales ${scale:-?}"
    # 新しいペインが描かれて実寸になるのを待つ
    sleep 1
    if [ "${placement:-ERR}" = "ERR" ]; then
      echo "$resp" | head -3
      bad "w$i の spawn が失敗した"
    fi
  done
  sleep 3

  echo "  --- tako list の実測 ---"
  panes_json | while read -r t p c r; do
    printf '    tab=%-4s pane=%-4s cols=%-5s role=%s\n' "$t" "$p" "$c" "$r"
  done

  local narrow tabs_used live worker_min
  narrow="$(panes_json | awk -v m="$min_cols" '$4 ~ /^orchestrator-worker/ && $3 != "None" && $3+0 < m {n++} END {print n+0}')"
  tabs_used="$(panes_json | awk '$4 ~ /^orchestrator-worker/ {print $1}' | sort -u | wc -l | tr -d ' ')"
  # PTY の桁数は**描かれたフレーム**でしか確定しない（GPUI は窓が隠れると描画を止める。
  # #838 / #470）。worker ペインが全部 spawn 直後の既定 80x24 のままなら「実測できて
  # いない」ので、桁数に依る判定は理由つきで飛ばす（配置の判定は材料が別なので残る）
  live="$(panes_json | awk '$4 ~ /^orchestrator-worker/ && $3 != "None" && $3+0 != 80 {n++} END {print n+0}')"
  worker_min="$(panes_json | awk '$4 ~ /^orchestrator-worker/ && $3 != "None" {if (m == "" || $3+0 < m) m = $3+0} END {print (m == "" ? 0 : m)}')"
  echo "  下限を割った worker ペイン: ${narrow} 枚 / 使ったタブ: ${tabs_used} / 実寸が載ったペイン: ${live} / 最狭 ${worker_min} 桁"
  # この腕の最狭桁数を呼び出し側へ残す（腕どうしの比較に使う）
  echo "$worker_min" > "$TMPDIR_ROOT/min-$arm"

  case "$arm" in
    default)
      check "$([ "$tabs_used" -eq 1 ] && echo 1 || echo 0)" \
        "#1439: worker 8 体でもタブは 1 枚のまま（使ったタブ=${tabs_used}）"
      case " $placements " in
        *" new_tab "*|*" overflow_tab "*) bad "別タブへ逃がした配置が出た（実測: ${placements}）" ;;
        *) pass "配置は全部 same_tab（実測: ${placements}）" ;;
      esac
      if [ "$live" -eq 0 ]; then
        echo "  SKIP: ウィンドウが描かれず PTY の桁数を実測できないため cols の判定を飛ばす"
      else
        # 下限に届いたか、届かないなら床で置いて cols_short が立っていること
        if [ "$narrow" -eq 0 ]; then
          pass "どの worker ペインも下限（${min_cols} 桁）以上（割った枚数=0）"
        else
          case " $shorts " in
            *" True "*) pass "床でも届かないぶんは cols_short=true で申告した（実測: ${shorts}）" ;;
            *) bad "下限を割ったのに cols_short が立っていない（実測: ${shorts}）" ;;
          esac
          case " $scales " in
            *" 0.6 "*) pass "床（0.6）まで縮めた（実測: ${scales}）" ;;
            *) bad "床まで縮めていない（実測: ${scales}）" ;;
          esac
        fi
      fi
      ;;
    split)
      check "$([ "$tabs_used" -gt 1 ] && echo 1 || echo 0)" \
        "対照（#1132）: 下限を割る spawn が別のタブへ分散した（使ったタブ=${tabs_used}）"
      ;;
    narrow)
      check "$([ "$tabs_used" -eq 1 ] && echo 1 || echo 0)" \
        "対照（#1132 前）: 全部同じタブへ割る（使ったタブ=${tabs_used}）"
      if [ "$live" -eq 0 ]; then
        echo "  SKIP: ウィンドウが描かれず PTY の桁数を実測できないため cols の判定を飛ばす"
      else
        check "$([ "$narrow" -gt 0 ] && echo 1 || echo 0)" \
          "対照（#1132 前）: 下限を割る worker ペインが出る（割った枚数=${narrow}）"
      fi
      ;;
  esac

  # 後始末: 明示 pid だけを落とす
  stop_isolated_gui "$APP_PID"
  pkill -f "$TMP/bin/claude" 2>/dev/null
  rm -rf "$TMP"
}

run_arm "default" "新挙動（#1439: 同じタブ + フォント自動縮小）"
run_arm "split" "対照（#1132: 別タブへ逃がす。TAKO_1439_LEGACY=1）"
run_arm "narrow" "対照（#1132 前: 狭いまま同じタブ。TAKO_1132_LEGACY=1）"

# 腕どうしの比較: 同じタブに詰めたまま、縮めたぶんだけ桁数が広い（#1439 の効き目）
if [ -f "$TMPDIR_ROOT/min-default" ] && [ -f "$TMPDIR_ROOT/min-narrow" ]; then
  d="$(cat "$TMPDIR_ROOT/min-default")"
  n="$(cat "$TMPDIR_ROOT/min-narrow")"
  if [ "$d" -gt 0 ] && [ "$n" -gt 0 ]; then
    check "$([ "$d" -gt "$n" ] && echo 1 || echo 0)" \
      "同じ 1 タブでも縮小のぶん最狭が広い（新 ${d} 桁 > 対照 ${n} 桁）"
  else
    echo "  SKIP: 実寸が載らなかった腕があるため腕どうしの桁数比較を飛ばす"
  fi
fi

echo
echo "==================================="
echo "PASS: $PASS  FAIL: $FAIL"
[ "$FAIL" -eq 0 ] || exit 1
