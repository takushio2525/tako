#!/bin/bash
# tako:run: bash scripts/promo/build-explainer.sh
# tako 解説動画（#1081）の合成: タイムライン（explainer-timeline.tsv）どおりに
# 素材（scenes/*-raw.mp4）を切り出し、章カード・テロップ・ナレーション・BGM を載せて 1 本にする。
#
# 使い方: scripts/promo/build-explainer.sh [出力パス]
#   既定の出力先は ~/Desktop/tako-promo/tako-explainer-v1.mp4
# 前提:
#   scripts/promo/record-explainer.sh all   … 素材と <scene>-beats.tsv
#   scripts/promo/record-pwa.cjs            … pwa-raw.mp4
#   scripts/promo/narrate.sh                … audio/narr/<id>.wav + durations.tsv
#   TAKO_BGM_TOTAL=660 TAKO_BGM_PROFILE=explainer scripts/promo/make-bgm.py audio/bgm-explainer.wav
#
# ffmpeg は既定で stdin を読む（対話コマンド）。while-read ループの中で呼ぶと tsv の次の行を
# 食ってしまう（実測: 行が丸ごと消えた）ので、すべて -nostdin で呼ぶ。
# 区間の長さは max(min_dur, ナレーション秒 + 0.8)。素材が足りない区間は最後のフレームを
# 伸ばして尺を保つ（tpad）。in 点は <scene>-beats.tsv のビート名 + offset で決める。
# 音声は区間開始に合わせてナレーションを置き、BGM はナレーション中だけ自動で下げる（sidechain）。
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
# shellcheck source=lib.sh
source "$SCRIPT_DIR/lib.sh"

OUT=${1:-"$PROMO_OUT/tako-explainer-v1.mp4"}
TSV=${TAKO_PROMO_TIMELINE:-"$SCRIPT_DIR/explainer-timeline.tsv"}
SCENES_DIR="$PROMO_OUT/scenes"
NARR_DIR="$PROMO_OUT/audio/narr"
BGM="$PROMO_OUT/audio/bgm-explainer.wav"
WORK=/private/tmp/tako-promo-explainer-build
W=1920; H=1080; FPS=30
PAD_AFTER_SPEECH=${TAKO_PROMO_PAD:-0.8}
CAPTION_FONT_PX=52

