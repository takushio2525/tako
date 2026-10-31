#!/bin/bash
# test-wrapped-links-2578.sh — 折り返しをまたぐ URL / パスを `tako links` が 1 本で返すかを実経路で測る（#2578）
#
# 何を測るか:
#   隔離 GUI のペインへ長い URL / パスを出し、`tako links --pane`（GUI の cmd+クリック・ホバー・
#   cmd+右クリックのメニューと同じ `tako_core::links` の 1 実装）が**全体を 1 本のリンク**として
#   返すかを読む。ペインは 2 種類（直結 = TAKO_PERSIST=0 / 器 = TAKO_PERSIST=1 の tmux 越し）、
#   形は 4 つ:
#     soft   端末の自動折り返し（1 行で出した長い URL / パスを端末が幅で折る。3 行以上・全角の境目つき）
#     pair   行末ちょうど幅いっぱいで終わる別々の URL 2 本（1 本へ融合させない）
#     app    アプリ自身の折り返し（claude と同じ「行頭の印 + 右端まで詰めて改行 + 2 桁の字下げ」を偽の出力で作る）
#     claude 実物の Claude Code（一時の設定 dir + 偽 Messages API）に URL / パスをそのまま答えさせる
#   加えて、出した後に窓の幅を変えた直後（折り返し位置が変わる）でも 1 本で返るかを見る。
#
# 判定: 期待する全体（URL / 実在するパス）がリンクの target に在れば OK。全体の途中で切れた
#   target（期待の真の接頭辞）が出たら NG（= #2578 の症状）。
#
# A/B: `TAKO_2578_LEGACY=1` を GUI へ渡すと修正前の結合（右端まで埋まった行だけ直結）へ戻る。
#   `ARM=legacy` で旧挙動の腕を走らせる（旧挙動では app / claude で NG が出るのが正しい = 検出力の実証）。
#
# 使い方: bash scripts/test-wrapped-links-2578.sh   （ONLY=direct|persist / ARM=legacy / KEEP=1 で一時 dir を残す）
# **CI には登録しない**（仮想ディスプレイと実物の claude が要る）。CI 側の担保は tako-core の単体
# （links.rs の #2578 の節）と番犬（issue2578_wrapped_link_watchdog）。
# 落とすのは自分で起こした pid と自分の tmux ソケットだけ（名前一致では殺さない）。
set -uo pipefail

# 本番 GUI を指す env を最初に落とす（#1449 / #1450 / #1970）
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_ORCHESTRATOR_ROLE TAKO_MCP_URL \
  CLAUDE_CODE_PLUGIN_DIRS TAKO_CLI TAKO_ORCHESTRATOR_DIR TAKO_DISCOVERY_DIR TAKO_2578_LEGACY
unset CLAUDE_CODE_CHILD_SESSION CLAUDE_CODE_SESSION_ID CLAUDECODE CLAUDE_CODE_ENTRYPOINT
# 自分の claude が使っている本物の設定 dir・認証を混ぜない
unset CLAUDE_CONFIG_DIR ANTHROPIC_API_KEY ANTHROPIC_AUTH_TOKEN ANTHROPIC_BASE_URL
export LANG=en_US.UTF-8 LC_ALL=en_US.UTF-8
export TAKO_ISOLATED=1

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ONLY="${ONLY:-}"
ARM="${ARM:-new}"
PASS=0
FAIL=0
UNMEASURED=0
pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
fail() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }
unmeasured() { UNMEASURED=$((UNMEASURED + 1)); echo "  [未実測] $1"; }
want() { [ -z "$ONLY" ] || [ "$ONLY" = "$1" ]; }

# 置き場は短く（ソケットのパス長の上限）。
# claude の信頼は実パスで登録するので /tmp ではなく /private/tmp の形で持つ
TMP="$(cd "$(mktemp -d /tmp/t2578-XXXXXX)" && pwd -P)"
SOCK="t2578-$$"
FAKE_PID=""
cleanup() {
  stop_isolated_gui
  [ -n "$FAKE_PID" ] && kill "$FAKE_PID" 2>/dev/null
  # 自分の隔離ソケットの tmux サーバーだけを、ソケットファイルまで落とす（本番の -L tako には触れない）
  tmux_socket_kill "$SOCK"
  if [ -z "${KEEP:-}" ]; then rm -rf "${TMP:?}"; else echo "一時 dir: $TMP"; fi
}

# shellcheck source=lib/tmux-socket.sh
. "$REPO_ROOT/scripts/lib/tmux-socket.sh"
# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
isolated_gui_bins || exit 1
trap cleanup EXIT
# 隔離の置き場はビルドの後で置く（先に置くと順番待ちが本番を見失って素通りする = #1705）
export TAKO_DATA_DIR="$TMP/d" TAKO_DISCOVERY_DIR="$TMP/d"

