#!/usr/bin/env bash
# test-check-health-parallel-1622.sh — #1622: 環境診断の外部の問い合わせを重ねても答えが変わらないことの実経路テスト
#
# 偽の claude / codex / agy を置いた砂場で `tako check-health --json` と `tako setup --check` を回し、
# 修正後（重ねる）と A/B の旧経路（`TAKO_1622_LEGACY=1` = 重ねない）を比べる（番号は Issue #1622 の受け入れ条件）:
#   1. 出力（--json の全文・--check の全文）が 1 バイトも変わらない
#   2. 重なっていること（**時間では判定しない**: 偽の `agy models` = 導入状況の認証確認が、偽の
#      `claude mcp list` = 背景の問い合わせの**始まりを印のファイルで待つ**。重なっていれば待ち合いが
#      成立して印 `overlap` が立ち、逐次なら相手が始まらないまま偽物の側の待ちの上限を過ぎる）
#   3. `claude mcp list` の打ち切りの知らせが重ねた後も出る（--json の `probe_timeouts`・項目・stderr の 1 行）。
#      認証と `claude mcp list` の両方が打ち切られたときの stderr の知らせの並びも旧経路と同じ
#   エッジ: エージェント CLI が 1 つも無い砂場でも出力は旧経路と一致する
#
# **本物の claude / codex / agy は起こさない**（偽物はシェルの小さな実行ファイル。止まるときは `exec sleep` =
# 打ち切りの kill が届く直接の子）。**本番の tako にも利用者の CLI にも触らない**: data は mktemp 配下、
# 一時 HOME・偽物だけが居る PATH・ログインシェルにしない SHELL（#2255 / #1505 のスクリプトと同じ作り）。
# GUI は使わない（CLI が GUI に届かないときに自分で診断を回す経路）。落とすのは自分が起こした pid だけ。
#
# 使い方: bash scripts/test-check-health-parallel-1622.sh
#   TAKO_BIN=<tako CLI>（既定はこの木の target/debug/tako を順番待ちで建てる）
# CI には登録しない（数十秒掛かる）。CI 側の担保は単体テストの番犬
# （`crates/tako-control/src/diagnostics.rs` の `外部の問い合わせは導入状況と重ねて走り答えは逐次と同じ`）。
set -uo pipefail
# check-health / `tako setup --check` は表示言語に従う（#2365）。判定は日本語の字面なので、OS の言語に
# 依らず日本語表示へ固定する（settings.json の language が system のときだけ効く）
export TAKO_LANG=ja

# **本番 GUI を指す env を最初に落とす**（#1449 / #1450 で本番にペインが漏れた実例が 2 件）
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_ORCHESTRATOR_ROLE TAKO_MCP_URL \
  CLAUDE_CODE_PLUGIN_DIRS TAKO_CLI TAKO_1622_LEGACY TAKO_2255_LEGACY TAKO_AUTH_PROBE_TIMEOUT_SECS \
  TAKO_SETUP_PROBE_TIMEOUT_SECS TAKO_IPC_TIMEOUT_SECS
unset CLAUDE_CODE_CHILD_SESSION CLAUDE_CODE_SESSION_ID CLAUDECODE

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PASS=0
FAIL=0

pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
fail() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }
check_eq() {
  if [ "$2" = "$3" ]; then pass "$1"; else fail "$1（期待 '${2}' / 実際 '${3}'）"; fi
}
contains() { case "$(cat "$1")" in *"$2"*) return 0 ;; esac; return 1; }

TMP="$(mktemp -d /tmp/tako-1622-XXXXXX)"
FAKE_DIR="$TMP/fake"

# 偽物を落とす（止まる前に自分で書いた pid だけ）
kill_fakes() {
  local f
  for f in "$FAKE_DIR/pids"/*; do
    [ -f "$f" ] || continue
    kill -KILL "${f##*/}" >/dev/null 2>&1 || true
    rm -f "$f"
  done
}
cleanup() {
  kill_fakes
  if [ -n "${KEEP:-}" ]; then echo "（KEEP: $TMP を残した）"; else rm -rf "${TMP:?}"; fi
}
# shellcheck source=lib/exit-guard.sh
. "$REPO_ROOT/scripts/lib/exit-guard.sh"
tako_exit_trap cleanup "環境診断の問い合わせを重ねる実経路テスト"
# shellcheck source=lib/queue.sh
. "$REPO_ROOT/scripts/lib/queue.sh"
# shellcheck source=lib/fake-exec.sh
. "$REPO_ROOT/scripts/lib/fake-exec.sh"

