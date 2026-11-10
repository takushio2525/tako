#!/bin/bash
# record.sh — 入れ方・使い方動画（v2 = #2886。v1 は #2866）の実画面の収録。
#
# 収録の仕組み（砂場・隔離 GUI・読み戻し・連番の JPEG と時刻の表・片付け）は、プロモーション動画
# （#2864）の scripts/promo/remake/record.sh の関数をそのまま読み込んで使う（複製しない）。
# ここに書くのは、この動画の砂場の差分と場面だけ。台本と根拠は .agent/plans/2026-11-howto-video.md。
#
# 撮る流れ（#2879 の後の初回起動 = ボタン → setup → 対話 → master。段の正本は `tako welcome`）:
#   firstrun  起動しただけの画面。案内が出て、何も走らない（段 = setup）
#   guided    「セットアップを実行」→ setup のタブで tako setup → 対話のセットアップ → 閉じる →「master を起動」
#   update    tako update check（公開のリリース一覧を読むだけ）
#   master    tako master → 依頼 → 担当が並ぶ → 報告
#   noagent   Claude Code がまだ無い砂場: setup の導入の同意 →「ログイン」→「セットアップを続ける」
#
# 守っていること:
#   - 窓は tako-vd にだけ出す（立て方は scripts/lib/isolated-gui.sh の 1 実装）。本番の tako には触らない
#   - 本物の claude / codex / agy は起こさない。砂場の ~/.local/bin に台（demo/claude-stub.mjs）を置く
#   - GitHub へも出ない。砂場の PATH に gh の台（demo/gh-stub.sh）を置き、リポジトリの作成は砂場の中の
#     空のリポジトリで受ける（git の insteadOf で https://github.com/demo/ を砂場へ向ける）
#   - 本物のインストーラも brew も走らせない（curl と brew は砂場の台。呼ばれた回数を控えて最後に出す）
#   - `tako setup` は砂場の HOME にだけ書く。収録の前後で本物の HOME の設定と /Applications/tako.app が
#     変わっていないことを自分で確かめ、変わっていたら終了コード 3 で終わる
#   - 画面に出る HOME・プロンプト・パス・プロジェクトはすべてデモ用。tako の置き場も砂場の HOME の下の
#     既定と同じ形（~/Library/Application Support/tako）にして、実機と同じ見え方にする
#
# 使い方:
#   git apply scripts/promo/remake/promo-rec.patch      # 収録用の読み戻し（#2864 の差分）を入れて建てる
#   bash scripts/promo/howto/record.sh [場面…]           # 場面を省くと全部（firstrun guided update master noagent）
#   git checkout crates/tako-app/src/main.rs
#   建て済みの収録用バイナリを使い回すなら TAKO_BIN / APP_BIN で置き場を渡す（その場合は建て直さない）
#   bash scripts/promo/howto/record.sh hold              # 収録せずに立てたまま止める（<砂場>/STOP を置くと片付けて終わる）
#   bash scripts/promo/howto/record.sh hold-noagent      # 同じく、Claude Code が無い砂場で
#   出力は TAKO_PROMO_OUT（既定 ~/Movies/tako-promo/howto-v2）の scenes/<場面>/
#   #2864 の一式の置き場は TAKO_HOWTO_REMAKE_DIR（既定は同じ木の scripts/promo/remake）

set -uo pipefail

HOWTO=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
REMAKE=${TAKO_HOWTO_REMAKE_DIR:-$HOWTO/../remake}
[ -f "$REMAKE/record.sh" ] || {
    printf 'ERROR: %s\n' "#2864 の収録の一式が無い: ${REMAKE}（TAKO_HOWTO_REMAKE_DIR で置き場を渡す）" >&2
    exit 1
}
REMAKE=$(cd "$REMAKE" && pwd)

