#!/bin/bash
# nightly-footprint-watch.sh — 常駐メモリ・アイドル CPU・描画回数の回帰を夜間に見張る（#2023 / エピック #2011 の S8）
#
# 何のためか:
#   #814（2026-08）で本番 129 MB まで削ったものが、2 か月で 400〜835 MB に戻っていた。
#   **戻ったことに誰も気づけなかった**。そこで夜ごとに origin/main を同じ条件で測り
#   （`scripts/measure-footprint-2011.sh` の std）、直近 N 回の中央値（基準）より閾値を超えて
#   悪化したら `tako todo`（人がやること）へ 1 件載せる。閾値と基準の取り方は
#   `scripts/lib/footprint-watch.py` の METRICS（S0 の 2 回のぶれ = 設計書 §7.1 から決めた）。
#
# 使い方:
#   bash scripts/nightly-footprint-watch.sh            # origin/main を測って基準と比べる（そのコミットを測り済みなら飛ばす）
#   bash scripts/nightly-footprint-watch.sh --force    # 測り済みでも測る
#   bash scripts/nightly-footprint-watch.sh --status   # 直近の結果・基準・載せた悪化を見る（読むだけ）
#
# 夜間は scripts/nightly-release.sh の EXIT trap が、**リリースとロックの解放が済んだ後に**呼ぶ
# （リリースを遅らせない。どの終了コードでも夜間リリースの成否・終了コードは変わらない）。
#
# 1 回の流れ:
#   ロック → origin/main を使い捨ての worktree へ出す（共有ツリーの HEAD は触らない）→ ビルド
#   （順番待ち・上限つき）→ 計測（上限つき・tako-vd の上だけ）→ 履歴へ 1 行 → 判定 → 悪化なら起票
#
# **蓋閉じの夜**: tako-vd を用意できない（計測が終了コード 4）・時間切れは「未実測」として 1 行
#   残して静かに抜ける（起票しない）。tako-vd は起きたが画面ロック等でフレームが組まれない夜
#   （`drawn` が「なし」）は、描画の値（出力中の footprint・CPU・描画回数）を比べずに、
#   残りを描画なしの夜どうしで比べる。
# **同じ悪化を重ねて積まない**: 前の夜から続く悪化の項目を状態ファイルに持ち、新しい項目が
#   悪化したときだけ起票する（未完了の 1 件が残っていればそれを書き換える）。回帰なしの夜が
#   来たら状態を消す（todo は人が片付ける）。
# **起動の区間に重い処理が重なった夜**（#2652）: 計測は順番待ちの外で走るので、ビルドが枠を返した
#   その秒に他の worker の clippy 等が走り出して起動の区間に重なる（11/02 は起動 2.4 → 10.9 秒。
#   回帰前のコミットでも重ねると 10.6 秒 = 計測の外れ値）。計測の行の `startup_busy`（起こす直前と
#   `tako list` が通った直後の順番待ちの実行中の本数）が 1 以上なら、順番待ちが空くまで待って
#   （上限 20 分）**1 回だけ**測り直す。測り直しのバイナリは新しいファイルへ複製して渡す（同じファイルの
#   2 回目の exec は初回の検査が済んでいて起動が約 1.4 秒短く出る = 建てたての夜の基準と揃わない）。
#   それでも重なった・空かなかった・測り直せなかった夜は、その行の起動時間を比べず基準にも入れない
#   （判定は footprint-watch.py の CONTENDED_KEYS）。A/B は `TAKO_2652_LEGACY=1`（測り直さず重なりを見ない）
#
# 置き場（data dir ではなく開発側。`cargo clean` で消えないよう target/ の外 = 夜間の予約・mod の
#   検査の記録と同じ所）: ~/.claude-orchestrator/state/tako-footprint/
#     history.tsv … 1 晩 1 行（未実測も残す）/ state … 続いている悪化 / last-run.log … 直近の計測の全出力
#
# 終了コード: 0 = 判定まで済んだ（悪化を載せた・続いている・回帰なし・基準づくり中・測り済みで飛ばした）/
#   1 = 計測の失敗（ビルド・計測スクリプトの異常）か、悪化を tako todo へ載せられなかった /
#   2 = 引数の誤り / 3 = 未実測（tako-vd を用意できない・時間切れ）
#
# env（注入・テスト用）:
#   TAKO_FOOTPRINT_ENV                 計測する GUI へ足す env（計測スクリプトへそのまま渡る。
#                                      例 TAKO_2009_LEGACY=1 = わざと footprint を増やす注入。この行は基準に入らない）
#   TAKO_FOOTPRINT_WATCH_SCALE         閾値の倍率（既定 1。0.5 = 閾値を半分に下げる注入）
#   TAKO_FOOTPRINT_WATCH_MIN           判定に要る基準の回数（既定 3）/ TAKO_FOOTPRINT_WATCH_WINDOW 基準の回数（既定 5）
#   TAKO_FOOTPRINT_WATCH_TIMEOUT       計測の上限（秒。既定 900）
#   TAKO_FOOTPRINT_WATCH_QUIET_WAIT    起動に重なりがあったとき、順番待ちが空くのを待つ上限（秒。既定 1200 = #2652）
#   TAKO_FOOTPRINT_WATCH_QUIET_POLL    その待ちで順番待ちを読む間隔（秒。既定 10）
#   TAKO_FOOTPRINT_WATCH_BUILD_TIMEOUT ビルドの上限（順番待ちを含む。秒。既定 3600）
#   TAKO_FOOTPRINT_WATCH_DIR           履歴・状態の置き場
#   TAKO_FOOTPRINT_WATCH_TAKO          起票に使う tako（既定 /Applications/tako.app の tako → PATH の tako）
#   TAKO_FOOTPRINT_WATCH_MEASURE       計測スクリプト（既定は測るコミットの scripts/measure-footprint-2011.sh）
#   TAKO_FOOTPRINT_WATCH_REF           測るコミット（既定 origin/main。ブランチを夜間と同じ流れで測るとき。
#                                      その行は origin/main の行と比べられるが、origin/main の基準には入らない）
#
# bash 3.2（macOS 同梱）で通す。set -e は使わない（失敗はすべて自分で受けて記録する）。
set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
. "$ROOT/scripts/lib/tmux-socket.sh"
PY="$ROOT/scripts/lib/footprint-watch.py"
DIR="${TAKO_FOOTPRINT_WATCH_DIR:-$HOME/.claude-orchestrator/state/tako-footprint}"
HISTORY="$DIR/history.tsv"
STATE="$DIR/state"
LOCK_DIR="$HOME/.claude-orchestrator/locks/tako-footprint-watch.lock"
MEASURE_TIMEOUT="${TAKO_FOOTPRINT_WATCH_TIMEOUT:-900}"
QUIET_WAIT="${TAKO_FOOTPRINT_WATCH_QUIET_WAIT:-1200}"
QUIET_POLL="${TAKO_FOOTPRINT_WATCH_QUIET_POLL:-10}"
BUILD_TIMEOUT="${TAKO_FOOTPRINT_WATCH_BUILD_TIMEOUT:-3600}"
SCALE="${TAKO_FOOTPRINT_WATCH_SCALE:-1}"
MIN_N="${TAKO_FOOTPRINT_WATCH_MIN:-3}"
WINDOW="${TAKO_FOOTPRINT_WATCH_WINDOW:-5}"
FORCE=0
STATUS_ONLY=0

