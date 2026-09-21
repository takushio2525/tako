#!/bin/bash
# isolated-gui.sh — 隔離 GUI（`TAKO_ISOLATED=1` の tako-app）の起動・停止の 1 実装（#1490）
#
# 何のためか:
#   隔離検証スクリプトは「バイナリを決める → 仮想ディスプレイを用意する →
#   窓の env を置く → 起動する → CLI が繋がるまで待つ → 自分の pid だけ落とす」を
#   毎回やる。これが**各スクリプトに手書きで散っている**と、直し方を 1 本足すたびに
#   12 か所へ同じ修正を配る必要があり、実際に配り切れていなかった（#1490 の症状）。
#
#   踏んだ罠はこれ: **蓋閉じ運用の `tako-vd` はアイドルで眠り、眠った面は
#   CoreGraphics の active 一覧から落ちる**。その状態の検証用 GUI は
#   ユーザーの画面へ窓を出さずに**終了コード 4 で終わる**（#1160 の設計。これは正しい）
#   ので、`virtual-display.sh ensure` を**起動の直前に毎回**通していないスクリプトは
#   「GUI が立たない」で止まる。#1487 の worker は同じ検証に 3 回失敗した。
#
#   `ensure` は冪等（常設の面は作り直さない・消す機能は無い）なので、毎回通して困らない。
#   だから「起動する」を 1 本にまとめて、その中に `ensure` を畳み込むのが答え。
#
# 使い方:
#   . scripts/lib/isolated-gui.sh        # 関数として読む（直接実行はしない）
#
#   isolated_gui_bins                    # TAKO_BIN / APP_BIN を決める（無ければビルド）
#   launch_isolated_gui "$TMP/app.log"   # ensure → env の既定 → 起動（pid は $ISOLATED_GUI_PID）
#   wait_isolated_gui                    # `tako list` が通るまで待つ（任意）
#   stop_isolated_gui                    # 自分で起こした pid だけを落とす
#
#   起動ごとに env を足したいときは `VAR=VAL` を並べる（`export` を汚さない）:
#   launch_isolated_gui "$TMP/app-legacy.log" TAKO_1487_LEGACY=1
#
# **落とすのは自分で起こした pid だけ**（`pkill -f tako` / `killall tako-app` は
#   本番 `/Applications/tako.app` にも他 worker の隔離インスタンスにも当たる。
#   実際にユーザーの GUI を落とした事故がある）。このファイルに名前一致で殺す道は作らない。
#
# **窓の位置・寸法を AX（System Events）で動かさない**（#1442）。AX は複数の tako-app を
#   unix id にかかわらず同一プロセスとして返すので本番の窓に当たる。指定するなら
#   `ISOLATED_GUI_BOUNDS`（= tako 自身の口 `TAKO_WINDOW_BOUNDS`）を使う。

set -uo pipefail

# 起動するバイナリの置き場（**`target/…/tako-app` の直書きをこの 2 行に閉じる**。
# 散らすと「stale な target を掴んでいた」に気づけない = #432 の罠）
ISOLATED_GUI_APP_REL=${TAKO_ISO_APP_REL:-target/debug/tako-app}
ISOLATED_GUI_CLI_REL=${TAKO_ISO_CLI_REL:-target/debug/tako}

# 窓を出す面。`TAKO_ISOLATED` が立っていれば tako 側の既定も同じ面だが、
# **何も指定していないのか tako-vd を狙っているのかをスクリプトから読めるようにする**
ISOLATED_GUI_DISPLAY=${ISOLATED_GUI_DISPLAY:-${TAKO_VD_NAME:-tako-vd}}

# 窓の矩形（`x,y,w,h` か `w,h`）。**既定は空 = 指定しない**。
# 既定を与えると置き先の中央 960x600 から変わり、窓の実寸を測る検証
# （`test-worker-min-width.sh` の cols）の実測値が動くので、欲しいスクリプトだけが置く
ISOLATED_GUI_BOUNDS=${ISOLATED_GUI_BOUNDS:-}

# `tako list` が通るまで待つ上限（0.1 秒 × この回数）
ISOLATED_GUI_WAIT_TRIES=${ISOLATED_GUI_WAIT_TRIES:-200}

# 直前の `launch_isolated_gui` が起こした pid（呼び出し側はこれを自分の変数へ受ける）
ISOLATED_GUI_PID=""

iso_err() { echo "ERROR: $*" >&2; }

