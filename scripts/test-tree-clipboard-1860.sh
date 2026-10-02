#!/bin/bash
# test-tree-clipboard-1860.sh — ファイルツリーのコピー / 切り取り / 貼り付け（FR-3.33 / #1860）の実経路テスト
#
# 何を確かめるか（Issue #1860 の受け入れのうち、GUI・CLI・MCP・OS のクリップボードを通して初めて言えるもの）:
#   ① visual-test `tree-clipboard` 節: 隔離 GUI（tako-vd）で**実マウス・実キー**の入口から
#      ⌘C → ⌘V / ⌘X → ⌘V / 右クリックの「コピー」「貼り付け」/ 自分の配下・自分の行 /
#      別プロセス（AppKit）との往復 / 選択の外れ方。OS のクリップボードは名前付きペーストボード
#   ② A/B: `TAKO_1860_LEGACY=1`（ツリーがキーを受けない）の同じバイナリでは ① が FAILED になる
#   ③ 実 CLI と MCP `tako_file_op` が同じ操作をし、応答が**字面まで一致**する
#      （パスだけを置き換えて比べる）。断る場合（配下・自分自身・コピー元が無い・空の貼り付け）も一致
#   ④ **一般のペーストボード**（Finder が読み書きする場所）での往復: `tako file clipboard copy` を
#      Finder と同じ API（NSURL のファイル URL）で読める・Finder と同じ形で置いたファイルを
#      `tako file paste` が貼れる。**ユーザーのクリップボードは前後で保存・復元**し、途中でユーザーが
#      何かをコピーしていたら（変更番号が進んでいたら）上書きしない
#   ⑤ エッジ: 入れ子・リンク・権限（stat の比較）・空のフォルダ・同名が 3 つ以上・読めないファイルを
#      含むフォルダ（断って何も残さない）・切り取ったあと元を外で消した・別のボリューム
#      （コピーは通り、切り取りの貼り付けは理由つきで断る）
#
# 使い方: bash scripts/test-tree-clipboard-1860.sh
#
# **本番の tako / 設定・本物のファイルには一切触らない**（data / HOME / fixture は mktemp 配下、
# tmux は専用ソケット、落とすのは自分で起こした pid だけ）。窓は仮想ディスプレイ tako-vd へ出す
# （`scripts/lib/isolated-gui.sh` の 1 実装）。面を用意できなければ起動せず終了コード 4 で
# 「未実測」を返す。**CI には載せない**（実 GUI と AppKit が要る）。
set -uo pipefail

unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_ORCHESTRATOR_ROLE TAKO_MCP_URL \
  TAKO_FILE_PASTEBOARD

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT" || exit 1
PASS=0
FAIL=0
pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
fail() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }
check_eq() {
  if [ "$2" = "$3" ]; then pass "$1"; else fail "$1（期待 '${2}' / 実際 '${3}'）"; fi
}

TMP="$(mktemp -d "${TMPDIR:-/tmp}/tako-1860-XXXXXX")"
TMP="$(cd "$TMP" && pwd -P)"
APP_PID=""
TMUX_SOCKET="tako-1860-$$"
PB_NAME="tako-1860-$$"
MNT=""
# ④ で一般のペーストボードを触ったら 1（復元の要否）
GENERAL_SAVED=0
GENERAL_LAST=""

