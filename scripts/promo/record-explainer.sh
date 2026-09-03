#!/bin/bash
# tako:run: bash scripts/promo/record-explainer.sh all
# tako 解説動画（#1081）のシーン収録。
#
# 使い方:
#   scripts/promo/record-explainer.sh <scene>
#     scene = scatter | control | agent | setup | basics | master | restore | remote | windows | all
#
#   scatter … 1. 課題（4 つのタブに散らばる → 1 タブへ集約）
#   control … 2. 思想（CLI が GUI を動かす = AI フルコントロール）
#   agent   … 2. 思想（実 Claude Code が MCP でペインを割る。#470 の agent 相当）
#   setup   … 3. 導入（brew カード → bootstrap の導入計画 → tako setup → 対話アシスタント）
#   basics  … 4. 基本操作（分割 / タブ / ツリー / md ライブリロード / PDF / 画像 / コード / Code Runner）
#   master  … 5. AI に任せる（tako master → worker 3 体 → orch ビュー → かんたん表示 → 報告）
#   restore … 6. 再起動しても戻る（終了直前 / 再起動後の 2 クリップ）
#   remote  … 7. スマホから（Remote Control のプロファイル opt-in。PWA は record-pwa.cjs）
#   windows … 8. Windows と OSS（対応状況ページ / LICENSE / README / brew カード）
#
# 各シーンは lib.sh の隔離インスタンス（TAKO_ISOLATED=1 + 明示ソケット + デモ HOME）で撮る。
# 収録中に CLI 操作をした瞬間を promo_beat で <scene>-beats.tsv に残し、
# build-explainer.sh がそれを in 点にしてテロップ・ナレーションを合わせる。
# 出力: ~/Desktop/tako-promo/scenes/<scene>-raw.mp4 + <scene>-beats.tsv
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
# shellcheck source=lib.sh
source "$SCRIPT_DIR/lib.sh"

SCENE=${1:-}
[ -n "$SCENE" ] || { echo "usage: $0 <scatter|control|agent|setup|basics|master|restore|remote|windows|all>" >&2; exit 2; }
promo_require

FAST_MODEL=${TAKO_PROMO_MODEL:-claude-sonnet-5}   # 収録のテンポ用（挙動は変わらない）
FONT_SIZE=${TAKO_PROMO_FONT_SIZE:-15}

# シーン共通の前処理: 作業ディレクトリ・デモ環境・16:9 ウインドウ・隔離起動
explainer_begin() {
    local scene=$1 work=$2 socket=$3 persist=${4:-0}
    echo "== scene $scene"
    promo_wait_capturable "${TAKO_PROMO_WAIT_UNLOCK:-0}" || promo_check_capturable
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
    # デモ HOME の Claude Code へ tako MCP を登録する（ペインのシェルは HOME=デモ HOME）
    type_cmd "$base" "tako setup-mcp"
    sleep 8
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
    )
    explainer_begin master "$work" "$socket"
    trap 'promo_stop_isolated '"$socket" EXIT
    local base; base=$(promo_base_pane)
    tko welcome dismiss >/dev/null 2>&1 || true
    # master が spawn 先に使うプロジェクトと、収録用の軽い worker モデル（隔離 data_dir へ書く）
    tko orchestrator projects add --key awesome-app \
        --cwd "$PROMO_DEMO/awesome-app" --description "デモ用の Web アプリ" >/dev/null 2>&1 || true
    tko orchestrator profiles set default --worker-model haiku \
        --worker-model-policy fixed >/dev/null 2>&1 || true
    type_cmd "$base" "cd $PROMO_DEMO/awesome-app && clear"
    tko tab rename --tab 1 awesome-app >/dev/null 2>&1 || true
    type_cmd "$base" "tako setup-mcp"
    sleep 8
    type_cmd "$base" "clear && tako master"
    echo "   master の起動を待機..."
    sleep 30
    promo_wait_pii_clear 120 "$base" || { promo_stop_isolated "$socket"; trap - EXIT; PROMO_EXTRA_ENV=(); return 1; }

    promo_record_start "$raw" "${TAKO_PROMO_MASTER_DUR:-260}"
    sleep 14
    promo_beat request
    # --await-prompt は送達検証で Enter を撃ち直すため、生成中の master を中断させることがある。
    # ここは 1 回だけ素直に送る（#470 v3 の教訓）
    tko send --pane "$base" \
        "worker を 3 体 spawn して。project は awesome-app。それぞれ 'api' / 'ui' / 'docs' を担当し、プロンプトは「bash scripts/task.sh <担当名> を実行して、出力の最終行を報告して」でよい。確認は不要、すぐ spawn して。3 体の報告が揃ったら結果を 3 行でまとめて。" \
        >/dev/null 2>&1 || true
    # worker ペインが出そろうまで待つ（最大 150s）
    local i n=0
    for i in $(seq 1 30); do
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
    sleep 18
    tko ui-mode terminal >/dev/null 2>&1 || true
    tko panel --hide >/dev/null 2>&1 || true
    sleep 2
    promo_beat report
    # master の検収・報告を待つ（残り尺いっぱい）
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
    promo_stop_isolated "$socket"
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

case "$SCENE" in
    scatter) scene_scatter ;;
    control) scene_control ;;
    agent)   scene_agent ;;
    setup)   scene_setup ;;
    basics)  scene_basics ;;
    master)  scene_master ;;
    restore) scene_restore ;;
    remote)  scene_remote ;;
    windows) scene_windows ;;
    all)     scene_scatter; scene_control; scene_basics; scene_restore; scene_remote; scene_windows; scene_agent; scene_setup; scene_master ;;
    *) echo "unknown scene: $SCENE" >&2; exit 2 ;;
esac
echo "== done: $SCENE"
