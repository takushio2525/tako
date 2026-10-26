#!/usr/bin/env bash
# test-net-resume-2112.sh — ネット復帰の即時再開の実経路テスト（FR-2.27.17 / #2112）
#
# 隔離した data / HOME / tmux で**実 tako-app** を立て、claude の画面の形を描く偽のエージェント
# （`scripts/lib/fake-agent-net-2112.sh`。API へは繋がない）を並べて、
#   ① 止まったセッションが無いあいだは到達確認が 1 回も走らない
#      （GUI の数え上げ `probe.count` と、到達確認の先に立てた待ち受けへの接続数の両方で見る）
#   ② 回線断（到達確認の先 = 閉じたポート）のあいだは送らず、到達確認だけが走る
#   ③ 回線の復帰（待ち受けを開く）を検知した直後に、接続エラーで止まったペイン
#      （worker・master・**退避中の solo**）へ「続き」が届く。届くまでの秒数を出す
#   ④ 接続系でない停止（使用制限・ログイン失効・人への質問）と、人の下書き・選択ダイアログの
#      ペインには届かない（見送りの理由が状態と persist.log に出る）
#   ⑤ 同じ停止に 2 回送らない。送ったあとは到達確認も止まる
#   ⑥ OFF（`tako limit-resume off --network`）では止まったペインを拾わず、回線が戻っても送らない（A/B）。
#      ON に戻すと、すでに回線が通っているのですぐ届く
#   ⑦ CLI / MCP（HTTP の MCP 口 = エージェントが使う経路）/ settings.json の 3 つが同じ値を言う
#   ⑧ persist.log に再開した本数と見送りの理由が出る（画面の文字・送った文は出ない）
#   ⑨ 設定画面のトグル（visual-test `net-resume`。実マウスで押して画像を落とす）
# を実測する。
#
# 回線断は**到達確認の先を env（`TAKO_NET_PROBE`）で 127.0.0.1 の空きポートへ差し替えて**作る。
# この PC の実ネットワークには触らない（API へも 1 バイトも出ない）。
#
# **本番の tako / 設定には一切触らない**（data / HOME は mktemp 配下、tmux は専用ソケット、
# 落とすのは自分で起こした pid だけ）。窓は常設の仮想ディスプレイ（tako-vd）へ出る（#1141）。
# **CI には登録しない**（実 GUI を立てる）。CI 側の担保は単体テスト
# （`tako_core::net_resume` / `tako_control::net_stop` / dispatch の `issue2112_*`）。
#
# 使い方: bash scripts/test-net-resume-2112.sh
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

TMP="$(mktemp -d /tmp/tako-2112-XXXXXX)"
APP_PID=""
LISTEN_PID=""
TMUX_SOCKET="tako-2112-$$"
stop_listener() {
  if [ -n "$LISTEN_PID" ]; then
    kill "$LISTEN_PID" 2>/dev/null
    wait "$LISTEN_PID" 2>/dev/null
    LISTEN_PID=""
  fi
}
cleanup() {
  stop_listener
  stop_isolated_gui "$APP_PID"
  tmux_socket_kill "$TMUX_SOCKET"
  rm -rf "$TMP"
}
trap cleanup EXIT

# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
# 設定画面の節（⑨）は visual-test 版の tako-app が要る。バイナリを渡されたらビルドしない
# ビルドは HOME を差し替える前に（後だとツールチェーンを一時 HOME へ取り直す）
isolated_gui_bins visual-test || exit 1

