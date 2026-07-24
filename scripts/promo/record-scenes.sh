#!/bin/bash
# tako 紹介動画 シーン収録スクリプト (#470 Phase B)
#
# 使い方:
#   scripts/promo/record-scenes.sh <scene>
#     scene = agent | preview | setup | master | restore | outro | all
#
#   agent   … S1/S2 素材（実 Claude Code がペインを割って dev サーバー・プレビューを開く）
#   preview … S3 素材（Markdown ライブリロード + Code Runner）
#   setup   … S4 素材（tako setup --check / setup-mcp / claude mcp list の導入体験）
#   master  … S5 素材（tako master → worker が同じタブに並ぶオーケストレーション）
#   restore … 補足素材（再起動して全ペインが復元される）
#   outro   … S6 素材（テーマ切替 + パレット）
#
# 台本上の並び（.agent/plans/2026-07-promo-video.md）は
#   画面操作 → プレビュー → setup → master → クロージング
#
# 出力: ~/Desktop/tako-promo/scenes/<scene>-raw.mp4（編集前の素材。尺は台本より長め）
#       ~/Desktop/tako-promo/frames/<scene>/ にフレーム抽出（PII 全数チェック用）
#
# 収録は隔離 GUI インスタンス（TAKO_ISOLATED=1 + 明示ソケット）で行い本番に触れない。
# 画面には /private/tmp 配下のデモプロジェクトしか写らないようにしてある（lib.sh 参照）。
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
# shellcheck source=lib.sh
source "$SCRIPT_DIR/lib.sh"

SCENE=${1:-}
[ -n "$SCENE" ] || { echo "usage: $0 <agent|preview|restore|outro|all>" >&2; exit 2; }

promo_require
# 収録可否（ロック・権限）は各シーンの冒頭で個別に確認する

# ── S4: プレビュー（Markdown ライブリロード + Code Runner）──────────
scene_preview() {
    local work=/private/tmp/tako-promo-preview socket=tako-promo-prev
    local raw="$PROMO_OUT/scenes/preview-raw.mp4"
    echo "== scene preview"
    promo_wait_capturable "${TAKO_PROMO_WAIT_UNLOCK:-0}" || promo_check_capturable
    rm -rf "$work"
    promo_make_demo_env
    promo_start_isolated "$work" "$socket"
    trap 'promo_stop_isolated '"$socket" EXIT

    local base; base=$(promo_base_pane)
    tko send --pane "$base" "cd $PROMO_DEMO/awesome-app && clear" >/dev/null
    tko tab rename --tab 1 awesome-app >/dev/null 2>&1 || true
    sleep 1.5

    promo_record_start "$raw" 26

    # beat 1: README を Markdown プレビューで開く
    tko open --pane "$base" "$PROMO_DEMO/awesome-app/README.md" >/dev/null
    sleep 4
    # beat 2: 外部からファイルを書き換え → ライブリロードで即反映
    cat >> "$PROMO_DEMO/awesome-app/README.md" <<'ADD'

## Live reload

Edit any file and the preview updates instantly.
ADD
    sleep 4
    # beat 3: 実行可能スクリプトをコードプレビューで開く（再生ボタンが出る）
    tko open --pane "$base" "$PROMO_DEMO/awesome-app/scripts/build.sh" >/dev/null
    sleep 4
    # beat 4: Code Runner で実行 → 新ペインにビルドログ（基準ペインを明示する）
    tko run --pane "$base" "$PROMO_DEMO/awesome-app/scripts/build.sh" >/dev/null
    sleep 8

    promo_record_wait
    promo_stop_isolated "$socket"; trap - EXIT
    promo_verify "$raw" "$PROMO_FRAMES/preview" 1
}

