#!/bin/bash
# tako 紹介動画 シーン収録スクリプト (#470 Phase B)
#
# 使い方:
#   scripts/promo/record-scenes.sh <scene>
#     scene = agent | preview | setup | master | project | restore | outro | all
#
#   agent   … S1/S2/S3 素材（実 Claude Code がペインを割って dev サーバー・プレビューを開く）
#   preview … S4 素材（Markdown ライブリロード + Code Runner）
#   setup   … S5 素材（tako setup → 対話セットアップエージェントと会話して設定が決まる。v3）
#   master  … S6a/S6b 素材（tako master → worker が同じタブに並ぶオーケストレーション）
#   project … S6c 素材（ホームで起動した master が登録済みプロジェクトを解決する。v3）
#   restore … 補足素材（再起動して全ペインが復元される）
#   outro   … S7 素材（テーマ切替 + パレット）
#
# 台本上の並び（.agent/plans/2026-07-promo-video.md）は
#   画面操作 → プレビュー → setup → master（+ プロジェクト文脈）→ クロージング
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

# ── S5: setup（対話セットアップエージェント）────────────────────────
# v3 で訴求を作り直した。売りは「コマンドを覚えること」ではなく
# **設定ファイルを自分で書かずに、対話アシスタントと会話して環境に合わせられる**こと。
#
# 撮る事実（すべて実装で裏を取った実挙動。誇張しない）:
#   1. `tako setup` は質問ゼロで検出（検出値 → 前回値 → 既定値）を終える
#      （crates/tako-cli/src/setup.rs）
#   2. 検出が終わると **対話アシスタントが自動で起動する**。スキップは
#      `--yes` / 非 TTY / `--answers launch_agent=none` のときだけ
#      （setup.rs の skip_agent 判定）
#   3. アシスタントは resources/setup/system-prompt.md を指示として動き、
#      setup-context.yaml・グローバル指示ファイル・profiles/config/projects を
#      読んでから対話する（= 現状を把握したうえで相談できる）
#   4. 同梱の推奨ルール（resources/setup/templates/sections/ の 7 項目）と
#      既存のグローバル指示ファイルを項目単位で突き合わせ、
#      **同意した項目だけ**反映する（既存のカスタマイズを黙って上書きしない）
#   5. プロファイル（master / worker のエージェント・モデル・思考量）と
#      プロジェクト登録も会話で決められる
#
# HOME / PATH はデモ用（lib.sh の promo_make_demo_home）に差し替えて撮るので、
# 画面に出るパスは /private/tmp 配下だけになる。デモ HOME でも実 claude が
# 対話できるようにする細工は promo_demo_home_agent_ready を参照
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
        # 収録のテンポのため応答の速いモデルで対話させる（既定モデルだと 1 回の応答に
        # 1 分以上かかり画面が「thinking」のまま止まる）。挙動そのものは変わらない
        "ANTHROPIC_MODEL=${TAKO_PROMO_SETUP_MODEL:-claude-sonnet-5}"
    )
    promo_start_isolated "$work" "$socket"
    trap 'promo_stop_isolated '"$socket" EXIT

    local base; base=$(promo_base_pane)
    # 収録機と無関係な listen ポート（他アプリ）の提案チップが写り込まないようにする
    tko portdetect off >/dev/null 2>&1 || true
    tko send --pane "$base" "cd $PROMO_DEMO/awesome-app && clear" >/dev/null
    tko tab rename --tab 1 setup >/dev/null 2>&1 || true
    sleep 1.5

    # アシスタントは指示どおり現状（setup-context.yaml・指示ファイル・profiles・projects）を
    # 読んでから答えるため、1 往復に 40〜70 秒かかる。素材は長めに撮って編集で選ぶ
    promo_record_start "$raw" "${TAKO_PROMO_SETUP_DUR:-200}"

    # beat 1: `tako setup` … 質問ゼロで検出が終わり、対話アシスタントが自動で立ち上がる
    tko send --pane "$base" "tako setup" >/dev/null
    sleep 70
    # beat 2: 日本語で相談する（アシスタントは現状を読んでから答える）
    tko send --pane "$base" --await-prompt \
        "いまの設定を 3 行で教えて。" >/dev/null 2>&1 || true
    sleep 60
    # beat 3: 設定変更まで会話で決まる
    tko send --pane "$base" --await-prompt \
        "品質重視で使いたい。worker の設定をそれに合わせて。" >/dev/null 2>&1 || true
    sleep 70

    promo_record_wait
    promo_stop_isolated "$socket"; trap - EXIT
    PROMO_EXTRA_ENV=()
    promo_verify "$raw" "$PROMO_FRAMES/setup" 1
}