# --- 材料: 実在する長いパス 2 本と 3 行以上にまたがる URL 1 本 -----------------------------------
W="$TMP/w"
LONG_DIR="$W/a-very-long-directory-name-for-the-wrap-test-2578/another-quite-long-directory-segment-0123456789/third-level-folder"
LONG_PATH="$LONG_DIR/target-file-for-the-wrapped-link-check.txt"
WIDE_DIR="$W/日本語のフォルダ名を長くしたもの/さらに深い階層のフォルダ/もう一段深い階層の長いフォルダ名"
WIDE_PATH="$WIDE_DIR/資料（最新）の長いファイル名です.txt"
mkdir -p "$LONG_DIR" "$WIDE_DIR" "$TMP/home" "$TMP/zdot" "$TMP/work"
: > "$LONG_PATH"
: > "$WIDE_PATH"
A40=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
B60=bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb
C80=cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc
URL="https://example.com/tako/issues/2578/wrap-check?first=$A40&second=$B60&third=$C80&fourth=$A40#end-of-url"
# 証拠ログに実ユーザー名・実ホスト名を写さない（#927）
printf "PROMPT='tako %%%% '\nRPROMPT=''\n" > "$TMP/zdot/.zshrc"

# アプリ自身の折り返し（claude と同じ形）を作る: 先頭行は印 `* ` + 前置き、以降は 2 桁の字下げで
# 右端まで詰める。全角は残り 1 桁に入らなければ次の行へ送る（claude の返答と同じ）
app_wrap() { # $1 = 桁数 / $2 = 前置き / $3 = 折る語 / $4 = 後置き
  perl -CSA -e '
    my ($cols, $lead, $tok, $tail) = @ARGV;
    sub cw { ord($_[0]) < 0x80 ? 1 : 2 }
    my $row = "* $lead "; my $vis = length($row);
    for my $ch (split //, $tok) {
      if ($vis + cw($ch) > $cols) { print "$row\n"; $row = "  "; $vis = 2; }
      $row .= $ch; $vis += cw($ch);
    }
    if ($vis + 1 + length($tail) > $cols) { print "$row\n"; $row = "  $tail"; } else { $row .= " $tail"; }
    print "$row\n";
  ' "$1" "$2" "$3" "$4"
}

tk() { "$TAKO_BIN" "$@"; }
wait_for() {
  local limit="$1" i
  shift
  for i in $(seq 1 "$limit"); do
    if "$@"; then return 0; fi
    sleep 1
  done
  return 1
}
# 画面に $2 が出ているか（pipefail の下で早く抜ける読み手へパイプしない = 一度受けてから見る）
screen_has() {
  local out
  out="$(tk read --pane "$1" 2>/dev/null)" && [[ "$out" == *"$2"* ]]
}
# claude の答えが出たか（入力の写しと答えで END-CLAUDE が 2 回）
claude_answered() {
  local out
  out="$(tk read --pane "$1" 2>/dev/null)" || return 1
  [ "$(grep -c END-CLAUDE <<<"$out")" -ge 2 ]
}

# links の結果を判定する。$1 = 見出し / $2 = ペイン / $3 = 結果の置き場 / 残り = 期待の全体（URL / パス）
judge() {
  local label="$1" pane="$2" out="$3" want got cols short rows name
  shift 3
  tk links --pane "$pane" > "$out" 2>&1 || { fail "$label: tako links が失敗（$(head -c 200 "$out")）"; return; }
  cols="$(jq -r '.cols' "$out")"
  for want in "$@"; do
    got="$(jq -r --arg w "$want" '[.links[] | select(.target == $w)] | length' "$out")"
    # 期待の真の接頭辞（途中で切れた target）
    short="$(jq -r --arg w "$want" '[.links[] | .target as $t | select($t != $w and ($w | startswith($t)))] | length' "$out")"
    rows="$(jq -r --arg w "$want" '[.links[] | select(.target == $w) | .spans | length] | max // 0' "$out")"
    case "$want" in
      "$URL") name=URL ;;
      "$LONG_PATH") name=長いパス ;;
      "$WIDE_PATH") name=全角のパス ;;
      *) name="${want%%/x*}" ;;
    esac
    if [ "${got:-0}" -ge 1 ] && [ "${short:-0}" -eq 0 ]; then
      pass "$label: $name が 1 本で返る（cols=${cols}・${rows} 行・wrap=$(jq -r --arg w "$want" '[.links[] | select(.target == $w) | .wrap // "null"] | unique | join(",")' "$out")）"
    else
      fail "$label: $name が切れる / 無い（cols=${cols}・全体 ${got:-0} 本・途中で切れた target ${short:-0} 本）"
      jq -r --arg w "$want" '.links[] | .target as $t | select($t != $w and ($w | startswith($t))) | $t' "$out" \
        | sed "s#^$TMP#TMP#; s/^/      | 切れた target: /" | head -3
    fi
  done
}

