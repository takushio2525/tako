#!/usr/bin/env bash
# test-handoff-close-2330.sh — 引き継ぎの相互 close と送達の偽陰性（#2330）の実経路テスト
#
# 実 GUI（隔離 tako-app・仮想ディスプレイ tako-vd）+ 実 dispatch + 実物の Claude Code（一時の設定 dir と
# 偽の Messages API）で測る。番号は依頼の受け入れ条件:
#   handoff 1/2/3. 幅 約 48 桁の後任ペインへ実際に `tako orchestrator handoff` を撃ち、
#                  ① 送達の顛末（delivery）と後任の transcript（引き継ぎ文が届いたか）を突き合わせる
#                  ② 後任が受け取った後の「前任 → 後任」の close（force）を断る（CLI）
#                  ③ 「後任 → 前任」の close は通る（引き継ぎの完了）→ 閉じられた前任から残った後任を
#                     閉じる close は「タブのそのプロファイルの最後の master」なので force でも断る（MCP）
#   close   3.     役だけ master にしたペイン 3 枚で、同じタブに 2 人目が居る close は通る / 最後の 1 人を
#                  閉じる close は CLI・MCP とも force でも断り理由を返す / 自分で自分を閉じるのは通す /
#                  role を名乗らない呼び出し（人の CLI）は従来どおり
#   crosstab 3.    `--tab` で後任を別のタブに立てた実際の引き継ぎ: 帳面に無い別タブの master が前任を閉じる
#                  close は「最後の master」で断り、後任が前任を閉じる close は通す（前任を幽霊にしない）
#   miss    2.     注入 `TAKO_2330_INJECT_DROP_PASTE=1`（貼り付けを捨てる = 本当に届かない）では
#                  transcript に何も入らず、送達は従来どおり gave_up（受領の証拠を作らない）
#   legacy         同じバイナリの `TAKO_2330_LEGACY=1` では ① gave_up / ② ③ の close が通る（修正前）
#
# **本物の設定 dir（~/.claude*）・本番の tako には触らない**: claude の設定 dir と HOME は一時 dir
# （認証は写さず、偽 API とダミーの API キー）、data dir・orchestrator dir・tmux は一時 dir と専用ソケット。
#
# 使い方: bash scripts/test-handoff-close-2330.sh   （ONLY=deliver|handoff|close|crosstab|miss|legacy[-deliver|-handoff] で 1 段だけ）
#   修正前のバイナリで測るなら TAKO_BIN=<tako> APP_BIN=<tako-app> を並べる（その木で cargo build 済みのもの）
# **CI には登録しない**（実物の claude と仮想ディスプレイが要る）。CI 側の担保は Rust の単体
# （tako-core::handoff_guard / prompt_delivery・dispatch の close）と番犬 issue2330_handoff_close_watchdog。
set -uo pipefail

# 本番 GUI を指す env を最初に落とす（#1449 / #1450 / #1970）
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_ORCHESTRATOR_ROLE TAKO_MCP_URL \
  CLAUDE_CODE_PLUGIN_DIRS TAKO_CLI TAKO_ORCHESTRATOR_DIR TAKO_DISCOVERY_DIR TAKO_2330_LEGACY \
  TAKO_2330_INJECT_DROP_PASTE
unset CLAUDE_CODE_CHILD_SESSION CLAUDE_CODE_SESSION_ID CLAUDECODE CLAUDE_CODE_ENTRYPOINT
unset CLAUDE_CONFIG_DIR ANTHROPIC_API_KEY ANTHROPIC_AUTH_TOKEN ANTHROPIC_BASE_URL
export LANG=en_US.UTF-8 LC_ALL=en_US.UTF-8

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ONLY="${ONLY:-}"
PASS=0
FAIL=0
pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
fail() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }
check_eq() { if [ "$2" = "$3" ]; then pass "$1"; else fail "$1（期待 '${2}' / 実際 '${3}'）"; fi; }
# ONLY=legacy は legacy-deliver / legacy-handoff の両方（前半の語で選ぶ）
want() { [ -z "$ONLY" ] || [ "$ONLY" = "$1" ] || [ "$ONLY" = "${1%%-*}" ]; }

