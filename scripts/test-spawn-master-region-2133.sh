#!/usr/bin/env bash
# test-spawn-master-region-2133.sh — 引き継ぎの後の worker が master の領域を割らないこと（#2133）
#
# 実 CLI（`tako master` / `tako orchestrator spawn` / `tako orchestrator handoff` / `tako close`）と
# 実 tako-app を、隔離した data / orchestrator / tmux / discovery で走らせ、Issue の 1〜5 を
# そのまま踏む（master_ratio 0.3・worker 3 体から始める = 本番で master が踏んだ形）:
#
#   S1（Issue の 1〜5）: A が worker 3 体 → A が B へ引き継ぐ → A を閉じる → B から spawn →
#       w1 を閉じて spawn → w2 を閉じて spawn。続けて worker を全部閉じてから spawn（境界）
#   S2（境界）: 前任 A と後任 B が並んだまま（A を閉じる前）に B から spawn → その後 A を閉じて spawn
#   S0（境界）: worker 0 本の master からの初回 spawn
#
# 各段で `tako list` の矩形（タブ内の比率）と桁数を読み、
#   - 新しい worker が worker 領域（x ≥ master_ratio）に居るか
#   - master の領域（master の role のペインの幅の和）が master_ratio のままか
#   - 後任 master の桁数が A を閉じた直後の桁数から減っていないか
# を判定する。腕は 2 つ（どちらも同じバイナリ。GUI の env で切り替える）:
#   legacy … TAKO_2133_LEGACY=1（修正前の挙動）。Issue の症状（master が割られて潰れる）を再現できること
#   new    … 既定（修正後）。worker は worker 領域へ入り、master の取り分が保たれること
#
# claude はスタブ（起動したまま居座るだけ）。**本番の GUI・タブ・tmux・設定には触らない**。
#
# 使い方: bash scripts/test-spawn-master-region-2133.sh
#   ARMS="new" のように腕を絞れる（既定 "legacy new"）。
#   EXPECT_NEW=fail は修正前のバイナリで回すとき用（new の腕が落ちることを確かめる）
set -uo pipefail

# 本番 GUI を指す env を最初に落とす（tako のペインの中から走らせると継承される）
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_ORCHESTRATOR_ROLE TAKO_TAB_ID TAKO_MCP_URL

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ARMS="${ARMS:-legacy new}"
EXPECT_NEW="${EXPECT_NEW:-pass}"
RATIO=0.3
PASS=0
FAIL=0
pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
fail() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }
check() { if [ "$1" = "1" ]; then pass "$2"; else fail "$2"; fi; }

TMP_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/tk2133-XXXXXX")"
APP_PID=""
SOCKET=""
cleanup() {
  stop_isolated_gui "$APP_PID"
  [ -n "$SOCKET" ] && tmux -L "$SOCKET" kill-server 2>/dev/null
  rm -rf "$TMP_ROOT"
}
# 途中で死んだテストを「緑」に化けさせない（#1864）
. "$REPO_ROOT/scripts/lib/exit-guard.sh"
tako_exit_trap cleanup

. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
isolated_gui_bins || exit 1
TMUX_DIR="$(dirname "$(command -v tmux)")"

T() { "$TAKO_BIN" "$@"; }

# タブ 1 枚の矩形と桁数の要約（1 行 1 ペイン: id role x width cols spawned_by）
tab_panes() {
  T list 2>/dev/null | python3 -c '
import json, sys
d = json.load(sys.stdin)
tab = int(sys.argv[1])
for t in d.get("tabs", []):
    if t.get("id") != tab:
        continue
    for p in sorted(t.get("panes", []), key=lambda p: (p["rect"]["x"], p["rect"]["y"])):
        r = p["rect"]
        role = p.get("role") or "-"
        kind = "M" if "orchestrator-master" in role else ("W" if "orchestrator-worker" in role else "-")
        print(p["id"], kind, "%.4f" % r["x"], "%.4f" % r["y"], "%.4f" % r["width"], "%.4f" % r["height"],
              p.get("cols"), p.get("spawned_by"))
' "$1"
}

