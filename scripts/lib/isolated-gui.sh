#!/bin/bash
# isolated-gui.sh — 隔離 GUI（`TAKO_ISOLATED=1` の tako-app）の起動・停止の 1 実装（#1490）
#
# 何のためか:
#   隔離検証スクリプトは「バイナリを決める → 仮想ディスプレイを用意する →
#   窓の env を置く → 起動する → CLI が繋がるまで待つ → 自分の pid だけ落とす」を
#   毎回やる。これが**各スクリプトに手書きで散っている**と、直し方を 1 本足すたびに
#   12 か所へ同じ修正を配る必要があり、実際に配り切れていなかった（#1490 の症状）。
#
#   踏んだ罠はこれ: **蓋閉じ運用の `tako-vd` はアイドルで眠り、眠った面は
#   CoreGraphics の active 一覧から落ちる**。その状態の検証用 GUI は
#   ユーザーの画面へ窓を出さずに**終了コード 4 で終わる**（#1160 の設計。これは正しい）
#   ので、`virtual-display.sh ensure` を**起動の直前に毎回**通していないスクリプトは
#   「GUI が立たない」で止まる。#1487 の worker は同じ検証に 3 回失敗した。
#
#   `ensure` は冪等（常設の面は作り直さない・消す機能は無い）なので、毎回通して困らない。
#   だから「起動する」を 1 本にまとめて、その中に `ensure` を畳み込むのが答え。
#
# **面を用意できなければ起動しない**（#1744）: `ensure` が失敗した・成功と言ったのに面が
#   OS の一覧に無いときは、窓を開かずに終了コード 4（`ISOLATED_GUI_RC_NO_DISPLAY`）で返し、
#   理由を stderr へ 1 行「未実測: …」と出す。以前は「起動は続ける」で素通しする作りで、
#   面の指定を渡さない起動が tako の暗黙の既定（見えている面へ落ちる）に乗って
#   **ユーザーの画面へ窓が出うる**潜在経路があった（この道で窓が出た実例は確認されていない）。
#   蓋閉じ + ディスプレイスリープで面を用意できないのは日常なので、
#   呼び出し側は `launch_isolated_gui … || exit $?` で止め、報告には「未実測」と書く。
#   未配線の機（BetterDisplay が無い・CI）でも同じで、既定の面で続行する道は作らない。
#
# **tako-vd 以外の面の明示は通さない**（#1760）: 面を用意できても、呼び出し側が
#   `TAKO_DISPLAY=0`（= メイン画面）のように別の面を明示すると、以前はそのまま GUI へ渡っていた。
#   いまは起動の前に値を判定し（iso_display_allowed）、通さない値なら窓を開かずに
#   終了コード 2（`ISOLATED_GUI_RC_FOREIGN_DISPLAY`）で返して理由を stderr へ 1 行出す。
#   通すのは tako-vd の名前 / tako-vd の uuid / 空（tako の定義で未指定）/ 実在し得ない
#   index（`index:999` 等 = tako は見失って窓を開かずに終わる）だけ。「わざと当たらない面」を
#   指したい検査（#1697 の ④）は `TAKO_DISPLAY=index:999` を使う。opt-in で別の面を通す口は作らない。
#   判定は**面を起こす前**（#1783。面を用意できない機で書き方の誤りを 4 = 未実測に紛れさせない）。
#   uuid だけは形を起こす前に見て、ensure が記録した値との突き合わせ（iso_display_uuid_recorded）を
#   起こした後に残す（記録は ensure の締めが書くので、起こす前だと初回や面の作り直しで正しい uuid を断る）
#
# **tako-vd の名前に物理画面を渡されたら起動しない**（#1783）: 面の名前の正本は `TAKO_VD_NAME` なので、
#   そこへ内蔵ディスプレイ等の名前を入れると、以前は `ensure` がその面を tako-vd と見なして成功し、
#   ヘルパもその名前を通していた。いまの `ensure` は物理画面なら終了コード 3 で断り、ヘルパはそれを
#   「使い方の誤り」として終了コード 2（`ISOLATED_GUI_RC_NOT_VIRTUAL`）で返して理由を stderr へ 1 行出す
#   （4 = 未実測と読ませない）
#
# **検証用 GUI の印（`TAKO_ISOLATED`）は偽にできない**（#1784）: このヘルパは検証用 GUI 専用の
#   入口なので、GUI へは常に `TAKO_ISOLATED=1` を呼び出し側の `VAR=VAL` より後ろに置いて渡す。
#   以前は既定（`${TAKO_ISOLATED:-1}`）を前に置いていたので、呼び出し側の `TAKO_ISOLATED=0` が
#   そのまま GUI へ届き、検証用 GUI として扱われなかった（面の指定を見失うと、窓を開かずに終わる
#   #1697 の道ではなく既定の面 = ユーザーの画面へ落ちる）。偽（tako が真と読む 1 / true / on 以外。
#   空も含む）を渡されたら窓を開かずに終了コード 2（`ISOLATED_GUI_RC_NOT_ISOLATED`。#1760 と同じ
#   「使い方の誤り」）で返し、理由を stderr へ 1 行出す。真へ書き換えて続行しないのは、偽を求めた
#   検査を隔離の挙動で測って PASS と読ませないため
#
# **隔離 GUI は言語サーバを自動では起こさず・取らない**（#2048）: 編集を始めても rust-analyzer 等を
#   起こさない（ワークスペース全体の `cargo check` で検証の CPU / メモリを汚さない・ネットワークへ出ない）。
#   起こさなかった器は「止めた」に置かれ `tako lsp restart` で起きる。LSP を検証するスクリプトは
#   `TAKO_LSP_AUTOSTART=1`（取るところまで見るなら `TAKO_LSP_AUTO_FETCH=1` も）を export するか並べる
#
# **隔離 GUI は本物の claude・本番の tako へ届かない既定で起こす**（#1970）: 検証用 GUI は
#   本番 tako のペイン（= worker の claude の中）から起こされるので、何もしなければ本番の env を
#   丸ごと受け継ぐ。#1966 の worker はそれで 3 件踏んだ: ① 自動リネームが本物の claude（既定の設定 dir =
#   本物の認証と会話記録）を `claude -p` で起こした ② 本番の mod の env（`CLAUDE_CODE_PLUGIN_DIRS` /
#   `TAKO_CLI`）が一時の claude に載り、本番 tako へ報告を送りうる形になった ③ zsh の初回設定メニューが
#   `tako send` の入力を食った（#1945）。いまは起動ごとに次を既定にする（**呼び出し側が並べた `VAR=VAL` が
#   常に勝つ** = 下の既定はすべて上書きできる）:
#   - 本番 tako と呼び出し元の claude がペインへ配る env（ISOLATED_GUI_DROP_ENV）を GUI へ渡さない。
#     GUI の子（ペイン・自動リネーム・mod）はすべて GUI の env を継ぐので、ここで落とせば全部へ効く
#   - 自動リネームは止める（`TAKO_AUTO_RENAME=0`。命名を見る検証は `TAKO_AUTO_RENAME=1` を並べ、
#     `TAKO_CLAUDE_BIN=<偽の claude>` も並べる。export された値も尊重する）
#   - `CLAUDE_CONFIG_DIR` は本物へ向けない。**export された値は使わない**（worker の env の値は本物の
#     設定 dir）。GUI の HOME が一時の dir なら外すだけ（claude はその HOME の `.claude` を使う）、HOME が
#     利用者のホームのままならログの置き場の隣の一時 dir（ISOLATED_GUI_CLAUDE_CONFIG_DIR）。本物の認証で
#     実物の claude を動かす検証だけが `CLAUDE_CONFIG_DIR="$CCD"` を並べて明示する。
#     **HOME を本物のままにするとペインのシェルは利用者の rc を読む**ので、rc が `CLAUDE_CONFIG_DIR` を
#     export していればペインの claude は本物の設定 dir を使う（GUI の env では止められない）。
#     ペインの claude まで隔離するなら一時の HOME か ZDOTDIR を並べる
#   - GUI の HOME（か ZDOTDIR）が利用者のホームの外で、zsh の rc が 1 つも無ければ既定の `.zshrc`
#     （固定プロンプト。実ユーザー名・ホスト名を画面へ出さない = #927）を置く。**利用者のホームの中には書かない**
#   クリップボードはここでは守れない（GUI のコピーは OS の汎用ペーストボードへ直に書く）ので、
#   Copy を押す確認は隔離 tmux（`set-clipboard off`）で行う（#1963）
#
# 使い方:
#   . scripts/lib/isolated-gui.sh        # 関数として読む（直接実行はしない）
#
#   isolated_gui_bins                    # TAKO_BIN / APP_BIN を決め、今の木から建てる（毎回 cargo に任せる。#1643）
#   isolated_gui_bins visual-test        # visual-test の節を回すなら feature を求める（自前で cargo build しない）
#   launch_isolated_gui "$TMP/app.log" || exit $?   # 印と面の指定の判定 → ensure → uuid の突き合わせ → 起動（pid は $ISOLATED_GUI_PID）
#   wait_isolated_gui                    # `tako list` が通るまで待つ（任意）
#   stop_isolated_gui                    # 自分で起こした pid だけを落とす
#
#   起動ごとに env を足したいときは `VAR=VAL` を並べる（`export` を汚さない）:
#   launch_isolated_gui "$TMP/app-legacy.log" TAKO_1487_LEGACY=1
#
#   既定（#1970）を外すのも並べる形だけ（並べた値が既定に勝つ）:
#   launch_isolated_gui "$TMP/app.log" TAKO_AUTO_RENAME=1 TAKO_CLAUDE_BIN="$FAKE/claude"   # 自動命名を見る
#   launch_isolated_gui "$TMP/app.log" CLAUDE_CONFIG_DIR="$CCD"   # 本物の認証で実物の claude を動かす
#   （GUI へ渡した設定 dir は $ISOLATED_GUI_CLAUDE_CONFIG_DIR に残る。空なら一時の HOME の .claude）
#
# **落とすのは自分で起こした pid だけ**（`pkill -f tako` / `killall tako-app` は
#   本番 `/Applications/tako.app` にも他 worker の隔離インスタンスにも当たる。
#   実際にユーザーの GUI を落とした事故がある）。このファイルに名前一致で殺す道は作らない。
#
# **窓の位置・寸法を AX（System Events）で動かさない**（#1442）。AX は複数の tako-app を
#   unix id にかかわらず同一プロセスとして返すので本番の窓に当たる。指定するなら
#   `ISOLATED_GUI_BOUNDS`（= tako 自身の口 `TAKO_WINDOW_BOUNDS`）を使う。

