#!/usr/bin/env bash
# #1477 の実経路テスト: 予算を超えた追記（prompt_blocks.append）が
# `tako migrate run` で常時部 / on-demand 部へ自動分割され、system prompt が
# 予算に収まることを**隔離した HOME / TAKO_DATA_DIR**で実測する。
#
# 本番の ~/Library/Application Support/tako は一切触らない（HOME ごと差し替える）。
set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TAKO="${TAKO_BIN:-$ROOT/target/debug/tako}"
FIXTURE="$ROOT/crates/tako-control/tests/fixtures/prompt_append_oversized.md"
PASS=0; FAIL=0

ok()   { PASS=$((PASS+1)); echo "  PASS: $1"; }
ng()   { FAIL=$((FAIL+1)); echo "  FAIL: $1"; }
check(){ if [ "$2" = "$3" ]; then ok "$1"; else ng "$1 (期待 '$3' / 実際 '$2')"; fi; }
contains(){ if grep -qF -- "$2" "$3" 2>/dev/null; then ok "$1"; else ng "$1 ('$2' が $3 に無い)"; fi; }
absent(){ if grep -qF -- "$2" "$3" 2>/dev/null; then ng "$1 ('$2' が $3 に在る)"; else ok "$1"; fi; }

[ -x "$TAKO" ] || { echo "tako が無い: ${TAKO}（cargo build -p tako-cli）"; exit 1; }
[ -f "$FIXTURE" ] || { echo "fixture が無い: $FIXTURE"; exit 1; }

SANDBOX="$(mktemp -d "${TMPDIR:-/tmp}/tako-1477-e2e-XXXXXX")"
trap 'rm -rf "$SANDBOX"' EXIT
export HOME="$SANDBOX/home"
export TAKO_DATA_DIR="$SANDBOX/home/data"
export TAKO_ISOLATED=1
PROFILES="$TAKO_DATA_DIR/orchestrator/profiles"
mkdir -p "$PROFILES" "$HOME"

MARKER='<!-- tako:on-demand -->'
run_tako(){ "$TAKO" "$@" 2>&1; }

# --- 検体を並べる（1 プロファイル = 1 追記） --------------------------------
cp "$FIXTURE" "$HOME/local-rules.md"                       # 12.3 KB・見出し 12 本 → 分割される
printf '# 小さいルール\n\n## 言語\n\n日本語で書く\n' > "$HOME/small.md"
{ printf '常時のルール\n%s\n' "$MARKER"; cat "$FIXTURE"; } > "$HOME/already-marked.md"
python3 -c "import sys; sys.stdout.write('見出しの無い本文\n'*900)" > "$HOME/no-heading.md"

mkprofile(){ printf 'model: claude-opus-5\nprompt_blocks:\n  append: %s\n' "$2" > "$PROFILES/$1.yaml"; }
mkprofile big     '~/local-rules.md'
mkprofile small   '~/small.md'
mkprofile marked  '~/already-marked.md'
mkprofile flat    '~/no-heading.md'
mkprofile inline  '"常時だけのインライン追記"'
printf 'model: claude-opus-5\n' > "$PROFILES/plain.yaml"     # append 無し

cp "$HOME/local-rules.md" "$SANDBOX/original.md"
cp "$HOME/small.md" "$SANDBOX/small-original.md"
cp "$HOME/already-marked.md" "$SANDBOX/marked-original.md"
cp "$HOME/no-heading.md" "$SANDBOX/flat-original.md"

