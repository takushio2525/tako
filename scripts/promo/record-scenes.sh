#!/bin/bash
# tako 紹介動画 シーン収録スクリプト (#470 Phase B)
#
# 使い方:
#   scripts/promo/record-scenes.sh <scene>
#     scene = agent | preview | restore | outro | all
#
#   agent   … S1/S2/S3 素材（実 Claude Code + master + worker spawn を通しで収録）
#   preview … S4 素材（Markdown ライブリロード + Code Runner）
#   restore … S5 素材（再起動して全ペインが復元される）
#   outro   … S7 素材（テーマ切替 + パレット）
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

# ── S4: プレビュー（Markdown ライブリロード + Code Runner）──────────
scene_preview() {
    local work=/private/tmp/tako-promo-preview socket=tako-promo-prev
    local raw="$PROMO_OUT/scenes/preview-raw.mp4"
    echo "== scene preview"
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
    # beat 4: Code Runner で実行 → 新ペインにビルドログ
    tko run "$PROMO_DEMO/awesome-app/scripts/build.sh" >/dev/null 2>&1 || true
    sleep 6

    promo_record_wait
    promo_stop_isolated "$socket"; trap - EXIT
    promo_verify "$raw" "$PROMO_OUT/frames/preview" 1
}

# ── S1/S2/S3: 実 Claude Code + オーケストレーション ─────────────────
scene_agent() {
    local work=/private/tmp/tako-promo-agent socket=tako-promo-agent
    local raw="$PROMO_OUT/scenes/agent-raw.mp4"
    echo "== scene agent"
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
    promo_verify "$raw" "$PROMO_OUT/frames/agent" 1
}

# ── S5: 再起動して全ペインが復元される ─────────────────────────────
scene_restore() {
    local work=/private/tmp/tako-promo-restore socket=tako-promo-rest
    local raw="$PROMO_OUT/scenes/restore"
    echo "== scene restore"
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
    promo_verify "$PROMO_OUT/scenes/restore-before-raw.mp4" "$PROMO_OUT/frames/restore-before" 1
    promo_verify "$PROMO_OUT/scenes/restore-after-raw.mp4" "$PROMO_OUT/frames/restore-after" 1
}

# ── S7: テーマ切替 + コマンドパレット ──────────────────────────────
scene_outro() {
    local work=/private/tmp/tako-promo-outro socket=tako-promo-out
    local raw="$PROMO_OUT/scenes/outro-raw.mp4"
    echo "== scene outro"
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
    promo_verify "$raw" "$PROMO_OUT/frames/outro" 1
}

case "$SCENE" in
    preview) scene_preview ;;
    agent)   scene_agent ;;
    restore) scene_restore ;;
    outro)   scene_outro ;;
    all)     scene_preview; scene_agent; scene_restore; scene_outro ;;
    *) echo "unknown scene: $SCENE" >&2; exit 2 ;;
esac
echo "== done: $SCENE"