set -uo pipefail

# 重い処理（下のビルド）を順番待ちへ通す入口 = run_heavy（#1994）
# shellcheck source=queue.sh
. "$(dirname "${BASH_SOURCE[0]}")/queue.sh"

# 起動するバイナリの置き場（**`target/…/tako-app` の直書きをこの 2 行に閉じる**。
# 散らすと「stale な target を掴んでいた」に気づけない = #432 の罠）
# ビルドの種類は `TAKO_ISO_PROFILE`（既定 debug）で選ぶ（#1660。性能を測る検証は
# `TAKO_ISO_PROFILE=release`）。種類を変えても置き場の組み立てはこの 2 行のまま
ISOLATED_GUI_PROFILE=${TAKO_ISO_PROFILE:-debug}
ISOLATED_GUI_APP_REL=${TAKO_ISO_APP_REL:-target/$ISOLATED_GUI_PROFILE/tako-app}
ISOLATED_GUI_CLI_REL=${TAKO_ISO_CLI_REL:-target/$ISOLATED_GUI_PROFILE/tako}

# 窓を出す面。`TAKO_ISOLATED` が立っていれば tako 側の既定も同じ面だが、
# **何も指定していないのか tako-vd を狙っているのかをスクリプトから読めるようにする**。
#
# 面を用意できたら**常に明示して渡す**（#1697 / #1744）: 明示した `TAKO_DISPLAY` を
# tako が見失うと、検証用 GUI は既定の面（= ユーザーの画面）へ落ちずに窓を開かずに終わる。
# 渡さない起動は tako の暗黙の既定（見つからなければ既定の面へ開いて警告を出す）に乗るので、
# このヘルパからは作らない（用意できなければそもそも起動しない = #1744）。
#
# **面を用意する係と同じ名前だけを見る**（#1760）: 以前は `ISOLATED_GUI_DISPLAY` を別に
# 差し替えられたが、`ensure` が用意するのは `TAKO_VD_NAME` の面なので、ずらすと
# 「tako-vd を用意して別の面へ窓を出す」口になる
ISOLATED_GUI_DISPLAY=${TAKO_VD_NAME:-tako-vd}

