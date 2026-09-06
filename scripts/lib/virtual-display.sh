#!/bin/bash
# 常設の仮想ディスプレイ（既定名 tako-vd）を用意して座標を返す共通ヘルパ（#1141）。
#
# 何のためか:
#   隔離 GUI（TAKO_ISOLATED=1 の tako-app・セルフテスト・visual-test）の窓を
#   **ユーザーのメイン画面に出さない**ための退避先を用意する。
#   窓をそこへ置くのは tako 本体（TAKO_DISPLAY / 隔離起動の既定。境界は
#   crates/tako-core/src/platform/display.rs）で、このヘルパは「面を用意する」係。
#
#   別 Space へ逃がすのでは代わりにならない: GPUI は窓が完全に隠れる（他窓に覆われる /
#   表示中でない Space にある）と描画を止める（#470 の実測）。退避先は OS から
#   **実ディスプレイとして見える面**である必要がある。
#
# 使い方:
#   . scripts/lib/virtual-display.sh   # 関数として読む
#   scripts/lib/virtual-display.sh ensure          # 無ければ作る（冪等）
#   scripts/lib/virtual-display.sh bounds          # "x y w h"（Quartz グローバル・ポイント）
#   scripts/lib/virtual-display.sh status          # 状態を 1 行
#   scripts/lib/virtual-display.sh move-window PID # その窓を仮想ディスプレイへ移す
#
# **消す機能は用意しない**（意図的）。tako-vd は常設で、作業のたびに作り直さない運用
#   （ユーザー指示 2026-09-06: 隔離 GUI の検証は今後ずっと仮想ディスプレイでやる）。
#   ユーザーのディスプレイ構成・解像度・配置・ミラーリングにも触らない。
#
# いまの実装は BetterDisplay の仮想スクリーン。他の実装（DeskPad 等）へ広げるときは
# vd_backend_* だけを差し替える（表に出る ensure / bounds / status は変えない）。

set -uo pipefail

VD_NAME=${TAKO_VD_NAME:-tako-vd}
VD_BD_APP=${TAKO_VD_BETTERDISPLAY_APP:-/Applications/BetterDisplay.app}
VD_BD_BIN="${VD_BD_APP}/Contents/MacOS/BetterDisplay"
# 仮想ディスプレイの左上からこれだけ内側へ窓を置く（move-window 用・ポイント）
VD_PAD_X=${TAKO_VD_PAD_X:-40}
VD_PAD_Y=${TAKO_VD_PAD_Y:-60}

vd_err() { echo "ERROR: $*" >&2; }

# ── BetterDisplay の癖（#1081 / #1141 で実測。4.3.5）─────────────────
#  ① アプリが起動していないと CLI は応答せず固まる → alarm で切り、先に open しておく
#  ② 応答文は当てにならない（`set -connected=on` が「Failed.」と言いながら実際は繋がる）
#     → 結果は必ず**読み戻して**確かめる（このファイルは NSScreen で確かめる）
#  ③ 接続は tagID 指定で 1 回だけ。`-name=` 指定は複数オブジェクトに当たり、同じ画面が
#     何枚も繋がることがある
#  ④ 仮想スクリーンの tagID は操作のたびに並べ替わる → 使う直前に引き直す
#  ⑤ 仮想スクリーンの作成・接続に Pro は要らない
#  ⑥ `get -connected` も identifiers の `displayID` も当てにならない（接続中でも off /
#     0 を返すのを実測）。**繋がっているか = 窓を置けるか**なので、判定は NSScreen
#     （vd_screens）を正にする。この 1 本だけ見ていれば器の実装が変わっても壊れない
#  ⑦ 接続直後は **macOS の配置が落ち着くまで座標が動く**（#1141 で実測: 3 枚目を繋いだ
#     直後は既存の面と同じ x を返し、数秒後に本来の x へ移った）。ensure は座標が
#     2 回続けて同じ値になるまで待ってから返す
vd_bd() {
    perl -e 'alarm 25; exec @ARGV' "$VD_BD_BIN" "$@" 2>/dev/null
}

