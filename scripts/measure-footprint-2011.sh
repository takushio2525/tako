#!/bin/bash
# measure-footprint-2011.sh — tako 本体の常駐メモリ・アイドル CPU・描画回数を同じ条件で測る（#2011）
#
# 何のためか:
#   描画と RAM の軽量化（エピック #2011）の前後と、回帰の有無を**毎回同じ条件**で比べるため。
#   隔離 GUI（release・tako-vd・窓の寸法固定）にペインとプレビューを決まった形で並べ、
#   決まった秒数だけ放置して次を出す。判定はしない（数値を出すだけ。比べるのは人か後段）。
#
#   - footprint（`footprint -j`）: 合計と、malloc / 描画（IOAccelerator・unmapped graphics・IOSurface）/
#     その他の内訳。器のときは tmux のクライアント（tako-app の子）とサーバーも別に足す
#   - アイドル CPU（`proc_pid_rusage` v6）: 本体と、回収済みの子プロセス（tako が起こした ps・tmux・
#     ログインシェル等。`ps -o time` には出ない）・電力（ri_energy_nj）・wakeup。
#     回収済みの子は出力中の区間も出す（`busy_child_cpu_pct`。器のペインログの取り込みの
#     `tmux list-panes` / `capture-pane` はここに出る = #2018）
#   - 描画回数（`tako redraw-limit`）: ペイン本体の描画 `body_renders` と出力起因の再描画の要求 `flushes`
#   - 起動時間（起こしてから `tako list` が通るまで）・常駐スレッド数・子プロセス数
#
# どのシナリオも「① 放置」と「② 出力中（表に 1 枚足し、1 行 10ms で出し続ける）」の 2 区間を測る。
# **描画資源（IOSurface・… (graphics)）は描いている最中しか計上されない**（放置 40 秒ほどで外れる）ので、
# 描画のメモリは②の footprint で比べる。①の footprint は描画資源の下限。
#
# 条件（シナリオ）:
#   std       … 本番に寄せた 1 本（器 30 = 表 4 + 裏のタブ 26、裏のタブにプレビュー 5 = PNG 2・PDF・
#                動画・コード。本番の 10/13 は表 8・裏のタブ 18・プレビュー 8・退避 0）。
#                #2009 などの前後比較はこれを同じバイナリの A/B（TAKO_FOOTPRINT_ENV）で回す
#   sweep     … ペイン 1 枚あたりの持ち分を出す格子（直接 / 器 × 1・4・10・30。表は最大 4 枚、残りは
#                裏のタブに 3 枚ずつ）+ 器 30 の残り 26 枚を退避にした 1 本
#   all       … sweep + std
#   <mode>:<n>[bg] … 1 本だけ（例: persist:10 / direct:30 / persist:30bg）
#
# 使い方: bash scripts/measure-footprint-2011.sh [シナリオ（既定 std）] [放置の秒（既定 60）]
#   TAKO_FOOTPRINT_ENV="TAKO_2009_LEGACY=1"  起こす GUI へ足す env（A/B 用。空白区切り）
#   TAKO_FOOTPRINT_OUT=<file>                結果の TSV を追記する先（既定は表示だけ）
#   TAKO_FOOTPRINT_STUB_CLAUDE=1             GUI の PATH の先頭へ「即座に [] を返す claude」を置く（#2018）。
#                                            隔離 HOME には claude の台帳が無く #2017 の絞り込みが効かないので、
#                                            器に子が居ると 30 秒ごとに `claude agents --json`（Node）が起き、
#                                            回収済みの子の CPU の大半を占める。tmux の取り込みだけを比べるときに使う
#
# **蓋を閉じた機・画面ロック中はフレームが組まれない**（memory: lid-closed-no-frames）。出力の
#   `描画` 欄が「なし」の行は描画の CPU・回数・描画の資源を含まない（未実測と読む）。
#
# 隔離 GUI・tako-vd・本番の tako / 設定 / tmux には触らない。落とすのは自分で起こした pid と
# 自分の tmux ソケットだけ。CPU を焼く負荷生成はしない（出力させるのは 1 ペインだけ・1 行 10ms）。
set -uo pipefail
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_MCP_URL TAKO_ORCHESTRATOR_ROLE

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SCEN="${1:-std}"
IDLE="${2:-60}"
EXTRA_ENV="${TAKO_FOOTPRINT_ENV:-}"
STUB_CLAUDE="${TAKO_FOOTPRINT_STUB_CLAUDE:-0}"
OUT="${TAKO_FOOTPRINT_OUT:-}"
# 窓の寸法は固定する（描画面は窓の画素数に比例するので、寸法が揃わないと比べられない）
BOUNDS="${TAKO_FOOTPRINT_BOUNDS:-0,0,1600,1000}"
# ソケット名は短く（tmux / unix ソケットのパス長の上限に当たらないように）
TMP="$(mktemp -d /tmp/tako-fp-XXXXXX)"
SOCK_NAME="tako-fp-$$"
CAFF_PID=""

