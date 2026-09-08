#!/bin/bash
# tako 紹介動画 収録共通ライブラリ (#470 Phase B)
#
# 各シーン収録スクリプトから source して使う。責務:
#   - 継承 TAKO_* 環境の遮断（worker ペイン内から実行しても本番へ誤接続しない）
#   - PII を含まないデモ環境（ダミープロジェクト + クリーンプロンプト）の生成
#   - 隔離 GUI インスタンス（TAKO_ISOLATED=1 + 明示ソケット/データディレクトリ）の起動と後始末
#   - ウィンドウ単体キャプチャによる収録（screencapture -l<windowID> の連番 → ffmpeg 結合）
#   - 収録物の ffprobe 検証 + フレーム抽出（PII 全数チェック用）
#
# 収録エンジンについて（2026-07-23 実測）:
#   - screencapture -v（動画）は本環境で黒画面。静止画（-x）は正常なので連番で撮る
#   - 画面全体を撮って切り出す方式（ffmpeg avfoundation / screencapture -R）は、
#     対象ウィンドウの手前に別アプリのウィンドウが重なるとその中身ごと写り込むため使わない
#     （実際に他アプリの内容が混入した素材を作ってしまい破棄した）
#   - 画面ロック中と、隔離ウィンドウが別 Space にある場合はキャプチャできない。
#     promo_check_capturable がロックと権限不足を切り分けて事前に止める
#
# 収録の舞台（2026-09-06・#1081）:
#   - 既定は**仮想ディスプレイ**（BetterDisplay の仮想スクリーン `tako-vd`。常設・削除しない）。
#     隔離 tako の窓は起動時からそこへ出し、ユーザーのメイン画面には窓もフォーカス移動も出さない。
#     GPUI は窓が完全に隠れると描画を止めるが、仮想ディスプレイ上の窓は何にも隠れないので
#     従来の「定期的に最前面へ activate する」（= ユーザーの作業を毎 2 秒奪う）が要らなくなる
#   - `TAKO_PROMO_STAGE=main` で従来方式（メイン画面 + activate）へ戻せる（A/B 用。普段は使わない）

PROMO_APP=${TAKO_PROMO_APP:-/Applications/tako.app/Contents/MacOS/tako-app}
PROMO_CLI=${TAKO_PROMO_CLI:-/Applications/tako.app/Contents/MacOS/tako}
PROMO_OUT=${TAKO_PROMO_OUT:-"$HOME/Desktop/tako-promo"}
# フレーム抽出（PII 検証用の中間物）は Desktop の TCC 制限を避けて /private/tmp に置く
PROMO_FRAMES=${TAKO_PROMO_FRAMES:-/private/tmp/tako-promo-frames}
PROMO_DEMO=/private/tmp/tako-demo
PROMO_LIB_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# 収録で起こす claude（隔離 tako の master / worker / setup アシスタント）に使う Claude Code の
# 設定ディレクトリ（= アカウント）。空なら従来どおりデモ HOME 配下（`$PROMO_DEMO/home/.claude`。
# 資格情報はログインキーチェーンの既定項目を共有）。`TAKO_PROMO_CLAUDE_CONFIG_DIR=$HOME/.claude-univ`
# のように別アカウントの実ディレクトリを指すと、その資格情報（`Claude Code-credentials-<sha256 の先頭 8 桁>`）
# で動く。画面にアカウント名や config dir は出ないので絵は変わらない（2026-09-06: personal が
# ログアウト状態のあいだ univ で撮った）。**実ディレクトリの写しやシンボリックリンクは不可**
# （キーチェーンの項目名がパス文字列のハッシュなので、別パスだと資格情報が見つからない）
PROMO_CLAUDE_CONFIG_DIR=${TAKO_PROMO_CLAUDE_CONFIG_DIR:-}

# 継承環境を env -u で落とすための引数列。
#   TAKO_*      … 本番インスタンスへの誤接続を防ぐ
#   CLAUDE*/ANTHROPIC* … 収録スクリプト自体を AI エージェントのペインから流したときに、
#                        親セッションのマーカー（CLAUDE_CODE_CHILD_SESSION 等）が
#                        子の claude へ漏れて画面に警告行が出るのを防ぐ（#470 v3）
PROMO_ENV_CLEAN=()
# シーン固有の追加環境変数（"KEY=VALUE" 形式。promo_start_isolated が展開する）
PROMO_EXTRA_ENV=()
while IFS='=' read -r k _; do
    case "$k" in
        TAKO_*|CLAUDE*|ANTHROPIC_*) PROMO_ENV_CLEAN+=(-u "$k") ;;
    esac
done < <(env)

promo_require() {
    [ -x "$PROMO_APP" ] || { echo "ERROR: tako-app が無い: $PROMO_APP" >&2; return 1; }
    [ -x "$PROMO_CLI" ] || { echo "ERROR: tako CLI が無い: $PROMO_CLI" >&2; return 1; }
    command -v ffmpeg >/dev/null || { echo "ERROR: ffmpeg が必要" >&2; return 1; }
    command -v ffprobe >/dev/null || { echo "ERROR: ffprobe が必要" >&2; return 1; }
    mkdir -p "$PROMO_OUT/scenes" "$PROMO_FRAMES"
    # winbounds は収録ループから何度も呼ぶので、毎回 swift でコンパイルせず
    # 一度バイナリ化して使い回す（1 回あたり数秒の差になる）
    PROMO_WINBOUNDS=/private/tmp/tako-promo-winbounds
    if [ ! -x "$PROMO_WINBOUNDS" ] || \
       [ "$PROMO_LIB_DIR/winbounds.swift" -nt "$PROMO_WINBOUNDS" ]; then
        swiftc -O -o "$PROMO_WINBOUNDS" "$PROMO_LIB_DIR/winbounds.swift" 2>/dev/null || {
            echo "ERROR: winbounds.swift のコンパイルに失敗" >&2; return 1; }
    fi
    # ディスプレイ一覧（仮想ディスプレイの矩形と HiDPI 判定に使う）も同じ扱い
    PROMO_DISPLAYS=/private/tmp/tako-promo-displays
    if [ ! -x "$PROMO_DISPLAYS" ] || \
       [ "$PROMO_LIB_DIR/displays.swift" -nt "$PROMO_DISPLAYS" ]; then
        swiftc -O -o "$PROMO_DISPLAYS" "$PROMO_LIB_DIR/displays.swift" 2>/dev/null || {
            echo "ERROR: displays.swift のコンパイルに失敗" >&2; return 1; }
    fi
    # GUI 操作（#1081 の GUI モード章）。実クリックと実キー入力も同じ扱いで焼いておく
    PROMO_CLICK=/private/tmp/tako-promo-click
    if [ ! -x "$PROMO_CLICK" ] || [ "$PROMO_LIB_DIR/click.swift" -nt "$PROMO_CLICK" ]; then
        swiftc -O -o "$PROMO_CLICK" "$PROMO_LIB_DIR/click.swift" 2>/dev/null || {
            echo "ERROR: click.swift のコンパイルに失敗" >&2; return 1; }
    fi
    PROMO_KEYTYPE=/private/tmp/tako-promo-keytype
    if [ ! -x "$PROMO_KEYTYPE" ] || [ "$PROMO_LIB_DIR/keytype.swift" -nt "$PROMO_KEYTYPE" ]; then
        swiftc -O -o "$PROMO_KEYTYPE" "$PROMO_LIB_DIR/keytype.swift" 2>/dev/null || {
            echo "ERROR: keytype.swift のコンパイルに失敗" >&2; return 1; }
    fi
}

# 画面がロックされていないか調べる（ロック中は screencapture が一切動かない）。
# ロック中なら 1 を返す
promo_screen_locked() {
    ioreg -n Root -d1 -r 2>/dev/null | grep -q '"CGSSessionScreenIsLocked"=Yes'
}

