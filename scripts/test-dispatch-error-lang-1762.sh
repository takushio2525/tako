#!/bin/bash
# test-dispatch-error-lang-1762.sh — dispatch の断り（`DispatchError` の表示）が表示言語に従う
# 実経路テスト（Issue #1762）
#
# 何を確かめるか:
#   #1762 以前は `DispatchError` の表示が前置き（「無効なパラメータ: 」「ペイン N が見つからない」等）を
#   日本語で固定していたので、英語表示では「無効なパラメータ: Invalid color value: …」のように
#   日英が混ざっていた。表示言語ごとに隔離 GUI を立て、次の断りを CLI と MCP の両方から引き出す:
#      invalid   不正なパラメータ（理由の文が日英を持つ = テーマの色の値。理由は入れ子の `: `）
#      pane      見つからないペイン
#      tab       見つからない退避タブ
#      nosession ターミナルセッションの無いペイン（プレビュー）
#   #2358 で次も足した（tako-core の断りの理由の文と、dispatch へ届く前の手元の断り）:
#      noresize   分割の無いペインのリサイズ（PaneTreeError::NoResizableSplit）
#      lasttab    最後のタブの最後のペインを閉じる（WorkspaceError::LastTab）
#      ontoself   ペインを自分自身の隣へ移す（WorkspaceError::MoveOntoSelf）
#      lastwindow 最後のウィンドウを閉じる（WorkspaceError::LastWindow）
#      notarget   対象ペインの未指定（CLI は `--pane` も TAKO_PANE_ID も無い・MCP は `pane` も
#                 呼び出し元も無い。どちらも dispatch へ送らずに手元で断る）
#   ① 英語表示: どの出力にも日本語（かな・漢字）が混ざらない
#   ② 日本語表示: 日本語の前置きで返る（修正前とのバイト一致は TAKO_1762_KEEP の出力を突き合わせる）
#   ③ GUI を経由しない CLI の直呼び（`tako orchestrator layout`）も、前置きは settings.json の表示言語
#      （この理由の文は日本語だけなので、英語表示では前置きだけを見る）
#   ④ 表示言語の設定が無い（system）ときは既存の解決（TAKO_LANG → ロケール → OS）に従う
#      （ここでは TAKO_LANG=en で英語へ解決させ、① と同じく日本語が混ざらないことを見る）
#
# A/B: 修正前のバイナリを `TAKO_BIN=… APP_BIN=… bash scripts/test-dispatch-error-lang-1762.sh` で渡すと
# ① と ③ の英語が NG になる（#1762 の症状。#2358 より前なら #2358 の 5 種も NG）。`TAKO_1762_KEEP=<dir>` を渡すと全出力を
# `<dir>/<言語>/<名前>.(cli|mcp|rc)` へ残す（修正前後のバイト一致の突き合わせ用）。
#
# **本番の tako / 設定には一切触らない**（data / HOME は mktemp 配下、tmux は専用ソケット、
# 落とすのは自分で起こした pid だけ）。窓は常設の仮想ディスプレイ（tako-vd）へ出る（#1141）。
#
# 使い方: bash scripts/test-dispatch-error-lang-1762.sh
#
# **CI には載せない**（実 GUI + 仮想ディスプレイが要る）。日英の文言と網羅は単体テスト
# （`crates/tako-control/tests/issue1762_dispatch_error_lang.rs`）が CI で見る。
set -uo pipefail

# **本番 GUI を指す env を最初に落とす**（#1449 / #1450 で本番にペインが漏れた実例が 2 件）
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_MCP_URL TAKO_ORCHESTRATOR_ROLE TAKO_LANG

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PASS=0
FAIL=0

pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
fail() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }

TMP="$(mktemp -d /tmp/tako-1762-XXXXXX)"
APP_PID=""
TMUX_SOCKET=""
cleanup() {
  stop_isolated_gui "$APP_PID"
  [ -n "$TMUX_SOCKET" ] && tmux -L "$TMUX_SOCKET" kill-server >/dev/null 2>&1
  rm -rf "${TMP:?}"
}
trap cleanup EXIT

# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
isolated_gui_bins || exit 1

KEEP="${TAKO_1762_KEEP:-}"

