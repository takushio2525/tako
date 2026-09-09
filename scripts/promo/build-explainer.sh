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
# v6（#1081）のスライド PNG。scripts/promo/render-slides.mjs が HTML から描く
SLIDES_DIR=${TAKO_PROMO_SLIDES:-"$PROMO_OUT/slides"}
# v6 は別台本なのでナレーションも別ディレクトリ（v4 の 57 本を壊さない）
NARR_DIR=${TAKO_PROMO_NARR:-"$PROMO_OUT/audio/narr"}
BGM="$PROMO_OUT/audio/bgm-explainer.wav"
# ナレーションの声のクレジット（VOICEVOX 利用規約）。エンジンが応答すればそこから引き、
# 応答しなければ環境変数の値を使う（合成済みの wav から再合成せずに組めるようにする）。
# say バックエンドで作った音声にはクレジット義務が無いので TAKO_PROMO_VOICE_CREDIT= で空にする。
VOICE_CREDIT=${TAKO_PROMO_VOICE_CREDIT-$(
    /usr/bin/env python3 "$SCRIPT_DIR/voicevox-synth.py" --print-credit 2>/dev/null || true
)}
# BGM は make-bgm.py が波形から合成した自作音源（外部素材ではないのでクレジット不要）
WORK=/private/tmp/tako-promo-explainer-build
W=1920; H=1080; FPS=30
PAD_AFTER_SPEECH=${TAKO_PROMO_PAD:-0.8}
CAPTION_FONT_PX=${TAKO_PROMO_CAPTION_PX:-52}

