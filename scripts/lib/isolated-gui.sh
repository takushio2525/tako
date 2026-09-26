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
# **面を用意できなければ起動しない**（#1744）: `ensure` が失敗した・成功と言ったのに面が
#   OS の一覧に無いときは、窓を開かずに終了コード 4（`ISOLATED_GUI_RC_NO_DISPLAY`）で返し、
#   理由を stderr へ 1 行「未実測: …」と出す。以前は「起動は続ける」で素通しする作りで、
#   面の指定を渡さない起動が tako の暗黙の既定（見えている面へ落ちる）に乗って
#   **ユーザーの画面へ窓が出うる**潜在経路があった（この道で窓が出た実例は確認されていない）。
#   蓋閉じ + ディスプレイスリープで面を用意できないのは日常なので、
#   呼び出し側は `launch_isolated_gui … || exit $?` で止め、報告には「未実測」と書く。
#   未配線の機（BetterDisplay が無い・CI）でも同じで、既定の面で続行する道は作らない。
#
# **tako-vd 以外の面の明示は通さない**（#1760）: 面を用意できても、呼び出し側が
#   `TAKO_DISPLAY=0`（= メイン画面）のように別の面を明示すると、以前はそのまま GUI へ渡っていた。
#   いまは起動の前に値を判定し（iso_display_allowed）、通さない値なら窓を開かずに
#   終了コード 2（`ISOLATED_GUI_RC_FOREIGN_DISPLAY`）で返して理由を stderr へ 1 行出す。
#   通すのは tako-vd の名前 / tako-vd の uuid / 空（tako の定義で未指定）/ 実在し得ない
#   index（`index:999` 等 = tako は見失って窓を開かずに終わる）だけ。「わざと当たらない面」を
#   指したい検査（#1697 の ④）は `TAKO_DISPLAY=index:999` を使う。opt-in で別の面を通す口は作らない
#
# 使い方:
#   . scripts/lib/isolated-gui.sh        # 関数として読む（直接実行はしない）
#
#   isolated_gui_bins                    # TAKO_BIN / APP_BIN を決める（無ければビルド）
#   launch_isolated_gui "$TMP/app.log" || exit $?   # ensure → 面の指定の判定 → env の既定 → 起動（pid は $ISOLATED_GUI_PID）
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
# **何も指定していないのか tako-vd を狙っているのかをスクリプトから読めるようにする**。
#
# 面を用意できたら**常に明示して渡す**（#1697 / #1744）: 明示した `TAKO_DISPLAY` を
# tako が見失うと、検証用 GUI は既定の面（= ユーザーの画面）へ落ちずに窓を開かずに終わる。
# 渡さない起動は tako の暗黙の既定（見つからなければ既定の面へ開いて警告を出す）に乗るので、
# このヘルパからは作らない（用意できなければそもそも起動しない = #1744）。
#
# **面を用意する係と同じ名前だけを見る**（#1760）: 以前は `ISOLATED_GUI_DISPLAY` を別に
# 差し替えられたが、`ensure` が用意するのは `TAKO_VD_NAME` の面なので、ずらすと
# 「tako-vd を用意して別の面へ窓を出す」口になる
ISOLATED_GUI_DISPLAY=${TAKO_VD_NAME:-tako-vd}

# 面を用意する係（`virtual-display.sh`）の置き場。**差し替えは番犬のモックのためだけ**
# （`issue1490_isolated_gui_launch_watchdog.rs` が「用意できない面」を注入する口）
ISOLATED_GUI_VD=${ISOLATED_GUI_VD:-}

# 面を用意できずに起動しなかったときの終了コード（#1744）。tako 本体が「窓を開かずに
# 終わる」ときの終了コード（`REFUSED_EXIT_CODE` = 4。#1160 / #1697）と同じ番号にそろえる
ISOLATED_GUI_RC_NO_DISPLAY=4

# tako-vd 以外の面を明示されて起動しなかったときの終了コード（#1760）。**4 とは分ける**:
# 4 は「環境が揃わない = 未実測」、こちらは呼び出し側の書き方の誤り（使い方の誤り = 2）で、
# 未実測と読ませて見逃させない
ISOLATED_GUI_RC_FOREIGN_DISPLAY=2

# この番号以上の index は「実在し得ない面」として通す（#1760）。実機の面は 100 枚に届かない
# ので、tako は必ず見失い、明示の見失いとして窓を開かずに終わる（#1697）
ISOLATED_GUI_ABSENT_INDEX_MIN=100

