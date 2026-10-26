#!/usr/bin/env bash
# test-net-resume-2157.sh — ネット復帰の即時再開の取りこぼし（#2157）の実経路テスト（A/B）
#
# #2112 の実経路テスト（scripts/test-net-resume-2112.sh）と同じ形で、隔離した data / HOME / tmux に
# **実 tako-app** を立て、claude の画面の形を描く偽のエージェント（scripts/lib/fake-agent-net-2112.sh。
# API へは繋がない）を並べる。同じ手順を**新しい判定**と **`TAKO_2157_LEGACY=1`（#2157 前）**の
# 2 回まわして、次を比べる:
#   ① ターンの締めの後に通知行（plugin.json の再読み込み）が挟まった画面（pane 2670 の形・worker）を
#      接続エラーの停止として拾い、回線の復帰で「続き」が届く（旧: 拾わない・届かない）
#   ② 停止 → 人の入力で解除（4 秒の生成中を見せる）→ 再び停止（solo）。2 回目の停止も拾い、
#      回線の復帰で届く（旧も同じ = 差は無い。解除と再検知の回帰の確認）
#   ②' その後も「続き」を受けるたび生成中を挟んで同じ形で止まり直す。新しい判定は生成中
#      （会話が進んだ画面）を見て送った停止の鍵を捨て、打ち切り（30 分で 3 回）まで届ける。
#      旧は 2 回目の止まり直しの鍵が前と重なって止まる
#   ③ 止まった時点で回線がもう戻っている（master）。待たずに届く。「続き」を受けてすぐ同じ形で
#      止まり直すと、停止の鍵が前と重なる。新しい判定は送達が届いたのを見て次の停止と読み、
#      打ち切り（30 分で 3 回）まで届ける。旧は 2 回目の止まり直しを「送った停止」と読んで止まる
#   ④ 本当に会話が先へ進んだ画面（エラーの後に人の発言と返答・締めの後のツールの結果・
#      通知の後で生成中）へは届かない（新旧とも）
#   ⑤ persist.log に①の停止の検知が出る（新）/ 出ない（旧）
#
# 回線断は**到達確認の先を env（`TAKO_NET_PROBE`）で 127.0.0.1 の空きポートへ差し替えて**作る。
# この PC の実ネットワークには触らない（API へも 1 バイトも出ない）。
#
# **本番の tako / 設定には一切触らない**（data / HOME は mktemp 配下、tmux は専用ソケット、
# 落とすのは自分で起こした pid だけ）。窓は常設の仮想ディスプレイ（tako-vd）へ出る（#1141）。
# **CI には登録しない**（実 GUI を立てる）。CI 側の担保は単体テスト（`tako_control::net_stop` の
# `issue2157_*`）と番犬 `crates/tako-control/tests/issue2157_net_resume_redetect_watchdog.rs`。
#
# 使い方: bash scripts/test-net-resume-2157.sh
set -uo pipefail
# **本番 GUI を指す env を最初に落とす**（#1449 / #1450 で本番にペインが漏れた実例が 2 件）
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_ORCHESTRATOR_ROLE TAKO_MCP_URL \
  TAKO_BACKEND_SOCKET TAKO_CLI

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
. "$REPO_ROOT/scripts/lib/tmux-socket.sh"
FAKE="$REPO_ROOT/scripts/lib/fake-agent-net-2112.sh"
PASS=0
FAIL=0
pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
fail() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }
check_eq() {
  if [ "$2" = "$3" ]; then pass "$1"; else fail "$1（期待 '${2}' / 実際 '${3}'）"; fi
}

TMP="$(mktemp -d /tmp/tako-2157-XXXXXX)"
APP_PID=""
LISTEN_PID=""
TMUX_SOCKET=""
stop_listener() {
  if [ -n "$LISTEN_PID" ]; then
    kill "$LISTEN_PID" 2>/dev/null
    wait "$LISTEN_PID" 2>/dev/null
    LISTEN_PID=""
  fi
}
kill_tmux() {
  [ -n "$TMUX_SOCKET" ] && tmux_socket_kill "$TMUX_SOCKET"
  TMUX_SOCKET=""
}
cleanup() {
  stop_listener
  stop_isolated_gui "$APP_PID"
  kill_tmux
  rm -rf "$TMP"
}
trap cleanup EXIT

# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
# ビルドは HOME を差し替える前に（後だとツールチェーンを一時 HOME へ取り直す）
isolated_gui_bins || exit 1

# --- 隔離した環境 -------------------------------------------------------------
export HOME="$TMP/home"
mkdir -p "$HOME"
printf "PROMPT='tako %%1~ %%%% '\nRPROMPT=''\n" > "$HOME/.zshrc"
export TAKO_ISOLATED=1

