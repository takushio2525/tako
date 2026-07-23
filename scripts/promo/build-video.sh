#!/bin/bash
# tako 紹介動画 編集・合成スクリプト (#470 Phase B)
#
# record-scenes.sh が撮った素材（~/Desktop/tako-promo/scenes/*-raw.mp4）から
# 台本どおりの区間を切り出し、テロップを載せ、クロスフェードで繋ぎ、BGM を合成する。
#
# 使い方: scripts/promo/build-video.sh [出力パス]
#   既定の出力先は ~/Desktop/tako-promo/tako-intro-v1.mp4
#
# 素材が足りないシーンは警告して飛ばすので、途中まででも通しで確認できる。
# テロップは caption.swift（CoreText）で PNG 化して overlay する
# （本環境の ffmpeg は drawtext を持たないため）。
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
# shellcheck source=lib.sh
source "$SCRIPT_DIR/lib.sh"

OUT=${1:-"$PROMO_OUT/tako-intro-v1.mp4"}
SCENES_DIR="$PROMO_OUT/scenes"
WORK=/private/tmp/tako-promo-build
BGM="$PROMO_OUT/audio/bgm.wav"
W=1920
H=1200
FPS=30
XFADE=0.5   # シーン間クロスフェード秒

CAPTION_BIN=/private/tmp/tako-promo-caption
if [ ! -x "$CAPTION_BIN" ] || [ "$SCRIPT_DIR/caption.swift" -nt "$CAPTION_BIN" ]; then
    swiftc -O -o "$CAPTION_BIN" "$SCRIPT_DIR/caption.swift" || {
        echo "ERROR: caption.swift のコンパイルに失敗" >&2; exit 1; }
fi

# シーン定義: id | 素材ファイル | 素材内の開始秒 | 尺 | テロップ本文 | 副題
# 台本 .agent/plans/2026-07-promo-video.md のシーン表と 1:1 で対応する
SCENES=(
  "s1|agent-raw.mp4|78|6|エージェントも、その子プロセスも、1 つのタブに|"
  "s2|agent-raw.mp4|8|12|Claude Code がそのまま画面を操作する|設定ゼロの内蔵 MCP サーバー"
  "s3|agent-raw.mp4|58|14|master が worker を並べて監視する|1 グループ = 1 タブ"
  "s4|preview-raw.mp4|2|12|成果物はターミナルの中で確認する|ライブリロードと Code Runner"
  "s5a|restore-before-raw.mp4|1|4|再起動しても、全部戻ってくる|"
  "s5b|restore-after-raw.mp4|2|6||画面の中身ごと復元"
  "s6|remote-raw.mp4|2|10|外出先はスマホから見て、答える|"
  "s7|outro-raw.mp4|3|8|tako|AI エージェント時代の GUI ターミナル / github.com/takushio2525/tako"
)

rm -rf "$WORK"; mkdir -p "$WORK"
mkdir -p "$(dirname "$OUT")"