# ── S6c: master のプロジェクト文脈理解 ──────────────────────────────
# 撮る事実（実装で裏を取った範囲だけ。誇張しない）:
#   - projects.yaml（`~/Library/Application Support/tako/orchestrator/projects.yaml`。
#     実体は data_dir 配下 = 隔離時は隔離ディレクトリ）に key / cwd / description を登録できる
#   - master の system prompt は Step 0 で「依頼を受けたらまず
#     `tako_orchestrator_projects(action=list)` を引き、key / cwd のベース名 /
#     description と突き合わせて対象プロジェクトを決める」と規定している
#     （crates/tako-control/src/orchestrator/default_system_prompt.md）
#   - spawn は project キーから cwd を解決して worker を起動する
#     （dispatch.rs の `ProjectsConfig::resolve_cwd`）。つまり **master 自身の cwd は
#     どこでもよく**、ホームで起動しても worker はプロジェクトのディレクトリで立つ
# 撮らない（実装で裏が取れないため）: 「最近何をやっていたかを自動で把握する」。
#   projects.yaml が持つのは key / cwd / description だけで、進捗の記憶ではない
scene_project() {
    local work=/private/tmp/tako-promo-project socket=tako-promo-proj
    local raw="$PROMO_OUT/scenes/project-raw.mp4"
    echo "== scene project"
    promo_wait_capturable "${TAKO_PROMO_WAIT_UNLOCK:-0}" || promo_check_capturable
    rm -rf "$work"
    promo_make_demo_env
    promo_make_demo_home
    # 「複数のプロジェクトから正しいものを選ぶ」ことが分かるよう、ダミーを 2 件足す
    mkdir -p "$PROMO_DEMO/docs-site" "$PROMO_DEMO/sensor-firmware"
    PROMO_EXTRA_ENV=(
        "HOME=$PROMO_DEMO/home"
        "PATH=$PROMO_DEMO/bin:/usr/bin:/bin:/usr/sbin:/sbin"
        # 収録のテンポのため応答の速いモデルで動かす（挙動そのものは変わらない）
        "ANTHROPIC_MODEL=${TAKO_PROMO_PROJECT_MODEL:-claude-sonnet-5}"
    )
    promo_start_isolated "$work" "$socket"
    trap 'promo_stop_isolated '"$socket" EXIT

    local base; base=$(promo_base_pane)
    tko portdetect off >/dev/null 2>&1 || true
    tko orchestrator projects add --key awesome-app \
        --cwd "$PROMO_DEMO/awesome-app" --description "デモ用の Web アプリ" >/dev/null 2>&1 || true
    tko orchestrator projects add --key docs-site \
        --cwd "$PROMO_DEMO/docs-site" --description "デモ用のドキュメントサイト" >/dev/null 2>&1 || true
    tko orchestrator projects add --key sensor-firmware \
        --cwd "$PROMO_DEMO/sensor-firmware" --description "デモ用のファームウェア" >/dev/null 2>&1 || true
    # 収録用に worker は軽いモデル・master は控えめな effort で回す
    # （tko は隔離 data_dir へ書くので本番のプロファイルには触れない）
    tko orchestrator profiles set default --worker-model haiku \
        --worker-model-policy fixed --effort medium >/dev/null 2>&1 || true

    # デモ HOME の Claude Code へ tako MCP を登録する（user スコープ）
    tko send --pane "$base" "tako setup-mcp" >/dev/null
    sleep 8
    # プロジェクトディレクトリではなく **ホーム** で master を起動する
    tko send --pane "$base" "cd ~ && clear" >/dev/null
    tko tab rename --tab 1 home >/dev/null 2>&1 || true
    sleep 1

    echo "   master を起動して待機..."
    tko send --pane "$base" "tako master" >/dev/null
    sleep 30
    promo_wait_pii_clear 120 "$base" || {
        promo_stop_isolated "$socket"; trap - EXIT; PROMO_EXTRA_ENV=(); return 1; }

    promo_record_start "$raw" "${TAKO_PROMO_PROJECT_DUR:-120}"
    sleep 4
    # プロジェクト名を言うだけ。cd もパス指定もしない。
    # --await-prompt は送達検証で Enter を撃ち直すため、生成中の master を
    # 中断させることがある（v3 の 1 回目の収録で実際に Interrupted になった）。
    # ここは 1 回だけ素直に送る
    tko send --pane "$base" \
        "awesome-app の件、worker を 1 体立てて bash scripts/worker.sh docs を実行して、最終行を報告して。" \
        >/dev/null 2>&1 || true
    sleep 110

    promo_record_wait
    promo_stop_isolated "$socket"; trap - EXIT
    PROMO_EXTRA_ENV=()
    promo_verify "$raw" "$PROMO_FRAMES/project" 1
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
    project) scene_project ;;
    restore) scene_restore ;;
    outro)   scene_outro ;;
    all)     scene_agent; scene_preview; scene_setup; scene_master; scene_project; scene_outro ;;
    *) echo "unknown scene: $SCENE" >&2; exit 2 ;;
esac
echo "== done: $SCENE"