# 日本語（かな・漢字・全角の括弧とコロン）を含むか。含めば 1 行目にその文字を出して 0 を返す
has_japanese() {
  python3 -c 'import re,sys
t = open(sys.argv[1], encoding="utf-8", errors="replace").read()
m = re.search(r"[　-ヿ一-鿿＀-￯]", t)
if m:
    print(m.group(0)); sys.exit(0)
sys.exit(1)' "$1"
}

# settings.json の表示言語を書く（GUI を通さずファイルだけ）。空なら language を消す（= system）
write_language() {
  python3 - "$TAKO_DATA_DIR/settings.json" "$1" <<'PY'
import json, os, sys
p, lang = sys.argv[1], sys.argv[2]
d = {}
if os.path.exists(p):
    try:
        d = json.load(open(p, encoding="utf-8"))
    except Exception:
        d = {}
if lang:
    d["language"] = lang
else:
    d.pop("language", None)
json.dump(d, open(p, "w", encoding="utf-8"), ensure_ascii=False, indent=2)
PY
}

# MCP の tools/call を 1 本撃ち、結果の本文を $2 へ書く。isError を返す
mcp_call() {
  local tool="$1" args="$2" out="$3" sock token
  cat > "$TMP/mcp-in.jsonl" <<JSONL
{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test-1762","version":"0"}}}
{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"${tool}","arguments":${args}}}
JSONL
  # `mcp serve` はツール公開の判定を env だけで行う（FR-2.3.2）ので接続情報ファイルから渡す。
  # **トークンは表示しない**（`conventions.md`: 診断ログに TAKO_TOKEN を出さない）
  sock="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["socket"])' "$TAKO_DISCOVERY_DIR/control.json" 2>/dev/null)"
  token="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["token"])' "$TAKO_DISCOVERY_DIR/control.json" 2>/dev/null)"
  if [ -z "$sock" ] || [ -z "$token" ]; then
    : > "$out"
    echo "is_error=__NO_DISCOVERY__"
    return
  fi
  env TAKO_SOCKET="$sock" TAKO_TOKEN="$token" \
    "$TAKO_BIN" mcp serve < "$TMP/mcp-in.jsonl" > "$TMP/mcp-out.jsonl" 2> "$TMP/mcp-err.log"
  python3 - "$TMP/mcp-out.jsonl" "$out" <<'PY'
import json, sys
out = open(sys.argv[2], "w", encoding="utf-8")
for line in open(sys.argv[1], encoding="utf-8"):
    line = line.strip()
    if not line:
        continue
    try:
        msg = json.loads(line)
    except Exception:
        continue
    if msg.get("id") != 2:
        continue
    res = msg.get("result") or {}
    text = "".join(c.get("text", "") for c in res.get("content", []) if c.get("type") == "text")
    if msg.get("error"):
        text = msg["error"].get("message", "")
    out.write(text)
    print("is_error=%s" % bool(res.get("isError") or msg.get("error")))
    sys.exit(0)
print("is_error=__NO_RESPONSE__")
PY
}

# CLI を 1 本撃って $DIR/<名前>.cli（標準出力 + 標準エラー）と .rc を書く
cli_case() {
  local name="$1"
  shift
  "$TAKO_BIN" "$@" > "$DIR/$name.cli" 2>&1
  echo "$?" > "$DIR/$name.rc"
}