for arg in "$@"; do
  case "$arg" in
    --force) FORCE=1 ;;
    --status) STATUS_ONLY=1 ;;
    -h|--help) sed -n '2,/^set -uo/p' "$0" | sed '$d'; exit 0 ;;
    *) echo "不明な引数: ${arg}（--force / --status）" >&2; exit 2 ;;
  esac
done

# 出力にホームの実パスを残さない（夜間のログへそのまま流れる）
mask() { sed -e "s#${HOME}#~#g"; }
say() { echo "$*" | mask; }

if [ ! -f "$PY" ]; then
  say "ERROR: ${PY} が無い（リポジトリが不完全）"
  exit 1
fi

if [ "$STATUS_ONLY" = 1 ]; then
  python3 "$PY" status --history "$HISTORY" --state "$STATE" --window "$WINDOW" --min "$MIN_N" \
    --scale "$SCALE" --history-label "$(echo "$HISTORY" | mask)"
  exit $?
fi

mkdir -p "$DIR"

# --- ロック（同時に 2 本走らせない。stale は pid の生存で判定） -------------------
mkdir -p "$(dirname "$LOCK_DIR")"
if ! mkdir "$LOCK_DIR" 2>/dev/null; then
  old_pid="$(cat "$LOCK_DIR/pid" 2>/dev/null || true)"
  if [ -n "$old_pid" ] && kill -0 "$old_pid" 2>/dev/null; then
    say "見張りは別の実行（PID ${old_pid}）が走っているので飛ばす"
    exit 0
  fi
  rm -rf "$LOCK_DIR"
  mkdir "$LOCK_DIR" || { say "ERROR: ロックを取れない"; exit 1; }