# ── S1/S2/S3: 実 Claude Code + オーケストレーション ─────────────────
scene_agent() {
    local work=/private/tmp/tako-promo-agent socket=tako-promo-agent
    local raw="$PROMO_OUT/scenes/agent-raw.mp4"
    echo "== scene agent"
    promo_wait_capturable "${TAKO_PROMO_WAIT_UNLOCK:-0}" || promo_check_capturable
    rm -rf "$work"
    promo_make_demo_env
    promo_start_isolated "$work" "$socket"
    trap 'promo_stop_isolated '"$socket" EXIT

    local base; base=$(promo_base_pane)
    tko send --pane "$base" "cd $PROMO_DEMO/awesome-app && clear" >/dev/null
    tko tab rename --tab 1 awesome-app >/dev/null 2>&1 || true
    sleep 1

    # 実 Claude Code を起動しておく（起動待ちは収録前に済ませる）
    tko send --pane "$base" "claude" >/dev/null
    echo "   claude の起動を待機..."
    sleep 20

    promo_record_start "$raw" 100

    # beat 1: 日本語で依頼 → tako MCP でペインが分割され dev サーバーが起動する
    tko send --pane "$base" --await-prompt \
        "tako の MCP を使って、このリポジトリの dev サーバー（scripts/dev-server.sh）を隣のペインで起動して。起動したら README.md もプレビューで開いて。" \
        >/dev/null 2>&1 || true
    sleep 55
    # beat 2: worker を並べる（オーケストレーション）
    tko send --pane "$base" --await-prompt \
        "次に、scripts/worker.sh を 'api'、'ui'、'docs' の 3 つの引数でそれぞれ別ペインに分割して起動して。" \
        >/dev/null 2>&1 || true
    sleep 40

    promo_record_wait
    promo_stop_isolated "$socket"; trap - EXIT
    promo_verify "$raw" "$PROMO_FRAMES/agent" 1
}

# ── S3: setup（設定ゼロ導入）────────────────────────────────────────
# 撮る事実（すべて実挙動。誇張しない）:
#   1. tako setup --check … 足りない設定を tako 自身が列挙する
#   2. tako setup-mcp     … Claude Code のユーザー設定へ tako MCP を 1 コマンド登録
#                           （内部で claude mcp add --scope user）→ 以後どのプロジェクトでも有効
#   3. claude mcp list    … Claude Code 側から tako が Connected として見える
# HOME / PATH はデモ用（lib.sh の promo_make_demo_home）に差し替えて撮るので、
# 画面に出るパスは /private/tmp 配下だけになる
scene_setup() {
    local work=/private/tmp/tako-promo-setup socket=tako-promo-setup
    local raw="$PROMO_OUT/scenes/setup-raw.mp4"
    echo "== scene setup"
    promo_wait_capturable "${TAKO_PROMO_WAIT_UNLOCK:-0}" || promo_check_capturable
    rm -rf "$work"
    promo_make_demo_env
    promo_make_demo_home
    PROMO_EXTRA_ENV=(
        "HOME=$PROMO_DEMO/home"
        "PATH=$PROMO_DEMO/bin:/usr/bin:/bin:/usr/sbin:/sbin"
    )
    promo_start_isolated "$work" "$socket"
    trap 'promo_stop_isolated '"$socket" EXIT

    local base; base=$(promo_base_pane)
    tko send --pane "$base" "cd $PROMO_DEMO/awesome-app && clear" >/dev/null
    tko tab rename --tab 1 setup >/dev/null 2>&1 || true
    sleep 1.5

    promo_record_start "$raw" 40

    # beat 1: 何が足りないかを tako 自身が診断する
    tko send --pane "$base" "tako setup --check" >/dev/null
    sleep 11
    # beat 2: Claude Code への MCP 登録はコマンド 1 つ（user スコープ = 全プロジェクト有効）
    tko send --pane "$base" "clear && tako setup-mcp" >/dev/null
    sleep 7
    # beat 3: Claude Code 側から tako が見えていることを確認する
    tko send --pane "$base" "claude mcp list" >/dev/null
    sleep 14

    promo_record_wait
    promo_stop_isolated "$socket"; trap - EXIT
    PROMO_EXTRA_ENV=()
    promo_verify "$raw" "$PROMO_FRAMES/setup" 1
}

