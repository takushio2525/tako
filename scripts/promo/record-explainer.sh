#!/bin/bash
# tako:run: bash scripts/promo/record-explainer.sh all
# tako 解説動画（#1081）のシーン収録。
#
# 使い方:
#   scripts/promo/record-explainer.sh <scene>
#     scene = scatter | control | agent | setup | basics | master | guimode | restore | remote | windows | all
#
#   scatter … 1. 課題（4 つのタブに散らばる → 1 タブへ集約）
#   control … 2. 思想（CLI が GUI を動かす = AI フルコントロール）
#   agent   … 2. 思想（実 Claude Code が MCP でペインを割る。#470 の agent 相当）
#   setup   … 3. 導入（brew カード → bootstrap の導入計画 → tako setup → 対話アシスタント）
#   basics  … 4. 基本操作（分割 / タブ / ツリー / md ライブリロード / PDF / 画像 / コード / Code Runner）
#   master  … 5. AI に任せる（tako master → worker 3 体 → orch ビュー → かんたん表示 → 報告）
#   guimode … 5. かんたん表示の手順デモ（新タブ → GUI 切替 → ボタン 3 枚 → master 起動 → worker）
#   restore … 6. 再起動しても戻る（終了直前 / 再起動後の 2 クリップ）
#   remote  … 7. スマホから（Remote Control のプロファイル opt-in。PWA は record-pwa.cjs）
#   windows … 8. Windows と OSS（対応状況ページ / LICENSE / README / brew カード）
#
# 各シーンは lib.sh の隔離インスタンス（TAKO_ISOLATED=1 + 明示ソケット + デモ HOME）で撮る。
# 隔離 tako の窓は**仮想ディスプレイ**（BetterDisplay の `tako-vd`。常設）へ出すので、
# 収録中もユーザーのメイン画面には何も出ない（`TAKO_PROMO_STAGE=main` で従来方式）。
# 収録の開始と終了は `RECORDING START <scene>-raw ...` / `RECORDING END <scene>-raw ...` の
# 1 行ずつで分かる（GUI 再起動などを避けてもらうための合図）。
# 収録中に CLI 操作をした瞬間を promo_beat で <scene>-beats.tsv に残し、
# build-explainer.sh がそれを in 点にしてテロップ・ナレーションを合わせる。
# 出力: ~/Desktop/tako-promo/scenes/<scene>-raw.mp4 + <scene>-beats.tsv
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
# shellcheck source=lib.sh
source "$SCRIPT_DIR/lib.sh"

SCENE=${1:-}
[ -n "$SCENE" ] || { echo "usage: $0 <scatter|control|agent|setup|basics|master|guimode|restore|remote|windows|all>" >&2; exit 2; }
promo_require

FAST_MODEL=${TAKO_PROMO_MODEL:-claude-sonnet-5}   # 収録のテンポ用（挙動は変わらない）
FONT_SIZE=${TAKO_PROMO_FONT_SIZE:-15}

# シーン共通の前処理: 作業ディレクトリ・デモ環境・16:9 ウインドウ・隔離起動
explainer_begin() {
    local scene=$1 work=$2 socket=$3 persist=${4:-0}
    echo "== scene $scene"
    promo_wait_capturable "${TAKO_PROMO_WAIT_UNLOCK:-0}" || promo_check_capturable
    # 舞台（仮想ディスプレイ）を先に用意する = 窓の seed 位置がその画面の中になる
    promo_stage_prepare || return 1
    rm -rf "$work"
    promo_seed_window_frame "$work" 960 540
    promo_start_isolated "$work" "$socket" "$persist"
    PROMO_BEATS_FILE="$PROMO_OUT/scenes/$scene-beats.tsv"
    : > "$PROMO_BEATS_FILE"
    tko theme --size "$FONT_SIZE" >/dev/null 2>&1 || true
    # 収録機と無関係な listen ポート（他アプリ）の提案チップが写り込まないようにする
    tko portdetect off >/dev/null 2>&1 || true
}

# 素のシェルのペインに 1 行打つ（画面に打った行がそのまま見える）
type_cmd() { tko send --pane "$1" "$2" >/dev/null; }

# ── GUI 操作（#1081 の GUI モード章）────────────────────────────────
# 押し場所は**窓内ピクセル**（1920x1080 基準・左上原点）で書く。窓は 960x540pt の
# Retina なので pt = px / 2。ここは 2026-09-09 に実測した値で、
#   - 「+」はタブ 1 の見出し幅で動く → タブ名を `awesome-app` に固定した状態の値
#   - トグルは右端からの固定位置（タブ枚数に依らない）
# 外したときに黙って進まないよう、押したあとは必ず `promo_check_*` で状態を確かめる
GUI_PLUS_X=554;    GUI_PLUS_Y=43
GUI_TOGGLE_X=1797; GUI_TOGGLE_Y=43
PROMO_CLICKS_FILE=${PROMO_CLICKS_FILE:-}

# 収録開始からの経過秒（promo_beat と同じ基準）
promo_rec_elapsed() {
    local marker="$PROMO_WORK/rec.start"
    [ -s "$marker" ] || { printf '0.00'; return 0; }
    /usr/bin/python3 -c 'import sys,time; print(f"{time.time()-float(open(sys.argv[1]).read()):.2f}")' "$marker"
}

# 窓内ピクセル → CG のグローバル座標（pt）
promo_win_to_global() {
    local px=$1 py=$2 b
    b=$("$PROMO_WINBOUNDS" "$PROMO_APP_PID" 2>/dev/null) || return 1
    /usr/bin/python3 -c "
import sys
_, x, y, w, h = sys.argv[1].split()[:5]
print(int(float(x) + $px / 2), int(float(y) + $py / 2))" "$b"
}

