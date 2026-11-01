#!/usr/bin/env bash
# test-periodic-prep-offload-2647.sh — 2 秒 tick の判定がペインの画面を UI スレッドで読まないことの実経路テスト（#2647）
#
# 隔離した data / HOME で**実 tako-app** を立て、claude の画面の形を描く偽のエージェント
# （`scripts/lib/fake-agent-tick-2647.sh`。API へは繋がない・実 claude は起こさない）を並べて、
#   ① 画面を読むロックへ遅延を注入（`TAKO_2647_INJECT_READ_MS`。PTY のスレッドがロックを握っている
#      状態の再現・負荷は作らない）しても、別の IPC（`tako list`）が待たされず、perf.log に
#      5 つの段（agent_metrics / queued_recovery / handoff_nudge / limit_autoresume / net_autoresume）の
#      専有が出ない。A/B（`TAKO_2647_LEGACY=1` = 修正前）では同じ注入で `tako list` が待たされ、段の専有が出る
#   ② 判定の結果と発火の時機が A/B で同じ: 修正後と修正前の GUI を同時に立て、同じ偽の状態から
#      master の引き継ぎの知らせ（#749。ctx 75% / 起動後の猶予 60 秒）・利用制限からの自動復帰
#      （#813。解除時刻 + 120 秒）・ネット復帰の再開（#2112）・キュー滞留の救出（#572）が、同じ文面で
#      同じ回数・同じ時機（2 秒 tick の 1 回ぶんの揺れまで）に届く
#   ③ 境界: エージェントのペインが 0 本のあいだも注入のもとで待たされない・背景で読んでいる途中に
#      ペインを閉じても GUI が落ちない（閉じたペインは読まない）
# を実測する。
#
# **本番の tako / 設定には一切触らない**（data / HOME は mktemp 配下、落とすのは自分で起こした pid だけ）。
# 窓は常設の仮想ディスプレイ（tako-vd）へ出る（#1141）。面を用意できなければ終了コード 4（未実測）。
# **CI には登録しない**（実 GUI を立てる）。CI 側の担保は単体テスト（`tick_screens` の
# `背景で採った1枚は修正前の読みと同じ答えを返す`）と番犬
# （`crates/tako-control/tests/issue2647_periodic_prep_offload_watchdog.rs`）。
#
# 使い方: bash scripts/test-periodic-prep-offload-2647.sh
#   修正前と比べるなら TAKO_BIN=<CLI> APP_BIN=<GUI> を渡す。② だけ / ①③ だけは TAKO_2647_ONLY=ab / perf
set -uo pipefail
# **本番 GUI を指す env を最初に落とす**（#1449 / #1450 で本番にペインが漏れた実例が 2 件）
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_ORCHESTRATOR_ROLE TAKO_MCP_URL \
  TAKO_BACKEND_SOCKET TAKO_CLI TMUX TAKO_2647_LEGACY TAKO_2647_INJECT_READ_MS

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
FAKE="$REPO_ROOT/scripts/lib/fake-agent-tick-2647.sh"
PASS=0
FAIL=0
pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
fail() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }
check_eq() {
  if [ "$2" = "$3" ]; then pass "$1"; else fail "$1（期待 '${2}' / 実際 '${3}'）"; fi
}
check_lt() {
  if [ -n "$3" ] && [ "$3" -lt "$2" ] 2>/dev/null; then pass "$1（${3}）"; else fail "$1（${2} 未満のはずが '${3}'）"; fi
}
check_ge() {
  if [ -n "$3" ] && [ "$3" -ge "$2" ] 2>/dev/null; then pass "$1（${3}）"; else fail "$1（${2} 以上のはずが '${3}'）"; fi
}

for bin in jq perl python3; do
  command -v "$bin" >/dev/null 2>&1 || { echo "未実測: ${bin} が無い"; exit 4; }
done

ONLY="${TAKO_2647_ONLY:-}"
# 注入する遅延（ms）。画面を読むロック 1 回ごと
INJECT_MS="${TAKO_2647_INJECT_MS:-30}"
# ① のエージェントのペインの本数と、`tako list` を叩き続ける秒数
PANES="${TAKO_2647_PANES:-8}"
HOLD="${TAKO_2647_HOLD:-20}"
# 「待たされていない」の上限（ms）。debug ビルドの CLI の起動と往復を含めて余裕を持たせる
QUICK_MS="${TAKO_2647_QUICK_MS:-250}"
# unix ソケットのパス長の上限に当たらないよう短い置き場にする
TMP="$(mktemp -d /tmp/t2647-XXXXXX)"
PIDS=""
LISTEN_PID=""

# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
# ビルドは HOME を差し替える前に（後だとツールチェーンを一時 HOME へ取り直す）
isolated_gui_bins || exit 1

cleanup() {
  local p
  for p in $PIDS; do stop_isolated_gui "$p"; done
  if [ -n "$LISTEN_PID" ]; then
    kill "$LISTEN_PID" 2>/dev/null
    wait "$LISTEN_PID" 2>/dev/null
  fi
  if [ -n "${KEEP:-}" ]; then echo "置き場を残した: $TMP"; else rm -rf "${TMP:?}"; fi
}
trap cleanup EXIT

export HOME="$TMP/home"
mkdir -p "$HOME"
printf "PROMPT='tako %%1~ %%%% '\nRPROMPT=''\n" > "$HOME/.zshrc"
export TAKO_ISOLATED=1

# GUI 1 つぶんの置き場（名前ごと）。② は 2 つを同時に立てるので、CLI は置き場を行ごとに渡す
inst_dir() { echo "$TMP/$1"; }
# T <名前> <tako の引数…>: その GUI へだけ繋ぐ（発見の置き場と data dir は一時の側）
T() {
  local d
  d="$(inst_dir "$1")"
  shift
  env -u TAKO_SOCKET -u TAKO_PANE_ID TAKO_ISOLATED=1 TAKO_DATA_DIR="$d/data" \
    TAKO_DISCOVERY_DIR="$d/disc" "$TAKO_BIN" "$@" 2>&1
}
jqv() { jq -r "$1" 2>/dev/null; }
now_ms() { /usr/bin/perl -MTime::HiRes=time -e 'printf "%d", time*1000'; }