cleanup() {
  [ -n "$CAFF_PID" ] && kill "$CAFF_PID" 2>/dev/null
  stop_isolated_gui "${ISOLATED_GUI_PID:-}"
  tmux -L "$SOCK_NAME" kill-server >/dev/null 2>&1 || true
  rm -rf "$TMP"
}
trap cleanup EXIT
# シグナルで落ちても後片付けを通す（`… | head` でパイプを閉じられると SIGPIPE で即死し、
# EXIT の trap が走らずに隔離 GUI と tmux が残った = #2011 の S0 で実際に取り残した）
trap 'exit 130' INT TERM HUP PIPE

case "$SCEN" in
  std) RUNS="persist:30std" ;;
  sweep) RUNS="direct:1 direct:4 direct:10 direct:30 persist:1 persist:4 persist:10 persist:30 persist:30bg" ;;
  all) RUNS="direct:1 direct:4 direct:10 direct:30 persist:1 persist:4 persist:10 persist:30 persist:30bg persist:30std" ;;
  direct:[0-9]*|persist:[0-9]*) RUNS="$SCEN" ;;
  *) echo "不明なシナリオ: ${SCEN}（std / sweep / all / <direct|persist>:<n>[bg|std]）" >&2; exit 2 ;;
esac

# --- ビルドは HOME を差し替える前に（memory: isolated-script-cargo-before-home） ---
export TAKO_ISO_PROFILE=release
# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
(cd "$REPO_ROOT" && run_heavy cargo build --release -q -p tako-cli -p tako-app) || exit 1
isolated_gui_bins || exit 1
ISOLATED_GUI_BOUNDS="$BOUNDS"

# --- 計測の道具（python は標準ライブラリだけ） ---
cat > "$TMP/ru.py" <<'PY'
# proc_pid_rusage v6: user / system / 回収済みの子の user・system（秒）/ 電力（J）/ wakeup
import ctypes, sys
class RU(ctypes.Structure):
    _fields_ = [("uuid", ctypes.c_uint8 * 16), ("f", ctypes.c_uint64 * 47), ("r", ctypes.c_uint64 * 9)]
class TB(ctypes.Structure):
    _fields_ = [("numer", ctypes.c_uint32), ("denom", ctypes.c_uint32)]
lib = ctypes.CDLL("/usr/lib/libSystem.B.dylib")
tb = TB(); lib.mach_timebase_info(ctypes.byref(tb))
k = tb.numer / tb.denom / 1e9
ru = RU()
if lib.proc_pid_rusage(int(sys.argv[1]), 6, ctypes.byref(ru)) != 0:
    sys.exit(1)
