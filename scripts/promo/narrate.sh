#!/bin/bash
# tako:run: bash scripts/promo/narrate.sh
# 解説動画（#1081）のナレーション音声を台本（explainer-timeline.tsv）から生成する。
#
# 使い方: scripts/promo/narrate.sh [タイムライン tsv] [出力ディレクトリ]
#   既定: scripts/promo/explainer-timeline.tsv → ~/Desktop/tako-promo/audio/narr/
#
# 合成手段は TAKO_PROMO_TTS で切り替える（既定 voicevox）。
#
#   voicevox … VOICEVOX ENGINE（既定・本番）。話者はずんだもん / ノーマル（speaker=3）。
#              **エンジンを先に起動しておくこと**:
#                ~/Desktop/tako-promo/tools/macos-arm64/run --host 127.0.0.1 --port 50021
#              取得手順は .agent/plans/2026-09-youtube-explainer.md「声の選定」にある。
#              生成音声の公開には話者クレジットが必要（「VOICEVOX:ずんだもん」）。
#              表記は voicevox-synth.py --print-credit がエンジンから引く。
#   say      … macOS 同梱の日本語 TTS（`say -v Kyoko`）。v2 まではこれだった。
#              抑揚が狭く棒読みに聞こえるため本番から降りたが、エンジン無しで動く
#              ので A/B と緊急用に残す。
#
# 1 区間 = 1 ファイル（<id>.wav / 48kHz stereo）で書き出し、durations.tsv に秒数を残す。
# build-explainer.sh はこの秒数から各区間の長さを決める（ナレーションが映像を駆動する）。
# 台本を直したら再実行するだけでよい（内容が同じ区間も上書きするが数十秒で終わる）。
#
# **どのバックエンドでも同じピークへそろえる**（TAKO_PROMO_PEAK_DB。既定 -12.3dBFS）。
# build-explainer.sh の音声段は「ナレーションのピークが -12dB 前後」を前提に +9.5dB の
# 持ち上げとリミッタを積んでいるので、そろえないと声が変わるたびに歪む
# （実測: say のピーク中央値 -12.3dB に対し VOICEVOX は -4.0dB = 5.5dB 過大）。
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
# shellcheck source=lib.sh
source "$SCRIPT_DIR/lib.sh"
TSV=${1:-"$SCRIPT_DIR/explainer-timeline.tsv"}
OUT=${2:-"${TAKO_PROMO_OUT:-$HOME/Desktop/tako-promo}/audio/narr"}
TTS=${TAKO_PROMO_TTS:-voicevox}
PEAK_DB=${TAKO_PROMO_PEAK_DB:--12.3}
# say バックエンド用
VOICE=${TAKO_PROMO_VOICE:-Kyoko}
RATE=${TAKO_PROMO_RATE:-180}
# voicevox バックエンド用（既定値と根拠は voicevox-synth.py 側）
VV_URL=${TAKO_PROMO_VV_URL:-http://127.0.0.1:50021}
VV_SPEAKER=${TAKO_PROMO_VV_SPEAKER:-3}

case "$TTS" in
say)
    say -v '?' | grep -q "^${VOICE} " || {
        echo "ERROR: 音声 ${VOICE} が無い（say -v '?' で確認）" >&2; exit 1; }
    backend_desc="macOS say -v ${VOICE} -r ${RATE}"
    ;;
voicevox)
    curl -sf -m 5 "${VV_URL}/version" >/dev/null || {
        echo "ERROR: VOICEVOX ENGINE (${VV_URL}) が応答しない。先に起動する:" >&2
        echo "       ~/Desktop/tako-promo/tools/macos-arm64/run --host 127.0.0.1 --port 50021" >&2
        exit 1; }
    credit=$(/usr/bin/env python3 "$SCRIPT_DIR/voicevox-synth.py" \
        --url "$VV_URL" --speaker "$VV_SPEAKER" --print-credit)
    backend_desc="VOICEVOX ENGINE $(curl -sf "${VV_URL}/version" | tr -d '"') / speaker=${VV_SPEAKER}（${credit}）"
    ;;
*)
    echo "ERROR: 未知の TAKO_PROMO_TTS=${TTS}（voicevox / say のいずれか）" >&2; exit 1
    ;;
esac

mkdir -p "$OUT"
: > "$OUT/durations.tsv"
echo "== ナレーション合成: ${backend_desc}"

# 生の合成結果 $1 を、ピークを PEAK_DB へそろえて $2 へ書く。
# 頭に 0.25 秒の無音を足す（区間の頭で映像が切り替わってから話し始める方が聞きやすい）。
#
# **測るのは最終形（48kHz stereo に変換したあと）**。変換の前に測って持ち上げると、
# ffmpeg の mono → stereo 行列が各チャンネルへ 1/√2（-3.01dB）を掛けるぶんだけ
# 狙いから外れる（実測: 目標 -12.3dB に対し出力は -15.3dB）。
normalize_to_wav() {
    local raw=$1 wav=$2 flat measured delta
    flat="${wav}.flat.wav"
    ffmpeg -nostdin -v error -y -i "$raw" \
        -af "adelay=250|250,apad=pad_dur=0.2" -ar 48000 -ac 2 "$flat"
    measured=$(ffmpeg -nostdin -hide_banner -i "$flat" -af volumedetect -f null - 2>&1 \
        | sed -n 's/.*max_volume: \(-*[0-9.]*\) dB.*/\1/p' | head -1)
    [ -n "$measured" ] || { echo "ERROR: ${flat} のピークを測れない" >&2; return 1; }
    delta=$(/usr/bin/python3 -c "print(f'{${PEAK_DB} - (${measured}):.2f}')")
    ffmpeg -nostdin -v error -y -i "$flat" -af "volume=${delta}dB" "$wav"
    rm -f "$flat"
}

n=0
while IFS=$'\t' read -r id kind source anchor offset min_dur caption subtitle speech; do
    speech=$(promo_tl_field "$speech")
    if [ -z "$speech" ]; then
        printf '%s\t0\n' "$id" >> "$OUT/durations.tsv"
        continue
    fi
    raw="$OUT/$id.raw"; wav="$OUT/$id.wav"
    case "$TTS" in
    say)
        say -v "$VOICE" -r "$RATE" -o "$raw.aiff" "$speech"
        mv "$raw.aiff" "$raw"
        ;;
    voicevox)
        /usr/bin/env python3 "$SCRIPT_DIR/voicevox-synth.py" \
            --url "$VV_URL" --speaker "$VV_SPEAKER" --text "$speech" --out "$raw"
        ;;
    esac
    normalize_to_wav "$raw" "$wav"
    rm -f "$raw"
    dur=$(ffprobe -v error -show_entries format=duration -of csv=p=0 "$wav")
    printf '%s\t%s\n' "$id" "$dur" >> "$OUT/durations.tsv"
    n=$((n + 1))
done < <(promo_timeline_rows "$TSV")

total=$(awk -F'\t' '{s+=$2} END {printf "%.1f", s}' "$OUT/durations.tsv")
echo "== ナレーション ${n} 区間 / 合計 ${total}s → $OUT"