fi
echo $$ > "$LOCK_DIR/pid"

WORK=""
WT=""
JOB_PID=""

# 上限つきで走らせているもの（計測・ビルド）を止める。計測スクリプトは TERM で後片付け
# （隔離 GUI・自分の tmux）を通すので、まず TERM で待ち、居座るときだけプロセスグループごと KILL
stop_job() {
  local i
  [ -n "$JOB_PID" ] || return 0
  kill -TERM -- "-$JOB_PID" 2>/dev/null || kill -TERM "$JOB_PID" 2>/dev/null || true
  for i in $(seq 1 100); do
    kill -0 "$JOB_PID" 2>/dev/null || break
    sleep 0.2
  done
  kill -KILL -- "-$JOB_PID" 2>/dev/null || true
  wait "$JOB_PID" 2>/dev/null || true
  # 計測の tmux サーバーは自分でセッションを分けるのでグループの外に居る。
  # 計測スクリプトのソケット名（tako-fp-<その pid>）で落とす（KILL で後片付けが走らなかったときの保険）
  tmux_socket_kill "tako-fp-$JOB_PID"
  JOB_PID=""
}

cleanup() {
  stop_job
  if [ -n "$WT" ]; then
    git -C "$ROOT" worktree remove --force "$WT" >/dev/null 2>&1 || true
    git -C "$ROOT" worktree prune >/dev/null 2>&1 || true
  fi
  [ -n "$WORK" ] && rm -rf "$WORK"
  rm -rf "$LOCK_DIR"
}
trap cleanup EXIT
trap 'exit 130' INT TERM HUP

# run_limited <秒> <出力ファイル> <コマンド...>
#   自分のプロセスグループで走らせ、上限を超えたら止めて 124 を返す（launchd の制御端末なしでも
#   set -m でグループが分かれる = scripts/check-claude-mod.sh の run_step と同じ作り）
run_limited() {
  local secs="$1" out="$2" i=0 rc=0
  shift 2
  set -m
  "$@" < /dev/null > "$out" 2>&1 &
  JOB_PID=$!
  set +m
  while kill -0 "$JOB_PID" 2>/dev/null; do
    if [ "$i" -ge $((secs * 5)) ]; then
      stop_job
      return 124
    fi
    sleep 0.2
    i=$((i + 1))
  done
  wait "$JOB_PID" || rc=$?
  JOB_PID=""
  return "$rc"
}

now() { date '+%Y-%m-%d %H:%M'; }
RUN_DATE="$(now)"

record() { # <status> <理由> <経過秒> [計測の TSV]
  python3 "$PY" record --history "$HISTORY" --date "$RUN_DATE" --sha "$SHA" --version "$VERSION" --ref "$REF" \
    --status "$1" --reason "$2" --elapsed "$3" --row "${4:-}"
}

# 起票に使う tako。**ビルドの順番待ちもこの tako の置き場で通す**: 夜間リリースの PATH は先頭が
# ~/.cargo/bin で、そこに古い tako（順番待ちを知らない）が居ると run_heavy が並ばずに走らせてしまう
TAKO_CLI="${TAKO_FOOTPRINT_WATCH_TAKO:-}"
if [ -z "$TAKO_CLI" ]; then
  if [ -x /Applications/tako.app/Contents/MacOS/tako ]; then
    TAKO_CLI=/Applications/tako.app/Contents/MacOS/tako
  else
    TAKO_CLI="$(command -v tako 2>/dev/null || true)"
  fi
