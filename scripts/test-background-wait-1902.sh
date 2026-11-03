#!/bin/bash
# test-background-wait-1902.sh — visual-test の節が background の仕事の戻りを「状態で」待つこと
# （#1902）の実経路テスト（隔離 GUI・visual-test 入り。debug / release は TAKO_ISO_PROFILE で選ぶ）
#
# 何が起きていたか:
#   visual-test / セルフテストの節は background の仕事（プレビューの読み込み・構文の塗り・
#   PDF のラスタライズ・tasks の一覧更新と送信・添付のサムネイル・hover の問い合わせ・偽サーバの
#   診断）の戻りを `for _ in 0..N { 待つ; 読む }` の回数の窓で待っていた。窓の長さ（N × 待ち）は
#   release の速さで決めてあり、debug（visual-test 入り）や混んだ機では戻る前に使い切りうる
#   （#1890 の large-file-decor は入った時点から debug で一度も通っていなかった = 同じ型）。
#   #1902 で待ちを 1 実装 `wait_for_background_state`（走っている間だけ待つ・戻ったのに揃わなければ
#   その場で偽・上限は state_wait_budget）へ寄せた。2 回目に background の残り（偽サーバの診断・
#   外部変更の競合・Markdown の目次・塗り直し・PDF の幾何と再ラスタ・動画の末尾停止）を同じ 1 実装へ、
#   画面の静止を撮り直す 4 か所を `wait_for_quiet_frame` へ寄せた。
#
# 何を確かめるか（遅れは注入で作る = CPU を焼かない）:
#   ① 素の状態で節（既定は下の SECTIONS の 12 節）が緑・その節の待ちが Landed
#   ② 観測の遅れ（TAKO_1902_INJECT=late。旧の窓 + 5 秒）でも同じ節が緑・診断行の waited が遅れ以上
#   ③ A/B: ② を 1 項目に絞り + TAKO_1902_LEGACY=<項目>（旧の回数の窓）→ 結果を判定に使う待ちは
#      その項目の検査が名指しで FAILED（= 修正前の予算は同じ遅れで落ちる）。注入は**観測だけ**を
#      遅らせる（相手は前へ進む）ので、結果を判定に使わない待ち（診断の到着・preview-memory の
#      描き切り・ime-preedit の mid・塗り直し）は旧の窓でも後段の検査が通る = 表に載せない
#   ④ 塗りそのものの遅れ（製品側の TAKO_1890_INJECT=slow:<SLOW_MS>。旧の窓 80 回 × 50ms より長い）:
#      新は緑・旧の窓（TAKO_1902_LEGACY=preview-highlight）は「① .ino が複数の構文色で塗られる」で FAILED
#   ⑤ 塗りを捨てる注入（TAKO_1890_INJECT=drop）→ 上限まで待たずに outcome=Settled で FAILED
#   ⑥ 検出力: TAKO_1902_INJECT=never:preview-maps は新の腕でも上限まで待って「平文 paint」で FAILED
#   ⑦ セルフテスト（TAKO_SELF_TEST=1。全項目を回すので長い = 既定の STAGES に入れない）: 素の状態と、
#      PDF の幾何・150% の再ラスタ・動画の末尾停止の 3 つの待ちへ観測の遅れを入れた状態で緑・3 つとも
#      Landed（SELFTEST_ARMS="plain late" で腕を選ぶ）。HOME は差し替えない（項目 1c「初期 cwd は
#      ホーム」と項目 14 の cargo が本物の HOME を要る。置き場は回ごとの TMPDIR の下）
#   ⑧ ⑦ の A/B: 1 項目ずつ旧の窓 + 遅れ → その項目の検査が名指しで FAILED（STAGES=8 で選ぶ）
#
# 判定は「緑 / 名指しの FAILED」と診断行（TAKO_SELF_TEST_1902）の outcome / waited だけ。
# 実時間は判定に使わない（waited と遅れの大小は「注入が効いた」ことの確認）。
#
# **本番の tako / 設定には一切触らない**（data / HOME は mktemp 配下、落とすのは自分で起こした pid
# だけ）。窓は常設の仮想ディスプレイ（tako-vd）へ出る（#1141 / #1490）。
#
# 使い方: TAKO_ISO_PROFILE=debug|release bash scripts/test-background-wait-1902.sh
#   STAGES="1 2 3 4 5 6"（既定。⑦ ⑧ のセルフテストは STAGES="7 8" で足す）で段を選ぶ。
#   SECTIONS で ①〜③ の節を絞る。SLOW_MS（既定 12000）= ④ の塗りの遅れ
set -uo pipefail

