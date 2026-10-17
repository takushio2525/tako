#!/usr/bin/env bash
# test-net-resume-2181.sh — 低いペインで締めの行より上が画面の外の停止（#2181）の実経路テスト（A/B）
#
# #2157 の実経路テスト（scripts/test-net-resume-2157.sh）と同じ形で、隔離した data / HOME / tmux に
# **実 tako-app** を立て、claude と同じく会話を**流し書き**する偽のエージェント
# （scripts/lib/fake-agent-net-2112.sh の offscreen 系。API へは繋がない）を**低いペイン**
# （タブの下側・LOW_MIN..LOW_MAX 行）で並べる。画面に見えるのは締めの行・通知行・入力欄だけで、
# エラーの項目は画面の外（器の履歴 / 外側の端末の履歴）にある = pane 2716 の形。
# 次の 3 回をまわして比べる:
#   new     器（tmux）のペイン・新しい判定
#   legacy  器（tmux）のペイン・`TAKO_2181_LEGACY=1`（#2181 前 = 見えている行だけで判定する）
#   direct  器の無い直接のペイン（`TAKO_PERSIST=0`）・新しい判定（外側の端末の履歴をメモリから読む）
# 見るもの:
#   ⓪ 前提: ペインが低い（LOW_MAX 行以下）・見えている画面にエラーの頭の行が無い・
#      器の履歴（new / legacy）にはある
#   ① 締めの行 + 通知だけが見える停止（worker）を回線待ちとして拾い、回線の復帰で「続き」が 1 通だけ届く
#      （legacy: 拾わない・届かない）
#   ② 通知の無い形（締めの行 + ヒントだけが見え、エラーの頭が画面の外。master）も同じ
#   ③ 画面の外が普通の返答・同じターンにログイン失効の停止へは届かない（新旧とも）
#   ④ 届いた後は会話が進んだ画面になり、2 通目は送らない
#   ⑤ persist.log に停止の検知が出る（new / direct）/ 出ない（legacy）。画面の文字・送った文は出ない
#
# 回線断は**到達確認の先を env（`TAKO_NET_PROBE`）で 127.0.0.1 の空きポートへ差し替えて**作る。
# この PC の実ネットワークには触らない（API へも 1 バイトも出ない）。
#
# **本番の tako / 設定には一切触らない**（data / HOME は mktemp 配下、tmux は専用ソケット、
# 落とすのは自分で起こした pid だけ）。窓は常設の仮想ディスプレイ（tako-vd）へ出る（#1141）。
# **CI には登録しない**（実 GUI を立てる）。CI 側の担保は単体テスト（`tako_control::net_stop` の
# `issue2181_*`・`tako_core::terminal` の `issue2181_*`）と番犬
# `crates/tako-control/tests/issue2181_net_resume_offscreen_watchdog.rs`。
#
# 使い方: bash scripts/test-net-resume-2181.sh
set -uo pipefail
# **本番 GUI を指す env を最初に落とす**（#1449 / #1450 で本番にペインが漏れた実例が 2 件）
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_ORCHESTRATOR_ROLE TAKO_MCP_URL \
  TAKO_BACKEND_SOCKET TAKO_CLI

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
FAKE="$REPO_ROOT/scripts/lib/fake-agent-net-2112.sh"
# 低いペインの行数の幅。pane 2716 と同じく「締めの行・通知・入力欄で埋まり、エラーの頭の行が
# 画面の外」になる高さ（偽のエージェントの描く形で、10 行ならエラーは 11 行目 = 画面の外・
# 締めの行は見える）。縦の取り分は窓の寸法に依らず、この幅に入るまで探す（fit_low）
LOW_MIN="${LOW_MIN:-9}"
LOW_MAX="${LOW_MAX:-10}"
PASS=0
FAIL=0
pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
fail() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }
check_eq() {
  if [ "$2" = "$3" ]; then pass "$1"; else fail "$1（期待 '${2}' / 実際 '${3}'）"; fi
}

TMP="$(mktemp -d /tmp/tako-2181-XXXXXX)"
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
  [ -n "$TMUX_SOCKET" ] && tmux -L "$TMUX_SOCKET" kill-server >/dev/null 2>&1
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
for p in $(seq 47300 47399); do
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
# 偽のエージェントが最後に測った端末の行数
rows_of() { grep " $1 SIZE " "$LOG" 2>/dev/null | tail -1 | awk '{print $4}' | cut -dx -f1; }

T() { "$TAKO_BIN" "$@" 2>&1; }
net() { T limit-resume --network; }
jqv() { jq -r "$1" 2>/dev/null; }

# ペイン $1（偽のエージェントの名前 $2）の行数が LOW_MIN..LOW_MAX に入るまで縦の取り分を探す
fit_low() {
  local pane="$1" name="$2" share rows i
  for share in 0.3 0.35 0.4 0.45 0.5 0.55 0.6 0.65 0.7 0.75 0.8; do
    T resize --pane "$pane" --share-y "$share" >/dev/null
    rows=""
    for i in 1 2 3 4 5 6; do
      sleep 0.3
      rows="$(rows_of "$name")"
      [ -n "$rows" ] && [ "$rows" -ge "$LOW_MIN" ] && [ "$rows" -le "$LOW_MAX" ] && return 0
    done
    # 高くなりすぎたら戻らない（取り分は単調）
    [ -n "$rows" ] && [ "$rows" -gt "$LOW_MAX" ] && return 1
  done
  return 1
}

