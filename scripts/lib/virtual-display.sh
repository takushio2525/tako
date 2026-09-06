#!/bin/bash
# 常設の仮想ディスプレイ（既定名 tako-vd）を用意して座標を返す共通ヘルパ（#1141 / #1150）。
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
#   scripts/lib/virtual-display.sh ensure               # 無ければ作る（冪等）
#   scripts/lib/virtual-display.sh bounds               # "x y w h"（Quartz グローバル・ポイント）
#   scripts/lib/virtual-display.sh status                # 状態を 1 行
#   scripts/lib/virtual-display.sh status --snapshot     # 前後比較用の機械可読な現況
#   scripts/lib/virtual-display.sh move-window PID       # その窓を仮想ディスプレイへ移す
#   scripts/lib/virtual-display.sh cleanup-orphans        # 孤児の下見（何も変えない）
#   scripts/lib/virtual-display.sh cleanup-orphans --apply # 実行条件を満たすときだけ掃除
#
# **仮想ディスプレイを消す機能は用意しない**（意図的）。tako-vd は常設で、作業のたびに
#   作り直さない運用（ユーザー指示 2026-09-06: 隔離 GUI の検証は今後ずっと仮想ディスプレイ
#   でやる）。ユーザーのディスプレイ構成・解像度・配置・ミラーリングにも触らない。
#   cleanup-orphans が落とすのは **器の管理外へ外れた残骸（孤児）** だけで、
#   tako-vd の device 定義は消さない（器を再起動しても定義は残り、ensure で繋ぎ直す）。
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

# ── BetterDisplay の癖（#1081 / #1141 / #1150 で実測。4.3.5）───────────
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
#  ⑧ **解釈できない引数を渡すと CLI として動かず「アプリの追加実体」として起きる**
#     （#1150 の実測: `create -virtualScreen -name=… -width=…` で起こされた実体が
#     5 時間走り続けていた。`--help` でも同じ）。実体が増えると同じ仮想スクリーンが
#     何枚も繋がりうるので、**引数は help に在るものだけを渡す**。増えた実体は
#     vd_backend_instances で数えられる
vd_bd() {
    perl -e 'alarm 25; exec @ARGV' "$VD_BD_BIN" "$@" 2>/dev/null
}

# いま OS が見ているディスプレイを 1 行 1 面のタブ区切りで:
#   name<TAB>x<TAB>y<TAB>w<TAB>h<TAB>displayID<TAB>builtin(0|1)<TAB>main(0|1)
#
# 座標は **Quartz グローバル（左上原点・y は下向き）** のポイントへ直してある
# （AppKit の frame は左下原点なので、主画面の高さで y を反転する。tako 本体の
# layout.json の window フレームや System Events の position と同じ系）。
# NSScreen に出ている = 窓を置ける、なので接続確認もこれで足りる。
#
# builtin / main は CoreGraphics に聞く（CGDisplayIsBuiltin / CGMainDisplayID）。
# 名前で内蔵を見分けるのはロケール依存で脆いので使わない。
vd_screens() {
    osascript -l JavaScript -e '
ObjC.import("AppKit");
ObjC.import("CoreGraphics");
var s = $.NSScreen.screens, out = [];
if (s.count > 0) {
  var h0 = ObjC.unwrap(s.objectAtIndex(0).frame).size.height;
  var mainID = $.CGMainDisplayID();
  for (var i = 0; i < s.count; i++) {
    var sc = s.objectAtIndex(i), f = ObjC.unwrap(sc.frame);
    var did = ObjC.unwrap(sc.deviceDescription.objectForKey("NSScreenNumber"));
    out.push([ObjC.unwrap(sc.localizedName),
              Math.round(f.origin.x),
              Math.round(h0 - (f.origin.y + f.size.height)),
              Math.round(f.size.width),
              Math.round(f.size.height),
              did,
              $.CGDisplayIsBuiltin(did) ? 1 : 0,
              did === mainID ? 1 : 0].join("\t"));
  }
}
out.join("\n")' 2>/dev/null
}

# 蓋の生の状態（ioreg の出力そのまま）。テストから差し替えられるよう関数にしてある
vd_clamshell_raw() {
    ioreg -r -k AppleClamshellState -d 4 2>/dev/null
}