# 実クリック（状態が変わるまで最大 3 回。clicks.tsv には**効いた 1 回だけ**を残す
# = 注釈のポインタが空振りを描かない）。
#
# **押すと窓が前面に出る**（macOS の標準挙動）ので、押した直後に元のアプリへフォーカスを
# 返す。返さないと**ユーザーのキー入力が隔離ウインドウへ流れ込む**（2026-09-09 実測:
# 前面に出したまま数秒置いた回で、シェルに「う」が 1 文字混ざって
# `う/Applications/…/tako master` になり master が起動しなかった）。
# 押し場所を外していないのに効かないときは、**別の窓が上に重なっている**ことを疑う
# （promo_force_window_frame が起動時に排除する。lib.sh の注記）
# $1 = px, $2 = py, $3.. = 効いたかを返す検査コマンド
promo_click_until() {
    local px=$1 py=$2; shift 2
    local g t i rc=1 front_before
    g=$(promo_win_to_global "$px" "$py") || { echo "ERROR: 窓の矩形が読めない" >&2; return 1; }
    front_before=$(promo_frontmost_pid)
    for i in 1 2 3; do
        t=$(promo_rec_elapsed)
        # shellcheck disable=SC2086
        "$PROMO_CLICK" ${g} --hover-ms 420 >/dev/null || break
        # 検査より先にフォーカスを返す（ユーザーのキー入力を拾う窓を最短にする）
        promo_give_back_focus "$front_before"
        if "$@"; then
            [ -n "$PROMO_CLICKS_FILE" ] && printf 'click\t%s\t%s\t%s\n' "$t" "$px" "$py" \
                >> "$PROMO_CLICKS_FILE"
            echo "   click ${px},${py} @ ${t}s（${i} 回目で反応）"
            rc=0
            break
        fi
        echo "   click ${px},${py}: 反応なし（${i} 回目）"
    done
    if [ "$rc" != 0 ]; then
        echo "ERROR: クリックが効かない（${px},${py}）。押し場所か、上に重なった窓を疑うこと" >&2
        promo_vd_window_clear "$PROMO_APP_PID" || true
        return 1
    fi
    sleep 1
    return 0
}

# 押すだけ（効いたかを読める状態が無いところ用。チャット入力欄など）。
# 呼び出し側が別の手段（OCR 等）で結果を確かめること。キー入力は
# `CGEventPostToPid` で前面化せずに届くので、ここでもフォーカスは即返す
promo_click_front() {
    local px=$1 py=$2 g t front_before
    g=$(promo_win_to_global "$px" "$py") || { echo "ERROR: 窓の矩形が読めない" >&2; return 1; }
    front_before=$(promo_frontmost_pid)
    t=$(promo_rec_elapsed)
    # shellcheck disable=SC2086
    "$PROMO_CLICK" ${g} --hover-ms 420 >/dev/null || { promo_give_back_focus "$front_before"; return 1; }
    promo_give_back_focus "$front_before"
    [ -n "$PROMO_CLICKS_FILE" ] && printf 'click\t%s\t%s\t%s\n' "$t" "$px" "$py" >> "$PROMO_CLICKS_FILE"
    echo "   click ${px},${py} @ ${t}s"
    sleep 1
}

# 短い検査（promo_click_until から呼ぶので数秒で返す）
promo_check_tabs() {
    local want=$1 i got
    for i in 1 2 3 4; do
        got=$(tko list 2>/dev/null | /usr/bin/python3 -c 'import json,sys; print(len(json.load(sys.stdin)["tabs"]))' 2>/dev/null)
        [ "${got:-0}" -eq "$want" ] && return 0
        sleep 0.8
    done
    return 1
}

promo_check_ui_mode() {
    local want=$1 i got
    for i in 1 2 3 4; do
        got=$(tko ui-mode 2>/dev/null | /usr/bin/python3 -c 'import json,sys; print(json.load(sys.stdin)["ui_mode"])' 2>/dev/null)
        [ "$got" = "$want" ] && return 0
        sleep 0.8
    done
    return 1
}

# $1 = ペイン, $2.. = 許す表示種別
promo_check_pane_display() {
    local pane=$1; shift
    local i got want
    for i in 1 2 3 4 5 6; do
        got=$(promo_pane_display "$pane")
        for want in "$@"; do
            [ "$got" = "$want" ] && return 0
        done
        sleep 0.8
    done
    return 1
}

# ボタンの説明中に枠で囲む（絵が動かない区間の「いまどこの話か」を示す）
promo_hi_at() {
    local x=$1 y=$2 w=$3 h=$4 dur=$5 t
    t=$(promo_rec_elapsed)
    [ -n "$PROMO_CLICKS_FILE" ] && printf 'hi\t%s\t%s\t%s\t%s\t%s\t%s\n' "$t" "$x" "$y" "$w" "$h" "$dur" \
        >> "$PROMO_CLICKS_FILE"
}

promo_tab_pane_count() {
    tko list 2>/dev/null | /usr/bin/python3 -c "
import json,sys
try: d=json.load(sys.stdin)
except Exception: print(0); raise SystemExit
for t in d['tabs']:
    if t['id'] == $1: print(len(t['panes'])); raise SystemExit
print(0)"
}

promo_tab_first_pane() {
    tko list 2>/dev/null | /usr/bin/python3 -c "
import json,sys
d=json.load(sys.stdin)
for t in d['tabs']:
    if t['id'] == $1: print(t['panes'][0]['id']); raise SystemExit
raise SystemExit('tab $1 が無い')"
}

promo_pane_display() {
    tko ui-mode 2>/dev/null | /usr/bin/python3 -c "
import json,sys
try: d=json.load(sys.stdin)
except Exception: raise SystemExit
print(d.get('pane_display', {}).get('$1', ''))"
}

# チャット入力欄へ実キー入力し、**画面に入ったことを OCR で確かめてから** Enter を打つ。
# 1 打目が化けることがある（実測）ので、化けていたら 1 度だけ消してやり直す
promo_type_verified() {
    local text=$1 needle=$2 ocr=/private/tmp/tako-promo-ocr shot="$PROMO_WORK/typed.png"
    if [ ! -x "$ocr" ] || [ "$SCRIPT_DIR/ocr-frames.swift" -nt "$ocr" ]; then
        swiftc -O -o "$ocr" "$SCRIPT_DIR/ocr-frames.swift" 2>/dev/null || {
            echo "ERROR: ocr-frames.swift のコンパイルに失敗" >&2; return 1; }
    fi
    local attempt
    for attempt in 1 2; do
        "$PROMO_KEYTYPE" "$PROMO_APP_PID" "$text" --per-char-ms 60 --backspaces 80 >/dev/null || return 1
        sleep 1.5
        rm -f "$shot"
        screencapture -x -o -l"$PROMO_WID" "$shot" 2>/dev/null || true
        if [ -s "$shot" ] && "$ocr" "$shot" 2>/dev/null | grep -q -- "$needle"; then
            "$PROMO_KEYTYPE" "$PROMO_APP_PID" "" --return >/dev/null || return 1
            echo "   入力を確認して送信（試行 ${attempt}）"
            rm -f "$shot"
            return 0
        fi
        echo "   入力欄に「${needle}」が見えない（試行 ${attempt}）"
    done
    echo "ERROR: チャット入力欄へ打てていない（送信しない）" >&2
    return 1
}

