#!/bin/bash
# tako:run bash scripts/test-cli-json-key-order-1829.sh
#
# test-cli-json-key-order-1829.sh — CLI の JSON のキーの並びがビルドの形で変わらないこと（#1829）の実経路テスト
#
# serde_json の `preserve_order` は gpui（zed 由来）が要求するので、tako-app と一緒にビルドしたときだけ
# 効いていた。手元の `cargo build -p tako-cli` では外れて `serde_json::Map` が辞書順（BTreeMap）になり、
# 同じ要求への出力が配布物とバイト単位で食い違った。ルートの Cargo.toml の `[workspace.dependencies]`
# で全メンバーへ有効にして並びを挿入順に固定した。
#
# 何を確かめるか:
#   ① 同じソースを 2 つの形でビルドする: `cargo build -p tako-cli`（単独 = AGENTS.md の「tako CLI ビルド」）と
#      `cargo build -p tako-app -p tako-cli`（配布物と同じ組み合わせ = scripts/build-app.sh /
#      installer/windows/build-installer.ps1）
#   ② 同じ要求への出力（標準出力・標準エラー・終了コード）が 2 本の間でバイト一致する。要求は
#      GUI の要らない 3 本（agent-support / platform）・誤りの経路 2 本（知らない系統 = clap・
#      GUI に届かない）・GUI の応答をそのまま出す 2 本（`tako theme`。偽の IPC サーバー = perl が
#      応答を 1 行返す。PR #1828 で食い違った形と、入れ子・日本語のキーを持つ応答）
#   ③ 並びが定義の順（挿入順）であって辞書順ではない
#
# 修正前のソースで回すと ② と ③ の単独ビルドの側が NG になる（検出力。実測 2026-10-17:
# 修正前 27 PASS / 8 FAIL → 修正後 35 PASS / 0 FAIL）。
#
# 使い方: bash scripts/test-cli-json-key-order-1829.sh（ビルドは scripts/lib/queue.sh の順番待ちへ通す）
#   ビルド済みの 2 本を比べるだけなら TAKO_1829_SOLO_BIN=<単独の tako> TAKO_1829_COMBINED_BIN=<配布形の tako>
set -uo pipefail

unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_ORCHESTRATOR_ROLE

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
. "$REPO_ROOT/scripts/lib/queue.sh"

PASS=0
FAIL=0
pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
fail() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }
check_eq() {
  if [ "$2" = "$3" ]; then pass "$1"; else fail "$1（期待 '${2}' / 実際 '${3}'）"; fi
}

TMP="$(mktemp -d /tmp/tako-1829-XXXXXX)"
SERVER_PID=""
cleanup() {
  if [ -n "$SERVER_PID" ]; then kill "$SERVER_PID" 2>/dev/null; fi
  rm -rf "$TMP"
}
trap cleanup EXIT

SOLO="${TAKO_1829_SOLO_BIN:-}"
COMBINED="${TAKO_1829_COMBINED_BIN:-}"
if [ -z "$SOLO" ] || [ -z "$COMBINED" ]; then
  TARGET_DIR="${CARGO_TARGET_DIR:-$REPO_ROOT/target}"
  echo "==> cargo build -p tako-cli（単独）"
  (cd "$REPO_ROOT" && run_heavy cargo build -q -p tako-cli) || exit 1
  command cp -f "$TARGET_DIR/debug/tako" "$TMP/tako-solo" || exit 1
  echo "==> cargo build -p tako-app -p tako-cli（配布物と同じ組み合わせ）"
  (cd "$REPO_ROOT" && run_heavy cargo build -q -p tako-app -p tako-cli) || exit 1
  command cp -f "$TARGET_DIR/debug/tako" "$TMP/tako-combined" || exit 1
  SOLO="$TMP/tako-solo"
  COMBINED="$TMP/tako-combined"
fi
for bin in "$SOLO" "$COMBINED"; do
  [ -x "$bin" ] || { echo "tako が無い: $bin"; exit 1; }
done

mkdir -p "$TMP/data"
# 偽の GUI へ向けるときだけ入れる env（TAKO_SOCKET / TAKO_TOKEN）
GUI_ENV=()
# 隔離した tako（本番の GUI にも data dir にも触らない）。$1 = 実行ファイル
run_tako() {
  local bin="$1"
  shift
  env -u TAKO_SOCKET -u TAKO_PANE_ID -u TAKO_TOKEN TAKO_ISOLATED=1 TAKO_DATA_DIR="$TMP/data" \
    ${GUI_ENV[@]+"${GUI_ENV[@]}"} "$bin" "$@"
}

