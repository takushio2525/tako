#!/usr/bin/env bash
# test-mod-dispatch-offload-2663.sh — tako mod の IPC（報告・status・on / off・帯の切り替え）が GUI のメインスレッドで
# 画面を採らず ui.json を読み書きしないかの実経路テスト（#2663）
#
# 隔離 GUI（`TAKO_ISOLATED=1`）に master 1 枚 + worker 2 本を立て、各ペインに 300 行の色つきの画面を出す。
# master と worker 1 本には 0.1 秒ごとに 1 行を出させ（PTY の読み手がロックを取る機会。負荷は作らない）、
# 読み手がロックを取ったまま止まる遅延を注入する（`TAKO_2663_INJECT_HOLD_MS`。検証プロセスでだけ効く）。
# 出力するペインは見えていないタブへ退ける（描画は端末のロックを取るので、注入が描画の待ちに混ざらない）。
# その間に次を流し、別の IPC が待たされないか・perf.log に専有が出ないかを測る:
#   ① master の報告 × N（worker 2 本の画面 + 自分の画面の末尾を採る）と worker の報告 × N
#   ② status（`tako mod --json` = 全ペインの画面を採る）× 5
#   ③ 帯の切り替え（band off / on）と on / off（同期の status つき）
#   ④ 帯のトグルの取り込みつきの報告（ui.json へ書く）・断り（居ないペイン・本文なし）
#   測るもの: 各要求の往復・その間の `tako lang`（端末のロックを取らない IPC = UI スレッドの空き）の往復・
#   perf.log の `dispatch:Mod` / `offload_prepare:Mod` の専有。`tako list` は自身が UI スレッドで各ペインの
#   端末のロックを取る（スクロールの位置と alt screen の印）ので注入の下では修正前後とも自分で待ち、その間は
#   UI スレッドも止まる = lang と同時に回すと lang の待ちに混ざる。list は別の回（master の報告の間）で測る
#   ⑤ A/B（TAKO_2663_LEGACY=1）: 修正前と同じく UI スレッドで採る（lang が待たされ、専有が出る）。
#      応答（帯の `view`・status の行）・hub の状態（`tako mod --json`）・ui.json・断りは ①〜④ と同じ
#      （実行ごとに変わる値 = 経過時間・置き場・壁時計の時刻は落として比べる）
#
# **本番の GUI / tmux / data dir には一切触らない**（置き場は mktemp 配下。落とすのは自分で起こした GUI だけ）。
# 窓は仮想ディスプレイ（tako-vd）へ出す。
#
# 使い方: bash scripts/test-mod-dispatch-offload-2663.sh
#   性能を見るので既定は release（TAKO_ISO_PROFILE=release）。証拠は TAKO_2663_EVIDENCE_DIR=<dir> へ写す
set -uo pipefail

# 本番 tako のペインから回しても CLI が本番の GUI へ繋がないよう、本番が配る env を落とす（#1970）
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_MCP_URL TMUX TAKO_2663_LEGACY TAKO_OFFLOAD
unset TAKO_2663_INJECT_HOLD_MS TAKO_ORCHESTRATOR_ROLE

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PASS=0
FAIL=0
pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
fail() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }
check_eq() {
  if [ "$2" = "$3" ]; then pass "$1"; else fail "$1（期待 '${2}' / 実際 '${3}'）"; fi
}
check_lt() {
  if [ "$3" -lt "$2" ]; then pass "$1（${3}）"; else fail "$1（${2} 未満のはずが ${3}）"; fi
}
check_ge() {
  if [ "$3" -ge "$2" ]; then pass "$1（${3}）"; else fail "$1（${2} 以上のはずが ${3}）"; fi
}

for bin in jq perl zsh; do
  command -v "$bin" >/dev/null 2>&1 || { echo "未実測: ${bin} が無い"; exit 4; }
done

HOLD_MS="${TAKO_2663_HOLD_MS:-50}"
REPS="${TAKO_2663_REPS:-10}"
MASTER_ROLE="orchestrator-master:t2663"
export TAKO_ISO_PROFILE="${TAKO_ISO_PROFILE:-release}"
TMP="$(mktemp -d /tmp/t2663-XXXXXX)"
APP_PID=""

. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
isolated_gui_bins || exit 1

cleanup() {
  if [ -n "$APP_PID" ]; then stop_isolated_gui "$APP_PID"; fi
  if [ -n "${KEEP:-}" ]; then echo "置き場を残した: $TMP"; else rm -rf "${TMP:?}"; fi
}
trap cleanup EXIT

mkdir -p "$TMP/ctl" "$TMP/zdot"
command cp -f "$TAKO_BIN" "$TMP/ctl/tako"
CLI="$TMP/ctl/tako"
printf '%s\n' 'PS1="[test@host:%1~]\$ "' > "$TMP/zdot/.zshrc"

# 各ペインに出す画面（claude の TUI に寄せた色つきの 300 行。中身は固定 = A/B で同じ画面）
SCREEN="$TMP/screen.txt"
perl -e '
  my @w = qw(the quick brown fox jumps over lazy dog cargo build test clippy tako mod report band view worker);
  srand(7);
  for my $i (0..299) {
    my $n = 4 + int(rand(36));
    my $body = join(" ", map { $w[int(rand(@w))] } 1..$n);
    my $c = (31, 32, 33, 34, 35, 36, 90)[int(rand(7))];
    printf "\e[%dm*\e[0m %s \e[2m(%d)\e[0m\n", $c, $body, $i;
  }' > "$SCREEN"

# 要求を回して往復（ms）を 1 行ずつ書く。`stop` が在れば止まる（`-` = 回数だけ回す）
TIMED="$TMP/timed.pl"
cat > "$TIMED" <<'PL_EOF'
use strict; use warnings; use Time::HiRes qw(time sleep);
my ($out, $reps, $stop, $gap, @cmd) = @ARGV;
open my $f, '>', $out or die "$out: $!";
my $n = 0;
while (1) {
  last if $stop ne '-' && -e $stop;
  last if $reps > 0 && $n >= $reps;
  my $t0 = time;
  system(@cmd);
  printf $f "%d\n", (time - $t0) * 1000;
  $n++;
  last if $n >= 2000;
  sleep($gap);
}
close $f;
PL_EOF

perf_hits_over() {
  perl -ne 'if (/メインスレッド専有: (\S+) が (\d+)ms/) { my ($tag, $ms) = ($1, $2); $n++ if $tag =~ /^(?:'"$1"')$/ && $ms >= '"$2"' } END { print $n + 0, "\n" }' "$3" 2>/dev/null
}
max_of() {
  if [ -s "$1" ]; then sort -n "$1" | tail -1 | tr -dc '0-9'; else echo 999999; fi
}
median_of() {
  if [ -s "$1" ]; then sort -n "$1" | awk '{a[NR]=$1} END {print a[int((NR+1)/2)]}'; else echo 999999; fi
}
# 揺れる値（経過時間・置き場・壁時計の時刻・回ごとの置き場の名前と一時の dir の pid）を揃えて比べる形へ
normalize() {
  jq -S 'walk(
    if type == "object" then del(.age_ms, .last_age_ms, .path, .at)
    elif type == "string" then gsub("/(new|legacy)/"; "/R/") | gsub("tako-agent-config-[0-9]+"; "tako-agent-config-N")
    elif type == "number" and . > 1000000000000 then "epoch_ms"
    else . end)' "$1" 2>/dev/null
}
report_body() {
  # $1 = turn・$2 = 帯の toggled_at（空なら帯を送らない）・$3 = ended
  local band=""
  [ -n "${2:-}" ] && band=",\"band\":{\"hidden\":true,\"shown\":false,\"segments\":[\"ctx\"],\"toggled_at\":$2}"
  printf '{"schema":1,"mod_version":"t","claude_version":"2.1.300","at":1,"model":"claude-opus","context":{"tokens":120000,"window":200000,"percent":60},"rate_limits":[{"kind":"five_hour","percent_used":42.5,"resets_at":"2026-11-02T20:00:00Z","observed_at":1}],"turn":"%s","renders":{"band":true,"usage_bar":"band","buttons":2,"band_hook":true,"hint_hook":true}%s,"ended":%s}' \
    "$1" "$band" "${3:-false}"
}
# 1 つの要求を流しながら、その間の別の IPC（$2 = lang / list）の往復を測る（背景の 1 本は stop で止まる）
measure() {
  local name="$1" probe="$2" reps="$3" gap="$4"
  shift 4
  rm -f "${TMP:?}/stop"
  perl "$TIMED" "$TMP/$name-$probe.txt" 0 "$TMP/stop" 0.03 sh -c "\"$CLI\" $probe >/dev/null 2>&1" &
  local probe_pid=$!
  sleep 0.3
  perl "$TIMED" "$TMP/$name-req.txt" "$reps" - "$gap" "$@"
  touch "$TMP/stop"
  wait "$probe_pid"
  echo "    （${name}: 要求の往復 中央 $(median_of "$TMP/$name-req.txt") / 最大 $(max_of "$TMP/$name-req.txt")ms・" \
    "その間の ${probe} 中央 $(median_of "$TMP/$name-$probe.txt") / 最大 $(max_of "$TMP/$name-$probe.txt")ms）"
}

