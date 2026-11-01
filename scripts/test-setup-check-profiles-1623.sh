#!/usr/bin/env bash
# test-setup-check-profiles-1623.sh — #1623: setup --check / check-health のプロファイルの行の実経路テスト
#
# 件数を変えたプロファイルを置いた砂場で `tako setup --check` と `tako check-health --json` を回し、
# 次を確かめる（番号は Issue #1623 の受け入れ条件）:
#   2. 0 / 1 / 先頭ちょうど（3）/ 1 個はみ出す（4・default 無し）/ 84 個で、日英とも
#      「件数・先頭 3 個（default が先頭・残りは名前順）・他 N 個」の 1 行。`--json` の `names` に全件（名前順）
#   3. `--check` の行と `check-health --json` の `lines` が字面まで同じ（sha256）。
#      プロファイル以外の行は A/B の旧形（`TAKO_1623_LEGACY=1`）とバイト一致
#   4. `TAKO_1623_LEGACY=1` で旧表示（1 行に全部並べる・`names` を載せない）
#   エッジ: 一覧を読めない（profiles/ が読めない）ときは確認失敗の行で `names` を載せない
#   任意: `TAKO_BIN_BEFORE=<#1623 前の tako>` を渡すと、旧形の出力が #1623 前とバイト一致することも見る
#
# **本物の claude / codex / agy は起こさない**（PATH はシステムの置き場だけ・ログインシェルにしない SHELL・一時 HOME）。
# **本番の tako には触らない**: data は mktemp 配下、GUI は使わない（CLI が GUI に届かないときに自分で診断を回す経路）。
#
# 使い方: bash scripts/test-setup-check-profiles-1623.sh
#   TAKO_BIN=<tako CLI>（既定はこの木の target/debug/tako を順番待ちで建てる）
# CI には登録しない（1 回 5 秒前後の診断を 30 回ほど回す）。CI 側の担保は単体テストの番犬
# （`crates/tako-control/src/diagnostics.rs` の `プロファイルの項目は先頭3個と他n個の1行で全件はnamesに載る`）。
set -uo pipefail

# **本番 GUI を指す env を最初に落とす**（#1449 / #1450 で本番にペインが漏れた実例が 2 件）
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_ORCHESTRATOR_ROLE TAKO_MCP_URL \
  CLAUDE_CODE_PLUGIN_DIRS TAKO_CLI TAKO_1623_LEGACY TAKO_LANG TAKO_ORCHESTRATOR_DIR
unset CLAUDE_CODE_CHILD_SESSION CLAUDE_CODE_SESSION_ID CLAUDECODE

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PASS=0
FAIL=0

pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
fail() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }
check_eq() {
  if [ "$2" = "$3" ]; then pass "$1"; else fail "$1（期待 '${2}' / 実際 '${3}'）"; fi
}

TMP="$(mktemp -d /tmp/tako-1623-XXXXXX)"
cleanup() {
  # 読めなくした profiles/ を戻してから消す
  chmod -R u+rwx "${TMP:?}" >/dev/null 2>&1 || true
  if [ -n "${KEEP:-}" ]; then echo "（KEEP: $TMP を残した）"; else rm -rf "${TMP:?}"; fi
}
# shellcheck source=lib/exit-guard.sh
. "$REPO_ROOT/scripts/lib/exit-guard.sh"
tako_exit_trap cleanup "setup --check のプロファイルの行の実経路テスト"
# shellcheck source=lib/queue.sh
. "$REPO_ROOT/scripts/lib/queue.sh"
# shellcheck source=lib/fake-exec.sh
. "$REPO_ROOT/scripts/lib/fake-exec.sh"

if [ -z "${TAKO_BIN:-}" ]; then
  echo "tako CLI を今の木から建てます（最新なら cargo は何もしない）…"
  (cd "$REPO_ROOT" && run_heavy cargo build -p tako-cli --quiet) || exit 1
  TAKO_BIN="$REPO_ROOT/target/debug/tako"
fi

HOME_DIR="$TMP/home"
mkdir -p "$HOME_DIR" "$TMP/stubs" "$TMP/out"
# ログインシェルにしない SHELL（利用者の rc が本物の CLI を引かない）
printf '#!/bin/sh\nif [ "$1" = "-l" ]; then shift; fi\nexec /bin/sh "$@"\n' > "$TMP/stubs/isosh"
chmod +x "$TMP/stubs/isosh"
fake_exec_warm "$TMP/stubs/isosh" || exit 1

# 砂場の data dir を件数ごとに作る: make_data <名前> <プロファイル名…>
make_data() {
  local name="$1"
  shift
  local dir="$TMP/data-$name/orchestrator/profiles"
  mkdir -p "$dir"
  local profile
  for profile in "$@"; do printf 'name: %s\n' "$profile" > "$dir/$profile.yaml"; done
}