# いま OS が見ているディスプレイを "name<TAB>x<TAB>y<TAB>w<TAB>h" で 1 行ずつ。
# 座標は **Quartz グローバル（左上原点・y は下向き）** のポイントへ直してある
# （AppKit の frame は左下原点なので、主画面の高さで y を反転する。tako 本体の
# layout.json の window フレームや System Events の position と同じ系）。
# NSScreen に出ている = 窓を置ける、なので接続確認もこれで足りる。
vd_screens() {
    osascript -l JavaScript -e '
ObjC.import("AppKit");
var s = $.NSScreen.screens, out = [];
if (s.count > 0) {
  var h0 = ObjC.unwrap(s.objectAtIndex(0).frame).size.height;
  for (var i = 0; i < s.count; i++) {
    var sc = s.objectAtIndex(i), f = ObjC.unwrap(sc.frame);
    out.push([ObjC.unwrap(sc.localizedName),
              Math.round(f.origin.x),
              Math.round(h0 - (f.origin.y + f.size.height)),
              Math.round(f.size.width),
              Math.round(f.size.height)].join("\t"));
  }
}
out.join("\n")' 2>/dev/null
}

# 仮想ディスプレイの行（"name x y w h"）。OS に見えていなければ 1
vd_row() {
    vd_screens | awk -F'\t' -v n="$VD_NAME" '$1==n {print $2, $3, $4, $5; found=1; exit} END {exit found ? 0 : 1}'
}

# BetterDisplay の仮想スクリーン一覧（tagID<TAB>displayID<TAB>name）
vd_backend_list() {
    vd_bd get -identifiers | /usr/bin/python3 -c '
import sys, json
raw = sys.stdin.read().strip()
try:
    data = json.loads("[" + raw + "]")
except Exception:
    sys.exit(0)
for d in data:
    if d.get("deviceType") == "VirtualScreen":
        print("\t".join(str(d.get(k, "")) for k in ("tagID", "displayID", "name")))
'
}

vd_backend_tag() {
    vd_backend_list | awk -F'\t' -v n="$VD_NAME" '$3==n {print $1; exit}'
}

# BetterDisplay を（未起動なら）起こして CLI が応答するまで待つ
vd_backend_ready() {
    if [ ! -x "$VD_BD_BIN" ]; then
        vd_err "仮想ディスプレイを作る BetterDisplay が無い（${VD_BD_APP}）"
        echo "       導入: brew install --cask betterdisplay" >&2
        echo "       CLI も入れるなら: brew install waydabber/betterdisplay/betterdisplaycli" >&2
        return 1
    fi
    if ! pgrep -x BetterDisplay >/dev/null 2>&1; then
        echo "   BetterDisplay を起動します（仮想ディスプレイの器）"
        open -g -a "$VD_BD_APP" || return 1
    fi
    local i
    for i in $(seq 1 30); do
        vd_bd get -identifiers | grep -q deviceType && return 0
        sleep 1
    done
    vd_err "BetterDisplay の CLI が応答しない（メニューバーにアイコンが出ているか確認）"
    return 1
}