# このファイルから見たリポジトリルート
iso_repo_root() {
    (cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
}

# TAKO_BIN（CLI）と APP_BIN（GUI）を決める。無ければビルドする。
# **呼び出し側の指定を尊重する**（`TAKO_BIN=… bash scripts/test-….sh` で差し替えられる）
isolated_gui_bins() {
    local root
    root=$(iso_repo_root)
    TAKO_BIN="${TAKO_BIN:-$root/$ISOLATED_GUI_CLI_REL}"
    APP_BIN="${APP_BIN:-$root/$ISOLATED_GUI_APP_REL}"
    if [ ! -x "$TAKO_BIN" ] || [ ! -x "$APP_BIN" ]; then
        echo "バイナリをビルドします…"
        (cd "$root" && cargo build -p tako-cli -p tako-app --quiet)
    fi
    local b
    for b in "$TAKO_BIN" "$APP_BIN"; do
        [ -x "$b" ] || { iso_err "バイナリが見つからない: $b"; return 1; }
    done
    return 0
}

# 窓の置き先を用意する（眠っていれば起こす）。**起動の直前に毎回通す**のが要点。
# 配線が無い環境（CI・他人の機・Windows）では素通しして続行する
# （そこでは既定の面へ落ちる = `.agent/conventions.md`「面が見えているのに
# 当たらないときは落ちる」。検証そのものが回らなくなるほうが悪い）
iso_ensure_display() {
    local vd
    vd="$(iso_repo_root)/scripts/lib/virtual-display.sh"
    [ -x "$vd" ] || return 0
    bash "$vd" ensure >/dev/null 2>&1 || \
        echo "  (注) 仮想ディスプレイを用意できなかった: 既定の面で続行する"
    return 0
}

# 隔離 GUI を起こす。pid は $ISOLATED_GUI_PID へ置く（**`$( )` で受けない**:
# 副シェルの子になると呼び出し側の `wait` が「子ではない」で失敗する）。
#
#   launch_isolated_gui <ログの置き場> [VAR=VAL …]
launch_isolated_gui() {
    local log="${1:-}"
    [ -n "$log" ] || { iso_err "launch_isolated_gui にはログの置き場が要る"; return 1; }
    shift
    [ -n "${APP_BIN:-}" ] || { iso_err "APP_BIN が空（先に isolated_gui_bins を呼ぶ）"; return 1; }

    iso_ensure_display

    local -a env_args=(
        "TAKO_ISOLATED=${TAKO_ISOLATED:-1}"
        "TAKO_DISPLAY=${TAKO_DISPLAY:-$ISOLATED_GUI_DISPLAY}"
    )
    [ -n "$ISOLATED_GUI_BOUNDS" ] && \
        env_args+=("TAKO_WINDOW_BOUNDS=${TAKO_WINDOW_BOUNDS:-$ISOLATED_GUI_BOUNDS}")
    # 呼び出し側が並べた `VAR=VAL` は既定より後ろ = そちらが勝つ
    env "${env_args[@]}" "$@" "$APP_BIN" > "$log" 2>&1 &
    ISOLATED_GUI_PID=$!
    return 0
}

# CLI が繋がるまで待つ。繋がらなければログの末尾を出して非ゼロ
#
#   wait_isolated_gui [ログの置き場] [試行回数]
wait_isolated_gui() {
    local log="${1:-}" tries="${2:-$ISOLATED_GUI_WAIT_TRIES}" i
    [ -n "${TAKO_BIN:-}" ] || { iso_err "TAKO_BIN が空（先に isolated_gui_bins を呼ぶ）"; return 1; }
    for i in $(seq 1 "$tries"); do
        if "$TAKO_BIN" list >/dev/null 2>&1; then return 0; fi
        sleep 0.1
    done
    echo "tako-app へ接続できない:"
    [ -n "$log" ] && [ -f "$log" ] && tail -20 "$log"
    return 1
}

# 自分で起こした隔離 GUI を落とす。**明示 pid だけ**（名前一致では殺さない）
#
#   stop_isolated_gui [pid]
stop_isolated_gui() {
    local pid="${1:-$ISOLATED_GUI_PID}" i
    [ -n "$pid" ] || return 0
    kill "$pid" 2>/dev/null || true
    # 素直に終わるのを待ってから、居座るときだけ強く落とす
    for i in $(seq 1 50); do
        kill -0 "$pid" 2>/dev/null || break
        sleep 0.1
    done
    kill -0 "$pid" 2>/dev/null && kill -9 "$pid" 2>/dev/null
    wait "$pid" 2>/dev/null || true
    [ "$pid" = "${ISOLATED_GUI_PID:-}" ] && ISOLATED_GUI_PID=""
    return 0
}

# 直接実行されたときは使い方だけ出す（**起動の口は持たせない**: 隔離の env は
# 呼び出し側のスクリプトが組み立てるもので、ここから出すと中途半端な隔離で本番を触る）
if [ "${BASH_SOURCE[0]}" = "${0}" ]; then
    echo "使い方: . ${0}   # source して関数を使う（直接実行する口は無い）" >&2
    echo "  isolated_gui_bins                    TAKO_BIN / APP_BIN を決める（無ければビルド）" >&2
    echo "  launch_isolated_gui <log> [VAR=VAL…] 仮想ディスプレイを起こして隔離 GUI を起動" >&2
    echo "  wait_isolated_gui [log] [試行回数]    tako list が通るまで待つ" >&2
    echo "  stop_isolated_gui [pid]              自分で起こした pid だけを落とす" >&2
    exit 1
fi
