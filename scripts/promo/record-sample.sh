#!/bin/bash
# tako 紹介動画 Phase A: サンプルクリップ収録スクリプト (#470)
#
# 隔離 GUI インスタンス（TAKO_ISOLATED=1 + 明示ソケット/データディレクトリ）を起動し、
# tako CLI でペイン操作をスクリプト再生しながら screencapture -v でウィンドウ領域を
# mp4 収録する。本番の tako・tmux・remote 状態には一切触れない。
#
# 前提:
#   - /Applications/tako.app が install 済み（app + 同梱 CLI を使う）
#   - 実行ホスト（ターミナル）に macOS の画面収録権限があること
#     （権限が無いと黒画面の mp4 になる。収録後に必ずフレームを目視確認する）
#   - ffmpeg / ffprobe（brew install ffmpeg）
#
# 使い方:
#   scripts/promo/record-sample.sh
#   出力: ~/Desktop/tako-promo/sample-ai-pane-ops.mp4 + frames/（1fps 抽出、PII 確認用）
#
# PII 対策（収録画面にユーザー名・ホスト名・実パスを出さない）:
#   - デモは /private/tmp/tako-demo/awesome-app 配下のダミープロジェクトで行う
#   - ZDOTDIR を差し替え、プロンプトを「ディレクトリ名 ❯」のみにする（%n@%m を出さない）
#   - 収録領域は隔離インスタンスのウィンドウ矩形のみ（背後の本番画面を含めない）
set -euo pipefail

APP=${TAKO_PROMO_APP:-/Applications/tako.app/Contents/MacOS/tako-app}
CLI=${TAKO_PROMO_CLI:-/Applications/tako.app/Contents/MacOS/tako}
OUT_DIR=${TAKO_PROMO_OUT:-"$HOME/Desktop/tako-promo"}
CLIP_NAME=${TAKO_PROMO_NAME:-sample-ai-pane-ops}
DUR=${TAKO_PROMO_DUR:-15}

DEMO=/private/tmp/tako-demo
WORK=/private/tmp/tako-promo-iso
SOCKET_NAME=tako-promo
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"

[ -x "$APP" ] || { echo "ERROR: tako-app が見つからない: $APP" >&2; exit 1; }
[ -x "$CLI" ] || { echo "ERROR: tako CLI が見つからない: $CLI" >&2; exit 1; }
command -v ffprobe >/dev/null || { echo "ERROR: ffprobe が必要（brew install ffmpeg）" >&2; exit 1; }

# ── 継承 TAKO_* 環境の遮断（worker ペイン内から実行しても本番へ誤接続しない）──
TAKO_UNSET=()
while IFS='=' read -r k _; do
    case "$k" in TAKO_*) TAKO_UNSET+=(-u "$k") ;; esac
done < <(env)

# ── デモ用ダミー環境の生成 ─────────────────────────────────────────
rm -rf "$DEMO" "$WORK"
mkdir -p "$DEMO/zdot" "$DEMO/awesome-app/src" "$DEMO/awesome-app/scripts" \
    "$WORK/discovery" "$WORK/data" "$OUT_DIR/frames"

# プロンプトを PII なしの最小形にする（ユーザーの .zshrc は読まない）
cat > "$DEMO/zdot/.zshrc" <<'ZRC'
PROMPT='%F{6}%1~%f ❯ '
RPROMPT=''
ZRC

cat > "$DEMO/awesome-app/README.md" <<'MD'
# awesome-app

A tiny web app used to demo **tako** — the GUI terminal built for the
AI-agent era.

## Features

- Fast REST API
- Live dashboard
- One-command deploy

## Quick start

```sh
scripts/dev-server.sh
```

Then open `http://localhost:5173`.
MD

cat > "$DEMO/awesome-app/src/app.py" <<'PY'
"""awesome-app: demo web app."""

import time


def handle(path: str) -> tuple[int, str]:
    if path == "/api/users":
        return 200, '[{"id": 1, "name": "alice"}]'
    return 404, "not found"


def main() -> None:
    print("awesome-app listening on :5173")
    while True:
        time.sleep(1)


if __name__ == "__main__":
    main()
PY