command -v jq >/dev/null 2>&1 || { echo "未実測: jq が無い"; exit 3; }
command -v node >/dev/null 2>&1 || { echo "未実測: node が無い（偽 API に使う）"; exit 3; }
CLAUDE_BIN="$(command -v claude 2>/dev/null || true)"
[ -n "$CLAUDE_BIN" ] || { echo "未実測: claude が無い"; exit 3; }
CLAUDE_DIR="$(dirname "$CLAUDE_BIN")"

# --- ビルド（HOME を差し替える前に = ツールチェーンを一時 HOME へ取り直さない）-----------------------
# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
isolated_gui_bins || exit 1

# `/tmp` 直下に短く取る（tmux のソケットが sun_path の上限に当たらない。#1441）
TMP="$(mktemp -d /tmp/tk2330-XXXXXX)"
TMP="$(cd "$TMP" && pwd -P)"
APP_PID=""
FAKE_PID=""
TMUX_SOCKET="tk2330-$$"

cleanup() {
  [ -n "$APP_PID" ] && stop_isolated_gui "$APP_PID"
  tmux -L "$TMUX_SOCKET" kill-server >/dev/null 2>&1 || true
  # ソケット名を明示した起動は自分で畳む（#1192。サーバーを止めてもファイルが残る）
  rm -f "${TMUX_TMPDIR:-/tmp}/tmux-$(id -u)/${TMUX_SOCKET:?}"
  if [ -n "$FAKE_PID" ]; then
    kill "$FAKE_PID" 2>/dev/null
    wait "$FAKE_PID" 2>/dev/null
  fi
  if [ -n "${KEEP:-}" ]; then echo "（KEEP: $TMP を残した）"; else rm -rf "${TMP:?}"; fi
}
trap cleanup EXIT
trap 'exit 130' INT TERM