# ── 1. 課題: 4 タブに散らばる → 1 タブへ集約 ────────────────────────
scene_scatter() {
    local work=/private/tmp/tako-promo-scatter socket=tako-promo-scat
    local raw="$PROMO_OUT/scenes/scatter-raw.mp4"
    promo_make_demo_env
    explainer_begin scatter "$work" "$socket"
    trap 'promo_stop_isolated '"$socket" EXIT
    local base; base=$(promo_base_pane)
    tko welcome dismiss >/dev/null 2>&1 || true
    type_cmd "$base" "cd $PROMO_DEMO/awesome-app && clear && bash scripts/worker.sh agent-main"
    tko tab rename --tab 1 agent >/dev/null 2>&1 || true
    local p2 p3 p4
    p2=$(tko tab new --title dev-server --cwd "$PROMO_DEMO/awesome-app" | /usr/bin/python3 -c 'import json,sys; print(json.load(sys.stdin)["pane"])')
    type_cmd "$p2" "clear && bash scripts/dev-server.sh"
    p3=$(tko tab new --title worker-api --cwd "$PROMO_DEMO/awesome-app" | /usr/bin/python3 -c 'import json,sys; print(json.load(sys.stdin)["pane"])')
    type_cmd "$p3" "clear && bash scripts/worker.sh api"
    p4=$(tko tab new --title logs --cwd "$PROMO_DEMO/awesome-app" | /usr/bin/python3 -c 'import json,sys; print(json.load(sys.stdin)["pane"])')
    type_cmd "$p4" "clear && bash scripts/logs.sh"
    tko tab select 1 >/dev/null
    sleep 3

    promo_record_start "$raw" 50
    promo_beat tabs
    sleep 5
    promo_beat cycle
    local round t
    for round in 1 2 3; do
        for t in 2 3 4 1; do tko tab select "$t" >/dev/null; sleep 1.3; done
    done
    sleep 1
    promo_beat collect
    # 2x2 に並べる（1 行 4 列だとログが折り返して読めない = 実測）
    tko tab move-pane --pane "$p2" --target "$base" --right >/dev/null; sleep 0.9
    tko tab move-pane --pane "$p3" --target "$base" --down >/dev/null; sleep 0.9
    tko tab move-pane --pane "$p4" --target "$p2" --down >/dev/null; sleep 0.9
    tko equalize --tab 1 >/dev/null 2>&1 || true
    tko tab rename --tab 1 awesome-app >/dev/null 2>&1 || true
    sleep 1
    promo_beat collected
    promo_record_wait
    promo_stop_isolated "$socket"; trap - EXIT
    promo_verify "$raw" "$PROMO_FRAMES/scatter" 1
}

# ── 2. 思想: CLI が GUI を動かす（AI フルコントロール）────────────────
scene_control() {
    local work=/private/tmp/tako-promo-control socket=tako-promo-ctrl
    local raw="$PROMO_OUT/scenes/control-raw.mp4"
    promo_make_demo_env
    explainer_begin control "$work" "$socket"
    trap 'promo_stop_isolated '"$socket" EXIT
    local base right
    base=$(promo_base_pane)
    tko welcome dismiss >/dev/null 2>&1 || true
    type_cmd "$base" "cd $PROMO_DEMO/awesome-app && clear"
    tko tab rename --tab 1 awesome-app >/dev/null 2>&1 || true
    right=$(tko split --pane "$base" --right --cwd "$PROMO_DEMO/awesome-app")
    type_cmd "$right" "clear"
    sleep 2

    promo_record_start "$raw" 60
    sleep 2
    promo_beat split
    type_cmd "$base" "tako split --down -- bash scripts/dev-server.sh"
    sleep 5
    promo_beat open
    type_cmd "$base" "tako open README.md"
    sleep 6
    promo_beat theme_light
    type_cmd "$base" "tako theme light"
    sleep 4
    promo_beat theme_dark
    type_cmd "$base" "tako theme dark"
    sleep 4
    promo_beat gui
    type_cmd "$base" "tako ui-mode gui >/dev/null"
    sleep 0.8
    # 打ち込んでいる側のペインは終端表示のまま残す（空のシェルは全部ボタンになるため）
    tko ui-mode release --pane "$base" >/dev/null 2>&1 || true
    sleep 6
    promo_beat terminal
    type_cmd "$base" "tako ui-mode terminal >/dev/null"
    sleep 0.8
    tko ui-mode restore --pane "$base" >/dev/null 2>&1 || true
    sleep 3
    promo_beat panel
    type_cmd "$base" "tako panel --show --view fleet >/dev/null"
    sleep 6
    promo_record_wait
    promo_stop_isolated "$socket"; trap - EXIT
    promo_verify "$raw" "$PROMO_FRAMES/control" 1
}