# start_gui <名前> [VAR=VAL…]: GUI を立てて繋がるまで待つ。pid は GUI_PID へ
start_gui() {
  local name="$1" d
  shift
  d="$(inst_dir "$name")"
  mkdir -p "$d/data" "$d/disc" "$d/orch"
  case "$d" in "$TMP"/*) : ;; *) echo "隔離されていないディレクトリ: $d"; exit 1 ;; esac
  printf '{"auto_rename": false}\n' > "$d/data/settings.json"
  : > "$d/recv.log"
  launch_isolated_gui "$d/app.log" TAKO_DATA_DIR="$d/data" TAKO_DISCOVERY_DIR="$d/disc" \
    TAKO_ORCHESTRATOR_DIR="$d/orch" TAKO_SESSIONS_FILE="$d/sessions.yaml" \
    TAKO_PANE_LOG_DIR="$d/panelogs" TAKO_WORKERS_FILE="$d/workers.yaml" \
    TAKO_PERF_LOG="$d/perf.log" TAKO_PERF_VERBOSE=1 ${@+"$@"} || exit $?
  GUI_PID="$ISOLATED_GUI_PID"
  PIDS="$PIDS $GUI_PID"
  local i
  for i in $(seq 1 300); do
    T "$name" list >/dev/null 2>&1 && break
    sleep 0.1
  done
  if [ "$(T "$name" list | jqv '.tabs | length')" != "1" ]; then
    echo "繋がった先が隔離インスタンスではない（${name}）。中止する。"
    tail -20 "$d/app.log"
    exit 1
  fi
}
stop_gui() {
  stop_isolated_gui "$1"
  local rest="" p
  for p in $PIDS; do [ "$p" = "$1" ] || rest="$rest $p"; done
  PIDS="$rest"
}

# perf.log の 5 つの段の専有の行数（`メインスレッド専有: periodic_prep:<段>`。verbose の 16ms 以上）
stage_stalls() {
  grep -cE "メインスレッド専有: periodic_prep:(agent_metrics|queued_recovery|handoff_nudge|limit_autoresume|net_autoresume) " \
    "$(inst_dir "$1")/perf.log" 2>/dev/null || true
}
# `tako list` を HOLD 秒叩き続けた往復（ms）の p50 / p95 / max を "p50 p95 max" で
list_latency() {
  local name="$1" hold="$2" out end t0 t1
  out="$(inst_dir "$name")/list-$3.txt"
  : > "$out"
  end=$(( $(date +%s) + hold ))
  while [ "$(date +%s)" -lt "$end" ]; do
    t0=$(now_ms)
    T "$name" list >/dev/null
    t1=$(now_ms)
    echo $((t1 - t0)) >> "$out"
    sleep 0.2
  done
  python3 - "$out" <<'EOF'
import sys
v = sorted(int(x) for x in open(sys.argv[1]) if x.strip())
q = lambda p: v[min(len(v) - 1, int(len(v) * p))]
print(q(.5), q(.95), v[-1])
EOF
}

# エージェントのペインを 4 本ずつのタブで足す（タブの先頭は master・残りは worker）
add_agents() {
  local name="$1" count="$2" redraw="$3" recv i tab first second id ctx role dir target
  recv="$(inst_dir "$name")/recv.log"
  i=0
  while [ "$i" -lt "$count" ]; do
    if [ $((i % 4)) -eq 0 ]; then
      tab="$(T "$name" tab new --title "a$i" | jqv .pane)"
      sleep 0.5
      T "$name" send --pane "$tab" "exec env FAKE_CTX=55 FAKE_REDRAW=$redraw /bin/bash $FAKE ctx a$i $recv" >/dev/null
      T "$name" title --pane "$tab" --role "orchestrator-master:m$i" >/dev/null
      first="$tab"
    else
      dir=--right
      target="$first"
      [ $((i % 4)) -ge 2 ] && dir=--down
      [ $((i % 4)) -eq 3 ] && target="$second"
      id="$(T "$name" split --pane "$target" $dir -- env FAKE_CTX=30 FAKE_REDRAW="$redraw" /bin/bash "$FAKE" ctx "a$i" "$recv" | tr -dc '0-9')"
      [ $((i % 4)) -eq 1 ] && second="$id"
      T "$name" title --pane "$id" --role "orchestrator-worker:p:a$i" >/dev/null
    fi
    i=$((i + 1))
  done
  [ "$count" -gt 0 ] && T "$name" limit-resume on --all >/dev/null
}

# ---------------------------------------------------------------------------------------------
# ① + ③: 注入のもとで待たされない（修正後）/ 待たされる（A/B の修正前）
# ---------------------------------------------------------------------------------------------
perf_run() {
  local mode="$1" name="perf-$1" extra=""
  [ "$mode" = legacy ] && extra="TAKO_2647_LEGACY=1"
  echo "== ① ${mode}: 注入 ${INJECT_MS}ms / ロック 1 回 =="
  start_gui "$name" TAKO_2647_INJECT_READ_MS="$INJECT_MS" $extra
  local pid="$GUI_PID"
  sleep 4
  local base0
  base0="$(stage_stalls "$name")"
  read -r Z50 Z95 ZMAX <<EOF2
$(list_latency "$name" 8 zero)
EOF2
  echo "  ③ エージェント 0 本: tako list p50=${Z50}ms p95=${Z95}ms max=${ZMAX}ms・段の専有 +$(( $(stage_stalls "$name") - base0 ))"
  add_agents "$name" "$PANES" 1
  sleep 6
  local base
  base="$(stage_stalls "$name")"
  read -r L50 L95 LMAX <<EOF2
$(list_latency "$name" "$HOLD" agents)
EOF2
  local stalls=$(( $(stage_stalls "$name") - base ))
  echo "  エージェント ${PANES} 本: tako list p50=${L50}ms p95=${L95}ms max=${LMAX}ms・段の専有 ${stalls} 件（${HOLD} 秒）"
  grep -hE "メインスレッド専有: periodic_prep" "$(inst_dir "$name")/perf.log" | tail -4 | sed 's/^/    | /'
  grep -h "span 統計 \[\(bg:\)\?periodic_tick_screens\]\|span 統計 \[periodic_prep:tick_screens\]" "$(inst_dir "$name")/perf.log" | tail -2 | sed 's/^/    | /'
  if [ "$mode" = new ]; then
    check_lt "③ 0 本のあいだも tako list は待たされない（p95）" "$QUICK_MS" "$Z95"
    check_lt "① 注入のもとで tako list は待たされない（p95）" "$QUICK_MS" "$L95"
    check_eq "① 5 つの段の専有は 0 件" "0" "$stalls"
    # ③ 背景で読んでいる途中にペインを閉じる（読みは ${PANES} 本 × ${INJECT_MS}ms 以上かかる）
    local ids p closed=0
    ids="$(T "$name" list | jq -r '[.tabs[].panes[] | select((.role // "") | startswith("orchestrator-worker"))][0:3][].id')"
    for p in $ids; do
      sleep 0.4
      T "$name" close --pane "$p" --force >/dev/null && closed=$((closed + 1))
    done
    sleep 5
    check_eq "③ 読みの途中で閉じても GUI は生きている" "alive" "$(kill -0 "$pid" 2>/dev/null && echo alive || echo dead)"
    check_eq "③ 閉じたペイン（${closed} 本）は一覧から消えた" "0" \
      "$(T "$name" list | jq "[.tabs[].panes[].id] as \$a | [$(echo "$ids" | paste -sd, -)] | map(select(. as \$x | \$a | index(\$x))) | length")"
    check_eq "③ panic が無い" "0" "$(grep -c 'panicked' "$(inst_dir "$name")/app.log" || true)"
  else
    check_ge "① A/B（修正前）は同じ注入で tako list が待たされる（p95）" "$QUICK_MS" "$L95"
    check_ge "① A/B（修正前）は 5 つの段の専有が出る" "1" "$stalls"
  fi
  stop_gui "$pid"
}

# ---------------------------------------------------------------------------------------------
# ②: 判定の結果と発火の時機（修正後と修正前を同時に立てて同じ偽の状態を見せる）
# ---------------------------------------------------------------------------------------------
listening() { lsof -nP -iTCP:"$1" -sTCP:LISTEN >/dev/null 2>&1; }
spawn_one() {
  local name="$1" mode="$2" who="$3" role="$4" env_extra="$5" base pane recv
  recv="$(inst_dir "$name")/recv.log"
  base="$(T "$name" tab new --title "$who" | jqv .pane)"
  pane="$(T "$name" split --pane "$base" --right --ratio 0.75 -- env $env_extra /bin/bash "$FAKE" "$mode" "$who" "$recv" | tr -dc '0-9')"
  T "$name" title --pane "$pane" --role "$role" >/dev/null
  echo "$pane"
}
ab_setup() {
  local name="$1" p
  p="$(spawn_one "$name" ctx master orchestrator-master "FAKE_CTX=75")"
  echo "$(now_ms) master $p" >> "$(inst_dir "$name")/setup.log"
  p="$(spawn_one "$name" limit limit orchestrator-worker:p:limit "FAKE_RESET=$RESET")"
  T "$name" limit-resume on --pane "$p" >/dev/null
  echo "$(now_ms) limit $p" >> "$(inst_dir "$name")/setup.log"
  p="$(spawn_one "$name" net net orchestrator-worker:p:net "FAKE_CTX=30")"
  echo "$(now_ms) net $p" >> "$(inst_dir "$name")/setup.log"
  p="$(spawn_one "$name" queued queued orchestrator-worker:p:queued "FAKE_CTX=30")"
  echo "$(now_ms) queued $p" >> "$(inst_dir "$name")/setup.log"
}
# 1 行 1 事象: "<who> <届いた回数> <最初に届くまでの秒（小数 1 桁）> <文面の sha1 の先頭 12 桁>"
ab_summary() {
  local d
  d="$(inst_dir "$1")"
  python3 - "$d/setup.log" "$d/recv.log" "$d" <<'EOF'
import sys, hashlib, os
setup = {}
# 文面に入る GUI ごとの置き場（引き継ぎファイルのパス）は比べる前に伏せる
inst = sorted({sys.argv[3], os.path.realpath(sys.argv[3])}, key=len, reverse=True)
def norm(text):
    for d in inst:
        text = text.replace(d, "<inst>")
    return text
for l in open(sys.argv[1]):
    ms, who, pane = l.split()
    setup[who] = int(ms) / 1000
got = {}
for l in open(sys.argv[2], encoding="utf-8", errors="replace"):
    parts = l.rstrip("\n").split(" ", 3)
    if len(parts) < 3 or parts[2] != "RECEIVED":
        continue
    got.setdefault(parts[1], []).append((float(parts[0]), parts[3] if len(parts) > 3 else ""))
for who in ["master", "limit", "net", "queued"]:
    # 1.5 秒以内に続いた行は同じ 1 回の送達（複数行の文面を行ごとに受けた場合の保険）
    deliveries = []
    for t, text in got.get(who, []):
        if deliveries and t - deliveries[-1][1] < 1.5:
            deliveries[-1][1] = t
            deliveries[-1][2].append(text)
        else:
            deliveries.append([t, t, [text]])
    if not deliveries:
        print(who, 0, "-", "-")
        continue
    t, _, texts = deliveries[0]
    digest = hashlib.sha1(norm("\n".join(texts)).encode()).hexdigest()[:12]
    print(who, len(deliveries), "%.1f" % (t - setup[who]), digest)
EOF
}
ab_run() {
  echo "== ② 判定の結果と発火の時機（修正後と A/B の修正前を同時に立てる）=="
  local port="" p
  for p in $(seq 47220 47299); do
    if ! listening "$p"; then port="$p"; break; fi
  done
  [ -n "$port" ] || { echo "空きポートが見つからない"; FAIL=$((FAIL + 1)); return; }
  # ネット復帰の到達確認の先 = 127.0.0.1 の待ち受け（回線は通っている = 止まったペインへすぐ送る）
  /usr/bin/perl -MIO::Socket::INET -e '
    my $s = IO::Socket::INET->new(LocalAddr => "127.0.0.1", LocalPort => $ARGV[0],
      Listen => 16, ReuseAddr => 1) or die "listen: $!";
    while (my $c = $s->accept) { close $c; }' "$port" 2>"$TMP/listener.err" &
  LISTEN_PID=$!
  # 利用制限の解除時刻 = 2 分後の分（Asia/Tokyo）。両方の GUI に同じ文字列を見せる
  RESET="$(TZ=Asia/Tokyo date -v+2M '+%-I:%M%p' | tr 'APM' 'apm')"
  echo "  利用制限の解除時刻: ${RESET} (Asia/Tokyo)・到達確認の先: 127.0.0.1:${port}"
  start_gui ab-new TZ=Asia/Tokyo TAKO_NET_PROBE="127.0.0.1:$port"
  local pid_new="$GUI_PID"
  start_gui ab-legacy TZ=Asia/Tokyo TAKO_NET_PROBE="127.0.0.1:$port" TAKO_2647_LEGACY=1
  local pid_legacy="$GUI_PID"
  ab_setup ab-new &
  local s1=$!
  ab_setup ab-legacy &
  local s2=$!
  wait "$s1" "$s2"
  # 最後に届くのは利用制限（解除時刻 + 120 秒）。両方で届くまで待つ（上限 6 分）
  local i
  for i in $(seq 1 180); do
    if grep -q " limit RECEIVED " "$(inst_dir ab-new)/recv.log" 2>/dev/null \
      && grep -q " limit RECEIVED " "$(inst_dir ab-legacy)/recv.log" 2>/dev/null; then
      break
    fi
    sleep 2
  done
  sleep 6
  local new legacy
  new="$(ab_summary ab-new)"
  legacy="$(ab_summary ab-legacy)"
  echo "  修正後:"
  echo "$new" | sed 's/^/    /'
  echo "  A/B（修正前）:"
  echo "$legacy" | sed 's/^/    /'
  local who n_cnt l_cnt n_t l_t n_h l_h
  for who in master limit net queued; do
    read -r _ n_cnt n_t n_h <<EOF2
$(echo "$new" | grep "^$who ")
EOF2
    read -r _ l_cnt l_t l_h <<EOF2
$(echo "$legacy" | grep "^$who ")
EOF2
    check_eq "② ${who}: 届いた回数が同じ（どちらも 1 回）" "1 1" "$n_cnt $l_cnt"
    check_eq "② ${who}: 文面が同じ" "$l_h" "$n_h"
    # 2 秒 tick の 1 回ぶん（+ 起こした順の差）の揺れまでは同じ時機
    check_eq "② ${who}: 時機の差は 2.5 秒以内（修正後 ${n_t}s / 修正前 ${l_t}s）" "ok" \
      "$(python3 -c "import sys; a, b = sys.argv[1], sys.argv[2]; print('ok' if a != '-' and b != '-' and abs(float(a) - float(b)) <= 2.5 else 'ng')" "$n_t" "$l_t")"
  done
  # 監査ログ（#749 / #813 の記録。`<data_dir>/supervisor.log`）の行の種類と回数が同じ
  local audit_new audit_legacy
  audit_new="$(grep -hoE 'ctx_handoff_nudge|limit_autoresume' "$(inst_dir ab-new)/data/supervisor.log" 2>/dev/null | sort | uniq -c | tr -s ' \n' ' ')"
  audit_legacy="$(grep -hoE 'ctx_handoff_nudge|limit_autoresume' "$(inst_dir ab-legacy)/data/supervisor.log" 2>/dev/null | sort | uniq -c | tr -s ' \n' ' ')"
  echo "  監査（supervisor.log）: 修正後 [${audit_new}] / 修正前 [${audit_legacy}]"
  check_eq "② 監査ログの行の種類と回数が同じ" "$audit_legacy" "$audit_new"
  echo "  master へ届いた文面（修正後・先頭 80 文字）: $(grep ' master RECEIVED ' "$(inst_dir ab-new)/recv.log" | head -1 | cut -d' ' -f4- | cut -c1-80)"
  stop_gui "$pid_new"
  stop_gui "$pid_legacy"
}

if [ "$ONLY" != ab ]; then
  perf_run new
  perf_run legacy
fi
if [ "$ONLY" != perf ]; then
  ab_run
fi

echo
echo "結果: PASS=$PASS FAIL=$FAIL"
[ "$FAIL" -eq 0 ]