# 面を用意する係（`virtual-display.sh`）の置き場。**差し替えは番犬のモックのためだけ**
# （`issue1490_isolated_gui_launch_watchdog.rs` が「用意できない面」を注入する口）
ISOLATED_GUI_VD=${ISOLATED_GUI_VD:-}

# 面を用意できずに起動しなかったときの終了コード（#1744）。tako 本体が「窓を開かずに
# 終わる」ときの終了コード（`REFUSED_EXIT_CODE` = 4。#1160 / #1697）と同じ番号にそろえる
ISOLATED_GUI_RC_NO_DISPLAY=4

# tako-vd 以外の面を明示されて起動しなかったときの終了コード（#1760）。**4 とは分ける**:
# 4 は「環境が揃わない = 未実測」、こちらは呼び出し側の書き方の誤り（使い方の誤り = 2）で、
# 未実測と読ませて見逃させない
ISOLATED_GUI_RC_FOREIGN_DISPLAY=2

# 呼び出し側が検証用 GUI の印（`TAKO_ISOLATED`）を偽にして起動しなかったときの終了コード（#1784）。
# 面の指定の誤り（#1760）と同じ「使い方の誤り = 2」
ISOLATED_GUI_RC_NOT_ISOLATED=2

# `TAKO_VD_NAME` が物理画面を指していて起動しなかったときの終了コード（#1783）。これも「使い方の誤り = 2」
ISOLATED_GUI_RC_NOT_VIRTUAL=2

# 面を用意する係の `ensure` が「名指しされた面は物理画面」と断ったときの終了コード（#1783）。
# **virtual-display.sh の VD_RC_NOT_VIRTUAL と同じ番号**（番犬が突き合わせる）
ISOLATED_GUI_VD_RC_NOT_VIRTUAL=3

# uuid の形（小文字で比べる。tako 側の is_uuid_shaped と同じ 8-4-4-4-12）
ISOLATED_GUI_UUID_RE='^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$'

# この番号以上の index は「実在し得ない面」として通す（#1760）。実機の面は 100 枚に届かない
# ので、tako は必ず見失い、明示の見失いとして窓を開かずに終わる（#1697）
ISOLATED_GUI_ABSENT_INDEX_MIN=100

# 窓の矩形（`x,y,w,h` か `w,h`）。**既定は空 = 指定しない**。
# 既定を与えると置き先の中央 960x600 から変わり、窓の実寸を測る検証
# （`test-worker-min-width.sh` の cols）の実測値が動くので、欲しいスクリプトだけが置く
ISOLATED_GUI_BOUNDS=${ISOLATED_GUI_BOUNDS:-}

# `tako list` が通るまで待つ上限（0.1 秒 × この回数）
ISOLATED_GUI_WAIT_TRIES=${ISOLATED_GUI_WAIT_TRIES:-200}

# 直前の `launch_isolated_gui` が起こした pid（呼び出し側はこれを自分の変数へ受ける）
ISOLATED_GUI_PID=""

# 直前の `iso_ensure_display` が面を用意できなかった理由（1 行。用意できたら空）
ISOLATED_GUI_NO_DISPLAY_REASON=""

# 直前の `iso_ensure_display` が「名指しされた面は物理画面」で断られた理由（1 行。それ以外は空。#1783）
ISOLATED_GUI_NOT_VIRTUAL_REASON=""

# 直前の `iso_display_allowed` が面の指定を通さなかった理由（1 行。通したら空）
ISOLATED_GUI_FOREIGN_REASON=""

