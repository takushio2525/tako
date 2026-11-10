#!/bin/bash
# record.sh — プロモ動画（リメイク版）の実画面の収録。
#
# 何をするか:
#   砂場（デモ用の HOME・プロジェクト・偽のエージェント）を作り、隔離した tako を仮想ディスプレイ
#   tako-vd へ立て、場面ごとに操作して、窓の絵を連番の JPEG と時刻の表にして書き出す。
#   終わったら自分で起こしたものだけを片付ける。
#
# 守っていること:
#   - 窓は tako-vd にだけ出す（立て方は scripts/lib/isolated-gui.sh の 1 実装）。本番の tako には触らない
#   - 本物の claude / codex / agy は起こさない。PATH の先頭に偽物（demo/agent-sim.mjs）を置く
#   - 画面に出る HOME・プロンプト・タブ名・プロジェクトはすべてデモ用（実ユーザー名・実ホームを出さない）
#
# 撮り方（読み戻し）:
#   画面がロックされていると OS の画面収録（screencapture / ScreenCaptureKit）は窓単体でも断られる。
#   そこで収録専用の差分 promo-rec.patch を手元の木に当て、visual-test の読み戻し（Metal の最終 scene）で
#   窓の絵を一定間隔で書き出す。差分は UI の描画に触らず、コミットもしない（当て方は使い方の節）。
#
# 使い方:
#   git apply scripts/promo/remake/promo-rec.patch      # 収録の前に 1 回
#   bash scripts/promo/remake/record.sh [場面…]          # 場面を省くと全部（fleet drive extras restore）
#   git checkout crates/tako-app/src/main.rs            # 終わったら戻す（差分はコミットしない）
#   建て済みの収録用バイナリを使い回すなら TAKO_BIN / APP_BIN で置き場を渡す（その場合は建て直さない）
#   出力は TAKO_PROMO_OUT（既定 ~/Movies/tako-promo/remake）の scenes/<場面>/ に
#   000001.jpg…（窓の絵）・frames.tsv（番号と場面の頭からの ms）・beats.tsv（出来事と ms）

set -uo pipefail

HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
ROOT=$(cd "$HERE/../../.." && pwd)
DEMO_SRC="$HERE/demo"
OUT=${TAKO_PROMO_OUT:-$HOME/Movies/tako-promo/remake}
# 砂場は短いパスに置く（IPC のソケットのパス長に上限がある）
SB=${TAKO_PROMO_SANDBOX:-/private/tmp/tkp-$$}
DEMO_HOME="$SB/home/demo"
REC="$SB/rec"
SOCK="tkp-$$"
WIN_W=${TAKO_PROMO_WIN_W:-1280}
WIN_H=${TAKO_PROMO_WIN_H:-720}
CAFF_PID=""
GUI_PID=""

# shellcheck source=../../lib/isolated-gui.sh
. "$ROOT/scripts/lib/isolated-gui.sh"
# shellcheck source=../../lib/tmux-socket.sh
. "$ROOT/scripts/lib/tmux-socket.sh"

say() { printf '%s\n' "$*"; }
die() { printf 'ERROR: %s\n' "$*" >&2; exit 1; }
now_ms() { perl -MTime::HiRes -e 'printf "%d", Time::HiRes::time() * 1000'; }

cleanup() {
    [ -n "$GUI_PID" ] && stop_isolated_gui "$GUI_PID"
    # 永続化の場面で起こした自分の tmux サーバー（隔離ソケット）だけを、ソケットファイルまで落とす
    tmux_socket_kill "$SOCK"
    [ -n "$CAFF_PID" ] && kill "$CAFF_PID" 2>/dev/null
    command rm -rf "${SB:?}"
    say "片付け: 隔離 GUI・tmux ソケット ${SOCK}・砂場 ${SB} を消した"
}
trap cleanup EXIT

# ---- 砂場 -------------------------------------------------------------------