# --- 隔離した環境 -------------------------------------------------------------
export HOME="$TMP/home"
mkdir -p "$HOME"
printf "PROMPT='tako %%1~ %%%% '\nRPROMPT=''\n" > "$HOME/.zshrc"
export TAKO_ISOLATED=1
export TAKO_DATA_DIR="$TMP/data"
export TAKO_DISCOVERY_DIR="$TMP/disc"
export TAKO_ORCHESTRATOR_DIR="$TMP/orch"
export TAKO_SESSIONS_FILE="$TMP/sessions.yaml"
export TAKO_PANE_LOG_DIR="$TMP/panelogs"
export TAKO_WORKERS_FILE="$TMP/workers.yaml"
export TAKO_TMUX_SOCKET="$TMUX_SOCKET"
mkdir -p "$TAKO_DATA_DIR" "$TAKO_DISCOVERY_DIR" "$TAKO_ORCHESTRATOR_DIR"
for d in "$HOME" "$TAKO_DATA_DIR" "$TAKO_ORCHESTRATOR_DIR" "$TAKO_DISCOVERY_DIR"; do
  case "$d" in
    "$TMP"/*) : ;;
    *) echo "隔離されていないディレクトリ: $d"; exit 1 ;;
  esac
done
# 古い版が書いた settings.json（network_resume の項目が無い）。自動命名は off（隔離でも claude を呼ぶ）
printf '{"auto_rename": false}\n' > "$TAKO_DATA_DIR/settings.json"

# 到達確認の先 = 127.0.0.1 の空きポート（開いていない = 回線断、待ち受けを立てる = 復帰）
listening() { lsof -nP -iTCP:"$1" -sTCP:LISTEN >/dev/null 2>&1; }
PORT=""
for p in $(seq 47120 47199); do
  if ! listening "$p"; then PORT="$p"; break; fi
done
[ -n "$PORT" ] || { echo "空きポートが見つからない"; exit 1; }
CONNS="$TMP/conns.log"
: > "$CONNS"
# 待ち受け（受けた接続ごとに時刻を 1 行。macOS 同梱の perl）
start_listener() {
  /usr/bin/perl -MIO::Socket::INET -e '
    my $s = IO::Socket::INET->new(LocalAddr => "127.0.0.1", LocalPort => $ARGV[0],
      Listen => 16, ReuseAddr => 1) or die "listen: $!";
    $| = 1;
    while (my $c = $s->accept) { print time(), "\n"; close $c; }' "$PORT" >> "$CONNS" 2>"$TMP/listener.err" &
  LISTEN_PID=$!
  for _ in $(seq 1 50); do
    listening "$PORT" && return 0
    sleep 0.1
  done
  echo "待ち受けが立たない: $(cat "$TMP/listener.err")"
  return 1
}
conns() { wc -l < "$CONNS" | tr -d ' '; }

LOG="$TMP/received.log"
: > "$LOG"
received() { grep -c " $1 RECEIVED " "$LOG" 2>/dev/null || true; }
keys_to() { grep -c " $1 KEY" "$LOG" 2>/dev/null || true; }

T() { "$TAKO_BIN" "$@" 2>&1; }
net() { T limit-resume --network; }
jqv() { jq -r "$1" 2>/dev/null; }
persist_log() { cat "$TAKO_DATA_DIR/persist.log" 2>/dev/null; }

# MCP（HTTP の MCP 口 = エージェントが繋ぐのと同じ経路）で tako_limit_resume を呼ぶ
mcp_limit_resume() {
  local url tok
  url="$(jq -r .mcp_url "$TAKO_DISCOVERY_DIR/control.json")"
  tok="$(jq -r .token "$TAKO_DISCOVERY_DIR/control.json")"
  curl -s -X POST "$url" -H "Authorization: Bearer $tok" -H 'Content-Type: application/json' \
    -H 'Accept: application/json, text/event-stream' \
    -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/call\",\"params\":{\"name\":\"tako_limit_resume\",\"arguments\":$1}}" |
    jq -r '.result.content[0].text'
}

# 偽のエージェントを**自分のタブ**の右側の大きなペインで立て、role を貼る（role で
# 「エージェントのペイン」になる）。1 つのタブへ縦に積むと背が足りず、入力欄が会話に重なる
spawn_fake() {
  local mode="$1" name="$2" role="$3" base pane
  base="$(T tab new --title "$name" | jqv .pane)"
  pane="$(T split --pane "$base" --right --ratio 0.75 -- /bin/bash "$FAKE" "$mode" "$name" "$LOG" | tr -dc '0-9')"
  [ -n "$role" ] && T title --pane "$pane" --role "$role" >/dev/null
  echo "$pane"
}

# 状態の `network.stopped` にペインが載り、state / hold が期待どおりになるまで待つ。
# 届かなければ画面の末尾を stderr へ出す（何が描かれていたかを後から読めるように）
wait_state() {
  local pane="$1" want="$2" got=""
  for _ in $(seq 1 30); do
    got="$(net | jqv "[.stopped[] | select(.pane == $pane) | (.state + \":\" + (.hold // \"\"))][0] // \"none\"")"
    [ "$got" = "$want" ] && break
    sleep 1
  done
  if [ "$got" != "$want" ]; then
    echo "    --- pane $pane の画面" >&2
    T read --pane "$pane" --lines 20 | sed 's/^/    | /' >&2
  fi
  echo "$got"
}

echo "== 隔離 GUI を起こす（到達確認の先 = 127.0.0.1:${PORT}・まだ待ち受けていない = 回線断）=="
launch_isolated_gui "$TMP/app.log" TAKO_NET_PROBE="127.0.0.1:$PORT" TAKO_PERSIST=1 || exit $?
APP_PID="$ISOLATED_GUI_PID"
wait_isolated_gui "$TMP/app.log" || exit 1
TABS="$(T list | jqv '.tabs | length')"
if [ "$TABS" != "1" ]; then
  echo "繋がった先が隔離インスタンスではない（タブ ${TABS} 枚）。中止する。"; exit 1
fi
echo "  pid=$APP_PID data=$TAKO_DATA_DIR"

echo "== ⑦ 前: 既定 ON（項目の無い settings.json でも ON・読むだけでは書き換えない）=="
check_eq "⑦ CLI: 既定 ON" "true" "$(net | jqv .enabled)"
check_eq "⑦ MCP: 既定 ON" "true" "$(mcp_limit_resume '{"all": true}' | jqv .network.enabled)"
check_eq "⑦ settings.json は読むだけでは書き換えない" "missing" \
  "$(jq -r 'if has("network_resume") then .network_resume else "missing" end' "$TAKO_DATA_DIR/settings.json")"

echo "== ① 止まったセッションが無いあいだ（接続系でない停止の偽ペインだけ）=="
P_LIMIT="$(spawn_fake limit limit orchestrator-worker:t)"
P_LOGIN="$(spawn_fake login login orchestrator-worker:t)"
P_QUESTION="$(spawn_fake question question solo)"
echo "  使用制限=$P_LIMIT ログイン失効=$P_LOGIN 質問=$P_QUESTION"
start_listener || exit 1
BASE_CONNS="$(conns)"
sleep 14
check_eq "① 到達確認の回数（GUI の数え上げ）は 0" "0" "$(net | jqv .probe.count)"
check_eq "① 到達確認の先（待ち受け）への接続は 0" "0" "$(( $(conns) - BASE_CONNS ))"
check_eq "① 止まっているペインは無い" "0" "$(net | jqv '.stopped | length')"
stop_listener

echo "== ② 回線断のあいだ =="
P_WORKER="$(spawn_fake enotfound worker orchestrator-worker:t)"
P_MASTER="$(spawn_fake refused master orchestrator-master)"
P_SHELVED="$(spawn_fake enotfound shelved solo)"
P_DRAFT="$(spawn_fake draft draft orchestrator-worker:t)"
P_DIALOG="$(spawn_fake dialog dialog orchestrator-worker:t)"
sleep 2
T background --pane "$P_SHELVED" >/dev/null
echo "  worker=$P_WORKER master=$P_MASTER solo(退避)=$P_SHELVED 下書き=$P_DRAFT ダイアログ=$P_DIALOG"
check_eq "② worker は回線待ち" "waiting_network:" "$(wait_state "$P_WORKER" "waiting_network:")"
check_eq "② master は回線待ち" "waiting_network:" "$(wait_state "$P_MASTER" "waiting_network:")"
check_eq "② 退避中の solo も回線待ち" "waiting_network:" "$(wait_state "$P_SHELVED" "waiting_network:")"
check_eq "② 退避中と分かる" "true" "$(net | jqv "[.stopped[] | select(.pane == $P_SHELVED) | .shelved][0]")"
check_eq "② 下書きのペインは見送り" "held:user_draft" "$(wait_state "$P_DRAFT" "held:user_draft")"
check_eq "② ダイアログのペインは見送り" "held:dialog" "$(wait_state "$P_DIALOG" "held:dialog")"
check_eq "② 種別は接続系の slug（dns / refused）" "dns refused" \
  "$(net | jqv "[.stopped[] | select(.pane == $P_WORKER or .pane == $P_MASTER) | .kind] | join(\" \")")"
sleep 8
N1="$(net)"
echo "  到達確認: $(echo "$N1" | jq -c '.probe | {count, last_ok, last_detail, active}')"
if [ "$(echo "$N1" | jqv .probe.count)" -ge 2 ] 2>/dev/null; then
  pass "② 回線断のあいだは到達確認が走る（$(echo "$N1" | jqv .probe.count) 回）"
else
  fail "② 到達確認が走っていない（$(echo "$N1" | jq -c .probe)）"
fi
check_eq "② 到達できない" "false" "$(echo "$N1" | jqv .probe.last_ok)"
check_eq "② 回線断のあいだは 1 通も送らない" "0" "$(grep -c RECEIVED "$LOG" || true)"

echo "== ③ 回線の復帰（待ち受けを開く）=="
now_ms() { /usr/bin/perl -MTime::HiRes=time -e 'printf "%.3f", time'; }
T0="$(now_ms)"
start_listener || exit 1
GOT=""
for _ in $(seq 1 40); do
  if [ "$(received worker)" -ge 1 ] && [ "$(received master)" -ge 1 ] && [ "$(received shelved)" -ge 1 ]; then
    GOT=1; break
  fi
  sleep 0.5
done
if [ -n "$GOT" ]; then
  # 内訳: 回線の復帰（待ち受けを開いた）→ 到達確認が通った（GUI の last_round.at = 秒）→ 各ペインが受けた
  DETECTED="$(net | jqv .last_round.at)"
  LAST="$(grep -E ' (worker|master|shelved) RECEIVED ' "$LOG" | awk '{print $1}' | sort -n | tail -1)"
  FIRST_AT="$(grep -E ' (worker|master|shelved) RECEIVED ' "$LOG" | awk '{print $1}' | sort -n | head -1)"
  echo "    復帰 → 検知: $(awk -v a="$T0" -v b="$DETECTED" 'BEGIN{printf "%.1f", b-a}') 秒（到達確認の間隔ぶん。検知の時刻は秒の精度）"
  echo "    検知 → 最初の送達: $(awk -v a="$DETECTED" -v b="$FIRST_AT" 'BEGIN{printf "%.1f", b-a}') 秒 / 最後の送達: $(awk -v a="$DETECTED" -v b="$LAST" 'BEGIN{printf "%.1f", b-a}') 秒"
  for name in worker master shelved; do
    AT="$(grep " $name RECEIVED " "$LOG" | head -1 | awk '{print $1}')"
    echo "      $name: 検知から $(awk -v a="$DETECTED" -v b="$AT" 'BEGIN{printf "%.1f", b-a}') 秒"
  done
  pass "③ 復帰から $(awk -v a="$T0" -v b="$LAST" 'BEGIN{printf "%.1f", b-a}') 秒で 3 本（worker / master / 退避中の solo）に届いた"
else
  fail "③ 届かない（worker=$(received worker) master=$(received master) shelved=$(received shelved)）"
fi
FIRST="$(grep ' worker RECEIVED ' "$LOG" | head -1 | sed 's/^[0-9.]* worker RECEIVED //')"
case "$FIRST" in
  "【tako 自動通知】ネットワーク"* | "[tako auto-notice] The network"*) pass "③ 届いた文は自動通知の印で始まる" ;;
  *) fail "③ 届いた文が想定と違う: ${FIRST:0:40}" ;;
esac
check_eq "③ 再開した本数が状態に出る" "3" "$(net | jqv '.last_round.resumed | length')"

echo "== ④ 送ってはいけないペイン =="
sleep 4
for name in limit login question draft; do
  check_eq "④ ${name} には届かない" "0" "$(received "$name")"
done
check_eq "④ ダイアログへはキーも届かない" "0" "$(keys_to dialog)"
SKIPPED="$(net | jqv '[.last_round.skipped[] | (.pane|tostring) + ":" + .reason] | join(",")')"
check_eq "④ 見送りは下書きとダイアログ（理由つき）" "${P_DRAFT}:user_draft,${P_DIALOG}:dialog" "$SKIPPED"

echo "== ⑤ 同じ停止に 2 回送らない・送った後は到達確認も止まる =="
C_PROBES="$(net | jqv .probe.count)"
C_CONNS="$(conns)"
sleep 12
check_eq "⑤ worker へは 1 回だけ" "1" "$(received worker)"
check_eq "⑤ master へは 1 回だけ" "1" "$(received master)"
check_eq "⑤ 退避中の solo へは 1 回だけ" "1" "$(received shelved)"
check_eq "⑤ 送った後は到達確認が増えない（GUI の数え上げ）" "$C_PROBES" "$(net | jqv .probe.count)"
check_eq "⑤ 送った後は待ち受けへの接続が増えない" "$C_CONNS" "$(conns)"

echo "== ⑥ OFF（A/B）=="
OFF="$(T limit-resume off --network)"
check_eq "⑥ CLI で OFF" "false" "$(echo "$OFF" | jqv .enabled)"
check_eq "⑥ settings.json に false" "false" "$(jq -r .network_resume "$TAKO_DATA_DIR/settings.json")"
check_eq "⑥ MCP からも OFF と読める" "false" "$(mcp_limit_resume '{"all": true}' | jqv .network.enabled)"
stop_listener
P_OFF="$(spawn_fake enotfound off orchestrator-worker:t)"
OFF_PROBES="$(net | jqv .probe.count)"
sleep 8
check_eq "⑥ OFF のあいだは止まったペインを拾わない" "0" "$(net | jqv '.stopped | length')"
start_listener || exit 1
sleep 10
check_eq "⑥ OFF なら回線が戻っても送らない" "0" "$(received off)"
check_eq "⑥ OFF なら到達確認も走らない" "$OFF_PROBES" "$(net | jqv .probe.count)"
ON="$(mcp_limit_resume '{"network": true}')"
check_eq "⑥ MCP で ON に戻す" "true" "$(echo "$ON" | jqv .network.enabled)"
check_eq "⑦ settings.json も true" "true" "$(jq -r .network_resume "$TAKO_DATA_DIR/settings.json")"
GOT=""
for _ in $(seq 1 30); do
  [ "$(received off)" -ge 1 ] && { GOT=1; break; }
  sleep 0.5
done
if [ -n "$GOT" ]; then pass "⑥ ON に戻すと、通っている回線ですぐ届く"; else fail "⑥ ON に戻しても届かない"; fi

echo "== ⑧ persist.log =="
LOGTXT="$(persist_log | grep '\[net-resume\]')"
echo "$LOGTXT" | sed 's/^/    /' | tail -14
if grep -q "回線の復帰を検知 .*再開 3 本 .*見送り 2 本 \[${P_DRAFT}:user_draft, ${P_DIALOG}:dialog\]" <<<"$LOGTXT"; then
  pass "⑧ 再開した本数と見送りの理由が出る"
else
  fail "⑧ 再開の 1 行が見つからない"
fi
if grep -q "pane=${P_DIALOG} 見送り reason=dialog" <<<"$LOGTXT" &&
  grep -q "pane=${P_DRAFT} 見送り reason=user_draft" <<<"$LOGTXT"; then
  pass "⑧ ペインごとの見送りの理由が出る"
else
  fail "⑧ 見送りの理由の行が無い"
fi
if log_text="$(persist_log)" && grep -q "API Error\|ENOTFOUND\|自動通知\|auto-notice" <<<"$log_text"; then
  fail "⑧ persist.log に画面の文字か送った文が出ている"
else
  pass "⑧ persist.log に画面の文字・送った文が出ない（種別の slug だけ）"
fi

echo "== ⑨ 設定画面のトグル（visual-test net-resume）=="
stop_isolated_gui "$APP_PID"
APP_PID=""
sleep 1
DUMP="$TMP/dump"
launch_isolated_gui "$TMP/visual.log" TAKO_VISUAL_TEST=1 TAKO_VISUAL_ONLY=net-resume \
  TAKO_VISUAL_DUMP_DIR="$DUMP" || exit $?
VPID="$ISOLATED_GUI_PID"
for _ in $(seq 1 120); do
  kill -0 "$VPID" 2>/dev/null || break
  sleep 1
done
stop_isolated_gui "$VPID"
grep -E "TAKO_VISUAL_PIXEL: net-resume|TAKO_APP_SELF_TEST_FAILED|TAKO_VISUAL_TEST_OK" "$TMP/visual.log" | sed 's/^/    /'
if grep -q "TAKO_VISUAL_TEST_OK" "$TMP/visual.log"; then
  pass "⑨ 設定画面のトグルで実行状態・settings.json・CLI / MCP の答えが揃って切り替わる"
else
  fail "⑨ visual-test net-resume が通らない"
fi
if [ -n "${TAKO_2112_KEEP_DUMP:-}" ] && [ -d "$DUMP" ]; then
  mkdir -p "$TAKO_2112_KEEP_DUMP"
  cp "$DUMP"/*.png "$TAKO_2112_KEEP_DUMP"/ 2>/dev/null
  echo "  画像: $TAKO_2112_KEEP_DUMP"
fi

echo
echo "結果: PASS=$PASS FAIL=$FAIL"
[ "$FAIL" -eq 0 ]