# GUI へ渡さない env（#1970）。**本番 tako がペインへ配るもの**と**呼び出し元の claude の
# セッションの印**。GUI の子（ペイン・自動リネームの `claude -p`・mod）は GUI の env を継ぐので、
# 残すと検証の claude / tako が本番の GUI・本番の mod・呼び出し元の会話へ繋がる。
# 呼び出し側が `VAR=VAL` を並べたものは落とさない（並べた値が届く）。
# **本番側で注入する env を増やしたら、ここにも足す**（番犬 verification_isolation_watchdog が
# tako-app の spawn_session と tako-core の表から突き合わせて落とす）
ISOLATED_GUI_DROP_ENV=(
    # 制御プレーン（tako-app の spawn_session が積む）
    TAKO_SOCKET TAKO_TOKEN TAKO_MCP_URL TAKO_PANE_ID TAKO_TAB_ID
    # 器の中のシェルへ渡すもの（tako_core::backend::PANE_SCOPED_ENV）
    TAKO_OSC_SINK TAKO_BACKEND_SOCKET
    # tako mod（tako_core::claude_mod::INJECTED_KEYS）
    CLAUDE_CODE_PLUGIN_DIRS TAKO_CLI
    # orchestrator の役割（launch_cmd が起動コマンドへ前置きする）
    TAKO_ORCHESTRATOR_ROLE
    # シェル統合の控え（tako_core::shell_integration::INJECTED_KEYS のうち tako 固有のもの。
    # ZDOTDIR / PROMPT_COMMAND / XDG_DATA_DIRS は利用者も使う名前で、隔離 GUI が自分の値を積み直す）
    TAKO_ORIG_ZDOTDIR
    # 呼び出し元の claude のセッションの印（子セッション扱いで transcript を保存しない・
    # 呼び出し元の会話の連絡口へ繋がる。AGENTS.md の実 claude の e2e と同じ）
    CLAUDECODE CLAUDE_CODE_SESSION_ID CLAUDE_CODE_CHILD_SESSION CLAUDE_CODE_ENTRYPOINT
    CLAUDE_CODE_EXECPATH CLAUDE_CODE_MESSAGING_SOCKET CLAUDE_CODE_MESSAGING_TOKEN
    CLAUDE_CODE_BRIDGE_SESSION_ID CLAUDE_CODE_SESSION_ATTENDED CLAUDE_PID
)

# 直前の `launch_isolated_gui` が GUI へ渡した `CLAUDE_CONFIG_DIR`（#1970。呼び出し側が並べたならその値。
# 空 = 外した = GUI の一時の HOME の `.claude` を claude が使う）
ISOLATED_GUI_CLAUDE_CONFIG_DIR=""

# 利用者のホーム（既定の `.zshrc` を書かない範囲。#1970）。**差し替えは番犬のモックのためだけ**
# （空 = パスワードの登録から引く。$HOME は検証スクリプトが差し替えていることがあるので使わない）
ISOLATED_GUI_REAL_HOME=${ISOLATED_GUI_REAL_HOME:-}

# 既定の `.zshrc` の中身（#1970）。固定プロンプトは既存の検証スクリプトが手で置いていた形と同じ
# （実ユーザー名・ホスト名を画面に出さない = #927）
ISOLATED_GUI_ZSHRC="# isolated-gui.sh が置いた既定の .zshrc（#1970）。zsh の初回設定メニューを出さず、
# 画面へ実ユーザー名・ホスト名を出さない（#927）
PROMPT='tako %1~ %% '
RPROMPT=''"

iso_err() { echo "ERROR: $*" >&2; }