make_sandbox() {
    local node agent
    node=$(command -v node) || die "node が見つからない"
    mkdir -p "$DEMO_HOME" "$SB/bin" "$SB/data" "$SB/log" "$REC" || die "砂場を作れない: $SB"
    # 固定のプロンプト（実ユーザー名・ホスト名を出さない）
    printf '%s\n' "PROMPT='%F{magenta}demo%f %1~ %# '" "RPROMPT=''" > "$DEMO_HOME/.zshrc"
    printf '%s\n' '[user]' '    name = demo' '    email = demo@example.com' '[init]' '    defaultBranch = main' > "$DEMO_HOME/.gitconfig"
    # 偽のエージェント: 本物の CLI の名前で PATH の先頭に置く
    for agent in claude codex agy; do
        printf '%s\n' '#!/bin/sh' "exec \"$node\" \"$DEMO_SRC/agent-sim.mjs\" $agent \"\$@\"" > "$SB/bin/$agent"
        chmod +x "$SB/bin/$agent"
    done
    # デモ用プロジェクト（履歴つき）
    command cp -R "$DEMO_SRC/shop-app" "$DEMO_HOME/shop-app"
    (
        cd "$DEMO_HOME/shop-app" || exit 1
        export HOME="$DEMO_HOME" GIT_CONFIG_NOSYSTEM=1
        git init -q .
        git add package.json src/lib/http.ts && git commit -q -m "プロジェクトの土台"
        git checkout -q -b feature/retry
        git add src/lib/retry.ts src/api/payments.ts && git commit -q -m "決済にリトライを入れる"
        git add test/payments.test.ts && git commit -q -m "リトライのテストを足す"
        git checkout -q main
        git checkout -q -b feature/search
        git add src/api/search.ts && git commit -q -m "商品検索を実装する"
        git checkout -q main
        git merge -q --no-ff feature/retry -m "feature/retry を取り込む"
        git merge -q --no-ff feature/search -m "feature/search を取り込む"
        git add README.md && git commit -q -m "README に使い方を書く"
        # 作業中の差分を 1 つ残す（git パネルに変更が出る）
        printf '\n// TODO: タイムアウトも設定できるようにする\n' >> src/lib/http.ts
    ) || die "デモ用プロジェクトを作れない"
}

# ---- 隔離 GUI ---------------------------------------------------------------

gui_up() {
    # $1 = ログの名前。残りは GUI へ足す VAR=VAL
    local log="$SB/log/$1.log" node_dir
    shift
    node_dir=$(dirname "$(command -v node)")
    ISOLATED_GUI_BOUNDS="${WIN_W},${WIN_H}"
    launch_isolated_gui "$log" \
        HOME="$DEMO_HOME" \
        TAKO_DATA_DIR="$SB/data" \
        PATH="$SB/bin:$(dirname "$TAKO_BIN"):$node_dir:/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin" \
        TAKO_PROMO_REC_DIR="$REC" \
        TAKO_PROMO_REC_INTERVAL_MS="${TAKO_PROMO_REC_INTERVAL_MS:-100}" \
        AGENT_SIM_BEATS="$REC/beats.tsv" \
        AGENT_SIM_DIR="$DEMO_SRC/personas" \
        DEMO_DIR="$DEMO_SRC" \
        "$@" || exit $?
    GUI_PID=$ISOLATED_GUI_PID
    wait_isolated_gui "$log" || die "隔離 GUI へ繋がらない"
    say "隔離 GUI を起動: pid=${GUI_PID} 面=tako-vd 窓=${WIN_W}x${WIN_H}pt"
}

# 隔離 GUI へだけ届く CLI（iso_cli）。台帳や設定を手元で読み書きするコマンドのために、
# GUI と同じデモ用の HOME と data dir も渡す（渡さないと「本番の置き場は使わない」と断られる）
cli() {
    HOME="$DEMO_HOME" TAKO_DATA_DIR="$SB/data" iso_cli "$@"
}