fi
BUILD_PATH="$PATH"
[ -n "$TAKO_CLI" ] && BUILD_PATH="$(dirname "$TAKO_CLI"):$PATH"

# --- 測るコミット（既定 origin/main）---------------------------------------------
REF="${TAKO_FOOTPRINT_WATCH_REF:-origin/main}"
git -C "$ROOT" fetch origin --quiet 2>/dev/null || say "WARN: git fetch に失敗。手元の ${REF} を測る"
SHA="$(git -C "$ROOT" rev-parse --verify --quiet "${REF}^{commit}" 2>/dev/null || true)"
if [ -z "$SHA" ]; then
  say "ERROR: ${REF} が引けないので測らない"
  exit 1
fi
VERSION="$(git -C "$ROOT" show "$SHA:Cargo.toml" 2>/dev/null | sed -n 's/^version = "\(.*\)"/\1/p' | head -1)"
VERSION="${VERSION:--}"
SHORT="$(printf '%s' "$SHA" | cut -c1-7)"

if [ "$FORCE" != 1 ] && python3 "$PY" measured --history "$HISTORY" --sha "$SHA"; then
  say "測り済み（v${VERSION}・${SHORT} は描画ありで記録がある）なので飛ばす（測り直すなら --force）"
  exit 0
fi

WORK="$(mktemp -d "${TMPDIR:-/tmp}/tako-fpw-XXXXXX")"
WT="$WORK/src"
if ! git -C "$ROOT" worktree add --detach --quiet "$WT" "$SHA" 2>"$WORK/wt.err"; then
  say "ERROR: worktree を作れない: $(head -1 "$WORK/wt.err")"
  WT=""
  record failed "worktree を作れない" 0
  exit 1
fi
# ビルドキャッシュは共有ツリーの target/ を使い回す（夜間リリースと同じ。毎晩のフルビルドを避ける）
[ -e "$ROOT/target" ] && ln -s "$ROOT/target" "$WT/target"

MEASURE="${TAKO_FOOTPRINT_WATCH_MEASURE:-$WT/scripts/measure-footprint-2011.sh}"
if [ ! -f "$MEASURE" ]; then
  say "ERROR: 計測スクリプトが無い（${SHORT} に scripts/measure-footprint-2011.sh が無い）"
  record failed "計測スクリプトが無い" 0
  exit 1
fi
say "計測: v${VERSION}（${SHORT}）を std で測る（ビルドの上限 ${BUILD_TIMEOUT} 秒・計測の上限 ${MEASURE_TIMEOUT} 秒）"

# --- ビルド（順番待ちを通す。計測スクリプトの中のビルドはこの後なら空振りで済む）------------
# **cargo の前に PWA の dist/ を作る**（build-app.sh と同じ）: 使い捨ての worktree には dist/ が無く、
# 無いまま cargo を回すと tako-control の build.rs が PWA を作る。作った dist/ の時刻のせいで
# 2 回目のビルド（計測スクリプトの中 = 順番待ちの外）でも build.rs が再実行され、tako-control から
# 下流が丸ごと組み直しになる（実測 約 9 分。tako / tako-app が計測の段の最中に書き換わっていた）
t0=$(date +%s)
rc=0
# 順番待ちは上限の 2/3 で諦めさせる（残りをビルドに充てる。諦めた tako queue は 75 で返る）
run_limited "$BUILD_TIMEOUT" "$WORK/build.log" \
  env PATH="$BUILD_PATH" TAKO_QUEUE_MAX_WAIT="$((BUILD_TIMEOUT * 2 / 3))s" \
  /bin/bash -c 'cd "$1" && . scripts/lib/queue.sh && run_heavy /bin/bash -c "{ [ ! -f scripts/build-pwa.sh ] || bash scripts/build-pwa.sh --if-missing; }; cargo build --release -q -p tako-cli -p tako-app"' _ "$WT" \
  || rc=$?