f = ru.f
print(f"{f[0]*k:.4f} {f[1]*k:.4f} {f[10]*k:.4f} {f[11]*k:.4f} {f[40]/1e9:.4f} {f[2]+f[3]}")
PY
cat > "$TMP/fp.py" <<'PY'
# footprint -j の tako-app を「合計 / malloc / 描画 / その他」へ、残り（tmux）は合計だけ出す（MB）。
# **各区分は `dirty` だけを足す**: `dirty` は圧縮・スワップされた分を既に含み、区分の `dirty` の和が
# phys_footprint と一致する。`swapped` を足すと二重に数え、メモリ圧が高いと malloc が合計を超える
# （#2011 の S0 で実際に踏んだ）
import json, sys
d = json.load(open(sys.argv[1]))
procs = d.get("processes") or []
MB = 1024 * 1024
def gfx(k):
    return k.startswith("IOAccelerator") or "(graphics)" in k or k == "IOSurface" or k == "CoreAnimation"
def cat(p, pred):
    return sum(v.get("dirty", 0) for k, v in (p.get("categories") or {}).items() if pred(k))
app = next((p for p in procs if str(p.get("pid")) == sys.argv[2]), None)
if app is None:
    print("- - - - -"); sys.exit(0)
total = app["auxiliary"]["phys_footprint"]
malloc = cat(app, lambda k: k.startswith("MALLOC"))
other = cat(app, lambda k: not k.startswith("MALLOC") and not gfx(k))
tmux = sum(p["auxiliary"]["phys_footprint"] for p in procs if p is not app)
print(" ".join(f"{x / MB:.1f}" for x in (total, malloc, total - malloc - other, other, tmux)))
PY
now_ms() { python3 -c 'import time; print(int(time.time() * 1000))'; }
cat > "$TMP/pstart.py" <<'PY'
# プロセスの起動時刻（epoch ミリ秒）を proc_pidinfo(PROC_PIDTBSDINFO) で読む
import ctypes, sys
class BSD(ctypes.Structure):
    _fields_ = [("u", ctypes.c_uint32 * 12), ("comm", ctypes.c_char * 16), ("name", ctypes.c_char * 32),
                ("v", ctypes.c_uint32 * 5), ("nice", ctypes.c_int32), ("sec", ctypes.c_uint64), ("usec", ctypes.c_uint64)]
b = BSD()
if ctypes.CDLL("/usr/lib/libSystem.B.dylib").proc_pidinfo(int(sys.argv[1]), 3, ctypes.c_uint64(0), ctypes.byref(b), ctypes.sizeof(b)) != ctypes.sizeof(b):
    sys.exit(1)