# 窓だけを閉じる（永続化の場面: tmux の中のプロセスは残す）
gui_quit_keep_sessions() {
    local i
    kill "$GUI_PID" 2>/dev/null || true
    for i in $(seq 1 80); do
        kill -0 "$GUI_PID" 2>/dev/null || break
        sleep 0.1
    done
    wait "$GUI_PID" 2>/dev/null || true
    GUI_PID=""
    ISOLATED_GUI_PID=""
}

# いま前面のタブで、フォーカスのあるペインの ID
active_pane() {
    cli list | node -e '
        let s = ""; process.stdin.on("data", (d) => (s += d)).on("end", () => {
            const d = JSON.parse(s);
            const tab = (d.tabs || []).find((t) => t.id === d.active_tab) || {};
            let found = "";
            const walk = (n) => {
                if (!n || typeof n !== "object") return;
                if (Array.isArray(n)) return n.forEach(walk);
                if (n.focused === true && typeof n.id === "number") found = found || String(n.id);
                Object.values(n).forEach(walk);
            };
            walk(tab);
            process.stdout.write(found || String(tab.focused_pane || tab.active_pane || ""));
        });'
}

# ---- 収録の区切り -------------------------------------------------------------

SCENE=""
SCENE_T0=0

rec_beat() {
    printf 'rec:%s\t%s\n' "$1" "$(now_ms)" >> "$REC/beats.tsv"
}

wait_beat() {
    # $1 = 目印（<persona>:<名前>）・$2 = 上限秒
    local name="$1" limit="${2:-60}" i
    for i in $(seq 1 $((limit * 5))); do
        grep -q "^${name}	" "$REC/beats.tsv" 2>/dev/null && return 0
        sleep 0.2
    done
    die "目印 ${name} が ${limit} 秒以内に出なかった（場面 ${SCENE}）"
}

rec_start() {
    SCENE="$1"
    command rm -f "${REC:?}"/f-*.rgba
    SCENE_T0=$(now_ms)
    : > "$REC/REC"
    say "RECORDING START ${SCENE} t0=${SCENE_T0}"
}