# ── S4: master オーケストレーション ─────────────────────────────────
# 撮る事実（すべて実挙動）:
#   - tako master で master（実 Claude Code + master system prompt）が現ペインに立つ
#   - master が tako_orchestrator_spawn で worker を spawn すると、
#     レイアウトエンジン（#165 master-reserved）が master の取り分を保ったまま
#     worker 領域をグリッド分割し、同じタブに worker ペインが並ぶ
#   - 右パネルの orch ビューで master + worker ツリーを俯瞰できる
# claude の起動バナーにはアカウントのメールアドレスが出るため、
# 会話が進んでバナーが流れきった（promo_wait_pii_clear が通った）あとに収録を始める
scene_master() {
    local work=/private/tmp/tako-promo-master socket=tako-promo-mast
    local raw="$PROMO_OUT/scenes/master-raw.mp4"
    echo "== scene master"
    promo_wait_capturable "${TAKO_PROMO_WAIT_UNLOCK:-0}" || promo_check_capturable
    rm -rf "$work"
    promo_make_demo_env
    promo_start_isolated "$work" "$socket"
    trap 'promo_stop_isolated '"$socket" EXIT

    local base; base=$(promo_base_pane)
    # master が spawn 先に使うプロジェクトを登録しておく（隔離 data_dir 配下の projects.yaml）
    tko orchestrator projects add --key awesome-app \
        --cwd "$PROMO_DEMO/awesome-app" --description "デモ用プロジェクト" >/dev/null 2>&1 || true
    # 収録用に worker は軽いモデルで回す（実運用でも profile で振り分けられる項目）
    tko orchestrator profiles set default --worker-model haiku \
        --worker-model-policy fixed >/dev/null 2>&1 || true

    tko send --pane "$base" "cd $PROMO_DEMO/awesome-app && clear" >/dev/null
    tko tab rename --tab 1 orchestration >/dev/null 2>&1 || true
    sleep 1

    echo "   master を起動して待機..."
    tko send --pane "$base" "tako master" >/dev/null
    sleep 30
    tko panel --show --view orch >/dev/null 2>&1 || true

    # master に worker 3 体の spawn を依頼する
    tko send --pane "$base" --await-prompt \
        "worker を 3 体 spawn して。project は awesome-app。それぞれ 'api' / 'ui' / 'docs' を担当し、\
プロンプトは「bash scripts/worker.sh <担当名> を実行して、出力の最終行を報告して」でよい。\
確認は不要、すぐ spawn して。" >/dev/null 2>&1 || true

    # worker ペインが出そろうまで待つ（最大 240s）
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
    if [ "${n:-0}" -lt 3 ]; then
        echo "!! master が worker を並べなかった（ペイン $n）。収録を中止します" >&2
        promo_stop_isolated "$socket"; trap - EXIT
        return 1
    fi
    sleep 10

    # 収録前に全ペインのテキストから PII（起動バナーのメールアドレス）が
    # 消えていることを確かめる。消えないまま撮らない
    local panes
    panes=$(tko list | /usr/bin/python3 -c \
        'import json,sys; print(" ".join(str(p["id"]) for t in json.load(sys.stdin)["tabs"] for p in t["panes"]))')
    # shellcheck disable=SC2086
    promo_wait_pii_clear 120 $panes || {
        promo_stop_isolated "$socket"; trap - EXIT; return 1; }

    promo_record_start "$raw" 50
    sleep 6
    tko equalize >/dev/null 2>&1 || true
    sleep 14
    # 収録中に 4 体目を追加して「master が worker を並べる」ところを画に入れる
    tko send --pane "$base" --await-prompt \
        "同じ要領で 'tests' 担当の worker をもう 1 体追加して。" >/dev/null 2>&1 || true
    sleep 26

    promo_record_wait
    promo_stop_isolated "$socket"; trap - EXIT
    promo_verify "$raw" "$PROMO_FRAMES/master" 1
}