# このファイルから見たリポジトリルート
iso_repo_root() {
    (cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
}

# TAKO_BIN（CLI）と APP_BIN（GUI）を決め、**今の木のソースから、呼び手が求める feature で建てる**（#1643）。
#
#   isolated_gui_bins [feature …]    # feature は tako-app の feature 名（例: visual-test）
#
# **毎回 cargo build を通す**: 以前は 2 本のどちらかが無いときだけ素のビルドを走らせていたので、
#   ① 新しい worktree で先に `--features visual-test` の tako-app を建てると、CLI が無いので素のビルドが
#     それを上書きした（「TAKO_VISUAL_TEST には --features visual-test が必要」で exit 1）
#   ② 2 本が在ればビルドしないので、rebase や checkout で木を進めたあとも前の版で隔離 GUI が立った
#     （「修正後」の段まで NG = 偽の FAIL / 偽の PASS）
#   どちらも「バイナリが在るか」で鮮度と feature を推し量っていたのが原因。鮮度と feature の正本は
#   cargo の fingerprint（ソース・feature・依存）なので判定は cargo に任せる。最新なら cargo は何も
#   建てずに返り、feature ごとの成果物は target の deps に別々に残る（素 ↔ visual-test の行き来は
#   2 回目からリンクの差し替えだけ）。**呼び手は tako-app を自前で建てない**（求める feature は
#   ここへ渡す。番犬 issue1490_isolated_gui_launch_watchdog が落とす。この入口より前の段で CLI だけを
#   使うスクリプトが CLI を建てるのは構わない = tako-app に触れない）
# **呼び出し側が差し替えた TAKO_BIN / APP_BIN は建てない**（`TAKO_BIN=… bash scripts/test-….sh`。
#   既定の置き場を指していれば建てる）。feature は tako-app を建てるときだけ渡す
# **ビルドは利用者のホームで回す**: HOME を一時 dir へ差し替えた後に呼ばれても、ツールチェーンと
#   レジストリを一時 HOME へ取り直さない（iso_real_home が引けなければ HOME のまま）
isolated_gui_bins() {
    local root cli_default app_default home feature features="" build=""
    root=$(iso_repo_root)
    cli_default="$root/$ISOLATED_GUI_CLI_REL"
    app_default="$root/$ISOLATED_GUI_APP_REL"
    TAKO_BIN="${TAKO_BIN:-$cli_default}"
    APP_BIN="${APP_BIN:-$app_default}"
    local -a build_args=()
    [ "$ISOLATED_GUI_PROFILE" = release ] && build_args+=(--release)
    if [ "$TAKO_BIN" = "$cli_default" ]; then
        build_args+=(-p tako-cli)
        build=1
    fi
    if [ "$APP_BIN" = "$app_default" ]; then
        build_args+=(-p tako-app)
        build=1
        for feature in "$@"; do
            features="${features:+$features,}tako-app/$feature"
        done
        [ -n "$features" ] && build_args+=(--features "$features")
    fi
    if [ -n "$build" ]; then
        echo "バイナリを今の木から建てます（${ISOLATED_GUI_PROFILE}・feature: ${*:-なし}。最新なら cargo は何もしない）…"
        home=$(iso_real_home) || home=${HOME-}
        (cd "$root" && HOME="$home" run_heavy cargo build ${build_args[@]+"${build_args[@]}"} --quiet) || {
            iso_err "cargo build ${build_args[*]+${build_args[*]}} が失敗した"
            return 1
        }
    fi
    local b
    for b in "$TAKO_BIN" "$APP_BIN"; do
        [ -x "$b" ] || { iso_err "バイナリが見つからない: $b"; return 1; }
    done
    return 0
}

# 窓の置き先を用意する（眠っていれば起こす）。**起動の直前に毎回通す**のが要点。
# 終了コードは「窓を置ける面を用意できたか」。用意できなければ理由を 1 行
# `ISOLATED_GUI_NO_DISPLAY_REASON` へ置いて非ゼロ（#1744）。
#
# **用意できないときに素通しして続行しない**（#1744）: 以前は配線が無い環境（CI・他人の機）の
# ために「起動は続ける」で返していたが、その起動は面の指定を持たず、tako の暗黙の既定で
# ユーザーの画面へ落ちうる。`ensure` の応答だけを信じず、面が OS の一覧に居ることまで
# `bounds` で読み戻す（器の応答文を当てにしない作法 = virtual-display.sh の癖 ② と同じ）。
#
# `ensure` が「名指しされた面は物理画面」（終了コード ISOLATED_GUI_VD_RC_NOT_VIRTUAL）で断ったときは
# 理由を `ISOLATED_GUI_NOT_VIRTUAL_REASON` にも置く（#1783。呼び手が未実測ではなく使い方の誤りとして断る）
iso_ensure_display() {
    local vd out rc=0 line reason=""
    vd="${ISOLATED_GUI_VD:-$(iso_repo_root)/scripts/lib/virtual-display.sh}"
    ISOLATED_GUI_NO_DISPLAY_REASON=""
    ISOLATED_GUI_NOT_VIRTUAL_REASON=""
    if [ ! -f "$vd" ]; then
        reason="面を用意する ${vd##*/} が見つからない"
    else
        out=$(bash "$vd" ensure 2>&1) || rc=$?
        if [ "$rc" -ne 0 ]; then
            # ensure の理由は最後の ERROR 行（「起こせなかった」「BetterDisplay が無い」等）
            while IFS= read -r line; do
                case "$line" in ERROR:*) reason=${line#ERROR: } ;; esac
            done <<< "$out"
            reason=${reason:-"virtual-display.sh ensure が終了コード ${rc} で失敗した"}
        elif ! bash "$vd" bounds >/dev/null 2>&1; then
            reason="ensure は成功を返したが ${ISOLATED_GUI_DISPLAY} が OS のディスプレイ一覧に無い"
        fi
    fi
    [ -z "$reason" ] && return 0
    ISOLATED_GUI_NO_DISPLAY_REASON=$reason
    [ "$rc" -eq "$ISOLATED_GUI_VD_RC_NOT_VIRTUAL" ] && ISOLATED_GUI_NOT_VIRTUAL_REASON=$reason
    return 1
}

# 前後の空白を落とす（tako も面の指定の前後の空白を無視する）
iso_trim() {
    local s="${1-}"
    s=${s#"${s%%[![:space:]]*}"}
    printf '%s' "${s%"${s##*[![:space:]]}"}"
}

# 面の指定（`TAKO_DISPLAY` の値）を GUI へ渡してよいかを判定する（#1760）。
# 通すなら 0、通さないなら理由を 1 行 `ISOLATED_GUI_FOREIGN_REASON` へ置いて非ゼロ。
#
# 通すのは「tako が tako-vd へ当てる」か「tako がどの面にも当てられない」値だけ
# （tako 側の当て方は crates/tako-core/src/platform/display.rs の select_with）:
#   - 空・空白だけ: tako の定義で未指定（explicit_request）= 検証用の既定で tako-vd を探す
#   - tako-vd の名前（大文字小文字は無視。tako と同じ）
#   - uuid の形の値: **ここでは形だけ見て通し**、`ensure` が記録した値との突き合わせは
#     面を起こした後の iso_display_uuid_recorded が行う（記録と一致しなければそこで断る）
#   - 実在し得ない index（`index:N` / `N` で N >= ISOLATED_GUI_ABSENT_INDEX_MIN）
# **tako-vd を index で指すのは通さない**: index は OS の列挙順で決まり、判定から起動までに
# 面が眠る・繋がると並びがずれて別の面（ユーザーの画面）を指す。当たらないはずの名前
# （`no-such-display` 等）も通さない: その名前の面が無いことをこちらからは確かめられない
# （名前が読めない瞬間もある = #1697）。わざと当たらない面は index で指す。
#
# **面を起こす前に呼ぶ**（#1783）: 係（virtual-display.sh）に何も聞かない = 面を用意できない機
# （CI）でも書き方の誤りを 4（未実測）ではなく 2 で断れる
iso_display_allowed() {
    local spec lower n
    ISOLATED_GUI_FOREIGN_REASON=""
    spec=$(iso_trim "${1-}")
    [ -n "$spec" ] || return 0
    lower=$(printf '%s' "$spec" | tr '[:upper:]' '[:lower:]')
    [ "$lower" = "$(printf '%s' "$ISOLATED_GUI_DISPLAY" | tr '[:upper:]' '[:lower:]')" ] && return 0
    [[ $lower =~ $ISOLATED_GUI_UUID_RE ]] && return 0
    n=${lower#index:}
    case "$n" in
        '' | *[!0-9]*) ;;
        *)
            # 桁が多いものは算術へ通さない（桁あふれ）。`08` を 8 進と読ませないよう 10# を付ける
            if [ "${#n}" -gt 6 ] || [ "$((10#$n))" -ge "$ISOLATED_GUI_ABSENT_INDEX_MIN" ]; then
                return 0
            fi
            ISOLATED_GUI_FOREIGN_REASON="index ${n} は実在しうる面を指す。並びは起動までに変わりうるので ${ISOLATED_GUI_DISPLAY} を指していても通さない"
            return 1
            ;;
    esac
    ISOLATED_GUI_FOREIGN_REASON="${spec} は ${ISOLATED_GUI_DISPLAY} の名前ではない"
    return 1
}

# uuid の形の面の指定が、`ensure` が記録した tako-vd の uuid と一致するか（#1760）。
# 一致すれば 0、しなければ理由を 1 行 `ISOLATED_GUI_FOREIGN_REASON` へ置いて非ゼロ。uuid の形で
# ない値は iso_display_allowed が判定済みなので 0。記録が無ければ確かめられないので通さない。
#
# **面を起こした後に呼ぶ**（#1783 で確かめた）: 記録は `ensure` の締め（vd_record_uuid）が書くので、
# 起こす前に読むと、初回（隔離した HOME で記録がまだ無い = test-display-uuid-1697.sh の形）や
# 面の作り直し（uuid が変わった）で**いまの tako-vd の uuid を断る**。起こした後へ残しても、
# 面を用意できない機ではそもそも起動しない（4）ので、違う uuid が画面へ届く道は無い
iso_display_uuid_recorded() {
    local spec lower recorded vd
    ISOLATED_GUI_FOREIGN_REASON=""
    spec=$(iso_trim "${1-}")
    lower=$(printf '%s' "$spec" | tr '[:upper:]' '[:lower:]')
    [[ $lower =~ $ISOLATED_GUI_UUID_RE ]] || return 0
    # 記録を読むのも面を用意した係（iso_ensure_display と同じ差し替え口）
    vd="${ISOLATED_GUI_VD:-$(iso_repo_root)/scripts/lib/virtual-display.sh}"
    recorded=$(bash "$vd" recorded-uuid 2>/dev/null | tr '[:upper:]' '[:lower:]')
    [ -n "$recorded" ] && [ "$lower" = "$recorded" ] && return 0
    ISOLATED_GUI_FOREIGN_REASON="uuid ${spec} は ensure が記録した ${ISOLATED_GUI_DISPLAY} の uuid と一致しない。記録: ${recorded:-なし}"
    return 1
}

# 面の指定を通さなかったことを stderr へ 1 行出す（#1760。判定は起こす前と、uuid だけ起こした後の 2 か所）
iso_refuse_display() {
    iso_err "TAKO_DISPLAY=${1-} は通さないので検証用 GUI を開かない（${ISOLATED_GUI_FOREIGN_REASON}。ヘルパ経由の起動が通すのは ${ISOLATED_GUI_DISPLAY} の名前か uuid だけで、わざと当たらない面は TAKO_DISPLAY=index:999 で指す。#1760）"
}

# 検証用 GUI の印（`TAKO_ISOLATED` の値）を tako が真と読むかを判定する（#1784）。真なら 0。
# **真の集合は tako の読み方と同じ**（crates/tako-core/src/platform/display.rs の
# is_verification_gui = `1` / `true` / `on`。大文字小文字も前後の空白も区別する = 空も偽）。
# 広げるとヘルパは真と見たのに tako は偽と読む値が生まれ、狭めると正しい値を断る
iso_isolated_allowed() {
    case "${1-}" in
        1 | true | on) return 0 ;;
    esac
    return 1
}

# 実在する dir の物理パス（シンボリックリンクを解く。/tmp → /private/tmp 等）。無ければ非ゼロ
iso_phys_dir() {
    [ -d "${1-}" ] || return 1
    (cd "$1" 2>/dev/null && pwd -P)
}

# 利用者のホームの物理パス（#1970）。**$HOME は使わない**（検証スクリプトが一時 dir へ差し替えている）。
# パスワードの登録から引き、引けなければ非ゼロ（= 既定の .zshrc を書かない側へ倒す）
iso_real_home() {
    local user home
    if [ -n "$ISOLATED_GUI_REAL_HOME" ]; then
        iso_phys_dir "$ISOLATED_GUI_REAL_HOME"
        return
    fi
    user=$(id -un 2>/dev/null) || return 1
    # eval へ渡すのはユーザー名の形の文字だけ（チルダ展開 = getpwnam を使うため）
    case "$user" in '' | *[!A-Za-z0-9._-]*) return 1 ;; esac
    eval "home=~$user"
    case "$home" in '~'*) return 1 ;; esac
    iso_phys_dir "$home"
}