# 到達確認の先 = 127.0.0.1 の空きポート（開いていない = 回線断、待ち受けを立てる = 復帰）
listening() { lsof -nP -iTCP:"$1" -sTCP:LISTEN >/dev/null 2>&1; }
PORT=""
for p in $(seq 47200 47299); do
  if ! listening "$p"; then PORT="$p"; break; fi
done
[ -n "$PORT" ] || { echo "空きポートが見つからない"; exit 1; }
start_listener() {
  /usr/bin/perl -MIO::Socket::INET -e '
    my $s = IO::Socket::INET->new(LocalAddr => "127.0.0.1", LocalPort => $ARGV[0],
      Listen => 16, ReuseAddr => 1) or die "listen: $!";
    while (my $c = $s->accept) { close $c; }' "$PORT" 2>"$TMP/listener.err" &
  LISTEN_PID=$!
  for _ in $(seq 1 50); do
    listening "$PORT" && return 0
    sleep 0.1
  done
  echo "待ち受けが立たない: $(cat "$TMP/listener.err")"
  return 1
}

LOG="$TMP/received.log"
: > "$LOG"
# 届いた「続き」（自動通知）の数と、届いた文の総数
nudges() { grep " $1 RECEIVED " "$LOG" 2>/dev/null | grep -c -e "自動通知" -e "auto-notice" || true; }
received() { grep -c " $1 RECEIVED " "$LOG" 2>/dev/null || true; }

T() { "$TAKO_BIN" "$@" 2>&1; }
net() { T limit-resume --network; }
jqv() { jq -r "$1" 2>/dev/null; }
now_ms() { /usr/bin/perl -MTime::HiRes=time -e 'printf "%.3f", time'; }

# 偽のエージェントを**自分のタブ**の右側の大きなペインで立て、role を貼る
spawn_fake() {
  local mode="$1" name="$2" role="$3" base pane
  base="$(T tab new --title "$name" | jqv .pane)"
  pane="$(T split --pane "$base" --right --ratio 0.75 -- /bin/bash "$FAKE" "$mode" "$name" "$LOG" | tr -dc '0-9')"
  [ -n "$role" ] && T title --pane "$pane" --role "$role" >/dev/null
  echo "$pane"
}

# 状態の `network.stopped` でのペインの state:hold（載っていなければ none）
state_of() {
  net | jqv "[.stopped[] | select(.pane == $1) | (.state + \":\" + (.hold // \"\"))][0] // \"none\""
}
since_of() {
  net | jqv "[.stopped[] | select(.pane == $1) | .since][0] // \"none\""
}
# state:hold が期待になるまで最大 N 秒待つ（届かなければ画面の末尾を stderr へ）
wait_state() {
  local pane="$1" want="$2" secs="${3:-20}" got=""
  for _ in $(seq 1 "$secs"); do
    got="$(state_of "$pane")"
    [ "$got" = "$want" ] && break
    sleep 1
  done
  if [ "$got" != "$want" ]; then
    echo "    --- pane $pane の画面" >&2
    T read --pane "$pane" --lines 16 | sed 's/^/    | /' >&2
  fi
  echo "$got"
}
# 名前 $1 への自動通知が $2 通以上になるまで最大 $3 秒待つ
wait_nudges() {
  local name="$1" want="$2" secs="$3"
  for _ in $(seq 1 $((secs * 2))); do
    [ "$(nudges "$name")" -ge "$want" ] && return 0
    sleep 0.5
  done
  return 1
}