build_s=$(( $(date +%s) - t0 ))
if [ "$rc" -eq 124 ] || [ "$rc" -eq 75 ]; then
  say "未実測: ビルドが上限に届いた（exit ${rc}・${build_s} 秒。75 = 順番待ちの上限 / 124 = ビルドの上限 ${BUILD_TIMEOUT} 秒）"
  record unmeasured "ビルドの時間切れ（exit ${rc}）" "$build_s"
  exit 3
elif [ "$rc" -ne 0 ]; then
  say "ERROR: ビルドに失敗（exit ${rc}・${build_s} 秒）"
  tail -5 "$WORK/build.log" | mask | sed 's/^/  /'
  record failed "ビルドに失敗（exit ${rc}）" "$build_s"
  exit 1
fi
say "ビルド: ${build_s} 秒"

# --- 計測（tako-vd の上だけ。窓の置き場・後片付けは計測スクリプトと isolated-gui.sh が持つ）------
# - TAKO_NO_QUEUE: ビルドは上で済んでいる（中の cargo は確かめるだけ）ので、もう一度並ばない
# - TAKO_FOOTPRINT_STUB_CLAUDE: 隔離 HOME では `claude agents --json` の絞り込みが効かず、子の CPU が
#   claude CLI の版で揺れる（#2018）。夜ごとの比較を tako の変化だけにする
# - 呼び出し元の tako の接続情報・バイナリの指定は渡さない（隔離 GUI だけを相手にする）。
#   起動の区間の混み具合（#2652）は起票に使う tako と同じ 1 本で読ませる
# measure_once <名前> [計測へ足す env …] → 結果は MRC（終了コード）・MEASURE_S（秒）・
#   $WORK/measure-<名前>.log・$WORK/row-<名前>.tsv
measure_once() {
  local tag="$1" t0
  shift
  t0=$(date +%s)
  MRC=0
  run_limited "$MEASURE_TIMEOUT" "$WORK/measure-$tag.log" \
    env -u TAKO_BIN -u APP_BIN -u TAKO_SOCKET -u TAKO_TOKEN -u TAKO_PANE_ID \
    TAKO_NO_QUEUE=1 TAKO_FOOTPRINT_STUB_CLAUDE=1 TAKO_FOOTPRINT_OUT="$WORK/row-$tag.tsv" \
    ${TAKO_CLI:+"TAKO_FOOTPRINT_QUEUE_TAKO=$TAKO_CLI"} "$@" \
    /bin/bash "$MEASURE" std \
    || MRC=$?
  MEASURE_S=$(( $(date +%s) - t0 ))
  mask < "$WORK/measure-$tag.log" > "$DIR/last-run.log" 2>/dev/null || true
  # 夜間のログへは要点だけ（全文は last-run.log）
  grep -E '^  persist|^  direct|窓:|起動の混み具合:|未実測|ERROR|接続できない' "$WORK/measure-$tag.log" 2>/dev/null | mask | sed 's/^ *//; s/^/  /'
}
# 計測が結果の行を出したか
has_row() { grep -qv '^scenario' "$1" 2>/dev/null; }
# 本番の順番待ちの実行中の本数（読めなければ -）
queue_running() {
  local out=""
  [ -n "$TAKO_CLI" ] && out="$("$TAKO_CLI" queue --json 2>/dev/null | python3 "$PY" queue-busy 2>/dev/null)"
  echo "${out%% *}" | sed 's/^$/-/'
}
# 順番待ちの実行が 0 本になるまで待つ（上限 QUIET_WAIT 秒）。読めない（GUI が居ない等）なら待たずに進む。
# 空かなければ 1（最後に読んだ本数は QUIET_LAST）
wait_quiet() {
  local deadline=$(( $(date +%s) + QUIET_WAIT ))
  QUIET_LAST="-"
  while :; do
    QUIET_LAST="$(queue_running)"
    case "$QUIET_LAST" in -|0) return 0 ;; esac
    [ "$(date +%s)" -ge "$deadline" ] && return 1
    sleep "$QUIET_POLL"
  done
}

measure_once 1
rc=$MRC
measure_s=$MEASURE_S

