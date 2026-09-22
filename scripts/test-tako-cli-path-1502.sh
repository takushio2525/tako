#!/bin/bash
# test-tako-cli-path-1502.sh — tako CLI の PATH 設置（FR-2.14.5 / #1502）の実経路テスト
#
# 何を確かめるか:
#   `tako setup` が **外部ターミナル**（Terminal.app / iTerm2 等）から `tako` を
#   打てる状態を自分で作るか。作ったものを `undo-path` で元へ戻せるか。
#   `.app` を動かしても壊れたままにならないか。
#
# どう隔離するか（**本番の ~/.zprofile / ~/.zshrc / ~/Library/... へ 1 バイトも書かない**）:
#   - `HOME` を一時ディレクトリへ振る（`TAKO_DATA_DIR` では `~/.zprofile` を隔離できない）
#   - `TAKO_DATA_DIR` / `TAKO_ORCHESTRATOR_DIR` / `TAKO_TMUX_SOCKET` / `TAKO_SOCKET` も一時へ
#   - `env -i` で本番の env を持ち込まない。`PATH` は launchd の既定 + 隔離 HOME の bin
#   - 実行するのは**隔離 HOME の中に作った偽の .app に置いた tako**（実体の置き場所が
#     設置先の選び方に効くので、本物の /Applications は使わない）
#
# GUI を使う項目（check-health）は仮想ディスプレイへ出す（#1141 / #1490）。
#
# 使い方: bash scripts/test-tako-cli-path-1502.sh
#         TAKO_1502_LEGACY=1 bash scripts/test-tako-cli-path-1502.sh   # A/B（設置前の挙動）
#
# 配列は `${arr[@]+"${arr[@]}"}` で展開する（macOS 同梱の bash 3.2 は `set -u` の下で
# 空配列の `"${arr[@]}"` を未定義として落とす。#1499 の実測。`.agent/conventions.md`）

set -uo pipefail

# **本番 GUI を指す env を最初に落とす**。tako のペインの中から走らせると
# `TAKO_SOCKET` / `TAKO_TOKEN` / `TAKO_PANE_ID` が隔離 GUI へも継承され、
# CLI がユーザーの本番 GUI を触る（#1449 で実測）
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID

REPO_ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"

OK=0; NG=0
pass() { OK=$((OK+1)); echo "  OK  $*"; }
fail() { NG=$((NG+1)); echo "  NG  $*"; }
check() { if [ "$1" = "1" ]; then pass "$2"; else fail "$2"; fi; }
section() { echo; echo "=== $* ==="; }

TMP=$(mktemp -d /tmp/tako1502.XXXXXX) || exit 1
ISO_HOME="$TMP/home"
APP_MACOS="$TMP/A/tako.app/Contents/MacOS"
ISO_TAKO="$APP_MACOS/tako"
GUI_PID=""

cleanup() {
    [ -n "$GUI_PID" ] && stop_isolated_gui "$GUI_PID"
    # **一時ディレクトリ配下であることを確かめてから消す**（実環境破壊の再発防止）
    case "$TMP" in /tmp/tako1502.*) rm -rf "$TMP" ;; esac
}
trap cleanup EXIT

isolated_gui_bins || exit 1

# CLI の IPC の向き先。**既定は在りもしないソケット**（GUI を立てない段で
# CLI が本番 GUI へ迷い込まない）。GUI を立てる段だけ空にして、隔離 GUI と同じ
# `TAKO_DATA_DIR` から解決させる
IPC_GUARD=("TAKO_SOCKET=$TMP/nowhere.sock" "TAKO_DISCOVERY_DIR=$TMP/nowhere")

# 隔離した tako を叩く
run() {
    local -a extra=()
    [ -n "${TAKO_1502_LEGACY:-}" ] && extra+=("TAKO_1502_LEGACY=1")
    env -i HOME="$ISO_HOME" PATH="$ISO_HOME/.local/bin:/usr/bin:/bin:/usr/sbin:/sbin" \
        SHELL=/bin/zsh TAKO_DATA_DIR="$TMP/d" TAKO_ORCHESTRATOR_DIR="$TMP/d/orch" \
        TAKO_TMUX_SOCKET="$TMP/tmux.sock" \
        ${IPC_GUARD[@]+"${IPC_GUARD[@]}"} ${extra[@]+"${extra[@]}"} \
        "$ISO_TAKO" "$@" < /dev/null 2>&1
}