# dir が実在し、利用者のホーム（とその中）の外にあれば 0（#1970）。確かめられなければ非ゼロ
# （= 「利用者のホームかもしれない」側へ倒す）
iso_outside_real_home() {
    local dir real
    dir=$(iso_phys_dir "${1-}") || return 1
    real=$(iso_real_home) || return 1
    case "$dir/" in "$real"/*) return 1 ;; esac
    return 0
}

# GUI へ届く値（#1970）: 並べた `NAME=…` の最後 → export された値。名前は HOME / ZDOTDIR だけ
iso_gui_env() {
    local name="$1" value="" arg
    shift
    case "$name" in
        HOME) value=${HOME-} ;;
        ZDOTDIR) value=${ZDOTDIR-} ;;
        *) return 1 ;;
    esac
    for arg in "$@"; do
        case "$arg" in "$name"=*) value=${arg#"$name"=} ;; esac
    done
    printf '%s' "$value"
}

# GUI のシェルが rc を読む dir に既定の `.zshrc` を置く（#1970）。
#
#   iso_default_zshrc [VAR=VAL …]   （launch_isolated_gui の引数をそのまま渡す）
#
# 読む dir は GUI へ届く `ZDOTDIR`、無ければ `HOME`（どちらも iso_gui_env の順）。
# tako のシェル統合は `.zshenv` で ZDOTDIR を呼び出し側の値へ戻すので、zsh が rc を探すのはここ。
# 書くのは **dir が実在し・利用者のホームの外で・zsh の rc（.zshenv / .zprofile / .zshrc / .zlogin）が
# 1 つも無い**ときだけ（1 つでもあれば zsh の初回設定メニューは出ない。呼び出し側の rc を上書きしない）。
# 書いたら 0、書かなかったら 1（理由は問わない）
iso_default_zshrc() {
    local dir f
    dir=$(iso_gui_env ZDOTDIR "$@")
    [ -n "$dir" ] || dir=$(iso_gui_env HOME "$@")
    iso_outside_real_home "$dir" || return 1
    dir=$(iso_phys_dir "$dir") || return 1
    for f in .zshenv .zprofile .zshrc .zlogin; do
        [ -e "$dir/$f" ] && return 1
    done
    printf '%s\n' "$ISOLATED_GUI_ZSHRC" > "$dir/.zshrc" || return 1
    return 0
}

# 隔離 GUI を起こす。pid は $ISOLATED_GUI_PID へ置く（**`$( )` で受けない**:
# 副シェルの子になると呼び出し側の `wait` が「子ではない」で失敗する）。
#
#   launch_isolated_gui <ログの置き場> [VAR=VAL …]
launch_isolated_gui() {
    local log="${1:-}"
    [ -n "$log" ] || { iso_err "launch_isolated_gui にはログの置き場が要る"; return 1; }
    shift
    [ -n "${APP_BIN:-}" ] || { iso_err "APP_BIN が空（先に isolated_gui_bins を呼ぶ）"; return 1; }

    # 検証用 GUI の印は偽にさせない（#1784）。値は面の指定と同じく 1 つに決めてから判定する:
    # 呼び出し側が並べた `TAKO_ISOLATED=`（最後のものが勝つ = env と同じ）→ export された値
    # （空も「偽を渡した」と読む。tako が空を偽と読むので）。どちらも無ければ判定しない。
    # **面を起こす前に**見る: 書き方の誤りを、面を用意できない機（CI）で 4 = 未実測に紛れさせない
    local isolated="" isolated_from="" arg
    [ -n "${TAKO_ISOLATED+x}" ] && { isolated=$TAKO_ISOLATED; isolated_from="export"; }
    for arg in "$@"; do
        case "$arg" in TAKO_ISOLATED=*) isolated=${arg#TAKO_ISOLATED=}; isolated_from="launch_isolated_gui の引数" ;; esac
    done
    if [ -n "$isolated_from" ] && ! iso_isolated_allowed "$isolated"; then
        iso_err "TAKO_ISOLATED='${isolated}'（${isolated_from}）は tako では偽と読むので検証用 GUI を開かない（真は 1 / true / on だけ。ヘルパ経由の起動は常に TAKO_ISOLATED=1 で起こし、偽にする口は作らない。#1784）"
        return "$ISOLATED_GUI_RC_NOT_ISOLATED"
    fi

    # 面の指定は常に明示する = tako は見失ったら既定の面へ落ちずに窓を開かずに終わる（#1697）。
    # 値は 1 つに決めてから判定する: 呼び出し側が並べた `TAKO_DISPLAY=`（最後のものが勝つ =
    # env と同じ）→ export された TAKO_DISPLAY（空は未指定扱い）→ tako-vd
    local display="${TAKO_DISPLAY:-$ISOLATED_GUI_DISPLAY}"
    for arg in "$@"; do
        case "$arg" in TAKO_DISPLAY=*) display=${arg#TAKO_DISPLAY=} ;; esac
    done
    # **tako-vd 以外の面の明示は通さない**（#1760。通すとユーザーの画面へ窓が出る）。
    # これも**面を起こす前に**見る（#1783。印の判定と同じ理由で、4 = 未実測に紛れさせない）
    if ! iso_display_allowed "$display"; then
        iso_refuse_display "$display"
        return "$ISOLATED_GUI_RC_FOREIGN_DISPLAY"
    fi

    # 面を起こす。**用意できなければ起動しない**（#1744）。呼び出し側が `TAKO_DISPLAY` を
    # 明示していても同じ（狙いが tako-vd なら見失っているし、別の面ならユーザーの画面へ出す）。
    # ISOLATED_GUI_PID は触らない（先に起こした GUI を trap の後片付けから外さないため）
    if ! iso_ensure_display; then
        # 名指しされた面が物理画面（#1783）= 環境ではなく TAKO_VD_NAME の渡し方の誤り
        if [ -n "$ISOLATED_GUI_NOT_VIRTUAL_REASON" ]; then
            iso_err "TAKO_VD_NAME=${ISOLATED_GUI_DISPLAY} は仮想ディスプレイではないので検証用 GUI を開かない（${ISOLATED_GUI_NOT_VIRTUAL_REASON}。TAKO_VD_NAME は仮想ディスプレイの名前だけ = #1783）"
            return "$ISOLATED_GUI_RC_NOT_VIRTUAL"
        fi
        echo "未実測: ${ISOLATED_GUI_DISPLAY} を用意できないので検証用 GUI を開かない（${ISOLATED_GUI_NO_DISPLAY_REASON}。状態は scripts/lib/virtual-display.sh status。#1744）" >&2
        return "$ISOLATED_GUI_RC_NO_DISPLAY"
    fi

    # uuid の指定だけは面を起こした後で記録と突き合わせる（記録は ensure の締めが書く。#1760 / #1783）
    if ! iso_display_uuid_recorded "$display"; then
        iso_refuse_display "$display"
        return "$ISOLATED_GUI_RC_FOREIGN_DISPLAY"
    fi
    # 本番 tako と呼び出し元の claude の env は GUI へ渡さない（#1970）。`env -u` は代入より先に
    # 効くので、呼び出し側が同じ名前を `VAR=VAL` で並べればその値が届く
    local -a drop_args=()
    local key
    for key in ${ISOLATED_GUI_DROP_ENV[@]+"${ISOLATED_GUI_DROP_ENV[@]}"}; do
        drop_args+=(-u "$key")
    done

    local -a env_args=()
    [ -n "$ISOLATED_GUI_BOUNDS" ] && \
        env_args+=("TAKO_WINDOW_BOUNDS=${TAKO_WINDOW_BOUNDS:-$ISOLATED_GUI_BOUNDS}")
    # 自動リネームは既定で止める（#1970。本物の claude を `claude -p` で起こす経路）。export された値は尊重する
    env_args+=("TAKO_AUTO_RENAME=${TAKO_AUTO_RENAME:-0}")

    # claude の設定 dir は本物へ向けない（#1970）。**export された値は使わない**（worker の env の値は
    # 本物の設定 dir）: 並べた値 → GUI の HOME が利用者のホームの外（一時の HOME）なら外すだけ
    # （claude はその HOME の .claude / .claude.json を使う = 一時の HOME に会話や資格情報を置く
    # 検証の形をそのまま保つ）→ HOME が利用者のホームのままならログの置き場の隣の一時 dir
    local claude_config="" claude_config_set=""
    for arg in "$@"; do
        case "$arg" in CLAUDE_CONFIG_DIR=*) claude_config=${arg#CLAUDE_CONFIG_DIR=}; claude_config_set=1 ;; esac
    done
    if [ -z "$claude_config_set" ]; then
        if iso_outside_real_home "$(iso_gui_env HOME "$@")"; then
            drop_args+=(-u CLAUDE_CONFIG_DIR)
        else
            local log_dir
            log_dir=$(iso_phys_dir "$(dirname "$log")") || { iso_err "ログの置き場の dir が無い: $log"; return 1; }
            claude_config="$log_dir/isolated-claude-config"
            mkdir -p "$claude_config" || { iso_err "一時の claude の設定 dir を作れない: $claude_config"; return 1; }
            env_args+=("CLAUDE_CONFIG_DIR=${claude_config}")
        fi
    fi
    ISOLATED_GUI_CLAUDE_CONFIG_DIR=$claude_config

    # zsh の初回設定メニューが入力を食わないよう、一時の HOME へ既定の .zshrc（#1970 / #1945）
    iso_default_zshrc "$@" || true
    # 呼び出し側が並べた `VAR=VAL` は既定より後ろ = そちらが勝つ。
    # 検証用 GUI の印と面の指定だけは最後に置く = GUI へ届くのは必ず `TAKO_ISOLATED=1` と
    # 判定を通った面の指定（判定をすり抜ける道が増えても、印を偽にして届ける道は無い。#1760 / #1784）
    env ${drop_args[@]+"${drop_args[@]}"} ${env_args[@]+"${env_args[@]}"} "$@" "TAKO_ISOLATED=1" "TAKO_DISPLAY=${display}" "$APP_BIN" > "$log" 2>&1 &
    ISOLATED_GUI_PID=$!
    return 0
}

# CLI が繋がるまで待つ。繋がらなければログの末尾を出して非ゼロ
#
#   wait_isolated_gui [ログの置き場] [試行回数]
wait_isolated_gui() {
    local log="${1:-}" tries="${2:-$ISOLATED_GUI_WAIT_TRIES}" i
    [ -n "${TAKO_BIN:-}" ] || { iso_err "TAKO_BIN が空（先に isolated_gui_bins を呼ぶ）"; return 1; }
    for i in $(seq 1 "$tries"); do
        if "$TAKO_BIN" list >/dev/null 2>&1; then return 0; fi
        sleep 0.1
    done
    echo "tako-app へ接続できない:"
    [ -n "$log" ] && [ -f "$log" ] && tail -20 "$log"
    return 1
}

# 自分で起こした隔離 GUI を落とす。**明示 pid だけ**（名前一致では殺さない）
#
#   stop_isolated_gui [pid]
stop_isolated_gui() {
    local pid="${1:-$ISOLATED_GUI_PID}" i
    [ -n "$pid" ] || return 0
    kill "$pid" 2>/dev/null || true
    # 素直に終わるのを待ってから、居座るときだけ強く落とす
    for i in $(seq 1 50); do
        kill -0 "$pid" 2>/dev/null || break
        sleep 0.1
    done
    kill -0 "$pid" 2>/dev/null && kill -9 "$pid" 2>/dev/null
    wait "$pid" 2>/dev/null || true
    [ "$pid" = "${ISOLATED_GUI_PID:-}" ] && ISOLATED_GUI_PID=""
    return 0
}

# 直接実行されたときは使い方だけ出す（**起動の口は持たせない**: 隔離の env は
# 呼び出し側のスクリプトが組み立てるもので、ここから出すと中途半端な隔離で本番を触る）
if [ "${BASH_SOURCE[0]}" = "${0}" ]; then
    echo "使い方: . ${0}   # source して関数を使う（直接実行する口は無い）" >&2
    echo "  isolated_gui_bins [feature…]         TAKO_BIN / APP_BIN を決め、今の木から求める feature で建てる（毎回 cargo に任せる）" >&2
    echo "  launch_isolated_gui <log> [VAR=VAL…] 仮想ディスプレイを起こして隔離 GUI を起動（用意できなければ起動せず 4・tako-vd 以外の面の指定と偽の TAKO_ISOLATED と物理画面を指す TAKO_VD_NAME は起動せず 2）" >&2
    echo "  wait_isolated_gui [log] [試行回数]    tako list が通るまで待つ" >&2
    echo "  stop_isolated_gui [pid]              自分で起こした pid だけを落とす" >&2
    exit 1
fi
