#!/usr/bin/env bash
# test-external-change-1833.sh — 外部変更の続き（#1659 の後）の実経路テスト（#1833）
#
# 隔離した data / tmux で**実 tako-app** を立て、CLI / MCP から
#
#   ① 自動保存 ON で「外部変更 → 競合 → 読み直し → undo」: undo の応答に conflict が載り
#      （自動保存は止まり・知らせは 1 回）、待ってもディスクの外部変更を上書きしない
#      （修正前は undo の 500ms 後の自動保存が外部変更を黙って上書きした = Issue の本体）。
#      エッジ: undo をもう 1 回（読み直しの前の編集まで戻る）でも知らせ直さず書かない /
#      redo で読み直しをやり直すと解ける / もう一度戻して `save --overwrite`（--force の別名）で上書き
#   ② 自動保存 ON で「未編集のまま外部変更（監視が黙って追従）→ MCP の undo」も同じ
#   ③ エッジ: 自動保存 OFF なら既存の挙動のまま（conflict は出ず、明示の保存は通る）
#   ④ エッジ: 外部変更の無い undo では自動保存は止まらない（undo した中身が保存される）
#   ⑤ リモート由来（偽の ssh / sftp でローカルの dir を SSH 先に見立てる）の書き戻しの競合が、
#      ローカルと同じ conflict（remote: true）と notices に載り、ローカルの写しの監視では消えない。
#      `save --force` でリモートへ上書きでき、競合が解ける（修正前は conflict が載らなかった）
#   ⑥ A/B: `TAKO_1833_LEGACY=1`（#1833 前の挙動）の GUI では ① が外部変更を上書きする = 検出力がある
#
# **本番の tako / 設定には一切触らない**（data / HOME は mktemp 配下、tmux は専用ソケット、
# 落とすのは自分で起こした pid だけ。偽の ssh / sftp は一時の HOME のログインシェルの PATH にだけ置き、
# 実在の SSH 先へは繋がない）。窓は常設の仮想ディスプレイ（tako-vd）へ出る（#1141）。
# 帯の描画（見た目）はここでは見ない（応答と実ファイルの中身で見る）。
#
# 使い方: bash scripts/test-external-change-1833.sh
#   main など別のビルドで同じ検査を走らせるときは TAKO_BIN / APP_BIN でバイナリを差し替える
#   （その場合 ⑥ は飛ばす = 旧ビルドには A/B の口が無い）
#
# **CI には載せない**（実 GUI + 仮想ディスプレイが要る）。手元で走らせる前提の実経路テスト。
# 終了コード: 0 = 全部通った / 1 = どれか落ちた / 4 = 仮想ディスプレイを用意できず未実測
set -uo pipefail

# **本番 GUI を指す env を最初に落とす**（#1449 / #1450 で本番にペインが漏れた実例が 2 件）
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_ORCHESTRATOR_ROLE

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PASS=0
FAIL=0

pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
fail() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }
check_eq() {
  if [ "$2" = "$3" ]; then pass "$1"; else fail "$1（期待 '${2}' / 実際 '${3}'）"; fi
}

CUSTOM_BINS=0
if [ -n "${TAKO_BIN:-}" ] || [ -n "${APP_BIN:-}" ]; then
  CUSTOM_BINS=1
fi

TMP="$(mktemp -d /tmp/tako-1833-XXXXXX)"
APP_PID=""
TMUX_SOCKET="tako-1833-$$"
cleanup() {
  stop_isolated_gui "$APP_PID"
  stop_isolated_gui "${ISOLATED_GUI_PID:-}"
  tmux -L "$TMUX_SOCKET" kill-server >/dev/null 2>&1 || true
  rm -rf "${TMP:?}"
}
trap cleanup EXIT

# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
isolated_gui_bins visual-test || exit 1