# 1 つの表示言語で隔離 GUI を立て、全ケースを撃つ。$1 = 言語（en / ja / 空 = 未設定）、$2 = 置き場の名前
run_lang() {
  local lang="$1" label="$2" base="$TMP/$2" preview mcp first
  DIR="$base/out"
  # 2 回立てるので dir とソケットを分ける（discovery / tmux を共有すると 2 回目が前の器を拾う = #2157）
  TMUX_SOCKET="tako-1762-$$-$label"
  export HOME="$base/home"
  export TAKO_ISOLATED=1
  export TAKO_DATA_DIR="$base/data"
  export TAKO_DISCOVERY_DIR="$base/disc"
  export TAKO_ORCHESTRATOR_DIR="$base/orch"
  export TAKO_SESSIONS_FILE="$base/sessions.yaml"
  export TAKO_PANE_LOG_DIR="$base/panelogs"
  export TAKO_WORKERS_FILE="$base/workers.yaml"
  export TAKO_TMUX_SOCKET="$TMUX_SOCKET"
  mkdir -p "$HOME" "$TAKO_DATA_DIR" "$TAKO_DISCOVERY_DIR" "$TAKO_ORCHESTRATOR_DIR" "$DIR"
  case "$TAKO_DATA_DIR" in
    "$TMP"/*) : ;;
    *) echo "隔離されていないディレクトリ: $TAKO_DATA_DIR"; exit 1 ;;
  esac
  write_language "$lang"
  echo "== 表示言語 ${lang:-未設定}（${label}） =="
  # 面を用意できないときは起動せずに 4 が返る（#1744）。そのときは止めて「未実測」にする
  launch_isolated_gui "$base/app.log" || exit $?
  APP_PID="$ISOLATED_GUI_PID"
  if ! wait_isolated_gui "$base/app.log"; then
    echo "隔離 GUI へ CLI が繋がらない"
    exit 1
  fi
  printf '# 1762\n' > "$base/note.md"

  cli_case invalid theme color accent '#zz'
  cli_case pane scroll --pane 999999 --to 0
  cli_case tab foreground --tab 999999
  "$TAKO_BIN" list > "$DIR/list-before.json" 2>&1
  first="$(python3 -c 'import json,sys
print(json.load(open(sys.argv[1]))["tabs"][0]["panes"][0]["id"])' "$DIR/list-before.json" 2>/dev/null)"
  # #2358: tako-core の断り。タブ 1 つ・ペイン 1 つ・ウィンドウ 1 つのうちに撃つ（どれも断られて何も変わらない）
  "$TAKO_BIN" window list > "$DIR/windows.json" 2>&1
  window="$(python3 -c 'import json,sys
print(json.load(open(sys.argv[1]))["windows"][0]["id"])' "$DIR/windows.json" 2>/dev/null)"
  cli_case noresize resize --pane "${first:-0}" --dx 0.1
  cli_case lasttab close --pane "${first:-0}"
  cli_case ontoself tab move-pane --pane "${first:-0}" --target "${first:-0}"
  cli_case lastwindow window close "${window:-0}"
  # #2358: ペイン未指定（--pane も TAKO_PANE_ID も無い。TAKO_PANE_ID は冒頭で落としてある）
  cli_case notarget scroll --to 0
  mcp="$(mcp_call tako_resize_pane "{\"pane\":${first:-0},\"axis\":\"x\",\"delta\":0.1}" "$DIR/noresize.mcp")"
  echo "$mcp" > "$DIR/noresize.mcperr"
  mcp="$(mcp_call tako_close_pane "{\"pane\":${first:-0}}" "$DIR/lasttab.mcp")"
  echo "$mcp" > "$DIR/lasttab.mcperr"
  mcp="$(mcp_call tako_move_pane_to_tab "{\"pane\":${first:-0},\"target\":${first:-0}}" "$DIR/ontoself.mcp")"
  echo "$mcp" > "$DIR/ontoself.mcperr"
  mcp="$(mcp_call tako_window "{\"action\":\"close\",\"window\":${window:-0}}" "$DIR/lastwindow.mcp")"
  echo "$mcp" > "$DIR/lastwindow.mcperr"
  # MCP のペイン未指定（`pane` も呼び出し元も無い = このシェルは tako のペインではない）
  mcp="$(mcp_call tako_scroll_pane '{"to":0}' "$DIR/notarget.mcp")"
  echo "$mcp" > "$DIR/notarget.mcperr"
  # セッションの無いペイン = プレビュー。開く前のペインの集合との差で新しいペインを取る
  "$TAKO_BIN" open "$base/note.md" --pane "${first:-0}" --right > "$DIR/open.json" 2>&1
  "$TAKO_BIN" list > "$DIR/list-after.json" 2>&1
  preview="$(python3 -c 'import json,sys
ids = lambda p: {q["id"] for t in json.load(open(p))["tabs"] for q in t["panes"]}
try:
    new = sorted(ids(sys.argv[2]) - ids(sys.argv[1]))
except Exception:
    new = []
if new:
    print(new[0])' "$DIR/list-before.json" "$DIR/list-after.json")"
  if [ -n "$preview" ]; then
    cli_case nosession scroll --pane "$preview" --to 0
  else
    echo "プレビューのペインを開けない: $(head -c 200 "$DIR/open.json")" > "$DIR/nosession.cli"
    echo "?" > "$DIR/nosession.rc"
  fi
  # ③ GUI を経由しない直呼び（config.yaml だけの操作）
  cli_case local orchestrator layout --master-ratio 7

  mcp="$(mcp_call tako_theme '{"action":"set-color","key":"accent","value":"#zz"}' "$DIR/invalid.mcp")"
  echo "$mcp" > "$DIR/invalid.mcperr"
  mcp="$(mcp_call tako_scroll_pane '{"pane":999999,"to":0}' "$DIR/pane.mcp")"
  echo "$mcp" > "$DIR/pane.mcperr"
  mcp="$(mcp_call tako_foreground_pane '{"tab":999999}' "$DIR/tab.mcp")"
  echo "$mcp" > "$DIR/tab.mcperr"
  if [ -n "$preview" ]; then
    mcp="$(mcp_call tako_scroll_pane "{\"pane\":${preview},\"to\":0}" "$DIR/nosession.mcp")"
    echo "$mcp" > "$DIR/nosession.mcperr"
  fi

  stop_isolated_gui "$APP_PID"
  APP_PID=""
  tmux -L "$TMUX_SOCKET" kill-server >/dev/null 2>&1
  TMUX_SOCKET=""
  if [ -n "$KEEP" ]; then
    mkdir -p "$KEEP/$label"
    command cp -f "$DIR"/* "$KEEP/$label/"
  fi
}

# 出力を見せて、英語なら日本語が混ざらないこと・日本語なら日本語の前置きであることを確かめる
judge() {
  local label="$1" expect="$2" f name kind ch
  DIR="$TMP/$label/out"
  for f in "$DIR"/invalid.cli "$DIR"/pane.cli "$DIR"/tab.cli "$DIR"/nosession.cli \
    "$DIR"/local.cli "$DIR"/invalid.mcp "$DIR"/pane.mcp "$DIR"/tab.mcp "$DIR"/nosession.mcp \
    "$DIR"/noresize.cli "$DIR"/lasttab.cli "$DIR"/ontoself.cli "$DIR"/lastwindow.cli \
    "$DIR"/notarget.cli "$DIR"/noresize.mcp "$DIR"/lasttab.mcp "$DIR"/ontoself.mcp \
    "$DIR"/lastwindow.mcp "$DIR"/notarget.mcp; do
    name="$(basename "$f")"
    [ -f "$f" ] || { fail "$label $name が無い"; continue; }
    echo "      $label $name: $(head -c 240 "$f" | tr '\n' ' ')"
    case "$name" in
      *.mcp) kind="$(cat "${f%.mcp}.mcperr" 2>/dev/null)" ;;
      *) kind="rc=$(cat "${f%.cli}.rc" 2>/dev/null)" ;;
    esac
    case "$kind" in
      is_error=True | rc=1) : ;;
      *) fail "$label $name が断りで返っていない（${kind}）"; continue ;;
    esac
    if [ "$expect" = en ]; then
      if [ "$name" = local.cli ]; then
        # 直呼びの理由の文は日本語だけのもの（#1762 の外）。見るのは前置きだけ
        case "$(cat "$f")" in
          "error: invalid parameter: "*) pass "$label $name の前置きは英語" ;;
          *) fail "$label $name の前置きが英語でない" ;;
        esac
      elif ch="$(has_japanese "$f")"; then
        fail "$label $name に日本語が混ざる（${ch}）"
      else
        pass "$label $name に日本語が混ざらない"
      fi
    else
      case "$(cat "$f")" in
        *"無効なパラメータ: "* | *"が見つからない"* | *"ターミナルセッションがない"* \
          | *"リサイズできる分割がない"* | *"は閉じられない"* | *"自分自身の隣へは移動できない"* \
          | *"対象ペインを特定できない"*)
          pass "$label $name は日本語の前置き" ;;
        *) fail "$label $name に日本語の前置きが無い" ;;
      esac
    fi
  done
}

run_lang en en
run_lang ja ja
# 表示言語の設定が無い（system）。TAKO_LANG で英語へ解決させる = 既存の解決（i18n::detect_os_lang）を通る
export TAKO_LANG=en
run_lang "" unset-en
unset TAKO_LANG

echo "== ① 英語表示 =="
judge en en
echo "== ② 日本語表示 =="
judge ja ja
echo "== 表示言語が未設定（TAKO_LANG=en で解決） =="
judge unset-en en

echo
echo "結果: PASS=$PASS FAIL=$FAIL"
[ "$FAIL" -eq 0 ]