# いま走っている器（BetterDisplay）の実体数。**2 つ以上は増殖の既知原因**（癖 ⑧）
vd_backend_instances() {
    pgrep -x BetterDisplay 2>/dev/null | grep -c . || true
}

# **引数つきで走っている器の実体**を "pid コマンド行" で 1 行ずつ（癖 ⑧）。
# 正規の実体は引数なしで走っている。引数つきの実体は「CLI を叩いたつもりが
# アプリの追加実体として起きたまま終わらなかった」もので、同じ仮想スクリーンを
# 二重に繋ぐ疑いがある（#1150 の実測: 誤った create で起きた実体が 5 時間走り続けていた）
vd_backend_strays() {
    ps -eo pid,args 2>/dev/null | awk -v bin="$VD_BD_BIN" '$2 == bin && NF > 2'
}

# ── ここから純関数（副作用なし・入力は引数と stdin）───────────────────
# 検証は scripts/test-virtual-display-guard.sh（実ディスプレイに触らずスタブで回す）

# 枝番（`名前（1）` / `名前 (1)`）を落とした基準名。
# macOS は同じ名前の面が 2 枚以上あると**枝番を付けて見せる**（#1150 の実測: 1 回の
# ensure で `tako-vd-ab1141（1）`「（2）」の 2 枚が生えた）。枝番を無視して比べないと
# 「その名前は無い」に見え、ensure がもう 1 枚繋いで増殖に気づけない
vd_strip_index() {
    local s=$1
    # 全角・半角の括弧はブラケットに入れず**そのまま並べる**（bash の =~ はロケールに
    # よってブラケット内の多バイト文字をバイト単位で扱うため）
    local re='^(.+)(（|[(])[0-9]+(）|[)])$'
    if [[ $s =~ $re ]]; then
        s=${BASH_REMATCH[1]}
        while [ "${s% }" != "$s" ]; do s=${s% }; done
    fi
    printf '%s' "$s"
}

# stdin のテーブルから「基準名が $1 と一致する行」だけを出す（枝番付きも一致扱い）
vd_rows_named() {
    local want=$1 line name
    while IFS= read -r line; do
        [ -n "$line" ] || continue
        name=$(vd_strip_index "${line%%$'\t'*}")
        [ "$name" = "$want" ] && printf '%s\n' "$line"
    done
    return 0
}

# stdin のテーブルから「名前が $1 で始まる行」= このヘルパが作る系統だけを出す
vd_family_rows() {
    local prefix=$1 line name
    while IFS= read -r line; do
        [ -n "$line" ] || continue
        name=${line%%$'\t'*}
        case $name in "$prefix"*) printf '%s\n' "$line" ;; esac
    done
    return 0
}

# stdin（vd_family_rows の出力）から孤児だけを出す。
# 残すのは「基準名がちょうど $1 の行」の 1 行目だけ。それ以外（枝番の 2 枚目以降・
# `tako-vd-ab1141` のような一時名）はすべて孤児
vd_orphan_rows() {
    local base=$1 line name kept=0
    while IFS= read -r line; do
        [ -n "$line" ] || continue
        name=$(vd_strip_index "${line%%$'\t'*}")
        if [ "$name" = "$base" ] && [ "$kept" = 0 ]; then
            kept=1
            continue
        fi
        printf '%s\n' "$line"
    done
    return 0
}