CAPTION_BIN=/private/tmp/tako-promo-caption
TITLE_BIN=/private/tmp/tako-promo-titlecard
for pair in "caption.swift:$CAPTION_BIN" "titlecard.swift:$TITLE_BIN"; do
    src=${pair%%:*}; bin=${pair##*:}
    if [ ! -x "$bin" ] || [ "$SCRIPT_DIR/$src" -nt "$bin" ]; then
        swiftc -O -o "$bin" "$SCRIPT_DIR/$src" || { echo "ERROR: $src のコンパイルに失敗" >&2; exit 1; }
    fi
done
[ -f "$NARR_DIR/durations.tsv" ] || { echo "ERROR: ナレーションが無い。先に scripts/promo/narrate.sh を実行" >&2; exit 1; }

rm -rf "$WORK"; mkdir -p "$WORK"
mkdir -p "$(dirname "$OUT")"

# ビート名 → 秒（無ければ数値として解釈）
beat_time() {
    local scene=$1 anchor=$2
    local f="$SCENES_DIR/$scene-beats.tsv"   # local は語の展開が先なので 1 行にまとめない（set -u で落ちる）
    if [[ "$anchor" =~ ^-?[0-9]+(\.[0-9]+)?$ ]]; then echo "$anchor"; return; fi
    [ -f "$f" ] || { echo "ERROR: ビート表が無い: $f" >&2; return 1; }
    awk -F'\t' -v n="$anchor" '$1==n {print $2; found=1; exit} END {if (!found) exit 1}' "$f" \
        || { echo "ERROR: ビート $anchor が $f に無い" >&2; return 1; }
}
narr_dur() { awk -F'\t' -v id="$1" '$1==id {print $2; exit}' "$NARR_DIR/durations.tsv"; }
fnum() { /usr/bin/python3 -c "print(f'{$1:.3f}')"; }

parts=(); ids=(); starts=(); durs=(); missing=()
idx=0; cursor=0
while IFS=$'\t' read -r id kind source anchor offset min_dur caption subtitle speech; do
    source=$(promo_tl_field "$source"); caption=$(promo_tl_field "$caption")
    subtitle=$(promo_tl_field "$subtitle"); speech=$(promo_tl_field "$speech")
    nd=$(narr_dur "$id"); nd=${nd:-0}
    dur=$(/usr/bin/python3 -c "print(f'{max(float($min_dur), float($nd) + float($PAD_AFTER_SPEECH) if float($nd) > 0 else float($min_dur)):.3f}')")
    seg="$WORK/$(printf '%02d' "$idx")-$id.mp4"
    fo_start=$(/usr/bin/python3 -c "print(max(0.0, $dur - 0.7))")
    case "$kind" in
    card)
        png="$WORK/$id-card.png"
        "$TITLE_BIN" "$png" "$W" "$H" "$caption" "$subtitle" "$source" \
            "github.com/takushio2525/tako  /  tako-docs.pages.dev"
        ffmpeg -nostdin -v error -y -loop 1 -framerate "$FPS" -t "$dur" -i "$png" \
            -vf "format=yuv420p,fade=t=in:st=0:d=0.5,fade=t=out:st=${fo_start}:d=0.6" \
            -c:v libx264 -preset medium -crf 18 -pix_fmt yuv420p -r "$FPS" "$seg"
        ;;
    clip)
        src="$SCENES_DIR/$source-raw.mp4"
        if [ ! -f "$src" ]; then missing+=("${id}（${source}）"); continue; fi
        if ! t=$(beat_time "$source" "$anchor"); then
            missing+=("${id}（${source}: ビート ${anchor}）"); continue
        fi
        start=$(/usr/bin/python3 -c "print(max(0.0, float($t) + float($offset)))")
        avail=$(ffprobe -v error -show_entries format=duration -of csv=p=0 "$src")
        # 素材の残り尺が足りない分は最後のフレームで保つ（tpad）。in 点は動かさない
        vf="tpad=stop_mode=clone:stop_duration=900,trim=duration=${dur},setpts=PTS-STARTPTS,scale=${W}:${H}:force_original_aspect_ratio=decrease,pad=${W}:${H}:(ow-iw)/2:(oh-ih)/2:color=0x0d1117,fps=${FPS},setsar=1"
        if [ -n "$caption" ] || [ -n "$subtitle" ]; then
            png="$WORK/$id-cap.png"
            "$CAPTION_BIN" "$png" "$W" "$CAPTION_FONT_PX" "$caption" "$subtitle"
            ffmpeg -nostdin -v error -y -ss "$start" -i "$src" \
                -loop 1 -framerate "$FPS" -t "$dur" -i "$png" \
                -filter_complex "[0:v]${vf}[bg];[1:v]format=rgba,setpts=PTS-STARTPTS,fade=t=in:st=0.25:d=0.45:alpha=1,fade=t=out:st=${fo_start}:d=0.5:alpha=1[cap];[bg][cap]overlay=0:H-h-64:format=auto,format=yuv420p[v]" \
                -map "[v]" -an -t "$dur" -c:v libx264 -preset medium -crf 18 -pix_fmt yuv420p -r "$FPS" "$seg"
        else
            ffmpeg -nostdin -v error -y -ss "$start" -i "$src" -vf "$vf,format=yuv420p" \
                -an -t "$dur" -c:v libx264 -preset medium -crf 18 -pix_fmt yuv420p -r "$FPS" "$seg"
        fi
        avail_i=${avail%.*}
        if /usr/bin/python3 -c "import sys; sys.exit(0 if float($start) + float($dur) > float($avail) + 0.5 else 1)"; then
            echo "   !! $id: 素材の末尾を超えている（${start}s + ${dur}s > ${avail}s）。末尾フレームで補う" >&2
        fi
        ;;
    *) echo "ERROR: 不明な kind: $kind（$id）" >&2; exit 1 ;;
    esac
    # 実際にエンコードされた長さを採用する（フレーム丸めで台本値と 1/30 秒ずれうる）
    real=$(ffprobe -v error -show_entries format=duration -of csv=p=0 "$seg")
    parts+=("$seg"); ids+=("$id"); starts+=("$cursor"); durs+=("$real")
    echo "   $(printf '%-12s' "$id") ${kind}  start=$(fnum "$cursor")s  dur=$(fnum "$real")s  narr=${nd}s"
    cursor=$(/usr/bin/python3 -c "print($cursor + $real)")
    idx=$((idx + 1))
done < <(promo_timeline_rows "$TSV")

