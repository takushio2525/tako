#!/bin/bash
# test-preview-load-app-nap-1926.sh — プレビューの読み込みが App Nap で E コアへ寄せられない（#1926）の実経路テスト
#
# 何を確かめるか（隔離 GUI・release・tako-vd）:
#   ① 間引かれた（App Nap で優先度が落ちた）のを**状態で**待ってから PDF を開く × 2 回。
#      **ラスタライズの間 GUI のプロセスが P コアで走る**ことを判定する（旧挙動は E コアへ寄せられる）
#   ② 間引かれてから、開いている PDF をズームする（描き直しのラスタライズ）。同じく P コアで走ること
#   ③ 読み込みが戻ったら**また間引かれる**（依頼を握りっぱなしにしない = アプリ全体では止めない）
#   ④ Markdown: 間引かれた GUI で開いて目次が組み上がる（所要と P コアの比率は出力だけ。
#      2 MB 前後で打ち切られる軽い処理なので、判定には使わない）
#   ⑤ 前提: スリープ防止（#173）のアサーションを握っている間は間引かれず、外すとまた間引かれる
#      （エージェント稼働中に App Nap を止めなくてよい根拠。A/B の腕に依らず通る）
#
# 所要は出力するだけで判定に使わない（`.agent/conventions.md`「効果を測る単体テストは実時間で
# 比べない」）。判定は「P コアで走った比率」と状態だけ。
#
# A/B: `TAKO_1926_LEGACY=1 bash scripts/test-preview-load-app-nap-1926.sh` で ① と ② が名指しで
# FAILED（旧挙動 = 読み込みの間も App Nap に間引かせたまま。③ は旧では一度も戻らないので素通りする
# = 検出力は新の腕でだけ持つ）。
#
# E コアの無い機・間引かれない機（前面に出ている等）では測れないので「未実測」で終了コード 4
# （#1744 と同じ番号）。**本番の tako / 設定には一切触らない**（data / HOME は mktemp 配下、
# 落とすのは自分で起こした pid だけ）。窓は常設の仮想ディスプレイ（tako-vd）へ出る。
#
# 使い方: bash scripts/test-preview-load-app-nap-1926.sh
set -uo pipefail

# **本番 GUI を指す env を最初に落とす**（#1449 / #1450 で本番にペインが漏れた実例が 2 件）
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_ORCHESTRATOR_ROLE

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PASS=0
FAIL=0
UNMEASURED=0
pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
fail() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }
# 「P コアで走った」とみなす比率の下限（#1916 と同じ値。実測は修正後 0.848〜0.971・旧挙動 0.000〜0.002）
P_CORE_MIN=0.5
on_p_cores() { python3 -c "import sys; sys.exit(0 if $1 >= $P_CORE_MIN else 1)"; }