# 窓の矩形（`x,y,w,h` か `w,h`）。**既定は空 = 指定しない**。
# 既定を与えると置き先の中央 960x600 から変わり、窓の実寸を測る検証
# （`test-worker-min-width.sh` の cols）の実測値が動くので、欲しいスクリプトだけが置く
ISOLATED_GUI_BOUNDS=${ISOLATED_GUI_BOUNDS:-}

# `tako list` が通るまで待つ上限（0.1 秒 × この回数）
ISOLATED_GUI_WAIT_TRIES=${ISOLATED_GUI_WAIT_TRIES:-200}

# 直前の `launch_isolated_gui` が起こした pid（呼び出し側はこれを自分の変数へ受ける）
ISOLATED_GUI_PID=""

# 直前の `iso_ensure_display` が面を用意できなかった理由（1 行。用意できたら空）
ISOLATED_GUI_NO_DISPLAY_REASON=""

# 直前の `iso_display_allowed` が面の指定を通さなかった理由（1 行。通したら空）
ISOLATED_GUI_FOREIGN_REASON=""

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
# 終了コードは「窓を置ける面を用意できたか」。用意できなければ理由を 1 行
# `ISOLATED_GUI_NO_DISPLAY_REASON` へ置いて非ゼロ（#1744）。
#
# **用意できないときに素通しして続行しない**（#1744）: 以前は配線が無い環境（CI・他人の機）の
# ために「起動は続ける」で返していたが、その起動は面の指定を持たず、tako の暗黙の既定で
# ユーザーの画面へ落ちうる。`ensure` の応答だけを信じず、面が OS の一覧に居ることまで
# `bounds` で読み戻す（器の応答文を当てにしない作法 = virtual-display.sh の癖 ② と同じ）
iso_ensure_display() {
    local vd out rc=0 line reason=""
    vd="${ISOLATED_GUI_VD:-$(iso_repo_root)/scripts/lib/virtual-display.sh}"
    ISOLATED_GUI_NO_DISPLAY_REASON=""
    if [ ! -f "$vd" ]; then
        reason="面を用意する ${vd##*/} が見つからない"
    else
        out=$(bash "$vd" ensure 2>&1) || rc=$?
        if [ "$rc" -ne 0 ]; then
            # ensure の理由は最後の ERROR 行（「起こせなかった」「BetterDisplay が無い」等）
            while IFS= read -r line; do
                case "$line" in ERROR:*) reason=${line#ERROR: } ;; esac
            done <<< "$out"
            reason=${reason:-"virtual-display.sh ensure が終了コード ${rc} で失敗した"}
        elif ! bash "$vd" bounds >/dev/null 2>&1; then
            reason="ensure は成功を返したが ${ISOLATED_GUI_DISPLAY} が OS のディスプレイ一覧に無い"
        fi
    fi
    [ -z "$reason" ] && return 0
    ISOLATED_GUI_NO_DISPLAY_REASON=$reason
    return 1
}