# 砂場で 1 回走らせる: run <出力名> <data の名前> <言語> [VAR=VAL …] -- <tako の引数…>
# `--check` の行は stderr、`check-health --json` の JSON は stdout に出る（`<出力名>.stderr` / `.stdout`）
run() {
  local out="$1" data="$2" lang="$3"
  shift 3
  local extra=()
  while [ $# -gt 0 ] && [ "$1" != "--" ]; do extra+=("$1"); shift; done
  [ "${1:-}" = "--" ] && shift
  env HOME="$HOME_DIR" SHELL="$TMP/stubs/isosh" CODEX_HOME="$HOME_DIR/.codex" TAKO_ISOLATED=1 \
    TAKO_DATA_DIR="$TMP/data-$data" TAKO_LANG="$lang" PATH="/usr/bin:/bin:/usr/sbin:/sbin" \
    ${extra[@]+"${extra[@]}"} "${BIN:-$TAKO_BIN}" "$@" < /dev/null \
    > "$TMP/out/$out.stdout" 2> "$TMP/out/$out.stderr"
}

# `--check` の出力からプロファイルの行を抜く（日英どちらのラベルでも）
check_line() { grep -E '^  \[[^]]+\] (プロファイル|profiles):' "$1" || true; }

# check-health --json の profiles の項目から 1 欄を抜く（lines は 1 行目・names は JSON の配列）
json_field() {
  python3 -I -c '
import json, sys
text = open(sys.argv[1], encoding="utf-8").read()
v = json.loads(text)
item = next((i for i in v["diagnostics"]["items"] if i["key"] == "profiles"), None)
f = sys.argv[2]
if item is None:
    print("no-item")
elif f == "line":
    print(item["lines"][0] if len(item["lines"]) == 1 else "lines=%d" % len(item["lines"]))
elif f == "names":
    print(json.dumps(item["names"]) if "names" in item else "absent")
elif f == "all-lines":
    for i in v["diagnostics"]["items"]:
        for l in i["lines"]:
            print(l)
' "$1" "$2"
}

sha() { printf '%s' "$1" | shasum -a 256 | cut -c1-16; }

# `--check` の全文からプロファイルの行だけを落とす
without_profiles() { grep -v -E '^  \[[^]]+\] (プロファイル|profiles):' "$1" || true; }

# JSON の全項目の行が `--check` の出力の中にひと続きで在るか（#1505 の字面一致）
lines_embedded() {
  python3 -I -c '
import sys
check = open(sys.argv[1], encoding="utf-8").read()
lines = open(sys.argv[2], encoding="utf-8").read()
sys.exit(0 if lines and lines in check else 1)
' "$1" "$2"
}

# --- 砂場の件数 ----------------------------------------------------------------
MANY=(codex default takodev)
for i in $(seq 1 81); do MANY+=("$(printf 'p%02d' "$i")"); done
make_data 0
make_data 1 work
make_data 3 work codex default
make_data 4 gamma alpha delta beta
make_data 84 ${MANY[@]+"${MANY[@]}"}
check_eq "84 個の砂場" "84" "$(command ls "$TMP/data-84/orchestrator/profiles" | grep -c '\.yaml$')"

# 期待（件数 / 言語 → --check の行・names）
expect_line() {
  case "$1/$2" in
    0/ja) echo "  [情報] プロファイル: 未作成（tako master で自動生成されます）" ;;
    0/en) echo "  [info] profiles: none yet (tako master creates one automatically)" ;;
    1/ja) echo "  [OK] プロファイル: 1 個（work）" ;;
    1/en) echo "  [OK] profiles: 1 (work)" ;;
    3/ja) echo "  [OK] プロファイル: 3 個（default / codex / work）" ;;
    3/en) echo "  [OK] profiles: 3 (default / codex / work)" ;;
    4/ja) echo "  [OK] プロファイル: 4 個（alpha / beta / delta …他 1 個）" ;;
    4/en) echo "  [OK] profiles: 4 (alpha / beta / delta … 1 more)" ;;
    84/ja) echo "  [OK] プロファイル: 84 個（default / codex / p01 …他 81 個）" ;;
    84/en) echo "  [OK] profiles: 84 (default / codex / p01 … 81 more)" ;;
  esac
}
sorted_json() {
  python3 -I -c 'import json, sys; print(json.dumps(sorted(sys.argv[1:])))' "$@"
}
expect_names() {
  case "$1" in
    0) echo "[]" ;;
    1) sorted_json work ;;
    3) sorted_json work codex default ;;
    4) sorted_json gamma alpha delta beta ;;
    84) sorted_json ${MANY[@]+"${MANY[@]}"} ;;
  esac
}

# --- 2 / 3: 件数 × 言語 ----------------------------------------------------------
for n in 0 1 3 4 84; do
  for lang in ja en; do
    echo "== 2 / 3: $n 個・$lang"
    run "check-$n-$lang" "$n" "$lang" -- setup --check
    run "json-$n-$lang" "$n" "$lang" -- check-health --json
    line="$(check_line "$TMP/out/check-$n-$lang.stderr")"
    jline="$(json_field "$TMP/out/json-$n-$lang.stdout" line)"
    check_eq "--check の行" "$(expect_line "$n" "$lang")" "$line"
    check_eq "check-health --json の lines と字面一致（sha256）" "$(sha "$line")" "$(sha "$jline")"
    check_eq "--json の names が全件（名前順）" "$(expect_names "$n")" "$(json_field "$TMP/out/json-$n-$lang.stdout" names)"
    json_field "$TMP/out/json-$n-$lang.stdout" all-lines > "$TMP/out/json-$n-$lang.lines"
    if lines_embedded "$TMP/out/check-$n-$lang.stderr" "$TMP/out/json-$n-$lang.lines"; then
      pass "--json の全項目の行が --check の出力にひと続きで在る"
    else
      fail "--json の全項目の行が --check の出力と食い違う"
    fi
  done