# 幅ちょうどの URL（`https://<host>/x` + 埋め草で桁数ぴったり）
exact_url() { # $1 = ホスト / $2 = 桁数
  local head="https://$1/x" fill
  fill="$(printf '%*s' $(($2 - ${#head})) '' | tr ' ' 'y')"
  printf '%s%s' "$head" "$fill"
}

# --- 偽 Messages API（claude の形）------------------------------------------------------------
REAL_CLAUDE="$(command -v claude 2>/dev/null || true)"
API_URL=""
if [ -n "$REAL_CLAUDE" ] && command -v node >/dev/null 2>&1; then
  node "$REPO_ROOT/scripts/lib/fake-messages-api.mjs" 0 > "$TMP/fake.out" 2>&1 &
  FAKE_PID=$!
  for _ in $(seq 1 50); do
    PORT="$(sed -n 's/^FAKE_API_PORT=//p' "$TMP/fake.out" 2>/dev/null)"
    [ -n "$PORT" ] && break
    sleep 0.1
  done
  [ -n "${PORT:-}" ] && API_URL="http://127.0.0.1:${PORT}"
fi
# ダミーの API キー（本物ではない。一時の .claude.json で末尾 20 字を承認済みにする）
API_KEY="sk-ant-api03-fake2578fake2578fake2578fake2578fake2578fake2578AA"
CCD="$TMP/cfg"
mkdir -p "$CCD"
jq -n --arg key "$(printf '%s' "$API_KEY" | tail -c 20)" --arg work "$TMP/work" '{
  hasCompletedOnboarding: true, theme: "dark",
  customApiKeyResponses: { approved: [$key], rejected: [] },
  projects: { ($work): { hasTrustDialogAccepted: true, hasCompletedProjectOnboarding: true } }
}' > "$CCD/.claude.json"