# --- 隔離した環境 ---------------------------------------------------------------------------
export TAKO_ISOLATED=1
export TAKO_DATA_DIR="$TMP/data"
export TAKO_DISCOVERY_DIR="$TMP/disc"
export TAKO_ORCHESTRATOR_DIR="$TMP/orch"
export TAKO_SESSIONS_FILE="$TMP/sessions.yaml"
export TAKO_PANE_LOG_DIR="$TMP/panelogs"
export TAKO_WORKERS_FILE="$TMP/workers.yaml"
export TAKO_TMUX_SOCKET="$TMUX_SOCKET"
GHOME="$TMP/home"
WORK="$TMP/work"
CCD="$TMP/cfg"
mkdir -p "$TAKO_DATA_DIR" "$TAKO_DISCOVERY_DIR" "$TAKO_ORCHESTRATOR_DIR/handoff" "$GHOME" "$WORK" "$CCD"
for d in "$TAKO_DATA_DIR" "$TAKO_ORCHESTRATOR_DIR" "$TAKO_DISCOVERY_DIR" "$GHOME" "$CCD"; do
  case "$d" in "$TMP"/*) : ;; *) echo "隔離されていない: $d"; exit 1 ;; esac
done
tk() { "$TAKO_BIN" "$@"; }
jget() { jq -r "$1" 2>/dev/null; }

# 偽の Messages API（どの発話にも「ok」と返す）と、それを承認済みにした一時の設定 dir
API_KEY="sk-ant-api03-fake2330fake2330fake2330fake2330fake2330fake2330AAAA"
FAKE_LOG="$TMP/fake.log"
: > "$FAKE_LOG"
FAKE_LOG="$FAKE_LOG" node "$REPO_ROOT/scripts/lib/fake-messages-api.mjs" 0 > "$TMP/fake.out" 2>&1 &
FAKE_PID=$!
API_PORT=""
for _ in $(seq 1 50); do
  API_PORT="$(sed -n 's/^FAKE_API_PORT=//p' "$TMP/fake.out" 2>/dev/null)"
  [ -n "$API_PORT" ] && break
  sleep 0.1
done
[ -n "$API_PORT" ] || { echo "偽 API が立たない"; exit 1; }
LAST20="$(printf '%s' "$API_KEY" | tail -c 20)"
# 後任はプロファイルの cwd が無いので前任のフォルダで立つ（#1055）。どちらも信頼済みにしておく
jq -n --arg key "$LAST20" --arg work "$WORK" --arg home "$GHOME" '{
  hasCompletedOnboarding: true, theme: "dark", bypassPermissionsModeAccepted: true,
  customApiKeyResponses: { approved: [$key], rejected: [] },
  projects: {
    ($work): { hasTrustDialogAccepted: true, hasCompletedProjectOnboarding: true },
    ($home): { hasTrustDialogAccepted: true, hasCompletedProjectOnboarding: true }
  }
}' > "$CCD/.claude.json"

# 前任が書いた体の引き継ぎファイル（プロファイル p1 の運用メモ。中身は長めの日本語 = 狭いペインで折り返す）
{
  echo "## 知識（マシン非依存）"
  for i in 1 2 3 4 5 6; do
    echo "- 決定事項その${i}: 送達の顛末と受け手の会話記録を突き合わせてから次の手を決める。閉じ直す前に必ず相手の画面を読む。"
  done
  echo
  echo "## 実行状態（このマシン限定）"
  echo "- 稼働中の worker は無い（隔離した検証環境）"
} > "$TAKO_ORCHESTRATOR_DIR/handoff/p1.md"

start_app() {
  local name="$1"
  shift
  ISOLATED_GUI_BOUNDS="${ISOLATED_GUI_BOUNDS:-0,0,${WIN_W:-900},900}"
  launch_isolated_gui "$TMP/app-${name}.log" TAKO_PERSIST=0 HOME="$GHOME" \
    CLAUDE_CONFIG_DIR="$CCD" ANTHROPIC_BASE_URL="http://127.0.0.1:$API_PORT" ANTHROPIC_API_KEY="$API_KEY" \
    DISABLE_AUTOUPDATER=1 CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC=1 \
    PATH="$CLAUDE_DIR:$PATH" ${@+"$@"} || exit $?
  APP_PID="$ISOLATED_GUI_PID"
  wait_isolated_gui "$TMP/app-${name}.log" || exit 1
  # 繋がった先が隔離インスタンスか（タブ 1 枚）。本番に繋がっていたら中止する
  if [ "$(tk list | jq '.tabs | length')" != "1" ]; then
    echo "繋がった先が隔離インスタンスではない。中止する"
    exit 1
  fi
}
stop_app() {
  stop_isolated_gui "$APP_PID"
  APP_PID=""
}
root_pane() { tk list 2>/dev/null | jq -r '.tabs[0].panes[0].id'; }
pane_ids() { tk list 2>/dev/null | jq -r '.tabs[].panes[].id' | sort; }
pane_cols() { tk list 2>/dev/null | jq -r --argjson p "$1" '.tabs[].panes[] | select(.id == $p) | .cols'; }
master_count() { tk list 2>/dev/null | jq -r '[.tabs[0].panes[] | select((.role // "") | startswith("orchestrator-master"))] | length'; }
pane_alive() { [ -n "$(tk list 2>/dev/null | jq -r --argjson p "$1" '.tabs[].panes[] | select(.id == $p) | .id')" ]; }
split_new() {
  local before after
  before="$(pane_ids)"
  tk split --pane "$1" "--${2:-right}" >/dev/null 2>&1
  after="$(pane_ids)"
  comm -13 <(printf '%s\n' "$before") <(printf '%s\n' "$after") | head -1
}
wait_for() {
  local limit="$1" i
  shift
  for i in $(seq 1 "$limit"); do
    if "$@"; then return 0; fi
    sleep 1
  done
  return 1
}

# MCP（GUI 内蔵の HTTP MCP）。呼び出し元のペインと role はヘッダで名乗る（stdio の TAKO_PANE_ID /
# TAKO_ORCHESTRATOR_ROLE と同じ口）
mcp_call() {
  python3 - "$1" "$2" "${3:-}" "${4:-}" <<'PY'
import json, os, sys, urllib.request
info = json.load(open(os.path.join(os.environ["TAKO_DISCOVERY_DIR"], "control.json")))
body = json.dumps({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
                   "params": {"name": sys.argv[1], "arguments": json.loads(sys.argv[2])}})
headers = {"Authorization": "Bearer " + info["token"], "Content-Type": "application/json",
           "Accept": "application/json, text/event-stream"}
if sys.argv[3]:
    headers["X-Tako-Pane"] = sys.argv[3]
if sys.argv[4]:
    headers["X-Tako-Role"] = sys.argv[4]
req = urllib.request.Request(info["mcp_url"], data=body.encode(), headers=headers)
with urllib.request.urlopen(req, timeout=60) as r:
    resp = json.loads(r.read())
if "error" in resp:
    print(json.dumps({"rpc_error": resp["error"]["message"]}, ensure_ascii=False))
    sys.exit(0)
result = resp["result"]
text = result["content"][0]["text"]
try:
    out = json.loads(text)
except ValueError:
    out = {"text": text}
if result.get("isError"):
    out = {"is_error": True, "text": text}
print(json.dumps(out, ensure_ascii=False))
PY
}

# transcript（一時の設定 dir の projects 配下）に、目印を含む利用者の発話が何件あるか
transcript_hits() {
  python3 - "$CCD" "$1" <<'PY'
import glob, json, os, sys
root, mark = sys.argv[1], sys.argv[2]
hits = 0
for path in glob.glob(os.path.join(root, "projects", "*", "*.jsonl")):
    for line in open(path, encoding="utf-8", errors="replace"):
        try:
            rec = json.loads(line)
        except ValueError:
            continue
        if rec.get("type") != "user" or rec.get("isMeta"):
            continue
        content = rec.get("message", {}).get("content")
        if isinstance(content, list):
            content = "\n".join(p.get("text", "") for p in content if isinstance(p, dict))
        if isinstance(content, str) and mark in content:
            hits += 1
print(hits)
PY
}

# 送達フローが決着するまで（≤ 60 秒）delivery と mod_turn を 1 秒ごとに記録し、最後の delivery を出す
watch_delivery() {
  local pane="$1" out="$2" i r state
  : > "$out"
  for i in $(seq 1 60); do
    r="$(mcp_call tako_read_pane "{\"pane\":$pane}")"
    printf '%s\n' "$r" | jq -c --arg t "$i" '{t: ($t | tonumber), state: .delivery.state, reason: .delivery.reason, outcome: .delivery.outcome, turn: .mod_turn.turn}' >> "$out"
    state="$(printf '%s' "$r" | jget '.delivery.state')"
    # 待っているあいだの画面（判定の材料。本文は一時 dir の中だけ）
    [ "$i" = 5 ] && printf '%s' "$r" | jq -r '.text' > "$out.screen-wait.txt"
    case "$state" in delivered | gave_up) break ;; esac
    sleep 1
  done
  tail -1 "$out"
}

# --- handoff: 狭い後任ペインへの実際の引き継ぎ ----------------------------------------------
phase_handoff() {
  local legacy="${1:-}" tag a s out last hits cols closed
  tag="handoff${legacy:+-legacy}"
  if [ -z "$legacy" ]; then
    echo "== 1/2/3. 幅 約 48 桁の後任へ実際に引き継ぐ（送達の顛末・受領済みの後任を前任が閉じる・最後の master）"
    start_app "$tag"
  else
    echo "== A/B: TAKO_2330_LEGACY=1（修正前の挙動）で同じ引き継ぎ"
    start_app "$tag" TAKO_2330_LEGACY=1
  fi
  a="$(root_pane)"
  tk send --pane "$a" "cd '$WORK'" >/dev/null 2>&1
  tk title --pane "$a" --role "orchestrator-master:p1" >/dev/null 2>&1
  # 10/18 のタブには QR のプレビューや worker が同居していた。前任の下にペインを 1 枚置く
  # （前任がタブ最後のペインだと「最後のタブの最後のペイン」の規則で close が断られ、測りたいものが隠れる）
  split_new "$a" down >/dev/null
  # 前任 = root（role だけ master・中身はシェル）。前任の名乗りで引き継ぎを撃つ
  out="$(env TAKO_ORCHESTRATOR_ROLE=master:p1 TAKO_PANE_ID="$a" "$TAKO_BIN" orchestrator handoff --pane "$a" 2>&1)"
  s="$(printf '%s' "$out" | jget '.new_master_pane_id')"
  if [ -z "$s" ] || [ "$s" = "null" ]; then
    fail "引き継ぎを撃てない: $(printf '%s' "$out" | head -3)"
    stop_app
    return 1
  fi
  cols="$(pane_cols "$s")"
  echo "    前任 pane=$a / 後任 pane=${s}（${cols} 桁）"
  last="$(watch_delivery "$s" "$TMP/$tag-timeline.jsonl")"
  echo "    送達の経過: $(jq -c '[.t, .state, .reason, .turn]' "$TMP/$tag-timeline.jsonl" | tr '\n' ' ')"
  hits="$(transcript_hits "決定事項その1")"
  echo "    後任の transcript の引き継ぎ文: ${hits} 件 / 最後の delivery: $last"
  if [ "$hits" -lt 1 ]; then
    fail "後任へ引き継ぎ文が届いていない（この段の前提が崩れた）"
    stop_app
    return 1
  fi
  if [ -z "$legacy" ]; then
    check_eq "① 届いた引き継ぎは delivered（受領の証拠 = transcript）" "delivered" "$(printf '%s' "$last" | jget .state)"
    # ② 受領済みの後任を前任が閉じる（CLI・force）→ 断る
    out="$(env TAKO_ORCHESTRATOR_ROLE=master:p1 TAKO_PANE_ID="$a" "$TAKO_BIN" close --pane "$s" --force 2>&1)"
    if pane_alive "$s"; then closed=0; else closed=1; fi
    check_eq "② 受領済みの後任を前任が閉じる close（CLI・force）は断られ後任が残る" "0" "$closed"
    case "$out" in *"$s"*) pass "② 断りの理由に後任のペインが出る（$(printf '%s' "$out" | head -1 | cut -c1-160)）" ;;
      *) fail "② 断りの理由が読めない: $(printf '%s' "$out" | head -2)" ;; esac
    # ③ 後任が前任を閉じる（引き継ぎの完了。MCP・force）→ 通る
    out="$(mcp_call tako_close_pane "{\"pane\":$a,\"force\":true}" "$s" "master:p1")"
    if pane_alive "$a"; then closed=0; else closed=1; fi
    check_eq "境界: 後任 → 前任の close（MCP・force）は通る（引き継ぎの完了）" "1" "$closed"
    # ③ 閉じられた前任の名乗りで残った後任を閉じる（10/18 の 0.2 秒差の 2 本目）
    out="$(mcp_call tako_close_pane "{\"pane\":$s,\"force\":true}" "$a" "master:p1")"
    if pane_alive "$s"; then closed=0; else closed=1; fi
    check_eq "③ 最後の master を閉じる close（MCP・force）は断られ後任が残る" "0" "$closed"
    case "$(printf '%s' "$out" | jget .text)" in *master*) pass "③ 断りの理由と次の一手が応答に出る（$(printf '%s' "$out" | jget .text | head -1 | cut -c1-160)）" ;;
      *) fail "③ 断りの理由が読めない: $out" ;; esac
    check_eq "③ タブの master は 1 人のまま" "1" "$(master_count)"
  else
    # 10/18 の順: 前任が「届いていない」と見て後任を閉じ、後任は手順どおり前任を閉じる
    env TAKO_ORCHESTRATOR_ROLE=master:p1 TAKO_PANE_ID="$a" "$TAKO_BIN" close --pane "$s" --force >/dev/null 2>&1
    if pane_alive "$s"; then closed=0; else closed=1; fi
    check_eq "旧挙動: 受領済みの後任を前任が閉じる close（CLI・force）が通る" "1" "$closed"
    out="$(mcp_call tako_close_pane "{\"pane\":$a,\"force\":true}" "$s" "master:p1")"
    if pane_alive "$a"; then closed=0; else closed=1; fi
    check_eq "旧挙動: 閉じられた後任の名乗りで前任を閉じる close（MCP・force）も通り master が 0 人" "1 0" "$closed $(master_count)"
  fi
  stop_app
}

# --- deliver: 10/20 の形（狭いペインの claude へ長い日本語の本文を await_prompt で送る）----------------
# 本文は約 600 字・段落 3 つ（claude は貼り付けを畳まずに入力欄へ折り返して描く）
long_text() {
  local mark="$1" i
  printf '【復旧の連絡 %s】' "$mark"
  for i in 1 2 3; do
    printf '段落%s: 引き継ぎで前任と後任が互いのペインを閉じ合い、タブから master が消えていた件を復旧した。後任の会話を同じタブへ戻したので、引き継ぎ文の手順を最初から確かめ直し、worker の状態と spawn 元を突き合わせてから、残っている依頼を一つずつ片付けてほしい。閉じ直す前には必ず相手の画面と会話記録を読むこと。\n' "$i"
  done
}
phase_deliver() {
  local legacy="${1:-}" tag p q mark last hits cols
  tag="deliver${legacy:+-legacy}"
  if [ -z "$legacy" ]; then
    echo "== 1/2. 幅 約 48 桁の claude へ長い本文を await_prompt で送る（10/20 の形）"
    start_app "$tag"
  else
    echo "== A/B: TAKO_2330_LEGACY=1 で同じ送信"
    start_app "$tag" TAKO_2330_LEGACY=1
  fi
  p="$(root_pane)"
  q="$(split_new "$p")"
  tk resize --pane "$q" --axis x --share "${DELIVER_SHARE:-0.47}" >/dev/null 2>&1
  cols="$(pane_cols "$q")"
  tk send --pane "$q" "cd '$WORK' && claude --model haiku" >/dev/null 2>&1
  if ! wait_for 60 sh -c "'$TAKO_BIN' read --pane $q 2>/dev/null | grep -q '❯'"; then
    fail "claude が立たない"
    stop_app
    return 1
  fi
  sleep 2
  mark="DELIVER2330-$$-${tag}"
  mcp_call tako_send_input "$(jq -cn --argjson p "$q" --arg t "$(long_text "$mark")" '{pane:$p,text:$t,await_prompt:true}')" >/dev/null
  last="$(watch_delivery "$q" "$TMP/$tag-timeline.jsonl")"
  tk read --pane "$q" > "$TMP/$tag-screen.txt" 2>/dev/null
  echo "    pane=${q}（${cols} 桁）送達の経過: $(jq -c '[.t, .state, .reason, .outcome, .turn]' "$TMP/$tag-timeline.jsonl" | tr '\n' ' ')"
  hits="$(transcript_hits "$mark")"
  echo "    transcript の本文: ${hits} 件 / 最後の delivery: $last"
  if [ "$hits" -lt 1 ]; then
    fail "本文が claude へ届いていない（この段の前提が崩れた）"
    stop_app
    return 1
  fi
  if [ -z "$legacy" ]; then
    check_eq "① 届いた本文は delivered（顛末 received = 会話記録で確かめた）" "delivered received" \
      "$(printf '%s' "$last" | jq -r '"\(.state) \(.outcome)"')"
    check_eq "① persist.log に証拠の種類だけを書く（本文は書かない）" "1 0" \
      "$(grep -c '受領を確かめた（pane='"$q"' 証拠=transcript）' "$TAKO_DATA_DIR/persist.log" | tr -d ' ') $(grep -c "$mark" "$TAKO_DATA_DIR/persist.log" | tr -d ' ')"
  else
    check_eq "旧挙動: 届いているのに gave_up" "gave_up" "$(printf '%s' "$last" | jget .state)"
  fi
  stop_app
}

# --- close: role だけ master にしたペインで最後の 1 人の守り --------------------------------------
phase_close() {
  echo "== 3. 同じタブの master の数で close の可否が決まる（CLI・MCP・境界）"
  start_app close
  local r b c d out
  r="$(root_pane)"
  b="$(split_new "$r")"
  c="$(split_new "$b")"
  d="$(split_new "$c")"
  tk title --pane "$r" --role "orchestrator-master:p1" >/dev/null 2>&1
  tk title --pane "$b" --role "orchestrator-master:p1" >/dev/null 2>&1
  tk title --pane "$c" --role "orchestrator-master:p2" >/dev/null 2>&1
  # 境界: 同じタブ・同じプロファイルに 2 人目が居る → 通る（CLI・force）
  env TAKO_ORCHESTRATOR_ROLE=master:p1 TAKO_PANE_ID="$r" "$TAKO_BIN" close --pane "$b" --force >/dev/null 2>&1
  if pane_alive "$b"; then fail "境界: 2 人目が居る close が通らない"; else pass "境界: 同じプロファイルの 2 人目が残る close は通る（CLI）"; fi
  # 最後の 1 人（p1）を別の master（p2）が閉じる → 断る（CLI・force）
  out="$(env TAKO_ORCHESTRATOR_ROLE=master:p2 TAKO_PANE_ID="$c" "$TAKO_BIN" close --pane "$r" --force 2>&1)"
  if pane_alive "$r"; then pass "p1 の最後の master を p2 の master が閉じる close（CLI・force）は断る"; else fail "最後の master が閉じられた（CLI）"; fi
  case "$out" in *p1*) pass "CLI の断りにプロファイルと理由（$(printf '%s' "$out" | head -1 | cut -c1-120)）" ;; *) fail "CLI の断りが読めない: $out" ;; esac
  out="$(mcp_call tako_close_pane "{\"pane\":$r,\"force\":true}" "$c" "master:p2")"
  if pane_alive "$r"; then pass "同じ close（MCP・force）も断る"; else fail "最後の master が閉じられた（MCP）"; fi
  check_eq "MCP の応答はエラー" "true" "$(printf '%s' "$out" | jget .is_error)"
  # 自分で自分を閉じるのは通す（master 同士の close ではない）
  env TAKO_ORCHESTRATOR_ROLE=master:p2 TAKO_PANE_ID="$c" "$TAKO_BIN" close --pane "$c" >/dev/null 2>&1
  if pane_alive "$c"; then fail "自己片付け（最後の p2）が断られた"; else pass "境界: master が自分のペインを閉じるのは通る"; fi
  # role を名乗らない呼び出し（人がシェルで打つ CLI）は従来どおり
  env -u TAKO_ORCHESTRATOR_ROLE "$TAKO_BIN" close --pane "$r" >/dev/null 2>&1
  if pane_alive "$r"; then fail "role を名乗らない close が断られた"; else pass "境界: role を名乗らない close（人の CLI）は従来どおり通る"; fi
  : "$d"
  stop_app
}

# --- crosstab: 後任が別のタブに立った引き継ぎ -----------------------------------------------
phase_crosstab() {
  echo "== 3. --tab で別のタブへ後任を立てた引き継ぎ（後任は前任を閉じられる・帳面に無い master は断る）"
  start_app crosstab
  local a t2 m s out closed
  a="$(root_pane)"
  tk send --pane "$a" "cd '$WORK'" >/dev/null 2>&1
  tk title --pane "$a" --role "orchestrator-master:p1" >/dev/null 2>&1
  split_new "$a" down >/dev/null
  out="$(tk tab new 2>/dev/null)"
  t2="$(printf '%s' "$out" | jget '.tab')"
  m="$(printf '%s' "$out" | jget '.pane')"
  out="$(env TAKO_ORCHESTRATOR_ROLE=master:p1 TAKO_PANE_ID="$a" "$TAKO_BIN" orchestrator handoff --pane "$a" --tab "$t2" 2>&1)"
  s="$(printf '%s' "$out" | jget '.new_master_pane_id')"
  if [ -z "$s" ] || [ "$s" = "null" ]; then
    fail "別タブへの引き継ぎを撃てない: $(printf '%s' "$out" | head -3)"
    stop_app
    return 1
  fi
  check_eq "後任は別のタブに立つ" "$t2" "$(printf '%s' "$out" | jget '.new_master_tab_id')"
  echo "    前任 pane=${a}（タブ 1）/ 後任 pane=${s}（タブ ${t2}）"
  # 帳面に無い別タブの master（同じプロファイル）が前任を閉じる → 前任のタブの最後の master なので断る
  tk title --pane "$m" --role "orchestrator-master:p1" >/dev/null 2>&1
  out="$(env TAKO_ORCHESTRATOR_ROLE=master:p1 TAKO_PANE_ID="$m" "$TAKO_BIN" close --pane "$a" --force 2>&1)"
  if pane_alive "$a"; then closed=0; else closed=1; fi
  check_eq "帳面に無い別タブの master が前任を閉じる close（CLI・force）は断る" "0" "$closed"
  # 後任が手順どおり前任を閉じる（MCP・force）→ タブの人数に関係なく通る
  out="$(mcp_call tako_close_pane "{\"pane\":$a,\"force\":true}" "$s" "master:p1")"
  if pane_alive "$a"; then closed=0; else closed=1; fi
  check_eq "別タブの後任が前任を閉じる close（MCP・force）は通る（前任を幽霊にしない）" "1" "$closed"
  [ "$closed" = 1 ] || echo "    応答: $(printf '%s' "$out" | jget .text | head -1 | cut -c1-160)"
  stop_app
}

# --- miss: 本当に届かない送信は gave_up のまま ------------------------------------------------
phase_miss() {
  echo "== 2. 貼り付けを捨てる注入（本当に届かない）では受領の証拠が無く gave_up のまま"
  start_app miss TAKO_2330_INJECT_DROP_PASTE=1
  local p mark last hits
  p="$(root_pane)"
  tk send --pane "$p" "cd '$WORK' && claude --model haiku" >/dev/null 2>&1
  if ! wait_for 60 sh -c "'$TAKO_BIN' read --pane $p 2>/dev/null | grep -q '❯'"; then
    fail "claude が立たない"
    stop_app
    return 1
  fi
  mark="MISS2330-$$"
  mcp_call tako_send_input "$(jq -cn --argjson p "$p" --arg t "届かないはずの依頼 $mark" '{pane:$p,text:$t,await_prompt:true}')" >/dev/null
  last="$(watch_delivery "$p" "$TMP/miss-timeline.jsonl")"
  hits="$(transcript_hits "$mark")"
  check_eq "transcript に目印は入らない（本当に届いていない）" "0" "$hits"
  check_eq "送達は gave_up のまま（受領の証拠を作らない）" "gave_up" "$(printf '%s' "$last" | jget .state)"
  stop_app
}

want deliver && phase_deliver
want handoff && phase_handoff
want close && phase_close
want crosstab && phase_crosstab
want miss && phase_miss
want legacy-deliver && phase_deliver legacy
want legacy-handoff && phase_handoff legacy

echo
echo "結果: PASS=$PASS FAIL=$FAIL"
[ "$FAIL" -eq 0 ]