# **新しいターミナルと同じ条件**でログインシェルを起こす（PATH は launchd の既定だけ）
login_shell() {
    env -i HOME="$ISO_HOME" PATH=/usr/bin:/bin:/usr/sbin:/sbin SHELL=/bin/zsh \
        /bin/zsh -l -c "$1" 2>&1
}

markers() {
    [ -f "$ISO_HOME/.zprofile" ] || { echo 0; return; }
    # `grep -c` は 0 件のとき "0" を出して exit 1 なので、状態だけ捨てる
    grep -c '# >>> tako PATH >>>' "$ISO_HOME/.zprofile" 2>/dev/null || true
}

# ログインシェルから tako が引けないこと（引けたら NG）
login_shell_lacks_tako() {
    login_shell 'command -v tako' > /dev/null 2>&1 && echo 0 || echo 1
}

# 隔離 HOME を作り直す（項目ごとに前提をそろえる）
reset_home() {
    rm -rf "$ISO_HOME"
    mkdir -p "$ISO_HOME/.local/bin"
    # 認証済みの claude スタブ。これが無いと setup が「ゼロスタート導入」へ入り、
    # 公式インストーラをネットワークから取ってきてしまう（隔離の意味が薄れる）。
    # **同時に #1502 の肝**でもある: claude が ready なら PATH 段は走らないので、
    # tako CLI の設置が「エージェントの PATH 段のついで」では成り立たないことを示す
    cat > "$ISO_HOME/.local/bin/claude" <<'STUB'
#!/bin/sh
case "$*" in
  "auth status --json") echo '{"loggedIn":true}'; exit 0 ;;
  "--version") echo "1.0.0 (stub)"; exit 0 ;;
  "mcp list"*) echo "tako: /stub/tako mcp serve - Connected"; exit 0 ;;
esac
exit 0
STUB
    chmod +x "$ISO_HOME/.local/bin/claude"
    printf '# user own first line\nexport TAKO_1502_USER_LINE=1\n' > "$ISO_HOME/.zprofile"
    rm -rf "$TMP/d"
}

mkdir -p "$APP_MACOS"
# 実体は 1 つだけ置く（同居物を PATH へ出さないことを見るので tako-app は要らない）
ln "$TAKO_BIN" "$ISO_TAKO" 2>/dev/null || cp "$TAKO_BIN" "$ISO_TAKO"

echo "tako CLI の PATH 設置（#1502）— 隔離 HOME: $ISO_HOME"
[ -n "${TAKO_1502_LEGACY:-}" ] && echo "  (A/B) TAKO_1502_LEGACY=1: 設置しない旧挙動"

# --- A. 設置前 -------------------------------------------------------------
section "A. 設置前（受け入れ条件 1 の前提）"
reset_home
check "$([ "$(markers)" = "0" ] && echo 1 || echo 0)" "A1 .zprofile に tako のブロックが無い"
check "$(login_shell_lacks_tako)" "A2 新しいログインシェルから tako が引けない"
out=$(run setup --check)
check "$(echo "$out" | grep -q '\[不足\] tako CLI の PATH' && echo 1 || echo 0)" \
      "A3 setup --check が tako CLI の PATH を [不足] と報告する"
check "$(echo "$out" | grep -q 'tako setup bootstrap path で設置できます' && echo 1 || echo 0)" \
      "A4 次の一手（最簡形の 1 コマンド）を出す"
check "$(echo "$out" | grep -q '\[OK\] エージェント CLI の導入: 使える系統 claude' && echo 1 || echo 0)" \
      "A5 claude は ready（= エージェントの PATH 段は走らない前提）"

