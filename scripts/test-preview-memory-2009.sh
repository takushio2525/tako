#!/bin/bash
# test-preview-memory-2009.sh — 見えていないプレビューの手放しと、tako のメモリの内訳（#2009）を隔離 GUI で実測する
#
# 何を確かめるか:
#   scenario 段（新 / 旧 = TAKO_2009_LEGACY=1 を同じ手順で 1 本ずつ）:
#     本番に近い条件（14 タブ / 器つき端末 28 / プレビュー 8 = PDF 3・画像 2・動画 1・コード 2）を作り、
#     全タブを 1 度ずつ表に出して PDF はページを送り・動画は数秒再生して止め、tab1 へ戻して
#     猶予（30 秒）+ 余裕のあとの footprint をカテゴリ別に採る。
#     その後プレビューを 1 本ずつ閉じた差分で、どのプレビューがどれだけ持っていたかを出す（内訳）。
#     判定（新）: ① 見えていないプレビューはすべて released・デコード済み 0 ② footprint が旧より小さい
#                ③ PDF のタブへ戻すと作り直し（restored）が数えられ、手放したページが再ラスタで戻る
#                ④ 動画は一時停止した位置のまま（`tako video status` の position が変わらない）
#     判定（旧）: released_total = 0（A/B の検出力 = 旧では手放さない）
#   terminal 段: ペイン 1 枚あたりの tako 側の持ち分（器 / 直接 × 裏 / 表）を footprint の差で出す
#     （#2009 の追加 (5)。器のペインの tako 側の履歴が tmux と二重になっていないかも `scroll.history` で見る）
#   visual 段: 表に戻したときの見た目と操作（visual-test 節 `preview-memory`。Metal の scene を読み戻す
#     ので画面収録権限は要らない）。新は全相が緑・旧（TAKO_2009_LEGACY=1）は ② で落ちる（検出力）。
#     画像は 出力の置き場/visual-new・visual-legacy へ別々に落とす
#
# 使い方: bash scripts/test-preview-memory-2009.sh [出力の置き場]
#   段の選び方: STAGES="scenario terminal visual"（既定は全部）。ARMS="new legacy"（scenario / visual の腕）
#   結果は 出力の置き場/<腕>.txt に 1 行ずつ残す（置き場を省くと一時 dir に置いて最後に表示する）
#
# 数値は release ビルドで測る（本番と同じ。`TAKO_ISO_PROFILE=release`）。
# 窓は仮想ディスプレイ tako-vd へ出す（`scripts/lib/isolated-gui.sh` の 1 実装。#1141 / #1490）。
# 蓋を閉じた機ではフレームが組まれず描画のメモリが載らないので、ヘルパが面を用意できなければ
# 終了コード 4（未実測）で止まる。器（tmux）の経路を通すため `TAKO_PERSIST=1` で起こす。
# 自動命名は `TAKO_AUTO_RENAME=0` で止める（隔離 GUI でも claude を呼ぶ = #1968 の罠）。
# tmux は隔離ソケット（`TAKO_TMUX_SOCKET`）だけを使い、終わったらそのサーバーだけを落とす。
# 落とすのは自分で起こした pid だけ。素材（PDF・画像・動画・コード）はこの中で作る（要 ffmpeg）。
set -uo pipefail

unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_ORCHESTRATOR_ROLE TAKO_MCP_URL

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PASS=0
FAIL=0
pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
fail() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }

TMP="$(mktemp -d /tmp/t2009-XXXXXX)"
OUT_DIR="${1:-$TMP/out}"
mkdir -p "$OUT_DIR"
SOCKET="t2009-$$"
cleanup() {
  stop_isolated_gui "${ISOLATED_GUI_PID:-}"
  tmux -L "$SOCKET" kill-server 2>/dev/null
  rm -rf "$TMP"
}
trap cleanup EXIT

command -v ffmpeg >/dev/null || { echo "未実測: ffmpeg が無い（素材の動画・PDF のページ画像を作れない）"; exit 4; }
command -v jq >/dev/null || { echo "jq が要る"; exit 1; }

