#!/bin/bash
# tako 紹介動画 収録共通ライブラリ (#470 Phase B)
#
# 各シーン収録スクリプトから source して使う。責務:
#   - 継承 TAKO_* 環境の遮断（worker ペイン内から実行しても本番へ誤接続しない）
#   - PII を含まないデモ環境（ダミープロジェクト + クリーンプロンプト）の生成
#   - 隔離 GUI インスタンス（TAKO_ISOLATED=1 + 明示ソケット/データディレクトリ）の起動と後始末
#   - ffmpeg avfoundation によるウィンドウ領域収録（screencapture -v は TCC 制約で黒画面のため不採用）
#   - 収録物の ffprobe 検証 + フレーム抽出（PII 全数チェック用）
#
# 収録エンジンについて（2026-07-23 実測）:
#   screencapture -v は本環境で黒画面（静止画 -x は正常）。ffmpeg -f avfoundation は
#   30fps でフルスクリーンを正常取得できるため、crop フィルタでウィンドウ領域だけを切り出す。
#   avfoundation はピクセル、CGWindowList はポイントのため winbounds.swift のスクリーン
#   論理サイズからスケールを求めて変換する。

PROMO_APP=${TAKO_PROMO_APP:-/Applications/tako.app/Contents/MacOS/tako-app}
PROMO_CLI=${TAKO_PROMO_CLI:-/Applications/tako.app/Contents/MacOS/tako}
PROMO_OUT=${TAKO_PROMO_OUT:-"$HOME/Desktop/tako-promo"}
# フレーム抽出（PII 検証用の中間物）は Desktop の TCC 制限を避けて /private/tmp に置く
PROMO_FRAMES=${TAKO_PROMO_FRAMES:-/private/tmp/tako-promo-frames}
PROMO_DEMO=/private/tmp/tako-demo
PROMO_LIB_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# 継承 TAKO_* を env -u で落とすための引数列
PROMO_ENV_CLEAN=()
while IFS='=' read -r k _; do
    case "$k" in TAKO_*) PROMO_ENV_CLEAN+=(-u "$k") ;; esac
done < <(env)

promo_require() {
    [ -x "$PROMO_APP" ] || { echo "ERROR: tako-app が無い: $PROMO_APP" >&2; return 1; }
    [ -x "$PROMO_CLI" ] || { echo "ERROR: tako CLI が無い: $PROMO_CLI" >&2; return 1; }
    command -v ffmpeg >/dev/null || { echo "ERROR: ffmpeg が必要" >&2; return 1; }
    command -v ffprobe >/dev/null || { echo "ERROR: ffprobe が必要" >&2; return 1; }
    mkdir -p "$PROMO_OUT/scenes" "$PROMO_FRAMES"
}

# ── デモ環境（PII ゼロ）────────────────────────────────────────────
# プロンプトは %n@%m（ユーザー名@ホスト名）を含まない「ディレクトリ名 ❯」のみにする。
# 作業パスは /private/tmp 配下だけを使い、ホームディレクトリを一切写さない。
promo_make_demo_env() {
    rm -rf "$PROMO_DEMO"
    mkdir -p "$PROMO_DEMO/zdot" "$PROMO_DEMO/awesome-app/src" \
        "$PROMO_DEMO/awesome-app/scripts" "$PROMO_DEMO/awesome-app/docs"

    cat > "$PROMO_DEMO/zdot/.zshrc" <<'ZRC'
PROMPT='%F{6}%1~%f ❯ '
RPROMPT=''
unset ZSH_THEME
ZRC

    cat > "$PROMO_DEMO/awesome-app/README.md" <<'MD'
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

    cat > "$PROMO_DEMO/awesome-app/src/app.py" <<'PY'
"""awesome-app: demo web app."""

import time

ROUTES = {
    "/api/users": '[{"id": 1, "name": "alice"}]',
    "/api/posts": '[{"id": 7, "title": "hello"}]',
}


def handle(path: str) -> tuple[int, str]:
    """Return (status, body) for a request path."""
    if path in ROUTES:
        return 200, ROUTES[path]
    return 404, "not found"


def main() -> None:
    print("awesome-app listening on :5173")
    while True:
        time.sleep(1)


if __name__ == "__main__":
    main()
PY

    # tako:run 宣言つき = Code Runner の再生ボタンで実行できるデモ用スクリプト
    cat > "$PROMO_DEMO/awesome-app/scripts/build.sh" <<'BLD'
#!/bin/bash
# tako:run: bash scripts/build.sh
set -e
steps=("resolving deps" "compiling src/app.py" "bundling assets" "writing dist/")
for s in "${steps[@]}"; do
    printf '\033[36m>>\033[0m %s\n' "$s"
    sleep 0.5
done
printf '\033[32mbuild succeeded\033[0m in 2.1s\n'
# 収録用: 完了直後にペインが exit で閉じると結果が写らないので保持する
sleep 600
BLD
    chmod +x "$PROMO_DEMO/awesome-app/scripts/build.sh"

    cat > "$PROMO_DEMO/awesome-app/scripts/dev-server.sh" <<'SRV'
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
    chmod +x "$PROMO_DEMO/awesome-app/scripts/dev-server.sh"

    # worker が「作業しているように見える」ダミーログ（S3 のワーカーペイン用）
    cat > "$PROMO_DEMO/awesome-app/scripts/worker.sh" <<'WRK'
#!/bin/bash
# デモ用のダミー worker: 受け取ったタスク名の作業ログを流して完了する
task=${1:-task}
printf '\033[35mworker\033[0m %s\n' "$task"
lines=("reading source files" "applying changes" "running tests" "all checks passed")
for l in "${lines[@]}"; do
    printf '  \033[90m*\033[0m %s\n' "$l"
    sleep 1.2
done
printf '\033[32mdone\033[0m %s\n' "$task"
sleep 600
WRK
    chmod +x "$PROMO_DEMO/awesome-app/scripts/worker.sh"
}