# 偽のエージェントを**自分のタブ**の下側の低いペインで立て、role を貼る
spawn_low() {
  local mode="$1" name="$2" role="$3" base pane
  base="$(T tab new --title "$name" | jqv .pane)"
  pane="$(T split --pane "$base" --down --ratio 0.3 -- /bin/bash "$FAKE" "$mode" "$name" "$LOG" | tr -dc '0-9')"
  fit_low "$pane" "$name" || echo "    $name: ${LOW_MIN}..${LOW_MAX} 行に収められない（$(rows_of "$name") 行）" >&2
  [ -n "$role" ] && T title --pane "$pane" --role "$role" >/dev/null
  echo "$pane"
}

# 見えている画面に $2 が何行あるか
visible_count() { T read --pane "$1" | grep -c -e "$2" || true; }
# 器（tmux）の履歴 + 画面に $2 が何行あるか（偽のエージェントの見出しで器のセッションを探す）
history_count() {
  local name="$1" needle="$2" s text
  for s in $(tmux -L "$TMUX_SOCKET" list-sessions -F '#{session_name}' 2>/dev/null); do
    text="$(tmux -L "$TMUX_SOCKET" capture-pane -p -t "$s" -S -300 2>/dev/null)"
    if grep -q "fake agent (#2181 $name)" <<<"$text"; then
      printf '%s\n' "$text" | grep -c -e "$needle" || true
      return
    fi
  done
  echo "no-session"
}

