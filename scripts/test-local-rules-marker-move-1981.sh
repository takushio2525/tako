#!/usr/bin/env bash
# #1981 の実経路テスト: 区切り `<!-- tako:on-demand -->` のある追記（local-rules.md）の
# 常時部があとから伸びて取り分（5,632 B）を超えたとき、
#   1) `tako migrate`（status）にだけ「N バイト超過・区切りを『## X』の前へ上げる案」が出て
#   2) 書き換えは起きず
#   3) 明示の `tako migrate run` で見出し境界へ区切りが上がり、区切りの行を除いた本文が
#      バイト単位で一致し、`tako context-budget check` の違反が消える
# ことを、**隔離した HOME / TAKO_DATA_DIR**で実測する。
#
# 検体は既定で fixture から作る（#1477 の分割を当てたあと、常時部へ節を書き足す）。
# 手元の写しで試すときは LOCAL_RULES_SRC=<path>（読むだけ。写しを sandbox へ置く）。
# 本番の ~/Library/Application Support/tako は一切触らない（HOME ごと差し替える）。
set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TAKO="${TAKO_BIN:-$ROOT/target/debug/tako}"
FIXTURE="$ROOT/crates/tako-control/tests/fixtures/prompt_append_oversized.md"
MARKER='<!-- tako:on-demand -->'
PASS=0; FAIL=0

ok()   { PASS=$((PASS+1)); echo "  PASS: $1"; }
ng()   { FAIL=$((FAIL+1)); echo "  FAIL: $1"; }
check(){ if [ "$2" = "$3" ]; then ok "$1"; else ng "$1 (期待 '$3' / 実際 '$2')"; fi; }
same() { if cmp -s "$2" "$3"; then ok "$1"; else ng "$1 ($2 と $3 が違う)"; fi; }

[ -x "$TAKO" ] || { echo "tako が無い: ${TAKO}（cargo build -p tako-cli）"; exit 1; }

SANDBOX="$(mktemp -d "${TMPDIR:-/tmp}/tako-1981-XXXXXX")"
trap 'rm -rf "$SANDBOX"' EXIT
export HOME="$SANDBOX/home"
export TAKO_DATA_DIR="$SANDBOX/d"
export TAKO_ISOLATED=1
unset TAKO_SOCKET TAKO_PANE_ID
PROFILES="$TAKO_DATA_DIR/orchestrator/profiles"
mkdir -p "$PROFILES" "$HOME"
RULES="$HOME/local-rules.md"
run_tako(){ "$TAKO" "$@" 2>&1; }
printf 'model: claude-opus-5\nprompt_blocks:\n  append: ~/local-rules.md\n' > "$PROFILES/rules.yaml"

# files[] から local-rules.md の 1 行を取り出して key=value で並べる
field(){ # $1 = JSON ファイル, $2 = キー
  python3 - "$1" "$2" "$RULES" <<'PY'
import json, sys
d = json.load(open(sys.argv[1]))
for f in d["files"]:
    if f["path"] == sys.argv[3]:
        v = f.get(sys.argv[2])
        print("" if v is None else (json.dumps(v, ensure_ascii=False) if isinstance(v, (list, dict)) else v))
PY
}
# 区切りの手前 + 索引（prompt が払う量）と system prompt の違反数を context-budget から読む
budget(){
  run_tako context-budget check --cwd "$SANDBOX" --profile rules --json > "$SANDBOX/b.json"
  python3 - "$SANDBOX/b.json" <<'PY'
import json, sys
d = json.load(open(sys.argv[1]))
for i in d["items"]:
    if i["kind"] == "system_prompt" and "master" in i["path"]:
        app = sum(p["bytes"] for p in i["pieces"] if p["name"].startswith("append"))
        print(f"violations={len(i.get('violations', []))} total={i['bytes']} append={app}")
PY
}