# --- 隔離した環境 -------------------------------------------------------------
export HOME="$TMP/home"
mkdir -p "$HOME"
export TAKO_ISOLATED=1
export TAKO_DATA_DIR="$TMP/data"
export TAKO_DISCOVERY_DIR="$TMP/disc"
export TAKO_ORCHESTRATOR_DIR="$TMP/orch"
export TAKO_SESSIONS_FILE="$TMP/sessions.yaml"
export TAKO_PANE_LOG_DIR="$TMP/panelogs"
export TAKO_WORKERS_FILE="$TMP/workers.yaml"
export TAKO_TMUX_SOCKET="$TMUX_SOCKET"
mkdir -p "$TAKO_DATA_DIR" "$TAKO_DISCOVERY_DIR" "$TAKO_ORCHESTRATOR_DIR"
for d in "$HOME" "$TAKO_DATA_DIR" "$TAKO_ORCHESTRATOR_DIR"; do
  case "$d" in
    "$TMP"/*) : ;;
    *) echo "隔離されていないディレクトリ: $d"; exit 1 ;;
  esac
done

# --- 偽の ssh / sftp（⑤）-------------------------------------------------------
# tako は ssh / sftp をログインシェルの `command -v` で引く（`platform::exe::find`）。一時の HOME の
# .zprofile / .bash_profile で PATH の先頭へ置くので、この GUI の外の何にも効かない。
# sftp は `-b -` のバッチを読み、リモートのパスを同じ名前のローカルのパスとして扱う
# （本物と同じく各行を `sftp> <cmd>` で echo する）。ssh は ControlMaster の器を作ったふりをする
FAKEBIN="$TMP/fakebin"
mkdir -p "$FAKEBIN"
printf '#!/bin/sh\nexit 0\n' > "$FAKEBIN/ssh"
cat > "$FAKEBIN/sftp" <<'PY'
#!/usr/bin/env python3
import os, shlex, shutil, stat, sys, time

def ls_line(path):
    st = os.stat(path)
    mode = stat.filemode(st.st_mode)
    t = time.strftime("%b %d %H:%M", time.localtime(st.st_mtime))
    return f"{mode}    1 testuser testgroup {st.st_size:>8} {t} {path}"

ok = True
for raw in sys.stdin:
    line = raw.rstrip("\n")
    if not line or line == "bye":
        continue
    print(f"sftp> {line}", flush=True)
    soft = line.startswith("-")
    cmd = line[1:] if soft else line
    try:
        args = shlex.split(cmd)
        op = args[0]
        if op == "pwd":
            print("Remote working directory: /")
        elif op == "ls":
            target = args[-1].rstrip("/") or "/"
            if os.path.isdir(target):
                for name in sorted(os.listdir(target)):
                    print(ls_line(os.path.join(target, name)))
            else:
                print(ls_line(target))
        elif op in ("get", "put"):
            shutil.copyfile(args[1], args[2])
        elif op == "rename":
            os.replace(args[1], args[2])
        elif op == "chmod":
            os.chmod(args[2], int(args[1], 8))
        else:
            raise RuntimeError(f"unsupported: {op}")
    except Exception as e:
        print(f"Couldn't {cmd}: {e}", file=sys.stderr)
        if not soft:
            ok = False
            break
sys.exit(0 if ok else 1)
PY
chmod +x "$FAKEBIN/ssh" "$FAKEBIN/sftp"
printf 'export PATH="%s:$PATH"\n' "$FAKEBIN" > "$HOME/.zprofile"
printf 'export PATH="%s:$PATH"\n' "$FAKEBIN" > "$HOME/.bash_profile"

# 応答 JSON から 1 つの値を取る（`conflict.notices` のように辿れる）
jget() {
  python3 -c '
import json, sys
path = sys.argv[1].split(".")
try:
    cur = json.loads(sys.stdin.read())
except Exception:
    print("__NOT_JSON__"); sys.exit(0)
for key in path:
    if isinstance(cur, dict) and key in cur:
        cur = cur[key]
    else:
        print("__MISSING__"); sys.exit(0)
print(cur if isinstance(cur, str) else json.dumps(cur, ensure_ascii=False, sort_keys=True))
' "$1"
}

# `tako` を叩いて stdout と stderr をまとめて OUT へ、終了コードを RC へ置く。
# **`$( )` の中で呼ばない**（サブシェルで立てた RC は親へ戻らない）
OUT=""
RC=0
run() {
  OUT="$("$TAKO_BIN" "$@" 2>&1)"
  RC=$?
}

# ファイルを開いて編集を始め、ペイン ID を返す（自動保存は $2 = true / false）
open_edit() {
  local file="$1" autosave="$2" pane
  pane="$("$TAKO_BIN" open "$file" --pane "$ROOT_PANE" --new-tab 2>&1 | jget pane)"
  case "$pane" in
    ''|*[!0-9]*) echo "開けない: $file" >&2; return 1 ;;
  esac
  "$TAKO_BIN" edit start --pane "$pane" >/dev/null || return 1
  "$TAKO_BIN" edit autosave "$autosave" --pane "$pane" >/dev/null || return 1
  printf '%s' "$pane"
}

# 自動保存の 500ms と監視の取り込みを待つ。1 回の IPC ごとに自動保存の判定が回るので叩きながら待つ
settle() { # ペイン
  local i
  for i in 1 2 3 4; do
    sleep 0.7
    "$TAKO_BIN" edit status --pane "$1" >/dev/null 2>&1
  done
}

start_gui() { # ログ 追加の env…
  local log="$1" rc=0
  shift
  launch_isolated_gui "$log" "$@" || rc=$?
  if [ "$rc" -eq 4 ]; then
    echo "仮想ディスプレイを用意できないので未実測で止める（${ISOLATED_GUI_NO_DISPLAY_REASON:-理由不明}）"
    exit 4
  fi
  [ "$rc" -eq 0 ] || exit "$rc"
  wait_isolated_gui "$log" || exit 1
}

root_pane() {
  "$TAKO_BIN" list 2>/dev/null | python3 -c '
import json, sys
d = json.load(sys.stdin)
print(d["tabs"][0]["panes"][0]["id"])
'
}

mcp_call() { # ツール名 引数の JSON → 結果の本文（isError なら先頭に ERROR:）
  local sock="$TAKO_DATA_DIR/tako.sock"
  [ -f "$TAKO_DATA_DIR/tako.sock.path" ] && sock="$(cat "$TAKO_DATA_DIR/tako.sock.path")"
  printf '%s\n' \
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"t1833","version":"0"}}}' \
    "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"$1\",\"arguments\":$2}}" \
    | TAKO_SOCKET="$sock" TAKO_TOKEN="$(cat "$TAKO_DATA_DIR/token")" "$TAKO_BIN" mcp serve 2>/dev/null \
    | python3 -c '
import json, sys
for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    try:
        msg = json.loads(line)
    except Exception:
        continue
    if msg.get("id") != 2:
        continue
    if "error" in msg:
        print("ERROR:" + json.dumps(msg["error"], ensure_ascii=False)); break
    res = msg.get("result", {})
    text = res.get("content", [{}])[0].get("text", "")
    print(("ERROR:" if res.get("isError") else "") + text)
'
}

# ① の手順（⑥ の A/B でも同じものを流す）。ディスクに残った中身を DISK_AFTER へ置く
DISK_AFTER=""
UNDO_REPLY=""
reload_then_undo() { # ファイル
  local file="$1" pane
  printf 'a\nb\nc\n' > "$file"
  pane="$(open_edit "$file" true)" || exit 1
  # 打った直後（自動保存の 500ms より前）にディスク側を書き換える = 競合になる
  "$TAKO_BIN" edit replace-range 2:0 2:1 MINE --pane "$pane" >/dev/null
  printf 'a\nDISK\nc\n' > "$file"
  settle "$pane"
  "$TAKO_BIN" edit reload --pane "$pane" >/dev/null
  run edit undo --pane "$pane"
  UNDO_REPLY="$OUT"
  settle "$pane"
  DISK_AFTER="$(cat "$file")"
  P1="$pane"
}

echo "== 隔離 GUI を起こす =="
start_gui "$TMP/app.log"
APP_PID="$ISOLATED_GUI_PID"
echo "  pid=$APP_PID"
ROOT_PANE="$(root_pane)"

echo
echo "== ① 自動保存 ON: 外部変更 → 競合 → 読み直し → undo =="
F1="$TMP/one.rs"
reload_then_undo "$F1"
echo "  undo の応答: $UNDO_REPLY"
check_eq "undo の応答に conflict.state=changed" changed "$(printf '%s' "$UNDO_REPLY" | jget conflict.state)"
check_eq "undo の応答に conflict.notices=1" 1 "$(printf '%s' "$UNDO_REPLY" | jget conflict.notices)"
check_eq "undo の応答に conflict.autosave_paused=true" true "$(printf '%s' "$UNDO_REPLY" | jget conflict.autosave_paused)"
check_eq "undo の応答は dirty=true（自分の変更へ戻った）" true "$(printf '%s' "$UNDO_REPLY" | jget dirty)"
check_eq "待っても自動保存が外部変更を上書きしない" "$(printf 'a\nDISK\nc\n')" "$DISK_AFTER"
run edit diff --pane "$P1"
check_eq "diff の state も changed（基準と同じでも本文は外部変更より前の版）" changed "$(printf '%s' "$OUT" | jget diff.state)"
# エッジ: undo をもう 1 回（読み直しの前の編集まで戻る）
run edit undo --pane "$P1"
U2="$OUT"
settle "$P1"
check_eq "undo 2 回目でも知らせ直さない（notices=1）" 1 "$(printf '%s' "$U2" | jget conflict.notices)"
check_eq "undo 2 回目の後も外部変更を上書きしない" "$(printf 'a\nDISK\nc\n')" "$(cat "$F1")"
# redo 2 回で読み直しをやり直すと解ける
"$TAKO_BIN" edit redo --pane "$P1" >/dev/null
run edit redo --pane "$P1"
R2="$OUT"
check_eq "redo で読み直しをやり直すと conflict が消える" __MISSING__ "$(printf '%s' "$R2" | jget conflict)"
check_eq "redo の後は dirty=false" false "$(printf '%s' "$R2" | jget dirty)"
# もう一度戻して、MCP と同じ綴りの別名で上書きする
"$TAKO_BIN" edit undo --pane "$P1" >/dev/null
run edit save --overwrite --pane "$P1"
check_eq "save --overwrite（--force の別名）が通る" 0 "$RC"
check_eq "上書きで自分の変更がディスクへ残る" "$(printf 'a\nMINE\nc\n')" "$(cat "$F1")"
check_eq "上書きの後は conflict が消える" __MISSING__ "$(printf '%s' "$OUT" | jget conflict)"

echo
echo "== ② 自動保存 ON: 未編集のまま外部変更（監視が追従）→ MCP の undo =="
F2="$TMP/two.rs"
printf 'x\ny\n' > "$F2"
P2="$(open_edit "$F2" true)" || exit 1
printf 'x\nEXT\n' > "$F2"
settle "$P2"
run edit status --pane "$P2"
check_eq "監視がディスクへ追従した（dirty=false・conflict 無し）" "false/__MISSING__" \
  "$(printf '%s' "$OUT" | jget dirty)/$(printf '%s' "$OUT" | jget conflict)"
M_UNDO="$(mcp_call tako_preview_undo "{\"pane\":$P2}")"
echo "  MCP tako_preview_undo: $M_UNDO"
check_eq "MCP の undo の応答にも conflict.state=changed" changed "$(printf '%s' "$M_UNDO" | jget conflict.state)"
settle "$P2"
check_eq "追従を undo しても外部変更を上書きしない" "$(printf 'x\nEXT\n')" "$(cat "$F2")"

echo
echo "== ③ エッジ: 自動保存 OFF なら既存の挙動のまま =="
F3="$TMP/three.rs"
printf 'a\nb\nc\n' > "$F3"
P3="$(open_edit "$F3" false)" || exit 1
"$TAKO_BIN" edit replace-range 2:0 2:1 MINE --pane "$P3" >/dev/null
printf 'a\nDISK\nc\n' > "$F3"
"$TAKO_BIN" edit reload --pane "$P3" >/dev/null
run edit undo --pane "$P3"
check_eq "自動保存 OFF の undo では conflict を出さない" __MISSING__ "$(printf '%s' "$OUT" | jget conflict)"
settle "$P3"
check_eq "自動保存 OFF なら何も書かない" "$(printf 'a\nDISK\nc\n')" "$(cat "$F3")"
run edit save --pane "$P3"
check_eq "明示の保存は従来どおり通る" 0 "$RC"
check_eq "明示の保存で自分の変更が残る" "$(printf 'a\nMINE\nc\n')" "$(cat "$F3")"

echo
echo "== ④ エッジ: 外部変更の無い undo では自動保存は止まらない =="
F4="$TMP/four.rs"
printf 'p\nq\n' > "$F4"
P4="$(open_edit "$F4" true)" || exit 1
"$TAKO_BIN" edit replace-range 1:0 1:1 P --pane "$P4" >/dev/null
settle "$P4"
check_eq "打鍵は自動保存される" "$(printf 'P\nq\n')" "$(cat "$F4")"
run edit undo --pane "$P4"
check_eq "undo の応答に conflict は無い" __MISSING__ "$(printf '%s' "$OUT" | jget conflict)"
settle "$P4"
check_eq "undo した中身も自動保存される" "$(printf 'p\nq\n')" "$(cat "$F4")"

echo
echo "== ⑤ リモート由来のペインの書き戻しの競合（偽の ssh / sftp）=="
mkdir -p "$TMP/remote"
R="$TMP/remote/r.txt"
printf 'one\ntwo\n' > "$R"
run remote-folder open-file fakehost "$R"
P5="$(printf '%s' "$OUT" | jget pane)"
case "$P5" in
  ''|*[!0-9]*) fail "リモートのファイルを開けない: $OUT" ;;
  *)
    "$TAKO_BIN" edit start --pane "$P5" >/dev/null
    "$TAKO_BIN" edit replace-range 1:0 1:3 MINE --pane "$P5" >/dev/null
    printf 'one\nREMOTE\n' > "$R"
    run edit save --pane "$P5"
    echo "  save の応答: $OUT"
    check_eq "リモートが変わっていたら保存は断る（終了コード非 0）" 1 "$([ "$RC" -ne 0 ] && echo 1 || echo 0)"
    check_eq "リモートの中身は上書きしない" "$(printf 'one\nREMOTE\n')" "$(cat "$R")"
    settle "$P5"
    run edit status --pane "$P5"
    S5="$OUT"
    echo "  status: $(printf '%s' "$S5" | jget conflict)"
    check_eq "status に conflict.state=changed" changed "$(printf '%s' "$S5" | jget conflict.state)"
    check_eq "status に conflict.remote=true" true "$(printf '%s' "$S5" | jget conflict.remote)"
    check_eq "知らせは 1 回（ローカルの写しの監視でも消えない・増えない）" 1 "$(printf '%s' "$S5" | jget conflict.notices)"
    run edit save --pane "$P5"
    run edit status --pane "$P5"
    check_eq "保存をもう一度試しても知らせ直さない（notices=1）" 1 "$(printf '%s' "$OUT" | jget conflict.notices)"
    M_SAVE="$(mcp_call tako_preview_save "{\"pane\":$P5}")"
    case "$M_SAVE" in
      ERROR:*) pass "MCP の保存も断る" ;;
      *) fail "MCP の保存も断る（${M_SAVE}）" ;;
    esac
    run edit save --force --pane "$P5"
    echo "  save --force の応答: $OUT"
    check_eq "save --force でリモートへ上書きできる" "$(printf 'MINE\ntwo\n')" "$(cat "$R")"
    check_eq "上書きの後は conflict が消える" __MISSING__ "$(printf '%s' "$OUT" | jget conflict)"
    # ⑤-b もう一度ぶつけて、今度はリモートの中身を採る（取り直し = open-file）
    "$TAKO_BIN" edit replace-range 2:0 2:3 MINE2 --pane "$P5" >/dev/null
    printf 'MINE\nREMOTE2\n' > "$R"
    run edit save --pane "$P5"
    run edit status --pane "$P5"
    check_eq "2 回目の競合も conflict.remote=true" true "$(printf '%s' "$OUT" | jget conflict.remote)"
    run remote-folder open-file fakehost "$R"
    check_eq "取り直しは同じペインを使う" "$P5" "$(printf '%s' "$OUT" | jget pane)"
    settle "$P5"
    run edit status --pane "$P5"
    echo "  取り直した後の status: $OUT"
    # 未保存の変更が無いので開き直しは表示を差し替え、編集セッション（と競合）も閉じる
    check_eq "取り直してリモートの中身を採ると conflict が消える" __MISSING__ "$(printf '%s' "$OUT" | jget conflict)"
    "$TAKO_BIN" edit start --pane "$P5" >/dev/null
    run edit save --pane "$P5"
    echo "  取り直した後の save: rc=$RC"
    check_eq "取り直した後は編集を始め直して保存が通る（基準がリモートの今の中身）" 0 "$RC"
    check_eq "取り直した後もリモートの中身はそのまま" "$(printf 'MINE\nREMOTE2\n')" "$(cat "$R")"
    ;;
esac

stop_isolated_gui "$APP_PID"
APP_PID=""

if [ "$CUSTOM_BINS" -eq 1 ]; then
  echo
  echo "（差し替えたバイナリなので ⑥ A/B は飛ばす）"
else
  echo
  echo "== ⑥ A/B: TAKO_1833_LEGACY=1（#1833 前の挙動）では ① が外部変更を上書きする =="
  rm -rf "${TAKO_DATA_DIR:?}"
  mkdir -p "$TAKO_DATA_DIR"
  start_gui "$TMP/app-legacy.log" TAKO_1833_LEGACY=1
  APP_PID="$ISOLATED_GUI_PID"
  ROOT_PANE="$(root_pane)"
  reload_then_undo "$TMP/legacy.rs"
  echo "  legacy: undo の応答の conflict=$(printf '%s' "$UNDO_REPLY" | jget conflict) / ディスク=$(printf '%s' "$DISK_AFTER" | tr '\n' '|')"
  check_eq "legacy では外部変更が上書きされる（① の検査が #1833 前を検出できる）" \
    "$(printf 'a\nMINE\nc\n')" "$DISK_AFTER"
  stop_isolated_gui "$APP_PID"
  APP_PID=""
fi

echo
echo "== 結果 =="
echo "  PASS=$PASS FAIL=$FAIL"
[ "$FAIL" -eq 0 ]