done
echo "  （84 個・日本語の行は $(check_line "$TMP/out/check-84-ja.stderr" | python3 -I -c 'import sys; print(len(sys.stdin.read().rstrip("\n")))') 字）"

# --- 3 / 4: A/B の旧形 -----------------------------------------------------------
for n in 0 3 84; do
  for lang in ja en; do
    echo "== 3 / 4: A/B の旧形（TAKO_1623_LEGACY=1）$n 個・$lang"
    run "old-check-$n-$lang" "$n" "$lang" TAKO_1623_LEGACY=1 -- setup --check
    run "old-json-$n-$lang" "$n" "$lang" TAKO_1623_LEGACY=1 -- check-health --json
    without_profiles "$TMP/out/check-$n-$lang.stderr" > "$TMP/out/check-$n-$lang.rest"
    without_profiles "$TMP/out/old-check-$n-$lang.stderr" > "$TMP/out/old-check-$n-$lang.rest"
    if cmp -s "$TMP/out/old-check-$n-$lang.rest" "$TMP/out/check-$n-$lang.rest"; then
      pass "プロファイル以外の行は旧形とバイト一致"
    else
      fail "プロファイル以外の行が旧形と違う"
      diff "$TMP/out/old-check-$n-$lang.rest" "$TMP/out/check-$n-$lang.rest" | head -6 | sed 's/^/      /'
    fi
    check_eq "旧形は names を載せない" "absent" "$(json_field "$TMP/out/old-json-$n-$lang.stdout" names)"
  done
done
OLD84="$(check_line "$TMP/out/old-check-84-ja.stderr")"
JOINED="$(printf '%s\n' ${MANY[@]+"${MANY[@]}"} | LC_ALL=C sort | paste -sd ',' - | sed 's/,/, /g')"
check_eq "旧形は 1 行に全部並べる（84 個）" "  [OK] プロファイル: 84 個（${JOINED}）" "$OLD84"
check_eq "旧形の 3 個は , 区切り" "  [OK] profiles: 3 (codex, default, work)" "$(check_line "$TMP/out/old-check-3-en.stderr")"

# --- エッジ: 一覧を読めない ---------------------------------------------------------
echo "== エッジ: profiles/ が読めない"
make_data unreadable a b
chmod 000 "$TMP/data-unreadable/orchestrator/profiles"
run check-unreadable unreadable en -- setup --check
run json-unreadable unreadable en -- check-health --json
chmod 755 "$TMP/data-unreadable/orchestrator/profiles"
UNREADABLE="$(check_line "$TMP/out/check-unreadable.stderr")"
case "$UNREADABLE" in
  "  [info] profiles: check failed ("*) pass "確認失敗の行（${UNREADABLE}）" ;;
  *) fail "確認失敗の行でない: '$UNREADABLE'" ;;
esac
check_eq "読めないときは names を載せない" "absent" "$(json_field "$TMP/out/json-unreadable.stdout" names)"

# --- 任意: #1623 前の tako と旧形の一致 ---------------------------------------------
if [ -n "${TAKO_BIN_BEFORE:-}" ]; then
  for n in 0 3 84; do
    echo "== 任意: #1623 前の tako と A/B の旧形（$n 個・ja）"
    BIN="$TAKO_BIN_BEFORE" run "before-check-$n" "$n" ja -- setup --check
    BIN="$TAKO_BIN_BEFORE" run "before-json-$n" "$n" ja -- check-health --json
    if cmp -s "$TMP/out/before-check-$n.stderr" "$TMP/out/old-check-$n-ja.stderr"; then
      pass "--check の全文が #1623 前とバイト一致"
    else
      fail "--check の全文が #1623 前と違う"
      diff "$TMP/out/before-check-$n.stderr" "$TMP/out/old-check-$n-ja.stderr" | head -6 | sed 's/^/      /'
    fi
    json_field "$TMP/out/before-json-$n.stdout" all-lines > "$TMP/out/before-json-$n.lines"
    json_field "$TMP/out/old-json-$n-ja.stdout" all-lines > "$TMP/out/old-json-$n-ja.lines"
    if cmp -s "$TMP/out/before-json-$n.lines" "$TMP/out/old-json-$n-ja.lines"; then
      pass "--json の全項目の行が #1623 前とバイト一致"
    else
      fail "--json の全項目の行が #1623 前と違う"
    fi
  done
fi

echo
echo "結果: OK $PASS / NG $FAIL"
[ "$FAIL" -eq 0 ] || exit 1
tako_exit 0
