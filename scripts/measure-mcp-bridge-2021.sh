#!/bin/bash
# tako:run bash scripts/measure-mcp-bridge-2021.sh target/release/tako
# measure-mcp-bridge-2021.sh — MCP の stdio ブリッジ（`tako mcp serve`）1 本の footprint と、
# 起動から initialize の応答までの時間を同じ手順で測り、渡したバイナリどうしの応答を突き合わせる（#2021）
#
# 何のためか:
#   ブリッジは claude（codex / agy）1 本につき 1 本、claude が終わるまで常駐する（10/13 の本番で 26 本）。
#   #2021 の前後と回帰を、毎回同じメッセージ列・同じ待ち方で比べるため。判定はしない（数値を出すだけ）。
#
# 使い方: bash scripts/measure-mcp-bridge-2021.sh [名前=]<tako の実行ファイル>...
#   - .app の中の `…/Contents/MacOS/tako` を渡すと、隣の `tako-mcp` へ引き渡される配布物の形を測る
#   - 名前を付けないときはパスが名前になる。1 本目が突き合わせの基準
#   TAKO_BRIDGE_RUNS=7                 起動の回数（initialize までの時間の中央値・最小・最大。既定 7）
#   TAKO_BRIDGE_ENV="TAKO_2021_LEGACY=1"  起こすブリッジへ足す env（同じバイナリの A/B。空白区切り）。
#                                      `名前=` の前に `env:A=1,B=2@` を付けると、その 1 本だけに足す
#                                      （例: `before=env:TAKO_2021_LEGACY=1@target/release/tako`）
#
# 流すもの（どのバイナリにも同じ列。tools/call は GUI 無しの IPC + dispatch = mcp_host が受ける）:
#   ① initialize → ② tools/list（tako の中 = カタログ全体）→ ③ tools/call 5 本
#      （list_panes・platform・set_title・未知パラメータの拒否・届かない操作 = read_pane は mcp_host に端末が無い）
#   ④ 最後の回だけ、③の 3 種（list_panes・platform・未知パラメータ）を 200 回ずつ流す（長く居座った後）
#   各段の後に `footprint <pid>` の値を採る（0.4 秒置いてから）
#
# 出力: バイナリごとに 1 行（initialize ms の中央値 / 最小 / 最大・footprint の 3 段・像の名前・リンクする dylib の数）
#   と、応答の行（②③）の sha256 が基準と同じか。`TAKO_BRIDGE_OUT=<file>` で TSV を追記する
#
# 本番には触らない: 継承した TAKO_* は全部落とし、mcp_host（使い捨ての IPC）と一時の data dir だけを使う。
# 落とすのは自分で起こした pid だけ。/bin/bash（3.2）で通る
set -uo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
RUNS="${TAKO_BRIDGE_RUNS:-7}"

if [ "$#" -lt 1 ]; then
  echo "使い方: bash scripts/measure-mcp-bridge-2021.sh [名前=]<tako の実行ファイル>..." >&2
  exit 2
fi

if [ -z "${TAKO_BRIDGE_IN_HOST:-}" ]; then
  # 外側: 継承した接続情報（本番 GUI）を落とし、mcp_host の中で自分を走らせ直す
  for v in $(env | sed -n 's/^\(TAKO_[A-Z0-9_]*\)=.*/\1/p'); do
    case "$v" in TAKO_BRIDGE_*) ;; *) unset "$v" ;; esac
  done
  # shellcheck source=lib/queue.sh
  . "$REPO_ROOT/scripts/lib/queue.sh" # run_heavy = 重い処理を順番待ちへ（#1994）
  run_heavy cargo build -p tako-control --example mcp_host --quiet --manifest-path "$REPO_ROOT/Cargo.toml" || exit 1
  TMP="$(mktemp -d /tmp/tk2021m-XXXXXX)"
  trap 'rm -rf "$TMP"' EXIT
  TAKO_BRIDGE_IN_HOST=1 TAKO_BRIDGE_TMP="$TMP" TAKO_ISOLATED=1 TAKO_DATA_DIR="$TMP/host-data" \
    "$REPO_ROOT/target/debug/examples/mcp_host" /bin/bash "$0" "$@"
  exit $?