print(b.sec * 1000 + b.usec // 1000)
PY
ru() { python3 "$TMP/ru.py" "$1"; }
renders() { # body_renders と flushes（focused + unfocused）
  "$TAKO_BIN" redraw-limit 2>/dev/null | python3 -c '
import json, sys
d = json.load(sys.stdin)
print(d["body_renders"], d["flushes"]["focused"] + d["flushes"]["unfocused"])'
}
first_pane() { "$TAKO_BIN" list | python3 -c 'import json,sys; print(json.load(sys.stdin)["tabs"][0]["panes"][0]["id"])'; }
active_tab() { "$TAKO_BIN" list | python3 -c 'import json,sys; print(json.load(sys.stdin)["active_tab"])'; }
new_tab() { # 新しいタブ（前面にしない）の id と最初のペイン id（`tab new` は {"tab":…,"pane":…} を返す）
  "$TAKO_BIN" tab new 2>/dev/null | python3 -c 'import json,sys; d=json.load(sys.stdin); print(d["tab"], d["pane"])'
}
split_into() { # <基準ペイン> <向き> → 新ペインの id（`split` は id だけを 1 行で返す）
  "$TAKO_BIN" split --pane "$1" "--$2" 2>/dev/null | tr -d '[:space:]'
}
# <枚数> を 3 枚ずつの新しいタブ（前面にしない）へ並べる。作ったタブは MADE_TABS、ペインは ALL_TERMS へ足す
# （サブシェルで呼ぶと足したぶんが消えるので、結果は大域変数で返す）
make_tabs() {
  local want="$1" made=0 in_tab ids tid p
  MADE_TABS=""
  while [ "$made" -lt "$want" ]; do
    ids="$(new_tab)" || break
    tid="${ids%% *}"; p="${ids##* }"
    [ -n "$tid" ] && [ -n "$p" ] || break
    MADE_TABS="$MADE_TABS $tid"; ALL_TERMS="$ALL_TERMS $p"; made=$((made + 1)); in_tab=1
    while [ "$in_tab" -lt 3 ] && [ "$made" -lt "$want" ]; do
      p="$(split_into "$p" down)"; ALL_TERMS="$ALL_TERMS $p"; made=$((made + 1)); in_tab=$((in_tab + 1))
    done
  done
}
# 中身: 3000 行（日本語を含む）を流してスクロールバックへ積む。最後の行の印で入ったかを確かめる
fill() { "$TAKO_BIN" send --pane "$1" "seq -f '%g tako 計測用の行 lorem ipsum dolor sit amet' 1 3000; echo FP-DONE" >/dev/null 2>&1; }
filled_count() {
  local n=0 p
  for p in $ALL_TERMS; do
    "$TAKO_BIN" read --pane "$p" 2>/dev/null | grep -q '^FP-DONE' && n=$((n + 1))
  done
  echo "$n"
}
lid_state() { ioreg -r -k AppleClamshellState -d 1 2>/dev/null | grep -q '"AppleClamshellState" = Yes' && echo "閉" || echo "開"; }
# 画面がロックされている間はフレームが組まれない（描画の値が未実測になる）ので行に残す
lock_state() { ioreg -n Root -d1 -a 2>/dev/null | grep -A1 CGSSessionScreenIsLocked | grep -q '<true/>' && echo "ロック" || echo "-"; }
# footprint を 1 回で採る: 本体 + 器のときは tmux のクライアント（本体の子）とサーバー
measure_fp() {
  local pid="$1" fp_args c srv
  fp_args="-p $pid"
  for c in $(pgrep -P "$pid" 2>/dev/null); do
    case "$(ps -o comm= -p "$c" 2>/dev/null)" in *tmux*) fp_args="$fp_args -p $c" ;; esac
  done
  srv="$(tmux -L "$SOCK_NAME" display -p '#{pid}' 2>/dev/null)"
  [ -n "$srv" ] && fp_args="$fp_args -p $srv"
  rm -f "$TMP/fp.json"
  # shellcheck disable=SC2086
  footprint -j "$TMP/fp.json" $fp_args >/dev/null 2>&1
  python3 "$TMP/fp.py" "$TMP/fp.json" "$pid" 2>/dev/null || echo "- - - - -"
}
# 出力の生成: 1 行 10ms（100 行 / 秒）の python 1 本 = 1 コアも使わない量（CPU を焼かない）
GEN='python3 -u -c "import time,itertools
for i in itertools.count():
    print(i, time.time(), flush=True); time.sleep(0.01)"'

