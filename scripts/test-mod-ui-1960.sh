#!/bin/bash
# #1960 の実経路テスト: tako mod の定型の UI 設定（<data_dir>/claude-mod/ui.json）を
# CLI（`tako mod ui`）・setup（`tako setup --yes --answers -` の mod_ui と `--review` の対話）・
# MCP（隔離 GUI 越しの `tako_mod` の action=ui）の 3 つの口で同じように変えて、ui.json が字面で
# 一致すること、不正な値はどの口でも書かれず使える値が返ること、壊れた / 新しい ui.json で落ちない
# こと、報告の応答の `tako.view.ui` に検証済みの値が載ることを実物のバイナリで確かめる。
#
# tako:run: bash scripts/test-mod-ui-1960.sh
#
# ONLY=cli（GUI 無し = 速い）/ ONLY=gui（隔離 GUI だけ）/ 既定は両方。
#
# **本番の tako / 設定・claude の設定 dir には一切触れない**: CLI と setup は `env -i` で HOME /
# TAKO_DATA_DIR / PATH を一時 dir へ差し替え、claude / codex / git / tmux / tailscale / brew はスタブ。
# 隔離 GUI は `scripts/lib/isolated-gui.sh` の 1 実装（仮想ディスプレイ tako-vd・AX 不使用）で、
# 自動リネームと復元を止める（TAKO_AUTORENAME=0 / TAKO_PERSIST=0）。PATH に本物の claude を置かない。
# 落とすのは自分で起こした pid だけ。面を用意できなければ GUI の段は「未実測」で終了コード 4。
set -uo pipefail

unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_ORCHESTRATOR_ROLE TAKO_MCP_URL \
  TAKO_CLI CLAUDE_CODE_PLUGIN_DIRS CLAUDE_CONFIG_DIR TAKO_DATA_DIR

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT" || exit 1
ONLY="${ONLY:-all}"
PASS=0
FAIL=0
pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
fail() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }
check_eq() {
  if [ "$2" = "$3" ]; then pass "$1"; else fail "$1（期待 '${2}' / 実際 '${3}'）"; fi
}
contains() {
  case "$2" in *"$3"*) pass "$1" ;; *) fail "$1（'${3}' が無い: ${2}）" ;; esac
}

TMP="$(mktemp -d "${TMPDIR:-/tmp}/tako-1960-XXXXXX")"
TMP="$(cd "$TMP" && pwd -P)"
APP_PID=""
cleanup() {
  if [ -n "$APP_PID" ]; then
    stop_isolated_gui "$APP_PID"
  fi
  rm -rf "$TMP"
}
# shellcheck source=lib/exit-guard.sh
. "$REPO_ROOT/scripts/lib/exit-guard.sh"
tako_exit_trap cleanup "test-mod-ui-1960"

cargo build -q -p tako-cli || exit 1
TAKO_BIN="$REPO_ROOT/target/debug/tako"
PTY="$REPO_ROOT/scripts/lib/pty-answer.py"

# --- 隔離した CLI / setup の環境（#1506 の型） ---------------------------------
BIN="$TMP/bin"
mkdir -p "$BIN" "$TMP/h"
cat > "$BIN/claude" <<'EOF'
#!/bin/sh
if [ "${1:-}" = "auth" ] && [ "${2:-}" = "status" ]; then
  echo '{"loggedIn":true,"authMethod":"claude.ai","subscriptionType":"Max"}'; exit 0