if [ "$rc" -eq 4 ]; then
  # 原因は「未実測: … を開かない（<原因>。状態は …）」の括弧の中（形が違えば行ごと）
  line="$(grep -m1 '^未実測: ' "$WORK/measure-1.log" 2>/dev/null || true)"
  reason="$(printf '%s' "$line" | sed -n 's/^未実測: .*を開かない（\(.*\)。状態は.*$/\1/p')"
  [ -n "$reason" ] || reason="${line#未実測: }"
  reason="$(printf '%s' "$reason" | cut -c1-120)"
  say "未実測: tako-vd を用意できない（蓋閉じ・スリープ）。起票しない（${measure_s} 秒）"
  record unmeasured "tako-vd を用意できない${reason:+: ${reason}}" "$measure_s"
  exit 3
elif [ "$rc" -eq 124 ]; then
  say "未実測: 計測が上限 ${MEASURE_TIMEOUT} 秒に届いたので止めた"
  record unmeasured "計測の時間切れ（${MEASURE_TIMEOUT} 秒）" "$measure_s"
  exit 3
elif [ "$rc" -ne 0 ] || ! has_row "$WORK/row-1.tsv"; then
  say "ERROR: 計測が終了コード ${rc} で終わった（結果の行なし・${measure_s} 秒。全文は $(echo "$DIR/last-run.log" | mask)）"
  record failed "計測が終了コード ${rc} で終わった" "$measure_s"
  exit 1
fi
say "計測: ${measure_s} 秒"
ROW="$WORK/row-1.tsv"
NOTE="-"

# --- 起動の区間に重い処理が重なった夜は 1 回だけ測り直す（#2652）-------------------------
read -r busy load first_ms <<<"$(python3 "$PY" row-busy --row "$ROW")"
case "$busy" in ''|-|0|*[!0-9]*) busy=0 ;; esac
if [ "$busy" -gt 0 ] && [ "${TAKO_2652_LEGACY:-}" != 1 ]; then
  say "起動の区間に順番待ちの実行 ${busy} 本が重なった（load1 ${load}・起動 ${first_ms} ms）。空くのを待って 1 回だけ測り直す（上限 ${QUIET_WAIT} 秒）"
  fresh="$WORK/fresh-bin"
  src="$WT/target/release"
  if ! wait_quiet; then
    say "${QUIET_WAIT} 秒待っても順番待ちが空かない（実行 ${QUIET_LAST} 本）。測り直さず 1 回目の行を残す（起動時間は比べない）"
    NOTE="起動に重なり ${busy} 本。順番待ちが ${QUIET_WAIT} 秒空かず測り直さない"
  elif ! { [ -x "$src/tako" ] && [ -x "$src/tako-app" ] && mkdir -p "$fresh" \
      && command cp -f "$src/tako" "$src/tako-app" "$fresh/"; }; then
    say "測り直しのバイナリを用意できない（$(echo "$src" | mask)）。1 回目の行を残す（起動時間は比べない）"
    NOTE="起動に重なり ${busy} 本。バイナリを用意できず測り直さない"
  else
    # 新しいファイルへ複製したバイナリ = 初回 exec の検査を含めて建てたての夜と同じ条件で測る
    measure_once 2 TAKO_BIN="$fresh/tako" APP_BIN="$fresh/tako-app"
    if [ "$MRC" -eq 0 ] && has_row "$WORK/row-2.tsv"; then
      ROW="$WORK/row-2.tsv"
      measure_s=$MEASURE_S
      NOTE="測り直し（1 回目は起動 ${first_ms} ms・順番待ちの実行 ${busy} 本が重なった）"
      read -r busy2 _ second_ms <<<"$(python3 "$PY" row-busy --row "$ROW")"
      say "測り直し: ${MEASURE_S} 秒（起動 ${first_ms} → ${second_ms} ms）"
      case "$busy2" in ''|-|0|*[!0-9]*) ;; *) say "測り直しも起動の区間に順番待ちの実行 ${busy2} 本が重なった（起動時間は比べない）" ;; esac
    else
      say "測り直しが終了コード ${MRC} で終わった（${MEASURE_S} 秒）。1 回目の行を残す（起動時間は比べない）"
      NOTE="起動に重なり ${busy} 本。測り直しが終了コード ${MRC} で終わった"
    fi
  fi
