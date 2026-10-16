#!/bin/bash
# fake-agent-net-2112.sh — ネット復帰の即時再開（#2112）の検証用の偽エージェント
#
# claude の画面の**形だけ**を描き（会話の項目・入力欄の区切り線・フッター）、
# 入力欄へ届いた文字を記録する。API には繋がない（ネットワークに 1 バイトも出ない）。
# 停止の文言は claude 2.1.294 の実画面・会話記録から写した。
#
# 使い方: fake-agent-net-2112.sh <mode> <name> <log>
#   mode:
#     enotfound  接続エラー（ENOTFOUND）で止まっている
#     refused    接続エラー（ECONNREFUSED）で止まっている
#     draft      ENOTFOUND で止まっているが、入力欄に人の未送信の文字がある
#     dialog     ENOTFOUND の後に選択ダイアログが出ている（入力欄が無い）
#     limit      使用制限で止まっている（#813 の担当 = 送られてはいけない）
#     login      ログイン失効で止まっている（#757 = 送られてはいけない）
#     question   人へ質問して入力待ち（送られてはいけない）
#   #2157 で足した形（scripts/test-net-resume-2157.sh が使う）:
#     notice      ENOTFOUND・ターンの締めの後に plugin.json の再読み込みの通知行（pane 2670 の実画面の形）
#     again       ENOTFOUND。届いた文で即座に同じ形のまま止まり直す（締めの行も同じ = 停止の鍵が重なる）
#     again-slow  ENOTFOUND。届いた文で 4 秒生成中（中断の案内）を見せてから同じ形で止まり直す
#     answered    ENOTFOUND の後に人の発言と返答・締め・通知（会話が先へ進んだ = 送られてはいけない）
#     toolafter   ENOTFOUND・締めの後にツールの呼び出しと結果（送られてはいけない）
#     generating  ENOTFOUND・締め・通知の後で生成中（中断の案内。送られてはいけない）
#   name: 記録に書く名前 / log: 届いた文字を 1 行ずつ追記するファイル
#
# 届いた行: `<unix 秒（小数 3 桁）> <name> RECEIVED <文字>`。ダイアログへのキーは `<name> KEY`。
# Enter で送られた文字は会話へ人の発言として積み、入力欄を空に戻す（claude と同じ見え方）。
#
# macOS 同梱の bash 3.2 で動く（#1499 / #1518）。

MODE="$1"
NAME="$2"
LOG="$3"

# 入力はバイト単位で読む（UTF-8 の文字を途中で割っても、つなげれば元のバイト列に戻る）
export LC_ALL=C

buf=""
answered=""
[ "$MODE" = "draft" ] && buf="まだ送っていない下書き"

ERR="⏺ API Error: Can't reach the API server — check your internet or DNS (ENOTFOUND)"
CLOSING="✻ Worked for 0s · done 6:55 PM"
NOTICE1="⏺ tako: plugin.json changed — reloaded (modules:"
NOTICE2="  hooks/register.ts unchanged; 14 hooks: …)"
conv=("❯ 作業を続けて" "")
case "$MODE" in
  enotfound | draft | dialog | again | again-slow)
    conv+=("$ERR") ;;
  notice)
    conv+=("$ERR" "" "✻ Cooked for 31m 35s · done 9:20 AM" "" "$NOTICE1" "$NOTICE2") ;;
  answered)
    conv+=("$ERR" "" "$CLOSING" "" "❯ つづき" "" "⏺ 続けます。" "" "✻ Worked for 12s · done 9:41 AM" "" "$NOTICE1" "$NOTICE2") ;;
  toolafter)
    conv+=("$ERR" "" "$CLOSING" "" "⏺ Bash(ls)" "  ⎿  a.txt") ;;
  generating)
    conv+=("$ERR" "" "$CLOSING" "" "$NOTICE1" "$NOTICE2" "" "✻ Thinking… (esc to interrupt)") ;;
  refused)
    conv+=("⏺ API Error: Connection refused — a firewall or proxy may be blocking it (ECONNREFUSED)") ;;
  limit)
    conv+=("⏺ You've hit your session limit · resets 3am (Asia/Tokyo)") ;;
  login)
    conv+=("⏺ API Error: Unable to connect to API (ENOTFOUND)"
      "  ⎿  OAuth refresh token is no longer valid; run /login to re-authenticate") ;;
  question)
    conv+=("⏺ どちらの方針で進めますか？ A か B を選んでください。") ;;
  *) echo "unknown mode: $MODE" >&2; exit 2 ;;
esac
case "$MODE" in
  notice | answered | toolafter | generating) : ;;
  *) conv+=("" "$CLOSING") ;;
esac