# Main 保護の判定（純関数）。stdin はテーブル、$1 は常設の仮想ディスプレイの基準名。
# 出力は 1 行:
#   noop <理由>              触らない
#   restore <id> <名前>      その内蔵ディスプレイを Main へ戻す
#
# **戻すのは「Main が我々の仮想ディスプレイ」かつ「内蔵が居る」ときだけ**（#1150）。
# 内蔵が居ないとき（蓋閉じ）は戻す先が無いので何もしない。外部モニタが Main のときは
# ユーザーの構成なので触らない（#1141 の約束: 他人のディスプレイ構成を変えない）
vd_main_protection_plan() {
    local base=$1
    local name x y w h did builtin is_main
    local builtin_id="" builtin_name="" main_name="" main_is_virtual=0 main_seen=0
    while IFS=$'\t' read -r name x y w h did builtin is_main; do
        [ -n "${name:-}" ] || continue
        if [ "${builtin:-0}" = 1 ] && [ -z "$builtin_id" ]; then
            builtin_id=${did:-}
            builtin_name=$name
        fi
        if [ "${is_main:-0}" = 1 ] && [ "$main_seen" = 0 ]; then
            main_seen=1
            main_name=$name
            [ "$(vd_strip_index "$name")" = "$base" ] && main_is_virtual=1
        fi
    done
    if [ "$main_seen" = 0 ]; then
        echo "noop Main が読めない（見える面が 0 枚 / ディスプレイスリープ中）"
        return 0
    fi
    if [ "$main_is_virtual" = 0 ]; then
        echo "noop Main は ${main_name} で仮想ディスプレイではない"
        return 0
    fi
    if [ -z "$builtin_id" ]; then
        echo "noop Main は ${main_name} だが内蔵ディスプレイが居ないので戻す先が無い"
        return 0
    fi
    echo "restore ${builtin_id} ${builtin_name}"
}

# 蓋の状態（純関数。$1 は vd_clamshell_raw の出力）。open / closed / unknown。
# デスクトップ機は蓋が無いのでキー自体が出ない = unknown
vd_clamshell_state() {
    case ${1:-} in
        *'"AppleClamshellState" = Yes'*) echo closed ;;
        *'"AppleClamshellState" = No'*) echo open ;;
        *) echo unknown ;;
    esac
}

# 孤児の後片付けを実行してよいか（純関数）。stdin はテーブル、$1 は蓋の状態。
#   ok            実行してよい（終了 0）
#   block <理由>  実行してはいけない（終了 1）
#
# 器の再起動は**一瞬すべての仮想ディスプレイを落とす**ので、内蔵が NSScreen に
# 居ないときに走らせると**画面が 0 枚になり機械が眠る**（#1150: 蓋閉じで tako-vd が
# 唯一の画面。走っている worker が全部巻き添えになる）。
# 内蔵の有無と蓋の状態は**独立に**見る（片方の読み違いで通ってしまわないように）
vd_cleanup_gate() {
    local clamshell=${1:-unknown}
    local name x y w h did builtin is_main
    local builtin_seen=0 rows=0
    while IFS=$'\t' read -r name x y w h did builtin is_main; do
        [ -n "${name:-}" ] || continue
        rows=$((rows + 1))
        [ "${builtin:-0}" = 1 ] && builtin_seen=1
    done
    if [ "$rows" = 0 ]; then
        echo "block ディスプレイが 1 枚も読めない（ディスプレイスリープ中）"
        return 1
    fi
    if [ "$builtin_seen" = 0 ]; then
        echo "block 内蔵ディスプレイが NSScreen に居ない（器を再起動すると画面が 0 枚になる）"
        return 1
    fi
    if [ "$clamshell" = closed ]; then
        echo "block 蓋が閉じている（内蔵が居ても眠れば画面が 0 枚になる）"
        return 1
    fi
    echo ok
    return 0
}

# ── ここから副作用のある操作 ────────────────────────────────────