CAPTION_BIN=/private/tmp/tako-promo-caption
TITLE_BIN=/private/tmp/tako-promo-titlecard
for pair in "caption.swift:$CAPTION_BIN" "titlecard.swift:$TITLE_BIN"; do
    src=${pair%%:*}; bin=${pair##*:}
    if [ ! -x "$bin" ] || [ "$SCRIPT_DIR/$src" -nt "$bin" ]; then
        swiftc -O -o "$bin" "$SCRIPT_DIR/$src" || { echo "ERROR: $src のコンパイルに失敗" >&2; exit 1; }
    fi
done
# ナレーション無しで組む構成（#1284 の X 向けショート = 無音再生前提で字幕が意味を運ぶ）は
# TAKO_PROMO_NO_NARR=1 を立てる。既定は今までどおりナレーション必須。
NO_NARR=${TAKO_PROMO_NO_NARR:-0}
if [ "$NO_NARR" != 1 ]; then
    [ -f "$NARR_DIR/durations.tsv" ] || { echo "ERROR: ナレーションが無い。先に scripts/promo/narrate.sh を実行" >&2; exit 1; }
fi

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
narr_dur() {
    [ -f "$NARR_DIR/durations.tsv" ] || return 0
    awk -F'\t' -v id="$1" '$1==id {print $2; exit}' "$NARR_DIR/durations.tsv"
}
fnum() { /usr/bin/python3 -c "print(f'{$1:.3f}')"; }

# ── 前提の検査（エンコードの前に落ちる）─────────────────────────────
# **区間を飛ばしたまま完成品を作らない**（#1081）。飛ばしても警告 1 行で先へ進む形だと、
# 素材やビート表が欠けた回に「尺だけ短い、それらしい動画」が出来てしまう
# （2026-09-09 実測: 中断したテイクが `<scene>-beats.tsv` を空にしていたので、
# そのまま組めば GUI 章の 11 区間が丸ごと落ちた動画になっていた）。
# 制作中に部分ビルドをしたいときだけ `TAKO_PROMO_ALLOW_MISSING=1` を付ける
preflight=()
while IFS=$'\t' read -r id kind source anchor offset min_dur caption subtitle speech; do
    source=$(promo_tl_field "$source")
    # スライド（v6）は HTML から描いた PNG が要る。描き忘れたまま組まない
    if [ "$kind" = slide ]; then
        [ -f "$SLIDES_DIR/$source.png" ] || preflight+=("${id}: スライド ${source}.png が無い")
        continue
    fi
    [ "$kind" = clip ] || continue
    if [ ! -f "$SCENES_DIR/$source-raw.mp4" ]; then
        preflight+=("${id}: 素材 ${source}-raw.mp4 が無い"); continue
    fi
    beat_time "$source" "$anchor" >/dev/null 2>&1 \
        || preflight+=("${id}: ビート ${anchor} が ${source}-beats.tsv に無い")
done < <(promo_timeline_rows "$TSV")
if [ "${#preflight[@]}" -gt 0 ]; then
    printf '!! %s\n' "${preflight[@]}" >&2
    if [ "${TAKO_PROMO_ALLOW_MISSING:-0}" != 1 ]; then
        echo "ERROR: ${#preflight[@]} 区間の前提が欠けている。飛ばしたまま完成品は作らない" >&2
        echo "       （制作中に部分ビルドをしたいときは TAKO_PROMO_ALLOW_MISSING=1）" >&2
        exit 1
    fi
    echo "!! TAKO_PROMO_ALLOW_MISSING=1: ${#preflight[@]} 区間を飛ばして続ける" >&2
fi

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
        # 末尾カードだけは脚注に音声素材のクレジットを載せる。VOICEVOX の利用規約は
        # 生成音声の公開に話者クレジットの表示を求めるので、ここと説明文の 2 か所に置く
        # （表記はエンジンから引いた値。narrate.sh / voicevox-synth.py と同じ 1 実装）。
        card_footer="github.com/takushio2525/tako  /  tako-docs.pages.dev"
        if [ "$id" = "outro_card" ] && [ -n "$VOICE_CREDIT" ]; then
            card_footer="${card_footer}    音声: ${VOICE_CREDIT}"
        fi
        "$TITLE_BIN" "$png" "$W" "$H" "$caption" "$subtitle" "$source" "$card_footer"
        ffmpeg -nostdin -v error -y -loop 1 -framerate "$FPS" -t "$dur" -i "$png" \
            -vf "format=yuv420p,fade=t=in:st=0:d=0.5,fade=t=out:st=${fo_start}:d=0.6" \
            -c:v libx264 -preset medium -crf 18 -pix_fmt yuv420p -r "$FPS" "$seg"
        ;;
    slide)
        # HTML から描いた 1920x1080 の PNG をそのまま尺ぶん流す。
        # 文字はスライド自身が持っているのでテロップは重ねない（v6）
        png="$SLIDES_DIR/$source.png"
        ffmpeg -nostdin -v error -y -loop 1 -framerate "$FPS" -t "$dur" -i "$png" \
            -vf "scale=${W}:${H}:force_original_aspect_ratio=decrease,pad=${W}:${H}:(ow-iw)/2:(oh-ih)/2:color=0x0d1117,setsar=1,format=yuv420p,fade=t=in:st=0:d=0.4,fade=t=out:st=${fo_start}:d=0.5" \
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
            # テロップは既定で下寄せ。画面下部にコマンドカード等が出る区間は caption の先頭に
            # `^` を付けると上寄せになる（重なって読めない = 実測）
            cap_y="H-h-64"
            if [ "${caption#^}" != "$caption" ]; then caption=${caption#^}; cap_y="64"; fi
            png="$WORK/$id-cap.png"
            "$CAPTION_BIN" "$png" "$W" "$CAPTION_FONT_PX" "$caption" "$subtitle"
            ffmpeg -nostdin -v error -y -ss "$start" -i "$src" \
                -loop 1 -framerate "$FPS" -t "$dur" -i "$png" \
                -filter_complex "[0:v]${vf}[bg];[1:v]format=rgba,setpts=PTS-STARTPTS,fade=t=in:st=0.25:d=0.45:alpha=1,fade=t=out:st=${fo_start}:d=0.5:alpha=1[cap];[bg][cap]overlay=0:${cap_y}:format=auto,format=yuv420p[v]" \
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
    *) echo "ERROR: 不明な kind: ${kind}（${id}）" >&2; exit 1 ;;
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
if [ "$n" = 0 ]; then
    if [ "$NO_NARR" != 1 ]; then
        echo "ERROR: ナレーション wav が 1 つも無い" >&2; exit 1
    fi
    # 無音前提の構成: ナレーションのバスを作らず BGM だけを敷く
    echo "   ナレーション無し（TAKO_PROMO_NO_NARR=1）。BGM のみで組む"
    narr_only="$WORK/narr.wav"
    ffmpeg -nostdin -v error -y -f lavfi -i "anullsrc=r=48000:cl=stereo" -t "$VDUR" "$narr_only"
fi
if [ "$n" -gt 0 ]; then
# ナレーションのバスを目標ラウドネスへそろえる。**固定ゲインで持ち上げてはいけない**:
# 声を替えるとクレストファクタが変わるので、同じピークにそろえてもラウドネスは一致しない
# （実測: ピーク -12.3dB で say は -25.3 LUFS・VOICEVOX/ずんだもんは -31.9 LUFS = 6.6dB 差）。
# 固定の +9.5dB は say 専用の値で、そのまま VOICEVOX に当てると最終段のリミッタが
# 振り切れて 0.0dBFS まで潰れた（実測）。測ってから当てれば、どの声でも同じ音量で出る。
printf '%samix=inputs=%d:normalize=0:dropout_transition=0,apad=whole_dur=%s[narr];\n' "$mix" "$n" "$VDUR" >> "$fc"
narr_flat="$WORK/narr-flat.wav"
ffmpeg -nostdin -v error -y "${inputs[@]}" -filter_complex_script "$fc" -map "[narr]" -t "$VDUR" -ar 48000 -ac 2 "$narr_flat"