# 面の指定（`TAKO_DISPLAY` の値）を GUI へ渡してよいかを判定する（#1760）。
# 通すなら 0、通さないなら理由を 1 行 `ISOLATED_GUI_FOREIGN_REASON` へ置いて非ゼロ。
#
# 通すのは「tako が tako-vd へ当てる」か「tako がどの面にも当てられない」値だけ
# （tako 側の当て方は crates/tako-core/src/platform/display.rs の select_with）:
#   - 空・空白だけ: tako の定義で未指定（explicit_request）= 検証用の既定で tako-vd を探す
#   - tako-vd の名前（大文字小文字は無視。tako と同じ）
#   - tako-vd の uuid: `ensure` が記録した値と一致するものだけ。記録が無ければ確かめられないので通さない
#   - 実在し得ない index（`index:N` / `N` で N >= ISOLATED_GUI_ABSENT_INDEX_MIN）
# **tako-vd を index で指すのは通さない**: index は OS の列挙順で決まり、判定から起動までに
# 面が眠る・繋がると並びがずれて別の面（ユーザーの画面）を指す。当たらないはずの名前
# （`no-such-display` 等）も通さない: その名前の面が無いことをこちらからは確かめられない
# （名前が読めない瞬間もある = #1697）。わざと当たらない面は index で指す
iso_display_allowed() {
    local raw="${1-}" spec lower recorded n vd
    local uuid_re='^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$'
    ISOLATED_GUI_FOREIGN_REASON=""
    # 前後の空白は tako と同じく無視する
    spec=${raw#"${raw%%[![:space:]]*}"}
    spec=${spec%"${spec##*[![:space:]]}"}
    [ -n "$spec" ] || return 0
    lower=$(printf '%s' "$spec" | tr '[:upper:]' '[:lower:]')
    [ "$lower" = "$(printf '%s' "$ISOLATED_GUI_DISPLAY" | tr '[:upper:]' '[:lower:]')" ] && return 0
    if [[ $lower =~ $uuid_re ]]; then
        # 記録を読むのも面を用意した係（iso_ensure_display と同じ差し替え口）
        vd="${ISOLATED_GUI_VD:-$(iso_repo_root)/scripts/lib/virtual-display.sh}"
        recorded=$(bash "$vd" recorded-uuid 2>/dev/null | tr '[:upper:]' '[:lower:]')
        [ -n "$recorded" ] && [ "$lower" = "$recorded" ] && return 0
        ISOLATED_GUI_FOREIGN_REASON="uuid ${spec} は ensure が記録した ${ISOLATED_GUI_DISPLAY} の uuid と一致しない。記録: ${recorded:-なし}"
        return 1
    fi
    n=${lower#index:}
    case "$n" in
        '' | *[!0-9]*) ;;
        *)
            # 桁が多いものは算術へ通さない（桁あふれ）。`08` を 8 進と読ませないよう 10# を付ける
            if [ "${#n}" -gt 6 ] || [ "$((10#$n))" -ge "$ISOLATED_GUI_ABSENT_INDEX_MIN" ]; then
                return 0
            fi
            ISOLATED_GUI_FOREIGN_REASON="index ${n} は実在しうる面を指す。並びは起動までに変わりうるので ${ISOLATED_GUI_DISPLAY} を指していても通さない"
            return 1
            ;;
    esac
    ISOLATED_GUI_FOREIGN_REASON="${spec} は ${ISOLATED_GUI_DISPLAY} の名前ではない"
    return 1
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

    # 面を起こす。**用意できなければ起動しない**（#1744）。呼び出し側が `TAKO_DISPLAY` を
    # 明示していても同じ（狙いが tako-vd なら見失っているし、別の面ならユーザーの画面へ出す）。
    # ISOLATED_GUI_PID は触らない（先に起こした GUI を trap の後片付けから外さないため）
    if ! iso_ensure_display; then
        echo "未実測: ${ISOLATED_GUI_DISPLAY} を用意できないので検証用 GUI を開かない（${ISOLATED_GUI_NO_DISPLAY_REASON}。状態は scripts/lib/virtual-display.sh status。#1744）" >&2
        return "$ISOLATED_GUI_RC_NO_DISPLAY"
    fi

    # 面の指定は常に明示する = tako は見失ったら既定の面へ落ちずに窓を開かずに終わる（#1697）。
    # 値は 1 つに決めてから判定する: 呼び出し側が並べた `TAKO_DISPLAY=`（最後のものが勝つ =
    # env と同じ）→ export された TAKO_DISPLAY（空は未指定扱い）→ tako-vd
    local display="${TAKO_DISPLAY:-$ISOLATED_GUI_DISPLAY}" arg
    for arg in "$@"; do
        case "$arg" in TAKO_DISPLAY=*) display=${arg#TAKO_DISPLAY=} ;; esac
    done
    # **tako-vd 以外の面の明示は通さない**（#1760。通すとユーザーの画面へ窓が出る）
    if ! iso_display_allowed "$display"; then
        iso_err "TAKO_DISPLAY=${display} は通さないので検証用 GUI を開かない（${ISOLATED_GUI_FOREIGN_REASON}。ヘルパ経由の起動が通すのは ${ISOLATED_GUI_DISPLAY} の名前か uuid だけで、わざと当たらない面は TAKO_DISPLAY=index:999 で指す。#1760）"
        return "$ISOLATED_GUI_RC_FOREIGN_DISPLAY"
    fi
    local -a env_args=("TAKO_ISOLATED=${TAKO_ISOLATED:-1}")
    [ -n "$ISOLATED_GUI_BOUNDS" ] && \
        env_args+=("TAKO_WINDOW_BOUNDS=${TAKO_WINDOW_BOUNDS:-$ISOLATED_GUI_BOUNDS}")
    # 呼び出し側が並べた `VAR=VAL` は既定より後ろ = そちらが勝つ。
    # 面の指定だけは判定した値を最後に置く = GUI へ届くのは必ず判定を通った値
    env ${env_args[@]+"${env_args[@]}"} "$@" "TAKO_DISPLAY=${display}" "$APP_BIN" > "$log" 2>&1 &
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
    echo "  launch_isolated_gui <log> [VAR=VAL…] 仮想ディスプレイを起こして隔離 GUI を起動（用意できなければ起動せず 4・tako-vd 以外の面の指定は起動せず 2）" >&2
    echo "  wait_isolated_gui [log] [試行回数]    tako list が通るまで待つ" >&2
    echo "  stop_isolated_gui [pid]              自分で起こした pid だけを落とす" >&2
    exit 1
fi