cat > "$DEMO/awesome-app/scripts/dev-server.sh" <<'SRV'
#!/bin/bash
# デモ用のダミー dev サーバー: それらしいアクセスログを流し続ける
printf '\033[1;36m  dev server\033[0m ready on \033[4mhttp://localhost:5173\033[0m\n\n'
paths=(/api/users /api/posts /assets/app.js /index.html /api/health)
i=0
while true; do
    p=${paths[$((i % ${#paths[@]}))]}
    ms=$(( (RANDOM % 40) + 3 ))
    printf '\033[90m%s\033[0m \033[32mGET\033[0m %-16s \033[1m200\033[0m %2dms\n' \
        "$(date '+%H:%M:%S')" "$p" "$ms"
    i=$((i + 1))
    sleep 0.4
done
SRV
chmod +x "$DEMO/awesome-app/scripts/dev-server.sh"

# ── 隔離インスタンス起動 ───────────────────────────────────────────
echo "== 隔離 tako を起動（socket=${SOCKET_NAME}）"
(
    cd "$DEMO/awesome-app"
    env "${TAKO_UNSET[@]}" \
        TAKO_ISOLATED=1 \
        TAKO_TMUX_SOCKET="$SOCKET_NAME" \
        TAKO_DISCOVERY_DIR="$WORK/discovery" \
        TAKO_DATA_DIR="$WORK/data" \
        ZDOTDIR="$DEMO/zdot" \
        "$APP" >"$WORK/app.log" 2>&1 &
    echo $! > "$WORK/app.pid"
)
APP_PID=$(cat "$WORK/app.pid")

cleanup() {
    kill "$APP_PID" 2>/dev/null || true
    sleep 1
    kill -9 "$APP_PID" 2>/dev/null || true
    tmux -L "$SOCKET_NAME" kill-server 2>/dev/null || true
}
trap cleanup EXIT

# IPC ソケット + トークンが生えるまで待つ
for _ in $(seq 1 60); do
    [ -S "$WORK/data/tako.sock" ] && [ -s "$WORK/data/token" ] && break
    sleep 0.5
done
[ -S "$WORK/data/tako.sock" ] || { echo "ERROR: 隔離インスタンスの IPC が起動しない（$WORK/app.log 参照）" >&2; exit 1; }

# CLI ラッパー: 隔離インスタンスのソケットへ明示接続（discovery 経由の誤接続を排除）
tko() {
    env "${TAKO_UNSET[@]}" \
        TAKO_SOCKET="$WORK/data/tako.sock" \
        TAKO_TOKEN="$(cat "$WORK/data/token")" \
        "$CLI" "$@"
}

# ペイン描画の安定を待ち、基準ペイン ID を取得
sleep 3
BASE_PANE=$(tko list | /usr/bin/python3 -c 'import json,sys; print(json.load(sys.stdin)["tabs"][0]["panes"][0]["id"])')
echo "== base pane: $BASE_PANE"

# ── 収録前の画面整備（PII 遮断: ホーム cwd のまま収録に入らない）────────
# 基準ペインをデモプロジェクトへ移動して画面をクリアし、タブ名も整える
tko send --pane "$BASE_PANE" "cd $DEMO/awesome-app && clear" >/dev/null
tko tab rename --tab 1 awesome-app >/dev/null 2>&1 || true
sleep 1.5

# ── ウィンドウ矩形の取得（収録領域 = 隔離ウィンドウのみ）──────────────
BOUNDS=$(swift "$SCRIPT_DIR/winbounds.swift" "$APP_PID")
read -r WX WY WW WH <<< "$BOUNDS"
echo "== window bounds: x=$WX y=$WY w=$WW h=$WH"

# ── 収録 + シーン再生 ─────────────────────────────────────────────
CLIP="$OUT_DIR/$CLIP_NAME.mp4"
rm -f "$CLIP"
echo "== 収録開始（${DUR}s）"
screencapture -v -V "$DUR" -R"$WX,$WY,$WW,$WH" "$CLIP" &
REC_PID=$!

sleep 1.5
# beat 1: README を Markdown プレビューで右に開く（AI がファイルを見せる動き）
tko open --pane "$BASE_PANE" "$DEMO/awesome-app/README.md" >/dev/null
sleep 3.5
# beat 2: dev サーバーを下ペインで起動（AI がプロセスを生やす動き）
tko split --pane "$BASE_PANE" --down --cwd "$DEMO/awesome-app" -- \
    bash scripts/dev-server.sh >/dev/null
sleep 3.5
# beat 3: 元ペインでコマンド実行（人とAIが同じ画面を共有している動き）
tko send --pane "$BASE_PANE" "ls -1" >/dev/null
sleep 2
# beat 4: レイアウト均等化
tko equalize >/dev/null 2>&1 || true

wait "$REC_PID" || true
echo "== 収録終了: $CLIP"

# ── 検証: ffprobe + フレーム抽出（PII 目視確認用）────────────────────
ffprobe -v error -select_streams v:0 \
    -show_entries stream=width,height,r_frame_rate,duration -of default=nw=1 "$CLIP"
rm -f "$OUT_DIR"/frames/*.png
ffmpeg -v error -i "$CLIP" -vf fps=1 "$OUT_DIR/frames/frame-%02d.png"
echo "== フレーム抽出: $OUT_DIR/frames/（全フレームに PII が無いか必ず目視確認する）"