has_stage() { case " ${STAGES:-scenario terminal visual} " in *" $1 "*) return 0 ;; *) return 1 ;; esac; }

export TAKO_ISO_PROFILE="${TAKO_ISO_PROFILE:-release}"
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
# ビルドは HOME を差し替える前に（後で呼ぶとツールチェーンとレジストリを一時 HOME へ取り直す）
isolated_gui_bins || exit 1
# visual 段は visual-test 版の debug（節は feature 付きのビルドにしか無い）。置き場はヘルパの
# 1 か所（ISOLATED_GUI_APP_REL）から debug の値で引く（直書きしない = #1490）
VISUAL_APP="$REPO_ROOT/$(TAKO_ISO_PROFILE=debug TAKO_ISO_APP_REL='' /bin/bash -c '. "$1/scripts/lib/isolated-gui.sh"; printf %s "$ISOLATED_GUI_APP_REL"' _ "$REPO_ROOT")"
if has_stage visual; then
  echo "visual-test 版の tako-app をビルドします…"
  (cd "$REPO_ROOT" && cargo build -p tako-app --features visual-test --quiet) || exit 1
fi

# --- 素材（本番の 8 本に寄せる: 講義スライド風 60 ページ・資料 15 ページ・書類 3 ページ・
#     大きな写真・シート・1080p 動画・コード 2 本）---
FX="$TMP/fx"
mkdir -p "$FX/frames"
echo "素材を作ります…"
ffmpeg -loglevel error -y -f lavfi -i "mandelbrot=s=1920x1080:rate=2" -frames:v 60 -q:v 3 "$FX/frames/m%03d.jpg" || exit 1
ffmpeg -loglevel error -y -f lavfi -i "testsrc2=s=1920x1080:rate=2" -frames:v 15 -q:v 3 "$FX/frames/t%03d.jpg" || exit 1
ffmpeg -loglevel error -y -f lavfi -i "smptehdbars=s=1240x1754:rate=1" -frames:v 3 -q:v 3 "$FX/frames/a%03d.jpg" || exit 1
ffmpeg -loglevel error -y -f lavfi -i "mandelbrot=s=4000x3000" -frames:v 1 "$FX/photo4k.png" || exit 1
ffmpeg -loglevel error -y -f lavfi -i "testsrc2=s=2400x1600" -frames:v 1 -q:v 3 "$FX/sheet.jpg" || exit 1
ffmpeg -loglevel error -y -f lavfi -i "testsrc2=s=1920x1080:rate=30" -f lavfi -i "sine=frequency=440:sample_rate=48000" \
  -t 30 -c:v libx264 -pix_fmt yuv420p -c:a aac -shortest "$FX/clip1080.mp4" || exit 1
for i in $(seq 1 600); do printf 'def func_%d(x):\n    return x * %d\n' "$i" "$i"; done > "$FX/task.py"
for i in $(seq 1 400); do printf 'fn f%d(x: u64) -> u64 { x.wrapping_mul(%d) }\n' "$i" "$i"; done > "$FX/lib.rs"
# JPEG を 1 ページ 1 枚で貼った PDF（外部の PDF 道具に頼らず組む。DCTDecode でそのまま埋める）
python3 - "$FX" <<'PY'
import glob, os, struct, sys
out = sys.argv[1]
def jpeg_size(data):
    i = 2
    while i < len(data):
        if data[i] != 0xFF:
            i += 1
            continue
        marker = data[i + 1]
        if marker in (0xC0, 0xC1, 0xC2):
            h, w = struct.unpack(">HH", data[i + 5:i + 9])
            return w, h
        i += 2 + struct.unpack(">H", data[i + 2:i + 4])[0]
    raise ValueError("SOF が無い")