fi
if [ "${1:-}" = "--version" ]; then echo "2.1.294 (Claude Code)"; exit 0; fi
if [ "${1:-}" = "mcp" ] && [ "${2:-}" = "list" ]; then echo "tako: /stub/tako mcp serve - Connected"; exit 0; fi
exit 0
EOF
printf '#!/bin/sh\nif [ "$1" = "-l" ]; then shift; fi\nexec /bin/sh "$@"\n' > "$BIN/isosh"
for t in git tmux tailscale; do printf '#!/bin/sh\necho "%s (stub)"\nexit 0\n' "$t" > "$BIN/$t"; done
printf '#!/bin/sh\nexit 0\n' > "$BIN/brew"
chmod +x "$BIN"/*

# iso <data dir> <コマンド...>
iso() {
  local data="$1"
  shift
  env -i HOME="$TMP/h" TAKO_DATA_DIR="$data" TAKO_ISOLATED=1 TAKO_LANG=ja \
    CODEX_HOME="$TMP/h/.codex" SHELL="$BIN/isosh" TAKO_TMUX_BIN="$BIN/tmux" \
    PATH="$BIN:/usr/bin:/bin:/usr/sbin:/sbin" TERM=dumb LANG=ja_JP.UTF-8 "$@"
}
ui_file() { printf '%s/claude-mod/ui.json' "$1"; }
sha() { if [ -f "$1" ]; then shasum -a 256 "$1" | cut -d' ' -f1; else echo absent; fi; }
# JSON の 1 か所を読む（python3 -I。$1 = JSON のファイルか '-'、$2 = 式）
jget() {
  /usr/bin/python3 -I -c '
import json, sys
src = sys.argv[1]
d = json.load(sys.stdin if src == "-" else open(src, encoding="utf-8"))
v = eval(sys.argv[2], {"d": d})
print(json.dumps(v, ensure_ascii=False, sort_keys=True) if isinstance(v, (dict, list)) else v)' "$1" "$2"
}

# 同じ 6 つの変更（受け入れ条件 1）
CLI_STEPS='set usage_bar.place band
set colors.accent claude
set band.segments pane,attention,buttons
button add context
button add split-right
button move split-right first'

cli_steps() { # $1 = data dir
  local line rc=0
  while IFS= read -r line; do
    # shellcheck disable=SC2086
    iso "$1" "$TAKO_BIN" mod ui $line > /dev/null 2>&1 || rc=1
  done <<EOF
$CLI_STEPS
EOF
  return $rc
}

D_CLI="$TMP/d-cli"
mkdir -p "$D_CLI"

if [ "$ONLY" != "gui" ]; then
  echo "== ① CLI と setup --answers で同じ変更 → ui.json が字面で一致（受け入れ条件 1・3） =="
  cli_steps "$D_CLI"
  check_eq "CLI の 6 つの変更が全部通る" "0" "$?"
  D_SETUP="$TMP/d-setup"
  mkdir -p "$D_SETUP"
  ANSWERS='{"launch_agent":"none","mod_ui":{"set":{"usage_bar.place":"band","colors.accent":"claude","band.segments":["pane","attention","buttons"]},"buttons":[{"kind":"tako","value":"split-right"},{"value":"compact"},{"value":"context"}]}}'
  printf '%s' "$ANSWERS" | iso "$D_SETUP" "$TAKO_BIN" setup --yes --answers - > "$TMP/setup.log" 2>&1
  check_eq "setup --yes --answers が終了コード 0" "0" "$?"
  contains "setup が UI の段を出す" "$(cat "$TMP/setup.log")" "[input] Claude Code の画面の UI（ui.json）"
  check_eq "CLI と setup の ui.json が字面で一致" "$(sha "$(ui_file "$D_CLI")")" "$(sha "$(ui_file "$D_SETUP")")"
  check_eq "並べ替えが効いている" '["split-right", "compact", "context"]' \
    "$(jget "$(ui_file "$D_CLI")" '[b["id"] for b in d["buttons"]]')"

  echo
  echo "== ② 不正な値はどの口でも書かれず使える値が返る（受け入れ条件 2・エッジ） =="
  D_BAD="$TMP/d-bad"
  mkdir -p "$D_BAD"
  for v in clear context cost model effort tako split-right; do
    iso "$D_BAD" "$TAKO_BIN" mod ui button add "$v" > /dev/null 2>&1
  done
  BEFORE="$(sha "$(ui_file "$D_BAD")")"
  check_eq "8 個まで足せた" "8" "$(jget "$(ui_file "$D_BAD")" 'len(d["buttons"])')"
  bad() { # bad <名前> <含むはずの語> <tako mod ui の引数...>
    local name="$1" want="$2"
    shift 2
    local out rc
    out="$(iso "$D_BAD" "$TAKO_BIN" mod ui "$@" 2>&1)"
    rc=$?
    check_eq "${name}: 終了コード 1" "1" "$rc"
    contains "${name}: 使える値・理由を返す" "$out" "$want"
    contains "${name}: 書いていないと言う" "$out" "ui.json は書いていない"
    check_eq "${name}: ui.json は 1 バイトも変わらない" "$BEFORE" "$(sha "$(ui_file "$D_BAD")")"
  }
  bad "9 個目のボタン" "8 個まで" button add split-down
  bad "hotkey の重複" "hotkey" button add open-cwd --hotkey c
  bad "label 17 桁" "16 桁" button add shell "npm test" --label abcdefghijklmnopq
  bad "語彙外の action" "slash / tako / shell / prompt" button add javascript "alert(1)"
  bad "許可リスト外の slash" "compact / clear / context" button add slash bash
  bad "テーマに無い色" "suggestion" set colors.dim "#ff0000"
  bad "知らないキー" "band.hidden" set band.color red
  # setup の口も同じ検証で、書く前に断る
  D_SBAD="$TMP/d-setup-bad"
  mkdir -p "$D_SBAD"
  BAD_ANSWERS='{"launch_agent":"none","mod_ui":{"buttons":[{"kind":"slash","value":"bash"}]}}'
  OUT="$(printf '%s' "$BAD_ANSWERS" | iso "$D_SBAD" "$TAKO_BIN" setup --yes --answers - 2>&1)"
  RC=$?
  if [ "$RC" -ne 0 ]; then pass "setup --answers の不正な mod_ui は非ゼロで止まる（${RC}）"; else fail "setup が不正な mod_ui を通した"; fi
  contains "setup の理由も同じ文言" "$OUT" "mod_ui: slash で送れるのは許可リストのコマンドだけ"
  check_eq "setup は ui.json を書いていない" "absent" "$(sha "$(ui_file "$D_SBAD")")"

  echo
  echo "== ③ 壊れた ui.json は既定で動き退避が残り persist.log に記録（受け入れ条件 4） =="
  D_BROKEN="$TMP/d-broken"
  mkdir -p "$D_BROKEN/claude-mod"
  printf '{"schema_version": 1, "buttons": [{"id": 7}], "chat": "x"' > "$(ui_file "$D_BROKEN")"
  ORIG="$(sha "$(ui_file "$D_BROKEN")")"
  iso "$D_BROKEN" "$TAKO_BIN" mod ui --json > "$TMP/broken.json" 2>&1
  check_eq "読むだけは終了コード 0（落ちない）" "0" "$?"
  check_eq "state = repaired" "repaired" "$(jget "$TMP/broken.json" 'd["state"]')"
  check_eq "既定のボタンで動く" "compact" "$(jget "$TMP/broken.json" 'd["ui"]["buttons"][0]["id"]')"
  check_eq "元の中身が .unreadable.bak に残る" "$ORIG" "$(sha "$D_BROKEN/claude-mod/ui.json.unreadable.bak")"
  check_eq "読むだけでは元を書き換えない" "$ORIG" "$(sha "$(ui_file "$D_BROKEN")")"
  if grep -q 'claude-mod/ui.json の読めない部分を既定で動かす' "$D_BROKEN/persist.log" 2>/dev/null; then
    pass "persist.log に 1 行残る"
  else
    fail "persist.log に記録が無い"
  fi
  iso "$D_BROKEN" "$TAKO_BIN" mod ui --json > /dev/null 2>&1
  check_eq "同じ中身ならもう 1 行増えない" "1" \
    "$(grep -c 'claude-mod/ui.json の読めない部分' "$D_BROKEN/persist.log" 2>/dev/null)"
  iso "$D_BROKEN" "$TAKO_BIN" migrate status > "$TMP/migrate.txt" 2>&1
  contains "tako migrate status が ui.json の破損を申告する" "$(cat "$TMP/migrate.txt")" "claude_mod_ui"
  iso "$D_BROKEN" "$TAKO_BIN" mod ui set chat.code_copy false > /dev/null 2>&1
  check_eq "変更の操作で直した形を書く" "False" "$(jget "$(ui_file "$D_BROKEN")" 'd["chat"]["code_copy"]')"

  echo
  echo "== ④ 新しい tako が書いた形（schema_version 2）は読めるが書き換えない =="
  D_FUT="$TMP/d-future"
  mkdir -p "$D_FUT/claude-mod"
  /usr/bin/python3 -I - "$(ui_file "$D_CLI")" "$(ui_file "$D_FUT")" <<'PY'
import json, sys
d = json.load(open(sys.argv[1], encoding="utf-8"))
d["schema_version"] = 2
d["band"]["future_field"] = True
open(sys.argv[2], "w", encoding="utf-8").write(json.dumps(d))
PY
  FUT="$(sha "$(ui_file "$D_FUT")")"
  check_eq "読むと state = future" "future" "$(iso "$D_FUT" "$TAKO_BIN" mod ui --json 2>/dev/null | jget - 'd["state"]')"
  OUT="$(iso "$D_FUT" "$TAKO_BIN" mod ui set usage_bar.place off 2>&1)"
  check_eq "変更は終了コード 1" "1" "$?"
  contains "理由を言う" "$OUT" "書き換えない"
  check_eq "1 バイトも変わらない" "$FUT" "$(sha "$(ui_file "$D_FUT")")"

  echo
  echo "== ⑤ setup --review の対話（最後の 3 択 + ボタン選び）も同じ 1 実装で当てる =="
  if [ -x /usr/bin/python3 ] && [ -f "$PTY" ]; then
    D_REV="$TMP/d-review"
    mkdir -p "$D_REV"
    # 1 回目: すべて Enter（今のまま）。自分の問いより前に出る `選択 [` の数を数える
    iso "$D_REV" /usr/bin/python3 "$PTY" --expect '選択 [' --expect '[y/N]:' --timeout 180 \
      -- "$TAKO_BIN" setup --review > "$TMP/review1.log" 2>&1
    check_eq "Enter だけなら ui.json を作らない" "absent" "$(sha "$(ui_file "$D_REV")")"
    COUNT_PY='import sys
text = open(sys.argv[1], encoding="utf-8", errors="replace").read()
head = text.split("雛形を選択")[0]
print(head.count("選択 [") + head.count("[y/N]:"))'
    BEFORE_MINE="$(/usr/bin/python3 -I -c "$COUNT_PY" "$TMP/review1.log")"
    if grep -q '雛形を選択' "$TMP/review1.log"; then
      pass "--review の最後に雛形の 3 択が出る（前の問い ${BEFORE_MINE} 個）"
      ARGS=()
      i=0
      while [ "$i" -lt "$BEFORE_MINE" ]; do ARGS+=(--answer ""); i=$((i + 1)); done
      # 2 = おすすめ、ボタンは 1 = /compact と 8 = split right（組み込み 7 個の次が tako の操作）
      ARGS+=(--answer 2 --answer "1,8")
      iso "$D_REV" /usr/bin/python3 "$PTY" --expect '選択 [' --expect '[y/N]:' \
        ${ARGS[@]+"${ARGS[@]}"} --timeout 180 -- "$TAKO_BIN" setup --review > "$TMP/review2.log" 2>&1
      check_eq "おすすめ + /compact と split right の 2 個" '["compact", "split-right"]' \
        "$(jget "$(ui_file "$D_REV")" '[b["id"] for b in d["buttons"]]' 2>/dev/null)"
    else
      fail "--review に雛形の 3 択が出ない"
      sed -e "s#${TMP}#<TMP>#g" "$TMP/review1.log" | tail -20
    fi
  else
    echo "  [skip] /usr/bin/python3 か PTY ドライバが無い（未実測）"
  fi
fi

if [ "$ONLY" != "cli" ]; then
  echo
  echo "== ⑥ 隔離 GUI 越しの MCP / tako mod report / tako mod band（受け入れ条件 1・2・5） =="
  [ -s "$(ui_file "$D_CLI")" ] || cli_steps "$D_CLI"
  # shellcheck source=lib/isolated-gui.sh
  . "$REPO_ROOT/scripts/lib/isolated-gui.sh"
  isolated_gui_bins || exit 1
  D_GUI="$TMP/d-gui"
  mkdir -p "$D_GUI" "$TMP/gh" "$TMP/zdot" "$TMP/disc" "$TMP/orch" "$TMP/gtmp"
  printf "PROMPT='tako %%1~ %%%% '\nRPROMPT=''\n" > "$TMP/zdot/.zshrc"
  # この先の CLI（wait_isolated_gui の `tako list` を含む）も GUI と同じ隔離の env で叩く
  # （発見ファイル・HOME が本番のままだと、繋がらないか本番の GUI へ繋がる）。cargo はここより前で済ませた
  export TAKO_ISOLATED=1 HOME="$TMP/gh" TAKO_DATA_DIR="$D_GUI" TAKO_DISCOVERY_DIR="$TMP/disc" \
    TAKO_ORCHESTRATOR_DIR="$TMP/orch" TAKO_SESSIONS_FILE="$TMP/sessions.yaml" \
    TAKO_PANE_LOG_DIR="$TMP/panelogs" TAKO_WORKERS_FILE="$TMP/workers.yaml" \
    TAKO_TMUX_SOCKET="tako-1960-$$"
  launch_isolated_gui "$TMP/app.log" HOME="$TMP/gh" ZDOTDIR="$TMP/zdot" TAKO_LANG=ja \
    TMPDIR="$TMP/gtmp/" TAKO_PERSIST=0 TAKO_AUTORENAME=0 TAKO_DATA_DIR="$D_GUI" \
    TAKO_DISCOVERY_DIR="$TMP/disc" TAKO_ORCHESTRATOR_DIR="$TMP/orch" \
    TAKO_SESSIONS_FILE="$TMP/sessions.yaml" TAKO_PANE_LOG_DIR="$TMP/panelogs" \
    TAKO_WORKERS_FILE="$TMP/workers.yaml" TAKO_TMUX_SOCKET="tako-1960-$$" \
    PATH="/usr/bin:/bin:/usr/sbin:/sbin" || exit $?
  APP_PID="$ISOLATED_GUI_PID"
  wait_isolated_gui "$TMP/app.log" || exit 1
  SOCK="$D_GUI/tako.sock"
  [ -f "$D_GUI/tako.sock.path" ] && SOCK="$(cat "$D_GUI/tako.sock.path")"
  TOKEN="$(cat "$D_GUI/token" 2>/dev/null)"
  gui() { TAKO_SOCKET="$SOCK" TAKO_TOKEN="$TOKEN" "$TAKO_BIN" "$@"; }
  # MCP の tools/call tako_mod（$1 = arguments の JSON）。応答の result をそのまま出す
  mcp_mod() {
    local args="$1" init call
    init='{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t1960","version":"0"}}}'
    call="$(/usr/bin/python3 -I -c 'import json,sys; print(json.dumps({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"tako_mod","arguments":json.loads(sys.argv[1])}}))' "$args")"
    { printf '%s\n' "$init"; printf '%s\n' "$call"; sleep 1; } \
      | TAKO_SOCKET="$SOCK" TAKO_TOKEN="$TOKEN" "$TAKO_BIN" mcp serve 2>/dev/null \
      | /usr/bin/python3 -I -c '
import json, sys
for line in sys.stdin:
    try:
        m = json.loads(line)
    except Exception:
        continue
    if m.get("id") == 2:
        print(json.dumps(m.get("result", m.get("error"))))'
  }
  MCP_STEPS='{"action":"ui","op":"set","key":"usage_bar.place","value":"band"}
{"action":"ui","op":"set","key":"colors.accent","value":"claude"}
{"action":"ui","op":"set","key":"band.segments","value":["pane","attention","buttons"]}
{"action":"ui","op":"button_add","value":"context"}
{"action":"ui","op":"button_add","kind":"tako","value":"split-right"}
{"action":"ui","op":"button_move","id":"split-right","to":"first"}'
  MCP_ERR=0
  while IFS= read -r args; do
    if [ "$(mcp_mod "$args" | jget - 'd.get("isError")')" != "False" ]; then MCP_ERR=1; fi
  done <<EOF
$MCP_STEPS
EOF
  check_eq "MCP の 6 つの変更が全部通る" "0" "$MCP_ERR"
  check_eq "CLI と MCP の ui.json が字面で一致" "$(sha "$(ui_file "$D_CLI")")" "$(sha "$(ui_file "$D_GUI")")"
  GBEFORE="$(sha "$(ui_file "$D_GUI")")"
  RES="$(mcp_mod '{"action":"ui","op":"button_add","kind":"slash","value":"bash"}')"
  check_eq "MCP の不正な値は isError" "True" "$(printf '%s' "$RES" | jget - 'd["isError"]')"
  contains "MCP も使える値を返す" "$(printf '%s' "$RES" | jget - 'd["content"][0]["text"]')" "compact / clear / context"
  check_eq "MCP の不正な値は書かない" "$GBEFORE" "$(sha "$(ui_file "$D_GUI")")"
  # 報告の応答の tako.view.ui
  PANE="$(gui list 2>/dev/null | jget - 'd["tabs"][0]["panes"][0]["id"]')"
  REPORT='{"schema":1,"mod_version":"t1960","claude_version":"2.1.294","at":1,"rate_limits":[],"turn":"idle","classic_events":true,"ended":false}'
  printf '%s' "$REPORT" | TAKO_PANE_ID="$PANE" gui mod report > "$TMP/report1.json" 2>&1
  check_eq "報告の応答の tako.view.ui が ui.json と同じ" \
    "$(jget "$(ui_file "$D_GUI")" 'd')" "$(jget "$TMP/report1.json" 'd["tako"]["view"]["ui"]')"
  gui mod band off > /dev/null 2>&1
  check_eq "tako mod band off は ui.json の band.hidden を書く" "True" "$(jget "$(ui_file "$D_GUI")" 'd["band"]["hidden"]')"
  printf '%s' "$REPORT" | TAKO_PANE_ID="$PANE" gui mod report > "$TMP/report2.json" 2>&1
  check_eq "次の報告の band_request で中継する" "True" "$(jget "$TMP/report2.json" 'd["tako"]["view"]["band_request"]["hidden"]')"
  # mod の中で後から出した（toggled_at が新しい）→ ui.json へ取り込む
  LATER="$(( $(jget "$(ui_file "$D_GUI")" 'd["band"]["toggled_at"]') + 1000 ))"
  REPORT_BAND="$(/usr/bin/python3 -I -c 'import json,sys; d=json.loads(sys.argv[1]); d["band"]={"hidden":False,"shown":True,"segments":[],"toggled_at":int(sys.argv[2])}; print(json.dumps(d))' "$REPORT" "$LATER")"
  printf '%s' "$REPORT_BAND" | TAKO_PANE_ID="$PANE" gui mod report > /dev/null 2>&1
  check_eq "mod の中の切り替えを ui.json へ取り込む" "False/${LATER}" \
    "$(jget "$(ui_file "$D_GUI")" 'str(d["band"]["hidden"]) + "/" + str(d["band"]["toggled_at"])')"
  check_eq "tako mod の status に ui の様子" "valid" "$(gui mod --json 2>/dev/null | jget - 'd["ui"]["state"]')"
  # 動いている GUI のそばで壊す → 報告は止まらず既定で動く
  printf 'broken' > "$(ui_file "$D_GUI")"
  printf '%s' "$REPORT" | TAKO_PANE_ID="$PANE" gui mod report > "$TMP/report3.json" 2>&1
  check_eq "壊れても報告は受け取る" "stored" "$(jget "$TMP/report3.json" 'd["accepted"]')"
  check_eq "壊れたら応答は既定のボタン" "compact" "$(jget "$TMP/report3.json" 'd["tako"]["view"]["ui"]["buttons"][0]["id"]')"
  if [ -f "$D_GUI/claude-mod/ui.json.unreadable.bak" ]; then pass "GUI でも退避が残る"; else fail "GUI で退避が無い"; fi
  if kill -0 "$APP_PID" 2>/dev/null; then pass "GUI は生きている"; else fail "GUI が落ちた"; fi
fi

echo
echo "PASS=$PASS FAIL=$FAIL"
if [ "$FAIL" -eq 0 ]; then tako_exit 0; fi
exit 1
