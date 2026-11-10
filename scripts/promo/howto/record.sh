#!/bin/bash
# record.sh — 入れ方・使い方動画（#2866）の実画面の収録。
#
# 収録の仕組み（砂場・隔離 GUI・読み戻し・連番の JPEG と時刻の表・片付け）は、プロモーション動画
# （#2864）の scripts/promo/remake/record.sh の関数をそのまま読み込んで使う（複製しない）。
# ここに書くのは、この動画の砂場の差分と場面だけ。台本と根拠は .agent/plans/2026-11-howto-video.md。
#
# 守っていること:
#   - 窓は tako-vd にだけ出す（立て方は scripts/lib/isolated-gui.sh の 1 実装）。本番の tako には触らない
#   - 本物の claude / codex / agy は起こさない。砂場の ~/.local/bin に台（demo/claude-stub.mjs）を置く
#   - `tako setup` と初回起動の自動セットアップは砂場の HOME にだけ書く。収録の前後で本物の HOME の設定と
#     /Applications/tako.app が変わっていないことを自分で確かめ、変わっていたら終了コード 3 で終わる
#   - 画面に出る HOME・プロンプト・パス・プロジェクトはすべてデモ用。tako の置き場も砂場の HOME の下の
#     既定と同じ形（~/Library/Application Support/tako）にして、実機と同じ見え方にする
#
# 使い方:
#   git apply scripts/promo/remake/promo-rec.patch      # 収録用の読み戻し（#2864 の差分）を入れて建てる
#   bash scripts/promo/howto/record.sh [場面…]           # 場面を省くと全部（firstrun setup update master）
#   git checkout crates/tako-app/src/main.rs
#   建て済みの収録用バイナリを使い回すなら TAKO_BIN / APP_BIN で置き場を渡す（その場合は建て直さない）
#   bash scripts/promo/howto/record.sh hold              # 収録せずに立てたまま止める（<砂場>/STOP を置くと片付けて終わる）
#   出力は TAKO_PROMO_OUT（既定 ~/Movies/tako-promo/howto）の scenes/<場面>/
#   #2864 の一式の置き場は TAKO_HOWTO_REMAKE_DIR（既定は同じ木の scripts/promo/remake）

set -uo pipefail

HOWTO=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
REMAKE=${TAKO_HOWTO_REMAKE_DIR:-$HOWTO/../remake}
[ -f "$REMAKE/record.sh" ] || {
    printf 'ERROR: %s\n' "#2864 の収録の一式が無い: ${REMAKE}（TAKO_HOWTO_REMAKE_DIR で置き場を渡す）" >&2
    exit 1
}
REMAKE=$(cd "$REMAKE" && pwd)

# 出力先と砂場はこの動画のもの（#2864 の収録と混ざらない名前にする）。
# 砂場の名前は画面のパスに出るので、プロセス番号を入れない固定の名前にする
TAKO_PROMO_OUT=${TAKO_PROMO_OUT:-$HOME/Movies/tako-promo/howto}
TAKO_PROMO_SANDBOX=${TAKO_PROMO_SANDBOX:-/private/tmp/tako-demo}
# 片付けは砂場ごと消すので、先に在るもの（他の収録のもの）には手を出さずに止まる
[ -e "$TAKO_PROMO_SANDBOX" ] && {
    printf 'ERROR: %s\n' "砂場の置き場が既に在る: ${TAKO_PROMO_SANDBOX}（他の収録が使っていないか確かめる。別の置き場は TAKO_PROMO_SANDBOX）" >&2
    exit 1
}
# 窓は tako-vd の右下に置く（真ん中に出る #2864 の収録の窓と重なり切らない位置）
HOWTO_WIN_X=${TAKO_HOWTO_WIN_X:-1270}
HOWTO_WIN_Y=${TAKO_HOWTO_WIN_Y:-700}

# #2864 の record.sh の関数の部分（「本体」より前）だけを読む。HERE / ROOT はこのファイルの位置から決まる
# （scripts/promo/howto と scripts/promo/remake は同じ深さ）
eval "$(awk '/^# ---- 本体/ { exit } { print }' "$REMAKE/record.sh")"

# 読み込んだ既定のうち、この動画で変えるもの
DEMO_SRC="$REMAKE/demo"   # 偽のエージェント（agent-sim.mjs）とデモ用プロジェクト（shop-app）は #2864 のもの
SOCK="tkh-$$"
PERSONAS="$SB/personas"   # 台本の置き場（この動画の台本 + #2864 の担当の台本を砂場で 1 つにまとめる）
GUARD="$SB/guard"
DATA="$DEMO_HOME/Library/Application Support/tako"   # 実機の既定と同じ形（画面には ~/Library/… と出る）