def write_pdf(name, jpgs):
    objs = {1: b"<< /Type /Catalog /Pages 2 0 R >>"}
    kids = []
    nid = 3
    for path in jpgs:
        data = open(path, "rb").read()
        w, h = jpeg_size(data)
        pw, ph = w / 2, h / 2
        objs[nid] = (b"<< /Type /XObject /Subtype /Image /Width %d /Height %d /ColorSpace /DeviceRGB "
                     b"/BitsPerComponent 8 /Filter /DCTDecode /Length %d >>\nstream\n" % (w, h, len(data))
                     + data + b"\nendstream")
        content = b"q %.1f 0 0 %.1f 0 0 cm /Im0 Do Q" % (pw, ph)
        objs[nid + 1] = b"<< /Length %d >>\nstream\n" % len(content) + content + b"\nendstream"
        objs[nid + 2] = (b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 %.1f %.1f] "
                         b"/Resources << /XObject << /Im0 %d 0 R >> >> /Contents %d 0 R >>"
                         % (pw, ph, nid, nid + 1))
        kids.append(nid + 2)
        nid += 3
    objs[2] = ("<< /Type /Pages /Kids [%s] /Count %d >>"
               % (" ".join(f"{k} 0 R" for k in kids), len(kids))).encode()
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
    open(os.path.join(out, name), "wb").write(pdf)
write_pdf("slides60.pdf", sorted(glob.glob(os.path.join(out, "frames", "m*.jpg"))))
write_pdf("deck15.pdf", sorted(glob.glob(os.path.join(out, "frames", "t*.jpg"))))
write_pdf("memo3.pdf", sorted(glob.glob(os.path.join(out, "frames", "a*.jpg"))))
PY
[ -s "$FX/slides60.pdf" ] || { echo "PDF の素材を作れない"; exit 1; }

export HOME="$TMP/home"
export TAKO_DISCOVERY_DIR="$TMP/disc"
export TAKO_ORCHESTRATOR_DIR="$TMP/orch"
export TAKO_TMUX_SOCKET="$SOCKET"
mkdir -p "$HOME" "$TAKO_DISCOVERY_DIR" "$TAKO_ORCHESTRATOR_DIR"
# 画像・ログに実ユーザー名・ホスト名を写さない（zsh の既定プロンプトは %n@%m を出す）
printf "%s\n" "PROMPT='%1~ %# '" "RPROMPT=''" > "$HOME/.zshrc"
# 本番の窓（内蔵ディスプレイいっぱい）に寄せる
ISOLATED_GUI_BOUNDS=${ISOLATED_GUI_BOUNDS:-0,0,1512,945}

PID=""
t() { "$TAKO_BIN" "$@" 2>/dev/null; }
# footprint をカテゴリ別に MB で 1 行へ畳む（total / MALLOC_LARGE / MALLOC_SMALL+TINY /
# IOAccelerator (graphics) / 所有の物理 unmapped (graphics) / IOSurface）
fp_line() {
  footprint -f bytes "$PID" 2>/dev/null | awk -v label="$1" '
    /Footprint:/ { for (i = 1; i <= NF; i++) if ($i == "Footprint:") tot = $(i + 1) }
    /MALLOC_LARGE$/ { large = $1 }
    /MALLOC_SMALL$/ { small = $1 }
    /MALLOC_TINY$/ { tiny = $1 }
    /IOAccelerator \(graphics\)$/ { accel = $1 }
    /\(unmapped\) \(graphics\)$/ { ugfx = $1 }
    /IOSurface$/ { surf = $1 }
    END {
      mb = 1048576
      printf "%-34s total=%6.1f large=%6.1f small=%6.1f accel=%6.1f unmapped_gfx=%6.1f iosurface=%5.1f\n",
        label, tot / mb, large / mb, (small + tiny) / mb, accel / mb, ugfx / mb, surf / mb
    }'
}
fp_total() { footprint -f bytes "$PID" 2>/dev/null | awk '/Footprint:/ { for (i = 1; i <= NF; i++) if ($i == "Footprint:") print int($(i + 1) / 1048576) }'; }