# 素材（プレビュー用）。毎回同じ中身
make_assets() {
  python3 - "$TMP/assets" <<'PY'
import os, struct, sys, zlib
out = sys.argv[1]
os.makedirs(out, exist_ok=True)
def png(path, w, h, seed):
    rows = bytearray()
    for y in range(h):
        rows.append(0)
        for x in range(w):
            rows += bytes(((x * 255 // w + seed) & 255, (y * 255 // h) & 255, ((x ^ y) + seed) & 255))
    def chunk(t, d):
        return struct.pack(">I", len(d)) + t + d + struct.pack(">I", zlib.crc32(t + d) & 0xffffffff)
    data = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0))
    open(path, "wb").write(data + chunk(b"IDAT", zlib.compress(bytes(rows), 6)) + chunk(b"IEND", b""))
def pdf(path, pages):
    objs = ["<< /Type /Catalog /Pages 2 0 R >>",
            "<< /Type /Pages /Kids [%s] /Count %d >>" % (" ".join("%d 0 R" % (3 + i * 2) for i in range(pages)), pages)]
    font = 3 + pages * 2
    for i in range(pages):
        text = "".join("BT /F1 11 Tf 56 %d Td (page %d line %d lorem ipsum dolor sit amet) Tj ET\n" % (780 - n * 14, i + 1, n + 1) for n in range(52))
        objs.append("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] /Contents %d 0 R /Resources << /Font << /F1 %d 0 R >> >> >>" % (4 + i * 2, font))
        objs.append("<< /Length %d >>\nstream\n%sendstream" % (len(text), text))
    objs.append("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>")
    body = b"%PDF-1.4\n"; offs = []
    for n, o in enumerate(objs, 1):
        offs.append(len(body)); body += ("%d 0 obj\n%s\nendobj\n" % (n, o)).encode()
    x = len(body)
    body += ("xref\n0 %d\n0000000000 65535 f \n" % (len(objs) + 1)).encode() + "".join("%010d 00000 n \n" % o for o in offs).encode()
    body += ("trailer\n<< /Size %d /Root 1 0 R >>\nstartxref\n%d\n%%%%EOF\n" % (len(objs) + 1, x)).encode()
    open(path, "wb").write(body)
png(os.path.join(out, "a.png"), 2400, 1600, 0)
png(os.path.join(out, "b.png"), 2400, 1600, 97)
pdf(os.path.join(out, "c.pdf"), 30)
with open(os.path.join(out, "e.rs"), "w") as f:
    for i in range(3000):
        f.write("/// 行 %d\npub fn f%d(x: u64) -> Option<String> { if x %% %d == 0 { Some(format!(\"{x}-%d\")) } else { None } }\n" % (i, i, i + 2, i))
PY
  # 動画は ffmpeg がある機だけ（無ければ省いて、その旨を出す）
  if command -v ffmpeg >/dev/null 2>&1; then
    ffmpeg -loglevel error -f lavfi -i testsrc=size=1280x720:rate=30 -t 10 -pix_fmt yuv420p "$TMP/assets/d.mp4" </dev/null || true
  fi
}

# --- 隔離の置き場（ここから下は HOME を差し替える） ---
export HOME="$TMP/home"
export TAKO_DATA_DIR="$TMP/data"
export TAKO_DISCOVERY_DIR="$TMP/disc"
export TAKO_ORCHESTRATOR_DIR="$TMP/orch"
export TAKO_SESSIONS_FILE="$TMP/sessions.yaml"
export TAKO_PANE_LOG_DIR="$TMP/panelogs"
export TAKO_WORKERS_FILE="$TMP/workers.yaml"
export TAKO_TMUX_SOCKET="$SOCK_NAME"
mkdir -p "$HOME"
make_assets
# 偽の claude（TAKO_FOOTPRINT_STUB_CLAUDE=1）。HOME を差し替えた後なので本物の設定には触れない
STUB_PATH=""
ENV_LABEL="${EXTRA_ENV:--}"
if [ "$STUB_CLAUDE" = 1 ]; then
  mkdir -p "$TMP/stub-bin"
  printf '#!/bin/sh\nprintf "[]\\n"\n' > "$TMP/stub-bin/claude"
  chmod +x "$TMP/stub-bin/claude"
  STUB_PATH="$TMP/stub-bin:$PATH"
  ENV_LABEL="$ENV_LABEL stub-claude"
fi

HEADER="scenario	mode	panes	front	back_tab	bg	previews	startup_ms	footprint_mb	malloc_mb	gfx_mb	other_mb	tmux_mb	threads	children	cpu_pct	child_cpu_pct	energy_mw	wakeups_s	body_renders_s	flushes_s	busy_footprint_mb	busy_malloc_mb	busy_gfx_mb	busy_cpu_pct	busy_child_cpu_pct	busy_energy_mw	busy_body_renders_s	busy_flushes_s	filled	drawn	lid	locked	env"
echo "$HEADER" > "$TMP/result.tsv"

# 1 本ぶん: 起こす → 並べる → 放置して測る → 落とす
run_one() {
  local spec="$1" mode n variant persist front rest back_tab=0 bg=0 previews=0
  mode="${spec%%:*}"; n="${spec#*:}"; variant=""
  case "$n" in *std) variant=std; n="${n%std}" ;; *bg) variant=bg; n="${n%bg}" ;; esac
  persist=0; [ "$mode" = persist ] && persist=1

  rm -rf "$TAKO_DATA_DIR" "$TAKO_DISCOVERY_DIR" "$TAKO_ORCHESTRATOR_DIR" "$TAKO_PANE_LOG_DIR"
  mkdir -p "$TAKO_DATA_DIR" "$TAKO_DISCOVERY_DIR" "$TAKO_ORCHESTRATOR_DIR"
  tmux -L "$SOCK_NAME" kill-server >/dev/null 2>&1 || true

  local t0 t1
  # shellcheck disable=SC2086
  launch_isolated_gui "$TMP/app.log" TAKO_PERSIST=$persist $EXTRA_ENV ${STUB_PATH:+"PATH=$STUB_PATH"} || exit $?
  # 起動時間は「プロセスが生まれた時刻 → `tako list` が通った時刻」（仮想ディスプレイの用意の時間を含めない）
  t0="$(python3 "$TMP/pstart.py" "$ISOLATED_GUI_PID" 2>/dev/null || now_ms)"
  local sock_try
  for sock_try in $(seq 1 300); do
    [ -f "$TAKO_DATA_DIR/token" ] && break
    sleep 0.1
  done
  local sock_path="$TAKO_DATA_DIR/tako.sock"
  [ -f "$TAKO_DATA_DIR/tako.sock.path" ] && sock_path="$(cat "$TAKO_DATA_DIR/tako.sock.path")"
  export TAKO_SOCKET="$sock_path"
  TAKO_TOKEN="$(cat "$TAKO_DATA_DIR/token" 2>/dev/null)"
  export TAKO_TOKEN
  # 空なら進めない（空の TAKO_SOCKET は既定の解決 = 本番 GUI へ落ちる。memory: isolated-cli-verification）
  [ -n "$TAKO_SOCKET" ] && [ -n "$TAKO_TOKEN" ] || { echo "隔離先の socket / token が空なので止める"; exit 1; }
  wait_isolated_gui "$TMP/app.log" 600 || exit 1
  t1="$(now_ms)"
  # 隔離 GUI でも自動命名は claude を呼ぶので切る（memory: isolated-gui-autorename-calls-claude）
  "$TAKO_BIN" autorename off >/dev/null 2>&1 || true

  local first tab1 p tid
  first="$(first_pane)"; tab1="$(active_tab)"
  ALL_TERMS="$first"
  front=$n; [ "$front" -gt 4 ] && front=4
  rest=$((n - front))
  # 表: 最初のタブに最大 4 枚（2×2）
  if [ "$front" -ge 2 ]; then p="$(split_into "$first" right)"; ALL_TERMS="$ALL_TERMS $p"; fi
  if [ "$front" -ge 3 ]; then p="$(split_into "$first" down)"; ALL_TERMS="$ALL_TERMS $p"; fi
  if [ "$front" -ge 4 ]; then p="$(split_into "$(echo "$ALL_TERMS" | awk '{print $2}')" down)"; ALL_TERMS="$ALL_TERMS $p"; fi
  # 残り: 3 枚ずつ裏のタブ。bg は全部を退避（タブ単位で退避する）
  local to_bg=0
  [ "$variant" = bg ] && to_bg=$rest
  make_tabs $((rest - to_bg))
  make_tabs "$to_bg"
  local bg_tabs="$MADE_TABS"
  for p in $ALL_TERMS; do fill "$p"; done
  # プレビュー（std だけ）: 1 本ずつ新しいタブで開き、最後に最初のタブへ戻す = 全部が裏のタブ
  if [ "$variant" = std ]; then
    local f
    for f in a.png b.png c.pdf d.mp4 e.rs; do
      [ -f "$TMP/assets/$f" ] || continue
      "$TAKO_BIN" open "$TMP/assets/$f" --pane "$first" --new-tab >/dev/null 2>&1 && previews=$((previews + 1))
      sleep 1
    done
  fi
  # `send` は入力欄が現れてから打つ（裏のタブの器はシェルの起動が遅く、送達が後から起きる）ので、
  # 全部に入るまで最大 60 秒待つ。入り切らなければ `filled` 欄にそのまま残す（条件が崩れた印）
  local filled n_try
  for n_try in $(seq 1 30); do
    sleep 2
    filled="$(filled_count)"
    [ "$filled" -ge "$n" ] && break
  done
  for tid in $bg_tabs; do "$TAKO_BIN" background --tab "$tid" >/dev/null 2>&1; done
  # 数は実物から数え直す（退避が効いていなければ 0 と出る = 条件が崩れたことが行に残る）
  bg="$("$TAKO_BIN" list | python3 -c 'import json,sys; d=json.load(sys.stdin); print(sum(1 for t in d["tabs"] for p in t["panes"] if p.get("surface") == "background"))')"
  back_tab=$((n - front - bg))
  "$TAKO_BIN" tab select "$tab1" >/dev/null 2>&1
  "$TAKO_BIN" focus "$first" >/dev/null 2>&1

  # 面を眠らせない（`caffeinate -u` はユーザー活動の宣言だけ。自分の pid だけ後で落とす）
  caffeinate -u -t $((IDLE * 2 + 90)) >/dev/null 2>&1 &
  CAFF_PID=$!
  sleep 15
  local pid="$ISOLATED_GUI_PID" r0 r1 b0 b1 fp_idle threads children
  # ① 放置: CPU・描画回数を IDLE 秒、footprint はその直後（描いていないので描画資源は下限）
  r0="$(ru "$pid")"; b0="$(renders)"
  sleep "$IDLE"
  r1="$(ru "$pid")"; b1="$(renders)"
  fp_idle="$(measure_fp "$pid")"
  threads="$(ps -M -p "$pid" | tail -n +2 | wc -l | tr -d ' ')"
  children="$(pgrep -P "$pid" 2>/dev/null | wc -l | tr -d ' ')"

  # ② 出力中: 表に 1 枚足して 1 行 10ms で出し続ける（フォーカスは最初のペインのまま = 上限 30 fps 側）。
  #    描画資源は描いている最中しか計上されないので、footprint はこの区間の終わりに採る
  local q0 q1 s0 s1 fp_busy
  "$TAKO_BIN" split --pane "$first" --right -- /bin/sh -c "$GEN" >/dev/null 2>&1
  "$TAKO_BIN" focus "$first" >/dev/null 2>&1
  sleep 5
  q0="$(ru "$pid")"; s0="$(renders)"
  sleep "$IDLE"
  q1="$(ru "$pid")"; s1="$(renders)"
  fp_busy="$(measure_fp "$pid")"
  kill "$CAFF_PID" 2>/dev/null; CAFF_PID=""

  python3 - "$spec" "$mode" "$n" "$front" "$back_tab" "$bg" "$previews" "$((t1 - t0))" "$fp_idle" "$threads" "$children" \
    "$r0" "$r1" "$b0" "$b1" "$fp_busy" "$q0" "$q1" "$s0" "$s1" "$IDLE" "$filled/$n" "$(lid_state)" "$(lock_state)" "$ENV_LABEL" \
    >> "$TMP/result.tsv" <<'PY'