if [ -z "${TAKO_BIN:-}" ]; then
  echo "tako CLI を今の木から建てます（最新なら cargo は何もしない）…"
  (cd "$REPO_ROOT" && run_heavy cargo build -p tako-cli --quiet) || exit 1
  TAKO_BIN="$REPO_ROOT/target/debug/tako"
fi

# --- 偽の claude / codex / agy ---------------------------------------------------
# 印（$FAKE_DIR の下）:
#   hang-claude-auth / hang-claude-mcp … その問い合わせで止まる（打ち切りの知らせを見る）
#   wait-overlap … `agy models` が `claude mcp list` の始まりを待つ（1 / 2 の段だけ）
#   started-claude-mcp … `claude mcp list` が始まった
#   overlap … `agy models`（導入状況）が走っている間に `claude mcp list` が始まった = 重なった
HOME_DIR="$TMP/home"
CLAUDE_DIR="$HOME_DIR/.local/bin"
BIN_DIR="$TMP/bin"
mkdir -p "$CLAUDE_DIR" "$BIN_DIR" "$TMP/stubs" "$FAKE_DIR/pids" "$TMP/out"
cat > "$CLAUDE_DIR/claude" <<'SH'
#!/bin/sh
case "$*" in
  "auth status --json")
    if [ -e "$FAKE_DIR/hang-claude-auth" ]; then : > "$FAKE_DIR/pids/$$"; exec /bin/sleep 600; fi
    echo '{"loggedIn":true,"authMethod":"claude.ai","subscriptionType":"max"}' ;;
  "mcp list")
    : > "$FAKE_DIR/started-claude-mcp"
    if [ -e "$FAKE_DIR/hang-claude-mcp" ]; then : > "$FAKE_DIR/pids/$$"; exec /bin/sleep 600; fi
    echo "tako: $FAKE_TAKO mcp serve - Connected" ;;
esac
exit 0
SH
cat > "$BIN_DIR/codex" <<'SH'
#!/bin/sh
case "$*" in
  "login status") echo "Logged in using ChatGPT" ;;
  "mcp list"*) echo '[]' ;;
esac
exit 0
SH
# 導入状況の認証確認（`agy models`）が、背景の `claude mcp list` の始まりを待つ。上限 4 秒は
# 認証確認の上限（既定 10 秒）より短く取る = 逐次でも打ち切られずに答える（出力を揃えるため）
cat > "$BIN_DIR/agy" <<'SH'
#!/bin/sh
case "$*" in
  models)
    i=0
    [ -e "$FAKE_DIR/wait-overlap" ] || i=80
    while [ "$i" -lt 80 ]; do
      if [ -e "$FAKE_DIR/started-claude-mcp" ]; then : > "$FAKE_DIR/overlap"; break; fi
      /bin/sleep 0.05
      i=$((i + 1))
    done
    printf 'fake-model\tFake Model\n' ;;
esac
exit 0
SH
chmod +x "$CLAUDE_DIR/claude" "$BIN_DIR/codex" "$BIN_DIR/agy"
printf '#!/bin/sh\nif [ "$1" = "-l" ]; then shift; fi\nexec /bin/sh "$@"\n' > "$TMP/stubs/isosh"
chmod +x "$TMP/stubs/isosh"
# 作りたての実行ファイルの初回 exec の検査を上限の外で済ませる（#2222）
fake_exec_warm "$CLAUDE_DIR/claude" "$BIN_DIR/codex" "$BIN_DIR/agy" "$TMP/stubs/isosh" || exit 1
# claude の登録パス（`~/.claude.json`）は実在するファイルを指す = 「登録済み」の健全な形
FAKE_TAKO="$TMP/stubs/isosh"
printf '{"mcpServers":{"tako":{"command":"%s","args":["mcp","serve"]}}}\n' "$FAKE_TAKO" > "$HOME_DIR/.claude.json"

FAKES="$CLAUDE_DIR:$BIN_DIR"