# 腕ごとに 1 本起こす（データは腕ごとに分ける = 前の腕の layout.json を復元しない）
start_arm() {
  local arm="$1"
  shift
  export TAKO_DATA_DIR="$TMP/d-$arm"
  export TAKO_SESSIONS_FILE="$TMP/s-$arm.yaml" TAKO_PANE_LOG_DIR="$TMP/pl-$arm" TAKO_WORKERS_FILE="$TMP/w-$arm.yaml"
  mkdir -p "$TAKO_DATA_DIR"
  launch_isolated_gui "$TMP/app-$arm.log" TAKO_AUTO_RENAME=0 ${1+"$@"} || exit $?
  PID="$ISOLATED_GUI_PID"
  wait_isolated_gui "$TMP/app-$arm.log" || exit 1
  if [ "$(t list | jq '.tabs | length')" != "1" ]; then
    echo "繋がった先が隔離インスタンスではない。中止する。"
    exit 1
  fi
  t autorename off >/dev/null
}
stop_arm() {
  stop_isolated_gui "$PID"
  PID=""
  tmux -L "$SOCKET" kill-server 2>/dev/null
}

# ペインへ中身を流す（器のペインでは tmux の履歴に積まれる）
fill() { t send --pane "$1" "seq 1 ${LINES_PER_PANE:-3000}; ls -la /usr/bin | head -40" >/dev/null; }