echo "== 1) 移行前の状態 =="
run_tako migrate status --schema prompt_append > "$SANDBOX/status-before.txt"
contains "予算超過の追記が移行対象として挙がる" "local-rules.md" "$SANDBOX/status-before.txt"
run_tako context-budget check --cwd "$ROOT" --profile big --json > "$SANDBOX/budget-before.json"
BEFORE_V=$(python3 -c "
import json;d=json.load(open('$SANDBOX/budget-before.json'))
print(sum(len(i.get('violations',[])) for i in d['items'] if i['kind']=='system_prompt'))")
check "分割前は system_prompt が予算超過" "$BEFORE_V" "1"

echo "== 2) tako migrate run =="
run_tako migrate run > "$SANDBOX/run1.txt"
contains "移行が実施された" "local-rules.md" "$SANDBOX/run1.txt"
if [ -f "$HOME/local-rules.md.pre-v1.bak" ]; then ok "旧ファイルが .pre-v1.bak へ退避されている"; else ng "退避が無い"; fi
if diff -q "$SANDBOX/original.md" "$HOME/local-rules.md.pre-v1.bak" >/dev/null; then ok "退避の中身が移行前と同一"; else ng "退避の中身が違う"; fi
contains "区切りが入った" "$MARKER" "$HOME/local-rules.md"

echo "== 3) 内容は 1 文字も消えていない =="
grep -vF -- "$MARKER" "$HOME/local-rules.md" > "$SANDBOX/stripped.md"
if diff -q "$SANDBOX/stripped.md" "$SANDBOX/original.md" >/dev/null; then
  ok "マーカー行を除くと元の全文と一致（連結 = 原文）"
else ng "マーカー行以外が変わっている"; fi
check "増えた行はマーカー 1 行だけ" \
  "$(( $(wc -l < "$HOME/local-rules.md") - $(wc -l < "$SANDBOX/original.md") ))" "1"

echo "== 4) 分割後は予算に収まる =="
run_tako context-budget check --cwd "$ROOT" --profile big --json > "$SANDBOX/budget-after.json"
python3 - "$SANDBOX/budget-after.json" > "$SANDBOX/after.txt" <<'PY'
import json,sys
d=json.load(open(sys.argv[1]))
for i in d['items']:
    if i['kind']=='system_prompt' and 'master' in i['path']:
        base=sum(p['bytes'] for p in i['pieces'] if not p['name'].startswith('append'))
        app=sum(p['bytes'] for p in i['pieces'] if p['name'].startswith('append'))
        print(f"violations={len(i.get('violations',[]))} total={i['bytes']} base={base} append={app}")
        print("pieces=" + ",".join(p['name'] for p in i['pieces'] if p['name'].startswith('append')))
PY
cat "$SANDBOX/after.txt" | sed 's/^/     /'
contains "分割後は system_prompt 違反 0" "violations=0" "$SANDBOX/after.txt"
contains "常時部と索引の 2 piece になっている" "append index (local-rules.md)" "$SANDBOX/after.txt"

echo "== 5) 2 回目は no-op（冪等） =="
cp "$HOME/local-rules.md" "$SANDBOX/after-first.md"
run_tako migrate run > "$SANDBOX/run2.txt"
if diff -q "$SANDBOX/after-first.md" "$HOME/local-rules.md" >/dev/null; then ok "2 回目でファイルが変わらない"; else ng "2 回目で書き換わった"; fi
if [ -f "$HOME/local-rules.md.pre-v2.bak" ]; then ng "不要な退避が増えた"; else ok "不要な退避を作らない"; fi

echo "== 6) 触ってはいけないものを触らない =="
for pair in "small:$SANDBOX/small-original.md:$HOME/small.md" \
            "marked:$SANDBOX/marked-original.md:$HOME/already-marked.md" \
            "flat:$SANDBOX/flat-original.md:$HOME/no-heading.md"; do
  name="${pair%%:*}"; rest="${pair#*:}"; before="${rest%%:*}"; after="${rest#*:}"
  if diff -q "$before" "$after" >/dev/null; then ok "$name の追記は不変"; else ng "$name の追記が書き換わった"; fi
done
check "区切り済みの追記のマーカーは 1 本のまま" "$(grep -cF -- "$MARKER" "$HOME/already-marked.md")" "1"

echo "== 7) on-demand 部は guide から全文が引ける =="
run_tako orchestrator guide local-rules --profile big > "$SANDBOX/guide.txt" 2>&1
LAST_HEADING=$(grep '^## ' "$SANDBOX/original.md" | tail -1)
contains "guide が on-demand 部の最後の節を返す" "$LAST_HEADING" "$SANDBOX/guide.txt"
run_tako orchestrator guide --profile big > "$SANDBOX/guide-list.txt" 2>&1
contains "一覧に local-rules が出る" "local-rules" "$SANDBOX/guide-list.txt"
contains "一覧に delegation が出る" "delegation" "$SANDBOX/guide-list.txt"
run_tako orchestrator guide local-rules --profile plain > "$SANDBOX/guide-plain.txt" 2>&1
contains "追記の無いプロファイルでも案内を返す" "no on-demand local rules" "$SANDBOX/guide-plain.txt"

echo "== 8) 追記の無い / インラインのプロファイルは素通り =="
for p in plain inline small; do
  run_tako context-budget check --cwd "$ROOT" --profile "$p" --json > "$SANDBOX/b-$p.json"
  V=$(python3 -c "
import json;d=json.load(open('$SANDBOX/b-$p.json'))
print(sum(len(i.get('violations',[])) for i in d['items'] if i['kind']=='system_prompt'))")
  check "$p は system_prompt 違反 0" "$V" "0"
done

echo "== 9) 分割後も常時部が超える場合は具体値つきの助言が出る =="
# 常時部だけで予算を超える人工ケース（受け入れ条件 4）
{ python3 -c "import sys; sys.stdout.write('常時部の行\n'*900)"; printf '%s\n## 詳細\n本文\n' "$MARKER"; } > "$HOME/fat-always.md"
mkprofile fat '~/fat-always.md'
run_tako context-budget check --cwd "$ROOT" --profile fat --json > "$SANDBOX/budget-fat.json"
python3 "$ROOT/scripts/lib/dump-prompt-violations.py" "$SANDBOX/budget-fat.json" > "$SANDBOX/fat.txt"
sed 's/^/     /' "$SANDBOX/fat.txt"
contains "違反に移動すべき行数が入る" "move_lines=" "$SANDBOX/fat.txt"
contains "助言が区切りの名前を出す" "$MARKER" "$SANDBOX/fat.txt"
contains "助言が guide の引き方を出す" "tako orchestrator guide local-rules" "$SANDBOX/fat.txt"
contains "提案にも同じ行数が入る" "proposal_move_lines=" "$SANDBOX/fat.txt"
# **切れないので移行は触らない**（区切りは既にあるので二度と切らない）
cp "$HOME/fat-always.md" "$SANDBOX/fat-original.md"
run_tako migrate run > /dev/null
if diff -q "$SANDBOX/fat-original.md" "$HOME/fat-always.md" >/dev/null; then ok "区切り済みの超過ファイルは移行が触らない"; else ng "触った"; fi

echo "== 10) A/B: TAKO_1477_LEGACY=1 で分割前の姿へ戻る =="
TAKO_1477_LEGACY=1 run_tako context-budget check --cwd "$ROOT" --profile big --json > "$SANDBOX/budget-legacy.json"
python3 "$ROOT/scripts/lib/dump-prompt-size.py" "$SANDBOX/budget-legacy.json" > "$SANDBOX/legacy.txt"
sed 's/^/     /' "$SANDBOX/legacy.txt"
absent "legacy では予算に収まらない" "violations=0" "$SANDBOX/legacy.txt"
LEG_APP=$(sed -n 's/.*append=\([0-9]*\).*/\1/p' "$SANDBOX/legacy.txt")
NEW_APP=$(sed -n 's/.*append=\([0-9]*\).*/\1/p' "$SANDBOX/after.txt")
if [ "${LEG_APP:-0}" -gt "${NEW_APP:-0}" ]; then ok "legacy は追記を全文載せる（${LEG_APP} > ${NEW_APP}）"; else ng "A/B が効いていない（legacy=${LEG_APP} 現行=${NEW_APP}）"; fi
TAKO_1477_LEGACY=1 run_tako orchestrator guide delegation --profile big > "$SANDBOX/guide-legacy.txt" 2>&1
ok "legacy でも guide 自体は引ける（$(wc -c < "$SANDBOX/guide-legacy.txt") bytes）"

echo
echo "==== $PASS PASS / $FAIL FAIL ===="
[ "$FAIL" -eq 0 ]
