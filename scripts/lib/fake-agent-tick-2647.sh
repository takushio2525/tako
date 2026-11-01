#!/bin/bash
# fake-agent-tick-2647.sh — 2 秒 tick の判定（#2647）の検証用の偽エージェント
#
# claude の画面の**形だけ**を alternate screen に描き（会話の項目・入力欄の区切り線・フッター）、
# 入力欄へ届いた文字とキーを記録する。API には繋がない（ネットワークに 1 バイトも出ない）。
# 形は `scripts/lib/fake-agent-net-2112.sh`（claude 2.1.294 の実画面から写したもの）に揃えた。
#
# 使い方: fake-agent-tick-2647.sh <mode> <name> <log>
#   mode:
#     ctx     ふつうに入力待ち。フッターに `ctx NN%`（NN = FAKE_CTX。既定 30）
#     limit   使用制限で止まっている（`You've hit your session limit · resets <FAKE_RESET> (Asia/Tokyo)`）
#     net     接続エラー（ENOTFOUND）で止まっている
#     queued  入力欄に claude のキュー滞留のヒント（`Press up to edit queued messages`）。
#             ↑ を受けるとキューの中身を入力欄へ戻し、Enter で送る（claude と同じ見え方）
#   name: 記録に書く名前 / log: 届いた文字・キーを 1 行ずつ追記するファイル
#
# env:
#   FAKE_CTX     フッターの ctx%（既定 30）
#   FAKE_RESET   limit の解除時刻の表記（既定 `3am`）
#   FAKE_REDRAW  同じ画面を描き直す間隔（秒・整数。既定 0 = 描き直さない）。**中身は変えない**
#                （画面の指紋を動かさない = 自動復帰の「画面の静止」を崩さない）で PTY の出力だけを作る
#
# 届いた行: `<unix 秒（小数 3 桁）> <name> RECEIVED <文字>` / `<name> UP` / `<name> ENTER`。
#
# macOS 同梱の bash 3.2 で動く（#1499 / #1518）。

MODE="$1"
NAME="$2"
LOG="$3"
CTX="${FAKE_CTX:-30}"
RESET="${FAKE_RESET:-3am}"
REDRAW="${FAKE_REDRAW:-0}"
case "$REDRAW" in '' | *[!0-9]*) REDRAW=0 ;; esac

# 入力はバイト単位で読む（UTF-8 の文字を途中で割っても、つなげれば元のバイト列に戻る）
export LC_ALL=C

buf=""
queued_hint=""
[ "$MODE" = "queued" ] && queued_hint=1

ERR="⏺ API Error: Can't reach the API server — check your internet or DNS (ENOTFOUND)"
CLOSING="✻ Worked for 0s · done 6:55 PM"
conv=("❯ 作業を続けて" "")
case "$MODE" in
  ctx | queued) conv+=("⏺ 了解しました。続けます。") ;;
  limit) conv+=("⏺ You've hit your session limit · resets $RESET (Asia/Tokyo)") ;;
  net) conv+=("$ERR") ;;
  *) echo "unknown mode: $MODE" >&2; exit 2 ;;
esac
conv+=("" "$CLOSING")

input_view() {
  local LC_ALL=en_US.UTF-8
  printf '%s' "${buf:0:24}"
}

ROWS=24
COLS=80
RULE=""
measure() {
  local size
  size=$(stty size </dev/tty 2>/dev/null)
  ROWS=${size%% *}
  COLS=${size##* }
  case "$ROWS$COLS" in '' | *[!0-9]*) ROWS=24; COLS=80 ;; esac
  RULE=$(printf '%*s' "$COLS" '' | sed 's/ /─/g')
}

draw() {
  local n=${#conv[@]} from=0 i prompt
  printf '\033[H\033[2J'
  printf ' ▐▛███▛█   fake agent (#2647 %s)\n\n' "$NAME"
  [ "$n" -gt $((ROWS - 7)) ] && from=$((n - (ROWS - 7)))
  [ "$from" -lt 0 ] && from=0
  i=$from
  while [ "$i" -lt "$n" ]; do
    printf '%s\n' "${conv[$i]}"
    i=$((i + 1))
  done
  if [ -n "$queued_hint" ] && [ -z "$buf" ]; then
    prompt="Press up to edit queued messages"
  else
    prompt="$(input_view)"
  fi
  printf '\033[%d;1H' $((ROWS - 3))
  printf '%s\n❯ %s\n%s\n  [Opus 5 (1M context) · 1H] ctx %s%% · 5h 40%% · 7d 20%%' \
    "$RULE" "$prompt" "$RULE" "$CTX"
}

now_ms() { /usr/bin/perl -MTime::HiRes=time -e 'printf "%.3f", time'; }

submit() {
  printf '%s %s RECEIVED %s\n' "$(now_ms)" "$NAME" "$buf" >> "$LOG"
  conv+=("❯ $(input_view)" "" "⏺ （偽）続きを受け取りました" "")
  buf=""
}

cleanup() {
  stty sane 2>/dev/null
  printf '\033[?2004l\033[?1049l'
}
# tako:exit-guard-ok source されない実行ファイル（偽のエージェント）で set -e も使わない。後始末は端末の設定を戻すだけ
trap cleanup EXIT
trap 'measure; draw' WINCH
measure
in_paste=""

stty -icanon -echo min 1 time 0 2>/dev/null
# alternate screen（claude の TUI と同じ）+ ブラケットペースト（tako は貼り付けを ESC[200~ … ESC[201~ で包む）
printf '\033[?1049h\033[?2004h'
draw

# 1 バイト読む。FAKE_REDRAW があれば、その秒数だけ何も来なければ同じ画面を描き直す。
# bash 3.2 の read -t は時間切れと EOF を区別しない（どちらも 1）ので、待った秒数で見分ける:
# 時間切れなら $SECONDS が 1 以上進んでいる（整数秒の境目を必ず 1 つ跨ぐ）。EOF は即座に返る
read_key() {
  local t0
  while :; do
    if [ "$REDRAW" -gt 0 ]; then
      t0=$SECONDS
      if IFS= read -r -s -n1 -d '' -t "$REDRAW" c; then
        return 0
      fi
      [ $((SECONDS - t0)) -ge 1 ] || return 1
      draw
    else
      IFS= read -r -s -n1 -d '' c
      return $?
    fi
  done
}

while :; do
  read_key || exit 0
  case "$c" in
    $'\033')
      read_key || exit 0
      if [ "$c" = "[" ]; then
        seq=""
        while read_key; do
          seq="$seq$c"
          case "$c" in
            [@-~]) break ;;
          esac
        done
        case "$seq" in
          200~) in_paste=1 ;;
          201~) in_paste=""; draw ;;
          A)
            printf '%s %s UP\n' "$(now_ms)" "$NAME" >> "$LOG"
            if [ -n "$queued_hint" ] && [ -z "$buf" ]; then
              queued_hint=""
              buf="キューに残っていた指示"
              draw
            fi
            ;;
        esac
      fi
      ;;
    $'\n' | $'\r')
      printf '%s %s ENTER\n' "$(now_ms)" "$NAME" >> "$LOG"
      if [ -n "$buf" ]; then
        submit
      fi
      draw
      ;;
    $'\177' | $'\010')
      buf="${buf%?}"
      draw
      ;;
    *)
      buf="$buf$c"
      [ -z "$in_paste" ] && draw
      ;;
  esac
done
