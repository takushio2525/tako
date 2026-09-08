#!/bin/bash
# tako:run: bash scripts/promo/annotate-clicks.sh
# 収録素材へ「ポインタ・クリック波紋・ボタンのハイライト」を焼き込む（Issue #1081 の GUI モード章）。
#
# なぜ要るか:
#   ① `screencapture -l<windowID>`（ウインドウ単体キャプチャ）は**カーソルを写さない**。
#      手順型のデモは「どのボタンを押したか」が本題なので、押した位置が見えないと成立しない
#   ② スターター画面のようにボタンを説明するだけの区間は絵が動かず、`promo_verify` の
#      「素材がほとんど動いていない」検査と、完成動画の「15 秒以上の静止なし」に引っかかる。
#      ポインタとハイライトが動くと、説明している対象がフレームでも分かる
#
# 使い方:
#   annotate-clicks.sh <入力 mp4> <clicks.tsv> <出力 mp4>
#
# clicks.tsv（タブ区切り・`#` 始まりはコメント）:
#   click <t> <x> <y>                  … t 秒に (x,y) を押した。ポインタが寄ってきて波紋が出る
#   hi    <t> <x> <y> <w> <h> <dur>    … t 秒から dur 秒、その矩形を枠で囲む
#   move  <t> <x> <y>                  … 押さずにポインタだけそこへ移す（入力欄へ打つ前など）
#   座標は**収録フレームのピクセル**（1920x1080 なら左上原点でその値）
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
IN=${1:-}
TSV=${2:-}
OUT=${3:-}
[ -n "$IN" ] && [ -n "$TSV" ] && [ -n "$OUT" ] || {
    echo "usage: $0 <in.mp4> <clicks.tsv> <out.mp4>" >&2; exit 2; }
[ -f "$IN" ] || { echo "ERROR: 入力が無い: $IN" >&2; exit 1; }
[ -f "$TSV" ] || { echo "ERROR: clicks.tsv が無い: $TSV" >&2; exit 1; }

ANNOT_DIR=${TAKO_PROMO_ANNOT_DIR:-/private/tmp/tako-promo-annot}
POINTER_BIN=/private/tmp/tako-promo-pointer
if [ ! -x "$POINTER_BIN" ] || [ "$SCRIPT_DIR/pointer.swift" -nt "$POINTER_BIN" ]; then
    swiftc -O -o "$POINTER_BIN" "$SCRIPT_DIR/pointer.swift" || {
        echo "ERROR: pointer.swift のコンパイルに失敗" >&2; exit 1; }
fi
POINTER_META=$("$POINTER_BIN" "$ANNOT_DIR" --scale "${TAKO_PROMO_ANNOT_SCALE:-2}")
echo "$POINTER_META"

DUR=$(ffprobe -v error -select_streams v:0 -show_entries stream=duration -of default=nw=1:nk=1 "$IN")