# =====================================================================
# scenario 段
# =====================================================================
run_scenario() {
  local arm="$1"
  shift
  local out="$OUT_DIR/$arm.txt"
  : > "$out"
  log() { echo "$*" | tee -a "$out"; }
  start_arm "$arm" TAKO_PERSIST=1 ${1+"$@"}
  sleep 3
  log "$(fp_line "0 起動直後")"
  local tab1 p1
  tab1=$(t list | jq '.tabs[0].id')
  p1=$(t list | jq '.tabs[0].panes[0].id')
  # 本番（10/13）の 14 タブの形: 端末の本数とプレビュー
  local spec=(
    "5"
    "1"
    "2"
    "1"
    "2 deck15.pdf"
    "2"
    "1"
    "1"
    "3 photo4k.png clip1080.mp4 sheet.jpg"
    "3 task.py"
    "2 lib.rs"
    "2"
    "0 memo3.pdf"
    "3 slides60.pdf"
  )
  local i=0 s n tab first p k f r
  for s in ${spec[@]+"${spec[@]}"}; do
    set -- $s
    n=$1
    shift
    if [ $i -eq 0 ]; then
      first=$p1
    elif [ "$n" = 0 ]; then
      first=""
    else
      r=$(t tab new)
      first=$(echo "$r" | jq .pane)
    fi
    if [ -n "$first" ]; then
      fill "$first"
      k=1
      while [ $k -lt "$n" ]; do
        p=$(t split --pane "$first" --down | tr -dc '0-9')
        fill "$p"
        k=$((k + 1))
      done
    fi
    for f in "$@"; do
      if [ -n "$first" ]; then
        t open "$FX/$f" --pane "$first" --right >/dev/null
      else
        t open "$FX/$f" --pane "$p1" --new-tab >/dev/null
      fi
    done
    i=$((i + 1))
  done
  sleep 3
  t list | jq -r '.tabs[] | .id as $t | .panes[] | select(.preview != null) | "\($t) \(.id) \(.preview.mode) \(.preview.path | split("/") | last)"' > "$TMP/previews-$arm.txt"
  log "$(echo "tabs=$(t list | jq '.tabs | length') terminals=$(t list | jq '[.tabs[].panes[] | select(.preview == null)] | length') vessel=$(t list | jq '[.tabs[].panes[] | select(.preview == null and .tmux_session != null)] | length') previews=$(wc -l < "$TMP/previews-$arm.txt" | tr -d ' ')")"

  # 全タブを 1 度ずつ表に出す（描かせる）。PDF はページを送り、動画は数秒再生して止める
  for tab in $(t list | jq '.tabs[].id'); do
    t tab select "$tab" >/dev/null
    sleep 1.5
  done
  local video_pane="" pdf_tab="" pdf_pane=""
  while read -r tab pane mode name; do
    t tab select "$tab" >/dev/null
    sleep 1.5
    case "$mode" in
      pdf)
        for pg in 2 3 4 5 6 7 8 9 10; do t preview --pane "$pane" --page "$pg" >/dev/null; sleep 0.4; done
        [ "$name" = slides60.pdf ] && { pdf_tab=$tab; pdf_pane=$pane; }
        ;;
      video)
        video_pane=$pane
        t video play --pane "$pane" >/dev/null
        sleep 4
        t video pause --pane "$pane" >/dev/null
        sleep 0.5
        ;;
    esac
  done < "$TMP/previews-$arm.txt"
  local pos_before
  pos_before=$(t video status --pane "$video_pane" | jq -r '.position')
  t tab select "$tab1" >/dev/null
  log "$(fp_line "1 全タブを見た直後（tab1 表）")"
  # 猶予（30 秒）+ 定期更新（2 秒）+ 余裕
  sleep 40
  log "$(fp_line "2 tab1 表のまま 40 秒")"
  local cache
  cache=$(t preview-cache)
  log "preview-cache: $(echo "$cache" | jq -c '{max_mb, effective_mb: (.effective_max_bytes / 1048576 | floor), used_mb: (.used_bytes / 1048576 | floor), visible_mb: (.visible_bytes / 1048576 | floor), hidden_mb: (.hidden_bytes / 1048576 | floor), source_mb: (.source_bytes / 1048576 | floor), released_total, restored_total, legacy}')"
  echo "$cache" | jq -c '.panes[]? | {pane, kind, visible, hidden_secs, decoded_mb: (.decoded_bytes / 1048576 * 10 | floor / 10), source_mb: (.source_bytes / 1048576 * 10 | floor / 10), released, video_player, video_frame_mb: (.video_frame_bytes / 1048576 * 10 | floor / 10)}' | tee -a "$out"

  if [ "$arm" = new ]; then
    local hidden_not_released hidden_decoded
    hidden_not_released=$(echo "$cache" | jq '[.panes[] | select(.visible == false and .released == false)] | length')
    hidden_decoded=$(echo "$cache" | jq '[.panes[] | select(.visible == false) | .decoded_bytes] | add // 0')
    if [ "$hidden_not_released" = 0 ] && [ "$hidden_decoded" = 0 ]; then
      pass "① 見えていないプレビューはすべて手放した（released / デコード済み 0）"
    else
      fail "① 手放していないプレビューが残る（未手放し ${hidden_not_released} 本 / デコード済み ${hidden_decoded} bytes）"
    fi
  else
    if [ "$(echo "$cache" | jq '.released_total')" = 0 ]; then
      pass "旧: 手放さない（released_total = 0）"
    else
      fail "旧なのに手放している（A/B の腕が効いていない）"
    fi
  fi
  echo "$(fp_total)" > "$TMP/total-$arm"

  # ③ PDF のタブへ戻す: 作り直しが数えられ、手放したページが再ラスタで戻る
  if [ -n "$pdf_tab" ]; then
    local src_before t0 t1 restored ok=0
    src_before=$(echo "$cache" | jq ".panes[] | select(.pane == $pdf_pane) | .source_bytes")
    t0=$(python3 -c 'import time; print(time.time())')
    t tab select "$pdf_tab" >/dev/null
    for _ in $(seq 1 200); do
      cache=$(t preview-cache)
      if [ "$(echo "$cache" | jq ".panes[] | select(.pane == $pdf_pane) | .released")" = false ] \
        && [ "$(echo "$cache" | jq ".panes[] | select(.pane == $pdf_pane) | .source_bytes")" -gt "$((src_before * 3))" ]; then
        ok=1
        break
      fi
      sleep 0.1
    done
    t1=$(python3 -c 'import time; print(time.time())')
    restored=$(echo "$cache" | jq '.restored_total')
    log "3 PDF タブへ戻す: source $((src_before / 1048576)) MB → $(( $(echo "$cache" | jq ".panes[] | select(.pane == $pdf_pane) | .source_bytes") / 1048576 )) MB（$(python3 -c "print(round($t1 - $t0, 2))") 秒）restored_total=${restored}"
    if [ "$arm" = new ]; then
      if [ "$ok" = 1 ] && [ "$restored" -ge 1 ]; then
        pass "③ PDF のタブへ戻すと手放したページが再ラスタで戻る"
      else
        fail "③ PDF のページが戻らない（released / source_bytes が戻らない）"
      fi
    fi
    log "$(fp_line "3 PDF タブ表")"
  fi
  # ④ 動画の一時停止位置
  local vtab
  vtab=$(awk '$3=="video"{print $1}' "$TMP/previews-$arm.txt")
  t tab select "$vtab" >/dev/null
  sleep 2
  local pos_after
  pos_after=$(t video status --pane "$video_pane" | jq -r '.position')
  log "4 動画のタブへ戻す: 位置 ${pos_before} → ${pos_after}"
  if python3 -c "import sys; sys.exit(0 if abs(float('$pos_after') - float('$pos_before')) < 0.15 else 1)" 2>/dev/null; then
    pass "④ 動画は一時停止した位置のまま（${pos_before} → ${pos_after}）"
  else
    fail "④ 動画の位置が変わった（${pos_before} → ${pos_after}）"
  fi
  t tab select "$tab1" >/dev/null
  sleep 40

  # 内訳: プレビューを 1 本ずつ閉じた差分（旧の腕で読むのが「修正前に誰がどれだけ持っていたか」）
  log "$(fp_line "5 tab1 表のまま 40 秒（閉じる前）")"
  while read -r tab pane mode name; do
    t close --pane "$pane" >/dev/null
    sleep 3
    log "$(fp_line "6 閉じた: $mode $name")"
  done < "$TMP/previews-$arm.txt"
  stop_arm
}

