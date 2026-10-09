#!/bin/bash
# test-highlight-app-nap-1916.sh — 構文の塗りが App Nap で E コアへ寄せられない（#1916）の実経路テスト
#
# 何を確かめるか（隔離 GUI・release・tako-vd）:
#   ① テストスレッドで直接塗った所要（同じ 10 MB の入力。`perf_塗りの所要_テストスレッド`）
#   ② GUI を起こした直後（まだ間引かれていない）に開いて塗る
#   ③ 間引かれた（App Nap で優先度が落ちた）のを**状態で**待ってから開いて塗る × 3 回。
#      **塗りの間 GUI のプロセスが P コアで走る**ことを判定する（旧挙動は E コアへ寄せられる）
#   ④ 塗りの途中でペインを閉じる / 塗りの途中で別のファイルも開く（重なった塗り）→
#      次の塗りも P コアで走り、全部終わったら**また間引かれる**（依頼を握りっぱなしにしない）
#   ⑤ 小さいファイル: 塗りが戻って `highlighting` が消える
#
# 所要と「GUI ÷ テストスレッド」の比は出力するだけで判定に使わない（`.agent/conventions.md`
# 「効果を測る単体テストは実時間で比べない」）。判定は「P コアで走った比率」と状態だけ。
#
# A/B: `TAKO_1916_LEGACY=1 bash scripts/test-highlight-app-nap-1916.sh` で③と④の重なった 2 本が
# 名指しで FAILED（旧挙動 = 塗りの間も App Nap に間引かせたまま。④の「また間引かれる」は
# 旧では一度も戻らないので素通りする = 検出力は新の腕でだけ持つ）。
#
# E コアの無い機・間引かれない機（前面に出ている等）では③が測れないので「未実測」で
# 終了コード 4（#1744 と同じ番号）。**本番の tako / 設定には一切触らない**（data / HOME は
# mktemp 配下、落とすのは自分で起こした pid だけ）。窓は常設の仮想ディスプレイ（tako-vd）へ出る。
#
# 使い方: bash scripts/test-highlight-app-nap-1916.sh
set -uo pipefail

# **本番 GUI を指す env を最初に落とす**（#1449 / #1450 で本番にペインが漏れた実例が 2 件）
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_ORCHESTRATOR_ROLE

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PASS=0
FAIL=0
UNMEASURED=0
pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
# 「P コアで走った」とみなす比率の下限（判定はこの 1 か所の値で行う。実測は修正後 0.986〜1.000・
# 旧挙動で間引かれた後 0.000〜0.089 = どちらからも十分離れている）
P_CORE_MIN=0.5
on_p_cores() { python3 -c "import sys; sys.exit(0 if $1 >= $P_CORE_MIN else 1)"; }
fail() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }

TMP="$(mktemp -d /tmp/tako-1916-XXXXXX)"
cleanup() {
  stop_isolated_gui "${ISOLATED_GUI_PID:-}"
  tmux -L "tako-1916-$$" kill-server >/dev/null 2>&1 || true
  rm -rf "$TMP"
}
trap cleanup EXIT

if [ "$(uname -s)" != "Darwin" ] || [ "$(sysctl -n hw.nperflevels 2>/dev/null || echo 1)" -lt 2 ]; then
  echo "未実測: E コアのある macOS でだけ意味がある（App Nap と P / E コアの検査）"
  exit 4
fi

export TAKO_ISO_PROFILE=release
# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
echo "release の tako-app / tako とテストバイナリをビルドします…"
(cd "$REPO_ROOT" && cargo build --release -q -p tako-cli -p tako-app) || exit 1
isolated_gui_bins || exit 1
# テストスレッド側（①）のバイナリ。**HOME を差し替える前に**決める（後で cargo を呼ぶと
# 一時 HOME へツールチェーンとレジストリを丸ごと取り直す）
TEST_BIN="$(cd "$REPO_ROOT" && cargo test --release -q -p tako-app --no-run --message-format=json 2>/dev/null \
  | python3 -c '
import json, sys
for line in sys.stdin:
    try:
        m = json.loads(line)
    except Exception:
        continue
    if m.get("reason") == "compiler-artifact" and m.get("executable") and m["target"]["name"] == "tako-app":
        print(m["executable"])