run_round() {
  local r="$1" p
  shift
  local D="$TMP/$r"
  mkdir -p "$D/data" "$D/disc"
  export TAKO_ISOLATED=1 TAKO_DATA_DIR="$D/data" TAKO_DISCOVERY_DIR="$D/disc" TAKO_LANG=ja
  for p in "$TAKO_DATA_DIR" "$TAKO_DISCOVERY_DIR"; do
    case "$p" in "$TMP"/*) : ;; *) echo "隔離されていない: $p"; exit 1 ;; esac
  done
  local PERF="$D/data/perf.log"

  launch_isolated_gui "$D/app.log" "SHELL=/bin/zsh" "ZDOTDIR=$TMP/zdot" "TAKO_2663_INJECT_HOLD_MS=$HOLD_MS" "$@" || exit $?
  APP_PID="$ISOLATED_GUI_PID"
  # 立たない（tako-vd がディスプレイスリープで一覧から落ちた等）回を「専有 0 件」と数えない = 未実測で止める
  wait_isolated_gui "$D/app.log" || { echo "未実測: [$r] 隔離 GUI が起動しない（scripts/lib/virtual-display.sh status）"; exit 4; }
  "$CLI" autorename off >/dev/null 2>&1
  sleep 2
  MASTER="$("$CLI" list | jq -r '.tabs[0].panes[0].id')"
  "$CLI" title --pane "$MASTER" --role "$MASTER_ROLE" master >/dev/null 2>&1
  W1="$("$CLI" split --pane "$MASTER" --right | tr -dc '0-9')"
  W2="$("$CLI" split --pane "$W1" --down | tr -dc '0-9')"
  "$CLI" title --pane "$W1" --role "orchestrator-worker:t2663:w1" w1 >/dev/null 2>&1
  "$CLI" title --pane "$W2" --role "orchestrator-worker:t2663:w2" w2 >/dev/null 2>&1
  sleep 1
  for p in "$MASTER" "$W1" "$W2"; do
    "$CLI" send --pane "$p" "clear; cat '$SCREEN'" >/dev/null 2>&1
  done
  sleep 2
  for p in "$MASTER" "$W1"; do
    "$CLI" send --pane "$p" 'while :; do date +%s; sleep 0.1; done' >/dev/null 2>&1
  done
  # 出力するペインのタブを見えなくする（描画の端末のロックを注入に混ぜない）
  "$CLI" tab new --focus >/dev/null 2>&1
  sleep 2
  : > "$PERF"

  echo
  echo "== [$r] ① 報告 × ${REPS}（master = worker 2 本の画面と自分の末尾を採る / worker）=="
  report_body busy > "$D/body-busy.json"
  report_body idle > "$D/body-idle.json"
  measure "$r-master" lang "$REPS" 0.1 sh -c "TAKO_PANE_ID=$MASTER \"$CLI\" mod report < '$D/body-busy.json' > '$D/master-report.json' 2>&1"
  measure "$r-worker" lang "$REPS" 0.1 sh -c "TAKO_PANE_ID=$W1 \"$CLI\" mod report < '$D/body-idle.json' > '$D/worker-report.json' 2>&1"
  measure "$r-masterlist" list "$REPS" 0.1 sh -c "TAKO_PANE_ID=$MASTER \"$CLI\" mod report < '$D/body-busy.json' > /dev/null 2>&1"
  check_eq "[$r] ① master の報告は受け付けられる" stored "$(jq -r .accepted "$D/master-report.json" 2>/dev/null)"
  check_eq "[$r] ① master の応答の帯に worker 2 本" 2 "$(jq -r .tako.view.worker_count "$D/master-report.json" 2>/dev/null)"

  echo
  echo "== [$r] ② status（全ペインの画面を採る）× 5 =="
  measure "$r-status" lang 5 0.2 sh -c "\"$CLI\" mod --json > '$D/status.json' 2>&1"

  echo
  echo "== [$r] ④ 帯のトグルの取り込み（ui.json へ書く）=="
  report_body busy 7 > "$D/body-band.json"
  TAKO_PANE_ID=$MASTER "$CLI" mod report < "$D/body-band.json" > "$D/band-report.json" 2>&1
  command cp -f "$D/data/claude-mod/ui.json" "$D/ui-imported.json" 2>/dev/null
  check_eq "[$r] ④ 帯のトグルを ui.json へ取り込んだ" 7 "$(jq -r .band.toggled_at "$D/ui-imported.json" 2>/dev/null)"

  echo
  echo "== [$r] ③ 帯の切り替えと on / off =="
  measure "$r-band" lang 2 0.2 sh -c "\"$CLI\" mod band off --json > '$D/band-off.json' 2>&1; \"$CLI\" mod band on --json > '$D/band-on.json' 2>&1"
  measure "$r-onoff" lang 2 0.2 sh -c "\"$CLI\" mod off --json > '$D/off.json' 2>&1; \"$CLI\" mod on --json > '$D/on.json' 2>&1"

  echo
  echo "== [$r] ended・断り =="
  report_body idle "" true > "$D/body-ended.json"
  TAKO_PANE_ID=$W2 "$CLI" mod report < "$D/body-ended.json" > "$D/ended-report.json" 2>&1
  TAKO_PANE_ID=999999 "$CLI" mod report < "$D/body-idle.json" > "$D/missing-pane.txt" 2>&1
  echo "rc=$?" >> "$D/missing-pane.txt"
  TAKO_PANE_ID=$MASTER "$CLI" mod report < /dev/null > "$D/no-body.txt" 2>&1
  echo "rc=$?" >> "$D/no-body.txt"
  "$CLI" mod --json > "$D/status-final.json" 2>&1
  command cp -f "$D/data/claude-mod/ui.json" "$D/ui.json" 2>/dev/null
  check_eq "[$r] ended の報告" ended "$(jq -r .accepted "$D/ended-report.json" 2>/dev/null)"
  sleep 1
  command cp -f "$PERF" "$D/perf-copy.log"
  echo "    perf.log の Mod の専有: $(grep -cE "専有: (dispatch|offload_prepare):Mod" "$PERF" 2>/dev/null)" \
    "件（$(grep -E "専有: (dispatch|offload_prepare):Mod" "$PERF" 2>/dev/null | sed -E 's/.*専有: //' | tr '\n' ' ')）"

  stop_isolated_gui "$APP_PID"
  APP_PID=""
}

run_round new
run_round legacy TAKO_2663_LEGACY=1

N="$TMP/new"
L="$TMP/legacy"
for f in "$N/perf-copy.log" "$L/perf-copy.log"; do
  [ -f "$f" ] || { echo "未実測: 回の証拠が揃っていない（$f が無い）"; exit 4; }
done
TAGS='dispatch:Mod|offload_prepare:Mod'
echo
echo "== 判定 =="
check_eq "修正後: perf.log の dispatch:Mod / offload_prepare:Mod の専有は 0 件" 0 "$(perf_hits_over "$TAGS" 32 "$N/perf-copy.log")"
check_ge "修正前（A/B）: 同じ注入で専有が出る（注入が効いている）" 1 "$(perf_hits_over "$TAGS" 32 "$L/perf-copy.log")"
# UI スレッドが塞がったかの主の判定は上の perf.log（GUI 自身の計測）。lang の往復は CLI の起動の揺れを含む
# （混んだ機では 1 回だけ 50ms 台が出る。そのとき perf.log には何も出ない）ので、修正前との比べで見る。
# lang は UI スレッドが塞がっている途中から乗るので、待つのは残りの時間だけ
NEW_LANG_MAX=$(cat "$TMP"/new-*-lang.txt | sort -n | tail -1 | tr -dc '0-9')
LEGACY_LANG_MAX=$(cat "$TMP"/legacy-*-lang.txt | sort -n | tail -1 | tr -dc '0-9')
check_lt "修正後: 要求の間の lang（ロックを取らない IPC）の最長は修正前の最長（${LEGACY_LANG_MAX:-0}ms）より短い" \
  "${LEGACY_LANG_MAX:-0}" "${NEW_LANG_MAX:-999999}"
check_ge "修正前（A/B）: 報告の間の lang の最長は注入（${HOLD_MS}ms）の半分以上待たされる" "$((HOLD_MS / 2))" \
  "$(cat "$TMP/legacy-master-lang.txt" "$TMP/legacy-worker-lang.txt" | sort -n | tail -1 | tr -dc '0-9')"
echo "  [情報] 修正後の perf.log の専有（tako list 自身のロック待ち = dispatch:List / ipc_turn を除く）:" \
  "$(grep "専有" "$N/perf-copy.log" 2>/dev/null | grep -vc "dispatch:List\|ipc_turn") 件"
# list は自身が UI スレッドで各ペインの端末のロックを取る（修正の範囲外 = 注入の下では前後とも自分で待つ）。数字だけ残す
echo "  [情報] master の報告の間の tako list: 修正後 中央 $(median_of "$TMP/new-masterlist-list.txt") / 最大 $(max_of "$TMP/new-masterlist-list.txt")ms・" \
  "修正前 中央 $(median_of "$TMP/legacy-masterlist-list.txt") / 最大 $(max_of "$TMP/legacy-masterlist-list.txt")ms"

echo
echo "== 要求ごとの往復とその間の lang（ms・中央 / 最大。修正後 → 修正前）=="
for k in master worker status band onoff; do
  echo "  ${k}: 要求 $(median_of "$TMP/new-$k-req.txt") / $(max_of "$TMP/new-$k-req.txt") → $(median_of "$TMP/legacy-$k-req.txt") / $(max_of "$TMP/legacy-$k-req.txt")・" \
    "lang $(median_of "$TMP/new-$k-lang.txt") / $(max_of "$TMP/new-$k-lang.txt") → $(median_of "$TMP/legacy-$k-lang.txt") / $(max_of "$TMP/legacy-$k-lang.txt")"
done

echo
echo "== ⑤ 応答・hub・ui.json・断りの A/B 一致 =="
for f in master-report.json worker-report.json band-report.json ui-imported.json ended-report.json status-final.json band-off.json band-on.json off.json on.json ui.json; do
  check_eq "⑤ ${f} が A/B と一致" "$(normalize "$L/$f" | shasum | cut -c1-12)" "$(normalize "$N/$f" | shasum | cut -c1-12)"
done
for f in missing-pane.txt no-body.txt; do
  check_eq "⑤ ${f} が A/B と一致" "$(shasum < "$L/$f" | cut -c1-12)" "$(shasum < "$N/$f" | cut -c1-12)"
done
check_eq "⑤ 居ないペインの報告は断る" "rc=1" "$(tail -1 "$N/missing-pane.txt")"
check_eq "⑤ 本文の無い報告は断る" "rc=1" "$(tail -1 "$N/no-body.txt")"

if [ -n "${TAKO_2663_EVIDENCE_DIR:-}" ]; then
  mkdir -p "$TAKO_2663_EVIDENCE_DIR"
  for r in new legacy; do
    mkdir -p "$TAKO_2663_EVIDENCE_DIR/$r"
    command cp -f "$TMP/$r"/*.json "$TMP/$r"/*.txt "$TMP/$r"/perf-copy.log "$TAKO_2663_EVIDENCE_DIR/$r/" 2>/dev/null
  done
  command cp -f "$TMP"/*-lang.txt "$TMP"/*-list.txt "$TMP"/*-req.txt "$TAKO_2663_EVIDENCE_DIR/"
fi

echo
echo "PASS ${PASS} / FAIL ${FAIL}"
[ "$FAIL" -eq 0 ]