fi

# ---- 内側（mcp_host が TAKO_SOCKET / TAKO_TOKEN / TAKO_PANE_ID を注入した状態）----
TMP="$TAKO_BRIDGE_TMP"
DRIVER="$TMP/driver.pl"
cat >"$DRIVER" <<'PERL'
# 1 本のブリッジを起こして、段ごとの footprint と応答を出す
use strict; use warnings; use IPC::Open2; use Time::HiRes qw(time sleep);
my ($bin, $resp_file, $long) = @ARGV;
sub footprint_kb { my $o = `footprint $_[0] 2>/dev/null`; my ($kb) = $o =~ /Footprint:\s+(\d+) KB/; return $kb // -1; }
my $t0 = time;
my $pid = open2(my $out, my $in, $bin, 'mcp', 'serve');
$in->autoflush(1);
sub ask { print $in "$_[0]\n"; my $line = <$out>; die "応答が無い: $_[0]\n" unless defined $line; return $line; }
my $init = ask(q({"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"m2021","version":"0"}}}));
my $init_ms = (time - $t0) * 1000;
print $in qq({"jsonrpc":"2.0","method":"notifications/initialized"}\n);
sleep 0.4; my $fp_init = footprint_kb($pid);
my @resp;
my $t_list = time;
push @resp, ask(q({"jsonrpc":"2.0","id":1,"method":"tools/list"}));
my $list_ms = (time - $t_list) * 1000;
sleep 0.4; my $fp_list = footprint_kb($pid);
push @resp, ask(q({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"tako_list_panes","arguments":{}}}));
push @resp, ask(q({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"tako_platform","arguments":{}}}));
push @resp, ask(q({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"tako_set_title","arguments":{"title":"m2021"}}}));
push @resp, ask(q({"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"tako_split_pane","arguments":{"bogus":1}}}));
push @resp, ask(q({"jsonrpc":"2.0","id":6,"method":"tools/call","params":{"name":"tako_read_pane","arguments":{}}}));
sleep 0.4; my $fp_call = footprint_kb($pid);
# ④ 長く居座った後（同じ 3 種を 200 回ずつ = TAKO_BRIDGE_LONG。応答は捨てる。最後の回だけ）
for my $i (1 .. ($long ? ($ENV{TAKO_BRIDGE_LONG} // 200) : 0)) {
  for my $call ('"tako_list_panes","arguments":{}', '"tako_platform","arguments":{}', '"tako_split_pane","arguments":{"bogus":1}') {
    ask(sprintf('{"jsonrpc":"2.0","id":%d,"method":"tools/call","params":{"name":%s}}', 1000 + $i, $call));
  }
}
my $fp_long = -1;
if ($long) { sleep 0.4; $fp_long = footprint_kb($pid); }
my $comm = `ps -o comm= -p $pid`; chomp $comm; $comm =~ s{.*/}{};
close $in; waitpid $pid, 0;
open my $fh, '>', $resp_file or die; print $fh @resp; close $fh;
printf "%.1f\t%d\t%d\t%d\t%s\t%d\t%d\t%.1f\n", $init_ms, $fp_init, $fp_list, $fp_call, $comm, length($resp[0]), $fp_long, $list_ms;
PERL


mb() { awk -v kb="$1" 'BEGIN { if (kb < 0) print "-"; else printf "%.2f", kb / 1024 }'; }

# 数の並び（空白区切り）の 中央値 最小 最大
stats() {
  local sorted n
  # shellcheck disable=SC2086
  sorted="$(printf '%s\n' $1 | sort -n)"
  n="$(printf '%s\n' "$sorted" | wc -l | tr -d ' ')"
  printf '%s %s %s' "$(printf '%s\n' "$sorted" | sed -n "$(((n + 1) / 2))p")" \
    "$(printf '%s\n' "$sorted" | head -1)" "$(printf '%s\n' "$sorted" | tail -1)"
}

ROW='%-22s %7s %6s %6s %7s  %7s %7s %7s %7s  %-9s %5s %7s  %s\n'
# shellcheck disable=SC2059
printf "$ROW" "名前" "init" "最小" "最大" "一覧ms" "fp初期" "fp一覧" "fp呼出" "fp長居" "像" "dylib" "一覧B" "応答(基準と)"
BASE_SUM=""
BASE_IDX=""
idx=0
for spec in "$@"; do
  idx=$((idx + 1))
  extra=""
  case "$spec" in
    *=env:*@*) label="${spec%%=env:*}"; rest="${spec#*=env:}"; extra="${rest%%@*}"; bin="${rest#*@}" ;;
    *=*) label="${spec%%=*}"; bin="${spec#*=}" ;;
    *) label="$spec"; bin="$spec" ;;
  esac
  if [ ! -x "$bin" ]; then
    echo "${label}: 実行ファイルが無い（${bin}）" >&2
    continue
  fi
  envs="${TAKO_BRIDGE_ENV:-} $(printf '%s' "$extra" | tr ',' ' ')"
  inits=""
  lists=""
  last=""
  run=0
  while [ "$run" -lt "$RUNS" ]; do
    run=$((run + 1))
    long=""
    [ "$run" -eq "$RUNS" ] && long=1
    # shellcheck disable=SC2086
    last="$(env $envs TAKO_ISOLATED=1 TAKO_DATA_DIR="$TMP/data-$idx" perl "$DRIVER" "$bin" "$TMP/resp-$idx" $long)" || {
      echo "${label}: 計測に失敗" >&2
      last=""
      break
    }
    inits="$inits $(printf '%s' "$last" | cut -f1)"
    lists="$lists $(printf '%s' "$last" | cut -f8)"
  done
  [ -n "$last" ] || continue
  read -r med min max <<<"$(stats "$inits")"
  list_med="$(stats "$lists" | cut -d' ' -f1)"
  fp_init="$(printf '%s' "$last" | cut -f2)"
  fp_list="$(printf '%s' "$last" | cut -f3)"
  fp_call="$(printf '%s' "$last" | cut -f4)"
  comm="$(printf '%s' "$last" | cut -f5)"
  list_bytes="$(printf '%s' "$last" | cut -f6)"
  fp_long="$(printf '%s' "$last" | cut -f7)"
  image="$(dirname "$bin")/$comm"
  [ -x "$image" ] || image="$bin"
  dylibs="$(otool -L "$image" 2>/dev/null | tail -n +2 | wc -l | tr -d ' ')"
  sum="$(shasum -a 256 "$TMP/resp-$idx" | cut -c1-12)"
  if [ -z "$BASE_SUM" ]; then
    BASE_SUM="$sum"
    BASE_IDX="$idx"
    same="基準 $sum"
  elif [ "$sum" = "$BASE_SUM" ]; then
    same="一致 $sum"
  else
    same="不一致 $sum"
    diff <(tr ',' '\n' <"$TMP/resp-$BASE_IDX") <(tr ',' '\n' <"$TMP/resp-$idx") | head -8 >&2
  fi
  # shellcheck disable=SC2059
  printf "$ROW" "$label" "$med" "$min" "$max" "$list_med" "$(mb "$fp_init")" "$(mb "$fp_list")" \
    "$(mb "$fp_call")" "$(mb "$fp_long")" "$comm" "$dylibs" "$list_bytes" "$same"
  if [ -n "${TAKO_BRIDGE_OUT:-}" ]; then
    printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' "$label" "$med" "$min" "$max" \
      "$list_med" "$fp_init" "$fp_list" "$fp_call" "$fp_long" "$comm" "$dylibs" "$list_bytes" "$sum" \
      >>"$TAKO_BRIDGE_OUT"
  fi
done
echo "init = 起動から initialize の応答まで / 一覧ms = tools/list の応答まで（ms・${RUNS} 回の中央値。init は最小・最大も）"
echo "fp = footprint MB（最後の回）: 初期 = initialize の後 / 一覧 = tools/list の後 / 呼出 = tools/call 5 本の後 /"
echo "     長居 = さらに tools/call を $((3 * ${TAKO_BRIDGE_LONG:-200})) 本流した後"