import sys
(spec, mode, n, front, back, bg, prev, startup, fp_idle, threads, children,
 r0, r1, b0, b1, fp_busy, q0, q1, s0, s1, secs, filled, lid, locked, env) = sys.argv[1:]
secs = float(secs)
def f(s): return [float(x) for x in s.split()]
def rates(a, b, c, d):
    a, b, c, d = f(a), f(b), f(c), f(d)
    return dict(cpu=(b[0] + b[1] - a[0] - a[1]) / secs * 100, child=(b[2] + b[3] - a[2] - a[3]) / secs * 100,
                energy=(b[4] - a[4]) / secs * 1000, wake=(b[5] - a[5]) / secs,
                body=(d[0] - c[0]) / secs, flush=(d[1] - c[1]) / secs)
i = rates(r0, r1, b0, b1)
k = rates(q0, q1, s0, s1)
bt, bm, bg_, _, _ = (fp_busy.split() + ["-"] * 5)[:5]
# 出力中に本体の描画が 1 回も起きていなければ、フレームが組まれていない（蓋閉じ・画面ロック = 描画の値は未実測）
drawn = "あり" if k["body"] > 0 else "なし"
print("\t".join([spec, mode, n, front, back, bg, prev, startup] + fp_idle.split() + [threads, children,
      "%.2f" % i["cpu"], "%.2f" % i["child"], "%.1f" % i["energy"], "%.1f" % i["wake"], "%.2f" % i["body"], "%.2f" % i["flush"],
      bt, bm, bg_, "%.2f" % k["cpu"], "%.2f" % k["child"], "%.1f" % k["energy"], "%.1f" % k["body"], "%.1f" % k["flush"],
      filled, drawn, lid, locked, env]))
