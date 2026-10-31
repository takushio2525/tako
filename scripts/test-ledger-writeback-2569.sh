#!/bin/bash
# tako:run bash scripts/test-ledger-writeback-2569.sh
# test-ledger-writeback-2569.sh — 断った台帳・プロジェクト・アカウントの操作が置き場を書かない（#2569）
#
# 何を見るか（CLI と MCP の両方。どちらも同じ dispatch / 台帳の関数を通る）:
#   ① 台帳の無い置き場で、無いエントリへ record / amend → 断りが返り、ledger.yaml を作らない
#   ② 手で書いた台帳（コメント入り）の置き場で同じ断り → ledger.yaml がバイト一致・mtime / inode 不変・
#      世代バックアップ（.bak.1）を作らない
#   ③ 壊れた台帳（YAML として読めない）で record → 断り、壊れたファイルを上書きしない
#   ④ 無いプロジェクト / アカウントの remove → projects.yaml / accounts.yaml を作らない・
#      手で書いたものを書き換えない
#   ⑤ 正常な record / amend / prune の応答と結果の ledger.yaml を KEEP へ残す（修正前後のバイト一致の突き合わせ用）
#   ⑥ 同時に 2 つの record（ロック。CLI のみ）: 2 本とも残る・同時に断った 1 本が他方を消さない
#
# A/B: 修正前の木で走らせると ① ② ④ が NG になる（#2569 の症状）。`TAKO_2569_KEEP=<dir>` を渡すと
# 全出力を `<dir>/<cli|mcp>/<言語>/<名前>.(out|rc|file)` へ残す（`diff -r` で修正前後を突き合わせる）。
#
# **本番の tako / 設定には一切触らない**（HOME / data / discovery / orchestrator は mktemp 配下、
# tmux は専用ソケット、落とすのは自分で起こした pid だけ）。MCP の節は常設の仮想ディスプレイ（tako-vd）に
# 隔離 GUI を立てる。面を用意できなければ起動せずに終了コード 4（未実測）で止まる（#1744）。
#
# 使い方: bash scripts/test-ledger-writeback-2569.sh
#
# **CI には載せない**（実 GUI + 仮想ディスプレイが要る）。台帳の書き戻しは `ledger.rs` の単体、
# dispatch の経路は `crates/tako-control/tests/issue2365_ledger_values_lang.rs` が CI で見る。
set -uo pipefail

# **本番 GUI を指す env を最初に落とす**（#1449 / #1450 で本番にペインが漏れた実例が 2 件）
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_MCP_URL TAKO_ORCHESTRATOR_ROLE TAKO_LANG

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PASS=0
FAIL=0

pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
fail() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }

TMP="$(mktemp -d /tmp/tako-2569-XXXXXX)"
APP_PID=""
TMUX_SOCKET=""
cleanup() {
  stop_isolated_gui "$APP_PID"
  [ -n "$TMUX_SOCKET" ] && tmux -L "$TMUX_SOCKET" kill-server >/dev/null 2>&1
  rm -rf "${TMP:?}"
}
trap cleanup EXIT

# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
isolated_gui_bins || exit 1

KEEP="${TAKO_2569_KEEP:-}"

# 手で書いた台帳（コメントと flow 形式 = tako が書く形とバイトが違う。書き戻すと形が変わる）
HAND_LEDGER='# 手で書いた台帳（#2569 の検証）
entries:
  - {id: spawn_a, ts: "2026-10-31T00:00:00Z", project: tako, task_type: docs, model: opus}
  - {id: spawn_b, ts: "2026-10-31T00:00:01Z", project: tmp-x, task_type: review, model: sonnet}
'
BROKEN_LEDGER='entries: [
'
HAND_PROJECTS='# 手で書いたプロジェクト一覧（#2569 の検証）
version: 2
projects:
  alpha: {cwd: /tmp/alpha2569}