# フィルタグラフの生成は分岐が多いので python で組む（bash の算術では書ききれない）
FILTER=$(/usr/bin/python3 - "$TSV" "$POINTER_META" <<'PY'
import sys

tsv, meta = sys.argv[1], sys.argv[2]
pw = ph = hx = hy = 0
for line in meta.splitlines():
    parts = line.split()
    if parts and parts[0] == "pointer":
        pw, ph, hx, hy = (int(v) for v in parts[1:5])

clicks, highlights, moves = [], [], []
with open(tsv, encoding="utf-8") as f:
    for raw in f:
        line = raw.strip()
        if not line or line.startswith("#"):
            continue
        cols = line.split("\t")
        kind = cols[0]
        if kind in ("click", "move"):
            t, x, y = float(cols[1]), float(cols[2]), float(cols[3])
            (clicks if kind == "click" else moves).append((t, x, y))
        elif kind == "hi":
            t, x, y, w, h, dur = (float(v) for v in cols[1:7])
            highlights.append((t, x, y, w, h, dur))

# ポインタの経路: 「押す / 移す」の点を時刻順に並べ、1 つ前の点から寄ってくる
stops = sorted(clicks + moves, key=lambda s: s[0])
APPROACH = 1.5   # 寄り始めてから到着までの秒数
ARRIVE = 0.45    # 到着から押すまでの余裕
LINGER = 1.1     # 押したあとポインタが残る秒数

segments = []
prev = None
for t, x, y in stops:
    ts = t - APPROACH
    t0 = t - ARRIVE
    te = t + LINGER
    # 直前の停止点から寄る。無ければ画面の少し外から
    if prev is None:
        x0, y0 = x - 260, y + 210
    else:
        x0, y0 = prev
        # 前の停止からの余韻が終わる前に次が来るなら、そこから続けて動く
        ts = max(ts, prev_te - 0.35)
        t0 = max(t0, ts + 0.25)
    segments.append((ts, t0, te, x0, y0, x, y))
    prev = (x, y)
    prev_te = te

lines = []
n = len(segments)
if n:
    lines.append("[1:v]split=%d%s" % (n, "".join("[p%d]" % i for i in range(n))))
    for ring in (1, 2, 3):
        if clicks:
            lines.append(
                "[%d:v]split=%d%s" % (ring + 1, len(clicks),
                                      "".join("[r%d_%d]" % (ring, i) for i in range(len(clicks)))))

cur = "[0:v]"
step = 0

# ハイライト枠は drawbox（重ねる画像が要らない）
for t, x, y, w, h, dur in highlights:
    nxt = "[hb%d]" % step
    lines.append(
        "%sdrawbox=x=%d:y=%d:w=%d:h=%d:color=0x6b9dff@0.95:t=4:"
        "enable='between(t,%.2f,%.2f)'%s" % (cur, int(x), int(y), int(w), int(h), t, t + dur, nxt))
    cur = nxt
    step += 1

# 波紋（押した瞬間に 3 枚を順に）
for i, (t, x, y) in enumerate(clicks):
    for ring in (1, 2, 3):
        nxt = "[rg%d]" % step
        # 波紋画像は中心をクリック点に合わせる（画像サイズは ring の実寸を使う）
        lines.append(
            "%s[r%d_%d]overlay=x=%d-overlay_w/2:y=%d-overlay_h/2:"
            "enable='between(t,%.2f,%.2f)'%s"
            % (cur, ring, i, int(x), int(y), t + (ring - 1) * 0.15, t + ring * 0.15, nxt))
        cur = nxt
        step += 1

# ポインタ（寄ってきて留まる）
for i, (ts, t0, te, x0, y0, x, y) in enumerate(segments):
    nxt = "[pt%d]" % step
    prog = "min(1\\,max(0\\,(t-%.2f)/%.2f))" % (ts, max(t0 - ts, 0.2))
    xe = "%d+(%d)*%s-%d" % (int(x0), int(x - x0), prog, hx)
    ye = "%d+(%d)*%s-%d" % (int(y0), int(y - y0), prog, hy)
    lines.append(
        "%s[p%d]overlay=x='%s':y='%s':enable='between(t,%.2f,%.2f)'%s"
        % (cur, i, xe, ye, ts, te, nxt))
    cur = nxt
    step += 1

lines.append("%snull[vout]" % cur)
print(";".join(lines))
PY
)

echo "-- クリック注釈: $(grep -cv '^\s*\(#.*\)\?$' "$TSV") 件 / 尺 ${DUR}s"
rm -f "$OUT"
ffmpeg -nostdin -hide_banner -loglevel error \
    -i "$IN" \
    -loop 1 -i "$ANNOT_DIR/pointer.png" \
    -loop 1 -i "$ANNOT_DIR/ring1.png" \
    -loop 1 -i "$ANNOT_DIR/ring2.png" \
    -loop 1 -i "$ANNOT_DIR/ring3.png" \
    -filter_complex "$FILTER" -map "[vout]" \
    -t "$DUR" -c:v libx264 -preset medium -crf 18 -pix_fmt yuv420p -r 30 "$OUT"
echo "-- 出力: $OUT"
ffprobe -v error -select_streams v:0 -show_entries stream=width,height,duration,nb_frames \
    -of default=nw=1 "$OUT"