# 出力先と砂場はこの動画のもの（#2864 の収録とも、v1 の収録 ~/Movies/tako-promo/howto とも混ざらない名前にする）。
# 砂場の名前は画面のパスに出るので、プロセス番号を入れない固定の名前にする
TAKO_PROMO_OUT=${TAKO_PROMO_OUT:-$HOME/Movies/tako-promo/howto-v2}
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
DEMO_SRC="$REMAKE/demo"   # デモ用プロジェクト（shop-app）と担当の台本は #2864 のもの
SOCK="tkh-$$"
DEMO="$SB/demo"           # 台（claude / gh）と偽のエージェントの写し。砂場から起こす
PERSONAS="$SB/personas"   # 台本の置き場（この動画の台本 + #2864 の担当の台本を砂場で 1 つにまとめる）
GUARD="$SB/guard"
CALLS="$SB/log/outside-calls.log"   # 砂場の台（gh / brew / curl）が呼ばれた記録
MAIN_HOME="$DEMO_HOME"                # Claude Code が入っていてログイン済みの人の HOME
NOAGENT_HOME="$SB/home2/demo"         # Claude Code がまだ無い人の HOME（画面にはどちらも ~ と出る）
DATA=""                               # use_home が決める
FAST=""                               # 撮らずに同じ流れを通すとき（読ませる間を詰める）
GUARD_KEEP=guard                      # 前後比較の控えを残す先（出力先の下）

# HOME を選ぶ。tako の置き場は実機の既定と同じ形（画面には ~/Library/… と出る）
use_home() {
    DEMO_HOME="$1"
    DATA="$DEMO_HOME/Library/Application Support/tako"
}

# 隔離 GUI へだけ届く CLI。台帳や設定を手元で読み書きするコマンドのために、GUI と同じ HOME と data dir を渡す
cli() {
    HOME="$DEMO_HOME" TAKO_DATA_DIR="$DATA" iso_cli "$@"
}

# 読ませる間。撮らずに通すときは詰める
pause() {
    if [ -n "$FAST" ]; then sleep 0.3; else sleep "$1"; fi
}

# ---- 本物の HOME と /Applications が変わっていないことの確かめ -------------------

# 利用者のホーム（HOME は砂場へ差し替えるので、パスワードの登録から引く）
real_home() {
    iso_real_home 2> /dev/null || printf '%s' "$HOME"
}