# 隔離 GUI へだけ届く CLI。台帳や設定を手元で読み書きするコマンドのために、GUI と同じ HOME と data dir を渡す
cli() {
    HOME="$DEMO_HOME" TAKO_DATA_DIR="$DATA" iso_cli "$@"
}

# ---- 本物の HOME と /Applications が変わっていないことの確かめ -------------------

# 利用者のホーム（HOME は砂場へ差し替えるので、パスワードの登録から引く）
real_home() {
    iso_real_home 2> /dev/null || printf '%s' "$HOME"
}

# 控えを 1 枚書く。tako setup と初回起動の自動セットアップが書きうる先だけを見る
# （シェルの設定・エージェントの設定と MCP 登録・指示ファイル・tako CLI のリンク・tako の設定・アプリ本体）。
# ~/.claude.json は動いている claude が常に書き換えるので、tako の MCP 登録の欄だけを比べる
guard_snapshot() {
    local out="$1" home p
    home=$(real_home)
    : > "$out"
    for p in .zshrc .zprofile .zshenv .zlogin .bash_profile .bash_login .profile \
        .claude/CLAUDE.md .claude/settings.json .claude/skills/tako \
        .codex/config.toml .codex/AGENTS.md .gemini/GEMINI.md .gemini/config/mcp_config.json \
        .local/bin/tako \
        "Library/Application Support/tako/config.yaml" "Library/Application Support/tako/settings.json" \
        "Library/Application Support/tako/setup" "Library/Application Support/tako/profiles" \
        "Library/Application Support/tako/solo-profiles" "Library/Application Support/tako/orchestrator/projects.yaml" \
        Library/LaunchAgents; do
        if [ -e "$home/$p" ] || [ -L "$home/$p" ]; then
            printf '%s\t~/%s\n' "$(stat -f '%m %z' "$home/$p")" "$p" >> "$out"
        else
            printf 'なし\t~/%s\n' "$p" >> "$out"
        fi
    done
    printf '%s\t~/.local/bin/tako の指す先\n' "$(readlink "$home/.local/bin/tako" 2> /dev/null | sed "s#^$home#~#")" >> "$out"
    printf '%s\t~/.claude.json の mcpServers.tako\n' \
        "$(node -e 'try { const c = JSON.parse(require("fs").readFileSync(process.argv[1], "utf8")); process.stdout.write(require("crypto").createHash("sha256").update(JSON.stringify((c.mcpServers || {}).tako ?? null)).digest("hex").slice(0, 16)); } catch { process.stdout.write("読めない"); }' "$home/.claude.json")" >> "$out"
    for p in /Applications/tako.app /Applications/tako.app/Contents/Info.plist \
        /Applications/tako.app/Contents/MacOS/tako /Applications/tako.app/Contents/MacOS/tako-app; do
        if [ -e "$p" ]; then
            printf '%s\t%s\n' "$(stat -f '%m %z' "$p")" "$p" >> "$out"
        else
            printf 'なし\t%s\n' "$p" >> "$out"
        fi
    done
}

guard_check() {
    guard_snapshot "$GUARD/after.txt"
    if diff "$GUARD/before.txt" "$GUARD/after.txt" > "$GUARD/diff.txt"; then
        say "本物の HOME と /Applications/tako.app: 収録の前後で変化なし（$(wc -l < "$GUARD/after.txt" | tr -d ' ') 項目）"
        return 0
    fi
    printf 'ERROR: %s\n' "本物の HOME か /Applications/tako.app が収録の間に変わった（差分は下。原因を確かめるまで収録を使わない）" >&2
    cat "$GUARD/diff.txt" >&2
    return 1
}

