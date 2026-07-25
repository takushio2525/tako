#!/usr/bin/env bash
# Issue #494 の git パネル描画を隔離インスタンスで観察するための調査ハーネス。
#
# 使い方:
#   scripts/gitpanel-probe.sh burst <出力ディレクトリ> [幅...]   起動 → 各幅を連続キャプチャ → 終了
#   scripts/gitpanel-probe.sh start                              常駐起動（CLI 操作の検証用）
#   scripts/gitpanel-probe.sh cli <args...>                      隔離インスタンスへ tako CLI
#   scripts/gitpanel-probe.sh stop
#
# 収録上の制約（実測でハマった点）:
#   本番 tako がフルスクリーンだと隔離ウィンドウが遮蔽され、GPUI が描画を止める。
#   その状態の screencapture は「最後に描いたフレーム」を返し、操作しても同じ絵が
#   撮れ続ける（#470 v2 と同じ罠）。起動直後は描画が生きているので burst で撮り切り、
#   撮れた画像が全部同一バイトでないことを毎回チェックする。
#
# 本番インスタンスへ CLI が誤接続しないよう TAKO_SOCKET / TAKO_TOKEN /
# TAKO_PANE_ID / TAKO_TAB_ID を必ず環境から落として呼ぶ（progress.md の事故記録参照）。
set -uo pipefail

ROOT=$(cd "$(dirname "$0")/.." && pwd)
REPO=${TAKO_494_REPO:-/tmp/tako-494-repo}
ISO=/tmp/tako-494-iso
APP=${TAKO_494_APP:-$ROOT/target/debug/tako-app}
CLI=${TAKO_494_CLI:-$ROOT/target/debug/tako}
WINBOUNDS=/private/tmp/tako-promo-winbounds

iso_env=(
    env -u TAKO_SOCKET -u TAKO_TOKEN -u TAKO_PANE_ID -u TAKO_TAB_ID -u TAKO_MCP_URL
    TAKO_ISOLATED=1
    TAKO_PERSIST=0
    TAKO_DATA_DIR="$ISO/data"
    TAKO_DISCOVERY_DIR="$ISO/discovery"
    TAKO_TMUX_SOCKET=tako-494-iso
    TAKO_SESSIONS_FILE="$ISO/sessions.yaml"
    TAKO_PANE_LOG_DIR="$ISO/pane-logs"
    TAKO_REMOTE_STATE_DIR="$ISO/remote"
    TAKO_AUTORENAME=0
    ZDOTDIR="$ISO/zdot"
)

iso_start() {
    rm -rf "$ISO"; mkdir -p "$ISO/zdot"
    # 証拠スクショにユーザー名・ホスト名を写さない + cwd を再現用リポジトリに固定する
    {
        printf "PROMPT='%%1~ ❯ '\nRPROMPT=''\n"
        printf "cd %s 2>/dev/null\n" "$REPO"
    } > "$ISO/zdot/.zshrc"

    if [ ! -x "$WINBOUNDS" ] || [ "$ROOT/scripts/promo/winbounds.swift" -nt "$WINBOUNDS" ]; then
        swiftc -O -o "$WINBOUNDS" "$ROOT/scripts/promo/winbounds.swift" || exit 1
    fi

    ( cd "$REPO" && exec "${iso_env[@]}" "$APP" ) >"$ISO/app.log" 2>&1 &
    echo $! > "$ISO/app.pid"

    for _ in $(seq 1 80); do
        [ -f "$ISO/discovery/control.json" ] && break
        sleep 0.5
    done
    [ -f "$ISO/discovery/control.json" ] || { echo "ERROR: control.json が出ない"; cat "$ISO/app.log"; return 1; }

    local APP_PID b w
    APP_PID=$(cat "$ISO/app.pid")
    for _ in $(seq 1 80); do
        b=$("$WINBOUNDS" "$APP_PID" --activate 2>/dev/null || true)
        if [ -n "$b" ]; then
            w=$(echo "$b" | cut -d' ' -f4)
            if [ "${w:-0}" -ge 800 ]; then echo "$b" | cut -d' ' -f1 > "$ISO/wid"; break; fi
        fi
        sleep 0.5
    done
    [ -s "$ISO/wid" ] || { echo "ERROR: ウィンドウが現れない"; return 1; }
    echo "   pid=$APP_PID window=$(cat "$ISO/wid")"
}

iso_stop() {
    [ -f "$ISO/app.pid" ] && kill "$(cat "$ISO/app.pid")" 2>/dev/null
    tmux -L tako-494-iso kill-server 2>/dev/null
}

cmd=${1:-burst}; shift || true

case "$cmd" in
burst)
    OUT=${1:?出力ディレクトリを指定}; shift || true
    WIDTHS=("$@"); [ ${#WIDTHS[@]} -gt 0 ] || WIDTHS=(325 460 620)
    mkdir -p "$OUT"
    iso_stop; sleep 1
    echo "== 隔離 tako-app 起動（cwd=${REPO}）"
    iso_start || exit 1
    trap iso_stop EXIT
    WID=$(cat "$ISO/wid"); APP_PID=$(cat "$ISO/app.pid")
    sleep 2
    "${iso_env[@]}" "$CLI" panel --show --view git >/dev/null 2>&1
    sleep 3
    for W in "${WIDTHS[@]}"; do
        "${iso_env[@]}" "$CLI" panel --width "$W" >/dev/null 2>&1
        sleep 1.5
        "$WINBOUNDS" "$APP_PID" --activate >/dev/null 2>&1
        sleep 0.4
        screencapture -x -o -l"$WID" "$OUT/w${W}.png" || echo "   キャプチャ失敗 w=$W"
        echo "   w${W}.png: $(stat -f %z "$OUT/w${W}.png" 2>/dev/null) bytes"
    done
    # 描画停止（同じ絵が撮れ続ける）検出
    uniq_count=$(for W in "${WIDTHS[@]}"; do stat -f %z "$OUT/w${W}.png" 2>/dev/null; done | sort -u | wc -l | tr -d ' ')
    if [ ${#WIDTHS[@]} -gt 1 ] && [ "$uniq_count" -le 1 ]; then
        echo "ERROR: 全キャプチャが同一サイズ = 描画停止フレームの可能性。撮り直しが必要" >&2
        exit 1
    fi
    echo "== 完了: $OUT"
    ;;
start)
    iso_stop; sleep 1
    iso_start || exit 1
    sleep 2
    "${iso_env[@]}" "$CLI" panel --show --view git >/dev/null 2>&1
    sleep 2
    "${iso_env[@]}" "$CLI" panel 2>&1 | head -3
    ;;
cli)
    exec "${iso_env[@]}" "$CLI" "$@"
    ;;
shot)
    OUT=${1:?出力先 png}; W=${2:-}
    APP_PID=$(cat "$ISO/app.pid"); WID=$(cat "$ISO/wid")
    if [ -n "$W" ]; then "${iso_env[@]}" "$CLI" panel --width "$W" >/dev/null 2>&1; sleep 1.5; fi
    "$WINBOUNDS" "$APP_PID" --activate >/dev/null 2>&1
    sleep 0.5
    mkdir -p "$(dirname "$OUT")"
    screencapture -x -o -l"$WID" "$OUT" || { echo "キャプチャ失敗"; exit 1; }
    echo "$OUT: $(stat -f %z "$OUT") bytes"
    ;;
stop)
    iso_stop; echo "stopped";;
*)
    echo "unknown: $cmd" >&2; exit 2;;
esac