# 段の判定材料: master の領域の幅（M の幅の和。M は横に並ぶ）/ 最も左の worker の x /
# 指定ペインの x・桁数
region_of() {
  tab_panes "$1" | python3 -c '
import sys
rows = [l.split() for l in sys.stdin if l.strip()]
m_w = sum(float(r[4]) for r in rows if r[1] == "M")
w_x = [float(r[2]) for r in rows if r[1] == "W"]
print("%.4f" % m_w, ("%.4f" % min(w_x)) if w_x else "none", len(w_x))
'
}
pane_field() {
  # pane_field <tab> <pane> <x|width|cols>
  tab_panes "$1" | awk -v p="$2" -v f="$3" '$1 == p { if (f == "x") print $3; else if (f == "width") print $5; else print $7 }'
}

dump_tab() {
  echo "    --- タブ $1 の配置（id 種別 x y 幅 高さ 桁 spawned_by）---"
  tab_panes "$1" | sed 's/^/    /'
}

# 新しい worker が worker 領域（x ≥ RATIO）に居るか
in_worker_region() {
  python3 -c 'import sys; print(1 if float(sys.argv[1]) >= float(sys.argv[2]) - 0.002 else 0)' "$1" "$RATIO"
}
# master の領域が RATIO のままか
ratio_kept() {
  python3 -c 'import sys; print(1 if abs(float(sys.argv[1]) - float(sys.argv[2])) < 0.002 else 0)' "$1" "$RATIO"
}

spawn_from() {
  # spawn_from <master> <label> → 新しい pane id（失敗なら空）
  T orchestrator spawn --project p2133 --prompt "" --pane "$1" --label "$2" 2>/dev/null |
    python3 -c 'import json,sys
try:
    d = json.load(sys.stdin)
    print(d["pane_id"])
except Exception:
    print("")'
}

launch_master() {
  # 新しいタブで専用 master を立て、ペイン ID を返す
  T master -d2133 --tab > "$TMP/launch.log" 2>&1 || { cat "$TMP/launch.log" >&2; return 1; }
  grep -o 'ペイン [0-9]*' "$TMP/launch.log" | head -1 | grep -o '[0-9]*'
}
tab_of_pane() {
  T list 2>/dev/null | python3 -c 'import json,sys
d = json.load(sys.stdin)
for t in d["tabs"]:
    for p in t["panes"]:
        if p["id"] == int(sys.argv[1]): print(t["id"]); raise SystemExit
print("")' "$1"
}
role_of_pane() {
  T list 2>/dev/null | python3 -c 'import json,sys
d = json.load(sys.stdin)
for t in d["tabs"]:
    for p in t["panes"]:
        if p["id"] == int(sys.argv[1]): print(p.get("role") or ""); raise SystemExit
print("")' "$1"
}
wait_master_role() {
  local i
  for i in $(seq 1 100); do
    case "$(role_of_pane "$1")" in orchestrator-master*) return 0 ;; esac
    sleep 0.1
  done
  return 1
}
handoff_from() {
  # handoff_from <A> → 後任のペイン ID
  T orchestrator handoffs write --project p2133 --content "## 状態
- 検証用（#2133）" >/dev/null 2>&1 || true
  T orchestrator handoff --pane "$1" 2>/dev/null | python3 -c 'import json,sys
try:
    print(json.load(sys.stdin)["new_master_pane_id"])
except Exception:
    print("")'
}
settle() { sleep "${1:-1.5}"; }