' | tail -1)"
[ -x "$TEST_BIN" ] || { echo "テストバイナリが見つからない"; exit 1; }

export HOME="$TMP/home"
export TAKO_ISOLATED=1
export TAKO_DATA_DIR="$TMP/data"
export TAKO_DISCOVERY_DIR="$TMP/disc"
export TAKO_ORCHESTRATOR_DIR="$TMP/orch"
export TAKO_SESSIONS_FILE="$TMP/sessions.yaml"
export TAKO_PANE_LOG_DIR="$TMP/panelogs"
export TAKO_WORKERS_FILE="$TMP/workers.yaml"
export TAKO_TMUX_SOCKET="tako-1916-$$"
mkdir -p "$HOME" "$TAKO_DATA_DIR" "$TAKO_DISCOVERY_DIR" "$TAKO_ORCHESTRATOR_DIR" "$TMP/in"
for d in "$HOME" "$TAKO_DATA_DIR" "$TAKO_ORCHESTRATOR_DIR"; do
  case "$d" in
    "$TMP"/*) : ;;
    *) echo "隔離されていないディレクトリ: $d"; exit 1 ;;
  esac
done

# 10 MB の入力（visual-test 節 large-file-decor の LF と同じ作り = 1 行 100 バイト × 10 万行）と
# 小さいファイル
python3 - "$TMP/in" <<'PY'
import os, sys
out = sys.argv[1]
parts = []
for i in range(100_000):
    head = f"let value_{i} = {i}; // "
    parts.append(head + "~" * (99 - len(head)) + "\n")
text = "".join(parts)
assert len(text) == 10_000_000
for name in ("a.rs", "b.rs"):
    open(os.path.join(out, name), "w", newline="").write(text)
open(os.path.join(out, "small.rs"), "w").write("fn main() {\n    println!(\"hi\");\n}\n")
PY

# プロセスの資源の使い方（proc_pid_rusage v6）: user 時間 / うち P コア / 命令数
cat > "$TMP/ru.py" <<'PY'
import ctypes, sys
class RU(ctypes.Structure):
    _fields_ = [("uuid", ctypes.c_uint8 * 16), ("f", ctypes.c_uint64 * 47), ("r", ctypes.c_uint64 * 9)]
lib = ctypes.CDLL("/usr/lib/libSystem.B.dylib")
ru = RU()
if lib.proc_pid_rusage(int(sys.argv[1]), 6, ctypes.byref(ru)) != 0:
    sys.exit(1)
# user_time / user_ptime / instructions
print(ru.f[0], ru.f[36], ru.f[29])
PY
ru() { python3 "$TMP/ru.py" "$ISOLATED_GUI_PID"; }
# 2 つの ru の差から「P コアで走った比率」と命令数（G）を出す
ru_delta() { # 前 後
  python3 - "$1" "$2" <<'PY'
import sys
a = [int(x) for x in sys.argv[1].split()]
b = [int(x) for x in sys.argv[2].split()]
user, puser, instr = (b[i] - a[i] for i in range(3))
print(f"{puser / max(user, 1):.3f} {instr / 1e9:.1f}")
PY
}
now() { python3 -c 'import time; print(time.time())'; }
pri() { ps -o pri= -p "$ISOLATED_GUI_PID" 2>/dev/null | tr -d ' '; }

# ① テストスレッド
echo
echo "① テストスレッドで直接塗る（同じ 10 MB）"
BASE_MS="$(TAKO_1916_PERF_FILE="$TMP/in/a.rs" TAKO_1916_PERF_ROUNDS=2 "$TEST_BIN" --ignored --nocapture \
  --exact 'preview::tests::perf_塗りの所要_テストスレッド' 2>&1 \
  | sed -n 's/.*\[perf\] 1916 test-thread round=[0-9]* ms=\([0-9]*\).*/\1/p' | tee "$TMP/base.txt" | sort -n | head -1)"
echo "    テストスレッド: $(tr '\n' ' ' < "$TMP/base.txt")ms（比の分母は最小値 ${BASE_MS}ms）"
[ -n "$BASE_MS" ] || { echo "テストスレッドの所要を読めない"; exit 1; }