# 仮想ディスプレイの行（"x y w h"）。OS に見えていなければ 1。
# 枝番付きも同名として拾う（増えていても座標は読める = 検証は続けられる）
vd_row() {
    local row
    row=$(vd_screens | vd_rows_named "$VD_NAME" | head -1)
    [ -n "$row" ] || return 1
    printf '%s\n' "$row" | awk -F'\t' '{print $2, $3, $4, $5}'
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

# 同名が 2 枚以上に増えていないか（#1150）。増えていたら理由を出して非ゼロ。
# 「無いから作る」の判定と違って**増えたことは自分では直せない**（器の管理外へ
# 外れた面は器から消せない = 実測）ので、黙って進まずここで止める
vd_assert_single() {
    local rows count instances
    rows=$(vd_screens | vd_rows_named "$VD_NAME")
    count=$(printf '%s' "$rows" | grep -c . || true)
    [ "$count" -le 1 ] && return 0
    vd_err "仮想ディスプレイ ${VD_NAME} が ${count} 枚ある（常設の 1 枚だけであるべき）"
    printf '%s\n' "$rows" | sed 's/^/       /' >&2
    instances=$(vd_backend_instances)
    if [ "${instances:-0}" -gt 1 ]; then
        echo "       器（BetterDisplay）が ${instances} 実体走っている = 増殖の既知原因（#1150）" >&2
        echo "       解釈できない引数を渡すと CLI ではなくアプリの追加実体として起きる" >&2
        vd_backend_strays | sed 's/^/         引数つきの実体: /' >&2
    fi
    echo "       下見: scripts/lib/virtual-display.sh cleanup-orphans" >&2
    return 1
}

# Main が仮想ディスプレイのままなら内蔵へ戻す（内蔵が居るときだけ・#1150）。
# **失敗しても ensure は成功させる**（面は用意できているので検証は進められる。
# ここで止めると「窓を出さない」という本来の目的まで巻き添えになる）
vd_protect_main() {
    local plan did name i now
    plan=$(vd_screens | vd_main_protection_plan "$VD_NAME")
    case $plan in
        restore*)
            did=$(printf '%s' "$plan" | awk '{print $2}')
            name=$(printf '%s' "$plan" | cut -d' ' -f3-)
            echo "   Main を内蔵ディスプレイ（${name}）へ戻します（仮想を Main にしない）"
            vd_bd set "-displayID=${did}" -main=on >/dev/null || true
            # 応答文は当てにならないので読み戻して確かめる（癖 ②）
            for i in $(seq 1 6); do
                now=$(vd_screens | vd_main_protection_plan "$VD_NAME")
                case $now in
                    restore*) sleep 0.5 ;;
                    *) return 0 ;;
                esac
            done
            vd_err "Main を内蔵へ戻せなかった（手動: システム設定 > ディスプレイ > 配置）"
            ;;
    esac
    return 0
}

# 用意できた直後に必ず通す締め（#1150）。増殖の検査 → Main 保護の順。
# **ensure が成功で返る道はすべてここを通る**（番犬が拘束している）
vd_finish_ensure() {
    vd_assert_single || return 1
    vd_protect_main
    return 0
}

# 常設の仮想ディスプレイを使える状態にする。冪等（既に OS に見えていれば繋がない）
vd_ensure() {
    if vd_row >/dev/null; then
        vd_finish_ensure
        return $?
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
        if vd_row >/dev/null; then
            vd_settle
            vd_finish_ensure
            return $?
        fi
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
    local r plan clamshell main_note rc=0
    if r=$(vd_row); then
        echo "${VD_NAME}: 使用可（${r}）"
    else
        echo "${VD_NAME}: 未接続（ensure で用意する）"
        rc=1
    fi
    plan=$(vd_screens | vd_main_protection_plan "$VD_NAME")
    case $plan in
        restore*) main_note="要（内蔵 $(printf '%s' "$plan" | cut -d' ' -f3-) へ戻す）" ;;
        *) main_note="不要（${plan#noop }）" ;;
    esac
    clamshell=$(vd_clamshell_state "$(vd_clamshell_raw)")
    echo "  Main 保護: ${main_note} / 蓋: ${clamshell} / 器の実体: $(vd_backend_instances)"
    return $rc
}

# 前後比較用の機械可読な現況（#1150 の受け入れ条件「作業の前後で Main と内蔵の
# 有無が変わらない」を diff で示せる形）。行の並びは名前順で安定させる
vd_status_snapshot() {
    echo "clamshell=$(vd_clamshell_state "$(vd_clamshell_raw)")"
    echo "backend_instances=$(vd_backend_instances)"
    vd_screens | sort | awk -F'\t' 'NF { printf "screen\tname=%s\tframe=%s,%s %sx%s\tid=%s\tbuiltin=%s\tmain=%s\n", $1, $2, $3, $4, $5, $6, $7, $8 }'
}