run_arm() {
  local arm="$1"
  local expect_new_region
  TMP="$TMP_ROOT/$arm"
  SOCKET="tk2133-$arm-$$"
  mkdir -p "$TMP"
  echo
  echo "=================== 腕: $arm ==================="

  mkdir -p "$TMP/bin"
  cat > "$TMP/bin/claude" <<'STUB'
#!/usr/bin/env bash
echo "STUB_CLAUDE args=$*"
sleep 600
STUB
  chmod +x "$TMP/bin/claude"
  export PATH="$TMP/bin:$PATH"
  export TAKO_DATA_DIR="$TMP/data"
  export TAKO_DISCOVERY_DIR="$TMP/disc"
  export TAKO_ORCHESTRATOR_DIR="$TMP/orch"
  export TAKO_SESSIONS_FILE="$TMP/sessions.yaml"
  export TAKO_PANE_LOG_DIR="$TMP/panelogs"
  export TAKO_WORKERS_FILE="$TMP/workers.yaml"
  export TAKO_TMUX_SOCKET="$SOCKET"
  export TAKO_PERSIST=0
  mkdir -p "$TAKO_DATA_DIR" "$TAKO_DISCOVERY_DIR" "$TAKO_ORCHESTRATOR_DIR/profiles" "$TMP/proj" "$TMP/claude-cfg"
  for d in "$TAKO_DATA_DIR" "$TAKO_ORCHESTRATOR_DIR" "$TAKO_DISCOVERY_DIR"; do
    case "$d" in "$TMP_ROOT"/*) : ;; *) echo "隔離されていない: $d"; exit 1 ;; esac
  done
  cat > "$TAKO_ORCHESTRATOR_DIR/projects.yaml" <<YAML
projects:
  p2133:
    cwd: "$TMP/proj"
YAML
  for name in default d2133; do
    {
      echo "effort: high"
      [ "$name" = d2133 ] && printf "projects:\n- p2133\n"
      echo "env:"
      echo "  PATH: \"$TMP/bin:$TMUX_DIR:/usr/bin:/bin\""
      echo "  CLAUDE_CONFIG_DIR: \"$TMP/claude-cfg\""
    } > "$TAKO_ORCHESTRATOR_DIR/profiles/$name.yaml"
  done
  # 本番で master が踏んだ取り分（既定は 0.5）
  T orchestrator layout --master-ratio "$RATIO" >/dev/null 2>&1

  local gui_env=""
  [ "$arm" = legacy ] && gui_env="TAKO_2133_LEGACY=1"
  # shellcheck disable=SC2086
  launch_isolated_gui "$TMP/app.log" $gui_env || exit $?
  APP_PID="$ISOLATED_GUI_PID"
  wait_isolated_gui "$TMP/app.log" || { stop_isolated_gui "$APP_PID"; exit 1; }
  pass "隔離 tako-app が起動した（pid ${APP_PID}）"
  local got_ratio
  got_ratio="$(T orchestrator layout 2>/dev/null | python3 -c 'import json,sys; d=json.load(sys.stdin); print(d.get("policy"), d.get("master_ratio"))')"
  echo "  layout: $got_ratio"

  # ---------------- S0: worker 0 本からの初回 spawn ----------------
  echo
  echo "--- S0: worker 0 本の master からの初回 spawn ---"
  local M0 TAB0 n0 r0
  M0="$(launch_master)" || { fail "master を立てられない"; return; }
  wait_master_role "$M0" || fail "master の role が付かない（pane ${M0}）"
  TAB0="$(tab_of_pane "$M0")"
  n0="$(spawn_from "$M0" s0)"
  settle
  dump_tab "$TAB0"
  read -r r0 _ _ < <(region_of "$TAB0")
  check "$(in_worker_region "$(pane_field "$TAB0" "$n0" x)")" "S0: 初回の worker（pane ${n0}）は worker 領域"
  check "$(ratio_kept "$r0")" "S0: master の領域は ${RATIO}（実測 ${r0}）"

  # ---------------- S1: Issue の 1〜5 ----------------
  echo
  echo "--- S1: Issue の 1〜5（A → B へ引き継ぎ → A を閉じる → spawn / close の繰り返し）---"
  local A TAB w1 w2 w3 B n1 n2 n3 n4 b_cols b_cols0 mw wx nw
  A="$(launch_master)" || { fail "master A を立てられない"; return; }
  wait_master_role "$A" || fail "master A の role が付かない"
  TAB="$(tab_of_pane "$A")"
  w1="$(spawn_from "$A" w1)"; settle 1
  w2="$(spawn_from "$A" w2)"; settle 1
  w3="$(spawn_from "$A" w3)"; settle
  echo "  A=${A} w1=${w1} w2=${w2} w3=${w3}"
  dump_tab "$TAB"
  # 1. 引き継ぎ
  B="$(handoff_from "$A")"
  [ -n "$B" ] && pass "1. 引き継ぎで後任 B（pane ${B}）が立った" || { fail "1. 引き継ぎが通らない"; return; }
  settle
  dump_tab "$TAB"
  read -r mw wx nw < <(region_of "$TAB")
  check "$(ratio_kept "$mw")" "1. A と B の領域の和が ${RATIO}（実測 ${mw}）"
  # 2. A を閉じる
  T close --pane "$A" >/dev/null 2>&1
  settle
  dump_tab "$TAB"
  b_cols0="$(pane_field "$TAB" "$B" cols)"
  read -r mw wx nw < <(region_of "$TAB")
  check "$(ratio_kept "$mw")" "2. A を閉じた後の B の幅が ${RATIO}（実測 ${mw}・${b_cols0} 桁）"
  # 3. B から spawn
  n1="$(spawn_from "$B" n1)"; settle
  dump_tab "$TAB"
  read -r mw wx nw < <(region_of "$TAB")
  b_cols="$(pane_field "$TAB" "$B" cols)"
  echo "  3. n1=${n1}: master の領域 ${mw} / B ${b_cols} 桁（A を閉じた直後 ${b_cols0} 桁）"
  local s3_in s3_kept
  s3_in="$(in_worker_region "$(pane_field "$TAB" "$n1" x)")"
  s3_kept="$(ratio_kept "$mw")"
  # 4. w1 を閉じて spawn
  T close --pane "$w1" --force >/dev/null 2>&1; settle 1
  n2="$(spawn_from "$B" n2)"; settle
  dump_tab "$TAB"
  read -r mw wx nw < <(region_of "$TAB")
  local s4_in s4_kept b4
  s4_in="$(in_worker_region "$(pane_field "$TAB" "$n2" x)")"
  s4_kept="$(ratio_kept "$mw")"
  b4="$(pane_field "$TAB" "$B" cols)"
  echo "  4. n2=${n2}: master の領域 ${mw} / B ${b4} 桁"
  # 5. w2 を閉じて spawn
  T close --pane "$w2" --force >/dev/null 2>&1; settle 1
  n3="$(spawn_from "$B" n3)"; settle
  dump_tab "$TAB"
  read -r mw wx nw < <(region_of "$TAB")
  local s5_in s5_kept b5 n3_cols
  s5_in="$(in_worker_region "$(pane_field "$TAB" "$n3" x)")"
  s5_kept="$(ratio_kept "$mw")"
  b5="$(pane_field "$TAB" "$B" cols)"
  n3_cols="$(pane_field "$TAB" "$n3" cols)"
  echo "  5. n3=${n3}: master の領域 ${mw} / B ${b5} 桁 / n3 ${n3_cols} 桁"
  echo "  B の桁の推移: A を閉じた直後 ${b_cols0} → 3. ${b_cols} → 4. ${b4} → 5. ${b5}"
  echo "  RESULT_${arm}_S1 b_cols=${b_cols0},${b_cols},${b4},${b5} n3_cols=${n3_cols} master_region=${mw}"

  if [ "$arm" = legacy ]; then
    # 修正前の挙動 = Issue の症状を再現できていること
    check "$([ "$s3_in" = 0 ] && echo 1 || echo 0)" "legacy 3.: n1 が master の領域に入った（= Issue の症状）"
    check "$([ "$s5_in" = 0 ] && [ "$s5_kept" = 0 ] && echo 1 || echo 0)" "legacy 5.: n3 も master の領域で、master の取り分が割られた（${mw}）"
    check "$(python3 -c 'import sys; print(1 if int(sys.argv[2]) < int(sys.argv[1]) // 3 else 0)' "$b_cols0" "$b5")" \
      "legacy: B が A を閉じた直後の 1/3 未満まで潰れた（${b_cols0} → ${b5} 桁）"
  else
    check "$s3_in" "3. n1 は worker 領域（x ≥ ${RATIO}）"
    check "$s3_kept" "3. master の領域が ${RATIO} のまま"
    check "$s4_in" "4. w1 を閉じた後の n2 は worker 領域"
    check "$s4_kept" "4. master の領域が ${RATIO} のまま"
    check "$s5_in" "5. w2 を閉じた後の n3 は worker 領域"
    check "$s5_kept" "5. master の領域が ${RATIO} のまま"
    check "$([ "$b_cols" = "$b_cols0" ] && [ "$b4" = "$b_cols0" ] && [ "$b5" = "$b_cols0" ] && echo 1 || echo 0)" \
      "3.〜5. B の桁が A を閉じた直後（${b_cols0} 桁）から変わらない"
    # 境界: worker を全部閉じた後の spawn
    T close --pane "$n1" --force >/dev/null 2>&1
    T close --pane "$n2" --force >/dev/null 2>&1
    T close --pane "$n3" --force >/dev/null 2>&1
    T close --pane "$w3" --force >/dev/null 2>&1
    settle 1
    n4="$(spawn_from "$B" n4)"; settle
    dump_tab "$TAB"
    read -r mw wx nw < <(region_of "$TAB")
    check "$(in_worker_region "$(pane_field "$TAB" "$n4" x)")" "境界: worker を全部閉じた後の n4 は worker 領域"
    check "$(ratio_kept "$mw")" "境界: そのときも master の領域が ${RATIO}（実測 ${mw}）"
  fi

  # ---------------- S2: 前任と後任が並んだまま spawn ----------------
  echo
  echo "--- S2: 前任 A と後任 B が並んだまま（A を閉じる前）に B から spawn ---"
  local A2 TAB2 v1 v2 B2 m1 m2 s2_in s2_kept s2b_in s2b_kept
  A2="$(launch_master)" || { fail "master A2 を立てられない"; return; }
  wait_master_role "$A2" || fail "master A2 の role が付かない"
  TAB2="$(tab_of_pane "$A2")"
  v1="$(spawn_from "$A2" v1)"; settle 1
  v2="$(spawn_from "$A2" v2)"; settle 1
  B2="$(handoff_from "$A2")"; settle
  m1="$(spawn_from "$B2" m1)"; settle
  dump_tab "$TAB2"
  read -r mw wx nw < <(region_of "$TAB2")
  s2_in="$(in_worker_region "$(pane_field "$TAB2" "$m1" x)")"
  s2_kept="$(ratio_kept "$mw")"
  echo "  S2: m1=${m1}: master の領域（A2 + B2）${mw}"
  T close --pane "$A2" >/dev/null 2>&1; settle 1
  m2="$(spawn_from "$B2" m2)"; settle
  dump_tab "$TAB2"
  read -r mw wx nw < <(region_of "$TAB2")
  s2b_in="$(in_worker_region "$(pane_field "$TAB2" "$m2" x)")"
  s2b_kept="$(ratio_kept "$mw")"
  echo "  RESULT_${arm}_S2 m1_in=${s2_in} m2_in=${s2b_in} master_region=${mw}"
  if [ "$arm" = legacy ]; then
    check "$([ "$s2_in" = 0 ] && echo 1 || echo 0)" "legacy S2: 並んだままの spawn は B2 の右（master の領域）へ入った"
  else
    check "$s2_in" "S2: 並んだままの spawn（m1）は worker 領域"
    check "$s2_kept" "S2: そのとき A2 + B2 の領域は ${RATIO} のまま"
    check "$s2b_in" "S2: A2 を閉じた後の spawn（m2）も worker 領域"
    check "$s2b_kept" "S2: A2 を閉じた後の B2 は ${RATIO}（実測 ${mw}）"
  fi

  stop_isolated_gui "$APP_PID"
  APP_PID=""
  tmux -L "$SOCKET" kill-server 2>/dev/null
  SOCKET=""
}

for arm in $ARMS; do
  run_arm "$arm"
done

echo
echo "PASS=${PASS} FAIL=${FAIL}"
if [ "$EXPECT_NEW" = fail ]; then
  # 修正前のバイナリで new の腕を回したとき: 落ちることが期待（= A/B の「修正前」）
  [ "$FAIL" -gt 0 ] && { echo "期待どおり new の判定が落ちた（修正前）"; tako_exit 0; }
  echo "修正前のはずなのに new の判定が通った"; exit 1
fi
[ "$FAIL" -eq 0 ] && tako_exit 0
exit 1