# **本番 GUI を指す env を最初に落とす**（#1449 / #1450 で本番にペインが漏れた実例が 2 件）
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_ORCHESTRATOR_ROLE

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SLOW_MS="${SLOW_MS:-12000}"
STAGES="${STAGES:-1 2 3 4 5 6}"
# ①〜③ で回す節。後ろの 8 節は 2 回目の移送（background の残りと画面の静止）
SECTIONS="${SECTIONS:-ino-highlight tasks-tick task-attachment tasks-input preview-code external-change md-edit-resume md-find-restore syntax-alias screen-lines ime-preedit preview-memory}"
PASS=0
FAIL=0
pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
fail() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }
has_stage() { case " $STAGES " in *" $1 "*) return 0 ;; *) return 1 ;; esac; }

TMP="$(mktemp -d /tmp/tako-1902-XXXXXX)"
cleanup() {
  stop_isolated_gui "${ISOLATED_GUI_PID:-}"
  tmux -L "tako-1902-$$" kill-server >/dev/null 2>&1 || true
  rm -rf "${TMP:?}"
}
trap cleanup EXIT

# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
# ビルドは HOME を差し替える前に（後で呼ぶとツールチェーンとレジストリを一時 HOME へ取り直す）
isolated_gui_bins visual-test || exit 1
PROFILE="$ISOLATED_GUI_PROFILE"
echo "プロファイル: $PROFILE"
# preview-code の診断は本体の隣の偽の言語サーバを起こす（#1679。言語サーバの自動起動は隔離 GUI では
# 既定で切れている = #2048 なので、その節にだけ TAKO_LSP_AUTOSTART=1 を付ける = section_env）
case " $SECTIONS " in
  *" preview-code "*)
    echo "偽の言語サーバ（tako-lsp-fake）をビルドします…"
    FAKE_ARGS=(-q -p tako-control --bin tako-lsp-fake)
    [ "$PROFILE" = release ] && FAKE_ARGS+=(--release)
    (cd "$REPO_ROOT" && run_heavy cargo build ${FAKE_ARGS[@]+"${FAKE_ARGS[@]}"}) || exit 1
    ;;
esac
section_env() { # 節 → その節にだけ付ける env（空可）
  case "$1" in
    preview-code) echo "TAKO_LSP_AUTOSTART=1" ;;
  esac
}

# ⑦ ⑧ のセルフテストだけは本物の HOME で起こす（項目 1c「初期 cwd はホーム」と項目 14 の cargo）
REAL_HOME="$HOME"
export HOME="$TMP/home"
export TAKO_ISOLATED=1
export TAKO_DISCOVERY_DIR="$TMP/disc"
export TAKO_SESSIONS_FILE="$TMP/sessions.yaml"
export TAKO_PANE_LOG_DIR="$TMP/panelogs"
export TAKO_WORKERS_FILE="$TMP/workers.yaml"
export TAKO_TMUX_SOCKET="tako-1902-$$"
mkdir -p "$HOME" "$TAKO_DISCOVERY_DIR" "$TMP/zdot"
printf "PROMPT='tako %%1~ %%%% '\nRPROMPT=''\n" > "$TMP/zdot/.zshrc"