# ── 2. 思想: 実 Claude Code が MCP でペインを割る ────────────────────
scene_agent() {
    local work=/private/tmp/tako-promo-agent socket=tako-promo-agent
    local raw="$PROMO_OUT/scenes/agent-raw.mp4"
    promo_make_demo_env
    promo_make_demo_home
    PROMO_EXTRA_ENV=(
        "HOME=$PROMO_DEMO/home"
        "PATH=$PROMO_DEMO/bin:/usr/bin:/bin:/usr/sbin:/sbin"
        "ANTHROPIC_MODEL=$FAST_MODEL"
    )
    explainer_begin agent "$work" "$socket"
    trap 'promo_stop_isolated '"$socket" EXIT
    local base; base=$(promo_base_pane)
    tko welcome dismiss >/dev/null 2>&1 || true
    type_cmd "$base" "cd $PROMO_DEMO/awesome-app && clear"
    tko tab rename --tab 1 awesome-app >/dev/null 2>&1 || true
    # デモ HOME の Claude Code へ tako MCP を登録する（ペインのシェルは HOME=デモ HOME）。
    # 別アカウントの実 config dir で撮るときはそのアカウントの登録をそのまま使う
    if [ -z "$PROMO_CLAUDE_CONFIG_DIR" ]; then
        type_cmd "$base" "tako setup-mcp"
        sleep 8
    fi
    type_cmd "$base" "clear && claude"
    echo "   claude の起動を待機..."
    sleep 20
    promo_wait_pii_clear 120 "$base" || { promo_stop_isolated "$socket"; trap - EXIT; PROMO_EXTRA_ENV=(); return 1; }

    promo_record_start "$raw" 115
    sleep 2
    promo_beat req1
    tko send --pane "$base" --await-prompt \
        "tako の MCP を使って、このリポジトリの dev サーバー（scripts/dev-server.sh）を隣のペインで起動して。起動したら README.md もプレビューで開いて。" \
        >/dev/null 2>&1 || true
    sleep 55
    promo_beat req2
    tko send --pane "$base" --await-prompt \
        "次に、scripts/worker.sh を 'api'、'ui'、'docs' の 3 つの引数でそれぞれ別ペインに分割して起動して。" \
        >/dev/null 2>&1 || true
    sleep 50
    promo_record_wait
    promo_stop_isolated "$socket"; trap - EXIT
    PROMO_EXTRA_ENV=()
    promo_verify "$raw" "$PROMO_FRAMES/agent" 1
}

# ── 3. 導入: brew カード → 導入計画（claude 無し）→ tako setup → 対話 ──
scene_setup() {
    local work=/private/tmp/tako-promo-setup socket=tako-promo-setup
    local raw="$PROMO_OUT/scenes/setup-raw.mp4"
    promo_make_demo_env
    promo_make_demo_home
    # 前半は「Claude Code が入っていない環境」を演出する: デモ PATH から claude を外す
    rm -f "$PROMO_DEMO/bin/claude"
    PROMO_EXTRA_ENV=(
        "HOME=$PROMO_DEMO/home"
        "PATH=$PROMO_DEMO/bin:/usr/bin:/bin:/usr/sbin:/sbin"
        "ANTHROPIC_MODEL=$FAST_MODEL"
    )
    explainer_begin setup "$work" "$socket"
    trap 'promo_stop_isolated '"$socket" EXIT
    local base; base=$(promo_base_pane)
    # 初回起動バナー（tako setup / tako master の案内）はこの章の絵として残す
    type_cmd "$base" "cd $PROMO_DEMO/awesome-app && clear"
    tko tab rename --tab 1 setup >/dev/null 2>&1 || true
    sleep 1.5

    promo_record_start "$raw" "${TAKO_PROMO_SETUP_DUR:-190}"
    sleep 1.5
    promo_beat brew
    tko show-command --pane "$base" --label "Homebrew でインストール" \
        "brew install --cask takushio2525/tako/tako" >/dev/null 2>&1 || true
    sleep 8
    tko show-command --pane "$base" --dismiss >/dev/null 2>&1 || true
    promo_beat bootstrap
    type_cmd "$base" "tako setup bootstrap install --dry-run"
    sleep 13
    # ここから「導入済み」の環境へ（PATH は動的に引かれるので symlink を戻すだけ）
    ln -sf "$(command -v claude)" "$PROMO_DEMO/bin/claude"
    type_cmd "$base" "clear"
    sleep 1
    promo_beat setup
    type_cmd "$base" "tako setup"
    sleep 70
    promo_beat ask
    tko send --pane "$base" "いまの設定を 3 行で教えて。" >/dev/null 2>&1 || true
    sleep 70
    promo_record_wait
    promo_stop_isolated "$socket"; trap - EXIT
    PROMO_EXTRA_ENV=()
    promo_verify "$raw" "$PROMO_FRAMES/setup" 1
}