# 砂場で 1 回走らせる: run <名前> <PATH の先頭（空 = 偽物なし）> [VAR=VAL …] -- <tako の引数…>
run() {
  local name="$1" fakes="$2"
  shift 2
  local extra=()
  while [ $# -gt 0 ] && [ "$1" != "--" ]; do extra+=("$1"); shift; done
  [ "${1:-}" = "--" ] && shift
  rm -f "${FAKE_DIR:?}/started-claude-mcp" "${FAKE_DIR:?}/overlap"
  env HOME="$HOME_DIR" SHELL="$TMP/stubs/isosh" CODEX_HOME="$HOME_DIR/.codex" TAKO_ISOLATED=1 \
    TAKO_DATA_DIR="$TMP/data" PATH="${fakes:+$fakes:}/usr/bin:/bin:/usr/sbin:/sbin" \
    FAKE_DIR="$FAKE_DIR" FAKE_TAKO="$FAKE_TAKO" ${extra[@]+"${extra[@]}"} \
    "$TAKO_BIN" "$@" < /dev/null > "$TMP/out/$name.out" 2> "$TMP/out/$name.err"
  OVERLAP=no
  [ -e "$FAKE_DIR/overlap" ] && OVERLAP=yes
  kill_fakes
}
# 偽物の振る舞いの印を入れ替える（hang-* / wait-overlap）
set_hang() {
  rm -f "${FAKE_DIR:?}"/hang-* "${FAKE_DIR:?}/wait-overlap"
  local name
  for name in "$@"; do : > "$FAKE_DIR/$name"; done
}
jf() {
  python3 -I -c '
import json, sys
v = json.load(sys.stdin)["diagnostics"]
for k in [k for k in sys.argv[1].split("/") if k]:
    if isinstance(v, list) and k.startswith("key="):
        v = next((r for r in v if r.get("key") == k[4:]), None)
    elif isinstance(v, list) and k.isdigit():
        v = v[int(k)] if int(k) < len(v) else None
    elif isinstance(v, dict):
        v = v.get(k)
    if v is None:
        break
print("null" if v is None else (json.dumps(v, ensure_ascii=False) if isinstance(v, (dict, list)) else v))' "$1"
}
# 2 つの出力がバイト単位で同じか（違えば先頭の差分を見せる）
same_bytes() {
  if cmp -s "$2" "$3"; then
    pass "$1"
  else
    fail "$1"
    diff "$2" "$3" | head -8 | sed 's/^/      /'
  fi
}
# 打ち切りの知らせ（agent_probe が stderr へ出す `[確認できません] …` の行）だけを抜く
notices() { grep '\[確認できません\]' "$1" || true; }

# --- 1 / 2: 重ねても出力は同じ・実際に重なっている ------------------------------
echo "== 1 / 2: 修正後（重ねる）と A/B の旧経路（重ねない）"
set_hang wait-overlap
run new-json "$FAKES" -- check-health --json
NEW_OVERLAP="$OVERLAP"
run old-json "$FAKES" TAKO_1622_LEGACY=1 -- check-health --json
OLD_OVERLAP="$OVERLAP"
check_eq "偽の claude は導入済み・認証済み" "ok" "$(jf items/key=agent_cli.claude/status < "$TMP/out/new-json.out")"
check_eq "Claude MCP は登録済み（偽の claude mcp list の答え）" "ok" "$(jf items/key=mcp.claude/status < "$TMP/out/new-json.out")"
same_bytes "--json の全文が旧経路と 1 バイトも変わらない" "$TMP/out/old-json.out" "$TMP/out/new-json.out"
check_eq "修正後は導入状況（agy models）の間に claude mcp list が始まる（重なる）" "yes" "$NEW_OVERLAP"
check_eq "旧経路は重ならない（導入状況が終わってから claude mcp list）" "no" "$OLD_OVERLAP"
run new-check "$FAKES" -- setup --check
run old-check "$FAKES" TAKO_1622_LEGACY=1 -- setup --check
same_bytes "--check の全文（stderr）が旧経路と 1 バイトも変わらない" "$TMP/out/old-check.err" "$TMP/out/new-check.err"
same_bytes "--check の stdout も同じ" "$TMP/out/old-check.out" "$TMP/out/new-check.out"

# --- 3: 打ち切りの知らせ ----------------------------------------------------------
echo "== 3: claude mcp list が返らない（上限は注入で 3 秒）"
set_hang hang-claude-mcp
run new-hang "$FAKES" TAKO_SETUP_PROBE_TIMEOUT_SECS=3 -- check-health --json
run old-hang "$FAKES" TAKO_1622_LEGACY=1 TAKO_SETUP_PROBE_TIMEOUT_SECS=3 -- check-health --json
check_eq "mcp.claude は警告" "warn" "$(jf items/key=mcp.claude/status < "$TMP/out/new-hang.out")"
check_eq "mcp.claude の要約に打ち切り" "Claude MCP: 登録状態を確認できません（claude mcp list が 3 秒で応答なし）" \
  "$(jf items/key=mcp.claude/summary < "$TMP/out/new-hang.out")"
check_eq "probe_timeouts に claude mcp list" '[{"label": "claude mcp list", "waited_secs": 3}]' \
  "$(jf probe_timeouts < "$TMP/out/new-hang.out")"
if contains "$TMP/out/new-hang.err" "[確認できません] claude mcp list（3 秒応答なし）"; then
  pass "stderr に打ち切りの知らせ（#1503 の文面）"
else
  fail "stderr に打ち切りの知らせが無い"
fi
same_bytes "打ち切り: --json の全文が旧経路と同じ" "$TMP/out/old-hang.out" "$TMP/out/new-hang.out"

echo "== 3: 認証と claude mcp list の両方が返らない（認証 2 秒 / mcp list 3 秒）"
set_hang hang-claude-auth hang-claude-mcp
run new-both "$FAKES" TAKO_AUTH_PROBE_TIMEOUT_SECS=2 TAKO_SETUP_PROBE_TIMEOUT_SECS=3 -- setup --check
run old-both "$FAKES" TAKO_1622_LEGACY=1 TAKO_AUTH_PROBE_TIMEOUT_SECS=2 TAKO_SETUP_PROBE_TIMEOUT_SECS=3 -- setup --check
notices "$TMP/out/old-both.err" > "$TMP/out/old-both.notices"
notices "$TMP/out/new-both.err" > "$TMP/out/new-both.notices"
same_bytes "stderr の知らせの並びが旧経路と同じ（認証 → claude mcp list）" \
  "$TMP/out/old-both.notices" "$TMP/out/new-both.notices"
check_eq "stderr の知らせは 2 件" "2" "$(wc -l < "$TMP/out/new-both.notices" | tr -d ' ')"
same_bytes "--check の全文が旧経路と同じ" "$TMP/out/old-both.err" "$TMP/out/new-both.err"
run new-both-json "$FAKES" TAKO_AUTH_PROBE_TIMEOUT_SECS=2 TAKO_SETUP_PROBE_TIMEOUT_SECS=3 -- check-health --json
check_eq "probe_timeouts は認証 → claude mcp list の順" \
  '[{"label": "claude auth status --json", "waited_secs": 2}, {"label": "claude mcp list", "waited_secs": 3}]' \
  "$(jf probe_timeouts < "$TMP/out/new-both-json.out")"

# --- エッジ: 導入済みだが PATH に無い / エージェント CLI が 1 つも無い ------------------
# MCP の問い合わせは導入状況を待たずに背景で始まる。項目は今まで通り導入状況で決まること
echo "== エッジ: claude はインストーラの置き場（~/.local/bin）にだけ在り、PATH に無い"
set_hang
run new-offpath "" -- check-health --json
run old-offpath "" TAKO_1622_LEGACY=1 -- check-health --json
check_eq "claude は導入済み・PATH の段" "path" "$(jf items/key=agent_cli.claude/summary < "$TMP/out/new-offpath.out" | sed 's/.*(\(.*\))$/\1/')"
check_eq "mcp.claude は今まで通り未登録（PATH で見つからない）" "missing" "$(jf items/key=mcp.claude/status < "$TMP/out/new-offpath.out")"
same_bytes "--json の全文が旧経路と同じ" "$TMP/out/old-offpath.out" "$TMP/out/new-offpath.out"

echo "== エッジ: エージェント CLI が 1 つも無い砂場"
command mv -f "$CLAUDE_DIR/claude" "$TMP/claude.away"
run new-none "" -- check-health --json
run old-none "" TAKO_1622_LEGACY=1 -- check-health --json
command mv -f "$TMP/claude.away" "$CLAUDE_DIR/claude"
check_eq "agent_cli は不足" "missing" "$(jf items/key=agent_cli/status < "$TMP/out/new-none.out")"
check_eq "MCP の項目は出ない（未導入の系統の答えは捨てる）" "null" "$(jf items/key=mcp.claude < "$TMP/out/new-none.out")"
same_bytes "--json の全文が旧経路と同じ" "$TMP/out/old-none.out" "$TMP/out/new-none.out"

echo
echo "結果: OK $PASS / NG $FAIL"
[ "$FAIL" -eq 0 ] || exit 1
tako_exit 0