'
HAND_ACCOUNTS='# 手で書いたアカウント一覧（#2569 の検証）
accounts:
  main2569: {inherit: true}
'

# ファイルの指紋（無ければ absent）。sha256・mtime（ns）・inode
fingerprint() {
  python3 - "$1" <<'PY'
import hashlib, os, sys
p = sys.argv[1]
if not os.path.lexists(p):
    print("absent"); sys.exit(0)
st = os.stat(p)
print("%s %d %d" % (hashlib.sha256(open(p, "rb").read()).hexdigest(), st.st_mtime_ns, st.st_ino))
PY
}

# 置き場（orchestrator dir）の中身を空にする（ロックファイルは残してよい = 中身を持たない）
reset_orch() {
  local dir="$1"
  case "$dir" in
    "$TMP"/*) : ;;
    *) echo "隔離されていないディレクトリ: $dir"; exit 1 ;;
  esac
  rm -rf "${dir:?}"
  mkdir -p "$dir"
}

# 出力を KEEP へ写す（$1 = 経路 / 言語、$2 = 名前、$3 = 応答のファイル、$4 = 終了コード or is_error）
keep_out() {
  [ -n "$KEEP" ] || return 0
  mkdir -p "$KEEP/$1"
  command cp -f "$3" "$KEEP/$1/$2.out"
  echo "$4" > "$KEEP/$1/$2.rc"
}

# 台帳を KEEP へ写す（正常系の結果の突き合わせ用）
keep_file() {
  [ -n "$KEEP" ] || return 0
  mkdir -p "$KEEP/$1"
  if [ -f "$3" ]; then command cp -f "$3" "$KEEP/$1/$2.file"; else echo absent > "$KEEP/$1/$2.file"; fi
}

# settings.json の表示言語を書く（CLI はこれを読む）
write_language() {
  python3 - "$TAKO_DATA_DIR/settings.json" "$1" <<'PY'
import json, os, sys
p, lang = sys.argv[1], sys.argv[2]
d = {}
if os.path.exists(p):
    try:
        d = json.load(open(p, encoding="utf-8"))
    except Exception:
        d = {}
d["language"] = lang
json.dump(d, open(p, "w", encoding="utf-8"), ensure_ascii=False, indent=2)
PY
}

# 経路ごとの 1 回の操作。$1 = 経路（cli / mcp）、$2 = 出力の置き場、以降は操作
#   ledger record <id> <outcome> [rounds note] / ledger amend <id> <note> / ledger prune <prefix>
#   projects remove <key> / accounts remove <name>
# 終了コード（CLI）か is_error（MCP）を標準出力へ 1 行出す
op() {
  local route="$1" out="$2" kind="$3" action="$4"
  shift 4
  if [ "$route" = cli ]; then
    case "$kind/$action" in
      ledger/record)
        if [ $# -ge 4 ]; then
          "$TAKO_BIN" orchestrator ledger record "$1" --outcome "$2" --rounds "$3" --note "$4" > "$out" 2>&1
        else
          "$TAKO_BIN" orchestrator ledger record "$1" --outcome "$2" > "$out" 2>&1
        fi ;;
      ledger/amend) "$TAKO_BIN" orchestrator ledger amend "$1" --note "$2" > "$out" 2>&1 ;;
      ledger/prune) "$TAKO_BIN" orchestrator ledger prune --project-prefix "$1" > "$out" 2>&1 ;;
      projects/remove) "$TAKO_BIN" orchestrator projects remove --key "$1" > "$out" 2>&1 ;;
      accounts/remove) "$TAKO_BIN" orchestrator accounts remove "$1" > "$out" 2>&1 ;;
      *) echo "未知の操作: $kind/$action"; exit 1 ;;
    esac
    echo "rc=$?"
    return
  fi
  local args
  case "$kind/$action" in
    ledger/record)
      if [ $# -ge 4 ]; then
        args="{\"action\":\"record\",\"id\":\"$1\",\"outcome\":\"$2\",\"rounds\":$3,\"note\":\"$4\"}"
      else
        args="{\"action\":\"record\",\"id\":\"$1\",\"outcome\":\"$2\"}"
      fi ;;
    ledger/amend) args="{\"action\":\"amend\",\"id\":\"$1\",\"note\":\"$2\"}" ;;
    ledger/prune) args="{\"action\":\"prune\",\"project\":\"$1\"}" ;;
    projects/remove) args="{\"action\":\"remove\",\"key\":\"$1\"}" ;;
    accounts/remove) args="{\"action\":\"remove\",\"name\":\"$1\"}" ;;
    *) echo "未知の操作: $kind/$action"; exit 1 ;;
  esac
  mcp_call "tako_orchestrator_$kind" "$args" "$out"
}

# MCP の tools/call を 1 本撃ち、結果の本文を $3 へ書く。is_error を出す
mcp_call() {
  local tool="$1" args="$2" out="$3" sock token
  cat > "$TMP/mcp-in.jsonl" <<JSONL
{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test-2569","version":"0"}}}
{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"${tool}","arguments":${args}}}
JSONL
  # `mcp serve` はツール公開の判定を env だけで行う（FR-2.3.2）ので接続情報ファイルから渡す。
  # **トークンは表示しない**（`conventions.md`: 診断ログに TAKO_TOKEN を出さない）
  sock="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["socket"])' "$TAKO_DISCOVERY_DIR/control.json" 2>/dev/null)"
  token="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["token"])' "$TAKO_DISCOVERY_DIR/control.json" 2>/dev/null)"
  if [ -z "$sock" ] || [ -z "$token" ]; then
    : > "$out"
    echo "is_error=__NO_DISCOVERY__"
    return
  fi
  env TAKO_SOCKET="$sock" TAKO_TOKEN="$token" \
    "$TAKO_BIN" mcp serve < "$TMP/mcp-in.jsonl" > "$TMP/mcp-out.jsonl" 2> "$TMP/mcp-err.log"
  python3 - "$TMP/mcp-out.jsonl" "$out" <<'PY'
import json, sys
out = open(sys.argv[2], "w", encoding="utf-8")
for line in open(sys.argv[1], encoding="utf-8"):
    line = line.strip()
    if not line:
        continue
    try:
        msg = json.loads(line)
    except Exception:
        continue
    if msg.get("id") != 2:
        continue
    res = msg.get("result") or {}
    text = "".join(c.get("text", "") for c in res.get("content", []) if c.get("type") == "text")
    if msg.get("error"):
        text = msg["error"].get("message", "")
    out.write(text)
    print("is_error=%s" % bool(res.get("isError") or msg.get("error")))
    sys.exit(0)
print("is_error=__NO_RESPONSE__")
PY
}

# 断りが返ったか（CLI は rc≠0、MCP は is_error=True）。$2 / $3 を渡すと、応答（$2）に名指し（$3）が
# 載っていることも見る（引数の誤りなど、別の理由の断りを「断った」と数えない）
refused() {
  case "$1" in
    rc=0 | is_error=False | is_error=__*) return 1 ;;
  esac
  [ $# -lt 3 ] || grep -q -- "$3" "$2"
}

# 1 つの経路・言語で全ケースを撃つ。$1 = 経路、$2 = 言語、$3 = orchestrator dir
run_cases() {
  local route="$1" lang="$2" orch="$3" tag="$1/$2" out="$TMP/out-$1-$2" res res2 res3 before after name
  mkdir -p "$out"
  echo "== ${route} / ${lang} =="

  # ① 台帳の無い置き場で、無いエントリへ record / amend
  for name in record amend; do
    reset_orch "$orch"
    if [ "$name" = record ]; then
      res="$(op "$route" "$out/empty-$name" ledger record nope2569 pass)"
    else
      res="$(op "$route" "$out/empty-$name" ledger amend nope2569 x)"
    fi
    keep_out "$tag" "empty-$name" "$out/empty-$name" "$res"
    if refused "$res" "$out/empty-$name" nope2569; then pass "① 空の置き場の $name は断る（$(head -c 160 "$out/empty-$name" | tr '\n' ' ')）"; else fail "① 空の置き場の $name が断られない（${res}）"; fi
    if [ "$(fingerprint "$orch/ledger.yaml")" = absent ]; then
      pass "① 空の置き場の $name で ledger.yaml を作らない"
    else
      fail "① 空の置き場の $name で ledger.yaml ができた（中身: $(tr '\n' ' ' < "$orch/ledger.yaml")）"
    fi
  done

  # ② 手で書いた台帳の置き場で、無いエントリへ record / amend（と、検証で断る outcome）
  for name in record amend badoutcome; do
    reset_orch "$orch"
    printf '%s' "$HAND_LEDGER" > "$orch/ledger.yaml"
    before="$(fingerprint "$orch/ledger.yaml")"
    case "$name" in
      record) res="$(op "$route" "$out/hand-$name" ledger record nope2569 pass)" ;;
      amend) res="$(op "$route" "$out/hand-$name" ledger amend nope2569 x)" ;;
      badoutcome) res="$(op "$route" "$out/hand-$name" ledger record spawn_a bogus)" ;;
    esac
    keep_out "$tag" "hand-$name" "$out/hand-$name" "$res"
    after="$(fingerprint "$orch/ledger.yaml")"
    refused "$res" "$out/hand-$name" "$([ "$name" = badoutcome ] && echo bogus || echo nope2569)" || fail "② 手書きの台帳の $name が断られない（${res}）"
    if [ "$before" = "$after" ] && [ ! -e "$orch/ledger.yaml.bak.1" ]; then
      pass "② 手書きの台帳の $name の前後で ledger.yaml がバイト・mtime・inode とも一致し .bak.1 も無い"
    else
      fail "② 手書きの台帳の $name で ledger.yaml が書き換わった（前 ${before%% *} / 後 ${after%% *}・.bak.1: $([ -e "$orch/ledger.yaml.bak.1" ] && echo あり || echo 無し)）"
    fi
  done

  # ③ 壊れた台帳で record → 断り、上書きしない
  reset_orch "$orch"
  printf '%s' "$BROKEN_LEDGER" > "$orch/ledger.yaml"
  before="$(fingerprint "$orch/ledger.yaml")"
  res="$(op "$route" "$out/broken-record" ledger record nope2569 pass)"
  keep_out "$tag" "broken-record" "$out/broken-record" "$res"
  after="$(fingerprint "$orch/ledger.yaml")"
  if refused "$res" "$out/broken-record" ledger.yaml && [ "$before" = "$after" ]; then
    pass "③ 壊れた台帳は断って上書きしない"
  else
    fail "③ 壊れた台帳（${res}・前 ${before%% *} / 後 ${after%% *}）"
  fi

  # ④ 無いプロジェクト / アカウントの remove
  local kind file hand key
  for kind in projects accounts; do
    if [ "$kind" = projects ]; then file=projects.yaml; hand="$HAND_PROJECTS"; key=nope2569; else file=accounts.yaml; hand="$HAND_ACCOUNTS"; key=nope2569; fi
    reset_orch "$orch"
    res="$(op "$route" "$out/empty-$kind-remove" "$kind" remove "$key")"
    keep_out "$tag" "empty-$kind-remove" "$out/empty-$kind-remove" "$res"
    refused "$res" "$out/empty-$kind-remove" "$key" || fail "④ 空の置き場の $kind remove が断られない（${res}）"
    if [ "$(fingerprint "$orch/$file")" = absent ]; then
      pass "④ 空の置き場の $kind remove で $file を作らない"
    else
      fail "④ 空の置き場の $kind remove で $file ができた（中身: $(tr '\n' ' ' < "$orch/$file")）"
    fi
    reset_orch "$orch"
    printf '%s' "$hand" > "$orch/$file"
    before="$(fingerprint "$orch/$file")"
    res="$(op "$route" "$out/hand-$kind-remove" "$kind" remove "$key")"
    keep_out "$tag" "hand-$kind-remove" "$out/hand-$kind-remove" "$res"
    after="$(fingerprint "$orch/$file")"
    refused "$res" "$out/hand-$kind-remove" "$key" || fail "④ 手書きの $file の remove が断られない（${res}）"
    if [ "$before" = "$after" ] && [ ! -e "$orch/$file.bak.1" ]; then
      pass "④ 手書きの $file の remove（無いキー）の前後でバイト・mtime・inode とも一致"
    else
      fail "④ 手書きの $file の remove（無いキー）で書き換わった（前 ${before%% *} / 後 ${after%% *}）"
    fi
  done

  # ⑤ 正常な record / amend / prune（結果は KEEP で修正前後を突き合わせる）
  reset_orch "$orch"
  printf '%s' "$HAND_LEDGER" > "$orch/ledger.yaml"
  res="$(op "$route" "$out/ok-record" ledger record spawn_a rework 2 again)"
  keep_out "$tag" "ok-record" "$out/ok-record" "$res"
  keep_file "$tag" "ok-record-ledger" "$orch/ledger.yaml"
  res2="$(op "$route" "$out/ok-amend" ledger amend spawn_b late)"
  keep_out "$tag" "ok-amend" "$out/ok-amend" "$res2"
  keep_file "$tag" "ok-amend-ledger" "$orch/ledger.yaml"
  res3="$(op "$route" "$out/ok-prune" ledger prune tmp-)"
  keep_out "$tag" "ok-prune" "$out/ok-prune" "$res3"
  keep_file "$tag" "ok-prune-ledger" "$orch/ledger.yaml"
  if ! refused "$res" && ! refused "$res2" && ! refused "$res3" \
    && grep -q 'outcome: rework' "$orch/ledger.yaml" && ! grep -q 'spawn_b' "$orch/ledger.yaml"; then
    pass "⑤ 正常な record / amend / prune が通り台帳へ反映（$res / $res2 / ${res3}）"
  else
    fail "⑤ 正常な record / amend / prune（$res / $res2 / ${res3}）: $(tr '\n' ' ' < "$orch/ledger.yaml")"
  fi
  # 空の置き場の prune（断りではない = 修正前と同じく 0 件で通る）
  reset_orch "$orch"
  res="$(op "$route" "$out/empty-prune" ledger prune tmp-)"
  keep_out "$tag" "empty-prune" "$out/empty-prune" "$res"
  keep_file "$tag" "empty-prune-ledger" "$orch/ledger.yaml"
  if ! refused "$res"; then pass "⑤ 空の置き場の prune は 0 件で通る"; else fail "⑤ 空の置き場の prune が断られた（${res}）"; fi
}

# ⑥ 同時に 2 つの record（CLI。ロックで直列化され、2 本とも残る / 断った 1 本が他方を消さない）
run_concurrent() {
  local orch="$1" i lost=0 clobbered=0 out="$TMP/out-concurrent"
  mkdir -p "$out"
  echo "== 同時の record（CLI・20 回） =="
  for i in $(seq 1 20); do
    reset_orch "$orch"
    printf '%s' "$HAND_LEDGER" > "$orch/ledger.yaml"
    "$TAKO_BIN" orchestrator ledger record spawn_a --outcome pass > "$out/a" 2>&1 &
    local pa=$!
    "$TAKO_BIN" orchestrator ledger record spawn_b --outcome fail > "$out/b" 2>&1 &
    local pb=$!
    "$TAKO_BIN" orchestrator ledger record nope2569 --outcome pass > "$out/n" 2>&1 &
    local pn=$!
    wait "$pa"; wait "$pb"; wait "$pn"
    python3 - "$orch/ledger.yaml" <<'PY' || lost=$((lost + 1))
import re, sys
t = open(sys.argv[1], encoding="utf-8").read()
# 2 本とも残る（spawn_a = pass / spawn_b = fail）。断った 1 本（nope2569）はエントリを足さない
blocks = re.split(r"\n- ", "\n" + t)
a = [b for b in blocks if "id: spawn_a" in b]
b = [b for b in blocks if "id: spawn_b" in b]
ok = len(a) == 1 and len(b) == 1 and "outcome: pass" in a[0] and "outcome: fail" in b[0] and "nope2569" not in t
sys.exit(0 if ok else 1)
PY
    [ -e "$orch/ledger.yaml" ] || clobbered=$((clobbered + 1))
  done
  if [ "$lost" -eq 0 ] && [ "$clobbered" -eq 0 ]; then
    pass "⑥ 同時の record 3 本 × 20 回で更新の取りこぼし 0・断った 1 本は台帳を消さない"
  else
    fail "⑥ 同時の record で取りこぼし ${lost} 回・台帳の消失 ${clobbered} 回"
  fi
}

# ---- CLI（手元の処理 = dispatch / 台帳の関数を直呼び） ----
base="$TMP/cli"
export HOME="$base/home"
export TAKO_ISOLATED=1
export TAKO_DATA_DIR="$base/data"
export TAKO_DISCOVERY_DIR="$base/disc"
export TAKO_ORCHESTRATOR_DIR="$base/orch"
mkdir -p "$HOME" "$TAKO_DATA_DIR" "$TAKO_DISCOVERY_DIR" "$TAKO_ORCHESTRATOR_DIR"
case "$TAKO_DATA_DIR" in
  "$TMP"/*) : ;;
  *) echo "隔離されていないディレクトリ: $TAKO_DATA_DIR"; exit 1 ;;
esac
for lang in en ja; do
  write_language "$lang"
  run_cases cli "$lang" "$TAKO_ORCHESTRATOR_DIR"
done
run_concurrent "$TAKO_ORCHESTRATOR_DIR"

# ---- MCP（隔離 GUI の dispatch） ----
base="$TMP/mcp"
TMUX_SOCKET="tako-2569-$$"
export HOME="$base/home"
export TAKO_DATA_DIR="$base/data"
export TAKO_DISCOVERY_DIR="$base/disc"
export TAKO_ORCHESTRATOR_DIR="$base/orch"
export TAKO_SESSIONS_FILE="$base/sessions.yaml"
export TAKO_PANE_LOG_DIR="$base/panelogs"
export TAKO_WORKERS_FILE="$base/workers.yaml"
export TAKO_TMUX_SOCKET="$TMUX_SOCKET"
mkdir -p "$HOME" "$TAKO_DATA_DIR" "$TAKO_DISCOVERY_DIR" "$TAKO_ORCHESTRATOR_DIR"
case "$TAKO_DATA_DIR" in
  "$TMP"/*) : ;;
  *) echo "隔離されていないディレクトリ: $TAKO_DATA_DIR"; exit 1 ;;
esac
write_language en
launch_isolated_gui "$base/app.log" || exit $?
APP_PID="$ISOLATED_GUI_PID"
if ! wait_isolated_gui "$base/app.log"; then
  echo "隔離 GUI へ CLI が繋がらない"
  exit 1
fi
for lang in en ja; do
  res="$(mcp_call tako_lang "{\"action\":\"set\",\"value\":\"$lang\"}" "$TMP/lang.out")"
  [ "$res" = "is_error=False" ] || { echo "表示言語を $lang にできない（${res}）"; exit 1; }
  run_cases mcp "$lang" "$TAKO_ORCHESTRATOR_DIR"
done

echo
echo "結果: OK ${PASS} / NG ${FAIL}"
[ "$FAIL" -eq 0 ]