# 節を 1 回走らせて、終わるまで**状態で**待つ（プロセスが消えるまで。上限 900 秒 = never の上限
# 4 倍ぶんを含む）。回ごとに data / 置き場を分ける（前の回の一覧を持ち越さない）。緑なら 0
RUN=0
run_section() { # ログ 節 追加の env…
  local log="$1" section="$2" dir extra
  shift 2
  extra="$(section_env "$section")"
  RUN=$((RUN + 1))
  dir="$TMP/run-$RUN"
  mkdir -p "$dir/data" "$dir/orch"
  case "$dir" in
    "$TMP"/*) : ;;
    *) echo "隔離されていないディレクトリ: ${dir}"; exit 1 ;;
  esac
  launch_isolated_gui "$log" TAKO_VISUAL_TEST=1 TAKO_VISUAL_ONLY="$section" \
    TAKO_DATA_DIR="$dir/data" TAKO_ORCHESTRATOR_DIR="$dir/orch" \
    TAKO_USER_TASKS_FILE="$dir/orch/user-tasks.yaml" TAKO_AUTO_RENAME=0 \
    ZDOTDIR="$TMP/zdot" $extra ${1+"$@"} || exit $?
  local pid="$ISOLATED_GUI_PID" i
  for i in $(seq 1 9000); do
    kill -0 "$pid" 2>/dev/null || break
    sleep 0.1
  done
  stop_isolated_gui "$pid"
  grep -E "TAKO_SELF_TEST_1902|TAKO_SELF_TEST_STATE_TIMEOUT|TAKO_APP_SELF_TEST_FAILED" "$log" \
    | cut -c1-200 | sed 's/^/    /'
  grep -q "TAKO_VISUAL_TEST_OK" "$log"
}

# 診断行（`TAKO_SELF_TEST_1902: item=<項目> outcome=<X> waited=<秒>s …`）の n 番目から 1 欄を採る
diag_field() { # ログ 項目 欄 [n]
  grep "TAKO_SELF_TEST_1902: item=$2 " "$1" | sed -n "${4:-1}p" | tr ' ' '\n' \
    | sed -n "s/^$3=//p" | head -1
}
# その項目の待ちがすべて Landed か（1 つも無ければ偽）
all_landed() { # ログ 項目
  local n bad
  n=$(grep -c "TAKO_SELF_TEST_1902: item=$2 " "$1")
  bad=$(grep "TAKO_SELF_TEST_1902: item=$2 " "$1" | grep -vc "outcome=Landed")
  [ "$n" -gt 0 ] && [ "$bad" -eq 0 ]
}
# 1 つ目の待ちの waited（小数の秒）が `秒` 以上か
waited_at_least() { # ログ 項目 秒
  local waited
  waited="$(diag_field "$1" "$2" waited)"
  waited="${waited%s}"
  [ -n "$waited" ] && [ "${waited%.*}" -ge "$3" ]
}

# 節が通る待ちの項目（観測の遅れを入れると、どれも旧の窓 + 5 秒は待つ）
items_of() {
  case "$1" in
    ino-highlight) echo "preview-maps preview-highlight" ;;
    tasks-tick) echo "user-tasks" ;;
    task-attachment) echo "task-thumb" ;;
    tasks-input) echo "tasks-input-bare tasks-input-closed tasks-input-burst tasks-input-restore" ;;
    preview-code) echo "diagnostics-arrive" ;;
    external-change) echo "external-conflict" ;;
    md-edit-resume | md-find-restore) echo "md-outline" ;;
    syntax-alias) echo "syntax-repaint" ;;
    screen-lines) echo "screen-quiet" ;;
    ime-preedit) echo "ime-quiet ime-quiet-mid" ;;
    preview-memory) echo "preview-settle" ;;
  esac
}

if has_stage 1; then
  echo
  echo "① 素の状態で節が緑（${PROFILE}）"
  for S in $SECTIONS; do
    if run_section "$TMP/plain-$S.log" "$S"; then
      pass "$S が緑"
    else
      fail "$S が緑にならない"
    fi
    for I in $(items_of "$S"); do
      if all_landed "$TMP/plain-$S.log" "$I"; then
        pass "$S の $I は Landed（waited=$(diag_field "$TMP/plain-$S.log" "$I" waited)）"
      else
        fail "$S の $I が Landed でない"
      fi
    done
  done
fi

if has_stage 2; then
  echo
  echo "② 観測の遅れ（TAKO_1902_INJECT=late = 旧の窓 + 5 秒）でも節が緑（${PROFILE}）"
  for S in $SECTIONS; do
    if run_section "$TMP/late-$S.log" "$S" TAKO_1902_INJECT=late; then
      pass "$S が緑（遅れの向こうまで待った）"
    else
      fail "$S が緑にならない（遅れの注入）"
    fi
    for I in $(items_of "$S"); do
      if all_landed "$TMP/late-$S.log" "$I" && waited_at_least "$TMP/late-$S.log" "$I" 5; then
        pass "$S の $I は遅れ以上待って揃った（waited=$(diag_field "$TMP/late-$S.log" "$I" waited) inject=$(diag_field "$TMP/late-$S.log" "$I" inject)）"
      else
        fail "$S の $I が遅れの向こうで揃っていない"
      fi
    done
  done
fi

if has_stage 3; then
  echo
  echo "③ A/B: 遅れを 1 項目に絞り + TAKO_1902_LEGACY=<項目>（旧の回数の窓）→ 名指しで FAILED（${PROFILE}）"
  # 節 項目 落ちる検査のラベル（結果を判定に使う待ちだけ。tasks-input の 4 つは後続の検査が判定する）
  while IFS='|' read -r S I LABEL; do
    [ -n "$S" ] || continue
    case " $SECTIONS " in *" $S "*) : ;; *) continue ;; esac
    log="$TMP/legacy-$S-$I.log"
    if run_section "$log" "$S" TAKO_1902_INJECT="late:$I" TAKO_1902_LEGACY="$I"; then
      fail "$I: 旧の窓でも通った（遅れが窓より短い？）"
    elif grep -q "TAKO_APP_SELF_TEST_FAILED: .*$LABEL" "$log"; then
      pass "$I: 旧の窓は「${LABEL}」で FAILED（outcome=$(diag_field "$log" "$I" outcome) budget=$(diag_field "$log" "$I" budget) legacy=$(diag_field "$log" "$I" legacy)）"
    else
      fail "$I: 旧の窓の落ち方が違う: $(grep -o 'TAKO_APP_SELF_TEST_FAILED: .*' "$log" | head -1 | cut -c1-160)"
    fi
  done <<'EOF'
ino-highlight|preview-maps|平文 paint
ino-highlight|preview-highlight|① .ino が複数の構文色で塗られる
tasks-tick|user-tasks|一覧更新が着地しない
task-attachment|task-thumb|画像の添付にサムネイルが載る
external-change|external-conflict|外部変更を競合として検知する
md-edit-resume|md-outline|描画表示で開いて目次が 3 件
md-find-restore|md-outline|描画表示で開いて目次が 3 件
screen-lines|screen-quiet|画面が静止しない
ime-preedit|ime-quiet|画面が静止しない
EOF
fi

if has_stage 4; then
  echo
  echo "④ 塗りそのものを ${SLOW_MS} ms 遅らせる（製品側の TAKO_1890_INJECT。${PROFILE}）"
  log="$TMP/slow-new.log"
  if run_section "$log" ino-highlight TAKO_1890_INJECT="slow:$SLOW_MS"; then
    if waited_at_least "$log" preview-highlight $((SLOW_MS / 1000)); then
      pass "新は緑・塗りの遅れ以上待って揃った（waited=$(diag_field "$log" preview-highlight waited)）"
    else
      fail "新は緑だが waited が遅れより短い（注入が効いていない？）"
    fi
  else
    fail "新が緑にならない（塗りの遅れ）"
  fi
  log="$TMP/slow-legacy.log"
  if run_section "$log" ino-highlight TAKO_1890_INJECT="slow:$SLOW_MS" TAKO_1902_LEGACY=preview-highlight; then
    fail "旧の窓でも通った（SLOW_MS が旧の窓より短い？）"
  elif grep -q "TAKO_APP_SELF_TEST_FAILED: ① .ino が複数の構文色で塗られる" "$log"; then
    pass "旧の窓は「① .ino が複数の構文色で塗られる」で FAILED（塗りはまだ走っていた = outcome=$(diag_field "$log" preview-highlight outcome)）"
  else
    fail "旧の窓の落ち方が違う: $(grep -o 'TAKO_APP_SELF_TEST_FAILED: .*' "$log" | head -1 | cut -c1-160)"
  fi
fi

if has_stage 5; then
  echo
  echo "⑤ 塗りを捨てる注入（TAKO_1890_INJECT=drop）→ 上限まで待たずに FAILED（${PROFILE}）"
  log="$TMP/drop.log"
  if run_section "$log" ino-highlight TAKO_1890_INJECT=drop; then
    fail "塗りを捨てても通った（検出力が無い）"
  elif grep -q "TAKO_APP_SELF_TEST_FAILED: ① .ino が複数の構文色で塗られる" "$log" \
    && [ "$(diag_field "$log" preview-highlight outcome)" = "Settled" ]; then
    pass "outcome=Settled で FAILED（waited=$(diag_field "$log" preview-highlight waited) budget=$(diag_field "$log" preview-highlight budget)）"
  else
    fail "落ち方が違う: outcome=$(diag_field "$log" preview-highlight outcome) $(grep -o 'TAKO_APP_SELF_TEST_FAILED: .*' "$log" | head -1 | cut -c1-120)"
  fi
fi

if has_stage 6; then
  echo
  echo "⑥ 検出力: TAKO_1902_INJECT=never:preview-maps は新の腕でも上限で FAILED（${PROFILE}）"
  log="$TMP/never.log"
  if run_section "$log" ino-highlight TAKO_1902_INJECT=never:preview-maps; then
    fail "ずっと観測しなくても通った（上限で諦めていない）"
  elif grep -q "TAKO_APP_SELF_TEST_FAILED: visual-test ino-highlight: 平文 paint" "$log" \
    && grep -q 'TAKO_SELF_TEST_STATE_TIMEOUT: label="preview-maps"' "$log"; then
    pass "上限で諦めて「平文 paint」で FAILED（outcome=$(diag_field "$log" preview-maps outcome) waited=$(diag_field "$log" preview-maps waited) budget=$(diag_field "$log" preview-maps budget)）"
  else
    fail "落ち方が違う: $(grep -o 'TAKO_APP_SELF_TEST_FAILED: .*' "$log" | head -1 | cut -c1-160)"
  fi
fi

# セルフテストを 1 回走らせて、終わるまで待つ（上限 3600 秒）。置き場は回ごとの TMPDIR の下
# （隔離 GUI の既定の置き場 = tako-iso-*-<pid> もそこへ入る = 後片付けは TMP ごと）。緑なら 0
run_selftest() { # ログ 追加の env…
  local log="$1" dir
  shift
  RUN=$((RUN + 1))
  dir="$TMP/run-$RUN"
  mkdir -p "$dir/tmp" "$dir/orch"
  launch_isolated_gui "$log" TAKO_SELF_TEST=1 HOME="$REAL_HOME" TMPDIR="$dir/tmp/" \
    TAKO_ORCHESTRATOR_DIR="$dir/orch" TAKO_AUTO_RENAME=0 ${1+"$@"} || exit $?
  local pid="$ISOLATED_GUI_PID" i
  for i in $(seq 1 36000); do
    kill -0 "$pid" 2>/dev/null || break
    sleep 0.1
  done
  stop_isolated_gui "$pid"
  # セルフテストのログは多バイトの途中で切れた行を含みうるので、バイトで切る（cut が止まらない）
  grep -E "TAKO_SELF_TEST_1902: item=selftest-|TAKO_SELF_TEST_STATE_TIMEOUT|TAKO_APP_SELF_TEST_FAILED|TAKO_APP_SELF_TEST_OK" "$log" \
    | LC_ALL=C cut -c1-200 | sed 's/^/    /'
  grep -q "TAKO_APP_SELF_TEST_OK" "$log"
}
SELFTEST_ITEMS="selftest-pdf-geometry selftest-pdf-zoom selftest-video-end"

if has_stage 7; then
  # 素の腕は「走り終えたのに揃わない」の早い打ち切り（Settled）が素の時機で誤爆しないことを、
  # 遅れの腕は遅れの向こうまで待つことを見る（遅れの間は観測が Running に上書きされるので、
  # 遅れの腕だけでは素の時機を通らない）。SELFTEST_ARMS で腕を選ぶ
  for ARM in ${SELFTEST_ARMS:-plain late}; do
    echo
    log="$TMP/selftest-$ARM.log"
    if [ "$ARM" = late ]; then
      echo "⑦ セルフテスト: 3 つの待ちへ観測の遅れを入れても緑（${PROFILE}）"
      run_selftest "$log" TAKO_1902_INJECT="late:$(echo $SELFTEST_ITEMS | tr ' ' ',')"
    else
      echo "⑦ セルフテスト: 素の状態で緑（${PROFILE}）"
      run_selftest "$log"
    fi
    if [ $? -eq 0 ]; then
      pass "セルフテストが緑（${ARM}）"
    else
      fail "セルフテストが緑にならない（${ARM}）: $(grep -o 'TAKO_APP_SELF_TEST_FAILED: .*' "$log" | head -1 | cut -c1-160)"
    fi
    for I in $SELFTEST_ITEMS; do
      if [ "$ARM" = late ]; then
        if all_landed "$log" "$I" && waited_at_least "$log" "$I" 5; then
          pass "$I は遅れ以上待って揃った（waited=$(diag_field "$log" "$I" waited) inject=$(diag_field "$log" "$I" inject)）"
        else
          fail "$I が遅れの向こうで揃っていない（outcome=$(diag_field "$log" "$I" outcome)）"
        fi
      elif all_landed "$log" "$I"; then
        pass "$I は Landed（waited=$(diag_field "$log" "$I" waited) polls=$(diag_field "$log" "$I" polls)）"
      else
        fail "$I が Landed でない（outcome=$(diag_field "$log" "$I" outcome)）"
      fi
    done
  done
fi

if has_stage 8; then
  echo
  echo "⑧ セルフテストの A/B: 1 項目ずつ旧の窓 + 遅れ → 名指しで FAILED（${PROFILE}）"
  while IFS='|' read -r I LABEL; do
    [ -n "$I" ] || continue
    log="$TMP/selftest-legacy-$I.log"
    if run_selftest "$log" TAKO_1902_INJECT="late:$I" TAKO_1902_LEGACY="$I"; then
      fail "$I: 旧の窓でも通った（遅れが窓より短い？）"
    elif grep -q "TAKO_APP_SELF_TEST_FAILED: .*$LABEL" "$log"; then
      pass "$I: 旧の窓は「${LABEL}」で FAILED（outcome=$(diag_field "$log" "$I" outcome) budget=$(diag_field "$log" "$I" budget)）"
    else
      fail "$I: 旧の窓の落ち方が違う: $(grep -o 'TAKO_APP_SELF_TEST_FAILED: .*' "$log" | head -1 | cut -c1-160)"
    fi
  done <<'EOF'
selftest-pdf-geometry|PDFKit アウトライン一覧と 2 ページ目ジャンプ
selftest-pdf-zoom|PDF 150% の再ラスタライズ後も文字座標が画像へ追従する
selftest-video-end|末尾で自動停止し、再生ボタンで先頭から再開する
EOF
fi

echo
echo "結果（${PROFILE}）: PASS=$PASS FAIL=$FAIL"
[ "$FAIL" -eq 0 ]