# --- B. tako setup（受け入れ条件 1）----------------------------------------
section "B. tako setup --yes（受け入れ条件 1）"
out=$(run setup --yes)
rc=$?
check "$([ $rc -eq 0 ] && echo 1 || echo 0)" "B1 setup が exit 0 で完走する"
check "$(echo "$out" | grep -q '\[設置\] tako CLI' && echo 1 || echo 0)" "B2 設置したことを 1 行で伝える"
check "$([ "$(markers)" = "1" ] && echo 1 || echo 0)" "B3 マーカーブロックがちょうど 1 個"
check "$(grep -q '\$HOME/\.local/bin' "$ISO_HOME/.zprofile" && echo 1 || echo 0)" \
      "B4 ブロックが \$HOME/.local/bin を通す（home 相対）"
check "$([ -L "$ISO_HOME/.local/bin/tako" ] && echo 1 || echo 0)" "B5 ~/.local/bin/tako が symlink"
check "$([ "$(readlink "$ISO_HOME/.local/bin/tako")" = "$ISO_TAKO" ] && echo 1 || echo 0)" \
      "B6 symlink が .app 同梱の実体を指す"
found=$(login_shell 'command -v tako')
check "$([ "$found" = "$ISO_HOME/.local/bin/tako" ] && echo 1 || echo 0)" \
      "B7 新しいログインシェルで tako が見つかる（${found}）"
check "$(login_shell 'tako --version' | grep -q '^tako ' && echo 1 || echo 0)" \
      "B8 そこから実際に tako が動く"
# 実体のディレクトリ（同居物ごと）を PATH へ出していない = #1502 の設計の芯
check "$(grep -q "$APP_MACOS" "$ISO_HOME/.zprofile" && echo 0 || echo 1)" \
      "B9 .app の Contents/MacOS を PATH へ直接入れていない"
check "$(grep -q 'user own first line' "$ISO_HOME/.zprofile" && echo 1 || echo 0)" \
      "B10 ユーザーの既存行が残っている"

# --- C. 冪等（受け入れ条件 2）----------------------------------------------
section "C. 2 回目（受け入れ条件 2）"
cp "$ISO_HOME/.zprofile" "$TMP/zprofile.1"
out=$(run setup --yes)
check "$([ "$(markers)" = "1" ] && echo 1 || echo 0)" "C1 2 回目でもブロックは 1 個"
if diff -u "$TMP/zprofile.1" "$ISO_HOME/.zprofile" > "$TMP/zprofile.diff"; then
    pass "C2 .zprofile のバイト列が 1 回目と同一（diff 空）"
else
    fail "C2 .zprofile が変化した:"; sed 's/^/      /' "$TMP/zprofile.diff"
fi
check "$([ "$(readlink "$ISO_HOME/.local/bin/tako")" = "$ISO_TAKO" ] && echo 1 || echo 0)" \
      "C3 symlink も張り直されていない"
check "$(echo "$out" | grep -q '\[OK\] tako CLI' && echo 1 || echo 0)" "C4 2 回目は [OK] と出る（設置済み）"

# --- D. --check（受け入れ条件 4 の前半）------------------------------------
section "D. setup --check（受け入れ条件 4）"
out=$(run setup --check)
check "$(echo "$out" | grep -q '\[OK\] tako CLI の PATH: 外部ターミナルからも使えます' && echo 1 || echo 0)" \
      "D1 設置後は [OK] と報告する"

# --- E. undo-path（受け入れ条件 3）-----------------------------------------
section "E. undo-path（受け入れ条件 3）"
out=$(run setup bootstrap undo-path)
check "$([ "$(markers)" = "0" ] && echo 1 || echo 0)" "E1 ブロックが消える"
check "$([ ! -e "$ISO_HOME/.local/bin/tako" ] && echo 1 || echo 0)" "E2 symlink も消える"
check "$(printf '# user own first line\nexport TAKO_1502_USER_LINE=1\n' | diff -q - "$ISO_HOME/.zprofile" >/dev/null && echo 1 || echo 0)" \
      "E3 ユーザーの行だけが元のバイト列で残る"
check "$(login_shell_lacks_tako)" "E4 新しいログインシェルから引けなくなる"
check "$([ -x "$ISO_HOME/.local/bin/claude" ] && echo 1 || echo 0)" "E5 同じディレクトリの claude は消えない"