echo "== 0) 検体 =="
if [ -n "${LOCAL_RULES_SRC:-}" ]; then
  cp "$LOCAL_RULES_SRC" "$RULES"
  echo "  検体: LOCAL_RULES_SRC の写し（$(wc -c < "$RULES" | tr -d ' ') B）"
else
  # #1477 の分割を当て、常時部（区切りの直前）へ 1 節書き足す = 利用者が規則を足した姿
  cp "$FIXTURE" "$RULES"
  run_tako migrate run --schema prompt_append > /dev/null
  grep -qF -- "$MARKER" "$RULES" || { echo "前提: #1477 の分割が当たっていない"; exit 1; }
  python3 - "$RULES" "$MARKER" <<'PY'
import sys
path, marker = sys.argv[1], sys.argv[2]
text = open(path, encoding="utf-8").read()
at = text.index(marker)
section = "## あとから書き足した規則\n\n" + "- 書き足した行の本文はここに続く\n" * 200 + "\n"
open(path, "w", encoding="utf-8").write(text[:at] + section + text[at:])
PY
  rm -f "$RULES.pre-v1.bak"
  echo "  検体: fixture + #1477 の分割 + 常時部へ 1 節（$(wc -c < "$RULES" | tr -d ' ') B）"
fi
command cp -f "$RULES" "$SANDBOX/before.md"
BEFORE_BUDGET=$(budget); echo "     $BEFORE_BUDGET"

echo "== 1) tako migrate（status）に案が 1 か所に出る =="
run_tako migrate > "$SANDBOX/status.json"
check "local-rules.md が proposed" "$(field "$SANDBOX/status.json" state)" "proposed"
check "承認で当たる案" "$(field "$SANDBOX/status.json" approvable)" "True"
OVER=$(field "$SANDBOX/status.json" over_bytes)
LIMIT=$(field "$SANDBOX/status.json" limit)
check "取り分は 5632 B" "$LIMIT" "5632"
if [ "${OVER:-0}" -gt 0 ]; then ok "超過バイト数が出る（${OVER} B）"; else ng "over_bytes が無い"; fi
HEAD=$(field "$SANDBOX/status.json" move_before)
HLINE=$(field "$SANDBOX/status.json" move_before_line)
check "move_before は実在の見出し行" "$(sed -n "${HLINE}p" "$RULES")" "$HEAD"
check "承認のコマンドは最簡形" "$(field "$SANDBOX/status.json" approve)" "tako migrate run"
echo "     案: $(field "$SANDBOX/status.json" proposal)"
python3 -c "import json,sys; d=json.load(open(sys.argv[1])); print(d['notice'])" "$SANDBOX/status.json" > "$SANDBOX/notice.txt"
if grep -qF 'tako migrate run' "$SANDBOX/notice.txt"; then ok "一言にも承認のコマンドが出る"; else ng "notice: $(cat "$SANDBOX/notice.txt")"; fi
same "status は書き換えない" "$SANDBOX/before.md" "$RULES"

echo "== 2) 承認 = tako migrate run =="
run_tako migrate run > "$SANDBOX/run.json"
check "local-rules.md が migrated" "$(field "$SANDBOX/run.json" state)" "migrated"
grep -vF -- "$MARKER" "$SANDBOX/before.md" > "$SANDBOX/before.stripped"
grep -vF -- "$MARKER" "$RULES" > "$SANDBOX/after.stripped"
same "区切りの行を除いた本文がバイト単位で一致" "$SANDBOX/before.stripped" "$SANDBOX/after.stripped"
check "区切りの行は 1 本のまま" "$(grep -cF -- "$MARKER" "$RULES")" "$(grep -cF -- "$MARKER" "$SANDBOX/before.md")"
NEW_LINE=$(grep -nF -- "$MARKER" "$RULES" | head -1 | cut -d: -f1)
check "区切りは案の見出しの直前" "$(sed -n "$((NEW_LINE + 1))p" "$RULES")" "$HEAD"
same "旧内容は .pre-v2.bak" "$SANDBOX/before.md" "$RULES.pre-v2.bak"
AFTER_BUDGET=$(budget); echo "     $AFTER_BUDGET"
case "$AFTER_BUDGET" in *violations=0*) ok "context-budget の system prompt 違反が 0";; *) ng "違反が残る: $AFTER_BUDGET";; esac
case "$BEFORE_BUDGET" in
  *violations=0*) echo "     （当てる前も合計の違反は無かった = この base では取り分の超過だけ）";;
  *) ok "当てる前は違反があった（${BEFORE_BUDGET}）";;