[ "${#parts[@]}" -gt 0 ] || { echo "ERROR: 区間が 1 つも作れない" >&2; exit 1; }
if [ "${#missing[@]}" -gt 0 ]; then
    echo "!! 素材が無い区間（飛ばした）: ${missing[*]}" >&2
fi

# ── 映像の連結（同一パラメータで作った区間なので copy で繋ぐ）───────────
list="$WORK/concat.txt"; : > "$list"
for p in "${parts[@]}"; do printf "file '%s'\n" "$p" >> "$list"; done
video="$WORK/video.mp4"
ffmpeg -nostdin -v error -y -f concat -safe 0 -i "$list" -c copy "$video"
VDUR=$(ffprobe -v error -show_entries format=duration -of csv=p=0 "$video")
echo "   映像尺: ${VDUR}s（${#parts[@]} 区間）"

# ── 音声: ナレーションを区間の頭に置き、BGM をその下に敷く ────────────
fc="$WORK/audio.filter"; : > "$fc"
inputs=(); n=0; mix=""
for i in "${!ids[@]}"; do
    wav="$NARR_DIR/${ids[$i]}.wav"
    [ -f "$wav" ] || continue
    ms=$(/usr/bin/python3 -c "print(int(round(${starts[$i]} * 1000)))")
    inputs+=(-i "$wav")
    printf '[%d:a]adelay=%d|%d[n%d];\n' "$n" "$ms" "$ms" "$n" >> "$fc"
    mix+="[n$n]"; n=$((n + 1))
done
[ "$n" -gt 0 ] || { echo "ERROR: ナレーション wav が 1 つも無い" >&2; exit 1; }
# say の出力はピークが -13dB 程度と小さい（実測）ので +7dB 持ち上げる（クリップは limiter で防ぐ）
printf '%samix=inputs=%d:normalize=0:dropout_transition=0,volume=2.2,alimiter=limit=0.95,apad=whole_dur=%s[narr];\n' "$mix" "$n" "$VDUR" >> "$fc"
narr_only="$WORK/narr.wav"
ffmpeg -nostdin -v error -y "${inputs[@]}" -filter_complex_script "$fc" -map "[narr]" -t "$VDUR" -ar 48000 -ac 2 "$narr_only"

if [ -f "$BGM" ]; then
    fade_start=$(/usr/bin/python3 -c "print(max(0.0, $VDUR - 3.0))")
    # BGM は薄く（-16dB 相当）。ナレーション中はさらに sidechain で下げる
    ffmpeg -nostdin -v error -y -i "$video" -i "$narr_only" -stream_loop -1 -i "$BGM" \
        -filter_complex "[2:a]atrim=0:${VDUR},asetpts=PTS-STARTPTS,volume=0.20,afade=t=in:st=0:d=2,afade=t=out:st=${fade_start}:d=3[bgm];[bgm][1:a]sidechaincompress=threshold=0.015:ratio=8:attack=40:release=700:makeup=1[duck];[1:a][duck]amix=inputs=2:normalize=0:dropout_transition=0[a]" \
        -map 0:v -map "[a]" -c:v copy -c:a aac -ar 48000 -b:a 192k -movflags +faststart -shortest "$OUT"
else
    echo "!! BGM が無い（$BGM）。ナレーションのみで書き出す" >&2
    ffmpeg -nostdin -v error -y -i "$video" -i "$narr_only" -map 0:v -map 1:a -c:v copy -c:a aac -ar 48000 -b:a 192k -movflags +faststart -shortest "$OUT"
fi

# 章のタイムスタンプ（YouTube 説明文用）を区間表から出す
chap="$WORK/chapters.txt"; : > "$chap"
for i in "${!ids[@]}"; do
    case "${ids[$i]}" in
    op_card|c*_card|outro_card)
        s=${starts[$i]}
        /usr/bin/python3 -c "s=int(round($s)); print(f'{s//60:02d}:{s%60:02d}  ${ids[$i]}')" >> "$chap"
        ;;
    esac
done
cp "$chap" "$PROMO_OUT/tako-explainer-chapters.txt"
echo "== 完成: $OUT"
ffprobe -v error -show_entries format=duration,size -show_entries stream=codec_type,codec_name,width,height,r_frame_rate,sample_rate,channels \
    -of default=nw=1 "$OUT"
echo "-- 章のタイムスタンプ: $PROMO_OUT/tako-explainer-chapters.txt"
cat "$chap"