launch_isolated_gui "$TMP/app.log" ${TAKO_1916_LEGACY:+TAKO_1916_LEGACY=$TAKO_1916_LEGACY} || exit $?
wait_isolated_gui "$TMP/app.log" 600 || exit 1
SOCK_PATH="$TAKO_DATA_DIR/tako.sock"
[ -f "$TAKO_DATA_DIR/tako.sock.path" ] && SOCK_PATH="$(cat "$TAKO_DATA_DIR/tako.sock.path")"
export TAKO_SOCKET="$SOCK_PATH"
TAKO_TOKEN="$(cat "$TAKO_DATA_DIR/token")"
export TAKO_TOKEN
ROOT_PANE="$("$TAKO_BIN" list 2>/dev/null | python3 -c '
import json, sys
print(json.load(sys.stdin)["tabs"][0]["panes"][0]["id"])
')"
echo "    GUI pid=$ISOLATED_GUI_PID legacy=${TAKO_1916_LEGACY:-0}"

open_pane() { "$TAKO_BIN" open "$1" --pane "$ROOT_PANE" --new-tab 2>&1 | python3 -c 'import json,sys; print(json.load(sys.stdin)["pane"])'; }
highlighting() { # ペインの読み取り表示の塗りが走っているか（true / false）
  "$TAKO_BIN" edit status --pane "$1" 2>/dev/null | python3 -c '
import json, sys
print("true" if json.load(sys.stdin).get("highlighting") else "false")
' 2>/dev/null || echo "false"
}
# 塗りが走り始めるまで状態で待つ（上限 30 秒。開いた直後は次の描画で起きる）
wait_highlight_start() { # pane
  local i
  for i in $(seq 1 300); do
    [ "$(highlighting "$1")" = true ] && return 0
    sleep 0.1
  done
  return 1
}
# 塗りが戻るまで状態で待つ（上限 600 秒）
wait_highlight_end() { # pane
  local i
  for i in $(seq 1 6000); do
    [ "$(highlighting "$1")" = false ] && return 0
    sleep 0.1
  done
  return 1
}
# 間引かれる（優先度が 10 以下へ落ちる）のを状態で待つ（上限 300 秒）
wait_napped() {
  local i
  for i in $(seq 1 600); do
    p="$(pri)"
    [ -n "$p" ] && [ "$p" -le 10 ] && return 0
    sleep 0.5
  done
  return 1
}
# 1 回開いて塗る。結果を「所要 ms / P コアの比率 / 命令 G / 塗りの途中の優先度」で返す
measure_open() { # file
  local before after t0 t1 pane mid
  before="$(ru)"
  t0="$(now)"
  pane="$(open_pane "$1")"
  if ! wait_highlight_start "$pane"; then
    "$TAKO_BIN" close --pane "$pane" --force >/dev/null 2>&1
    echo "nostart"
    return
  fi
  sleep 1
  mid="$(pri)"
  wait_highlight_end "$pane"
  t1="$(now)"
  after="$(ru)"
  "$TAKO_BIN" close --pane "$pane" --force >/dev/null 2>&1
  echo "$(python3 -c "print(int(($t1 - $t0) * 1000))") $(ru_delta "$before" "$after") $mid"
}
report_round() { # ラベル 結果
  set -- "$1" $2
  local ratio
  ratio="$(python3 -c "print(f'{$2 / $BASE_MS:.2f}')")"
  echo "    $1: ${2}ms（テストスレッド比 ${ratio}）P コア比率 ${3}・命令 ${4}G・塗りの途中の優先度 ${5:-?}"
}

# ② 間引かれる前
echo
echo "② 起こした直後（間引かれる前）に開いて塗る（優先度 $(pri)）"
R="$(measure_open "$TMP/in/a.rs")"
if [ "$R" = nostart ]; then fail "② 塗りが走り始めない"; else report_round "間引かれる前" "$R"; fi