# ── S5: 再起動して全ペインが復元される ─────────────────────────────
scene_restore() {
    local work=/private/tmp/tako-promo-restore socket=tako-promo-rest
    local raw="$PROMO_OUT/scenes/restore"
    echo "== scene restore"
    promo_wait_capturable "${TAKO_PROMO_WAIT_UNLOCK:-0}" || promo_check_capturable
    rm -rf "$work"
    promo_make_demo_env
    # 1 回目: 永続化 ON で起動しペインを組む
    promo_start_isolated "$work" "$socket" 1
    trap 'promo_stop_isolated '"$socket" EXIT

    local base; base=$(promo_base_pane)
    tko send --pane "$base" "cd $PROMO_DEMO/awesome-app && clear" >/dev/null
    tko tab rename --tab 1 awesome-app >/dev/null 2>&1 || true
    sleep 1
    tko split --pane "$base" --down --cwd "$PROMO_DEMO/awesome-app" -- \
        bash scripts/dev-server.sh >/dev/null
    sleep 1
    tko split --pane "$base" --right --cwd "$PROMO_DEMO/awesome-app" -- \
        bash scripts/worker.sh api >/dev/null
    sleep 2
    tko equalize >/dev/null 2>&1 || true
    sleep 2

    # 前半: 終了する直前の画（ペインが揃っている状態）
    promo_record_start "$PROMO_OUT/scenes/restore-before-raw.mp4" 6
    promo_record_wait
    echo "   終了 → 再起動（復元を待つ）"
    promo_stop_isolated "$socket"
    sleep 3
    # 後半: 再起動して復元された画。ウィンドウ ID が変わるので撮り直す
    promo_start_isolated "$work" "$socket" 1
    sleep 6
    promo_record_start "$PROMO_OUT/scenes/restore-after-raw.mp4" 10
    promo_record_wait

    promo_stop_isolated "$socket"; trap - EXIT
    promo_verify "$PROMO_OUT/scenes/restore-before-raw.mp4" "$PROMO_FRAMES/restore-before" 1
    promo_verify "$PROMO_OUT/scenes/restore-after-raw.mp4" "$PROMO_FRAMES/restore-after" 1
}

# ── S7: テーマ切替 + コマンドパレット ──────────────────────────────
scene_outro() {
    local work=/private/tmp/tako-promo-outro socket=tako-promo-out
    local raw="$PROMO_OUT/scenes/outro-raw.mp4"
    echo "== scene outro"
    promo_wait_capturable "${TAKO_PROMO_WAIT_UNLOCK:-0}" || promo_check_capturable
    rm -rf "$work"
    promo_make_demo_env
    promo_start_isolated "$work" "$socket"
    trap 'promo_stop_isolated '"$socket" EXIT

    local base; base=$(promo_base_pane)
    tko send --pane "$base" "cd $PROMO_DEMO/awesome-app && clear" >/dev/null
    tko tab rename --tab 1 awesome-app >/dev/null 2>&1 || true
    sleep 1
    tko split --pane "$base" --down --cwd "$PROMO_DEMO/awesome-app" -- \
        bash scripts/dev-server.sh >/dev/null
    sleep 1
    tko open --pane "$base" "$PROMO_DEMO/awesome-app/README.md" >/dev/null
    sleep 2

    promo_record_start "$raw" 20
    sleep 2
    tko theme light >/dev/null; sleep 4
    tko theme dark >/dev/null;  sleep 4
    tko lang en >/dev/null 2>&1 || true; sleep 3
    tko lang ja >/dev/null 2>&1 || true; sleep 3

    promo_record_wait
    promo_stop_isolated "$socket"; trap - EXIT
    promo_verify "$raw" "$PROMO_FRAMES/outro" 1
}

case "$SCENE" in
    preview) scene_preview ;;
    agent)   scene_agent ;;
    setup)   scene_setup ;;
    master)  scene_master ;;
    restore) scene_restore ;;
    outro)   scene_outro ;;
    all)     scene_agent; scene_preview; scene_setup; scene_master; scene_outro ;;
    *) echo "unknown scene: $SCENE" >&2; exit 2 ;;
esac
echo "== done: $SCENE"