# ── 4. 基本操作 ────────────────────────────────────────────────────
scene_basics() {
    local work=/private/tmp/tako-promo-basics socket=tako-promo-basic
    local raw="$PROMO_OUT/scenes/basics-raw.mp4"
    promo_make_demo_env
    explainer_begin basics "$work" "$socket"
    trap 'promo_stop_isolated '"$socket" EXIT
    local base; base=$(promo_base_pane)
    tko welcome dismiss >/dev/null 2>&1 || true
    type_cmd "$base" "cd $PROMO_DEMO/awesome-app && clear && ls"
    tko tab rename --tab 1 awesome-app >/dev/null 2>&1 || true
    sleep 2

    promo_record_start "$raw" 95
    sleep 2
    promo_beat split_right
    local p_dev p_wrk
    p_dev=$(tko split --pane "$base" --right --cwd "$PROMO_DEMO/awesome-app" -- bash scripts/dev-server.sh)
    sleep 4
    promo_beat split_down
    p_wrk=$(tko split --pane "$base" --down --cwd "$PROMO_DEMO/awesome-app" -- bash scripts/worker.sh tests)
    sleep 4
    promo_beat tab
    mkdir -p "$PROMO_DEMO/docs-site"
    tko tab new --title docs-site --cwd "$PROMO_DEMO/docs-site" --focus >/dev/null
    sleep 3
    tko tab select 1 >/dev/null
    sleep 2
    promo_beat tree
    tko panel --filetree on --sidebar-width 190 >/dev/null
    sleep 5
    # プレビューを広く見せるため、分割の実演に使った 2 ペインは片付ける
    tko close --pane "$p_wrk" >/dev/null 2>&1 || true
    tko close --pane "$p_dev" >/dev/null 2>&1 || true
    sleep 1
    promo_beat md
    tko open --pane "$base" "$PROMO_DEMO/awesome-app/README.md" >/dev/null
    sleep 1
    # プレビューペイン（= タブ 1 で base 以外）の取り分を広げる
    local p_prev
    p_prev=$(tko list | /usr/bin/python3 -c 'import json,sys
d=json.load(sys.stdin); b=int(sys.argv[1])
print(next((p["id"] for p in d["tabs"][0]["panes"] if p["id"]!=b), ""))' "$base")
    [ -n "$p_prev" ] && tko resize --pane "$p_prev" --share-x 0.64 >/dev/null 2>&1 || true
    sleep 5
    promo_beat reload
    cat >> "$PROMO_DEMO/awesome-app/README.md" <<'ADD'

## Live reload

Edit any file and the preview updates instantly.
ADD
    sleep 6
    promo_beat pdf
    tko open --pane "$base" "$PROMO_DEMO/awesome-app/docs/spec.pdf" >/dev/null
    sleep 7
    promo_beat image
    tko open --pane "$base" "$PROMO_DEMO/awesome-app/docs/logo.png" >/dev/null
    sleep 5
    promo_beat code
    tko open --pane "$base" "$PROMO_DEMO/awesome-app/src/app.py" >/dev/null
    sleep 6
    promo_beat run
    tko run --pane "$base" "$PROMO_DEMO/awesome-app/scripts/build.sh" >/dev/null
    sleep 10
    promo_record_wait
    promo_stop_isolated "$socket"; trap - EXIT
    promo_verify "$raw" "$PROMO_FRAMES/basics" 1
}

# ── 5. AI に任せる: tako master → worker → 俯瞰 → かんたん表示 → 報告 ──
scene_master() {
    local work=/private/tmp/tako-promo-master socket=tako-promo-mast
    local raw="$PROMO_OUT/scenes/master-raw.mp4"
    mkdir -p "$PROMO_DEMO/docs-site"
    promo_make_demo_env
    promo_make_demo_home
    PROMO_EXTRA_ENV=(
        "HOME=$PROMO_DEMO/home"
        "PATH=$PROMO_DEMO/bin:/usr/bin:/bin:/usr/sbin:/sbin"
        "ANTHROPIC_MODEL=$FAST_MODEL"
        # worker への指示は従来のキー操作経路（#32）で届ける。第 1 層（#790 の Cross-Session
        # Messaging）だと worker の画面に「別セッションからの指示として扱え」の定型文が
        # 冒頭に大きく出て、視聴者には無関係な注意書きが worker ペインを埋める
        "TAKO_PEER_MESSAGING=off"
    )
    # チャット表示（かんたん表示）の判定は agents 走査 → tmux ペインの pid 対応付けに乗るので
    # **器（tmux バックエンド）が要る**。persist=0 だと claude ペインが永久に terminal のまま
    # （隔離で実測: persist=0 は 40 秒待っても terminal / persist=1 は 5 秒で chat）
    explainer_begin master "$work" "$socket" 1
    trap 'promo_stop_isolated '"$socket" EXIT
    local base; base=$(promo_base_pane)
    tko welcome dismiss >/dev/null 2>&1 || true
    # master が spawn 先に使うプロジェクトと、収録用の軽い worker モデル（隔離 data_dir へ書く）
    tko orchestrator projects add --key awesome-app \
        --cwd "$PROMO_DEMO/awesome-app" --description "デモ用の Web アプリ" >/dev/null 2>&1 || true
    # 既定プロファイルは effort=max で master が最初の spawn まで 40 秒以上考える（実測）ので、
    # 収録では medium に落とす（挙動そのものは変わらない）
    tko orchestrator profiles set default --worker-model haiku \
        --worker-model-policy fixed --effort medium --worker-effort medium >/dev/null 2>&1 || true
    # #1132: worker 1 枚に 60 桁を保証する既定のままだと、960x540pt / フォント 15 の窓では
    # 割った worker が 59 桁になるので **spawn が同じタブへ割らず別タブへ出る**（実測）。
    # この章は「隣のペインに増える」絵が要件なので下限を下げる
    tko orchestrator layout --min-worker-cols 40 >/dev/null 2>&1 || true
    # **ローカルルールで spawn に寄せる**（2026-09-09 実測）。素の依頼だと master は
    # `tako_orchestrator_run`（ペインを作らない headless 実行）で 1 体ずつ回すので、
    # 「担当 AI のペインが隣に生える」絵が一度も撮れない（#1081 の master 章と同じ罠）。
    # 画面に出る依頼文はユーザーが実際に打つ自然な日本語のままにしたいので、
    # steering は profile の `prompt_blocks.append`（= ユーザーが置けるローカルルール）へ置く。
    # 見せている機能（spawn がペインを作る）は本物で、選び方だけを固定している
    cat >> "$work/data/orchestrator/profiles/default.yaml" <<'PROFILE_YAML'
prompt_blocks:
  append: |
    ## この環境のローカルルール（他の手順より優先）
    - **`tako_orchestrator_run` は使用禁止**。sync / 非同期のどちらでも呼ばない。
      この環境は「担当 AI のペインが画面に生えて、そこで作業が進む」ことを見せるためのもので、
      ペインを残さない実行は要件を満たさない。
    - ワーカーは必ず `tako_orchestrator_spawn` で立てる。1 つの依頼につき 1 体だけ。
    - ワーカーのペインは作業が終わっても閉じない（auto_close は使わない）。
    - 「確認は不要」と言われても、**spawn でペインを立てる**点は変えない。
PROFILE_YAML
    type_cmd "$base" "cd $PROMO_DEMO/awesome-app && clear"
    tko tab rename --tab 1 awesome-app >/dev/null 2>&1 || true
    # デモ HOME の claude へ tako MCP を登録する。別アカウントの実 config dir で撮るときは
    # そのアカウントの登録をそのまま使う（ユーザーの .claude.json を書き換えない）
    if [ -z "$PROMO_CLAUDE_CONFIG_DIR" ]; then
        type_cmd "$base" "tako setup-mcp"
        sleep 8
    fi
    type_cmd "$base" "clear && tako master"
    echo "   master の起動を待機..."
    sleep 30
    promo_wait_pii_clear 120 "$base" || { promo_stop_isolated "$socket"; trap - EXIT; PROMO_EXTRA_ENV=(); return 1; }

    promo_record_start "$raw" "${TAKO_PROMO_MASTER_DUR:-420}"
    sleep 14
    promo_beat request
    # --await-prompt は送達検証で Enter を撃ち直すため、生成中の master を中断させることがある。
    # ここは 1 回だけ素直に送る（#470 v3 の教訓）。
    # 「同時に」「run ではなく spawn」「ペインは閉じない」は絵のための指定: 2026-09-06 の実測では
    # master（sonnet・medium）が `tako_orchestrator_run` を 1 体ずつ直列に回し、各 worker を完了直後に
    # auto_close したので、3 体が並ぶ絵・orch ビュー・かんたん表示が一度も撮れなかった
    tko send --pane "$base" \
        "worker を 3 体、tako_orchestrator_spawn で同時に立てて（tako_orchestrator_run は使わない）。project は awesome-app。担当は 'api' / 'ui' / 'docs' で、各 worker へのプロンプトは「bash scripts/task.sh <担当名> を実行して、出力の最終行を報告して」でよい。確認は不要、すぐ spawn して。worker のペインは閉じないでそのまま残して。3 体の報告が揃ったら結果を 3 行でまとめて。" \
        >/dev/null 2>&1 || true
    # worker ペインが出そろうまで待つ（最大 240s。spawn は 1 体 40 秒前後 = 実測）
    local i n=0
    for i in $(seq 1 48); do
        n=$(tko list 2>/dev/null | /usr/bin/python3 -c \
            'import json,sys
try: d=json.load(sys.stdin)
except Exception: print(0); raise SystemExit
print(sum(len(t["panes"]) for t in d["tabs"]))' 2>/dev/null || echo 0)
        [ "${n:-0}" -ge 4 ] && break
        sleep 5
    done
    echo "   ペイン数: $n"
    sleep 6
    promo_beat workers_up
    sleep 16
    promo_beat orch
    tko panel --show --view orch >/dev/null 2>&1 || true
    sleep 16
    promo_beat gui
    tko ui-mode gui >/dev/null 2>&1 || true
    # claude ペインがチャット表示へ切り替わるまで待つ（判定は agents 走査に乗るので数秒〜30 秒）
    for i in $(seq 1 8); do
        sleep 4
        tko ui-mode 2>/dev/null | grep -q '"chat"' && break
    done
    echo "   pane_display: $(tko ui-mode 2>/dev/null | /usr/bin/python3 -c 'import json,sys; print(json.load(sys.stdin).get("pane_display"))' 2>/dev/null)"
    sleep 16
    tko ui-mode terminal >/dev/null 2>&1 || true
    tko panel --hide >/dev/null 2>&1 || true
    sleep 2
    promo_beat report
    # master の検収・報告（idle へ戻る）を待つ。残り尺を超えない範囲で
    for i in $(seq 1 30); do
        tko orchestrator status --pane "$base" 2>/dev/null | grep -q '"status": *"idle"' && break
        sleep 5
    done
    promo_beat report_done
    promo_record_wait
    promo_stop_isolated "$socket"; trap - EXIT
    PROMO_EXTRA_ENV=()
    promo_verify "$raw" "$PROMO_FRAMES/master" 1
}

# ── 6. 再起動しても戻る（persist ON。前半 / 後半で別クリップ）────────
scene_restore() {
    local work=/private/tmp/tako-promo-restore socket=tako-promo-rest
    promo_make_demo_env
    explainer_begin restore "$work" "$socket" 1
    trap 'promo_stop_isolated '"$socket" EXIT
    local base; base=$(promo_base_pane)
    tko welcome dismiss >/dev/null 2>&1 || true
    type_cmd "$base" "cd $PROMO_DEMO/awesome-app && clear"
    tko tab rename --tab 1 awesome-app >/dev/null 2>&1 || true
    sleep 1
    tko split --pane "$base" --down --cwd "$PROMO_DEMO/awesome-app" -- bash scripts/dev-server.sh >/dev/null
    sleep 1
    tko split --pane "$base" --right --cwd "$PROMO_DEMO/awesome-app" -- bash scripts/worker.sh api >/dev/null
    sleep 1
    tko open --pane "$base" "$PROMO_DEMO/awesome-app/README.md" >/dev/null
    sleep 2
    tko equalize --tab 1 >/dev/null 2>&1 || true
    sleep 3
    PROMO_BEATS_FILE="$PROMO_OUT/scenes/restore-before-beats.tsv"; : > "$PROMO_BEATS_FILE"
    promo_record_start "$PROMO_OUT/scenes/restore-before-raw.mp4" 12
    promo_record_wait
    echo "   終了 → 再起動（復元を待つ）"
    # 器（tmux）は生かしたままアプリだけ終了する = 実運用の「tako を閉じる」と同じ
    promo_stop_isolated_keep_sessions
    sleep 3
    promo_start_isolated "$work" "$socket" 1
    tko theme --size "$FONT_SIZE" >/dev/null 2>&1 || true
    sleep 6
    PROMO_BEATS_FILE="$PROMO_OUT/scenes/restore-after-beats.tsv"; : > "$PROMO_BEATS_FILE"
    promo_record_start "$PROMO_OUT/scenes/restore-after-raw.mp4" 16
    promo_record_wait
    promo_stop_isolated "$socket"; trap - EXIT
    promo_verify "$PROMO_OUT/scenes/restore-before-raw.mp4" "$PROMO_FRAMES/restore-before" 1
    promo_verify "$PROMO_OUT/scenes/restore-after-raw.mp4" "$PROMO_FRAMES/restore-after" 1
}

# ── 7. スマホから: Remote Control のプロファイル opt-in（CLI 経路）──────
scene_remote() {
    local work=/private/tmp/tako-promo-remote socket=tako-promo-remo
    local raw="$PROMO_OUT/scenes/remote-raw.mp4"
    promo_make_demo_env
    explainer_begin remote "$work" "$socket"
    trap 'promo_stop_isolated '"$socket" EXIT
    local base; base=$(promo_base_pane)
    tko welcome dismiss >/dev/null 2>&1 || true
    type_cmd "$base" "cd $PROMO_DEMO/awesome-app && clear"
    tko tab rename --tab 1 awesome-app >/dev/null 2>&1 || true
    sleep 2
    promo_record_start "$raw" 40
    sleep 1.5
    promo_beat profile
    type_cmd "$base" "tako orchestrator profiles set default --remote-control true"
    sleep 6
    type_cmd "$base" "tako orchestrator profiles show default"
    sleep 12
    promo_record_wait
    promo_stop_isolated "$socket"; trap - EXIT
    promo_verify "$raw" "$PROMO_FRAMES/remote" 1
}

# ── 8. Windows と OSS: 対応状況ページ / LICENSE / README / brew カード ──
scene_windows() {
    local work=/private/tmp/tako-promo-windows socket=tako-promo-win
    local raw="$PROMO_OUT/scenes/windows-raw.mp4"
    promo_make_demo_env
    explainer_begin windows "$work" "$socket"
    trap 'promo_stop_isolated '"$socket" EXIT
    local base; base=$(promo_base_pane)
    tko welcome dismiss >/dev/null 2>&1 || true
    type_cmd "$base" "cd $PROMO_DEMO/tako-docs && clear && ls"
    tko tab rename --tab 1 tako >/dev/null 2>&1 || true
    sleep 2
    promo_record_start "$raw" 60
    sleep 1.5
    promo_beat winsupport
    tko open --pane "$base" "$PROMO_DEMO/tako-docs/windows-support.md" >/dev/null
    sleep 12
    promo_beat license
    tko open --pane "$base" "$PROMO_DEMO/tako-docs/LICENSE" >/dev/null
    sleep 8
    promo_beat readme
    tko open --pane "$base" "$PROMO_DEMO/tako-docs/README.md" >/dev/null
    sleep 6
    promo_beat brew
    tko show-command --pane "$base" --label "Homebrew でインストール" \
        "brew install --cask takushio2525/tako/tako" >/dev/null 2>&1 || true
    sleep 12
    promo_record_wait
    promo_stop_isolated "$socket"; trap - EXIT
    promo_verify "$raw" "$PROMO_FRAMES/windows" 1
}

# ── 5. かんたん表示（GUI モード）の手順デモ ──────────────────────────
# ユーザー評価（2026-09-08）: 「簡単 GUI 表示モードの解説がすごく分かりづらい。
# しっかりと、その章で新しいタブを立てて、GUI モードで新しいペインを出すとどんなボタンが
# 出て、マスター起動ボタンを押すとどんな UI で…そんな感じでしっかりやってほしい」。
# = 1 ナレーション 1 操作の手順型。**ペインは最大 2 枚**（master + worker 1 体）に抑える。
#
# 操作はすべて**本物のクリック / キー入力**（click.swift / keytype.swift）で行う。
# CLI で同じことをすると絵が変わってしまうため:
#   - カード押下は `begin_pane_settle`（#720）を張るので「準備中… / AI を起動しています」が
#     出るが、シェルへ `tako master` を送るだけだと claude の起動ログが素通しで見える
#   - チャット入力欄に文字が入っていく絵は `tako send`（claude の TUI へ送る経路）では出ない
# 押した座標と時刻は `<scene>-clicks.tsv` に残し、annotate-clicks.sh がポインタと波紋を
# 焼き込む（ウインドウ単体キャプチャはカーソルを写さない）。
scene_guimode() {
    local work=/private/tmp/tako-promo-guimode socket=tako-promo-gui
    local plain="$PROMO_OUT/scenes/guimode-plain.mp4"
    local raw="$PROMO_OUT/scenes/guimode-raw.mp4"
    local clicks="$PROMO_OUT/scenes/guimode-clicks.tsv"
    promo_make_demo_env
    promo_make_demo_home
    PROMO_EXTRA_ENV=(
        "HOME=$PROMO_DEMO/home"
        "PATH=$PROMO_DEMO/bin:/usr/bin:/bin:/usr/sbin:/sbin"
        "ANTHROPIC_MODEL=$FAST_MODEL"
        # worker ペインの冒頭に「別セッションからの指示として扱え」の定型文を出さない（#790）
        "TAKO_PEER_MESSAGING=off"
    )
    # かんたん表示のチャット判定は器（tmux バックエンド）が要る = persist=1
    explainer_begin guimode "$work" "$socket" 1
    trap 'promo_stop_isolated '"$socket" EXIT
    # #1149 以降は layout.json の seed が効かないので、AX で 16:9 の置き場所へ直す。
    # **この章で押す 4 点**を宣言しておくと、他の worker の窓に覆われない場所を選ぶ
    # （覆われているとクリックが相手へ吸われる。lib.sh の注記）
    PROMO_CLICK_POINTS="${GUI_PLUS_X},${GUI_PLUS_Y} ${GUI_TOGGLE_X},${GUI_TOGGLE_Y} 960,424 400,915"
    promo_force_window_frame "$PROMO_APP_PID" 960 540 || {
        promo_stop_isolated "$socket"; trap - EXIT; PROMO_EXTRA_ENV=(); return 1; }
    local base; base=$(promo_base_pane)
    tko welcome dismiss >/dev/null 2>&1 || true
    # 収録用アカウント（`TAKO_PROMO_CLAUDE_CONFIG_DIR`）を使うならチャット判定のために
    # 登録が要る（lib.sh の注記）。ただし**この章は既定アカウントで撮る**:
    # 2026-09-09 に univ アカウントで撮ったら、そのアカウントの PreToolUse フック
    # （tako 開発ディレクトリ以外では tako ツールを拒否する）が spawn を止め、
    # 「このアカウントは…専用に制限されており」という拒否文が master のチャットに
    # 写り込んだ（worker が 1 体も立たない = 章が成立しない）
    promo_register_recording_account || {
        promo_stop_isolated "$socket"; trap - EXIT; PROMO_EXTRA_ENV=(); return 1; }
    tko orchestrator projects add --key awesome-app \
        --cwd "$PROMO_DEMO/awesome-app" --description "デモ用の Web アプリ" >/dev/null 2>&1 || true
    tko orchestrator profiles set default --worker-model haiku \
        --worker-model-policy fixed --effort medium --worker-effort medium >/dev/null 2>&1 || true
    type_cmd "$base" "cd $PROMO_DEMO/awesome-app && clear"
    sleep 2
    # デモ HOME の claude へ tako の MCP を登録する（登録先は `$HOME/.claude.json` =
    # ユーザーグローバルなので、master の cwd がどこでも tako ツールが使える）。
    # **外部 config dir（`TAKO_PROMO_CLAUDE_CONFIG_DIR`）では収録できない**（下の注記）
    if [ -z "$PROMO_CLAUDE_CONFIG_DIR" ]; then
        type_cmd "$base" "tako setup-mcp"
        sleep 10
        type_cmd "$base" "clear"
        sleep 2
    fi
    # タブ名は自動リネームだと cwd 由来（ホームだとユーザー名が出る = PII）なので固定する
    tko tab rename --tab 1 awesome-app >/dev/null 2>&1 || true
    sleep 3

    : > "$clicks"
    PROMO_CLICKS_FILE=$clicks
    promo_record_start "$plain" "${TAKO_PROMO_GUI_DUR:-360}"

    # ① 新しいタブを作る（「+」を押す）
    sleep 8
    promo_beat newtab
    promo_click_until "$GUI_PLUS_X" "$GUI_PLUS_Y" promo_check_tabs 2 || return 1
    local GUI_NEW_PANE; GUI_NEW_PANE=$(promo_tab_first_pane 2)
    echo "   新しいタブのペイン: $GUI_NEW_PANE"

    # ② かんたん表示へ切り替える（タブバー右上のトグル）
    sleep 15
    promo_beat togui
    promo_click_until "$GUI_TOGGLE_X" "$GUI_TOGGLE_Y" promo_check_ui_mode gui || return 1

    # ③ 3 枚のボタンが並んだ状態（全体）
    sleep 14
    promo_beat cards
    # ④〜⑥ ボタンを 1 枚ずつ枠で囲んで説明する。
    # **枠の秒数と間合いは実測したナレーション秒（narr/durations.tsv）から決めてある**
    # （区間の尺 = max(min_dur, ナレーション秒 + 0.8)。GUI 章は 9.2〜17.5 秒）。
    # 短いと説明の途中で枠が消え、間合いが足りないと次の操作が区間へ写り込む
    sleep 15
    promo_beat card1
    promo_hi_at 501 352 919 141 16
    sleep 17
    promo_beat card2
    promo_hi_at 501 509 919 142 16
    sleep 17
    promo_beat card3
    promo_hi_at 501 666 919 142 14
    sleep 8
    promo_hi_at 495 828 640 104 6

    # ⑦ 「AI チームに任せる」を押す → 準備中… → チャット
    sleep 11
    promo_beat press
    promo_click_until 960 424 promo_check_pane_display "$GUI_NEW_PANE" preparing chat || return 1
    # チャット確定まで待って**その瞬間**をビートにする（区間 c5_gui8 の in 点）。
    # agents 走査の鮮度窓が 30 秒（#1011）あるので最大 60 秒みる
    local i disp
    for i in $(seq 1 20); do
        disp=$(promo_pane_display "$GUI_NEW_PANE")
        [ "$disp" = chat ] && break
        sleep 3
    done
    echo "   pane_display（押下後）: ${disp:-不明}"
    if [ "${disp:-}" != chat ]; then
        echo "ERROR: 押下後にチャット表示にならない（${disp:-不明}）。この章はチャット画面が本題なので中止する" >&2
        echo "-- ペインの末尾（stray 文字が混ざっていないか / claude が起動したか）" >&2
        tko read --pane "$GUI_NEW_PANE" 2>&1 | tail -6 >&2
        return 1
    fi

    # ⑧ チャット画面の各部（ヘッダ → 入力欄の順に囲む）。この区間は 17.5 秒（最長）なので
    # 次の操作までの間合いを 20 秒とる = 説明中に入力の絵が写り込まない
    promo_beat chat
    promo_hi_at 8 96 1904 52 8
    sleep 9
    promo_hi_at 25 880 1870 120 10

    # ⑨ 日本語で頼む（チャット入力欄へ実キー入力 → Enter）
    sleep 11
    promo_beat ask
    # 入力欄は「押せたか」を読める状態が無いので、**打てたことを OCR で確かめる**のを
    # クリックの検査に兼ねる（1 回目が食われたら 2 周目で入る）
    local typed=0 round
    for round in 1 2; do
        promo_click_front 400 915 || return 1
        if promo_type_verified "awesome-app のテストを直してほしい。担当の AI を 1 体立てて任せて。確認は不要" \
            "テストを直して"; then typed=1; break; fi
    done
    [ "$typed" = 1 ] || { echo "ERROR: チャット入力欄へ打てない" >&2; return 1; }

    # ⑩ worker が隣のペインに立つ（最大 100 秒待つ。spawn は 40 秒前後 = 実測）
    local n=0
    for i in $(seq 1 25); do
        n=$(promo_tab_pane_count 2)
        [ "${n:-0}" -ge 2 ] && break
        sleep 4
    done
    echo "   タブ 2 のペイン数: $n"
    promo_beat worker
    sleep 30

    # ⑪ 同じボタンでターミナル表示へ戻す
    promo_beat back
    promo_click_until "$GUI_TOGGLE_X" "$GUI_TOGGLE_Y" promo_check_ui_mode terminal || return 1
    sleep 12

    promo_record_wait
    promo_stop_isolated "$socket"; trap - EXIT
    PROMO_EXTRA_ENV=()
    PROMO_CLICKS_FILE=""
    bash "$SCRIPT_DIR/annotate-clicks.sh" "$plain" "$clicks" "$raw"
    promo_verify "$raw" "$PROMO_FRAMES/guimode" 1
}

case "$SCENE" in
    scatter) scene_scatter ;;
    control) scene_control ;;
    agent)   scene_agent ;;
    setup)   scene_setup ;;
    basics)  scene_basics ;;
    master)  scene_master ;;
    guimode) scene_guimode ;;
    restore) scene_restore ;;
    remote)  scene_remote ;;
    windows) scene_windows ;;
    all)     scene_scatter; scene_control; scene_basics; scene_restore; scene_remote; scene_windows; scene_agent; scene_setup; scene_master; scene_guimode ;;
    *) echo "unknown scene: $SCENE" >&2; exit 2 ;;
esac
echo "== done: $SCENE"