parts=()
missing=()
idx=0
for spec in "${SCENES[@]}"; do
    IFS='|' read -r id src start dur cap sub <<< "$spec"
    if [ ! -f "$SCENES_DIR/$src" ]; then
        missing+=("${id}（${src}）")
        continue
    fi
    # 素材の尺が足りなければ切り出し位置を前に寄せる
    avail=$(ffprobe -v error -show_entries format=duration -of csv=p=0 "$SCENES_DIR/$src")
    avail_i=${avail%.*}
    if [ "$((start + dur))" -gt "$avail_i" ]; then
        start=$(( avail_i - dur ))
        [ "$start" -lt 0 ] && start=0
    fi

    seg="$WORK/$(printf '%02d' "$idx")-$id.mp4"
    filter="scale=${W}:${H}:force_original_aspect_ratio=decrease,pad=${W}:${H}:(ow-iw)/2:(oh-ih)/2:color=0x0d1117,fps=${FPS},setsar=1"

    if [ -n "$cap" ] || [ -n "$sub" ]; then
        png="$WORK/$id-cap.png"
        "$CAPTION_BIN" "$png" "$W" 60 "$cap" "$sub"
        # テロップは下寄せ。0.4s でフェードイン、終了 0.6s 前からフェードアウト
        fo_start=$(/usr/bin/python3 -c "print(max(0.0, $dur - 1.0))")
        # PNG は -loop 1 + -framerate でストリーム化する。framerate を与えないと
        # PTS が進まず fade（時間ベース）が一切効かない
        ffmpeg -v error -ss "$start" -t "$dur" -i "$SCENES_DIR/$src" \
            -loop 1 -framerate "$FPS" -t "$dur" -i "$png" \
            -filter_complex "[0:v]${filter}[bg];[1:v]format=rgba,setpts=PTS-STARTPTS,fade=t=in:st=0.3:d=0.5:alpha=1,fade=t=out:st=${fo_start}:d=0.6:alpha=1[cap];[bg][cap]overlay=0:H-h-90:format=auto[v]" \
            -map "[v]" -an -c:v libx264 -preset medium -crf 18 -pix_fmt yuv420p -y "$seg"
    else
        ffmpeg -v error -ss "$start" -t "$dur" -i "$SCENES_DIR/$src" \
            -vf "$filter" -an -c:v libx264 -preset medium -crf 18 -pix_fmt yuv420p -y "$seg"
    fi
    parts+=("$seg")
    idx=$((idx + 1))
    echo "   シーン $id: ${dur}s（$src @ ${start}s）"
done

[ "${#parts[@]}" -gt 0 ] || { echo "ERROR: 使える素材が 1 本もない" >&2; exit 1; }
if [ "${#missing[@]}" -gt 0 ]; then
    echo "!! 素材が無いシーン: ${missing[*]}" >&2
    echo "!! （そのシーンを飛ばして繋ぎます）" >&2
fi

# ── 連結（クロスフェード）───────────────────────────────────────────
video="$WORK/video.mp4"
if [ "${#parts[@]}" -eq 1 ]; then
    cp "${parts[0]}" "$video"
else
    inputs=(); fc=""; prev="[0:v]"; offset=0
    for i in "${!parts[@]}"; do inputs+=(-i "${parts[$i]}"); done
    for i in $(seq 1 $(( ${#parts[@]} - 1 ))); do
        d=$(ffprobe -v error -show_entries format=duration -of csv=p=0 "${parts[$((i-1))]}")
        offset=$(/usr/bin/python3 -c "print(round($offset + $d - $XFADE, 3))")
        out="[x$i]"
        [ "$i" -eq $(( ${#parts[@]} - 1 )) ] && out="[vout]"
        fc+="${prev}[${i}:v]xfade=transition=fade:duration=${XFADE}:offset=${offset}${out};"
        prev="$out"
    done
    fc=${fc%;}
    ffmpeg -v error "${inputs[@]}" -filter_complex "$fc" -map "[vout]" \
        -c:v libx264 -preset medium -crf 18 -pix_fmt yuv420p -y "$video"
fi

DUR=$(ffprobe -v error -show_entries format=duration -of csv=p=0 "$video")
echo "   映像尺: ${DUR}s"

# ── BGM 合成 ──────────────────────────────────────────────────────
if [ -f "$BGM" ]; then
    fade_start=$(/usr/bin/python3 -c "print(max(0.0, $DUR - 2.5))")
    ffmpeg -v error -i "$video" -i "$BGM" \
        -filter_complex "[1:a]atrim=0:${DUR},afade=t=in:st=0:d=1.5,afade=t=out:st=${fade_start}:d=2.5,volume=0.85[a]" \
        -map 0:v -map "[a]" -c:v copy -c:a aac -b:a 192k -shortest -y "$OUT"
else
    echo "!! BGM が無い（${BGM}）。無音で書き出します" >&2
    cp "$video" "$OUT"
fi

echo "== 完成: $OUT"
ffprobe -v error -show_entries format=duration,size -show_entries stream=codec_type,codec_name,width,height \
    -of default=nw=1 "$OUT"