# --- F. .app の移動（受け入れ条件 5）---------------------------------------
section "F. .app を移動しても壊れたままにしない（受け入れ条件 5）"
reset_home
run setup bootstrap path > /dev/null
check "$([ "$(readlink "$ISO_HOME/.local/bin/tako")" = "$ISO_TAKO" ] && echo 1 || echo 0)" \
      "F1 bootstrap path 単体でも設置できる（CLI / MCP と 1:1）"
MOVED="$TMP/B/tako.app/Contents/MacOS"
mkdir -p "$MOVED"; mv "$ISO_TAKO" "$MOVED/tako"; rm -rf "$TMP/A"
check "$([ ! -e "$ISO_HOME/.local/bin/tako" ] && echo 1 || echo 0)" "F2 移動でリンクが切れる（前提）"
ISO_TAKO="$MOVED/tako"
out=$(run setup --check)
check "$(echo "$out" | grep -q 'リンクの指す先が消えています' && echo 1 || echo 0)" \
      "F3 --check が「指す先が消えた」と名指しする（PATH には在るので cli_in_path では拾えない）"
run setup bootstrap path > /dev/null
check "$([ "$(readlink "$ISO_HOME/.local/bin/tako")" = "$MOVED/tako" ] && echo 1 || echo 0)" \
      "F4 移動先の tako から bootstrap path を打つと張り直る"
check "$([ "$(login_shell 'command -v tako')" = "$ISO_HOME/.local/bin/tako" ] && echo 1 || echo 0)" \
      "F5 新しいログインシェルから再び引ける"
check "$([ "$(markers)" = "1" ] && echo 1 || echo 0)" "F6 ブロックは増えていない"
APP_MACOS="$MOVED"

# --- G. エッジ --------------------------------------------------------------
section "G. エッジ"
# G1-G2: .zprofile が無い
reset_home; rm -f "$ISO_HOME/.zprofile"
run setup bootstrap path > /dev/null
check "$([ -f "$ISO_HOME/.zprofile" ] && echo 1 || echo 0)" "G1 .zprofile が無ければ作る"
check "$([ "$(markers)" = "1" ] && echo 1 || echo 0)" "G2 作ったファイルにブロックは 1 個"

# G3-G4: 既に手書きで PATH が通っている
reset_home
printf '# user own first line\nexport PATH="$HOME/.local/bin:$PATH"\n' > "$ISO_HOME/.zprofile"
run setup bootstrap path > /dev/null
check "$([ "$(markers)" = "0" ] && echo 1 || echo 0)" "G3 手書きで通っているならブロックを足さない"
check "$([ -L "$ISO_HOME/.local/bin/tako" ] && echo 1 || echo 0)" "G4 それでも symlink は張る（実体が要る）"
check "$([ "$(login_shell 'command -v tako')" = "$ISO_HOME/.local/bin/tako" ] && echo 1 || echo 0)" \
      "G5 手書き経路でも外部ターミナルから引ける"

# G6-G8: .zprofile が読み取り専用
reset_home
chmod 444 "$ISO_HOME/.zprofile"
out=$(run setup --yes); rc=$?
chmod 644 "$ISO_HOME/.zprofile"
check "$([ $rc -eq 0 ] && echo 1 || echo 0)" "G6 profile へ書けなくても setup は止まらない"
check "$(echo "$out" | grep -q '\[警告\] tako CLI の PATH 設置を見送りました' && echo 1 || echo 0)" \
      "G7 黙らず理由と次の一手を出す"
check "$(grep -q 'user own first line' "$ISO_HOME/.zprofile" && echo 1 || echo 0)" "G8 読めないファイルを壊さない"

# G9-G10: symlink でない実体が置かれている
reset_home
printf '#!/bin/sh\necho user-own-tako\n' > "$ISO_HOME/.local/bin/tako"
chmod +x "$ISO_HOME/.local/bin/tako"
run setup bootstrap path > /dev/null
check "$([ ! -L "$ISO_HOME/.local/bin/tako" ] && echo 1 || echo 0)" "G9 ユーザーが置いた実体を symlink で上書きしない"
check "$(grep -q 'user-own-tako' "$ISO_HOME/.local/bin/tako" && echo 1 || echo 0)" "G10 中身も壊さない"
run setup bootstrap undo-path > /dev/null
check "$([ -f "$ISO_HOME/.local/bin/tako" ] && echo 1 || echo 0)" "G11 undo-path でもユーザーの実体は消さない"