# AppKit のペーストボードを別プロセス（osascript = Finder と同じ API）から読み書きする
pb_js() {
  /usr/bin/osascript -l JavaScript -e "$1" 2>"$TMP/osascript.err"
}
# 名前付き（$1 = 名前）または一般（$1 = 空）のペーストボードを JS で引く式
PB_EXPR='(function(){ var env = $.NSProcessInfo.processInfo.environment; var n = env.objectForKey("PB"); return (n && !n.isNil() && n.js !== "") ? $.NSPasteboard.pasteboardWithName(n) : $.NSPasteboard.generalPasteboard; })()'
pb_read() {
  PB="$1" pb_js "ObjC.import('AppKit'); var pb = ${PB_EXPR};
    var opts = \$.NSDictionary.dictionaryWithObjectForKey(\$.NSNumber.numberWithBool(true), \$('NSPasteboardURLReadingFileURLsOnlyKey'));
    var urls = pb.readObjectsForClassesOptions(\$.NSArray.arrayWithObject(\$.NSURL), opts);
    var u = []; if (urls && !urls.isNil()) { for (var i = 0; i < urls.count; i++) u.push(urls.objectAtIndex(i).path.js); }
    var text = pb.stringForType(\$('public.utf8-plain-text'));
    JSON.stringify({count: pb.changeCount, urls: u, text: (text && !text.isNil()) ? text.js : null})"
}
pb_write_url() {
  PB="$1" P="$2" pb_js "ObjC.import('AppKit'); var pb = ${PB_EXPR};
    var env = \$.NSProcessInfo.processInfo.environment;
    pb.clearContents;
    pb.writeObjects(\$.NSArray.arrayWithObject(\$.NSURL.fileURLWithPath(env.objectForKey('P'))));
    JSON.stringify({count: pb.changeCount})"
}
pb_release() {
  PB="$1" pb_js "ObjC.import('AppKit'); var pb = ${PB_EXPR}; pb.releaseGlobally; '{}'" >/dev/null
}
# 一般のペーストボードの全項目・全型を $1（JSON）へ保存する
general_save() {
  OUT="$1" pb_js 'ObjC.import("AppKit"); var pb = $.NSPasteboard.generalPasteboard;
    var items = []; var its = pb.pasteboardItems;
    for (var i = 0; i < its.count; i++) { var it = its.objectAtIndex(i); var types = it.types; var d = {};
      for (var j = 0; j < types.count; j++) { var t = types.objectAtIndex(j).js; var data = it.dataForType($(t));
        if (data && !data.isNil()) { d[t] = data.base64EncodedStringWithOptions(0).js; } }
      items.push(d); }
    var json = $(JSON.stringify({count: pb.changeCount, items: items}));
    json.writeToFileAtomicallyEncodingError($.NSProcessInfo.processInfo.environment.objectForKey("OUT"), true, $.NSUTF8StringEncoding, null);
    String(pb.changeCount)'
}
# 保存した中身を戻す（$2 = 最後に自分が見た変更番号。進んでいたら = ユーザーがコピーした → 戻さない）
general_restore() {
  IN="$1" LAST="$2" pb_js 'ObjC.import("AppKit"); var pb = $.NSPasteboard.generalPasteboard;
    var env = $.NSProcessInfo.processInfo.environment;
    if (String(pb.changeCount) !== env.objectForKey("LAST").js) { "skipped:" + pb.changeCount; } else {
    var text = $.NSString.stringWithContentsOfFileEncodingError(env.objectForKey("IN"), $.NSUTF8StringEncoding, null).js;
    var saved = JSON.parse(text); var objs = $.NSMutableArray.array;
    saved.items.forEach(function (d) { var it = $.NSPasteboardItem.alloc.init;
      Object.keys(d).forEach(function (t) { it.setDataForType($.NSData.alloc.initWithBase64EncodedStringOptions($(d[t]), 0), $(t)); });
      objs.addObject(it); });
    pb.clearContents; if (saved.items.length > 0) { pb.writeObjects(objs); } "restored:" + saved.items.length; }'
}

cleanup() {
  stop_isolated_gui "$APP_PID"
  if [ "$GENERAL_SAVED" = "1" ]; then
    echo "  一般のペーストボードを戻す: $(general_restore "$TMP/general.json" "$GENERAL_LAST")"
  fi
  pb_release "$PB_NAME"
  tmux -L "$TMUX_SOCKET" kill-server >/dev/null 2>&1 || true
  if [ -n "$MNT" ]; then
    hdiutil detach "$MNT" -quiet -force >/dev/null 2>&1 || true
  fi
  chmod -R u+rwx "$TMP" 2>/dev/null || true
  rm -rf "$TMP"
}
trap cleanup EXIT