# 状態の `network.stopped` でのペインの state:hold（載っていなければ none）
state_of() {
  net | jqv "[.stopped[] | select(.pane == $1) | (.state + \":\" + (.hold // \"\"))][0] // \"none\""
}
# state:hold が期待になるまで最大 N 秒待つ（届かなければ画面を stderr へ）
wait_state() {
  local pane="$1" want="$2" secs="${3:-20}" got=""
  for _ in $(seq 1 "$secs"); do
    got="$(state_of "$pane")"
    [ "$got" = "$want" ] && break
    sleep 1
  done
  if [ "$got" != "$want" ]; then
    echo "    --- pane $pane の画面" >&2
    T read --pane "$pane" | sed 's/^/    | /' >&2
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

# 1 回まわす。$1 = new / legacy / direct
run_round() {
  local label="$1" extra_env="" persist=1 d
  [ "$label" = legacy ] && extra_env="TAKO_2181_LEGACY=1"
  [ "$label" = direct ] && persist=0
  export TAKO_DATA_DIR="$TMP/$label/data"
  export TAKO_DISCOVERY_DIR="$TMP/$label/disc"
  export TAKO_ORCHESTRATOR_DIR="$TMP/$label/orch"
  export TAKO_SESSIONS_FILE="$TMP/$label/sessions.yaml"
  export TAKO_PANE_LOG_DIR="$TMP/$label/panelogs"
  export TAKO_WORKERS_FILE="$TMP/$label/workers.yaml"
  TMUX_SOCKET="tako-2181-$$-$label"
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

  echo "== [$label] 隔離 GUI を起こす（到達確認の先 = 127.0.0.1:${PORT}・回線断・TAKO_PERSIST=${persist}）${extra_env:+ $extra_env}=="
  # shellcheck disable=SC2086
  launch_isolated_gui "$TMP/app-$label.log" TAKO_NET_PROBE="127.0.0.1:$PORT" TAKO_PERSIST=$persist $extra_env || exit $?
  APP_PID="$ISOLATED_GUI_PID"
  wait_isolated_gui "$TMP/app-$label.log" || exit 1
  local tabs
  tabs="$(T list | jqv '.tabs | length')"
  if [ "$tabs" != "1" ]; then
    echo "繋がった先が隔離インスタンスではない（タブ ${tabs} 枚）。中止する。"; exit 1
  fi
  echo "  pid=$APP_PID"

  local n_off="off-$label" n_bare="bare-$label" n_ans="ans-$label" n_login="login-$label"
  local P_OFF P_BARE P_ANS P_LOGIN
  P_OFF="$(spawn_low offscreen "$n_off" orchestrator-worker:t)"
  P_BARE="$(spawn_low offscreen-bare "$n_bare" orchestrator-master)"
  P_ANS="$(spawn_low offscreen-answered "$n_ans" orchestrator-worker:t)"
  P_LOGIN="$(spawn_low offscreen-login "$n_login" orchestrator-worker:t)"
  echo "  通知=$P_OFF 通知なし=$P_BARE 普通の返答=$P_ANS ログイン失効=$P_LOGIN"
  sleep 3

  echo "== [$label] ⓪ 前提: 低いペインで、エラーの頭の行は画面の外 =="
  local name rows
  for name in "$n_off" "$n_bare" "$n_ans" "$n_login"; do
    rows="$(rows_of "$name")"
    if [ -n "$rows" ] && [ "$rows" -ge "$LOW_MIN" ] && [ "$rows" -le "$LOW_MAX" ] 2>/dev/null; then
      pass "⓪ $name のペインは ${rows} 行（${LOW_MIN}..${LOW_MAX} 行）"
    else
      fail "⓪ $name のペインが低くない（${rows:-不明} 行）"
    fi
  done
  check_eq "⓪ 見えている画面にエラーの頭の行が無い（通知 / 通知なし / ログイン失効）" "0 0 0" \
    "$(visible_count "$P_OFF" "API Error") $(visible_count "$P_BARE" "API Error") $(visible_count "$P_LOGIN" "API Error")"
  check_eq "⓪ 見えている画面に締めの行がある（通知 / 通知なし）" "1 1" \
    "$(visible_count "$P_OFF" "Worked for") $(visible_count "$P_BARE" "Worked for")"
  if [ "$persist" = 1 ]; then
    check_eq "⓪ 器の履歴にはエラーがある（通知 / 通知なし）" "1 1" \
      "$(history_count "$n_off" "API Error: Can't reach") $(history_count "$n_bare" "API Error: Can't reach")"
  else
    check_eq "⓪ 器を使っていない（直接のペイン）" "" "$(tmux -L "$TMUX_SOCKET" list-sessions -F '#{session_name}' 2>/dev/null)"
  fi

  echo "== [$label] ①② 回線断のあいだ =="
  if [ "$label" = legacy ]; then
    sleep 8
    check_eq "① [legacy] 締めの行 + 通知だけが見える停止を拾わない（取りこぼしの再現）" "none" "$(state_of "$P_OFF")"
    check_eq "② [legacy] 通知の無い形も拾わない" "none" "$(state_of "$P_BARE")"
  else
    check_eq "① [$label] 締めの行 + 通知だけが見える停止を回線待ちとして拾う" "waiting_network:" \
      "$(wait_state "$P_OFF" "waiting_network:")"
    check_eq "② [$label] 通知の無い形（エラーの頭が画面の外）も拾う" "waiting_network:" \
      "$(wait_state "$P_BARE" "waiting_network:")"
  fi
  check_eq "③ 画面の外が普通の返答 / ログイン失効の停止は拾わない" "none none" \
    "$(state_of "$P_ANS") $(state_of "$P_LOGIN")"
  check_eq "回線断のあいだは自動通知を 1 通も送らない" "0" \
    "$(grep -e "-$label RECEIVED " "$LOG" | grep -c -e 自動通知 -e auto-notice || true)"

  echo "== [$label] 回線の復帰（待ち受けを開く）=="
  start_listener || exit 1
  if [ "$label" = legacy ]; then
    sleep 10
    check_eq "① [legacy] 届かない（取りこぼしの再現）" "0" "$(nudges "$n_off")"
    check_eq "② [legacy] 届かない" "0" "$(nudges "$n_bare")"
  else
    if wait_nudges "$n_off" 1 25; then pass "① [$label] 「続き」が届いた（worker）"; else fail "① [$label] 届かない（worker）"; fi
    if wait_nudges "$n_bare" 1 25; then pass "② [$label] 「続き」が届いた（master）"; else fail "② [$label] 届かない（master）"; fi
    # 届いた後は会話が進んだ画面（2 通目を送らない）
    sleep 10
    check_eq "④ [$label] 2 通目は送らない" "1 1" "$(nudges "$n_off") $(nudges "$n_bare")"
    check_eq "④ [$label] 届いた後は止まっていない" "none none" "$(state_of "$P_OFF") $(state_of "$P_BARE")"
  fi
  check_eq "③ 普通の返答 / ログイン失効へは届かない" "0 0" "$(received "$n_ans") $(received "$n_login")"

  echo "== [$label] ⑤ persist.log =="
  local plog
  plog="$(grep '\[net-resume\]' "$TAKO_DATA_DIR/persist.log" 2>/dev/null)"
  echo "$plog" | sed 's/^/    /' | tail -10
  if [ "$label" = legacy ]; then
    check_eq "⑤ [legacy] 停止の検知は出ない" "0" \
      "$(echo "$plog" | grep -c -e "pane=$P_OFF 接続エラーで停止" -e "pane=$P_BARE 接続エラーで停止")"
  else
    check_eq "⑤ [$label] 停止の検知が 1 本ずつ出る（通知 / 通知なし）" "1 1" \
      "$(echo "$plog" | grep -c "pane=$P_OFF 接続エラーで停止 kind=dns") $(echo "$plog" | grep -c "pane=$P_BARE 接続エラーで停止 kind=dns")"
    check_eq "⑤ [$label] 画面の外を読めなかった行が無い" "0" "$(echo "$plog" | grep -c "画面の外を読めない")"
  fi
  if grep -q "API Error\|ENOTFOUND\|自動通知\|auto-notice\|Login expired" "$TAKO_DATA_DIR/persist.log" 2>/dev/null; then
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
run_round direct

echo
echo "結果: PASS=$PASS FAIL=$FAIL"
[ "$FAIL" -eq 0 ]