# 常設の仮想ディスプレイを使える状態にする。冪等（既に OS に見えていれば何もしない）
vd_ensure() {
    if vd_row >/dev/null; then
        return 0
    fi
    vd_backend_ready || return 1
    local tag i
    tag=$(vd_backend_tag)
    if [ -z "$tag" ]; then
        echo "   仮想スクリーン ${VD_NAME} を作成（16:9 / HiDPI・常設）"
        vd_bd create -type=VirtualScreen "-virtualScreenName=${VD_NAME}" \
            -aspectWidth=16 -aspectHeight=9 -virtualScreenHiDPI=on >/dev/null || true
        for i in $(seq 1 10); do
            tag=$(vd_backend_tag); [ -n "$tag" ] && break; sleep 1
        done
        [ -n "$tag" ] || {
            vd_err "仮想スクリーン ${VD_NAME} を作れない（BetterDisplay の Virtual screens で手動作成する）"
            return 1
        }
    fi
    # 繋がっていないときだけ tagID 指定で 1 回。応答文は見ずに NSScreen で確かめる（癖 ②③）。
    # tagID は操作のたびに並べ替わるので**打つ直前に引き直す**（癖 ④）
    echo "   仮想スクリーン ${VD_NAME} を接続"
    tag=$(vd_backend_tag)
    vd_bd set "-tagID=${tag}" -connected=on >/dev/null || true
    for i in $(seq 1 20); do
        vd_row >/dev/null && { vd_settle; return 0; }
        sleep 1
    done
    vd_err "仮想ディスプレイ ${VD_NAME} が OS のディスプレイ一覧に現れない"
    return 1
}

# 配置が落ち着く（座標が 2 回続けて同じになる）まで待つ（癖 ⑦）。
# 落ち着かなくても諦めて返す = 座標が要る側は bounds を読み直せばよい
vd_settle() {
    local prev="" now i
    for i in $(seq 1 10); do
        now=$(vd_row) || return 0
        [ "$now" = "$prev" ] && return 0
        prev=$now
        sleep 0.5
    done
}

# "x y w h" を出す（用意できていなければ非ゼロ）
vd_bounds() {
    vd_row || { vd_err "仮想ディスプレイ ${VD_NAME} が見つからない（先に ensure すること）"; return 1; }
}

vd_status() {
    local r
    if r=$(vd_row); then
        echo "${VD_NAME}: 使用可（${r}）"
    else
        echo "${VD_NAME}: 未接続（ensure で用意する）"
        return 1
    fi
}

# 窓を仮想ディスプレイへ移す（System Events の AX 経由。GPUI の窓にも効く = 実測）。
# tako 本体は TAKO_DISPLAY で最初からそこへ開くので、これは **TAKO_DISPLAY に対応しない
# 旧バイナリや tako 以外の窓**向けの当座しのぎ
vd_move_window() {
    local pid=$1 r x y
    r=$(vd_row) || { vd_err "仮想ディスプレイ ${VD_NAME} が見つからない"; return 1; }
    x=$(( $(echo "$r" | awk '{print $1}') + VD_PAD_X ))
    y=$(( $(echo "$r" | awk '{print $2}') + VD_PAD_Y ))
    osascript -e "tell application \"System Events\" to tell (first application process whose unix id is ${pid}) to set position of window 1 to {${x}, ${y}}" >/dev/null 2>&1 || {
        vd_err "窓を移せない（pid=${pid}。システム設定 > プライバシーとセキュリティ > アクセシビリティ / オートメーションの許可を確認）"
        return 1
    }
}

# 直接実行されたときだけサブコマンドとして動く（source されたら関数だけ提供する）
if [ "${BASH_SOURCE[0]}" = "${0}" ]; then
    case "${1:-}" in
        ensure) vd_ensure ;;
        bounds) vd_bounds ;;
        status) vd_status ;;
        move-window)
            [ $# -ge 2 ] || { vd_err "move-window には pid が要る"; exit 1; }
            vd_move_window "$2"
            ;;
        *)
            echo "使い方: ${0} {ensure|bounds|status|move-window <pid>}" >&2
            echo "  ensure       仮想ディスプレイ ${VD_NAME} を用意する（冪等・消す機能は無い）" >&2
            echo "  bounds       \"x y w h\"（Quartz グローバル・ポイント）" >&2
            echo "  status       状態を 1 行" >&2
            echo "  move-window  その pid の窓を仮想ディスプレイへ移す" >&2
            exit 1
            ;;
    esac
fi