# 控えを 1 枚書く。tako setup と対話のセットアップが書きうる先だけを見る
# （シェルの設定・エージェントの設定と MCP 登録・指示ファイル・tako CLI のリンク・tako の設定・設定共有の
# リポジトリ・git と gh の設定・アプリ本体）。
# ~/.claude.json は動いている claude が常に書き換えるので、tako の MCP 登録の欄だけを比べる
guard_snapshot() {
    local out="$1" home p
    home=$(real_home)
    : > "$out"
    for p in .zshrc .zprofile .zshenv .zlogin .bash_profile .bash_login .profile \
        .claude/CLAUDE.md .claude/settings.json .claude/skills/tako \
        .codex/config.toml .codex/AGENTS.md .gemini/GEMINI.md .gemini/config/mcp_config.json \
        .local/bin/tako .local/bin/claude \
        "Library/Application Support/tako/config.yaml" "Library/Application Support/tako/settings.json" \
        "Library/Application Support/tako/setup" "Library/Application Support/tako/profiles" \
        "Library/Application Support/tako/solo-profiles" "Library/Application Support/tako/orchestrator/projects.yaml" \
        "Library/Application Support/tako/orchestrator/config.yaml" \
        "Library/Application Support/tako/orchestrator/profiles/default.yaml" \
        "Library/Application Support/tako/config-share.json" \
        .gitconfig .config/gh/hosts.yml tako-config-sync \
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
        # 控えは出力先へ残す。撮らずに立てただけの回（hold）は、収録の回の控えを上書きしないよう別の置き場へ
        mkdir -p "$OUT/$GUARD_KEEP" && command cp -f "$GUARD"/*.txt "$OUT/$GUARD_KEEP/" 2> /dev/null
        # 砂場の台が外へ出る代わりに受けた呼び出し（本物の gh / brew / インストーラは 1 回も走っていない）
        if [ -s "$CALLS" ]; then
            command cp -f "$CALLS" "$OUT/$GUARD_KEEP/outside-calls.log"
            say "砂場の台が受けた呼び出し: $(wc -l < "$CALLS" | tr -d ' ') 回（$OUT/$GUARD_KEEP/outside-calls.log）"
        fi
    fi
    cleanup
    [ "$rc" = 0 ] || exit 3
}
trap howto_cleanup EXIT

# ---- 砂場（#2864 の砂場に、この動画の差分を足す） --------------------------------

# claude の台を起こす 2 行（本物のインストーラと同じ置き場 ~/.local/bin に置く）
claude_launcher() {
    printf '%s\n' '#!/bin/sh' "exec \"$SB/bin/node\" \"$DEMO/claude-stub.mjs\" \"\$@\""
}

make_howto_sandbox() {
    local node
    make_sandbox
    node=$(command -v node) || die "node が見つからない"
    mkdir -p "$GUARD" "$PERSONAS" "$DEMO" "$SB/app" "$SB/remotes" "$SB/bin-noagent" "$NOAGENT_HOME" "$MAIN_HOME/.local/bin" ||
        die "砂場の置き場を作れない"
    : > "$CALLS"
    # 台と偽のエージェントは砂場へ写してから起こす（プロセスの引数や PATH に利用者のホームのパスを出さない）
    command cp -f "$HOWTO/demo/claude-stub.mjs" "$HOWTO/demo/gh-stub.sh" "$HOWTO/demo/setup-facts.mjs" "$DEMO_SRC/agent-sim.mjs" "$DEMO/" ||
        die "台を砂場へ写せない"
    # 台本: 担当（api / docs）は #2864 のものをそのまま使い、対話のセットアップと司令塔はこの動画のもの
    command cp -f "$DEMO_SRC/personas/api.json" "$DEMO_SRC/personas/docs.json" "$PERSONAS/" || die "#2864 の担当の台本が無い"
    command cp -f "$HOWTO"/demo/personas/*.json "$PERSONAS/" || die "この動画の台本が無い"
    # #2864 の偽物 3 つは外す。node は砂場の bin から引く（利用者のホームの下の置き場を PATH へ載せない）
    command rm -f "${SB:?}/bin/claude" "${SB:?}/bin/codex" "${SB:?}/bin/agy"
    ln -s "$node" "$SB/bin/node" || die "node を砂場へ繋げない"
    # gh の台: GitHub へ出ない。リポジトリの作成は砂場の中の空のリポジトリで受ける
    printf '%s\n' '#!/bin/sh' "DEMO_REMOTES=\"$SB/remotes\" DEMO_CALLS=\"$CALLS\" exec /bin/sh \"$DEMO/gh-stub.sh\" \"\$@\"" > "$SB/bin/gh"
    # brew の台: 何も入れない（呼ばれたら記録して断る）。本物の brew を砂場から走らせないための蓋
    printf '%s\n' '#!/bin/sh' "echo \"brew \$*\" >> \"$CALLS\"" 'echo "デモ用の環境です。brew は実行しません" >&2' 'exit 1' > "$SB/bin/brew"
    chmod +x "$SB/bin/gh" "$SB/bin/brew"
    # 設定共有の push を砂場で受ける: https://github.com/demo/<名前> を砂場の中のリポジトリへ読み替える
    printf '%s\n' "[url \"file://$SB/remotes/\"]" '    insteadOf = https://github.com/demo/' >> "$MAIN_HOME/.gitconfig"
    # この動画の主な前提は「Claude Code が入っていてログイン済み」
    claude_launcher > "$MAIN_HOME/.local/bin/claude"
    chmod +x "$MAIN_HOME/.local/bin/claude"
    printf 'demo\n' > "$MAIN_HOME/.claude-demo-login"
    # 作業中の差分は無しで始める（master に頼む前のきれいな状態）
    (cd "$MAIN_HOME/shop-app" && HOME="$MAIN_HOME" GIT_CONFIG_NOSYSTEM=1 git checkout -q -- .) || die "デモ用プロジェクトを戻せない"

    # 「Claude Code がまだ無い人」の HOME: プロンプトだけを置く。claude は偽のインストーラが置く。
    # macOS のログイン zsh は /etc/zprofile で PATH を並べ替えて /usr/bin を前へ出す。それだと下の curl の台より
    # 本物の curl が先に見つかる（= 本物のインストーラを取りに行く）ので、この HOME だけ全体の rc を読ませない
    command cp -f "$MAIN_HOME/.zshrc" "$NOAGENT_HOME/.zshrc" || die "プロンプトを写せない"
    printf '%s\n' 'unsetopt GLOBAL_RCS' > "$NOAGENT_HOME/.zshenv"
    # curl の台（この場面の PATH にだけ載せる）: 公式インストーラを取りに行かず、`-o <置き先>` へ
    # 偽のインストーラ（claude の台を未ログインで置くだけ）を書く
    claude_launcher > "$SB/demo/claude-launcher"
    {
        printf '%s\n' '#!/bin/sh' "echo \"curl \$*\" >> \"$CALLS\"" 'dest=""'
        printf '%s\n' 'while [ $# -gt 0 ]; do case "$1" in -o) dest="$2"; shift 2 ;; *) shift ;; esac; done' '[ -n "$dest" ] || exit 2'
        printf '%s\n' "cat > \"\$dest\" << 'INSTALLER'" '#!/bin/bash' 'mkdir -p "$HOME/.local/bin"'
        printf '%s\n' "command cp -f \"$SB/demo/claude-launcher\" \"\$HOME/.local/bin/claude\"" 'chmod +x "$HOME/.local/bin/claude"'
        printf '%s\n' 'echo "Claude Code successfully installed!"' 'echo "（デモ用の環境です。本物のインストーラは実行していません）"' 'INSTALLER' 'exit 0'
    } > "$SB/bin-noagent/curl"
    chmod +x "$SB/bin-noagent/curl"
}

# 収録に使うバイナリを砂場へ写して、そこから起こす（置き場のパスが PATH やリンク先として画面へ出ても、
# 利用者のホームのパスにならない）
stage_bins() {
    command cp -f "$TAKO_BIN" "$SB/app/tako" && command cp -f "$APP_BIN" "$SB/app/tako-app" || die "バイナリを砂場へ写せない"
    TAKO_BIN="$SB/app/tako"
    APP_BIN="$SB/app/tako-app"
}

# 隔離 GUI へ渡す PATH（$1 = 先頭へ足す置き場。無ければ空）
gui_path() {
    printf '%s' "${1:+$1:}$DEMO_HOME/.local/bin:$SB/app:$SB/bin:/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin"
}

# 隔離 GUI。#2864 の gui_up に、この動画の置き場・台本・窓の位置・台の設定を足す（後ろに並べた値が勝つ）。
# 初回起動の導線は検証用 GUI では既定で動かない（HOME へ書くため）ので、HOME を砂場へ向けたうえで明示する
#   $1 = ログの名前・$2 = PATH の先頭へ足す置き場（無ければ空）・残りは GUI へ足す VAR=VAL
howto_gui_up() {
    local log="$1" extra_path="${2:-}"
    shift
    [ $# -gt 0 ] && shift
    gui_up "$log" \
        HOME="$DEMO_HOME" \
        SHELL=/bin/zsh \
        TAKO_DATA_DIR="$DATA" \
        PATH="$(gui_path "$extra_path")" \
        TAKO_WINDOW_BOUNDS="${HOWTO_WIN_X},${HOWTO_WIN_Y},${WIN_W},${WIN_H}" \
        TAKO_FIRST_RUN=1 \
        TAKO_TMUX_SOCKET="$SOCK" \
        AGENT_SIM_DIR="$PERSONAS" \
        AGENT_SIM_MJS="$DEMO/agent-sim.mjs" \
        DEMO_DIR="$DEMO_SRC" \
        DEMO_FACTS="$DEMO/setup-facts.mjs" \
        CLAUDE_STUB_BOOT_MS="${TAKO_HOWTO_BOOT_MS:-2500}" \
        CLAUDE_STUB_SETUP_BOOT_MS="${TAKO_HOWTO_SETUP_BOOT_MS:-9000}" \
        CLAUDE_STUB_LOG="$SB/log/claude-stub.tsv" \
        ${@+"$@"}
}

# JSON（標準入力）の 1 項目（a.b.c）を読む。無ければ空
jget() {
    "$SB/bin/node" -e '
        let s = ""; process.stdin.on("data", (d) => (s += d)).on("end", () => {
            let v; try { v = JSON.parse(s); } catch { return; }
            for (const k of process.argv[1].split(".")) v = v && typeof v === "object" ? v[k] : undefined;
            if (v !== undefined && v !== null) process.stdout.write(typeof v === "object" ? JSON.stringify(v) : String(v));
        });' "$1"
}

# 案内の段（`tako welcome` が返す。バナーが読んでいるのと同じ値）が kind（と phase）になるまで待つ
wait_step() {
    local kind="$1" phase="${2:-}" limit="${3:-90}" i state=""
    for i in $(seq 1 $((limit * 3))); do
        state=$(cli welcome 2> /dev/null)
        if [ "$(printf '%s' "$state" | jget onboarding.state.step.kind)" = "$kind" ] &&
            { [ -z "$phase" ] || [ "$(printf '%s' "$state" | jget onboarding.state.step.phase)" = "$phase" ]; }; then
            return 0
        fi
        sleep 0.3
    done
    die "案内の段が ${limit} 秒以内に ${kind}${phase:+ / $phase} にならなかった（場面 ${SCENE}）: $(printf '%s' "$state" | head -c 600)"
}

# 導線のタブ（題 setup）のペイン ID
setup_pane() {
    cli list 2> /dev/null | "$SB/bin/node" -e '
        let s = ""; process.stdin.on("data", (d) => (s += d)).on("end", () => {
            let d; try { d = JSON.parse(s); } catch { return; }
            const tab = (d.tabs || []).find((t) => t.title === "setup");
            const pane = tab && (tab.panes || [])[0];
            if (pane) process.stdout.write(String(pane.id));
        });'
}

wait_setup_pane() {
    local i pane=""
    for i in $(seq 1 100); do
        pane=$(setup_pane)
        [ -n "$pane" ] && { printf '%s' "$pane"; return 0; }
        sleep 0.2
    done
    return 1
}

# ペインの画面に文字列が出るまで待つ
wait_screen() {
    local pane="$1" text="$2" limit="${3:-60}" i
    for i in $(seq 1 $((limit * 4))); do
        case "$(cli read --pane "$pane" 2> /dev/null)" in *"$text"*) return 0 ;; esac
        sleep 0.25
    done
    die "ペイン ${pane} に「${text}」が ${limit} 秒以内に出なかった（場面 ${SCENE}）"
}

# 打った所が見えるよう、打ってから少しおいて Enter を押す
type_and_enter() {
    local pane="$1" text="$2" hold="${3:-1.4}"
    cli send --pane "$pane" --no-newline "$text" > /dev/null
    pause "$hold"
    cli send --pane "$pane" --key Enter > /dev/null
}

# ---- 場面 -------------------------------------------------------------------

# 場面 1: 初回起動。タブバーの下に案内が出る。起動しただけでは何も走らない（段 = setup。#2879）
scene_firstrun() {
    use_home "$MAIN_HOME"
    rec_start firstrun
    howto_gui_up firstrun
    rec_beat window
    wait_step setup
    rec_beat banner
    sleep 6
    # 何も走っていない・書いていないこと（押す前の HOME は置いたものだけ。tako の置き場に設定が無い）
    [ ! -e "$DATA/setup" ] && [ ! -e "$DATA/orchestrator/config.yaml" ] && [ ! -e "$DEMO_HOME/.claude" ] ||
        die "ボタンを押す前に setup が走った（起動しただけで設定が書かれている）"
    rec_stop
}

# 「セットアップを実行」→ setup のタブで tako setup → 対話のセットアップ → 閉じる →「master を起動」。
# ボタンは `tako welcome next` で押す（GUI のボタンと同じ要求）。対話の問いと答えの順は demo/personas/setup.json
guided_flow() {
    local pane
    pause 2.0
    rec_beat press
    cli welcome next > /dev/null || die "「セットアップを実行」を押せない"
    pane=$(wait_setup_pane) || die "setup のタブが開かない"
    # tako setup の出力は 1 秒ほどで流れ切る。終わった所（結果の一覧）を読み、遡って頭（検出・登録）を読み、戻る
    wait_screen "$pane" "対話アシスタントを起動します" 90
    rec_beat setup_done
    pause 2.4
    cli scroll --pane "$pane" --to 200 > /dev/null
    rec_beat setup_top
    pause 3.2
    cli scroll --pane "$pane" --to 0 > /dev/null
    rec_beat setup_bottom
    # 対話のセットアップ。1 通目（tako setup が渡す最初のメッセージ）で、現状の読み取り → Step 1 → Step 2 の問い
    wait_beat setup:ask_orch 90
    pause 6.5
    rec_beat answer_orch
    type_and_enter "$pane" "利用回数を節約したい"
    wait_beat setup:propose_orch 60
    pause 4.5
    rec_beat agree_orch
    type_and_enter "$pane" "はい" 1.0
    wait_beat setup:ask_project 60
    pause 4.5
    rec_beat answer_project
    type_and_enter "$pane" "~/shop-app を登録して"
    wait_beat setup:ask_share 60
    pause 8.0
    rec_beat answer_share
    type_and_enter "$pane" "お願いします。名前はそのままで"
    wait_beat setup:summary 90
    case "$(cli read --pane "$pane" 2> /dev/null)" in
        *'${'*) die "対話の画面に、値の入っていない欄（\${…}）が出ている（台本の変数名と setup-facts.mjs の欄を確かめる）" ;;
    esac
    pause 7.0
    # 対話を閉じる（本物の Claude Code と同じ /exit）。setup が終わり、案内が「master を起動」へ進む
    rec_beat exit
    type_and_enter "$pane" "/exit" 1.0
    wait_step master "" 60
    rec_beat master_step
    pause 6
}

# 対話で決めたことが、砂場の設定へ本当に入っていること（台本の文ではなく、本物のコマンドの結果を見る）
verify_guided() {
    local profile="$DATA/orchestrator/profiles/default.yaml" worker projects
    # 読み切ってから見る（pipefail の下で grep -q へ直にパイプすると、前段の SIGPIPE で偽の失敗になる）
    worker=$(grep -A 1 '^  claude:' "$profile" 2> /dev/null)
    grep -q 'effort: medium' <<< "$worker" || die "対話の結果: worker の effort が medium になっていない（${profile}）: ${worker}"
    projects=$(cli orchestrator projects list 2> /dev/null)
    grep -q '^shop-app ' <<< "$projects" || die "対話の結果: shop-app が登録されていない: ${projects}"
    [ "$(cli config status --json 2> /dev/null | jget linked)" = true ] || die "対話の結果: 設定共有が配線されていない"
    [ -n "$(git --git-dir "$SB/remotes/tako-config-sync" rev-parse -q --verify HEAD 2> /dev/null)" ] ||
        die "対話の結果: 設定が砂場の中のリポジトリへ push されていない"
    grep -q 'dialogue_done: true' "$DATA/orchestrator/config.yaml" || die "対話が済んだ印（setup.dialogue_done）が立っていない"
    [ "$(grep -c '^gh repo create ' "$CALLS")" = 1 ] || die "gh repo create が 1 回ではない"
    grep -q -e '^brew ' -e '^curl ' "$CALLS" && die "この流れで brew / curl が呼ばれた"
    say "対話の結果を確認: worker の effort = medium・shop-app を登録・設定共有を配線（砂場の中のリポジトリへ push）・dialogue_done"
}

# 場面 2
scene_guided() {
    rec_start guided
    guided_flow
    rec_stop
    verify_guided
}

# 場面 3: 更新の確認（tako update check。公開のリリース一覧を読むだけで、何も入れない）
scene_update() {
    rec_start update
    sleep 1.0
    rec_beat type
    type_and_enter 1 "tako update check"
    rec_beat enter
    sleep 6
    rec_stop
    cli send --pane 1 "clear" > /dev/null
    sleep 1
}

# 場面 4: master を起動して頼む（本物の tako master / orchestrator spawn）。
# プロジェクトは対話のセットアップで登録済みなので、依頼から始める
scene_master() {
    cli orchestrator layout --min-worker-cols 30 > /dev/null
    rec_start master
    sleep 1.0
    rec_beat type_master
    type_and_enter 1 "tako master"
    wait_beat master:ready 40
    sleep 3.0
    rec_beat ask_work
    cli send --pane 1 "決済 API にリトライを入れて、README も更新して" > /dev/null
    wait_beat master:report 150
    sleep 5
    rec_stop
}

# Claude Code がまだ無い人の砂場を立てる。立てる前に、導入の段が見つける curl が砂場の台であることを確かめる
# （tako はログインシェルの `command -v` で探す。同じ聞き方で先に聞く）
noagent_up() {
    local got
    use_home "$NOAGENT_HOME"
    got=$(env -i HOME="$DEMO_HOME" PATH="$(gui_path "$SB/bin-noagent")" /bin/zsh -l -c 'command -v curl' 2> /dev/null)
    [ "$got" = "$SB/bin-noagent/curl" ] || die "curl の台が先に見つからない（見つかったのは ${got:-なし}）。本物のインストーラを走らせないために止める"
    howto_gui_up noagent "$SB/bin-noagent"
    wait_step setup
}

# 場面 5: Claude Code がまだ無い人。setup が導入の同意を聞き、案内が「ログイン」→「セットアップを続ける」と進む。
# インストーラは砂場の台（本物は取りに行かない）、ログインも台（ブラウザは開かない）
scene_noagent() {
    local pane
    noagent_up
    sleep 2
    rec_start noagent
    sleep 1.5
    rec_beat press
    cli welcome next > /dev/null || die "「セットアップを実行」を押せない"
    pane=$(wait_setup_pane) || die "setup のタブが開かない"
    wait_screen "$pane" "インストールしますか" 60
    rec_beat ask_install
    sleep 5
    rec_beat yes
    type_and_enter "$pane" "Y" 1.0
    wait_step login "" 90
    rec_beat login_step
    sleep 6
    rec_beat press_login
    cli welcome next > /dev/null || die "「ログイン」を押せない"
    wait_step continue "" 90
    rec_beat continue_step
    sleep 6
    rec_stop
    # 走ったのは砂場の台だけ（公式インストーラの取得は台が 1 回受けた・置かれた claude は台）
    [ "$(grep -c '^curl ' "$CALLS")" = 1 ] || die "curl の台が受けた回数が 1 回ではない"
    grep -q 'claude-stub.mjs' "$NOAGENT_HOME/.local/bin/claude" || die "置かれた claude が台ではない"
    say "導入の場面を確認: インストーラの取得は砂場の台が 1 回受けた・置かれた claude は台・ログインは台の印"
}

# 収録せずに立てたまま止める（画面や CLI の振る舞いを確かめる用）。<砂場>/STOP を置くと片付けて終わる
hold() {
    local i
    say "HOLD: 砂場=${SB} 発見の置き場=${ISOLATED_GUI_DISCOVERY_DIR} GUI=${GUI_PID}"
    printf '%s\n' "SB=$SB" "DISCOVERY=$ISOLATED_GUI_DISCOVERY_DIR" "GUI_PID=$GUI_PID" "DEMO_HOME=$DEMO_HOME" "PATH=$1" > "$SB/hold.env"
    for i in $(seq 1 7200); do
        [ -e "$SB/STOP" ] && break
        kill -0 "$GUI_PID" 2> /dev/null || break
        sleep 1
    done
}

stop_gui() {
    [ -n "$GUI_PID" ] || return 0
    stop_isolated_gui "$GUI_PID"
    GUI_PID=""
}

# ---- 本体 -------------------------------------------------------------------

command -v ffmpeg > /dev/null || die "ffmpeg が見つからない"

scenes="$*"
[ -n "$scenes" ] || scenes="firstrun guided update master noagent"
for s in $scenes; do
    case "$s" in
        firstrun | guided | update | master | noagent | hold | hold-noagent) ;;
        *) die "知らない場面: ${s}（firstrun / guided / update / master / noagent / hold / hold-noagent）" ;;
    esac
done

case " $scenes " in *" hold "* | *" hold-noagent "*) GUARD_KEEP=guard-hold ;; esac
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

guided_done=""
banner_hidden=""
for s in $scenes; do
    case "$s" in
        firstrun) scene_firstrun ;;
        noagent)
            # 別の人の HOME で起動し直す（先に立てていた GUI は閉じる）
            stop_gui
            scene_noagent
            stop_gui
            ;;
        hold-noagent)
            stop_gui
            noagent_up
            hold "$(gui_path "$SB/bin-noagent")"
            ;;
        *)
            # 初回起動の場面を撮らずに始めるとき（場面を選んだ撮り直し・hold）も、同じ初回起動から始める
            if [ -z "$GUI_PID" ]; then
                use_home "$MAIN_HOME"
                howto_gui_up main
                wait_step setup
                sleep 2
            fi
            case "$s" in
                hold) hold "$(gui_path)" ;;
                guided)
                    scene_guided
                    guided_done=1
                    ;;
                update | master)
                    # 対話のセットアップを撮らずにここから始めるなら、撮らずに同じ流れを通しておく
                    # （プロジェクトの登録・プロファイルの調整・設定共有まで済んだ状態にする）
                    if [ -z "$guided_done" ]; then
                        FAST=1
                        SCENE="$s"
                        guided_flow
                        FAST=""
                        verify_guided
                        guided_done=1
                    fi
                    # 案内の「master を起動」は撮り終えている。この先は案内を「あとで」で閉じて進む
                    # （ボタンと同じ要求。この機ではボタンが /Applications の版を選ぶので、押さずに自分で打つ）
                    if [ -z "$banner_hidden" ]; then
                        cli welcome hide > /dev/null || die "案内を「あとで」で閉じられない"
                        cli tab select 1 > /dev/null || die "最初のタブへ切り替えられない"
                        banner_hidden=1
                        sleep 1
                    fi
                    "scene_$s"
                    ;;
            esac
            ;;
    esac
done
say "収録おわり: $OUT/scenes"