TMP="$(mktemp -d /tmp/tako-1926-XXXXXX)"
cleanup() {
  stop_isolated_gui "${ISOLATED_GUI_PID:-}"
  tmux -L "tako-1926-$$" kill-server >/dev/null 2>&1 || true
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
# **HOME を差し替える前に**ビルドする（後で cargo を呼ぶと一時 HOME へツールチェーンを取り直す）
echo "release の tako-app / tako をビルドします…"
(cd "$REPO_ROOT" && cargo build --release -q -p tako-cli -p tako-app) || exit 1
isolated_gui_bins || exit 1

export HOME="$TMP/home"
export TAKO_ISOLATED=1
export TAKO_DATA_DIR="$TMP/data"
export TAKO_DISCOVERY_DIR="$TMP/disc"
export TAKO_ORCHESTRATOR_DIR="$TMP/orch"
export TAKO_SESSIONS_FILE="$TMP/sessions.yaml"
export TAKO_PANE_LOG_DIR="$TMP/panelogs"
export TAKO_WORKERS_FILE="$TMP/workers.yaml"
export TAKO_TMUX_SOCKET="tako-1926-$$"
# ラスタライズの所要は background の区間（`bg:pdf_rasterize`）の統計で読む（TAKO_PERF_VERBOSE）
export TAKO_PERF_LOG="$TMP/perf.log"
mkdir -p "$HOME" "$TAKO_DATA_DIR" "$TAKO_DISCOVERY_DIR" "$TAKO_ORCHESTRATOR_DIR" "$TMP/in"
for d in "$HOME" "$TAKO_DATA_DIR" "$TAKO_ORCHESTRATOR_DIR"; do
  case "$d" in
    "$TMP"/*) : ;;
    *) echo "隔離されていないディレクトリ: $d"; exit 1 ;;
  esac
done

# 入力: 120 ページの PDF（文字だけ。外部の道具に頼らず組む）と、目次が 1 万件を超える Markdown
python3 - "$TMP/in" <<'PY'
import os, sys
out = sys.argv[1]
pages = 120
objs = {1: b"<< /Type /Catalog /Pages 2 0 R >>",
        3: b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>"}
kids = []
nid = 4
for p in range(pages):
    lines = [f"BT /F1 9 Tf 40 {800 - 12 * i} Td (page {p} line {i}: the quick brown fox jumps over the lazy dog 0123456789) Tj ET"
             for i in range(60)]
    stream = "\n".join(lines).encode()
    objs[nid] = b"<< /Length %d >>\nstream\n" % len(stream) + stream + b"\nendstream"
    objs[nid + 1] = (f"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 842] "
                     f"/Resources << /Font << /F1 3 0 R >> >> /Contents {nid} 0 R >>").encode()
    kids.append(nid + 1)
    nid += 2
objs[2] = ("<< /Type /Pages /Kids [%s] /Count %d >>" % (" ".join(f"{k} 0 R" for k in kids), pages)).encode()
pdf = bytearray(b"%PDF-1.4\n")
offsets = {}
for i in range(1, nid):
    offsets[i] = len(pdf)
    pdf += b"%d 0 obj\n" % i + objs[i] + b"\nendobj\n"
xref = len(pdf)
pdf += b"xref\n0 %d\n0000000000 65535 f \n" % nid
for i in range(1, nid):
    pdf += b"%010d 00000 n \n" % offsets[i]
pdf += b"trailer\n<< /Size %d /Root 1 0 R >>\nstartxref\n%d\n" % (nid, xref) + b"%%EOF\n"
open(os.path.join(out, "big.pdf"), "wb").write(pdf)
md = []
for i in range(14000):
    md.append(f"## 見出し {i}\n\n本文 {i} の段落。**太字** と `code` と [リンク](https://example.com/{i}) を含む行。\n\n- 項目 a\n- 項目 b\n\n")
open(os.path.join(out, "big.md"), "w").write("".join(md))
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
pri() { ps -o pri= -p "$ISOLATED_GUI_PID" 2>/dev/null | tr -d ' '; }

launch_isolated_gui "$TMP/app.log" TAKO_PERF_VERBOSE=1 ${TAKO_1926_LEGACY:+TAKO_1926_LEGACY=$TAKO_1926_LEGACY} || exit $?
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
echo "    GUI pid=$ISOLATED_GUI_PID legacy=${TAKO_1926_LEGACY:-0}"

open_pane() { "$TAKO_BIN" open "$1" --pane "$ROOT_PANE" --new-tab 2>&1 | python3 -c 'import json,sys; print(json.load(sys.stdin)["pane"])'; }
# 間引かれる（優先度が 10 以下へ落ちる）のを状態で待つ（上限 300 秒）
wait_napped() {
  local i p
  for i in $(seq 1 600); do
    p="$(pri)"
    [ -n "$p" ] && [ "$p" -le 10 ] && return 0
    sleep 0.5
  done
  return 1
}
# ラスタライズの統計行（10 秒ごとの窓で出て空になる）の数
raster_lines() { local n; n="$(grep -c 'span 統計 \[bg:pdf_rasterize\]' "$TAKO_PERF_LOG" 2>/dev/null)"; echo "${n:-0}"; }
# 統計行が $1 より増えるまで状態で待つ（上限 120 秒）。増えたらその行の所要（max）を返す
wait_raster() { # 前の行数
  local i
  for i in $(seq 1 240); do
    if [ "$(raster_lines)" -gt "$1" ]; then
      grep 'span 統計 \[bg:pdf_rasterize\]' "$TAKO_PERF_LOG" | tail -1 | sed -n 's/.*max=\([0-9]*\)ms.*/\1/p'
      return 0
    fi
    sleep 0.5
  done
  return 1
}
# 1 回ラスタライズさせて「所要 ms / P コアの比率 / 命令 G / 途中の優先度」を返す（$@ = 起こす操作）
measure_raster() {
  local n0 before after ms mid
  n0="$(raster_lines)"
  before="$(ru)"
  "$@" >"$TMP/op.out" 2>&1
  sleep 1
  mid="$(pri)"
  ms="$(wait_raster "$n0")" || { echo "nospan"; return; }
  after="$(ru)"
  echo "$ms $(ru_delta "$before" "$after") $mid"
}

