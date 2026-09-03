#!/bin/bash
# tako:run: bash scripts/promo/narrate.sh
# 解説動画（#1081）のナレーション音声を台本（explainer-timeline.tsv）から生成する。
#
# 使い方: scripts/promo/narrate.sh [タイムライン tsv] [出力ディレクトリ]
#   既定: scripts/promo/explainer-timeline.tsv → ~/Desktop/tako-promo/audio/narr/
#
# 音声は macOS 同梱の日本語 TTS（`say -v Kyoko`。この機で使える唯一の日本語音声）。
# 1 区間 = 1 ファイル（<id>.wav / 48kHz stereo）で書き出し、durations.tsv に秒数を残す。
# build-explainer.sh はこの秒数から各区間の長さを決める（ナレーションが映像を駆動する）。
# 台本を直したら再実行するだけでよい（内容が同じ区間も上書きするが数十秒で終わる）。
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
TSV=${1:-"$SCRIPT_DIR/explainer-timeline.tsv"}
OUT=${2:-"${TAKO_PROMO_OUT:-$HOME/Desktop/tako-promo}/audio/narr"}
VOICE=${TAKO_PROMO_VOICE:-Kyoko}
RATE=${TAKO_PROMO_RATE:-180}

say -v '?' | grep -q "^${VOICE} " || { echo "ERROR: 音声 ${VOICE} が無い（say -v '?' で確認）" >&2; exit 1; }
mkdir -p "$OUT"
: > "$OUT/durations.tsv"

n=0
while IFS=$'\t' read -r id kind source anchor offset min_dur caption subtitle speech; do
    [ -z "$id" ] && continue
    case "$id" in \#*) continue ;; esac
    if [ -z "$speech" ]; then
        printf '%s\t0\n' "$id" >> "$OUT/durations.tsv"
        continue
    fi
    aiff="$OUT/$id.aiff"; wav="$OUT/$id.wav"
    say -v "$VOICE" -r "$RATE" -o "$aiff" "$speech"
    # 動画の音声トラックと同じ 48kHz stereo にそろえ、頭に 0.25 秒の無音を足す
    # （区間の頭で映像が切り替わってから話し始める方が聞きやすい）
    ffmpeg -v error -y -i "$aiff" -af "adelay=250|250,apad=pad_dur=0.2" -ar 48000 -ac 2 "$wav"
    rm -f "$aiff"
    dur=$(ffprobe -v error -show_entries format=duration -of csv=p=0 "$wav")
    printf '%s\t%s\n' "$id" "$dur" >> "$OUT/durations.tsv"
    n=$((n + 1))
done < "$TSV"

total=$(awk -F'\t' '{s+=$2} END {printf "%.1f", s}' "$OUT/durations.tsv")
echo "== ナレーション ${n} 区間 / 合計 ${total}s → $OUT"