rec_stop() {
    local dir="$OUT/scenes/$SCENE" t1 n size f ms i=0
    command rm -f "${REC:?}/REC"
    sleep 0.8
    t1=$(now_ms)
    mkdir -p "$dir" || die "出力先を作れない: $dir"
    command rm -f "${dir:?}"/*.jpg "${dir:?}/frames.tsv" "${dir:?}/beats.tsv"
    n=$(command ls "$REC" | grep -c '\.rgba$')
    [ "$n" -gt 0 ] || die "場面 ${SCENE}: 1 枚も撮れていない（promo-rec.patch を当てて建てたか確かめる）"
    size=$(command ls "$REC" | grep '\.rgba$' | sed 's/.*-\([0-9]*x[0-9]*\)\.rgba$/\1/' | sort -u)
    [ "$(printf '%s\n' "$size" | wc -l | tr -d ' ')" = 1 ] || die "場面 ${SCENE}: 途中で窓の寸法が変わった（${size}）"
    : > "$dir/frames.tsv"
    while IFS= read -r f; do
        i=$((i + 1))
        ms=${f#f-}
        ms=${ms%%-*}
        printf '%d\t%d\n' "$i" "$((ms - SCENE_T0))" >> "$dir/frames.tsv"
    done <<EOF
$(command ls "$REC" | grep '\.rgba$' | sort)
EOF
    (cd "$REC" && command ls | grep '\.rgba$' | sort | xargs cat) |
        ffmpeg -nostdin -v error -y -f rawvideo -pix_fmt rgba -s "$size" -r 10 -i - -q:v 2 "$dir/%06d.jpg" ||
        die "場面 ${SCENE}: フレームを JPEG にできない"
    awk -F '\t' -v t0="$SCENE_T0" -v t1="$t1" '$2 >= t0 && $2 <= t1 { printf "%s\t%d\n", $1, $2 - t0 }' "$REC/beats.tsv" > "$dir/beats.tsv"
    printf '%s\t%s\t%s\n' "$SCENE" "$size" "$((t1 - SCENE_T0))" > "$dir/scene.tsv"
    command rm -f "${REC:?}"/f-*.rgba
    say "RECORDING END ${SCENE} frames=${n} size=${size} dur_ms=$((t1 - SCENE_T0)) → ${dir}"
}

# ---- 場面 -------------------------------------------------------------------

prepare_tab() {
    cli welcome dismiss > /dev/null 2>&1
    cli theme dark --size "${TAKO_PROMO_FONT:-14}" > /dev/null 2>&1
    cli tab rename --tab 1 shop-app > /dev/null 2>&1
}

# 場面 1: 司令塔が担当 3 体を同じタブに立て、監視して検収する（本物の tako master / orchestrator spawn）
scene_fleet() {
    cli orchestrator projects add --key shop-app --cwd "$DEMO_HOME/shop-app" --description "デモ用のショップ API" > /dev/null ||
        die "プロジェクトを登録できない"
    cli orchestrator layout --min-worker-cols 30 > /dev/null
    cli send --pane 1 "cd ~/shop-app && clear" > /dev/null
    sleep 1
    rec_start fleet
    sleep 1.2
    # コマンドを打った所が見えるよう、打ってから少しおいて Enter を押す
    rec_beat type_master
    cli send --pane 1 --no-newline "tako master" > /dev/null
    sleep 1.4
    cli send --pane 1 --key Enter > /dev/null
    wait_beat master:ready 40
    sleep 1.5
    rec_beat ask
    cli send --pane 1 "ショップアプリに「決済リトライ」「商品検索」「README 更新」を入れて" > /dev/null
    wait_beat master:report 150
    sleep 3.5
    rec_stop
    cli panel --hide > /dev/null 2>&1
}

# 場面 2: エージェントが CLI で画面を動かす（分割・プレビュー・テーマ）
scene_drive() {
    local pane
    cli tab new --title "shop-app" --focus --cwd "$DEMO_HOME/shop-app" > /dev/null || die "タブを作れない"
    sleep 1
    pane=$(active_pane)
    [ -n "$pane" ] || die "新しいタブのペインが分からない"
    cli send --pane "$pane" "clear && AGENT_SIM_PERSONA=guide claude" > /dev/null
    wait_beat guide:ready 30
    sleep 1
    rec_start drive
    sleep 1.2
    rec_beat ask1
    cli send --pane "$pane" "dev サーバーを起動して、README を見せて" > /dev/null
    wait_beat guide:done1 60
    sleep 3
    rec_beat ask2
    cli send --pane "$pane" "ライトテーマにして" > /dev/null
    wait_beat guide:done2 40
    sleep 3
    rec_stop
    cli theme dark > /dev/null 2>&1
    sleep 1
}

# 場面 3: 見る・直す道具（git パネル / ファイルツリーとコード / かんたん表示のボタン）。静止画として使う
scene_extras() {
    local pane
    cli tab new --title "shop-app" --focus --cwd "$DEMO_HOME/shop-app" > /dev/null || die "タブを作れない"
    sleep 1
    cli send --pane "$(active_pane)" "clear && git status --short && git log --oneline -5" > /dev/null
    cli panel --show --view git --width 520 > /dev/null 2>&1
    sleep 2
    rec_start extras-git
    sleep 2.5
    rec_stop
    cli panel --hide > /dev/null 2>&1
    cli tree add "$DEMO_HOME/shop-app" > /dev/null 2>&1
    cli panel --filetree on > /dev/null 2>&1
    # コードを右に開いてから元のシェルのペインを閉じる（ツリーとコードが隣り合う絵にする）
    pane=$(active_pane)
    cli open "$DEMO_HOME/shop-app/src/api/payments.ts" --pane "$pane" --right --focus > /dev/null 2>&1
    sleep 1.5
    cli close --pane "$pane" > /dev/null 2>&1
    sleep 2
    rec_start extras-code
    sleep 2.5
    rec_stop
    cli panel --filetree off > /dev/null 2>&1
    cli tab new --title "はじめて" --focus --cwd "$DEMO_HOME" > /dev/null
    sleep 1
    cli ui-mode gui > /dev/null 2>&1
    sleep 1.5
    rec_start extras-starter
    sleep 2.5
    rec_stop
    cli ui-mode terminal > /dev/null 2>&1
}

# 場面 4: 閉じても続いている（tmux バックエンド。窓を閉じて開き直す）
scene_restore() {
    local p2 p3
    cli send --pane 1 "cd ~/shop-app && clear && AGENT_SIM_PERSONA=refactor claude" > /dev/null
    wait_beat refactor:ready 30
    p2=$(cli split --pane 1 --right --cwd "$DEMO_HOME/shop-app" -- node "$DEMO_SRC/dev-server.mjs" | tr -dc '0-9')
    [ -n "$p2" ] || die "dev サーバーのペインを作れない"
    p3=$(cli split --pane "$p2" --down --cwd "$DEMO_HOME/shop-app" -- node "$DEMO_SRC/test-watch.mjs" | tr -dc '0-9')
    [ -n "$p3" ] || die "テストのペインを作れない"
    sleep 1
    cli send --pane 1 "API まわりを新しい書き方にそろえて" > /dev/null
    wait_beat refactor:start 30
    sleep 2
    rec_start restore-before
    sleep 7
    rec_stop
    rec_beat quit
    gui_quit_keep_sessions
    say "窓を閉じた（tmux の中のプロセスは動いたまま）。5 秒おいて開き直す"
    sleep 5
    gui_up restore-after TAKO_PERSIST=1 TAKO_TMUX_SOCKET="$SOCK"
    sleep 1.5
    rec_start restore-after
    sleep 9
    rec_stop
}

# ---- 本体 -------------------------------------------------------------------

command -v ffmpeg > /dev/null || die "ffmpeg が見つからない"

scenes="$*"
[ -n "$scenes" ] || scenes="fleet drive extras restore"

make_sandbox
mkdir -p "$OUT/scenes" || die "出力先を作れない: $OUT"
# 蓋を閉じた運用では tako-vd がすぐ眠るので、収録の間だけ起こしておく
caffeinate -d -u -t 3600 &
CAFF_PID=$!
isolated_gui_bins visual-test || die "収録用のバイナリを建てられない"
# 読み戻しの口（promo-rec.patch）が入ったバイナリか。入っていないと 1 枚も撮れないので、窓を開く前に止める
grep -q -a 'TAKO_PROMO_REC_DIR' "$APP_BIN" ||
    die "収録用の差分が入っていないバイナリ: ${APP_BIN}（先に git apply scripts/promo/remake/promo-rec.patch）"

first=""
for s in $scenes; do
    case "$s" in
        fleet | drive | extras)
            if [ -z "$first" ]; then
                gui_up main
                prepare_tab
                first=1
            fi
            "scene_$s"
            ;;
        restore) ;;
        *) die "知らない場面: ${s}（fleet / drive / extras / restore）" ;;
    esac
done
case " $scenes " in
    *" restore "*)
        if [ -n "$GUI_PID" ]; then
            stop_isolated_gui "$GUI_PID"
            GUI_PID=""
        fi
        command rm -rf "${SB:?}/data"
        mkdir -p "$SB/data"
        gui_up restore-before TAKO_PERSIST=1 TAKO_TMUX_SOCKET="$SOCK"
        prepare_tab
        scene_restore
        ;;
esac
say "収録おわり: $OUT/scenes"