# 片付けの前に確かめる（砂場ごと消える前に結果を出力先へ残す）
howto_cleanup() {
    local rc=0
    if [ -f "$GUARD/before.txt" ]; then
        guard_check || rc=1
        mkdir -p "$OUT/guard" && command cp -f "$GUARD"/*.txt "$OUT/guard/" 2> /dev/null
    fi
    cleanup
    [ "$rc" = 0 ] || exit 3
}
trap howto_cleanup EXIT

# ---- 砂場（#2864 の砂場に、この動画の差分を足す） --------------------------------

make_howto_sandbox() {
    local node
    make_sandbox
    node=$(command -v node) || die "node が見つからない"
    mkdir -p "$GUARD" "$PERSONAS" "$DEMO_HOME/.local/bin" "$SB/app" || die "砂場の置き場を作れない"
    # 台本: 担当（api / docs）は #2864 のものをそのまま使い、司令塔とセットアップのアシスタントはこの動画のもの
    command cp -f "$DEMO_SRC/personas/api.json" "$DEMO_SRC/personas/docs.json" "$PERSONAS/" || die "#2864 の担当の台本が無い"
    command cp -f "$HOWTO"/demo/personas/*.json "$PERSONAS/" || die "この動画の台本が無い"
    # この動画の前提は「Claude Code だけが入っていてログイン済み」。#2864 の偽物 3 つは外し、
    # claude の台を本物のインストーラーと同じ置き場（~/.local/bin）に置く
    command rm -f "${SB:?}/bin/claude" "${SB:?}/bin/codex" "${SB:?}/bin/agy"
    printf '%s\n' '#!/bin/sh' "exec \"$node\" \"$HOWTO/demo/claude-stub.mjs\" \"\$@\"" > "$DEMO_HOME/.local/bin/claude"
    chmod +x "$DEMO_HOME/.local/bin/claude"
    # 作業中の差分は無しで始める（master に頼む前のきれいな状態）
    (cd "$DEMO_HOME/shop-app" && HOME="$DEMO_HOME" GIT_CONFIG_NOSYSTEM=1 git checkout -q -- .) || die "デモ用プロジェクトを戻せない"
}

# 収録に使うバイナリを砂場へ写して、そこから起こす（置き場のパスが PATH やリンク先として画面へ出ても、
# 利用者のホームのパスにならない）
stage_bins() {
    command cp -f "$TAKO_BIN" "$SB/app/tako" && command cp -f "$APP_BIN" "$SB/app/tako-app" || die "バイナリを砂場へ写せない"
    TAKO_BIN="$SB/app/tako"
    APP_BIN="$SB/app/tako-app"
}

# 隔離 GUI。#2864 の gui_up に、この動画の置き場・台本・窓の位置・台の設定を足す（後ろに並べた値が勝つ）
howto_gui_up() {
    local log="$1" node_dir
    shift
    node_dir=$(dirname "$(command -v node)")
    gui_up "$log" \
        TAKO_DATA_DIR="$DATA" \
        PATH="$DEMO_HOME/.local/bin:$SB/app:$node_dir:/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin" \
        TAKO_WINDOW_BOUNDS="${HOWTO_WIN_X},${HOWTO_WIN_Y},${WIN_W},${WIN_H}" \
        TAKO_FIRST_RUN=1 \
        AGENT_SIM_DIR="$PERSONAS" \
        AGENT_SIM_MJS="$DEMO_SRC/agent-sim.mjs" \
        CLAUDE_STUB_BOOT_MS="${TAKO_HOWTO_BOOT_MS:-2500}" \
        CLAUDE_STUB_LOG="$SB/log/claude-stub.tsv" \
        "$@"
}

# 初回起動の自動セットアップが終わって「次にやること」が master になるまで待つ
# （案内の状態は `tako welcome` が返す。バナーが読んでいるのと同じ値）
wait_auto_setup() {
    local i state
    for i in $(seq 1 100); do
        # 読み切ってから見る（pipefail の下で grep -q へ直にパイプしない）
        state=$(cli welcome 2> /dev/null) && grep -q '"state":"done".*"kind":"master"' <<< "$state" && return 0
        sleep 0.3
    done
    die "初回起動の自動セットアップが 30 秒で終わらなかった: $(cli welcome 2>&1 | head -c 400)"
}

# コマンドを打った所が見えるよう、打ってから少しおいて Enter を押す
type_and_enter() {
    cli send --pane 1 --no-newline "$1" > /dev/null
    sleep 1.4
    cli send --pane 1 --key Enter > /dev/null
}

# ---- 場面 -------------------------------------------------------------------

# 場面 1: 初回起動。タブバーの下に案内が出て、初期設定が自動で進み、「次にやること」が出る
# （本物の初回起動の導線 = #1510。検証用 GUI では TAKO_FIRST_RUN=1 で明示して動かす）
scene_firstrun() {
    rec_start firstrun
    howto_gui_up firstrun
    rec_beat window
    wait_auto_setup
    rec_beat auto_done
    sleep 7
    rec_stop
}

# 場面 2: tako setup を自分で打つ（済んだ段は素通りし、最後に対話アシスタントが開く）
scene_setup() {
    rec_start setup
    sleep 1.2
    # 案内を「あとで」で閉じる（その回だけ閉じる。ボタンと同じ要求）
    rec_beat hide
    cli welcome hide > /dev/null
    sleep 1.6
    rec_beat type
    type_and_enter "tako setup"
    rec_beat enter
    wait_beat solo:ready 90
    sleep 5
    rec_stop
    # アシスタントを閉じてシェルへ戻る
    cli send --pane 1 --key C-c > /dev/null 2>&1
    sleep 1
    cli send --pane 1 "clear" > /dev/null
    sleep 1
}

# 場面 3: 更新の確認（tako update check。公開のリリース一覧を読むだけで、何も入れない）
scene_update() {
    rec_start update
    sleep 1.0
    rec_beat type
    type_and_enter "tako update check"
    rec_beat enter
    sleep 6
    rec_stop
    cli send --pane 1 "clear" > /dev/null
    sleep 1
}

# 場面 4: master を起動して頼む（本物の tako master / orchestrator projects add / orchestrator spawn）
scene_master() {
    cli orchestrator layout --min-worker-cols 30 > /dev/null
    rec_start master
    sleep 1.0
    rec_beat type_master
    type_and_enter "tako master"
    wait_beat master:ready 40
    sleep 2.5
    rec_beat ask_project
    cli send --pane 1 "~/shop-app を管理対象に追加して" > /dev/null
    wait_beat master:project_done 40
    sleep 3.0
    rec_beat ask_work
    cli send --pane 1 "決済 API にリトライを入れて、README も更新して" > /dev/null
    wait_beat master:report 150
    sleep 5
    rec_stop
}

# 収録せずに立てたまま止める（画面や CLI の振る舞いを確かめる用）。<砂場>/STOP を置くと片付けて終わる
hold() {
    local i
    say "HOLD: 砂場=${SB} 発見の置き場=${ISOLATED_GUI_DISCOVERY_DIR} GUI=${GUI_PID}"
    printf '%s\n' "SB=$SB" "DISCOVERY=$ISOLATED_GUI_DISCOVERY_DIR" "GUI_PID=$GUI_PID" "TAKO_BIN=$TAKO_BIN" "REC=$REC" > "$SB/hold.env"
    for i in $(seq 1 3600); do
        [ -e "$SB/STOP" ] && break
        kill -0 "$GUI_PID" 2> /dev/null || break
        sleep 1
    done
}

# ---- 本体 -------------------------------------------------------------------

command -v ffmpeg > /dev/null || die "ffmpeg が見つからない"

scenes="$*"
[ -n "$scenes" ] || scenes="firstrun setup update master"
for s in $scenes; do
    case "$s" in
        firstrun | setup | update | master | hold) ;;
        *) die "知らない場面: ${s}（firstrun / setup / update / master / hold）" ;;
    esac
done

make_howto_sandbox
guard_snapshot "$GUARD/before.txt"
mkdir -p "$OUT/scenes" || die "出力先を作れない: $OUT"
# 蓋を閉じた運用では tako-vd がすぐ眠るので、収録の間だけ起こしておく
caffeinate -d -u -t 3600 &
CAFF_PID=$!
isolated_gui_bins visual-test || die "収録用のバイナリを建てられない"
# 読み戻しの口（#2864 の promo-rec.patch）が入ったバイナリか。入っていないと 1 枚も撮れない
grep -q -a 'TAKO_PROMO_REC_DIR' "$APP_BIN" ||
    die "収録用の差分が入っていないバイナリ: ${APP_BIN}（先に git apply scripts/promo/remake/promo-rec.patch）"
stage_bins

for s in $scenes; do
    case "$s" in
        firstrun) scene_firstrun ;;
        *)
            # 初回起動の場面を撮らずに始めるとき（場面を選んだ撮り直し）も、同じ初回起動の導線を通してから撮る
            if [ -z "$GUI_PID" ]; then
                howto_gui_up main
                wait_auto_setup
                sleep 4
                # 案内を閉じる所は setup の場面で撮る。それより後ろの場面から始めるなら、ここで閉じておく
                [ "$s" = setup ] || [ "$s" = hold ] || cli welcome hide > /dev/null
            fi
            if [ "$s" = hold ]; then hold; else "scene_$s"; fi
            ;;
    esac
done
say "収録おわり: $OUT/scenes"