if has_stage scenario; then
  for arm in ${ARMS:-new legacy}; do
    echo "== scenario: $arm"
    if [ "$arm" = legacy ]; then
      run_scenario legacy TAKO_2009_LEGACY=1
    else
      run_scenario new
    fi
  done
  if [ -f "$TMP/total-new" ] && [ -f "$TMP/total-legacy" ]; then
    new_total=$(cat "$TMP/total-new")
    legacy_total=$(cat "$TMP/total-legacy")
    echo "  footprint（tab1 表のまま 40 秒）: 旧 ${legacy_total} MB → 新 ${new_total} MB（差 $((legacy_total - new_total)) MB）"
    if [ "$new_total" -lt "$legacy_total" ]; then
      pass "② 同じ条件で footprint が旧より小さい（${legacy_total} → ${new_total} MB）"
    else
      fail "② footprint が減らない（旧 ${legacy_total} / 新 ${new_total} MB）"
    fi
  fi
fi

# =====================================================================
# terminal 段: ペイン 1 枚あたりの tako 側の持ち分（器 / 直接 × 裏 / 表）
# =====================================================================
run_terminal() {
  local kind="$1" persist="$2"
  local out="$OUT_DIR/terminal-$kind.txt"
  : > "$out"
  start_arm "term-$kind" TAKO_PERSIST="$persist"
  sleep 4
  local a b c d tabs=() tab r first k p n=0
  a=$(fp_total)
  echo "$(fp_line "$kind 0 1 ペイン")" | tee -a "$out"
  # 裏のタブ 4 枚 × 5 ペイン（表に出さない）
  for _ in 1 2 3 4; do
    r=$(t tab new)
    tab=$(echo "$r" | jq .tab)
    first=$(echo "$r" | jq .pane)
    tabs+=("$tab")
    fill "$first"
    n=$((n + 1))
    for k in 2 3 4 5; do
      p=$(t split --pane "$first" --down | tr -dc '0-9')
      fill "$p"
      n=$((n + 1))
    done
  done
  sleep 8
  b=$(fp_total)
  echo "$(fp_line "$kind 1 裏に ${n} ペイン（未表示）")" | tee -a "$out"
  for tab in ${tabs[@]+"${tabs[@]}"}; do
    t tab select "$tab" >/dev/null
    sleep 2
  done
  c=$(fp_total)
  echo "$(fp_line "$kind 2 全タブを表に出した直後")" | tee -a "$out"
  t tab select "$(t list | jq '.tabs[0].id')" >/dev/null
  sleep 10
  d=$(fp_total)
  echo "$(fp_line "$kind 3 tab1 へ戻して 10 秒")" | tee -a "$out"
  echo "  $kind: 1 ペインあたり 裏（未表示）$(python3 -c "print(round(($b - $a) / $n, 2))") MB / 表に出した後に裏 $(python3 -c "print(round(($d - $a) / $n, 2))") MB（表に出していた間の増分 $(python3 -c "print(round(($c - $b) / $n, 2))") MB）" | tee -a "$out"
  echo "  $kind: tako 側の履歴（行）: $(t list | jq -c '[.tabs[].panes[] | select(.preview == null) | .scroll.history] | {max: max, min: min}') alt_screen: $(t list | jq -c '[.tabs[].panes[] | select(.preview == null) | .scroll.alt_screen] | unique')" | tee -a "$out"
  stop_arm
}