# GUI の代わりに応答を 1 行だけ返す偽の IPC サーバーを立て、受け口ができるまで待つ。
# $1 = ソケット、$2 = 応答の result（JSON の文字列そのまま）。届いた要求の 1 行は $1.req へ
start_fake_gui() {
  rm -f "$1" "$1.req"
  perl -MIO::Socket::UNIX -MSocket -e '
    my ($path, $result) = @ARGV;
    my $srv = IO::Socket::UNIX->new(Type => SOCK_STREAM, Local => $path, Listen => 1)
      or die "偽の IPC サーバーを立てられない: $!\n";
    my $conn = $srv->accept or die "接続を受けられない: $!\n";
    my $req = <$conn>;
    open(my $fh, ">", "$path.req") or die; print $fh $req; close $fh;
    print $conn "{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":$result}\n";
    close $conn;
  ' "$1" "$2" &
  SERVER_PID=$!
  local i=0
  while [ ! -S "$1" ] && [ "$i" -lt 100 ]; do
    sleep 0.05
    i=$((i + 1))
  done
  [ -S "$1" ] || { echo "偽の IPC サーバーの受け口ができない: $1"; exit 1; }
}

# 1 本の実行ファイルで要求を 1 回送り、標準出力・標準エラー・終了コードを $TMP/<名前>.<形>.* へ残す。
# $1 = 名前、$2 = 形（solo / combined）、$3 = 実行ファイル、$4 = 偽の GUI の応答（空なら GUI 無し）、残り = 引数
run_one() {
  local name="$1" form="$2" bin="$3" reply="$4"
  shift 4
  local out="$TMP/$name.$form"
  if [ -n "$reply" ]; then
    start_fake_gui "$TMP/s" "$reply"
    GUI_ENV=(TAKO_SOCKET="$TMP/s" TAKO_TOKEN=issue1829)
    run_tako "$bin" "$@" >"$out.out" 2>"$out.err" </dev/null
    echo "$?" >"$out.code"
    GUI_ENV=()
    wait "$SERVER_PID" 2>/dev/null
    SERVER_PID=""
  else
    run_tako "$bin" "$@" >"$out.out" 2>"$out.err" </dev/null
    echo "$?" >"$out.code"
  fi
}

# ②: 2 本の出力を比べる。$1 = 名前、$2 = 期待する終了コード、$3 = 偽の GUI の応答、残り = 引数
compare() {
  local name="$1" want_code="$2" reply="$3"
  shift 3
  run_one "$name" solo "$SOLO" "$reply" "$@"
  run_one "$name" combined "$COMBINED" "$reply" "$@"
  check_eq "$name: 終了コード（単独）" "$want_code" "$(cat "$TMP/$name.solo.code")"
  check_eq "$name: 終了コード（配布形）" "$want_code" "$(cat "$TMP/$name.combined.code")"
  local part
  for part in out err; do
    local a="$TMP/$name.solo.$part" b="$TMP/$name.combined.$part"
    if cmp -s "$a" "$b"; then
      pass "$name: std$part が単独と配布形でバイト一致（$(wc -c <"$a" | tr -d ' ') バイト）"
    else
      fail "$name: std$part が単独と配布形で食い違う（$(cmp "$a" "$b" 2>&1 | head -1)）"
      diff "$a" "$b" | head -6 | sed 's/^/      /'
    fi
  done
}

# 整形済み JSON の "agents" の最初の 1 件の直下のキーを並びのまま（空白区切り）
first_agent_keys() {
  awk '
    /"agents": \[/ { f = 1; next }
    f && !g && /^ *\{/ { g = 1; next }
    g && /^ *\}/ { exit }
    g && match($0, /^ *"[^"]+":/) {
      k = substr($0, RSTART, RLENGTH); gsub(/^ *"|":$/, "", k); printf "%s ", k
    }
  ' "$1"
}

echo "==> ② 同じ要求への出力のバイト一致"
compare agent-support 0 "" agent-support --json
compare agent-support-codex 0 "" agent-support --agent codex --json
compare platform 0 "" platform --json
compare agent-support-unknown 2 "" agent-support --agent no-such-agent --json
compare theme-no-gui 1 "" theme
THEME_REPLY='{"theme":"dark","mode":"dark","available":["dark","light"],"presets":[]}'
compare theme-reply 0 "$THEME_REPLY" theme
NESTED_REPLY='{"z":{"b":1,"a":[{"y":true,"x":null}]},"日本語":"値","a":{}}'
compare nested-reply 0 "$NESTED_REPLY" theme

echo "==> ③ 並びは定義の順（挿入順）"
check_eq "偽の GUI へ届いた要求は theme" "1" "$(grep -c '"method":"theme"' "$TMP/s.req" 2>/dev/null)"
for form in solo combined; do
  check_eq "agent-support の系統の 1 件（${form}）" "key label baseline " \
    "$(first_agent_keys "$TMP/agent-support.$form.out")"
  check_eq "GUI の応答を並びのまま出す（${form}）" "$THEME_REPLY" "$(cat "$TMP/theme-reply.$form.out")"
  check_eq "入れ子・日本語のキーも並びのまま（${form}）" "$NESTED_REPLY" "$(cat "$TMP/nested-reply.$form.out")"
done

echo
echo "結果: ${PASS} PASS / ${FAIL} FAIL"
[ "$FAIL" -eq 0 ]