# --- H. check-health（受け入れ条件 4 の後半。GUI 経由）---------------------
section "H. check-health（受け入れ条件 4）"
reset_home
run setup bootstrap path > /dev/null
# **GUI も同じ .app の中から起こす**（`cli_dir()` は実行中バイナリの隣を見るので、
# repo の target から起こすと「別のインストールの tako を指している」= Other になる）
ln "$APP_BIN" "$APP_MACOS/tako-app" 2>/dev/null || cp "$APP_BIN" "$APP_MACOS/tako-app"
APP_BIN="$APP_MACOS/tako-app"
# ここだけ IPC の目隠しを外す（隔離 GUI が置く接続情報ファイルを CLI に読ませる）。
# **本番の discovery は使わない**（同じ一時ディレクトリの中で閉じる）
mkdir -p "$TMP/disc"
IPC_GUARD=("TAKO_DISCOVERY_DIR=$TMP/disc")
launch_isolated_gui "$TMP/app.log" \
    "HOME=$ISO_HOME" "TAKO_DATA_DIR=$TMP/d" "TAKO_ORCHESTRATOR_DIR=$TMP/d/orch" \
    "TAKO_TMUX_SOCKET=$TMP/tmux.sock" "TAKO_DISCOVERY_DIR=$TMP/disc"
GUI_PID="$ISOLATED_GUI_PID"
ready=0
for _ in $(seq 1 200); do
    if run list > /dev/null 2>&1; then ready=1; break; fi
    sleep 0.1
done
check "$ready" "H1 隔離 GUI が立つ"
if [ "$ready" = "1" ]; then
    health=$(run check-health --json)
    echo "$health" > "$TMP/health.json"
    check "$(echo "$health" | grep -q '"link_state": "correct"' && echo 1 || echo 0)" \
          "H2 check-health の tako_cli_path.link_state = correct"
    check "$(echo "$health" | grep -q '"usable": true' && echo 1 || echo 0)" \
          "H3 tako_cli_path.usable = true（外部ターミナルから打てる）"
    check "$(echo "$health" | grep -q '"check": "cli_in_path"' && echo 0 || echo 1)" \
          "H4 cli_in_path の issue が出ない"
    check "$(echo "$health" | grep -q '"check": "tako_cli_link"' && echo 0 || echo 1)" \
          "H5 tako_cli_link の issue が出ない"
    # 起動時の自動修復（#1502）: .app を移すとリンクが切れるが、次の起動で張り直る
    stop_isolated_gui "$GUI_PID"; GUI_PID=""
    MOVED2="$TMP/C/tako.app/Contents/MacOS"; mkdir -p "$MOVED2"
    mv "$ISO_TAKO" "$MOVED2/tako"; ln "$APP_BIN" "$MOVED2/tako-app" 2>/dev/null || cp "$APP_BIN" "$MOVED2/tako-app"
    check "$([ ! -e "$ISO_HOME/.local/bin/tako" ] && echo 1 || echo 0)" "H6 移動でリンクが切れる（前提）"
    APP_BIN="$MOVED2/tako-app" launch_isolated_gui "$TMP/app2.log" \
        "HOME=$ISO_HOME" "TAKO_DATA_DIR=$TMP/d" "TAKO_ORCHESTRATOR_DIR=$TMP/d/orch" \
        "TAKO_TMUX_SOCKET=$TMP/tmux.sock" "TAKO_DISCOVERY_DIR=$TMP/disc"
    GUI_PID="$ISOLATED_GUI_PID"
    repaired=0
    for _ in $(seq 1 200); do
        [ "$(readlink "$ISO_HOME/.local/bin/tako" 2>/dev/null)" = "$MOVED2/tako" ] && { repaired=1; break; }
        sleep 0.1
    done
    check "$repaired" "H7 GUI を起動し直すと切れたリンクが自動で張り直る"
    stop_isolated_gui "$GUI_PID"; GUI_PID=""
fi

echo
echo "===================="
echo "OK=$OK NG=$NG"
[ "$NG" -eq 0 ] || exit 1