# 1 回まわす。$1 = new / legacy
# 回ごとに data / discovery / orchestrator / tmux を分ける（前の回の器のセッションを
# 次の GUI が拾うと、繋がった先のタブ数が変わる）
run_round() {
  local label="$1" legacy_env="" d
  [ "$label" = legacy ] && legacy_env="TAKO_2157_LEGACY=1"
  export TAKO_DATA_DIR="$TMP/$label/data"
  export TAKO_DISCOVERY_DIR="$TMP/$label/disc"
  export TAKO_ORCHESTRATOR_DIR="$TMP/$label/orch"
  export TAKO_SESSIONS_FILE="$TMP/$label/sessions.yaml"
  export TAKO_PANE_LOG_DIR="$TMP/$label/panelogs"
  export TAKO_WORKERS_FILE="$TMP/$label/workers.yaml"
  TMUX_SOCKET="tako-2157-$$-$label"
  export TAKO_TMUX_SOCKET="$TMUX_SOCKET"
  mkdir -p "$TAKO_DATA_DIR" "$TAKO_DISCOVERY_DIR" "$TAKO_ORCHESTRATOR_DIR"
  for d in "$HOME" "$TAKO_DATA_DIR" "$TAKO_ORCHESTRATOR_DIR" "$TAKO_DISCOVERY_DIR"; do
    case "$d" in
      "$TMP"/*) : ;;
      *) echo "隔離されていないディレクトリ: $d"; exit 1 ;;
    esac
  done
  printf '{"auto_rename": false}\n' > "$TAKO_DATA_DIR/settings.json"
  stop_listener

  echo "== [$label] 隔離 GUI を起こす（到達確認の先 = 127.0.0.1:${PORT}・回線断）${legacy_env:+ $legacy_env}=="
  # shellcheck disable=SC2086
  launch_isolated_gui "$TMP/app-$label.log" TAKO_NET_PROBE="127.0.0.1:$PORT" TAKO_PERSIST=1 $legacy_env || exit $?
  APP_PID="$ISOLATED_GUI_PID"
  wait_isolated_gui "$TMP/app-$label.log" || exit 1
  local tabs
  tabs="$(T list | jqv '.tabs | length')"
  if [ "$tabs" != "1" ]; then
    echo "繋がった先が隔離インスタンスではない（タブ ${tabs} 枚）。中止する。"; exit 1
  fi
  echo "  pid=$APP_PID"

  local n_notice="notice-$label" n_slow="slow-$label" n_again="again-$label"
  local P_NOTICE P_SLOW P_ANSWERED P_TOOL P_GEN P_AGAIN
  P_NOTICE="$(spawn_fake notice "$n_notice" orchestrator-worker:t)"
  P_SLOW="$(spawn_fake again-slow "$n_slow" solo)"
  P_ANSWERED="$(spawn_fake answered "answered-$label" orchestrator-worker:t)"
  P_TOOL="$(spawn_fake toolafter "tool-$label" orchestrator-worker:t)"
  P_GEN="$(spawn_fake generating "gen-$label" orchestrator-worker:t)"
  echo "  通知行=$P_NOTICE 止まり直し(solo)=$P_SLOW 返答済み=$P_ANSWERED ツール=$P_TOOL 生成中=$P_GEN"

  echo "== [$label] ① 締めの後に通知行が挟まった画面（回線断のあいだ）=="
  if [ "$label" = new ]; then
    check_eq "① [new] 通知行の画面を回線待ちとして拾う" "waiting_network:" "$(wait_state "$P_NOTICE" "waiting_network:")"
  else
    sleep 6
    check_eq "① [legacy] 通知行の画面を拾わない（取りこぼしの再現）" "none" "$(state_of "$P_NOTICE")"
  fi

  echo "== [$label] ② 停止 → 人の入力で解除 → 再び停止（回線断のあいだ）=="
  check_eq "② 1 回目の停止を拾う" "waiting_network:" "$(wait_state "$P_SLOW" "waiting_network:")"
  local s1 s2
  s1="$(since_of "$P_SLOW")"
  T send --pane "$P_SLOW" "続き" >/dev/null
  # 生成中（4 秒）のあいだは止まっていない
  local seen_gone=""
  for _ in $(seq 1 8); do
    sleep 0.5
    [ "$(state_of "$P_SLOW")" = "none" ] && { seen_gone=1; break; }
  done
  if [ -n "$seen_gone" ]; then pass "② 人の入力で解除される（生成中は止まっていない）"; else fail "② 人の入力の後も停止のまま"; fi
  check_eq "② 2 回目の停止も拾う" "waiting_network:" "$(wait_state "$P_SLOW" "waiting_network:" 20)"
  s2="$(since_of "$P_SLOW")"
  if [ "$s2" != "none" ] && [ "$s1" != "none" ] && [ "$s2" -gt "$s1" ] 2>/dev/null; then
    pass "② 2 回目は別の停止として拾い直した（since ${s1} → ${s2}）"
  else
    fail "② 2 回目の停止の since が変わらない（${s1} → ${s2}）"
  fi
  # 受けた記録は 2 回で 1 本なので、この回の名前（`-<label>`）だけを数える
  check_eq "② 回線断のあいだは自動通知を 1 通も送らない" "0" \
    "$(grep -e "-$label RECEIVED " "$LOG" | grep -c -e 自動通知 -e auto-notice || true)"

  echo "== [$label] 回線の復帰（待ち受けを開く）=="
  start_listener || exit 1
  if [ "$label" = new ]; then
    if wait_nudges "$n_notice" 1 20; then pass "① [new] 通知行の画面へ「続き」が届いた"; else fail "① [new] 通知行の画面へ届かない"; fi
  else
    sleep 8
    check_eq "① [legacy] 通知行の画面へは届かない（取りこぼしの再現）" "0" "$(nudges "$n_notice")"
  fi
  if wait_nudges "$n_slow" 1 20; then pass "② 2 回目の停止へ「続き」が届いた（solo）"; else fail "② 2 回目の停止へ届かない"; fi

  echo "== [$label] ③ 止まった時点で回線がもう戻っている・同じ形で止まり直す（master）=="
  local t0 t1
  t0="$(now_ms)"
  P_AGAIN="$(spawn_fake again "$n_again" orchestrator-master)"
  echo "  止まり直し(master)=$P_AGAIN"
  if wait_nudges "$n_again" 1 20; then
    t1="$(grep " $n_again RECEIVED " "$LOG" | head -1 | awk '{print $1}')"
    pass "③ 回線がもう戻っている停止へ $(awk -v a="$t0" -v b="$t1" 'BEGIN{printf "%.1f", b-a}') 秒で届いた（偽ペインを立てた時刻から）"
  else
    fail "③ 回線がもう戻っている停止へ届かない"
  fi
  # 打ち切り（30 分で 3 回）まで、または旧の止まりまで待つ
  sleep 30
  if [ "$label" = new ]; then
    check_eq "③ [new] 止まり直しのたびに届く（打ち切りの 3 回まで）" "3" "$(nudges "$n_again")"
    check_eq "③ [new] 4 回目は打ち切りで見送る" "held:too_many_resumes" "$(state_of "$P_AGAIN")"
  else
    check_eq "③ [legacy] 2 回目の止まり直しから先は届かない（鍵が重なる）" "2" "$(nudges "$n_again")"
    check_eq "③ [legacy] 届いていない停止を「送った」と読んでいる" "resumed:" "$(state_of "$P_AGAIN")"
  fi
  # ②のペインも「続き」を受けるたび 4 秒の生成中を挟んで同じ形で止まり直している（回線の復帰の後）。
  # 生成中（会話が進んだ画面）を見て鍵を捨てる経路の A/B: 新は打ち切りの 3 回まで、旧は 2 回目の
  # 止まり直しの鍵が 1 回目と重なって 2 通で止まる（1 通目は人の入力の後の停止へ）
  if [ "$label" = new ]; then
    check_eq "②' [new] 生成中を挟む止まり直しにも打ち切りの 3 回まで届く" "3" "$(nudges "$n_slow")"
  else
    check_eq "②' [legacy] 生成中を挟む止まり直しは 2 通で止まる" "2" "$(nudges "$n_slow")"
  fi

  echo "== [$label] ④ 本当に会話が先へ進んだ画面へは届かない =="
  check_eq "④ エラーの後に人の発言と返答" "0" "$(received "answered-$label")"
  check_eq "④ 締めの後にツールの結果" "0" "$(received "tool-$label")"
  check_eq "④ 通知の後で生成中" "0" "$(received "gen-$label")"
  check_eq "④ 3 本とも止まっているとは言わない" "none none none" \
    "$(state_of "$P_ANSWERED") $(state_of "$P_TOOL") $(state_of "$P_GEN")"

  echo "== [$label] ⑤ persist.log =="
  local plog
  plog="$(grep '\[net-resume\]' "$TAKO_DATA_DIR/persist.log" 2>/dev/null)"
  echo "$plog" | sed 's/^/    /' | tail -12
  if [ "$label" = new ]; then
    if grep -q "pane=$P_NOTICE 接続エラーで停止 kind=dns" <<<"$plog"; then
      pass "⑤ [new] 通知行の画面の停止の検知が出る"
    else
      fail "⑤ [new] 通知行の画面の停止の検知が無い"
    fi
    check_eq "⑤ [new] 止まり直しの検知が 4 行（3 回届けて 4 回目は見送り）" "4" \
      "$(echo "$plog" | grep -c "pane=$P_AGAIN 接続エラーで停止")"
    if grep -q "pane=$P_AGAIN 見送り reason=too_many_resumes" <<<"$plog"; then
      pass "⑤ [new] 打ち切りの見送りの理由が出る"
    else
      fail "⑤ [new] 打ち切りの見送りの行が無い"
    fi
  else
    check_eq "⑤ [legacy] 通知行の画面の停止の検知は出ない" "0" \
      "$(echo "$plog" | grep -c "pane=$P_NOTICE 接続エラーで停止")"
  fi
  if grep -q "API Error\|ENOTFOUND\|自動通知\|auto-notice" "$TAKO_DATA_DIR/persist.log" 2>/dev/null; then
    fail "⑤ persist.log に画面の文字か送った文が出ている"
  else
    pass "⑤ persist.log に画面の文字・送った文が出ない"
  fi

  stop_isolated_gui "$APP_PID"
  APP_PID=""
  stop_listener
  kill_tmux
  sleep 1
}

run_round new
run_round legacy

echo
echo "結果: PASS=$PASS FAIL=$FAIL"
[ "$FAIL" -eq 0 ]