if has_stage terminal; then
  echo "== terminal: 器（tmux）"
  run_terminal vessel 1
  echo "== terminal: 直接"
  run_terminal direct 0
fi

# =====================================================================
# visual 段: 表に戻したときの見た目と操作（visual-test 節 preview-memory）
# =====================================================================
run_visual() {
  local arm="$1"
  shift
  local dump="$OUT_DIR/visual-$arm" log="$TMP/visual-$arm.log" saved_app="$APP_BIN" i
  mkdir -p "$dump"
  export TAKO_DATA_DIR="$TMP/d-visual-$arm"
  export TAKO_SESSIONS_FILE="$TMP/s-visual-$arm.yaml" TAKO_PANE_LOG_DIR="$TMP/pl-visual-$arm" TAKO_WORKERS_FILE="$TMP/w-visual-$arm.yaml"
  mkdir -p "$TAKO_DATA_DIR"
  APP_BIN="$VISUAL_APP"
  launch_isolated_gui "$log" TAKO_PERSIST=1 TAKO_AUTO_RENAME=0 \
    TAKO_VISUAL_TEST=1 TAKO_VISUAL_ONLY=preview-memory TAKO_VISUAL_DUMP_DIR="$dump" ${1+"$@"} || exit $?
  APP_BIN="$saved_app"
  PID="$ISOLATED_GUI_PID"
  for i in $(seq 1 3000); do
    kill -0 "$PID" 2>/dev/null || break
    sleep 0.1
  done
  stop_arm
  grep -E "TAKO_VISUAL_PIXEL: preview-memory|TAKO_VISUAL_DUMP_FILE|TAKO_APP_SELF_TEST_FAILED" "$log" | sed 's/^/    /' | tee "$OUT_DIR/visual-$arm.txt"
  if [ "$arm" = new ]; then
    if grep -q "TAKO_VISUAL_TEST_OK" "$log"; then
      pass "visual: 手放して表に戻した絵が前と同じ・戻した直後に遠いページへ飛べる（TAKO_VISUAL_TEST_OK）"
    else
      fail "visual: 節が緑にならない"
      grep -E "FAILED|panicked|ERROR" "$log" | tail -5 | sed 's/^/    /'
    fi
  else
    if grep -q "TAKO_APP_SELF_TEST_FAILED: visual-test preview-memory: ② 見えていない 2 枚を手放す" "$log"; then
      pass "visual 旧: ② で落ちる（修正前は見えていないプレビューを手放さない = 節に検出力がある）"
    else
      fail "visual 旧: ② で落ちない（節に検出力が無い）"
      grep -E "FAILED|panicked" "$log" | tail -3 | sed 's/^/    /'
    fi
  fi
}

if has_stage visual; then
  for arm in ${ARMS:-new legacy}; do
    echo "== visual: $arm"
    if [ "$arm" = legacy ]; then
      run_visual legacy TAKO_2009_LEGACY=1
    else
      run_visual new
    fi
  done
fi

echo
echo "結果: $OUT_DIR"
for f in "$OUT_DIR"/*.txt; do
  echo "--- $(basename "$f")"
  cat "$f"
done
echo
echo "PASS=$PASS FAIL=$FAIL"
[ "$FAIL" -eq 0 ]