# 収録可能な状態か事前に検査する。ロック・権限のどちらで止まっているかを切り分ける
promo_check_capturable() {
    if promo_screen_locked; then
        echo "ERROR: 画面がロックされています。ロック中は macOS が画面キャプチャを" >&2
        echo "       一切許可しないため収録できません。ロックを解除してから再実行してください。" >&2
        return 1
    fi
    local probe=/private/tmp/tako-promo-capcheck.png
    rm -f "$probe"
    if ! screencapture -x -R0,0,64,64 "$probe" 2>/dev/null || [ ! -s "$probe" ]; then
        rm -f "$probe"
        echo "ERROR: 画面キャプチャができません（ロックはされていない）。" >&2
        echo "       システム設定 > プライバシーとセキュリティ > 画面収録 で" >&2
        echo "       このターミナルアプリに許可を与えてください。" >&2
        return 1
    fi
    rm -f "$probe"
    return 0
}

# 実際にキャプチャできる状態になるまで待つ（$1 = 最大待ち秒数。既定 0 = 待たない）。
# ロックフラグは解除直後にキャプチャ可否と食い違うことがあるため、
# 「テストキャプチャが 2 回続けて成功する」ことを判定基準にする
promo_wait_capturable() {
    local limit=${1:-0} waited=0 probe=/private/tmp/tako-promo-waitcap.png ok=0
    while :; do
        rm -f "$probe"
        if screencapture -x -R0,0,64,64 "$probe" 2>/dev/null && [ -s "$probe" ]; then
            ok=$((ok + 1))
            [ "$ok" -ge 2 ] && { rm -f "$probe"; return 0; }
        else
            ok=0
        fi
        rm -f "$probe"
        [ "$limit" -gt 0 ] || return 1
        sleep 5
        waited=$((waited + 5))
        if [ "$waited" -ge "$limit" ]; then
            echo "   収録可能になるのを待てませんでした（${limit}s 経過）" >&2
            return 1
        fi
        if [ $(( waited % 60 )) -eq 0 ]; then
            echo "   収録可能になるのを待機中（${waited}s。画面ロック中は撮れません）"
        fi
    done
}

# 後方互換の別名（呼び出し側の意図は「収録できるまで待つ」）
promo_wait_unlock() { promo_wait_capturable "$@"; }

# 対象 PID のウィンドウが実際にキャプチャできるようになるまで待つ。
# 成功したウィンドウ ID を PROMO_WID に入れる
# ウィンドウは開いた直後にサイズが変化する（GPUI の初期化過程）。過渡状態を掴むと
# 極端に小さい素材になるので、①最低幅を満たし ②2 回続けて同じサイズ になるまで待つ
PROMO_MIN_WIN_W=${TAKO_PROMO_MIN_WIN_W:-800}
promo_wait_window() {
    local probe="$PROMO_WORK/wid-probe.png" i b wid geom prev=""
    PROMO_WID=""
    for i in $(seq 1 60); do
        b=$("$PROMO_WINBOUNDS" "$PROMO_APP_PID" 2>/dev/null || true)
        if [ -n "$b" ]; then
            wid=$(echo "$b" | cut -d' ' -f1)
            geom=$(echo "$b" | cut -d' ' -f4,5)
            local w; w=$(echo "$geom" | cut -d' ' -f1)
            if [ "$w" -ge "$PROMO_MIN_WIN_W" ] && [ "$geom" = "$prev" ]; then
                rm -f "$probe"
                if screencapture -x -o -l"$wid" "$probe" 2>/dev/null && [ -s "$probe" ]; then
                    rm -f "$probe"
                    PROMO_WID=$wid
                    PROMO_WIN_GEOM=$geom
                    return 0
                fi
            fi
            prev=$geom
        fi
        sleep 0.5
    done
    echo "ERROR: 収録できるウィンドウが現れない（pid=${PROMO_APP_PID}、" >&2
    echo "       最低幅 ${PROMO_MIN_WIN_W}pt を満たすウィンドウが安定しない）" >&2
    return 1
}

