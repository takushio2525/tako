#!/bin/bash
# tako:run: bash scripts/promo/check-readings.sh
# 台本の全区間について VOICEVOX が実際にどう読むかを一覧で出す（#1081）。
#
# 使い方: scripts/promo/check-readings.sh [タイムライン tsv]
#   --diff  … 読み替え表の適用前 / 適用後が違う区間だけを出す
#
# VOICEVOX ENGINE の POST /audio_query は「エンジンが実際にどう読むか」を kana で返すので、
# **耳で聴かずに読みを点検できる**。声を替えたり台本を直したときはこれを全件読むこと
# （数詞 + 助数詞・英字の略語・同訓異字は grep では拾えない）。
# 誤読が見つかったら reading-overrides.tsv へ 1 行足して、もう一度これを回す。
#
# エンジンを先に起動しておくこと（narrate.sh の先頭コメント参照）。
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
# shellcheck source=lib.sh
source "$SCRIPT_DIR/lib.sh"
TSV=${1:-"$SCRIPT_DIR/explainer-timeline.tsv"}
DIFF_ONLY=0
[ "${1:-}" = "--diff" ] && { DIFF_ONLY=1; TSV="$SCRIPT_DIR/explainer-timeline.tsv"; }
[ "${2:-}" = "--diff" ] && DIFF_ONLY=1
VV_URL=${TAKO_PROMO_VV_URL:-http://127.0.0.1:50021}

curl -sf -m 5 "${VV_URL}/version" >/dev/null || {
    echo "ERROR: VOICEVOX ENGINE (${VV_URL}) が応答しない。先に起動する:" >&2
    echo "       ~/Desktop/tako-promo/tools/macos-arm64/run --host 127.0.0.1 --port 50021" >&2
    exit 1; }

# kana からアクセント記号・無声化・句切りを外して読める形にする
strip_kana() { tr -d "'_/"; }
say_kana() {  # say_kana <本文> [--no-overrides]
    /usr/bin/env python3 "$SCRIPT_DIR/voicevox-synth.py" --print-kana --text "$1" ${2:+"$2"} | strip_kana
}

n=0; changed=0
while IFS=$'\t' read -r id kind source anchor offset min_dur caption subtitle speech; do
    speech=$(promo_tl_field "$speech")
    [ -n "$speech" ] || continue
    n=$((n + 1))
    before=$(say_kana "$speech" --no-overrides)
    after=$(say_kana "$speech")
    if [ "$before" != "$after" ]; then
        changed=$((changed + 1))
        printf '=== [%02d] %s  ** 読み替えあり **\n' "$n" "$id"
        printf '  原文: %s\n  修正前: %s\n  修正後: %s\n' "$speech" "$before" "$after"
    elif [ "$DIFF_ONLY" -eq 0 ]; then
        printf '=== [%02d] %s\n' "$n" "$id"
        printf '  原文: %s\n  読み: %s\n' "$speech" "$after"
    fi
done < <(promo_timeline_rows "$TSV")

echo "== ${n} 区間 / 読み替えが効いた区間 ${changed}"