run_mode() { # $1 = direct | persist
  local mode="$1" persist=0 legacy_env=() p cols cols2 q
  [ "$mode" = persist ] && persist=1
  [ "$ARM" = legacy ] && legacy_env=(TAKO_2578_LEGACY=1)
  echo "== ${mode}（TAKO_PERSIST=${persist}・腕 ${ARM}）"
  ISOLATED_GUI_BOUNDS=0,0,1000,800 launch_isolated_gui "$TMP/$mode.log" \
    HOME="$TMP/home" ZDOTDIR="$TMP/zdot" SHELL=/bin/zsh \
    TAKO_PERSIST="$persist" TAKO_AUTO_RENAME=0 \
    TAKO_SESSIONS_FILE="$TMP/sessions-$mode.yaml" TAKO_PANE_LOG_DIR="$TMP/panelogs-$mode" \
    TAKO_TMUX_SOCKET="$SOCK" ${legacy_env[@]+"${legacy_env[@]}"} || exit $?
  if ! wait_isolated_gui "$TMP/$mode.log" 300; then
    fail "$mode: 隔離 GUI の CLI が繋がらない"
    stop_isolated_gui
    return 1
  fi
  p="$(tk tab new --focus 2>/dev/null | jq -r '.pane // empty')"
  [ -n "$p" ] || { fail "$mode: タブを作れない"; stop_isolated_gui; return 1; }
  wait_for 30 screen_has "$p" 'tako %' || unmeasured "$mode: プロンプトが出ない（続行）"
  cols="$(tk links --pane "$p" | jq -r '.cols')"
  echo "  ペインの桁数: $cols"

  # soft: 1 行で出して端末に折らせる。全角のパスは前置きの桁を 1 つずつずらして 3 回出す
  # （全角が右端にかかる 2 形 = 右端まで埋める / 残り 1 桁に入らず送る、のどちらかを踏む）
  printf 'URL: %s\n\nPATH: %s\n\nW: %s\nWW: %s\nWWW: %s\n\nEND-SOFT\n' \
    "$URL" "$LONG_PATH" "$WIDE_PATH" "$WIDE_PATH" "$WIDE_PATH" > "$TMP/soft.txt"
  tk send --pane "$p" "clear; cat '$TMP/soft.txt'" >/dev/null
  wait_for 15 screen_has "$p" END-SOFT || unmeasured "$mode soft: 出力が出ない"
  judge "$mode soft" "$p" "$TMP/$mode-soft.json" "$URL" "$LONG_PATH" "$WIDE_PATH"
  local wide_n
  wide_n="$(jq -r --arg w "$WIDE_PATH" '[.links[] | select(.target == $w)] | length' "$TMP/$mode-soft.json")"
  [ "${wide_n:-0}" -eq 3 ] && pass "$mode soft: 全角のパス 3 本（前置き 3 通り）がすべて 1 本ずつ返る" \
    || fail "$mode soft: 全角のパスが ${wide_n:-0} / 3 本しか返らない"

  # pair: 行末ちょうど幅いっぱいで終わる別々の 2 本（1 本へ融合させない）。2 本目の後ろは空行
  # （幅ちょうどの行のすぐ次に地の文を置くと、端末の折り返しと見分けられない = 修正前からの限界）
  local u1 u2
  u1="$(exact_url a.example.com "$cols")"
  u2="$(exact_url b.example.com "$cols")"
  printf '%s\n%s\n\nEND-PAIR\n' "$u1" "$u2" > "$TMP/pair.txt"
  tk send --pane "$p" "clear; cat '$TMP/pair.txt'" >/dev/null
  wait_for 15 screen_has "$p" END-PAIR || unmeasured "$mode pair: 出力が出ない"
  judge "$mode pair" "$p" "$TMP/$mode-pair.json" "$u1" "$u2"

  # resize: 窓の幅を変えた直後（端末が折り返し位置を組み直す）
  tk send --pane "$p" "clear; cat '$TMP/soft.txt'" >/dev/null
  wait_for 15 screen_has "$p" END-SOFT || true
  tk window resize 760 800 >/dev/null 2>&1
  sleep 2
  cols2="$(tk links --pane "$p" | jq -r '.cols')"
  if [ "$cols2" = "$cols" ]; then
    unmeasured "$mode resize: 桁数が変わらない（${cols} → ${cols2}。蓋閉じで描画が組まれないと寸法が伝わらない。単体の幅 40〜72 桁の走査で代える）"
  else
    judge "$mode resize（${cols} → ${cols2} 桁）" "$p" "$TMP/$mode-resize.json" "$URL" "$LONG_PATH" "$WIDE_PATH"
    tk window resize 1000 800 >/dev/null 2>&1
    sleep 2
    cols="$(tk links --pane "$p" | jq -r '.cols')"
  fi

  # app: アプリ自身の折り返し（偽の出力）
  {
    app_wrap "$cols" "See" "$URL" "for details."
    echo
    app_wrap "$cols" "Open" "$LONG_PATH" "next."
    echo
    app_wrap "$cols" "Also" "$WIDE_PATH" "done."
    echo
    echo "END-APP"
  } > "$TMP/app.txt"
  tk send --pane "$p" "clear; cat '$TMP/app.txt'" >/dev/null
  wait_for 15 screen_has "$p" END-APP || unmeasured "$mode app: 出力が出ない"
  judge "$mode app" "$p" "$TMP/$mode-app.json" "$URL" "$LONG_PATH" "$WIDE_PATH"

  # claude: 実物の Claude Code に答えさせる
  if [ -z "$API_URL" ]; then
    unmeasured "$mode claude: claude / node / 偽 API のどれかが無い"
  else
    q="$(tk tab new --focus 2>/dev/null | jq -r '.pane // empty')"
    wait_for 30 screen_has "$q" 'tako %' || true
    tk send --pane "$q" "cd '$TMP/work' && clear && env CLAUDE_CONFIG_DIR='$CCD' ANTHROPIC_BASE_URL='$API_URL' ANTHROPIC_API_KEY='$API_KEY' DISABLE_AUTOUPDATER=1 CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC=1 claude --model haiku" >/dev/null 2>&1
    if wait_for 60 screen_has "$q" 'for agents'; then
      sleep 2
      tk send --pane "$q" "Echo verbatim: See $URL for details. | Open $LONG_PATH next. | Also $WIDE_PATH done. | END-CLAUDE" >/dev/null 2>&1
      if wait_for 60 claude_answered "$q"; then
        sleep 1
        tk read --pane "$q" > "$TMP/$mode-claude.txt" 2>&1
        judge "$mode claude" "$q" "$TMP/$mode-claude.json" "$URL" "$LONG_PATH" "$WIDE_PATH"
      else
        unmeasured "$mode claude: 答えが出ない"
      fi
    else
      unmeasured "$mode claude: claude が起動しない"
    fi
  fi
  stop_isolated_gui
  tmux_socket_kill "$SOCK"
  return 0
}

want direct && run_mode direct
want persist && run_mode persist

echo "結果: PASS=$PASS FAIL=$FAIL 未実測=${UNMEASURED}（腕 ${ARM}）"
[ "$FAIL" = 0 ] || exit 1
[ "$UNMEASURED" = 0 ] || exit 3
exit 0