# ① 間引かれた後に PDF を開く × 2
echo
echo "① 間引かれた後に PDF（120 ページ）を開く × 2（ラスタライズの間 P コアで走ること）"
PDF_PANE=""
for round in 1 2; do
  if ! wait_napped; then
    echo "  未実測: 300 秒待っても GUI が間引かれない（優先度 $(pri)）"
    UNMEASURED=1
    break
  fi
  [ -n "$PDF_PANE" ] && "$TAKO_BIN" close --pane "$PDF_PANE" --force >/dev/null 2>&1
  echo "    間引かれた（優先度 $(pri)）"
  R="$(measure_raster "$TAKO_BIN" open "$TMP/in/big.pdf" --pane "$ROOT_PANE" --new-tab)"
  PDF_PANE="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["pane"])' "$TMP/op.out" 2>/dev/null)"
  if [ "$R" = nospan ]; then fail "①-${round} ラスタライズが 120 秒以内に戻らない"; continue; fi
  set -- $R
  echo "    ①-${round}: ${1}ms・P コア比率 ${2}・命令 ${3}G・途中の優先度 ${4:-?}"
  if on_p_cores "$2"; then
    pass "①-${round} 間引かれた後も PDF のラスタライズは P コアで走る（比率 ${2}）"
  else
    fail "①-${round} 間引かれた GUI の PDF のラスタライズが E コアへ寄せられた（P コア比率 ${2}・途中の優先度 ${4:-?}。#1926 の回帰 = spawn_preview_load が UserWork::begin_load を握っていない）"
  fi
done

# ② ズーム（描き直しのラスタライズ）
echo
echo "② 間引かれた後に開いている PDF をズームする（描き直しのラスタライズも P コアで走ること）"
# 描き直しは描画の中で起きるので、PDF のタブを前面に出しておく（`open --new-tab` は前面を変えない）。
# 前面に出た時点の表示幅に合わせた描き直しは、間引かれるのを待つ間に終わる
[ -n "$PDF_PANE" ] && "$TAKO_BIN" focus "$PDF_PANE" >/dev/null 2>&1
if [ "$UNMEASURED" = 0 ] && [ -n "$PDF_PANE" ] && wait_napped; then
  echo "    間引かれた（優先度 $(pri)）"
  R="$(measure_raster "$TAKO_BIN" preview --pane "$PDF_PANE" --zoom 200)"
  if [ "$R" = nospan ]; then
    fail "② ズームの描き直しが 120 秒以内に戻らない（$(head -c 200 "$TMP/op.out")）"
  else
    set -- $R
    echo "    ②: ${1}ms・P コア比率 ${2}・命令 ${3}G・途中の優先度 ${4:-?}"
    if on_p_cores "$2"; then
      pass "② 間引かれた後もズームの描き直しは P コアで走る（比率 ${2}）"
    else
      fail "② 間引かれた GUI のズームの描き直しが E コアへ寄せられた（P コア比率 ${2}・途中の優先度 ${4:-?}。#1926 の回帰 = ensure_pdf_raster_quality が UserWork::begin_load を握っていない）"
    fi
  fi
else
  echo "  未実測: 間引かれた状態か PDF のペインを作れない"
  UNMEASURED=1
fi