# 孤児の下見と後片付け（#1150）。既定は**下見だけ**（何も変えない）。
#
# 直せる手段が器の再起動しかない理由: 器の管理外へ外れた仮想スクリーンは、器の
# 削除操作を tagID / displayID / nameLike のどれで指定しても「Failed.」（#1150 の実測）。
# 再起動しても **tako-vd の device 定義は消えない**ので、掃除のあと ensure で繋ぎ直す。
#
# 実行条件（内蔵が NSScreen に居る + 蓋が開いている）を満たさなければ**拒否する**。
# 満たさないまま走らせると画面が 0 枚になり機械が眠る
vd_cleanup_orphans() {
    local apply=0
    case ${1:-} in
        --apply) apply=1 ;;
        --dry-run | "") apply=0 ;;
        *)
            vd_err "cleanup-orphans の引数は --dry-run（既定）か --apply"
            return 1
            ;;
    esac
    local table orphans count gate clamshell
    table=$(vd_screens)
    orphans=$(printf '%s\n' "$table" | vd_family_rows "$VD_NAME" | vd_orphan_rows "$VD_NAME")
    count=$(printf '%s' "$orphans" | grep -c . || true)
    if [ "$count" = 0 ]; then
        echo "孤児なし（${VD_NAME} の系統は 1 枚）"
        return 0
    fi
    echo "孤児 ${count} 枚:"
    printf '%s\n' "$orphans" |
        awk -F'\t' 'NF { printf "  %s（%s,%s %sx%s displayID=%s）\n", $1, $2, $3, $4, $5, $6 }'
    local strays
    strays=$(vd_backend_strays)
    if [ -n "$strays" ]; then
        echo "引数つきで走っている器の実体（同じ面を二重に繋ぐ疑い・#1150 の癖 ⑧）:"
        printf '%s\n' "$strays" | sed 's/^/  /'
        echo "  掃除しても孤児が残るなら、この pid を落としてからもう一度試す"
    fi
    clamshell=$(vd_clamshell_state "$(vd_clamshell_raw)")
    if ! gate=$(printf '%s\n' "$table" | vd_cleanup_gate "$clamshell"); then
        vd_err "後片付けは実行できない: ${gate#block }"
        echo "       蓋を開けて内蔵ディスプレイが NSScreen に戻ってから、これを実行する:" >&2
        echo "         scripts/lib/virtual-display.sh cleanup-orphans --apply" >&2
        return 2
    fi
    if [ "$apply" = 0 ]; then
        echo "実行条件は満たしている（下見なので何もしない）"
        echo "掃除する: scripts/lib/virtual-display.sh cleanup-orphans --apply"
        return 0
    fi
    echo "   器（BetterDisplay）を再起動して孤児を落とします（tako-vd の定義は残る）"
    vd_bd set -restartApp >/dev/null || true
    sleep 3
    vd_backend_ready || return 1
    vd_ensure || return 1
    orphans=$(vd_screens | vd_family_rows "$VD_NAME" | vd_orphan_rows "$VD_NAME")
    if [ -n "$orphans" ]; then
        vd_err "孤児が残っている（器の再起動では落ちない = 機械の再起動が要る）"
        printf '%s\n' "$orphans" | sed 's/^/       /' >&2
        return 1
    fi
    echo "孤児なし（${VD_NAME} は 1 枚）"
    return 0
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
        status)
            case "${2:-}" in
                --snapshot) vd_status_snapshot ;;
                "") vd_status ;;
                *) vd_err "status の引数は --snapshot だけ"; exit 1 ;;
            esac
            ;;
        cleanup-orphans) vd_cleanup_orphans "${2:-}" ;;
        move-window)
            [ $# -ge 2 ] || { vd_err "move-window には pid が要る"; exit 1; }
            vd_move_window "$2"
            ;;
        *)
            echo "使い方: ${0} {ensure|bounds|status [--snapshot]|move-window <pid>|cleanup-orphans [--apply]}" >&2
            echo "  ensure           仮想ディスプレイ ${VD_NAME} を用意する（冪等・消す機能は無い）" >&2
            echo "  bounds           \"x y w h\"（Quartz グローバル・ポイント）" >&2
            echo "  status           状態を 1 行（--snapshot は前後比較用の機械可読な現況）" >&2
            echo "  move-window      その pid の窓を仮想ディスプレイへ移す" >&2
            echo "  cleanup-orphans  孤児の下見（--apply は実行条件を満たすときだけ掃除）" >&2
            exit 1
            ;;
    esac
fi