fi
record ok "$NOTE" "$measure_s" "$ROW" || { say "ERROR: 履歴へ書けない"; exit 1; }

# --- 判定 -----------------------------------------------------------------------------
python3 "$PY" judge --history "$HISTORY" --state "$STATE" --out-dir "$WORK/judge" \
  --window "$WINDOW" --min "$MIN_N" --scale "$SCALE" || { say "ERROR: 判定に失敗"; exit 1; }
kv() { sed -n "s/^$1=//p" "$WORK/judge/result" | tail -1; }
sed 's/^/  /' "$WORK/judge/lines"
ACTION="$(kv action)"
say "判定: $(kv summary)"

case "$ACTION" in
  report) ;;
  continuing)
    say "前の夜から続いている悪化（$(kv open_metrics)・$(kv todo)）なので重ねて載せない"
    exit 0
    ;;
  clear)
    say "前に載せた悪化（$(kv todo)）は基準の範囲へ戻った。見張りの状態を消す（todo は人が片付ける）"
    rm -f "$STATE"
    exit 0
    ;;
  *) exit 0 ;;
esac

# --- 起票（同じ悪化の未完了の 1 件があれば書き換える）--------------------------------------
BODY="$WORK/judge/body.md"
PREV_SHA="$(kv prev_sha)"
if [ -n "$PREV_SHA" ]; then
  n_commits="$(git -C "$ROOT" rev-list --count "${PREV_SHA}..${SHA}" 2>/dev/null || echo '?')"
  web="$(git -C "$ROOT" remote get-url origin 2>/dev/null | sed -e 's#^git@github.com:#https://github.com/#' -e 's#\.git$##')"
  {
    echo
    echo "- 前に測れたコミット（\`$(printf '%s' "$PREV_SHA" | cut -c1-7)\`）からの変更: ${n_commits} 件"
    case "$web" in https://github.com/*) echo "  - ${web}/compare/${PREV_SHA}...${SHA}" ;; esac
  } >> "$BODY"
fi
TITLE="$(head -1 "$WORK/judge/title.txt")"

notify_fail() {
  osascript -e "display notification \"footprint の悪化を tako todo へ載せられなかった（ログを見る）\" with title \"tako 夜間リリース\"" >/dev/null 2>&1 || true
}
if [ -z "$TAKO_CLI" ]; then
  say "ERROR: tako が見つからないので悪化を載せられない: ${TITLE}"
  notify_fail
  exit 1
fi

existing=""
if list="$("$TAKO_CLI" todo list --json 2>/dev/null)"; then
  existing="$(printf '%s' "$list" | python3 "$PY" find-open --prefer "$(kv todo)" 2>/dev/null || true)"
fi
if [ -n "$existing" ]; then
  if out="$("$TAKO_CLI" todo update "$existing" --title "$TITLE" --body "$(cat "$BODY")" --attach "$HISTORY" --json 2>&1)"; then
    TODO_ID="$existing"
    say "未完了の ${existing} を書き換えた（新しく悪化した項目: $(kv new_metrics)）: ${TITLE}"
  else
    say "ERROR: ${existing} を書き換えられない: $(printf '%s' "$out" | head -1)"
    notify_fail
    exit 1
  fi
else
  if out="$("$TAKO_CLI" todo add "$TITLE" --body "$(cat "$BODY")" --kind review --project tako --attach "$HISTORY" --json 2>&1)"; then
    TODO_ID="$(printf '%s' "$out" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("id",""))' 2>/dev/null || true)"
    say "tako todo へ載せた（${TODO_ID:-id 不明}）: ${TITLE}"
  else
    # 状態は動かさない = 次の夜にもう一度載せようとする
    say "ERROR: tako todo へ載せられない（GUI が居ない等）: $(printf '%s' "$out" | head -1)"
    notify_fail
    exit 1
  fi
fi
since="$(kv since)"
printf 'open_metrics=%s\ntodo=%s\nsince=%s\nsha=%s\n' "$(kv open_metrics)" "${TODO_ID:-}" "${since:-$RUN_DATE}" "$SHA" > "$STATE"
exit 0
