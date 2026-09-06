#!/bin/bash
# tako:run: bash scripts/promo/pii-scan.sh ~/Desktop/tako-promo/tako-explainer-v1.mp4
# 動画（または PNG ディレクトリ）の全フレームを Vision OCR にかけ、個人情報のパターンを
# 機械検査する（#470 の作法を #1081 でスクリプト化）。
#
# 使い方: scripts/promo/pii-scan.sh <mp4|フレームディレクトリ> [抽出 fps（既定 1）]
# 出力:
#   /private/tmp/tako-promo-pii/<名前>/frames/   抽出フレーム
#   /private/tmp/tako-promo-pii/<名前>/ocr.tsv   ファイル名 \t 認識文字列
#   /private/tmp/tako-promo-pii/<名前>/hits.tsv  カテゴリ \t ファイル名 \t 該当行
# 標準出力にはカテゴリ別の件数だけを出す（値そのものは出さない = 報告へ転記してよい形）。
# 1 件でも当たれば exit 1。当たりを目で確かめて偽陽性なら理由を報告に書く。
#
# 検査語のうち環境依存のもの（ユーザー名・フルネーム・ホスト名・git の名前）は
# **実行時に環境から組み立てる**（リポジトリへ値を置かない = #927 の番犬と同じ方針）。
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
SRC=${1:-}
FPS=${2:-1}
[ -n "$SRC" ] || { echo "usage: $0 <mp4|frames-dir> [fps]" >&2; exit 2; }

OCR_BIN=/private/tmp/tako-promo-ocr
if [ ! -x "$OCR_BIN" ] || [ "$SCRIPT_DIR/ocr-frames.swift" -nt "$OCR_BIN" ]; then
    swiftc -O -o "$OCR_BIN" "$SCRIPT_DIR/ocr-frames.swift" || { echo "ERROR: ocr-frames.swift のコンパイルに失敗" >&2; exit 1; }
fi

name=$(basename "$SRC"); name=${name%.*}
WORK=/private/tmp/tako-promo-pii/$name
rm -rf "$WORK"; mkdir -p "$WORK/frames"
if [ -d "$SRC" ]; then
    cp "$SRC"/*.png "$WORK/frames/"
else
    ffmpeg -v error -i "$SRC" -vf "fps=$FPS" "$WORK/frames/f%05d.png"
fi
total=$(ls "$WORK/frames" | wc -l | tr -d ' ')
echo "== OCR: ${total} フレーム（${FPS} fps）..."
"$OCR_BIN" "$WORK/frames" > "$WORK/ocr.tsv"
lines=$(wc -l < "$WORK/ocr.tsv" | tr -d ' ')
echo "== 認識行: ${lines}"

# ── 検査パターン ────────────────────────────────────────────────────
# 環境由来の語（ユーザー名 / フルネームの各語 / ホスト名 / git user.name）。
# 3 文字未満や一般語は誤検知源なので落とす
terms=()
add_term() { local t=$1; [ "${#t}" -ge 3 ] && terms+=("$t"); return 0; }
add_term "${USER:-}"
for w in $(id -F 2>/dev/null | tr ' ' '\n'); do add_term "$w"; done
add_term "$(hostname -s 2>/dev/null || true)"
add_term "$(scutil --get LocalHostName 2>/dev/null || true)"
for w in $(git -C "$SCRIPT_DIR" config user.name 2>/dev/null | tr ' ' '\n'); do add_term "$w"; done
for w in ${TAKO_PII_TERMS:-}; do add_term "$w"; done

: > "$WORK/hits.tsv"
scan() {  # $1 = カテゴリ, $2 = 拡張正規表現（大文字小文字を無視）
    local cat=$1 re=$2
    grep -Ei -- "$re" "$WORK/ocr.tsv" | sed "s/^/${cat}\t/" >> "$WORK/hits.tsv" || true
}
scan email        '[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}'
scan home_path    '/Users/[A-Za-z0-9._-]+'
scan tailnet      '\.ts\.net|tailscale\.com/|ts\.net'
scan private_ip   '(^|[^0-9])(10|100|172|192)\.[0-9]{1,3}\.[0-9]{1,3}\.[0-9]{1,3}'
scan token        'sk-ant-|ghp_[A-Za-z0-9]{10,}|Bearer [A-Za-z0-9]|[A-Fa-f0-9]{32,}|session_[0-9A-Za-z]{16,}|claude\.ai/code/'
scan uuid         '[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}'
for t in "${terms[@]}"; do
    # 公開ハンドル（GitHub の takushio2525）は URL として意図的に出すので除外しない側に置かない:
    # ここでは環境由来の語をそのまま探す。何が当たったかは hits.tsv で目視する
    scan "term" "$(printf '%s' "$t" | sed 's/[][\.*^$/]/\\&/g')"
done

echo "== 検査カテゴリ別の件数（0 が期待値）"
for c in email home_path tailnet private_ip token uuid term; do
    n=$(awk -F'\t' -v c="$c" '$1==c' "$WORK/hits.tsv" | wc -l | tr -d ' ')
    printf '   %-12s %s\n' "$c" "$n"
done
hits=$(wc -l < "$WORK/hits.tsv" | tr -d ' ')
echo "== 環境由来の検査語: ${#terms[@]} 語（値は出さない）"
echo "== 詳細: $WORK/hits.tsv（値を含むのでリポジトリ・Issue へ貼らない）"
if [ "$hits" -gt 0 ]; then
    echo "!! PII 候補 ${hits} 件。hits.tsv を目視して偽陽性かを判断すること" >&2
    exit 1
fi
echo "== PII 候補なし"