PY
  stop_isolated_gui "$ISOLATED_GUI_PID"; ISOLATED_GUI_PID=""
  tmux -L "$SOCK_NAME" kill-server >/dev/null 2>&1 || true
  unset TAKO_SOCKET TAKO_TOKEN
  tail -1 "$TMP/result.tsv" | awk -F'\t' '{printf "  %-14s 放置 %6s MB（malloc %s・描画 %s・tmux %s）CPU %s%%・子 %s%%・描画 %s 回/秒 ｜ 出力中 %s MB（描画 %s）CPU %s%%・子 %s%%・描画 %s 回/秒 ｜ 起動 %s ms・スレッド %s・中身 %s・描画 %s・蓋 %s\n", $1, $9, $10, $11, $13, $16, $17, $20, $22, $24, $25, $26, $28, $8, $14, $30, $31, $32 ($33 == "ロック" ? "・画面ロック中" : "")}'
  sleep 3
}

echo "tako-app（release・tako-vd・窓 ${BOUNDS}・放置 ${IDLE} 秒）のシナリオ: ${RUNS}"
for spec in $RUNS; do run_one "$spec"; done

echo
echo "== 結果（TSV）"
cat "$TMP/result.tsv"
if [ -n "$OUT" ]; then
  [ -s "$OUT" ] || echo "$HEADER" > "$OUT"
  tail -n +2 "$TMP/result.tsv" >> "$OUT"
  echo "（${OUT} へ追記した）"