# ── 隔離インスタンス ───────────────────────────────────────────────
# $1 = 作業ディレクトリ, $2 = tmux ソケット名, $3 = persist（1 で永続化 ON）
promo_start_isolated() {
    local work=$1 socket=$2 persist=${3:-0}
    mkdir -p "$work/discovery" "$work/data"
    (
        cd "$PROMO_DEMO/awesome-app"
        env "${PROMO_ENV_CLEAN[@]}" \
            TAKO_ISOLATED=1 \
            TAKO_PERSIST="$persist" \
            TAKO_TMUX_SOCKET="$socket" \
            TAKO_DISCOVERY_DIR="$work/discovery" \
            TAKO_DATA_DIR="$work/data" \
            TAKO_REMOTE_STATE_DIR="$work/remote" \
            ZDOTDIR="$PROMO_DEMO/zdot" \
            "$PROMO_APP" >"$work/app.log" 2>&1 &
        echo $! > "$work/app.pid"
    )
    PROMO_APP_PID=$(cat "$work/app.pid")
    PROMO_WORK=$work
    local i
    for i in $(seq 1 80); do
        [ -S "$work/data/tako.sock" ] && [ -s "$work/data/token" ] && break
        sleep 0.5
    done
    if [ ! -S "$work/data/tako.sock" ]; then
        echo "ERROR: 隔離インスタンスの IPC が起動しない（$work/app.log）" >&2
        return 1
    fi
    PROMO_SOCKET_PATH="$work/data/tako.sock"
    PROMO_TOKEN=$(cat "$work/data/token")
    sleep 3
}

promo_stop_isolated() {
    local socket=$1
    # 既に終了しているプロセスへの kill は失敗するため、すべて || true で受ける
    # （set -e 下でここが非ゼロを返すと呼び出し側の検証まで飛ばされる）
    if [ -n "${PROMO_APP_PID:-}" ]; then
        kill "$PROMO_APP_PID" 2>/dev/null || true
        sleep 1
        kill -9 "$PROMO_APP_PID" 2>/dev/null || true
    fi
    [ -n "$socket" ] && { tmux -L "$socket" kill-server 2>/dev/null || true; }
    return 0
}

# 隔離インスタンスへ明示接続する CLI ラッパー
tko() {
    env "${PROMO_ENV_CLEAN[@]}" \
        TAKO_SOCKET="$PROMO_SOCKET_PATH" \
        TAKO_TOKEN="$PROMO_TOKEN" \
        "$PROMO_CLI" "$@"
}

# タブ 1 の先頭ペイン ID
promo_base_pane() {
    tko list | /usr/bin/python3 -c \
        'import json,sys; print(json.load(sys.stdin)["tabs"][0]["panes"][0]["id"])'
}