cargo build -q -p tako-app --features visual-test || exit 1
cargo build -q -p tako-cli -p tako-control --bin tako || exit 1

# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
isolated_gui_bins || exit 1

mkdir -p "$TMP/home" "$TMP/zdot" "$TMP/disc" "$TMP/orch"
# 証拠ログに実ユーザー名・実ホスト名を写さない（#927）
printf "PROMPT='tako %%1~ %%%% '\nRPROMPT=''\n" > "$TMP/zdot/.zshrc"
DUMP="${TAKO_1860_DUMP_DIR:-$TMP/dump}"
mkdir -p "$DUMP"

common_env=(
  HOME="$TMP/home" ZDOTDIR="$TMP/zdot" TAKO_LANG=ja
  TAKO_PERSIST=0 TAKO_AUTORENAME=0
  TAKO_DISCOVERY_DIR="$TMP/disc" TAKO_ORCHESTRATOR_DIR="$TMP/orch"
  TAKO_SESSIONS_FILE="$TMP/sessions.yaml" TAKO_PANE_LOG_DIR="$TMP/panelogs"
  TAKO_WORKERS_FILE="$TMP/workers.yaml" TAKO_TMUX_SOCKET="$TMUX_SOCKET"
)

# visual-test の節を 1 回走らせる（$1 = ログ / $2 = data dir / 残りは追加の env）
run_visual() {
  local log="$1" data="$2"
  shift 2
  ISOLATED_GUI_BOUNDS=${ISOLATED_GUI_BOUNDS:-0,0,1400,900}
  launch_isolated_gui "$log" ${common_env[@]+"${common_env[@]}"} \
    TAKO_DATA_DIR="$data" TAKO_FILE_PASTEBOARD="$PB_NAME" \
    TAKO_VISUAL_TEST=1 TAKO_VISUAL_ONLY=tree-clipboard TAKO_VISUAL_DUMP_DIR="$DUMP" \
    ${1+"$@"} || exit $?
  APP_PID="$ISOLATED_GUI_PID"
  wait "$APP_PID"
  local rc=$?
  APP_PID=""
  ISOLATED_GUI_PID=""
  return $rc
}

GENERAL_BEFORE="$(pb_read "" | jq -r '.count')"

echo "== ① visual-test tree-clipboard（実マウス・実キー・名前付きペーストボード） =="
run_visual "$TMP/visual.log" "$TMP/data-visual"
RC=$?
check_eq "節が終了コード 0 で終わる" "0" "$RC"
if grep -q 'TAKO_VISUAL_TEST_OK' "$TMP/visual.log"; then
  pass "TAKO_VISUAL_TEST_OK"
else
  fail "OK 行が出ない"
  grep 'TAKO_APP_SELF_TEST_FAILED' "$TMP/visual.log" | sed 's/^/    /'
fi
grep 'TAKO_VISUAL_PIXEL: tree-clipboard' "$TMP/visual.log" | sed -e "s#${TMP}#<TMP>#g" -e 's/^/  観測: /'
check_eq "① は一般のペーストボード（ユーザーのクリップボード）を書き換えない" \
  "$GENERAL_BEFORE" "$(pb_read "" | jq -r '.count')"

echo
echo "== ② A/B: TAKO_1860_LEGACY=1 でツリーがキーを受けず FAILED =="
run_visual "$TMP/legacy.log" "$TMP/data-legacy" TAKO_1860_LEGACY=1
RC=$?
if [ "$RC" -ne 0 ]; then
  pass "LEGACY の節は非ゼロで終わる（終了コード ${RC}）"
else
  fail "LEGACY でも節が通った（検出力が無い）"
fi
grep 'TAKO_APP_SELF_TEST_FAILED' "$TMP/legacy.log" | sed 's/^/  観測: /'
grep -q 'TAKO_APP_SELF_TEST_FAILED: visual-test tree-clipboard ①: 行を押すと選ばれる' \
  "$TMP/legacy.log" \
  && pass "落ちた理由はツリーの行が ⌘C / ⌘V の宛先にならないこと" \
  || fail "LEGACY の落ち方が想定と違う"