# ③ 間引かれた後 × 3
echo
echo "③ 間引かれた後に開いて塗る × 3（塗りの間 P コアで走ること）"
for round in 1 2 3; do
  if ! wait_napped; then
    echo "  未実測: 300 秒待っても GUI が間引かれない（優先度 $(pri)）"
    UNMEASURED=1
    break
  fi
  echo "    間引かれた（優先度 $(pri)）"
  R="$(measure_open "$TMP/in/a.rs")"
  if [ "$R" = nostart ]; then fail "③-${round} 塗りが走り始めない"; continue; fi
  report_round "③-${round}" "$R"
  set -- $R
  if on_p_cores "$2"; then
    pass "③-${round} 間引かれた後も塗りは P コアで走る（比率 ${2}）"
  else
    fail "③-${round} 間引かれた GUI の塗りが E コアへ寄せられた（P コア比率 ${2}・塗りの途中の優先度 ${4:-?}。#1916 の回帰 = 塗りの間 UserWork を握っていない）"
  fi
done

# ④ 取り消し・重なった塗り
echo
echo "④ 塗りの途中で閉じる / 塗りの途中で別のファイルも開く"
if [ "$UNMEASURED" = 0 ] && wait_napped; then
  P="$(open_pane "$TMP/in/a.rs")"
  if wait_highlight_start "$P"; then
    "$TAKO_BIN" close --pane "$P" --force >/dev/null 2>&1
    pass "④ 塗りの途中でペインを閉じられる"
  else
    fail "④ 閉じる前に塗りが走り始めない"
  fi
  # 閉じたペインの塗りは最後まで走る（取り消さない）。戻ったら依頼を手放す = また間引かれる
  if wait_napped; then
    pass "④ 閉じたペインの塗りが戻ったら、また間引かれる（握りっぱなしにしない。優先度 $(pri)）"
  else
    fail "④ 閉じた後 300 秒たっても間引かれない（依頼を手放していない？ 優先度 $(pri)）"
  fi
  before="$(ru)"
  PA="$(open_pane "$TMP/in/a.rs")"
  wait_highlight_start "$PA"
  PB="$(open_pane "$TMP/in/b.rs")"
  wait_highlight_start "$PB"
  # 2 本が重なって走っている（どちらも戻っていない）ところを通ってから待つ
  [ "$(highlighting "$PA")" = true ] && [ "$(highlighting "$PB")" = true ]; RO=$?
  wait_highlight_end "$PA"; RA=$?
  wait_highlight_end "$PB"; RB=$?
  after="$(ru)"
  set -- $(ru_delta "$before" "$after")
  echo "    重なった 2 本: P コア比率 ${1}・命令 ${2}G"
  if [ "$RO" = 0 ] && [ "$RA" = 0 ] && [ "$RB" = 0 ] && on_p_cores "$1"; then
    pass "④ 重なった 2 本の塗りも P コアで走り、両方戻る"
  else
    fail "④ 重なった 2 本の塗り（重なり=$RO 戻り a=$RA b=${RB}・P コア比率 ${1}）"
  fi
  "$TAKO_BIN" close --pane "$PA" --force >/dev/null 2>&1
  "$TAKO_BIN" close --pane "$PB" --force >/dev/null 2>&1
  if wait_napped; then
    pass "④ 重なった塗りが全部戻ったら、また間引かれる（優先度 $(pri)）"
  else
    fail "④ 重なった塗りの後 300 秒たっても間引かれない（優先度 $(pri)）"
  fi
else
  echo "  未実測: 間引かれた状態を作れない"
  UNMEASURED=1
fi

# ⑤ 小さいファイル
echo
echo "⑤ 小さいファイル"
P="$(open_pane "$TMP/in/small.rs")"
ok=0
for i in $(seq 1 100); do
  [ "$(highlighting "$P")" = false ] && { ok=1; break; }
  sleep 0.1
done
if [ "$ok" = 1 ]; then pass "⑤ 小さいファイルの塗りが戻る（highlighting が残らない）"; else fail "⑤ 小さいファイルの塗りが戻らない"; fi
"$TAKO_BIN" close --pane "$P" --force >/dev/null 2>&1

echo
echo "結果: PASS=$PASS FAIL=$FAIL"
if [ "$FAIL" -gt 0 ]; then
  echo "FAILED"
  exit 1
fi
[ "$UNMEASURED" = 1 ] && exit 4
exit 0