# ── デモ環境（PII ゼロ）────────────────────────────────────────────
# プロンプトは %n@%m（ユーザー名@ホスト名）を含まない「ディレクトリ名 ❯」のみにする。
# 作業パスは /private/tmp 配下だけを使い、ホームディレクトリを一切写さない。
promo_make_demo_env() {
    rm -rf "$PROMO_DEMO"
    mkdir -p "$PROMO_DEMO/zdot" "$PROMO_DEMO/awesome-app/src" \
        "$PROMO_DEMO/awesome-app/scripts" "$PROMO_DEMO/awesome-app/docs"

    cat > "$PROMO_DEMO/zdot/.zshrc" <<'ZRC'
PROMPT='%F{6}%1~%f ❯ '
RPROMPT=''
unset ZSH_THEME
ZRC

    cat > "$PROMO_DEMO/awesome-app/README.md" <<'MD'
# awesome-app

A tiny web app used to demo **tako** — the GUI terminal built for the
AI-agent era.

## Features

- Fast REST API
- Live dashboard
- One-command deploy

## Quick start

```sh
scripts/dev-server.sh
```

Then open `http://localhost:5173`.
MD

    cat > "$PROMO_DEMO/awesome-app/src/app.py" <<'PY'
"""awesome-app: demo web app."""

import time

ROUTES = {
    "/api/users": '[{"id": 1, "name": "alice"}]',
    "/api/posts": '[{"id": 7, "title": "hello"}]',
}


def handle(path: str) -> tuple[int, str]:
    """Return (status, body) for a request path."""
    if path in ROUTES:
        return 200, ROUTES[path]
    return 404, "not found"


def main() -> None:
    print("awesome-app listening on :5173")
    while True:
        time.sleep(1)


if __name__ == "__main__":
    main()
PY

    # tako:run 宣言つき = Code Runner の再生ボタンで実行できるデモ用スクリプト。
    # Run ペインの cwd は宣言ファイルのあるディレクトリ（scripts/）になるため、
    # ${file} で自分自身を絶対パス実行し、スクリプト内でプロジェクト直下へ cd する
    cat > "$PROMO_DEMO/awesome-app/scripts/build.sh" <<'BLD'
#!/bin/bash
# tako:run: bash ${file}
cd "$(dirname "$0")/.." || exit 1
set -e
steps=("resolving deps" "compiling src/app.py" "bundling assets" "writing dist/")
for s in "${steps[@]}"; do
    printf '\033[36m>>\033[0m %s\n' "$s"
    sleep 0.5
done
printf '\033[32mbuild succeeded\033[0m in 2.1s\n'
# 収録用: 完了直後にペインが exit で閉じると結果が写らないので保持する
sleep 600
BLD
    chmod +x "$PROMO_DEMO/awesome-app/scripts/build.sh"

    cat > "$PROMO_DEMO/awesome-app/scripts/dev-server.sh" <<'SRV'
#!/bin/bash
# デモ用のダミー dev サーバー: それらしいアクセスログを流し続ける
printf '\033[1;36m  dev server\033[0m ready on \033[4mhttp://localhost:5173\033[0m\n\n'
paths=(/api/users /api/posts /assets/app.js /index.html /api/health)
i=0
while true; do
    p=${paths[$((i % ${#paths[@]}))]}
    ms=$(( (RANDOM % 40) + 3 ))
    printf '\033[90m%s\033[0m \033[32mGET\033[0m %-16s \033[1m200\033[0m %2dms\n' \
        "$(date '+%H:%M:%S')" "$p" "$ms"
    i=$((i + 1))
    sleep 0.4
done
SRV
    chmod +x "$PROMO_DEMO/awesome-app/scripts/dev-server.sh"

    # worker が「作業しているように見える」ダミーログ（S3 のワーカーペイン用）
    cat > "$PROMO_DEMO/awesome-app/scripts/worker.sh" <<'WRK'
#!/bin/bash
# デモ用のダミー worker: 受け取ったタスク名の作業ログを流して完了する
task=${1:-task}
printf '\033[35mworker\033[0m %s\n' "$task"
lines=("reading source files" "applying changes" "running tests" "all checks passed")
for l in "${lines[@]}"; do
    printf '  \033[90m*\033[0m %s\n' "$l"
    sleep 1.2
done
printf '\033[32mdone\033[0m %s\n' "$task"
sleep 600
WRK
    chmod +x "$PROMO_DEMO/awesome-app/scripts/worker.sh"

    promo_make_demo_extras
}

# 解説動画（#1081）の基本操作章で使う追加素材。すべてリポジトリ内かその場生成で PII なし
promo_make_demo_extras() {
    local repo="$PROMO_LIB_DIR/../.."
    mkdir -p "$PROMO_DEMO/awesome-app/docs" "$PROMO_DEMO/awesome-app/tests" "$PROMO_DEMO/tako-docs"

    # 画像プレビュー用: tako のアイコン候補 PNG（リポジトリ同梱）
    cp "$repo/assets/icon/preview/icon-a-1024.png" "$PROMO_DEMO/awesome-app/docs/logo.png"

    # PDF プレビュー用: macOS 同梱の cupsfilter でテキストから作る（外部依存なし）
    cat > "$PROMO_DEMO/awesome-app/docs/spec.txt" <<'TXT'
awesome-app  Design Notes

1. Overview
   awesome-app is a tiny web app used to demo tako.

2. Architecture
   - src/app.py    HTTP routes
   - src/api.py    JSON API helpers
   - scripts/      dev server, build, worker

3. API
   GET /api/users   -> list users
   GET /api/posts   -> list posts
   GET /api/health  -> {"ok": true}

4. Build
   scripts/build.sh compiles and bundles the app.
TXT
    cupsfilter -i text/plain "$PROMO_DEMO/awesome-app/docs/spec.txt" \
        > "$PROMO_DEMO/awesome-app/docs/spec.pdf" 2>/dev/null || rm -f "$PROMO_DEMO/awesome-app/docs/spec.pdf"

    cat > "$PROMO_DEMO/awesome-app/src/api.py" <<'PY'
"""awesome-app: JSON API helpers."""

import json
from dataclasses import dataclass, asdict


@dataclass
class User:
    id: int
    name: str


def users() -> list[User]:
    return [User(1, "alice"), User(2, "bob")]


def to_json(items) -> str:
    return json.dumps([asdict(i) for i in items])
PY

    cat > "$PROMO_DEMO/awesome-app/tests/test_api.py" <<'PY'
from src.api import users, to_json


def test_users_are_serializable():
    assert '"alice"' in to_json(users())
PY

    # master 章の worker に渡す「完了する」タスク（worker.sh は末尾で sleep 600 するので
    # エージェントの Bash 呼び出しが終わらず、報告まで撮れない）。
    # 長さは約 80 秒（TAKO_PROMO_TASK_SECS）: 数秒で終わると master が `orchestrator_run` の
    # auto_close で worker ペインを畳み、「3 体が並ぶ / orch ビュー / かんたん表示」の絵が
    # 撮れないまま報告だけが残る（2026-09-06 に実測。sonnet・effort medium は 2 分で完走した）
    cat > "$PROMO_DEMO/awesome-app/scripts/task.sh" <<'TSK'
#!/bin/bash
# デモ用: 受け取ったタスク名の作業ログを流し、テストを回している風の進捗を出して完了する
task=${1:-task}
total=${TAKO_PROMO_TASK_SECS:-80}
printf 'task %s\n' "$task"
lines=("reading source files" "applying changes" "running tests")
for l in "${lines[@]}"; do
    printf '  * %s\n' "$l"
    sleep 1.5
done
# 残りの時間は 1 行ずつテスト結果を流す（画面が動き続ける = 「稼働中」に見える）
n=$(( (total - 8) / 2 )); [ "$n" -gt 0 ] || n=1
for i in $(seq 1 "$n"); do
    printf '    test_%s_%02d ... ok\n' "$task" "$i"
    sleep 2
done
printf '  * all checks passed\n'
printf 'done %s: 4 files changed, %d tests green\n' "$task" "$n"
TSK
    chmod +x "$PROMO_DEMO/awesome-app/scripts/task.sh"

    # 課題提起の章で「ログのタブ」として流す疑似ログ
    cat > "$PROMO_DEMO/awesome-app/scripts/logs.sh" <<'LOG'
#!/bin/bash
# デモ用: アプリケーションログを流し続ける
levels=(INFO INFO INFO WARN INFO DEBUG)
msgs=("request handled" "cache warmed" "job queued" "slow query 412ms" "user signed in" "gc pause 3ms")
i=0
while true; do
    printf '\033[90m%s\033[0m %-5s %s\n' "$(date '+%H:%M:%S')" "${levels[$((i % 6))]}" "${msgs[$((i % 6))]}"
    i=$((i + 1))
    sleep 0.7
done
LOG
    chmod +x "$PROMO_DEMO/awesome-app/scripts/logs.sh"

    # Windows / OSS の章で tako 自身のドキュメントをプレビューする（実 HOME のパスを
    # 画面に出さないよう、リポジトリから /private/tmp 配下へ写しを置く）
    cp "$repo/README.md" "$PROMO_DEMO/tako-docs/README.md"
    cp "$repo/LICENSE" "$PROMO_DEMO/tako-docs/LICENSE"
    # docs の frontmatter（--- title/description ---）は md プレビューでは本文として描かれるので
    # 見出しに置き換える
    { echo "# Windows 対応状況"; echo;
      awk 'BEGIN{fm=0} NR==1 && /^---$/ {fm=1; next} fm==1 && /^---$/ {fm=2; next} fm!=1 {print}' \
          "$repo/docs/src/content/docs/windows-support.md"; } > "$PROMO_DEMO/tako-docs/windows-support.md"
}

# setup シーン用のデモ HOME とデモ PATH（#470 v2）。
# `tako setup` 系は $HOME 配下（~/.claude.json 等）を書き換え、実行パスを画面に出す。
# 実 HOME のまま撮ると「/Users/<ユーザー名>/...」がフレームに残るため、
#   - HOME  … /private/tmp/tako-demo/home（使い捨て。実設定に触れない）
#   - PATH  … /private/tmp/tako-demo/bin（必要なコマンドの symlink だけ）
# に差し替えて撮る。画面に出るパスはすべて /private/tmp 配下になる。
promo_make_demo_home() {
    rm -rf "$PROMO_DEMO/home" "$PROMO_DEMO/bin"
    mkdir -p "$PROMO_DEMO/home" "$PROMO_DEMO/bin"
    # tako は収録に使う実体（バンドル内バイナリ）へリンクする
    ln -sf "$PROMO_CLI" "$PROMO_DEMO/bin/tako"
    local b p
    for b in claude codex agy node tmux git tailscale; do
        p=$(command -v "$b" 2>/dev/null) && ln -sf "$p" "$PROMO_DEMO/bin/$b"
    done
    promo_demo_home_agent_ready
}

# デモ HOME でも実エージェント CLI（claude）が対話セッションを張れるようにする（#470 v3）。
# 背景（実測 2026-07-24）:
#   HOME を差し替えると macOS のキーチェーン検索リストから**ログインキーチェーンが外れる**
#   （`security list-keychains` が System.keychain だけになる）。claude の認証情報は
#   ログインキーチェーンにあるため、デモ HOME では必ず "Not logged in" になり
#   対話セッションが撮れない。
# 対処: デモ HOME 側の検索リスト（デモ HOME の Preferences に書かれる）に、実ユーザーの
#   ログインキーチェーンを指定する。**認証情報のコピー・書き出しは一切しない**
#   （鍵は元の場所のまま。実 HOME 側の設定も変更しない）。
# 併せて、収録中にオンボーディング・信頼ダイアログ・許可プロンプトが出ないよう
# 使い捨て HOME にだけ最小の設定を置く。個人情報（メール・アカウント情報）は書かない。
promo_demo_home_agent_ready() {
    mkdir -p "$PROMO_DEMO/home/Library/Preferences" "$PROMO_DEMO/home/.claude"
    HOME="$PROMO_DEMO/home" security list-keychains -d user \
        -s "$HOME/Library/Keychains/login.keychain-db" >/dev/null 2>&1 || true
    # 共有キーチェーンのトークンを収録中に更新させない（下の promo_ensure_oauth_fresh を参照）
    promo_ensure_oauth_fresh "${TAKO_PROMO_OAUTH_NEED:-900}" || return 1
    if [ -n "$PROMO_CLAUDE_CONFIG_DIR" ]; then
        promo_external_config_ready || return 1
    fi
    /usr/bin/python3 - "$PROMO_DEMO" <<'PY'
import json, os, sys

demo = sys.argv[1]
home = os.path.join(demo, "home")
# 収録で claude が開くディレクトリはすべて事前に信頼済みにしておく
trusted = [
    home,
    os.path.join(home, "Library/Application Support/tako/setup"),
    os.path.join(demo, "awesome-app"),
]
config = {
    "hasCompletedOnboarding": True,
    "theme": "dark",
    "autoUpdates": False,
    "numStartups": 9,
    "projects": {
        path: {"hasTrustDialogAccepted": True, "hasCompletedProjectOnboarding": True}
        for path in trusted
    },
}
with open(os.path.join(home, ".claude.json"), "w") as f:
    json.dump(config, f, indent=1)
# 使い捨て HOME + /private/tmp のデモプロジェクト限定。収録中に許可プロンプトを出さない。
# bypassPermissions は起動時に赤い警告ダイアログ（Enter 待ち）が出て収録に写り込むため使わない
with open(os.path.join(home, ".claude/settings.json"), "w") as f:
    json.dump(
        {
            "permissions": {
                "defaultMode": "acceptEdits",
                "allow": ["Bash", "Read", "Edit", "Write", "Glob", "Grep", "mcp__tako"],
            }
        },
        f,
        indent=1,
    )
PY
}

# デモ HOME の claude が OAuth トークンを**更新しなくて済む**ことを確かめる（2026-09-06 の事故対策）。
# 背景: デモ HOME は実ユーザーのログインキーチェーン（`Claude Code-credentials` の 1 項目）を
#   共有するが、更新の排他（claude が HOME 配下で取る）は共有しない。共有トークンの期限切れの
#   瞬間に本番の claude 群と同時に refresh を打つと負けた側が invalid_grant を受け、claude は
#   **キーチェーンの資格情報を空にして「Login expired」**を出す = ユーザーごとログアウトされる
#   （22:52:24 に mdat が動き accessToken / refreshToken が空・expiresAt が epoch 0 になった。実測）。
# 対策: 収録が終わるまで（$1 秒）トークンが有効なら更新は起きない。足りなければ**実 HOME の claude**
#   （排他つき）に 1 回だけ更新させ、それでも足りなければ止めてユーザーの再ログインを待つ。
#   トークンの値は一切読まない（期限のフィールドだけ）
# 資格情報のキーチェーン項目名。既定 HOME は `Claude Code-credentials`、CLAUDE_CONFIG_DIR を指定した
# claude は `Claude Code-credentials-<そのパスの sha256 先頭 8 桁>`（キーチェーンの項目名と突き合わせて実測）
promo_oauth_service() {
    if [ -n "$PROMO_CLAUDE_CONFIG_DIR" ]; then
        printf 'Claude Code-credentials-%s' "$(printf '%s' "$PROMO_CLAUDE_CONFIG_DIR" | shasum -a 256 | cut -c1-8)"
    else
        printf 'Claude Code-credentials'
    fi
}
promo_oauth_expires_in() {
    security find-generic-password -s "$(promo_oauth_service)" -w 2>/dev/null | /usr/bin/python3 -c '
import sys, json, time
try:
    d = json.loads(sys.stdin.read())
    o = d.get("claudeAiOauth") or {}
    exp = float(o.get("expiresAt") or 0)
    if not o.get("refreshToken"): print(-1); sys.exit()
    exp = exp / 1000 if exp > 1e12 else exp
    print(int(exp - time.time()))
except Exception:
    print(-1)
'
}
promo_ensure_oauth_fresh() {
    local need=${1:-900} left
    left=$(promo_oauth_expires_in)
    local who=${PROMO_CLAUDE_CONFIG_DIR:-既定}
    if [ "${left:-0}" -lt 0 ]; then
        echo "ERROR: Claude Code の資格情報が無い（${who} はログアウト状態）。ユーザーに 'claude auth login' を依頼する" >&2
        return 1
    fi
    if [ "$left" -lt "$need" ]; then
        echo "   OAuth トークンの残り ${left}s < ${need}s → 実 HOME の claude に更新させる（デモ HOME からは更新しない）"
        env "${PROMO_ENV_CLEAN[@]}" ${PROMO_CLAUDE_CONFIG_DIR:+"CLAUDE_CONFIG_DIR=$PROMO_CLAUDE_CONFIG_DIR"} \
            claude -p 'ok' --model haiku --max-turns 1 >/dev/null 2>&1 || true
        left=$(promo_oauth_expires_in)
        if [ "${left:-0}" -lt "$need" ]; then
            echo "ERROR: トークンを更新できない（残り ${left}s）。ユーザーに 'claude auth login' を依頼する" >&2
            return 1
        fi
    fi
    echo "   OAuth トークン残り ${left}s（${who}。収録 ${need}s のあいだ更新は起きない）"
}

# 別アカウントの実 config dir（$PROMO_CLAUDE_CONFIG_DIR）で撮るときの下ごしらえ。
#   - そのアカウントの `.claude.json` にデモプロジェクトの信頼だけを足す（無いと起動時に信頼ダイアログが
#     出て収録が止まる。tako の spawn が worker に対して行う `ensure_trusted_in` と同じ書き込み。
#     他のキーには触らず、書く前の写しを /private/tmp へ残す）
#   - 権限はユーザーの settings.json を汚さず、デモプロジェクト側の `.claude/settings.local.json` で許可する
promo_external_config_ready() {
    local cfg="$PROMO_CLAUDE_CONFIG_DIR/.claude.json"
    [ -f "$cfg" ] || { echo "ERROR: ${cfg} が無い（そのアカウントで一度 claude を起動してログインしておく）" >&2; return 1; }
    mkdir -p /private/tmp/tako-promo-dev
    cp -p "$cfg" "/private/tmp/tako-promo-dev/claude.json.before-$(date +%H%M%S)"
    # 信頼が要るのは**収録で claude が開くディレクトリすべて**。デモプロジェクトだけでは
    # 足りない（#1081 の GUI 章で実測: 「+」で作った新タブの cwd はデモ HOME なので、
    # そこで起動した claude が「Is this a project you trust?」を出して収録が壊れた）
    /usr/bin/python3 - "$cfg" "$PROMO_DEMO/awesome-app" "$PROMO_DEMO/home" <<'PY'
import json, os, sys, tempfile
path, paths = sys.argv[1], sys.argv[2:]
with open(path, encoding="utf-8") as f:
    data = json.load(f)
projects = data.setdefault("projects", {})
added = []
for project in paths:
    entry = projects.setdefault(project, {})
    if entry.get("hasTrustDialogAccepted") and entry.get("hasCompletedProjectOnboarding"):
        continue
    entry["hasTrustDialogAccepted"] = True
    entry["hasCompletedProjectOnboarding"] = True
    added.append(project)
if not added:
    sys.exit(0)
fd, tmp = tempfile.mkstemp(dir=os.path.dirname(path), prefix=".claude.json.")
with os.fdopen(fd, "w", encoding="utf-8") as f:
    json.dump(data, f, indent=2, ensure_ascii=False)
os.replace(tmp, path)
print("   信頼を追加:", " ".join(added))
PY
    mkdir -p "$PROMO_DEMO/awesome-app/.claude"
    cat > "$PROMO_DEMO/awesome-app/.claude/settings.local.json" <<'JSON'
{
  "permissions": {
    "defaultMode": "acceptEdits",
    "allow": ["Bash", "Read", "Edit", "Write", "Glob", "Grep", "mcp__tako"]
  }
}
JSON
}

# 画面に出るテキストへ個人情報（メールアドレス・実ホームパス）が残っていないかを
# ペインのテキストとして検査する。画像の目視より確実な一次防衛線。
# $1 = ペイン ID。見つかったら 1 を返す
promo_pane_has_pii() {
    local pane=$1 text
    text=$(tko read --pane "$pane" 2>/dev/null || true)
    if printf '%s' "$text" | grep -Eq '[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}'; then
        echo "   PII 検出（メールアドレス）: pane $pane" >&2
        return 0
    fi
    if printf '%s' "$text" | grep -q "$HOME"; then
        echo "   PII 検出（実ホームパス）: pane $pane" >&2
        return 0
    fi
    return 1
}

# 対象ペイン群から PII が消えるまで待つ（エージェントの起動バナーは会話が進むと流れる）。
# $1 = 最大待ち秒数、$2.. = ペイン ID。時間内に消えなければ 1（呼び出し側で収録を止める）
promo_wait_pii_clear() {
    local limit=$1; shift
    local waited=0 pane dirty
    while :; do
        dirty=0
        for pane in "$@"; do
            promo_pane_has_pii "$pane" && dirty=1
        done
        [ "$dirty" -eq 0 ] && return 0
        [ "$waited" -ge "$limit" ] && {
            echo "ERROR: ${limit}s 待っても画面から PII が消えない。収録を中止します" >&2
            return 1
        }
        sleep 5
        waited=$((waited + 5))
    done
}

# 収録ウインドウの初期サイズを決める（#1081。YouTube 向けに 16:9 = 960x540pt → Retina で
# 1920x1080px）。tako-app は初回起動時に layout.json の `window` フレームを読んで
# その寸法で開く（TAKO_SELF_TEST 以外）。タブが空のレイアウトは復元段で「空」として
# 拒否され新規ワークスペースになるので、挙動は初回起動と同じでフレームだけが効く。
# 隔離 data_dir にだけ書くので本番の layout.json には触れない
promo_seed_window_frame() {
    local work=$1 w=${2:-960} h=${3:-540}
    local x=${TAKO_PROMO_WIN_X:-240} y=${TAKO_PROMO_WIN_Y:-160}
    mkdir -p "$work/data"
    printf '{"version":1,"active_tab":0,"tabs":[],"window":{"x":%s,"y":%s,"width":%s,"height":%s,"state":"windowed"}}\n' \
        "$x" "$y" "$w" "$h" > "$work/data/layout.json"
}

# **seed だけでは 16:9 にならない**（2026-09-09 実測 / #1149 以降）。
# `initial_window_bounds` は置き先ディスプレイが解決できた検証起動では保存フレームを
# 捨てて「960x600pt をその面の中央へ」置く（`main.rs` の #1141 の分岐）ので、
# layout.json の seed は位置もサイズも効かない。v1〜v3 の素材はこの分岐が入る前に
# 撮ったので 1920x1080px だった。起動後に AX でサイズだけ直す（位置は置き先が正しい）。
# $1 = pid, $2 = 幅pt（既定 960）, $3 = 高さpt（既定 540）
promo_force_window_size() {
    local pid=$1 w=${2:-960} h=${3:-540} i got
    for i in $(seq 1 10); do
        osascript -e "tell application \"System Events\" to tell (first application process whose unix id is $pid) to set size of window 1 to {$w, $h}" \
            >/dev/null 2>&1 || true
        sleep 0.6
        got=$("$PROMO_WINBOUNDS" "$pid" 2>/dev/null | cut -d' ' -f4,5)
        [ "$got" = "$w $h" ] && { echo "   窓サイズ: ${w}x${h}pt"; return 0; }
    done
    echo "ERROR: 窓を ${w}x${h}pt にできない（実測 ${got:-不明}）" >&2
    return 1
}

# 収録用アカウント（`TAKO_PROMO_CLAUDE_CONFIG_DIR`）を隔離インスタンスへ登録する。
#
# **これが無いと、かんたん表示のチャット判定が永久に立たない**（2026-09-09 実測）。
# チャット判定の材料 `live_claude_sessions_by_backend` は `claude agents --json` の
# 出力に乗るが、その走査対象は `agent_scan_targets`（= accounts.yaml + 既定）なので、
# `CLAUDE_CONFIG_DIR` を env で渡しただけのアカウントの会話は 1 件も見えない
# （既定の走査は `CLAUDE_CONFIG_DIR` を外して走る）。登録すると、その config dir でも
# 走査が走る = チャット表示になる（実測: 登録前 terminal のまま / 登録後 25 秒で chat。
# 認識が最悪 30 秒遅れるのは #1011 の UI 鮮度窓）。
# 書き込み先は隔離 data_dir（`tko` が TAKO_DATA_DIR を渡す）なので本番の
# accounts.yaml には触れない
promo_register_recording_account() {
    [ -n "$PROMO_CLAUDE_CONFIG_DIR" ] || return 0
    tko orchestrator accounts add rec --config-dir "$PROMO_CLAUDE_CONFIG_DIR" \
        --description "収録用（#1081）" >/dev/null 2>&1 || {
        echo "ERROR: 収録用アカウントを登録できない（チャット判定が立たない）" >&2
        return 1
    }
    echo "   収録用アカウント登録: rec → $PROMO_CLAUDE_CONFIG_DIR"
}

# タイムライン tsv（explainer-timeline.tsv）を bash の read で安全に読める形へ正規化する。
# bash の `IFS=$'\t' read` はタブを空白類として扱い**連続タブを 1 つに潰す**ので、
# 空欄（カードの source 等）があると列がずれる（実測: op_card の min_dur に "tako" が入った）。
# 空欄を `-` で埋めて 9 列に揃え、コメント行と空行を落として出す。読む側は `-` を空欄として扱う
promo_timeline_rows() {
    awk -F'\t' 'BEGIN{OFS="\t"} /^#/ || NF<2 {next} {for (i=1;i<=9;i++) if ($i=="") $i="-"; NF=9; print}' "$1"
}
promo_tl_field() { [ "$1" = "-" ] && printf '' || printf '%s' "$1"; }

# ── 収録の舞台: 仮想ディスプレイ（#1081・2026-09-06）────────────────
# 隔離 tako の窓をどこへ出すか。
#   virtual（既定）… BetterDisplay の仮想スクリーン $PROMO_VD_NAME へ出す。無ければ作り、
#                    切れていれば繋ぐ。**常設なので作業後も削除・切断しない**（ユーザー指示:
#                    隔離 tako の窓・セルフテスト・デバッグの GUI 検証は今後すべてここへ出す）
#   main           … #470 以来の従来方式（メイン画面に出し、描画維持のため定期的に activate する）。
#                    ユーザーの作業を邪魔するので A/B 用にだけ残す
PROMO_STAGE=${TAKO_PROMO_STAGE:-virtual}
PROMO_VD_NAME=${TAKO_PROMO_VD_NAME:-tako-vd}
# 窓を仮想ディスプレイの左上からこれだけ内側へ置く（座標はポイント）
PROMO_VD_PAD_X=${TAKO_PROMO_VD_PAD_X:-40}
PROMO_VD_PAD_Y=${TAKO_PROMO_VD_PAD_Y:-60}
PROMO_VD_READY=""

# 器（BetterDisplay）の扱いは全プロジェクト共通の `scripts/lib/virtual-display.sh`（#1141 / #1150）
# に任せる: 無ければ作る・切れていれば繋ぐ・同名の増殖を検査する・Main を仮想にしたままにしない。
# BetterDisplay CLI の癖（応答文が当てにならない / tagID が並べ替わる / `-name=` 指定で同じ画面が
# 何枚も繋がる 等）もそちらに集約してある。ここでは「面が用意できた」あとの収録固有の仕事
# （HiDPI の確認・窓の置き場所・窓がその中にあることの検査）だけを持つ
PROMO_VD_LIB="$PROMO_LIB_DIR/../lib/virtual-display.sh"
promo_vd_lib() {
    TAKO_VD_NAME="$PROMO_VD_NAME" bash "$PROMO_VD_LIB" "$@"
}

# 常設の仮想ディスプレイ $PROMO_VD_NAME を使える状態にし、矩形を PROMO_VD_ID / _X / _Y / _W / _H へ
# 入れる。窓の seed 位置（TAKO_PROMO_WIN_X / _Y）が未指定なら他の窓と重ならない空きにする。
# 2 回目以降は何もしない（1 プロセス内で冪等）
promo_vd_prepare() {
    [ -z "$PROMO_VD_READY" ] || return 0
    [ -f "$PROMO_VD_LIB" ] || { echo "ERROR: ${PROMO_VD_LIB} が無い" >&2; return 1; }
    promo_vd_lib ensure || return 1
    local bounds line
    bounds=$(promo_vd_lib bounds) || return 1
    # 同じ矩形の行を CG の一覧から引き、displayID と実ピクセル（HiDPI 判定）を得る
    line=$("$PROMO_DISPLAYS" | awk -v x="${bounds%% *}" -v r="$bounds" '
        { if (($2 " " $3 " " $4 " " $5) == r) { print; found = 1; exit } }
        END { exit found ? 0 : 1 }') || {
        echo "ERROR: 仮想ディスプレイ ${PROMO_VD_NAME}（${bounds}）が CG のディスプレイ一覧と一致しない" >&2
        return 1
    }
    read -r PROMO_VD_ID PROMO_VD_X PROMO_VD_Y PROMO_VD_W PROMO_VD_H PROMO_VD_PXW _ _ <<<"$line"
    # 960x540pt の窓を 1920x1080px で撮るには 2x（HiDPI）が必須
    if [ "$PROMO_VD_PXW" -lt $((PROMO_VD_W * 2)) ]; then
        echo "ERROR: 仮想ディスプレイが HiDPI でない（${PROMO_VD_W}pt = ${PROMO_VD_PXW}px）。" >&2
        echo "       BetterDisplay の ${PROMO_VD_NAME} の設定で HiDPI（2x）の解像度を選ぶこと" >&2
        return 1
    fi
    # 窓の置き場所。常設の画面は他の worker の隔離 tako とも共有するので、既にある窓と
    # 重ならない空きへ置く（重ねられて隠れると GPUI が描画を止め、同じ絵が撮れ続ける）
    if [ -z "${TAKO_PROMO_WIN_X:-}" ] || [ -z "${TAKO_PROMO_WIN_Y:-}" ]; then
        local origin
        origin=$(promo_vd_free_origin "${TAKO_PROMO_WIN_W:-960}" "${TAKO_PROMO_WIN_H:-600}")
        TAKO_PROMO_WIN_X=${origin% *}; TAKO_PROMO_WIN_Y=${origin#* }
    fi
    PROMO_VD_READY=1
    echo "   舞台: 仮想ディスプレイ ${PROMO_VD_NAME}（id=${PROMO_VD_ID} ${PROMO_VD_X},${PROMO_VD_Y} ${PROMO_VD_W}x${PROMO_VD_H}pt / ${PROMO_VD_PXW}px 幅）窓は ${TAKO_PROMO_WIN_X},${TAKO_PROMO_WIN_Y} へ"
}

# 仮想ディスプレイの中で、いま画面に出ている窓（PID を問わない）と重ならない w x h の置き場所を
# 「x y」で返す。左上から右へ、行が埋まれば下へ探し、空きが無ければ左上（重なる）に落ちる。
# 他の worker の隔離 tako と同じ画面を分け合うためのもの
promo_vd_free_origin() {
    local w=$1 h=$2
    "$PROMO_WINBOUNDS" --all 2>/dev/null | awk -v w="$w" -v h="$h" \
        -v X="$PROMO_VD_X" -v Y="$PROMO_VD_Y" -v W="$PROMO_VD_W" -v H="$PROMO_VD_H" \
        -v px="$PROMO_VD_PAD_X" -v py="$PROMO_VD_PAD_Y" '
        { n++; wx[n]=$3; wy[n]=$4; ww[n]=$5; wh[n]=$6 }
        END {
            # 余白の刻みで総当たり（窓幅の刻みだと 1 枚の大きな窓の右の空きを見逃す = 実測）
            for (y = Y + py; y + h <= Y + H; y += py) {
                for (x = X + px; x + w <= X + W; x += px) {
                    ok = 1
                    for (i = 1; i <= n; i++) {
                        if (wx[i] < x + w && x < wx[i] + ww[i] && wy[i] < y + h && y < wy[i] + wh[i]) { ok = 0; break }
                    }
                    if (ok) { print x, y; exit }
                }
            }
            print X + px, Y + py
        }'
}

# 舞台の準備（stage が main なら何もしない）。隔離インスタンスを起こす前に呼ぶ
promo_stage_prepare() {
    case "$PROMO_STAGE" in
        virtual) promo_vd_prepare ;;
        main) return 0 ;;
        *) echo "ERROR: TAKO_PROMO_STAGE は virtual か main（${PROMO_STAGE}）" >&2; return 1 ;;
    esac
}

# 隔離 tako の窓（$1 = pid）が仮想ディスプレイの矩形に収まっているか（0 = 収まっている）。
# 判定に使った矩形は PROMO_WIN_LAST_BOUNDS（"wid x y w h"）に残す
promo_vd_window_inside() {
    local b
    b=$("$PROMO_WINBOUNDS" "$1" 2>/dev/null) || return 1
    PROMO_WIN_LAST_BOUNDS=$b
    echo "$b" | awk -v X="$PROMO_VD_X" -v Y="$PROMO_VD_Y" -v W="$PROMO_VD_W" -v H="$PROMO_VD_H" \
        '{exit ($2>=X && $3>=Y && $2+$4<=X+W && $3+$5<=Y+H) ? 0 : 1}'
}

# 窓を仮想ディスプレイへ移す（System Events の AX 経由。GPUI の窓にも効く = 実測）
promo_vd_move_window() {
    local pid=$1 x=$((PROMO_VD_X + PROMO_VD_PAD_X)) y=$((PROMO_VD_Y + PROMO_VD_PAD_Y))
    osascript -e "tell application \"System Events\" to tell (first application process whose unix id is $pid) to set position of window 1 to {$x, $y}" >/dev/null 2>&1
}

# 窓が仮想ディスプレイの中にあることを確かめ、外なら移す。結果は必ず 1 行残す（受け入れの証拠）
promo_vd_place_window() {
    local pid=$1
    # 窓が見つからない理由の大半は「隔離 tako が居ない」（外から kill された / 起動に失敗した）。
    # 09-06 の実測: 本番 GUI の再起動に巻き込まれて隔離 tako が SIGTERM で落ち、
    # 「窓を移せない（?）」と出て収録が止まった。理由が分かる形で止める
    if ! kill -0 "$pid" 2>/dev/null; then
        echo "ERROR: 隔離 tako（pid ${pid}）が終了している。${PROMO_WORK:-}/app.log と data/persist.log を確認" >&2
        return 1
    fi
    if ! promo_vd_window_inside "$pid"; then
        echo "   窓が仮想ディスプレイの外（${PROMO_WIN_LAST_BOUNDS:-?}）→ 移動"
        promo_vd_move_window "$pid"; sleep 0.5
        promo_vd_window_inside "$pid" || {
            echo "ERROR: 窓を仮想ディスプレイへ移せない（${PROMO_WIN_LAST_BOUNDS:-?}）" >&2
            return 1
        }
    fi
    echo "   窓 ${PROMO_WIN_LAST_BOUNDS} は仮想ディスプレイ ${PROMO_VD_X},${PROMO_VD_Y} ${PROMO_VD_W}x${PROMO_VD_H} の中"
}

# 前面のアプリの pid（System Events）。取れなければ空
promo_frontmost_pid() {
    osascript -e 'tell application "System Events" to get unix id of first application process whose frontmost is true' 2>/dev/null
}

# 隔離 tako は起動時に自分を activate する（tako-app の `cx.activate(true)`）ので、窓が仮想
# ディスプレイ側でもキー入力の宛先が一瞬そちらへ移る。**隔離 tako が前面になっているときだけ**
# 起動前に前面だったアプリ（$1）へ戻す（ユーザーがその間に別のアプリへ移っていたら触らない）
promo_give_back_focus() {
    local prev=$1
    [ -n "$prev" ] && [ "$prev" != "${PROMO_APP_PID:-}" ] || return 0
    [ "$(promo_frontmost_pid)" = "${PROMO_APP_PID:-}" ] || return 0
    osascript -e "tell application \"System Events\" to set frontmost of (first application process whose unix id is $prev) to true" >/dev/null 2>&1 || true
}

# ── 隔離インスタンス ───────────────────────────────────────────────
# $1 = 作業ディレクトリ, $2 = tmux ソケット名, $3 = persist（1 で永続化 ON）
# 追加の環境変数は PROMO_EXTRA_ENV 配列（"KEY=VALUE" 形式）で渡す
promo_start_isolated() {
    local work=$1 socket=$2 persist=${3:-0}
    mkdir -p "$work/discovery" "$work/data"
    # 舞台（仮想ディスプレイ）を先に用意し、呼び出し側が窓の位置を seed していなければ
    # GPUI 既定サイズ（960x600pt）の窓を仮想ディスプレイ側へ seed する = 起動の瞬間から
    # メイン画面に出ない（record-scenes.sh のように seed しない呼び出し側のため）
    promo_stage_prepare || return 1
    if [ "$PROMO_STAGE" = virtual ] && [ ! -f "$work/data/layout.json" ]; then
        promo_seed_window_frame "$work" 960 600
    fi
    local front_before=""
    [ "$PROMO_STAGE" = virtual ] && front_before=$(promo_frontmost_pid)
    (
        cd "$PROMO_DEMO/awesome-app"
        env "${PROMO_ENV_CLEAN[@]}" \
            ${PROMO_EXTRA_ENV[@]+"${PROMO_EXTRA_ENV[@]}"} \
            ${PROMO_CLAUDE_CONFIG_DIR:+"CLAUDE_CONFIG_DIR=$PROMO_CLAUDE_CONFIG_DIR"} \
            TAKO_ISOLATED=1 \
            TAKO_PERSIST="$persist" \
            TAKO_TMUX_SOCKET="$socket" \
            TAKO_DISCOVERY_DIR="$work/discovery" \
            TAKO_DATA_DIR="$work/data" \
            TAKO_REMOTE_STATE_DIR="$work/remote" \
            ZDOTDIR="$PROMO_DEMO/zdot" \
            "$PROMO_APP" >"$work/app.log" 2>&1 &
        echo $! > "$work/app.pid"
    )
    PROMO_APP_PID=$(cat "$work/app.pid")
    PROMO_WORK=$work
    local i
    for i in $(seq 1 80); do
        [ -S "$work/data/tako.sock" ] && [ -s "$work/data/token" ] && break
        sleep 0.5
    done
    if [ ! -S "$work/data/tako.sock" ]; then
        echo "ERROR: 隔離インスタンスの IPC が起動しない（$work/app.log）" >&2
        return 1
    fi
    PROMO_SOCKET_PATH="$work/data/tako.sock"
    PROMO_TOKEN=$(cat "$work/data/token")
    # 起動時の activate で奪われたキーフォーカスを元のアプリへ返す（窓が出た直後と、
    # GPUI の初期化が落ち着いた後の 2 回。奪われていなければ何もしない）
    if [ "$PROMO_STAGE" = virtual ]; then
        for i in $(seq 1 20); do
            "$PROMO_WINBOUNDS" "$PROMO_APP_PID" >/dev/null 2>&1 && break
            sleep 0.25
        done
        promo_give_back_focus "$front_before"
        sleep 3
        promo_give_back_focus "$front_before"
    else
        sleep 3
    fi
}

# アプリだけを止め、器（tmux セッション）は生かしておく（#1081 の再起動復元シーン）。
# promo_stop_isolated は tmux kill-server まで行うので、それを挟むと再起動で
# 「tmux 再 attach 0 / 新規シェル」になり復元の絵が撮れない（実測）
promo_stop_isolated_keep_sessions() {
    if [ -n "${PROMO_APP_PID:-}" ]; then
        kill "$PROMO_APP_PID" 2>/dev/null || true
        local i
        for i in $(seq 1 30); do kill -0 "$PROMO_APP_PID" 2>/dev/null || break; sleep 0.5; done
        kill -9 "$PROMO_APP_PID" 2>/dev/null || true
    fi
    return 0
}

promo_stop_isolated() {
    local socket=$1
    # 既に終了しているプロセスへの kill は失敗するため、すべて || true で受ける
    # （set -e 下でここが非ゼロを返すと呼び出し側の検証まで飛ばされる）
    if [ -n "${PROMO_APP_PID:-}" ]; then
        kill "$PROMO_APP_PID" 2>/dev/null || true
        sleep 1
        kill -9 "$PROMO_APP_PID" 2>/dev/null || true
    fi
    [ -n "$socket" ] && { tmux -L "$socket" kill-server 2>/dev/null || true; }
    return 0
}

# 隔離インスタンスへ明示接続する CLI ラッパー。
# TAKO_DATA_DIR も渡すこと（#470 v3）: `tako orchestrator projects add` のような
# ローカル設定を直接書くサブコマンドは IPC を経由せず自分の data_dir を見るため、
# 渡さないと **本番の projects.yaml / profiles を書き換えてしまう**（実際に汚染した）
tko() {
    env "${PROMO_ENV_CLEAN[@]}" \
        TAKO_SOCKET="$PROMO_SOCKET_PATH" \
        TAKO_TOKEN="$PROMO_TOKEN" \
        TAKO_DATA_DIR="$PROMO_WORK/data" \
        "$PROMO_CLI" "$@"
}

# タブ 1 の先頭ペイン ID
promo_base_pane() {
    tko list | /usr/bin/python3 -c \
        'import json,sys; print(json.load(sys.stdin)["tabs"][0]["panes"][0]["id"])'
}

# ── 収録 ──────────────────────────────────────────────────────────
# 方式: screencapture -l<windowID> による**ウィンドウ単体**の連番キャプチャ →
# ffmpeg で結合。画面全体を撮って切り出す方式（avfoundation + crop /
# screencapture -R）は、収録中に別アプリのウィンドウが対象領域へ重なると
# その中身ごと写り込む（2026-07-23 に個人情報の写り込みが実際に発生）。
# ウィンドウ単体キャプチャなら手前に何が来ても対象ウィンドウの内容しか撮れない。
# screencapture -v（動画）は本環境では黒画面のため使わない。
#
# $1 = 出力 mp4, $2 = 尺（秒）。収録は background で走り promo_record_wait で待つ。
promo_record_start() {
    local out=$1 dur=$2
    if [ "$PROMO_STAGE" = virtual ]; then
        # 仮想ディスプレイの窓は何にも隠れないので activate は要らない（ユーザーのフォーカスも奪わない）。
        # 窓がその画面に収まっていることだけ確かめる（外なら移す）
        promo_vd_place_window "$PROMO_APP_PID" || return 1
    else
        # GPUI のウィンドウは完全に隠れると描画を止める。撮る直前に最前面へ出す
        "$PROMO_WINBOUNDS" "$PROMO_APP_PID" --activate >/dev/null 2>&1 || true
    fi
    # 実際にキャプチャできるウィンドウが現れるまで待ってから収録に入る
    promo_wait_window || return 1
    local wid=$PROMO_WID
    echo "   対象ウィンドウ: id=$wid ${PROMO_WIN_GEOM// /x}（尺 ${dur}s）"
    echo "RECORDING START $(basename "${out%.*}") stage=${PROMO_STAGE} window=${PROMO_WIN_LAST_BOUNDS:-${PROMO_WIN_GEOM}}"

    PROMO_REC_OUT=$out
    PROMO_REC_DUR=$dur
    PROMO_REC_DIR="$PROMO_WORK/frames-raw"
    rm -rf "$PROMO_REC_DIR"; mkdir -p "$PROMO_REC_DIR"
    local marker="$PROMO_WORK/rec.start"
    # ビート（テロップ・ナレーションの同期点。#1081）の基準にするので小数秒で残す
    /usr/bin/python3 -c 'import time; print(time.time())' > "$marker"

    (
        local end=$(( $(date +%s) + dur )) i=0 last="" miss=0
        while [ "$(date +%s)" -lt "$end" ]; do
            i=$((i + 1))
            local f
            f=$(printf '%s/f%05d.png' "$PROMO_REC_DIR" "$i")
            if [ $((i % 20)) -eq 1 ]; then
                if [ "$PROMO_STAGE" = virtual ]; then
                    # 何かの拍子に窓がメイン画面へ動いていたら戻す（activate はしない）
                    promo_vd_window_inside "$PROMO_APP_PID" || promo_vd_move_window "$PROMO_APP_PID" || true
                else
                    # 対象ウィンドウが他のウィンドウの背後に回ると GPUI が描画を止め、
                    # 同じ絵が撮れ続ける。定期的に最前面へ戻して描画を維持する（#470 v2）
                    "$PROMO_WINBOUNDS" "$PROMO_APP_PID" --activate >/dev/null 2>&1 || true
                fi
            fi
            if screencapture -x -o -l"$wid" "$f" 2>/dev/null && [ -s "$f" ]; then
                last=$f; miss=0
            else
                # ウィンドウが一時的に消えた場合は直前フレームで尺を保つ。
                # 連続で撮れないときはウィンドウが作り直された可能性が高いので ID を引き直す
                if [ -n "$last" ]; then cp "$last" "$f" 2>/dev/null || i=$((i - 1))
                else i=$((i - 1)); fi
                miss=$((miss + 1))
                if [ "$miss" -ge 10 ]; then
                    local nb
                    nb=$("$PROMO_WINBOUNDS" "$PROMO_APP_PID" 2>/dev/null || true)
                    [ -n "$nb" ] && wid=$(echo "$nb" | cut -d' ' -f1)
                    miss=0
                fi
            fi
        done
        echo "$i" > "$PROMO_WORK/rec.count"
    ) &
    PROMO_REC_PID=$!
    sleep 0.5
}

# 収録中の「いま何をした」を収録開始からの経過秒つきで記録する（#1081）。
# build-explainer.sh がこの表（<scene>-beats.tsv）を in 点にしてテロップと
# ナレーションを合わせるので、CLI 操作の直前に呼ぶ。収録外で呼ばれたら何もしない
PROMO_BEATS_FILE=${PROMO_BEATS_FILE:-}
promo_beat() {
    local name=$1 marker="$PROMO_WORK/rec.start" t
    [ -n "$PROMO_BEATS_FILE" ] && [ -s "$marker" ] || return 0
    t=$(/usr/bin/python3 -c 'import sys,time; print(f"{time.time()-float(open(sys.argv[1]).read()):.2f}")' "$marker")
    printf '%s\t%s\n' "$name" "$t" >> "$PROMO_BEATS_FILE"
    echo "   beat $name @ ${t}s"
}

promo_record_wait() {
    wait "$PROMO_REC_PID" 2>/dev/null || true
    local n
    n=$(cat "$PROMO_WORK/rec.count" 2>/dev/null || echo 0)
    [ "$n" -gt 0 ] || { echo "ERROR: フレームが 1 枚も撮れていない" >&2; return 1; }
    # 実測フレームレート = 枚数 / 実収録秒。動画尺を実時間に一致させる
    local fps
    fps=$(/usr/bin/python3 -c "print(f'{$n/$PROMO_REC_DUR:.3f}')")
    echo "   $n 枚 / ${PROMO_REC_DUR}s = ${fps} fps → エンコード"
    echo "RECORDING END $(basename "${PROMO_REC_OUT%.*}") frames=${n}"
    rm -f "$PROMO_REC_OUT"
    ffmpeg -hide_banner -loglevel error -framerate "$fps" \
        -i "$PROMO_REC_DIR/f%05d.png" \
        -c:v libx264 -preset medium -crf 18 -pix_fmt yuv420p -r 30 \
        "$PROMO_REC_OUT" >"$PROMO_WORK/ffmpeg.log" 2>&1
    rm -rf "$PROMO_REC_DIR"
}

# ── 検証 ──────────────────────────────────────────────────────────
# $1 = クリップ, $2 = フレーム抽出先ディレクトリ, $3 = 抽出 fps（既定 1）
promo_verify() {
    local clip=$1 fdir=$2 fps=${3:-1}
    echo "-- ffprobe: $(basename "$clip")"
    ffprobe -v error -select_streams v:0 \
        -show_entries stream=width,height,r_frame_rate,duration,codec_name \
        -of default=nw=1 "$clip"
    rm -rf "$fdir"; mkdir -p "$fdir"
    ffmpeg -v error -i "$clip" -vf "fps=$fps" "$fdir/frame-%03d.png"
    local total distinct
    total=$(ls "$fdir" | wc -l | tr -d ' ')
    echo "-- フレーム: ${total} 枚 → $fdir"
    # 「動いていない素材」の検出（#470 v2）。ウィンドウが他のウィンドウに完全に隠れて
    # いる等の理由で描画が止まると、同じ絵が延々と撮れて気づかないまま合成まで進む。
    # 抽出フレームのハッシュ種類数で機械的に弾く
    distinct=$(md5 -q "$fdir"/frame-*.png 2>/dev/null | sort -u | wc -l | tr -d ' ')
    echo "-- 異なるフレーム: ${distinct}/${total}"
    if [ "${total:-0}" -gt 3 ] && [ "$((distinct * 3))" -lt "$total" ]; then
        echo "!! 警告: 素材がほとんど動いていない（${distinct}/${total}）。" >&2
        echo "!! 収録ウィンドウが他のウィンドウに隠れて描画が止まっていた可能性が高い。" >&2
        echo "!! 仮想ディスプレイ（${PROMO_VD_NAME}）が繋がっていて窓がその中にあるか、" >&2
        echo "!! （stage=main なら最前面にあるか）を確かめてから撮り直すこと。" >&2
        return 1
    fi
    # 全黒フレーム（TCC 権限喪失）の自動検出
    local dark
    dark=$( { ffmpeg -v error -i "$clip" -vf "blackdetect=d=0.5:pic_th=0.98" -f null - 2>&1 || true; } \
        | grep -c blackdetect || true)
    if [ "$dark" -gt 0 ]; then
        echo "!! 警告: 全黒区間を検出（画面収録権限を確認すること）" >&2
    fi
}