# 入力欄に出すのは先頭だけ（長い文を折り返させると区切り線を潰す）。
# 文字単位で切るために、ここだけ UTF-8 の locale で数える
input_view() {
  local LC_ALL=en_US.UTF-8
  printf '%s' "${buf:0:24}"
}

ROWS=24
RULE=""
FOOTER_HINT=""
SPINNER=""
[ "$MODE" = "generating" ] && FOOTER_HINT=" · esc to interrupt"
measure() {
  local size cols
  # 端末の実寸（コマンド置換の中の tput は端末を見ずに 80 桁を返すので stty で引く）
  size=$(stty size </dev/tty 2>/dev/null)
  ROWS=${size%% *}
  cols=${size##* }
  case "$ROWS$cols" in '' | *[!0-9]*) ROWS=24; cols=80 ;; esac
  RULE=$(printf '%*s' "$cols" '' | sed 's/ /─/g')
}

draw() {
  local l
  printf '\033[H\033[2J'
  printf ' ▐▛███▛█   fake agent (#2112 %s)\n\n' "$NAME"
  # 会話は入力欄の上に収まる末尾だけを描く（止まり直しで伸びても区切り線に重ねない）
  local n=${#conv[@]} from=0 i
  [ "$n" -gt $((ROWS - 7)) ] && from=$((n - (ROWS - 7)))
  i=$from
  while [ "$i" -lt "$n" ]; do
    printf '%s\n' "${conv[$i]}"
    i=$((i + 1))
  done
  [ -n "$SPINNER" ] && printf '%s\n' "$SPINNER"
  if [ "$MODE" = "dialog" ] && [ -z "$answered" ]; then
    printf '\n Select model\n ❯ 1. Default (recommended)\n   2. Opus\n   3. Sonnet\n\n Enter to confirm · Esc to exit\n'
  else
    printf '\033[%d;1H' $((ROWS - 3))
    printf '%s\n❯ %s\n%s\n  ⏵⏵ auto mode on (shift+tab to cycle)%s' "$RULE" "$(input_view)" "$RULE" "$FOOTER_HINT"
  fi
}

# 受けた時刻（ミリ秒まで。macOS 同梱の perl。bash 3.2 の date は秒までしか出せない）
now_ms() { /usr/bin/perl -MTime::HiRes=time -e 'printf "%.3f", time'; }

submit() {
  printf '%s %s RECEIVED %s\n' "$(now_ms)" "$NAME" "$buf" >> "$LOG"
  case "$MODE" in
    again)
      # 届いた文で即座に同じ形で止まり直す（回線は戻ったのに API だけまだ届かない等）
      conv+=("" "❯ $(input_view)" "" "$ERR" "" "$CLOSING") ;;
    again-slow)
      # 4 秒生成中を見せてから止まり直す（そのあいだはキーを読まない = 端末が溜めておく）
      conv+=("" "❯ $(input_view)" "")
      buf=""
      SPINNER="✻ Thinking… (esc to interrupt)"
      FOOTER_HINT=" · esc to interrupt"
      draw
      sleep 4
      SPINNER=""
      FOOTER_HINT=""
      conv+=("$ERR" "" "$CLOSING") ;;
    *)
      conv+=("❯ $(input_view)" "" "⏺ （偽）続きを受け取りました" "") ;;
  esac
  buf=""
  answered=1
}

cleanup() {
  stty sane 2>/dev/null
  printf '\033[?2004l'
}
# tako:exit-guard-ok source されない実行ファイル（偽のエージェント）で set -e も使わない。後始末は端末の設定を戻すだけ
trap cleanup EXIT
trap 'measure; draw' WINCH
measure
in_paste=""

stty -icanon -echo min 1 time 0 2>/dev/null
# ブラケットペーストを求める（tako は貼り付けを ESC[200~ … ESC[201~ で包んで送る）
printf '\033[?2004h'
draw

while :; do
  IFS= read -r -s -n1 -d '' c || exit 0
  if [ "$MODE" = "dialog" ] && [ -z "$answered" ]; then
    printf '%s %s KEY\n' "$(date +%s)" "$NAME" >> "$LOG"
    continue
  fi
  case "$c" in
    $'\033')
      # CSI を読む。ブラケットペーストの印（200~ / 201~）のあいだは描き直さない
      # （1 バイトごとに描くと遅い）。それ以外（矢印キー等）は読み捨てる
      IFS= read -r -s -n1 -d '' c2 || exit 0
      if [ "$c2" = "[" ]; then
        seq=""
        while IFS= read -r -s -n1 -d '' c3; do
          seq="$seq$c3"
          case "$c3" in
            [@-~]) break ;;
          esac
        done
        case "$seq" in
          200~) in_paste=1 ;;
          201~) in_paste=""; draw ;;
        esac
      fi
      ;;
    $'\n' | $'\r')
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
