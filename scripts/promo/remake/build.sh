#!/bin/bash
# build.sh — プロモ動画（リメイク版）を書き出す: 音を合成 → 映像を描く → 音量をそろえて合わせる → 検査。
#
# 前提: record.sh で実画面を収録済み（TAKO_PROMO_OUT/scenes。既定 ~/Movies/tako-promo/remake/scenes）。
# 使い方: bash scripts/promo/remake/build.sh [出力.mp4]
#   既定の出力は ~/Movies/tako-promo/tako-promo-remake-v1.mp4（リポジトリには入れない）
#   確認用のフレーム（等間隔 TAKO_PROMO_CHECK_FRAMES 枚。既定 12）は <出力の隣>/checks/<名前>/ に出す
#
# 重い処理（映像の描画とエンコード）は tako の順番待ちを通す（scripts/lib/queue.sh の run_heavy）。

set -uo pipefail

HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
ROOT=$(cd "$HERE/../../.." && pwd)
BASE=${TAKO_PROMO_OUT:-$HOME/Movies/tako-promo/remake}
OUT=${1:-$HOME/Movies/tako-promo/tako-promo-remake-v1.mp4}
WORK="$BASE/work"
LUFS=${TAKO_PROMO_LUFS:--15}
FRAMES=${TAKO_PROMO_CHECK_FRAMES:-12}

# shellcheck source=../../lib/queue.sh
. "$ROOT/scripts/lib/queue.sh"

die() { printf 'ERROR: %s\n' "$*" >&2; exit 1; }

mkdir -p "$WORK" "$(dirname "$OUT")" || die "出力先を作れない"

echo "== 1/4 音を合成"
node "$HERE/bgm.mjs" "$WORK/bgm.wav" || die "音の合成に失敗"

echo "== 2/4 映像を描く（順番待ちを通す）"
run_heavy node "$HERE/render.mjs" --out "$WORK/video.mp4" || die "映像の書き出しに失敗"

echo "== 3/4 音量をそろえて合わせる（目標 ${LUFS} LUFS）"
# 1 回測ってから固定のゲインを当てる（1 パスの loudnorm は曲の強弱を潰すので使わない）。
# リミッタは level=disabled（既定のままだと絞ったあと 0 dB へ持ち上げ直す）
measured=$(ffmpeg -nostdin -hide_banner -i "$WORK/bgm.wav" -af ebur128 -f null - 2>&1 | awk '/Integrated loudness:/ { found = 1 } found && /I:/ { print $2; exit }')
[ -n "$measured" ] || die "音量を測れない"
gain=$(awk -v t="$LUFS" -v m="$measured" 'BEGIN { printf "%.2f", t - m }')
echo "   測定 ${measured} LUFS → ゲイン ${gain} dB"
ffmpeg -nostdin -v error -y -i "$WORK/video.mp4" -i "$WORK/bgm.wav" \
    -map 0:v -map 1:a -c:v copy \
    -af "volume=${gain}dB,alimiter=limit=0.84:level=disabled" -c:a aac -b:a 192k -ar 48000 \
    -shortest -movflags +faststart "$OUT" || die "映像と音を合わせられない"

echo "== 4/4 検査"
ffprobe -v error -show_entries format=duration,size,bit_rate \
    -show_entries stream=codec_type,codec_name,width,height,r_frame_rate,sample_rate,channels \
    -of default=noprint_wrappers=1 "$OUT"
# 最後まで再生できること: 全フレーム・全音声を実際にデコードして、エラーが 1 行も出ないこと
errors=$(ffmpeg -nostdin -v error -i "$OUT" -f null - 2>&1 | wc -l | tr -d ' ')
echo "デコードの全走査: エラー ${errors} 行"
[ "$errors" = 0 ] || die "デコードでエラーが出た"
ffmpeg -nostdin -hide_banner -i "$OUT" -af ebur128=peak=true -f null - 2>&1 |
    awk '/Integrated loudness:/ { found = 1 } found && (/I:/ || /LRA:/ || /Peak:/) { print "音量:", $0 }'
# 等間隔のフレーム（自分の目で見る用）
name=$(basename "$OUT")
name=${name%.*}
checks="$(dirname "$OUT")/checks/$name"
mkdir -p "$checks" || die "確認用の置き場を作れない"
command rm -f "${checks:?}"/frame-*.png
dur=$(ffprobe -v error -show_entries format=duration -of csv=p=0 "$OUT")
i=1
while [ "$i" -le "$FRAMES" ]; do
    t=$(awk -v d="$dur" -v i="$i" -v n="$FRAMES" 'BEGIN { printf "%.2f", d * (i - 0.5) / n }')
    ffmpeg -nostdin -v error -y -ss "$t" -i "$OUT" -frames:v 1 "$checks/frame-$(printf '%02d' "$i")-${t}s.png" || die "フレームを抜けない: ${t}s"
    i=$((i + 1))
done
echo "確認用のフレーム: ${checks}（${FRAMES} 枚）"
echo "できた: $OUT"