# 目標値は v2（say）の設計値そのまま。BGM を敷いたあと最終段で -14 LUFS へ寄せるので、
# ここを動かすと BGM とナレーションの相対バランスが変わる
NARR_LUFS=${TAKO_PROMO_NARR_LUFS:--15.8}
narr_i=$(ffmpeg -nostdin -hide_banner -i "$narr_flat" -af ebur128=framelog=quiet -f null - 2>&1 \
    | sed -n 's/^ *I: *\(-*[0-9.]*\) LUFS.*/\1/p' | tail -1)
narr_only="$WORK/narr.wav"
if [ -n "$narr_i" ]; then
    narr_gain=$(/usr/bin/python3 -c "print(f'{${NARR_LUFS} - (${narr_i}):.2f}')")
    echo "   ナレーション: ${narr_i} LUFS → ${NARR_LUFS} LUFS（${narr_gain}dB）"
    ffmpeg -nostdin -v error -y -i "$narr_flat" \
        -af "volume=${narr_gain}dB,alimiter=limit=0.95:level=disabled" -ar 48000 -ac 2 "$narr_only"
else
    echo "!! ナレーションのラウドネスを測れなかった。ゲイン調整なしで進む" >&2
    cp "$narr_flat" "$narr_only"
fi
fi

mixwav="$WORK/mix.wav"
if [ -f "$BGM" ]; then
    fade_start=$(/usr/bin/python3 -c "print(max(0.0, $VDUR - 3.0))")
    # BGM は薄く（-16dB 相当）。ナレーション中はさらに sidechain で下げる
    ffmpeg -nostdin -v error -y -i "$narr_only" -stream_loop -1 -i "$BGM" \
        -filter_complex "[1:a]atrim=0:${VDUR},asetpts=PTS-STARTPTS,volume=0.26,afade=t=in:st=0:d=2,afade=t=out:st=${fade_start}:d=3[bgm];[bgm][0:a]sidechaincompress=threshold=0.015:ratio=8:attack=40:release=700:makeup=1[duck];[0:a][duck]amix=inputs=2:normalize=0:dropout_transition=0[a]" \
        -map "[a]" -t "$VDUR" -ar 48000 -ac 2 "$mixwav"
else
    echo "!! BGM が無い（${BGM}）。ナレーションのみで書き出す" >&2
    cp "$narr_only" "$mixwav"
fi

# ── ラウドネスを YouTube の基準（-14 LUFS）へそろえる ─────────────────
# 測ってから固定ゲインを 1 回かける方式（loudnorm の 1 パスは音楽の下でポンプするため使わない）。
# 目標を外れて突き上がるピークは alimiter で止める（AAC のオーバーシュートぶんを見て -2dBFS で止める）。
# **level=disabled が必須**: alimiter の自動レベルは既定 true で、リミットしたあと
# 0dB へ戻すので limit の指定が無かったことになる（実測: limit=0.84 でも true peak +0.6dBFS）。
LUFS_TARGET=${TAKO_PROMO_LUFS:--14.0}
measured_i=$(ffmpeg -nostdin -hide_banner -i "$mixwav" -af ebur128=framelog=quiet -f null - 2>&1 \
    | sed -n 's/^ *I: *\(-*[0-9.]*\) LUFS.*/\1/p' | tail -1)
if [ -n "$measured_i" ]; then
    gain=$(/usr/bin/python3 -c "print(f'{${LUFS_TARGET} - (${measured_i}):.2f}')")
    echo "   ラウドネス: ${measured_i} LUFS → ${LUFS_TARGET} LUFS（${gain}dB）"
    afilter="volume=${gain}dB,alimiter=limit=0.79:level=disabled"
else
    echo "!! ラウドネスを測れなかった。ゲイン調整なしで書き出す" >&2
    afilter="anull"
fi
ffmpeg -nostdin -v error -y -i "$video" -i "$mixwav" -filter_complex "[1:a]${afilter}[a]" \
    -map 0:v -map "[a]" -c:v copy -c:a aac -ar 48000 -b:a 192k -movflags +faststart -shortest "$OUT"

# 章のタイムスタンプ（YouTube 説明文用）を区間表から出す
chap="$WORK/chapters.txt"; : > "$chap"
for i in "${!ids[@]}"; do
    case "${ids[$i]}" in
    op_card|c*_card|outro_card|op_title|c?_title|outro)
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