fi
# 持ち分（1 枚あたり）: 同じ回の行どうしの差から出す（sweep / all のときだけ意味がある）
python3 - "$TMP/result.tsv" <<'PY'
import csv, sys
rows = list(csv.DictReader(open(sys.argv[1]), delimiter="\t"))
by = {r["scenario"]: r for r in rows}
def val(k, col):
    r = by.get(k)
    if r is None or r[col] == "-":
        return None
    return float(r[col]) + (float(r["tmux_mb"]) if col == "footprint_mb" else 0.0)
def line(label, a, b, div, note):
    parts = []
    for col, name in (("footprint_mb", "footprint（tmux 込み）"), ("malloc_mb", "malloc")):
        x, y = val(a, col), val(b, col)
        if x is not None and y is not None:
            parts.append(f"{name} {(y - x) / div:6.2f} MB")
    if parts:
        out.append(f"  {label:<16} " + "・".join(parts) + f"（{note}）")
out = []
for mode in ("direct", "persist"):
    line(f"{mode} 表 1 枚", f"{mode}:1", f"{mode}:4", 3, "1 → 4 枚の差 / 3")
    line(f"{mode} 裏 1 枚", f"{mode}:10", f"{mode}:30", 20, "10 → 30 枚の差 / 20")
line("persist 退避 1 枚", "persist:4", "persist:30bg", 26, "表 4 と、表 4 + 退避 26 の差 / 26")
if out:
    print("\n== 1 枚あたりの持ち分")
    print("\n".join(out))
PY