# ③ 戻ったら、また間引かれる
echo
echo "③ 読み込みが戻ったら、また間引かれる（アプリ全体では止めない）"
if [ "$UNMEASURED" = 0 ]; then
  [ -n "$PDF_PANE" ] && "$TAKO_BIN" close --pane "$PDF_PANE" --force >/dev/null 2>&1
  if wait_napped; then
    pass "③ 読み込みが戻ったら、また間引かれる（握りっぱなしにしない。優先度 $(pri)）"
  else
    fail "③ 読み込みの後 300 秒たっても間引かれない（依頼を手放していない？ 優先度 $(pri)）"
  fi
else
  echo "  未実測"
fi

# ④ Markdown（判定は「読み込める」だけ。所要と比率は出力のみ）
echo
echo "④ 間引かれた後に大きい Markdown を開く"
if [ "$UNMEASURED" = 0 ] && wait_napped; then
  before="$(ru)"
  M="$(python3 - "$TAKO_BIN" "$ROOT_PANE" "$TMP/in/big.md" <<'PY'
import json, subprocess, sys, time
tako, root, path = sys.argv[1:4]
t = time.perf_counter()
r = subprocess.run([tako, "open", path, "--pane", root, "--new-tab"], capture_output=True, text=True)
pane = json.loads(r.stdout)["pane"]
n = 0
while time.perf_counter() - t < 120:
    o = subprocess.run([tako, "preview-outline", "--pane", str(pane)], capture_output=True, text=True)
    try:
        d = json.loads(o.stdout)
        n = len(d.get("items") or d.get("outline") or [])
    except Exception:
        n = 0
    if n > 0:
        break
    time.sleep(0.05)
print(f"{(time.perf_counter() - t) * 1000:.0f} {n} {pane}")
PY
)"
  after="$(ru)"
  set -- $M
  echo "    Markdown: ${1}ms・目次 ${2} 件・P コア比率 $(ru_delta "$before" "$after" | cut -d' ' -f1)"
  if [ "${2:-0}" -gt 0 ]; then pass "④ 間引かれた GUI でも Markdown の目次が組み上がる"; else fail "④ Markdown の目次が 120 秒以内に組み上がらない"; fi
  [ -n "${3:-}" ] && "$TAKO_BIN" close --pane "$3" --force >/dev/null 2>&1
else
  echo "  未実測"
fi

# ⑤ 前提: スリープ防止のアサーションを握っている間は間引かれない
echo
echo "⑤ 前提: スリープ防止（#173）のアサーションを握っている間は間引かれない"
if [ "$UNMEASURED" = 0 ] && wait_napped; then
  "$TAKO_BIN" sleep-guard set --mode on --power-condition always >/dev/null 2>&1
  ok=0
  for i in $(seq 1 60); do
    p="$(pri)"
    [ -n "$p" ] && [ "$p" -gt 10 ] && { ok=1; break; }
    sleep 0.5
  done
  # 間引かれるまでの約 30 秒を越えて保たれるか
  low=0
  if [ "$ok" = 1 ]; then
    for i in $(seq 1 45); do
      p="$(pri)"
      [ -n "$p" ] && [ "$p" -le 10 ] && low=1
      sleep 1
    done
  fi
  if [ "$ok" = 1 ] && [ "$low" = 0 ]; then
    pass "⑤ アサーションを握ると戻り、45 秒たっても間引かれない（優先度 $(pri)）"
  else
    fail "⑤ アサーションを握っても間引かれる（戻った=${ok}・途中で落ちた=${low}・優先度 $(pri)。エージェント稼働中に App Nap を止めなくてよいという前提が崩れた）"
  fi
  "$TAKO_BIN" sleep-guard set --mode off >/dev/null 2>&1
  if wait_napped; then
    pass "⑤ アサーションを外すと、また間引かれる（優先度 $(pri)）"
  else
    fail "⑤ アサーションを外して 300 秒たっても間引かれない（優先度 $(pri)）"
  fi
else
  echo "  未実測"
fi

echo
echo "結果: PASS=$PASS FAIL=$FAIL"
if [ "$FAIL" -gt 0 ]; then
  echo "FAILED"
  exit 1
fi
[ "$UNMEASURED" = 1 ] && exit 4
exit 0