esac
APP=$(echo "$AFTER_BUDGET" | sed -n 's/.*append=\([0-9]*\).*/\1/p')
if [ "${APP:-99999}" -le $((LIMIT + 1)) ]; then ok "追記の piece は取り分以内（${APP} B）"; else ng "追記の piece が ${APP} B"; fi
if grep -qF '区切りを上げた' "$TAKO_DATA_DIR/persist.log" 2>/dev/null; then ok "persist.log に記録"; else ng "persist.log に記録が無い"; fi

echo "== 3) 2 回目は何もしない =="
command cp -f "$RULES" "$SANDBOX/after-first.md"
run_tako migrate run > "$SANDBOX/run2.json"
check "2 回目は up_to_date" "$(field "$SANDBOX/run2.json" state)" "up_to_date"
same "2 回目でファイルが変わらない" "$SANDBOX/after-first.md" "$RULES"
run_tako migrate > "$SANDBOX/status2.json"
check "当てたあとの status は静か" "$(field "$SANDBOX/status2.json" state)" "up_to_date"

echo "== 4) 境界: 上げても収まらない / 区切りが先頭 / 取り分内 =="
python3 -c "import sys; sys.stdout.write('見出しの無い常時の規則\n'*400)" > "$RULES"
printf '%s\n## 詳細\n\n本文\n' "$MARKER" >> "$RULES"
command cp -f "$RULES" "$SANDBOX/flat.md"
run_tako migrate > "$SANDBOX/flat.json"
check "見出しの無い常時部は proposed" "$(field "$SANDBOX/flat.json" state)" "proposed"
check "…ただし承認では当たらない" "$(field "$SANDBOX/flat.json" approvable)" "False"
check "…承認のコマンドを出さない" "$(field "$SANDBOX/flat.json" approve)" ""
run_tako migrate run > /dev/null
same "…run でも動かさない" "$SANDBOX/flat.md" "$RULES"

{ printf '# ルール\n\n'; python3 -c "import sys; sys.stdout.write('前書き'*2000)"; printf '\n## 小さい節\n\nx\n%s\n## 詳細\n\n本文\n' "$MARKER"; } > "$RULES"
command cp -f "$RULES" "$SANDBOX/fat.md"
run_tako migrate run > "$SANDBOX/fat.json"
check "先頭の節だけで超過は承認不可の案内" "$(field "$SANDBOX/fat.json" approvable)" "False"
same "…run でも動かさない" "$SANDBOX/fat.md" "$RULES"

printf '%s\n# ルール\n\n## A\n\naaa\n' "$MARKER" > "$RULES"
run_tako migrate > "$SANDBOX/top.json"
check "区切りが先頭（常時部が空）は何も出ない" "$(field "$SANDBOX/top.json" state)" "up_to_date"

printf '# ルール\n\n## 言語\n\n日本語\n%s\n## 詳細\n\n本文\n' "$MARKER" > "$RULES"
run_tako migrate > "$SANDBOX/fits.json"
check "取り分内は何も出ない" "$(field "$SANDBOX/fits.json" state)" "up_to_date"
check "…案の件数は 0" "$(python3 -c "import json,sys; print(json.load(open(sys.argv[1]))['proposed'])" "$SANDBOX/fits.json")" "0"

echo
echo "==== $PASS PASS / $FAIL FAIL ===="
[ "$FAIL" -eq 0 ]
