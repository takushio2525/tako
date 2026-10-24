#!/bin/bash
# test-terminal-queries-2350.sh — 端末の問い合わせへの応答の実経路テスト（Issue #2350）
#
# 何を確かめるか: ペインの中のプログラムが端末へ尋ねる
#   OSC 10 / 11 / 12 / 4（前景・背景・カーソル・パレットの色）と CSI 14 t / 18 t（文字領域のピクセル数・セル数）
# に、直接のペイン（器なし）と器（tmux）越しのペインの両方で答えが返るか。色は今のテーマ
# （`tako theme colors`）と、寸法はペインの TIOCGWINSZ（カーネルが子へ見せている値）と突き合わせる。
# OSC 52 の読み出し（クリップボードの中身）には答えない（秘匿情報を子へ渡さない）。
#
# 読み方（固定の待ちは置かない）: ペインの中で perl のプローブを走らせ、raw にした端末へ
# 「問い合わせ + DA1（CSI c）」を書き、DA1 の答えが来た時点で読み終える。端末は DA1 に必ず答えるので、
# 問い合わせに答えない端末でも DA1 の答えで締まる（天井は 3 秒 = DA1 すら来ないとき）。
# DA1 より前に届いたバイト列が問い合わせへの答え。結果は画面ではなくファイルへ書かせる。
#
# MODE=measure は表を出すだけ（段 1 の切り分け・修正前の木で走らせる）。既定は判定まで行い、
# A/B（TAKO_2350_LEGACY=1 = 修正前の挙動）で修正前は答えが無いことも確かめる。
#
# **本番の tako / 設定 / 本番の tmux（-L tako）には一切触らない**（data / discovery / HOME は mktemp 配下、
# tmux は起動ごとの専用ソケット、落とすのは自分で起こした pid と自分のソケットだけ）。窓は常設の
# 仮想ディスプレイ（tako-vd）へ出る（#1141）。
#
# 使い方: bash scripts/test-terminal-queries-2350.sh
#
# **CI には載せない**（実 GUI + 仮想ディスプレイが要る）。手元で走らせる実経路テスト。
set -uo pipefail
# **本番 GUI を指す env を最初に落とす**（#1449 / #1450 で本番にペインが漏れた実例が 2 件）
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_MCP_URL TAKO_ORCHESTRATOR_ROLE
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MODE=${MODE:-check}
PASS=0
FAIL=0
pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
fail() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }
check() { if [ "$2" = 1 ]; then pass "$1"; else fail "$1"; fi; }
# ソケットのパスは sun_path（103 バイト）に収まる短い置き場へ
TMP="$(mktemp -d /tmp/tk2350-XXXX)"
APP_PID=""
cleanup() {
  local p name sockdir
  [ -n "$APP_PID" ] && stop_isolated_gui "$APP_PID"
  # tmux は kill-server の後もソケットファイルを残すことがあるので、自分の名前のものは消す
  sockdir="${TMUX_TMPDIR:-/tmp}/tmux-$(id -u)"
  for p in "$TMP"/*/tmux; do
    [ -f "$p" ] || continue
    name=$(cat "$p")
    tmux -L "$name" kill-server >/dev/null 2>&1
    rm -f "${sockdir:?}/${name:?}" "${sockdir:?}/${name:?}="
  done
  rm -rf "${TMP:?}"
}
trap cleanup EXIT
# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
isolated_gui_bins || exit 1
export HOME="$TMP/home"
mkdir -p "$HOME"
export TAKO_ISOLATED=1

# --- プローブ（ペインの中で走る） ---------------------------------------------------------
cat > "$TMP/probe.pl" <<'PL'
use strict;
use warnings;
use Time::HiRes qw(time);
use IO::Select;
my $out = shift @ARGV or die "usage: probe.pl <out>";
my @all = (
    [ 'osc11',    "\e]11;?\a" ],
    [ 'osc10',    "\e]10;?\a" ],
    [ 'csi14t',   "\e[14t" ],
    [ 'csi18t',   "\e[18t" ],
    [ 'osc12',    "\e]12;?\a" ],
    [ 'osc4_1',   "\e]4;1;?\a" ],
    [ 'osc4_200', "\e]4;200;?\a" ],
    [ 'osc11st',  "\e]11;?\e\\" ],
    [ 'dsr996',   "\e[?996n" ],
    [ 'osc52',    "\e]52;c;?\a" ],
    [ 'osc4_bad', "\e]4;999;?\a" ],
    [ 'osc11_bad', "\e]11;nope\a" ],
);
open( my $tty, '+<', '/dev/tty' ) or die "no tty: $!";
binmode $tty;
# 子へ見せている寸法（TIOCGWINSZ）。CSI 14 t / 18 t の答えと突き合わせる
my $ws = "\0" x 8;
my $req = $^O eq 'darwin' ? 0x40087468 : 0x5413;
ioctl( $tty, $req, $ws ) or die "TIOCGWINSZ: $!";
my ( $rows, $cols, $xpix, $ypix ) = unpack( 'S4', $ws );
chomp( my $saved = `stty -g < /dev/tty` );
system('stty raw -echo < /dev/tty');
my $sel = IO::Select->new($tty);
my @lines = ("W:$rows $cols $xpix $ypix");
for my $q (@all) {
    my ( $name, $seq ) = @$q;
    syswrite( $tty, $seq . "\e[c" );
    my ( $buf, $da1, $start ) = ( '', 0, time );
    while ( time - $start < 3 ) {
        last unless $sel->can_read( 3 - ( time - $start ) );
        my $n = sysread( $tty, my $chunk, 4096 );
        last unless $n;
        $buf .= $chunk;
        if ( $buf =~ s/\e\[\?[0-9;]*c// ) { $da1 = 1; last; }
    }
    my $ms = int( ( time - $start ) * 1000 );
    ( my $vis = $buf ) =~ s/\e/^[/g;
    $vis =~ s/\a/^G/g;
    $vis =~ s/([\x00-\x1f])/sprintf('^%c', ord($1) + 64)/ge;
    push @lines, "R:$name=[$vis] ${ms}ms da1=$da1";
}
system("stty $saved < /dev/tty");
open( my $fh, '>', "$out.part" ) or die;
print $fh "$_\n" for @lines, 'R:done';
close $fh;
rename "$out.part", $out;
print "probe done\n";
PL

# --- 起動ごとに data / discovery / tmux を分ける（2 回目が前の器を拾わない） --------------
prepare_dirs() {
  LABEL="$1"
  local dir="$TMP/$LABEL"
  mkdir -p "$dir/d" "$dir/disc" "$dir/orch"
  export TAKO_DATA_DIR="$dir/d"
  export TAKO_DISCOVERY_DIR="$dir/disc"
  export TAKO_ORCHESTRATOR_DIR="$dir/orch"
  export TAKO_SESSIONS_FILE="$dir/sessions.yaml"
  export TAKO_PANE_LOG_DIR="$dir/panelogs"
  export TAKO_WORKERS_FILE="$dir/workers.yaml"
  export TAKO_TMUX_SOCKET="tk2350-$$-$LABEL"
  echo "$TAKO_TMUX_SOCKET" > "$dir/tmux"
  printf '{\n  "language": "ja"\n}\n' > "$TAKO_DATA_DIR/settings.json"
  GUI_DIR="$dir"
  PROBE_SEQ=0
}
start_gui() {
  launch_isolated_gui "$GUI_DIR/app.log" "$@" || exit $?
  APP_PID=$ISOLATED_GUI_PID
  wait_isolated_gui "$GUI_DIR/app.log" 300 || exit 1
}
stop_gui() { stop_isolated_gui "$APP_PID"; APP_PID=""; }
first_pane() {
  "$TAKO_BIN" list 2>/dev/null | perl -MJSON::PP -0e 'my $j = eval { decode_json(<STDIN>) } or exit; my $t = ($j->{tabs} // $j)->[0]; print $t->{panes}[0]{id} // $t->{panes}[0]{pane_id} // ""'
}
# ペインのシェルがプロンプト（isolated-gui.sh の既定の .zshrc = `tako <dir> %`）を出すまで待つ。
# 出る前に送った行は器の attach の途中で落ちることがあり、送り直すとプローブの読みに
# 打鍵が混ざる（raw の端末から読むので）。だから「出てから 1 回だけ送る」
wait_prompt() {
  local pane="$1" i
  for i in $(seq 1 300); do
    "$TAKO_BIN" read --pane "$pane" 2>/dev/null | grep -q '^tako [^ ]* %' && return 0
    sleep 0.1
  done
  return 1
}
# ペインでプローブを走らせ、結果のファイルのパスを返す（読めなければ空）
run_probe() {
  local pane="$1" out i
  PROBE_SEQ=$((PROBE_SEQ + 1))
  out="$GUI_DIR/probe-$PROBE_SEQ.txt"
  wait_prompt "$pane" || { echo ""; return; }
  "$TAKO_BIN" send --pane "$pane" "perl $TMP/probe.pl $out" >/dev/null 2>&1
  for i in $(seq 1 600); do
    if grep -q '^R:done' "$out" 2>/dev/null; then
      echo "$out"
      return
    fi
    sleep 0.1
  done
  echo ""
}
# 結果のファイルから 1 種の答え（`[` と `]` の間）を読む
reply_of() { sed -n "s/^R:$2=\[\(.*\)\] [0-9]*ms da1=.*/\1/p" "$1"; }
ms_of() { sed -n "s/^R:$2=\[.*\] \([0-9]*\)ms da1=.*/\1/p" "$1"; }
winsize_of() { sed -n 's/^W://p' "$1"; }
# テーマの色（`tako theme colors` の hex）を OSC の答えの形（rgb:rrrr/gggg/bbbb）へ
theme_rgb() {
  "$TAKO_BIN" theme colors 2>/dev/null | perl -MJSON::PP -0e '
    my $j = eval { decode_json(<STDIN>) } or exit;
    my $h = $j->{colors}{$ARGV[0]}{hex} // exit;
    my ($r, $g, $b) = $h =~ /^#(..)(..)(..)$/ or exit;
    print "rgb:$r$r/$g$g/$b$b";' "$1"
}
health_json() { "$TAKO_BIN" check-health --json 2>/dev/null; }
# check-health の terminal_queries の値を 1 つ読む。`legacy` / `color_scheme_panes` のような節の値か、
# `osc11.answered` / `osc52_read.by` のような種類ごとの値（kinds[] の kind で引く）
tq_field() {
  perl -MJSON::PP -0e '
    binmode STDOUT, ":encoding(UTF-8)";
    my $j = eval { decode_json(<STDIN>) } or do { print "unreadable"; exit };
    my $t = $j->{terminal_queries} // do { print "null"; exit };
    my ($head, @rest) = split /\./, $ARGV[0];
    my ($row) = grep { $_->{kind} eq $head } @{ $t->{kinds} // [] };
    my $v = $row ? $row : $t->{$head};
    for my $k (@rest) { $v = ref $v eq "HASH" ? $v->{$k} : undef }
    print !defined $v ? "null"
      : JSON::PP::is_bool($v) ? ($v ? 1 : 0)
      : ref $v ? JSON::PP->new->canonical->encode($v) : $v;' "$1"
}
# MCP の stdio ブリッジ（`tako mcp serve`）から tako_check_health を 1 回呼び、terminal_queries の要点を返す
mcp_terminal_queries() {
  local sock tok
  sock=$(perl -MJSON::PP -0e 'print decode_json(<>)->{socket}' "$TAKO_DISCOVERY_DIR/control.json")
  tok=$(perl -MJSON::PP -0e 'print decode_json(<>)->{token}' "$TAKO_DISCOVERY_DIR/control.json")
  {
    printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t2350","version":"0"}}}'
    printf '%s\n' '{"jsonrpc":"2.0","method":"notifications/initialized"}'
    printf '%s\n' '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"tako_check_health","arguments":{}}}'
    sleep 10
  } | TAKO_SOCKET="$sock" TAKO_TOKEN="$tok" "$TAKO_BIN" mcp serve 2>/dev/null | perl -MJSON::PP -ne '
    my $m = eval { decode_json($_) } or next;
    next unless ($m->{id} // 0) == 2;
    binmode STDOUT, ":encoding(UTF-8)";
    my $t = $m->{result}{content}[0]{text} // ""; my $h = eval { JSON::PP->new->decode($t) } or do { print "isError: $t\n"; next };
    my $q = $h->{terminal_queries} // {};
    print map { "$_->{kind}: by=$_->{by} answered=" . ($_->{answered} // "null") . "\n" } @{ $q->{kinds} // [] };
    print "summary: $q->{summary}\n";'
}
mask() { sed -e "s#$TMP#<TMP>#g" -e "s#$REPO_ROOT#<REPO>#g"; }

# 1 回のプローブの結果を表の 1 行ずつで出す
print_probe() {
  local f="$1" k
  echo "    寸法（TIOCGWINSZ rows cols xpix ypix）: $(winsize_of "$f")"
  for k in osc10 osc11 osc11st osc12 osc4_1 osc4_200 dsr996 csi14t csi18t osc52 osc4_bad osc11_bad; do
    printf '    %-10s %-44s %s\n' "$k" "[$(reply_of "$f" "$k")]" "$(sed -n "s/^R:$k=\[.*\] //p" "$f")"
  done
}

# 色の答えがテーマの値と一致するか（直接のペイン = tako が答える / 器越し = tmux が tako から学んだ値を答える）
check_colors() {
  local f="$1" what="$2"
  local fg bg cur red
  fg=$(theme_rgb foreground)
  bg=$(theme_rgb background)
  cur=$(theme_rgb cursor)
  red=$(theme_rgb ansi_red)
  check "${what}: OSC 11 の答えがテーマの背景（${bg}）" "$( [ "$(reply_of "$f" osc11)" = "^[]11;${bg}^G" ] && echo 1)"
  check "${what}: OSC 10 の答えがテーマの前景（${fg}）" "$( [ "$(reply_of "$f" osc10)" = "^[]10;${fg}^G" ] && echo 1)"
  check "${what}: ST で尋ねた OSC 11 には ST で答える" "$( [ "$(reply_of "$f" osc11st)" = "^[]11;${bg}^[\\" ] && echo 1)"
  if [ "${3:-}" = direct ]; then
    check "${what}: OSC 12 の答えがテーマのカーソル（${cur}）" "$( [ "$(reply_of "$f" osc12)" = "^[]12;${cur}^G" ] && echo 1)"
    check "${what}: OSC 4;1 の答えがテーマの ANSI 赤（${red}）" "$( [ "$(reply_of "$f" osc4_1)" = "^[]4;1;${red}^G" ] && echo 1)"
    # 200 = 6x6x6 の立方体の (5,0,4) = rgb(255,0,215)（xterm の標準）
    check "${what}: OSC 4;200 の答えが 256 色の立方体の値" "$( [ "$(reply_of "$f" osc4_200)" = "^[]4;200;rgb:ffff/0000/d7d7^G" ] && echo 1)"
  fi
}
check_sizes() {
  local f="$1" what="$2" rows cols xpix ypix
  read -r rows cols xpix ypix <<<"$(winsize_of "$f")"
  check "${what}: CSI 14 t の答えが TIOCGWINSZ のピクセル数（${ypix}x${xpix}）" "$( [ -n "$ypix" ] && [ "$ypix" != 0 ] && [ "$(reply_of "$f" csi14t)" = "^[[4;${ypix};${xpix}t" ] && echo 1)"
  check "${what}: CSI 18 t の答えが TIOCGWINSZ のセル数（${rows}x${cols}）" "$( [ "$(reply_of "$f" csi18t)" = "^[[8;${rows};${cols}t" ] && echo 1)"
}
check_declines() {
  local f="$1" what="$2"
  check "${what}: OSC 52 の読み出しには答えない" "$( [ -z "$(reply_of "$f" osc52)" ] && grep -q '^R:osc52=\[\] [0-9]*ms da1=1' "$f" && echo 1)"
  check "${what}: 不正な問い合わせ（OSC 4;999 / OSC 11;nope）には答えない" "$( [ -z "$(reply_of "$f" osc4_bad)" ] && [ -z "$(reply_of "$f" osc11_bad)" ] && echo 1)"
}

# 1 つの構成（直接 / 器越し × 修正前 / 修正後）を回す
run_case() {
  local label="$1" what="$2"
  shift 2
  echo "== ${what}"
  prepare_dirs "$label"
  start_gui "$@"
  PANE=$(first_pane)
  F=$(run_probe "$PANE")
  if [ -z "$F" ]; then
    fail "${what}: プローブの結果が読めない"
    "$TAKO_BIN" read --pane "$PANE" 2>&1 | tail -n 8 | mask
    stop_gui
    return 1
  fi
  print_probe "$F"
  [ -n "${OUT_DIR:-}" ] && command cp -f "$F" "$OUT_DIR/$label-$(basename "$F")"
  return 0
}


# 明暗の答え（CSI ? 996 n → CSI ? 997 ; 1 = ダーク / 2 = ライト）
check_scheme() {
  check "${2}: CSI ? 996 n の答えが ${3}（997;${4}）" "$( [ "$(reply_of "$1" dsr996)" = "^[[?997;${4}n" ] && echo 1)"
}
# 修正前の形: tako が答える種類はどれも空（CSI 18 t だけは端末が答える）
check_legacy_silent() {
  local f="$1" what="$2" k silent=1
  for k in osc10 osc11 osc11st osc12 osc4_1 dsr996; do
    [ -z "$(reply_of "$f" "$k")" ] || silent=0
  done
  check "${what}: 色（OSC 10 / 11 / 12 / 4）と明暗（CSI ? 996 n）に答えない" "$silent"
}
# ペインを下へ分割して高さを変え、変わった寸法で CSI 14 t / 18 t が答えるかを見る（境界: 寸法が変わった直後）
check_after_split() {
  local what="$1" before after p2
  before=$(winsize_of "$F")
  p2=$("$TAKO_BIN" split --pane "$PANE" --down 2>/dev/null | perl -MJSON::PP -0e 'my $j = eval { decode_json(<STDIN>) } or exit; print $j->{pane} // $j->{pane_id} // ""')
  F=$(run_probe "$PANE")
  [ -n "$F" ] || { fail "${what}: 分割の後のプローブの結果が読めない"; return; }
  after=$(winsize_of "$F")
  echo "    分割（新ペイン ${p2:-?}）の前の寸法: ${before} → 後: ${after}"
  check "${what}: 分割でペインの寸法が変わった（前提）" "$( [ "$before" != "$after" ] && echo 1)"
  check_sizes "$F" "${what}（分割の直後）"
}

if [ "$MODE" = measure ]; then
  run_case direct "直接のペイン（TAKO_PERSIST なし）" && stop_gui
  run_case persist "器越しのペイン（TAKO_PERSIST=1）" TAKO_PERSIST=1 && stop_gui
  exit 0
fi

# ---------------------------------------------------------------------------------------------
run_case legacy-direct "① 修正前（TAKO_2350_LEGACY=1）: 直接のペイン" TAKO_2350_LEGACY=1 || exit 1
check_legacy_silent "$F" "①"
check "①: CSI 14 t にも答えない" "$( [ -z "$(reply_of "$F" csi14t)" ] && echo 1)"
check "①: CSI 18 t は端末（alacritty の Term）が修正前から答える" "$(grep -q '^R:csi18t=\[^\[\[8;' "$F" && echo 1)"
H=$(health_json)
echo "    check-health: legacy=$(tq_field legacy <<<"$H") osc11.by=$(tq_field osc11.by <<<"$H")"
check "①: check-health の terminal_queries が legacy で osc11 は declined" "$( [ "$(tq_field legacy <<<"$H")" = 1 ] && [ "$(tq_field osc11.by <<<"$H")" = declined ] && echo 1)"
stop_gui

run_case legacy-persist "② 修正前（TAKO_2350_LEGACY=1）: 器越しのペイン" TAKO_PERSIST=1 TAKO_2350_LEGACY=1 || exit 1
check "②: 器越しも OSC 10 / 11 に答えが無い（tmux が tako から色を学べない）" "$( [ -z "$(reply_of "$F" osc10)" ] && [ -z "$(reply_of "$F" osc11)" ] && echo 1)"
H=$(health_json)
check "②: mode 2031 の購読を覚えない（color_scheme_panes が空）" "$( [ "$(tq_field color_scheme_panes <<<"$H")" = '[]' ] && echo 1)"
stop_gui

run_case direct "③ 修正後: 直接のペイン" || exit 1
check_colors "$F" "③" direct
check_scheme "$F" "③" ダーク 1
check_sizes "$F" "③"
check_declines "$F" "③"
H=$(health_json)
echo "    check-health: legacy=$(tq_field legacy <<<"$H") osc11=$(tq_field osc11.answered <<<"$H") osc52_read.declined=$(tq_field osc52_read.declined <<<"$H") color_scheme_panes=$(tq_field color_scheme_panes <<<"$H")"
check "③: check-health が答えた回数を数える（osc11 は BEL / ST の 2 回以上・osc52_read は断り 1 回以上・csi18t は emulator）" "$( [ "$(tq_field legacy <<<"$H")" = 0 ] && [ "$(tq_field osc11.answered <<<"$H")" -ge 2 ] && [ "$(tq_field osc52_read.declined <<<"$H")" -ge 1 ] && [ "$(tq_field osc52_read.by <<<"$H")" = declined ] && [ "$(tq_field csi18t.by <<<"$H")" = emulator ] && echo 1)"
HT=$("$TAKO_BIN" check-health 2>/dev/null | grep '^端末の問い合わせ: ')
echo "    CLI: $HT"
check "③: 人が読む出力に要約 1 行" "$(grep -q '^端末の問い合わせ: 色（OSC 10 / 11 / 12 / 4）.*OSC 52 の読み出しには答えない（起動から応答 [0-9]* 回' <<<"$HT" && echo 1)"
M=$(mcp_terminal_queries)
echo "$M" | sed 's/^/    MCP: /'
check "③: MCP（stdio ブリッジ）の tako_check_health にも同じ表" "$(grep -q '^osc11: by=tako answered=[1-9]' <<<"$M" && grep -q '^osc52_read: by=declined' <<<"$M" && grep -q '^summary: 端末の問い合わせ: ' <<<"$M" && echo 1)"
"$TAKO_BIN" theme light >/dev/null 2>&1
F=$(run_probe "$PANE")
if [ -n "$F" ]; then
  print_probe "$F"
  check_colors "$F" "③ ライトへ切り替えた後" direct
  check_scheme "$F" "③ ライトへ切り替えた後" ライト 2
else
  fail "③: テーマを切り替えた後のプローブの結果が読めない"
fi
check_after_split "③"
stop_gui

run_case persist "④ 修正後: 器越しのペイン" TAKO_PERSIST=1 || exit 1
check_colors "$F" "④"
check_scheme "$F" "④" ダーク 1
check_sizes "$F" "④"
H=$(health_json)
echo "    check-health: color_scheme_panes=$(tq_field color_scheme_panes <<<"$H")（このペイン ${PANE}）"
check "④: 器（tmux）のペインが mode 2031 の購読者に並ぶ" "$(perl -MJSON::PP -e 'exit !(grep { $_ == $ARGV[1] } @{ decode_json($ARGV[0]) })' "$(tq_field color_scheme_panes <<<"$H")" "$PANE" 2>/dev/null && echo 1)"
"$TAKO_BIN" theme light >/dev/null 2>&1
F=$(run_probe "$PANE")
if [ -n "$F" ]; then
  print_probe "$F"
  check_colors "$F" "④ ライトへ切り替えた後（tmux が問い合わせ直した値）"
  check_scheme "$F" "④ ライトへ切り替えた後" ライト 2
else
  fail "④: テーマを切り替えた後のプローブの結果が読めない"
fi
H=$(health_json)
check "④: テーマの切り替えの通知を数える（scheme_notices ≥ 1）" "$( [ "$(tq_field scheme_notices <<<"$H")" -ge 1 ] 2>/dev/null && echo 1)"
check_after_split "④"
stop_gui

echo
echo "結果: PASS=$PASS FAIL=$FAIL"
[ "$FAIL" = 0 ]