echo
echo "== ③ 実 GUI + 実 CLI + MCP（名前付きペーストボード） =="
export TAKO_ISOLATED=1 HOME="$TMP/home" TAKO_DISCOVERY_DIR="$TMP/disc" \
  TAKO_ORCHESTRATOR_DIR="$TMP/orch" TAKO_SESSIONS_FILE="$TMP/sessions.yaml" \
  TAKO_PANE_LOG_DIR="$TMP/panelogs" TAKO_WORKERS_FILE="$TMP/workers.yaml" \
  TAKO_TMUX_SOCKET="$TMUX_SOCKET"
export TAKO_DATA_DIR="$TMP/data-gui"
mkdir -p "$TAKO_DATA_DIR"
for d in "$HOME" "$TAKO_DATA_DIR"; do
  case "$d" in
    "$TMP"/*) : ;;
    *) echo "隔離されていないディレクトリ: $d"; exit 1 ;;
  esac
done
launch_isolated_gui "$TMP/gui.log" ${common_env[@]+"${common_env[@]}"} \
  TAKO_DATA_DIR="$TAKO_DATA_DIR" TAKO_FILE_PASTEBOARD="$PB_NAME" || exit $?
APP_PID="$ISOLATED_GUI_PID"
wait_isolated_gui "$TMP/gui.log" || exit 1

# 同じ形の fixture を 2 つ（A = CLI / B = MCP）。**必ず $TMP の中**
make_fixture() {
  local dir="$1"
  case "$dir" in "$TMP"/*) : ;; *) echo "fixture が一時 dir の外: $dir"; exit 1 ;; esac
  mkdir -p "$dir/src" "$dir/folder/inner" "$dir/dst" "$dir/日本 語"
  printf 'A\n' > "$dir/src/a.txt"
  printf '# b\n' > "$dir/src/b c.md"
  printf 'X\n' > "$dir/folder/inner/x.txt"
}
A="$TMP/cli-ws"
B="$TMP/mcp-ws"
make_fixture "$A"
make_fixture "$B"

normalize() {
  local ws="$1" real
  real="$(cd "$ws" && pwd -P)"
  jq -S -c --arg ws "$ws" --arg real "$real" '
    def sw: if type == "string" then (split($real) | join("<WS>") | split($ws) | join("<WS>")) else . end;
    walk(sw) | if type == "object" and has("pasted") then .pasted |= map(if has("followed") then .followed |= map(.pane = "<PANE>") else . end) else . end'
}
normalize_text() {
  local ws="$1" real
  real="$(cd "$ws" && pwd -P)"
  sed -e "s#${real}#<WS>#g" -e "s#${ws}#<WS>#g" -e 's/^error: //'
}

# 同じ操作列（op|path|dest。dest が空なら付けない）
OPS=(
  "copy|src/a.txt|dst"
  "copy|src/a.txt|src"
  "copy|folder|dst"
  "copy|src/b c.md|日本 語"
  "copy|folder|folder/inner"
  "copy|folder|folder"
  "copy|nope.txt|dst"
  "clipboard_copy|src/a.txt|"
  "clipboard|日本 語|"
  "paste|日本 語|"
  "clipboard_cut|folder|"
  "paste|日本 語|"
  "paste|日本 語|"
)
cli_args() {
  local op="$1" path="$2" dest="$3" ws="$4"
  case "$op" in
    copy) echo "file|copy|$ws/$path|$ws/$dest" ;;
    clipboard_copy) echo "file|clipboard|copy|$ws/$path" ;;
    clipboard_cut) echo "file|clipboard|cut|$ws/$path" ;;
    clipboard) echo "file|clipboard|show|$ws/$path" ;;
    paste) echo "file|paste|$ws/$path" ;;
  esac
}

i=0
for spec in ${OPS[@]+"${OPS[@]}"}; do
  IFS='|' read -r op path dest <<EOF
$spec
EOF
  i=$((i + 1))
  IFS='|' read -r -a args <<EOF
$(cli_args "$op" "$path" "$dest" "$A")
EOF
  if out="$("$TAKO_BIN" ${args[@]+"${args[@]}"} 2>"$TMP/cli-$i.err")"; then
    printf '%s' "$out" | normalize "$A" > "$TMP/cli-$i.norm"
  else
    normalize_text "$A" < "$TMP/cli-$i.err" | tr -d '\n' > "$TMP/cli-$i.norm"
  fi
done

SOCK="$TAKO_DATA_DIR/tako.sock"
[ -f "$TAKO_DATA_DIR/tako.sock.path" ] && SOCK="$(cat "$TAKO_DATA_DIR/tako.sock.path")"
{
  printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t","version":"1"}}}'
  printf '%s\n' '{"jsonrpc":"2.0","method":"notifications/initialized"}'
  i=0
  for spec in ${OPS[@]+"${OPS[@]}"}; do
    IFS='|' read -r op path dest <<EOF
$spec
EOF
    i=$((i + 1))
    if [ -n "$dest" ]; then
      jq -n -c --argjson id "$((i + 1))" --arg op "$op" --arg path "$B/$path" --arg dest "$B/$dest" \
        '{jsonrpc:"2.0",id:$id,method:"tools/call",params:{name:"tako_file_op",arguments:{op:$op,path:$path,dest:$dest}}}'
    else
      jq -n -c --argjson id "$((i + 1))" --arg op "$op" --arg path "$B/$path" \
        '{jsonrpc:"2.0",id:$id,method:"tools/call",params:{name:"tako_file_op",arguments:{op:$op,path:$path}}}'
    fi
    # 順に処理させる（応答を待ってから次 = CLI と同じ並び。コピーは background で写る）
    sleep 0.5
  done
  jq -n -c --arg path "$B/src/a.txt" \
    '{jsonrpc:"2.0",id:99,method:"tools/call",params:{name:"tako_file_op",arguments:{op:"copy",path:$path}}}'
  sleep 2
} | TAKO_SOCKET="$SOCK" TAKO_TOKEN="$(cat "$TAKO_DATA_DIR/token" 2>/dev/null)" \
    "$TAKO_BIN" mcp serve 2>/dev/null > "$TMP/mcp.out"
i=0
for spec in ${OPS[@]+"${OPS[@]}"}; do
  i=$((i + 1))
  jq -c --argjson id "$((i + 1))" 'select(.id == $id) | .result' "$TMP/mcp.out" > "$TMP/mcp-$i.raw"
  if [ "$(jq -r '.isError // false' "$TMP/mcp-$i.raw")" = "true" ]; then
    jq -r '.content[0].text' "$TMP/mcp-$i.raw" | normalize_text "$B" | tr -d '\n' > "$TMP/mcp-$i.norm"
  else
    jq -r '.content[0].text' "$TMP/mcp-$i.raw" | normalize "$B" > "$TMP/mcp-$i.norm"
  fi
done

echo "  --- CLI と MCP の応答（正規化後の字面） ---"
i=0
for spec in ${OPS[@]+"${OPS[@]}"}; do
  i=$((i + 1))
  echo "  [$i] ${spec}"
  echo "      CLI: $(cat "$TMP/cli-$i.norm")"
  echo "      MCP: $(cat "$TMP/mcp-$i.norm")"
  if [ -s "$TMP/cli-$i.norm" ] && cmp -s "$TMP/cli-$i.norm" "$TMP/mcp-$i.norm"; then
    pass "[$i] CLI と MCP の応答が字面まで一致"
  else
    fail "[$i] CLI と MCP の応答が食い違う"
  fi
done
check_eq "CLI [1] 別のフォルダは元の名前のまま" '<WS>/dst/a.txt' "$(jq -r '.to' "$TMP/cli-1.norm")"
check_eq "CLI [2] 同じフォルダは別名で置く" '<WS>/src/a のコピー.txt' "$(jq -r '.to' "$TMP/cli-2.norm")"
check_eq "コピー元は書き換わらない" "A" "$(cat "$A/src/a.txt")"
check_eq "CLI [3] フォルダは中身ごと（entries = folder / inner / x.txt）" "3" "$(jq -r '.entries' "$TMP/cli-3.norm")"
[ -f "$A/日本 語/b c.md" ] && pass "CLI [4] 日本語と空白を含むパスへ写せる" || fail "CLI [4] 日本語のフォルダへ写らない"
case "$(cat "$TMP/cli-5.norm")" in *配下*) pass "CLI [5] 自分の配下へは理由つきで断る" ;; *) fail "CLI [5] $(cat "$TMP/cli-5.norm")" ;; esac
case "$(cat "$TMP/cli-6.norm")" in *自分自身*) pass "CLI [6] 自分自身へは理由つきで断る" ;; *) fail "CLI [6] $(cat "$TMP/cli-6.norm")" ;; esac
[ ! -e "$A/folder/inner/folder" ] && [ ! -e "$A/folder/folder" ] && pass "断ったら何も作らない" || fail "断ったのに何かができた"
case "$(cat "$TMP/cli-7.norm")" in *コピー元が見つからない*) pass "CLI [7] コピー元が無いと理由つきで断る" ;; *) fail "CLI [7] $(cat "$TMP/cli-7.norm")" ;; esac
check_eq "CLI [8] clipboard_copy は OS へも書く（os = true）" "true" "$(jq -r '.os' "$TMP/cli-8.norm")"
check_eq "CLI [9] clipboard は貼り付け先を返し何も変えない" '<WS>/日本 語' "$(jq -r '.items[0].dest' "$TMP/cli-9.norm")"
check_eq "CLI [10] コピーを貼ると複製" "copy" "$(jq -r '.mode' "$TMP/cli-10.norm")"
[ -f "$A/日本 語/a.txt" ] && pass "CLI [10] 貼り付け先へ置かれる" || fail "CLI [10] 置かれていない"
check_eq "CLI [12] 切り取りを貼ると移動" "true" "$(jq -r '.pasted[0].moved' "$TMP/cli-12.norm")"
[ -f "$A/日本 語/folder/inner/x.txt" ] && [ ! -e "$A/folder" ] && pass "CLI [12] 移っている（元は残らない）" || fail "CLI [12] 移っていない"
case "$(cat "$TMP/cli-13.norm")" in *クリップボードに無い*) pass "CLI [13] 移し終えたら空 = 理由つきで断る" ;; *) fail "CLI [13] $(cat "$TMP/cli-13.norm")" ;; esac
MISSING="$(jq -r 'select(.id == 99) | .result.content[0].text' "$TMP/mcp.out")"
case "$MISSING" in *dest*) pass "MCP dest の省略は理由つきで断る" ;; *) fail "MCP dest の省略: $MISSING" ;; esac

echo
echo "== ⑤ エッジ（CLI。名前付きペーストボードのまま） =="
E="$TMP/edge"
mkdir -p "$E/tree/sub/deep" "$E/tree/empty" "$E/out" "$E/dup" "$E/bad/ok"
printf 'run\n' > "$E/tree/sub/deep/run.sh"
printf 'ro\n' > "$E/tree/ro.txt"
chmod 751 "$E/tree/sub/deep/run.sh"
chmod 444 "$E/tree/ro.txt"
ln -s ../ro.txt "$E/tree/sub/rel-link"
ln -s nowhere "$E/tree/broken"
ln -s .. "$E/tree/empty/up"
chmod 550 "$E/tree/sub"
"$TAKO_BIN" file copy "$E/tree" "$E/out" > "$TMP/edge-tree.json" 2>"$TMP/edge-tree.err"
listing() {
  (cd "$1" && find . -print0 | LC_ALL=C sort -z | while IFS= read -r -d '' p; do
    if [ -L "$p" ]; then printf '%s %s -> %s\n' "$(stat -f '%Sp' "$p")" "$p" "$(readlink "$p")"
    else printf '%s %s\n' "$(stat -f '%Sp' "$p")" "$p"; fi
  done)
}
listing "$E/tree" > "$TMP/edge-src.lst"
listing "$E/out/tree" > "$TMP/edge-dst.lst" 2>/dev/null
if cmp -s "$TMP/edge-src.lst" "$TMP/edge-dst.lst"; then
  pass "入れ子・リンク（相対 / 壊れ / 祖先）・権限（751 / 444 / 550）がそのまま写る（$(wc -l < "$TMP/edge-src.lst" | tr -d ' ') 項目）"
else
  fail "写した木が元と違う"; diff "$TMP/edge-src.lst" "$TMP/edge-dst.lst" | sed 's/^/    /'
fi
[ -d "$E/out/tree/empty" ] && pass "空のフォルダも写る" || fail "空のフォルダが写らない"
chmod 755 "$E/tree/sub" "$E/out/tree/sub" 2>/dev/null

printf 'd\n' > "$E/dup/a.txt"
for n in 1 2 3; do "$TAKO_BIN" file copy "$E/dup/a.txt" "$E/dup" >/dev/null 2>&1; done
check_eq "同名が 3 つ以上でも上書きせず次の番号へ" "a のコピー 2.txt|a のコピー 3.txt|a のコピー.txt|a.txt" \
  "$(ls "$E/dup" | LC_ALL=C sort | paste -sd '|' -)"

printf 'ok\n' > "$E/bad/ok/fine.txt"
printf 'secret\n' > "$E/bad/secret.txt"
chmod 000 "$E/bad/secret.txt"
BAD_ERR="$("$TAKO_BIN" file copy "$E/bad" "$E/out" 2>&1)"
BAD_RC=$?
echo "  観測: $(printf '%s' "$BAD_ERR" | sed "s#${TMP}#<TMP>#g")"
if [ "$BAD_RC" -ne 0 ] && printf '%s' "$BAD_ERR" | grep -q '読めない'; then
  pass "読めないファイルを含むフォルダは理由つきで断る"
else
  fail "読めないファイルの断り方が想定と違う（rc=${BAD_RC}）"
fi
[ ! -e "$E/out/bad" ] && pass "断ったら途中まで写したものも残さない" || fail "半端なコピーが残った"
chmod 644 "$E/bad/secret.txt"

printf 'gone\n' > "$E/gone.txt"
"$TAKO_BIN" file clipboard cut "$E/gone.txt" >/dev/null
rm -f "$E/gone.txt"
GONE_ERR="$("$TAKO_BIN" file paste "$E/out" 2>&1)"
GONE_RC=$?
echo "  観測: $(printf '%s' "$GONE_ERR" | sed "s#${TMP}#<TMP>#g")"
if [ "$GONE_RC" -ne 0 ] && printf '%s' "$GONE_ERR" | grep -q '見つからない'; then
  pass "切り取ったあと元を外で消したら理由つきで断る"
else
  fail "外で消したときの断り方が想定と違う（rc=${GONE_RC}）"
fi

DMG="$TMP/vol.dmg"
if hdiutil create -size 4m -fs HFS+ -volname tako1860 "$DMG" -quiet >/dev/null 2>&1 \
  && hdiutil attach "$DMG" -mountpoint "$TMP/vol" -nobrowse -quiet >/dev/null 2>&1; then
  MNT="$TMP/vol"
  printf 'v\n' > "$E/vol.txt"
  if "$TAKO_BIN" file copy "$E/vol.txt" "$MNT" >/dev/null 2>&1 && [ -f "$MNT/vol.txt" ]; then
    pass "別のボリュームへのコピーは通る"
  else
    fail "別のボリュームへコピーできない"
  fi
  "$TAKO_BIN" file clipboard cut "$E/vol.txt" >/dev/null
  rm -f "$MNT/vol.txt"
  XDEV_ERR="$("$TAKO_BIN" file paste "$MNT" 2>&1)"
  XDEV_RC=$?
  echo "  観測: $(printf '%s' "$XDEV_ERR" | sed "s#${TMP}#<TMP>#g")"
  if [ "$XDEV_RC" -ne 0 ] && printf '%s' "$XDEV_ERR" | grep -q '別のボリューム' \
    && [ -f "$E/vol.txt" ] && [ ! -e "$MNT/vol.txt" ]; then
    pass "別のボリュームへの切り取りの貼り付けは理由つきで断り、元は残る"
  else
    fail "別のボリュームの切り取りの断り方が想定と違う（rc=${XDEV_RC}）"
  fi
  hdiutil detach "$MNT" -quiet -force >/dev/null 2>&1 || true
  MNT=""
else
  echo "  未実測: hdiutil で一時イメージを作れない（別のボリュームの検査を飛ばす）"
fi

stop_isolated_gui "$APP_PID"
APP_PID=""

echo
echo "== ④ 一般のペーストボード（Finder が読み書きする場所）での往復 =="
general_save "$TMP/general.json" >/dev/null
GENERAL_SAVED=1
G="$TMP/general-ws"
mkdir -p "$G/src" "$G/dst" "$G/finder"
printf 'g\n' > "$G/src/g.txt"
printf 'f\n' > "$G/finder/from-finder.txt"
export TAKO_DATA_DIR="$TMP/data-general"
mkdir -p "$TAKO_DATA_DIR"
launch_isolated_gui "$TMP/gui-general.log" ${common_env[@]+"${common_env[@]}"} \
  TAKO_DATA_DIR="$TAKO_DATA_DIR" || exit $?
APP_PID="$ISOLATED_GUI_PID"
wait_isolated_gui "$TMP/gui-general.log" || exit 1
"$TAKO_BIN" file clipboard copy "$G/src/g.txt" > "$TMP/general-copy.json"
READ="$(pb_read "")"
GENERAL_LAST="$(printf '%s' "$READ" | jq -r '.count')"
echo "  観測: $(printf '%s' "$READ" | jq -c '{urls, text}' | sed "s#${TMP}#<TMP>#g")"
check_eq "tako の ⌘C（clipboard copy）を Finder と同じ API（NSURL のファイル URL）で読める" \
  "[\"$G/src/g.txt\"]" "$(printf '%s' "$READ" | jq -c '.urls')"
check_eq "テキストとしてはパスが載る（ターミナルへ貼るとパスが入る）" "$G/src/g.txt" "$(printf '%s' "$READ" | jq -r '.text')"
WROTE="$(pb_write_url "" "$G/finder/from-finder.txt")"
GENERAL_LAST="$(printf '%s' "$WROTE" | jq -r '.count')"
PASTE_OUT="$("$TAKO_BIN" file paste "$G/dst" 2>&1)"
echo "  観測: $(printf '%s' "$PASTE_OUT" | sed "s#${TMP}#<TMP>#g")"
check_eq "Finder と同じ形で置いたファイルを tako file paste が貼る（source = os）" "os" \
  "$(printf '%s' "$PASTE_OUT" | jq -r '.source' 2>/dev/null)"
[ -f "$G/dst/from-finder.txt" ] && [ -f "$G/finder/from-finder.txt" ] \
  && pass "Finder 由来は複製（元は残る）" || fail "Finder 由来が貼れていない"
stop_isolated_gui "$APP_PID"
APP_PID=""
echo "  一般のペーストボードを戻す: $(general_restore "$TMP/general.json" "$GENERAL_LAST")"
GENERAL_SAVED=0

echo
echo "== before（配布中の版 = この変更の前）の観測 =="
OLD="/Applications/tako.app/Contents/MacOS/tako"
if [ -x "$OLD" ]; then
  OLD_HELP="$(env -u TAKO_SOCKET "$OLD" file copy a b 2>&1 | head -1)"
  echo "  観測: 配布中の tako file copy → ${OLD_HELP}"
  case "$OLD_HELP" in *unrecognized*|*unexpected*|*error*) pass "before: 配布中の CLI には file copy が無い" ;; *) fail "before: 配布中の CLI に file copy がある？" ;; esac
else
  echo "  未実測: 配布中の tako が無い"
fi

echo
echo "結果: PASS=$PASS FAIL=$FAIL"
[ "$FAIL" -eq 0 ]