# ── 収録 ──────────────────────────────────────────────────────────
# 方式: screencapture -l<windowID> による**ウィンドウ単体**の連番キャプチャ →
# ffmpeg で結合。画面全体を撮って切り出す方式（avfoundation + crop /
# screencapture -R）は、収録中に別アプリのウィンドウが対象領域へ重なると
# その中身ごと写り込む（2026-07-23 に個人情報の写り込みが実際に発生）。
# ウィンドウ単体キャプチャなら手前に何が来ても対象ウィンドウの内容しか撮れない。
# screencapture -v（動画）は本環境では黒画面のため使わない。
#
# $1 = 出力 mp4, $2 = 尺（秒）。収録は background で走り promo_record_wait で待つ。
promo_record_start() {
    local out=$1 dur=$2
    # ウィンドウ ID は起動直後に作り直されることがあるので、
    # 実際に 1 枚撮れる ID が得られるまで引き直す
    local wid="" wx wy ww wh bounds probe="$PROMO_WORK/wid-probe.png" try
    for try in 1 2 3 4 5; do
        bounds=$(swift "$PROMO_LIB_DIR/winbounds.swift" "$PROMO_APP_PID" 2>/dev/null) || {
            sleep 1; continue; }
        read -r wid wx wy ww wh <<< "$bounds"
        rm -f "$probe"
        if screencapture -x -o -l"$wid" "$probe" 2>/dev/null && [ -s "$probe" ]; then
            rm -f "$probe"; break
        fi
        wid=""; sleep 1
    done
    [ -n "$wid" ] || { echo "ERROR: 収録できるウィンドウを特定できない" >&2; return 1; }
    echo "   対象ウィンドウ: id=$wid ${ww}x${wh}（尺 ${dur}s）"

    PROMO_REC_OUT=$out
    PROMO_REC_DUR=$dur
    PROMO_REC_DIR="$PROMO_WORK/frames-raw"
    rm -rf "$PROMO_REC_DIR"; mkdir -p "$PROMO_REC_DIR"
    local marker="$PROMO_WORK/rec.start"
    date +%s > "$marker"

    (
        local end=$(( $(date +%s) + dur )) i=0 last="" miss=0
        while [ "$(date +%s)" -lt "$end" ]; do
            i=$((i + 1))
            local f
            f=$(printf '%s/f%05d.png' "$PROMO_REC_DIR" "$i")
            if screencapture -x -o -l"$wid" "$f" 2>/dev/null && [ -s "$f" ]; then
                last=$f; miss=0
            else
                # ウィンドウが一時的に消えた場合は直前フレームで尺を保つ。
                # 連続で撮れないときはウィンドウが作り直された可能性が高いので ID を引き直す
                if [ -n "$last" ]; then cp "$last" "$f" 2>/dev/null || i=$((i - 1))
                else i=$((i - 1)); fi
                miss=$((miss + 1))
                if [ "$miss" -ge 10 ]; then
                    local nb
                    nb=$(swift "$PROMO_LIB_DIR/winbounds.swift" "$PROMO_APP_PID" 2>/dev/null || true)
                    [ -n "$nb" ] && wid=$(echo "$nb" | cut -d' ' -f1)
                    miss=0
                fi
            fi
        done
        echo "$i" > "$PROMO_WORK/rec.count"
    ) &
    PROMO_REC_PID=$!
    sleep 0.5
}

promo_record_wait() {
    wait "$PROMO_REC_PID" 2>/dev/null || true
    local n
    n=$(cat "$PROMO_WORK/rec.count" 2>/dev/null || echo 0)
    [ "$n" -gt 0 ] || { echo "ERROR: フレームが 1 枚も撮れていない" >&2; return 1; }
    # 実測フレームレート = 枚数 / 実収録秒。動画尺を実時間に一致させる
    local fps
    fps=$(/usr/bin/python3 -c "print(f'{$n/$PROMO_REC_DUR:.3f}')")
    echo "   $n 枚 / ${PROMO_REC_DUR}s = ${fps} fps → エンコード"
    rm -f "$PROMO_REC_OUT"
    ffmpeg -hide_banner -loglevel error -framerate "$fps" \
        -i "$PROMO_REC_DIR/f%05d.png" \
        -c:v libx264 -preset medium -crf 18 -pix_fmt yuv420p -r 30 \
        "$PROMO_REC_OUT" >"$PROMO_WORK/ffmpeg.log" 2>&1
    rm -rf "$PROMO_REC_DIR"
}

# ── 検証 ──────────────────────────────────────────────────────────
# $1 = クリップ, $2 = フレーム抽出先ディレクトリ, $3 = 抽出 fps（既定 1）
promo_verify() {
    local clip=$1 fdir=$2 fps=${3:-1}
    echo "-- ffprobe: $(basename "$clip")"
    ffprobe -v error -select_streams v:0 \
        -show_entries stream=width,height,r_frame_rate,duration,codec_name \
        -of default=nw=1 "$clip"
    rm -rf "$fdir"; mkdir -p "$fdir"
    ffmpeg -v error -i "$clip" -vf "fps=$fps" "$fdir/frame-%03d.png"
    echo "-- フレーム: $(ls "$fdir" | wc -l | tr -d ' ') 枚 → $fdir"
    # 全黒フレーム（TCC 権限喪失）の自動検出
    local dark
    dark=$( { ffmpeg -v error -i "$clip" -vf "blackdetect=d=0.5:pic_th=0.98" -f null - 2>&1 || true; } \
        | grep -c blackdetect || true)
    if [ "$dark" -gt 0 ]; then
        echo "!! 警告: 全黒区間を検出（画面収録権限を確認すること）" >&2
    fi
}
