#!/usr/bin/env bash
# #1548 の実経路テスト: **docs 生成が古い tako バイナリを黙って選ばない**。
#
# 旧実装の `takoBin()` は `target/debug/tako` → `target/release/tako` の順に
# 「存在する方」を返すだけで版を見なかった。開発ツリーで debug だけ古いと
#   - `--check` が「マトリクスと同期していません」の**偽の赤**
#   - `--check` 無しで打つと docs が数日前へ**静かに巻き戻る**
# という嘘をつく（CI はフレッシュビルドなので緑のまま、手元だけが壊れる）。
#
# ここでは本物のリポジトリを汚さずに、テンポラリへ**偽リポジトリ**（scripts/ と
# Cargo.toml と docs/ だけを持つ）を組み、tako のスタブで版を自由に振って実測する。
# 本物のバイナリは JSON の採取（fixtures）と最後のツリー実測にだけ読み取りで使う。
set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TAKO="${TAKO_BIN:-$ROOT/target/debug/tako}"
PASS=0; FAIL=0; SKIP=0

ok()   { PASS=$((PASS+1)); echo "  PASS: $1"; }
ng()   { FAIL=$((FAIL+1)); echo "  FAIL: $1"; }
skip() { SKIP=$((SKIP+1)); echo "  SKIP: $1"; }
check(){ if [ "$2" = "$3" ]; then ok "$1"; else ng "$1 (期待 '$3' / 実際 '$2')"; fi; }
contains(){ if grep -qF -- "$2" "$3" 2>/dev/null; then ok "$1"; else ng "$1 ('$2' が出ていない)"; fi; }
absent(){ if grep -qF -- "$2" "$3" 2>/dev/null; then ng "$1 ('$2' が出ている)"; else ok "$1"; fi; }

[ -x "$TAKO" ] || { echo "tako が無い: ${TAKO}（cargo build -p tako-cli）"; exit 1; }
command -v node >/dev/null 2>&1 || { echo "node が無い環境では走らない"; exit 1; }

SANDBOX="$(mktemp -d "${TMPDIR:-/tmp}/tako-1548-XXXXXX")"
cleanup(){ rm -rf "$SANDBOX"; }
trap cleanup EXIT

FAKE="$SANDBOX/repo"
FIX="$FAKE/fixtures"
DEBUG_BIN="$FAKE/target/debug/tako"
RELEASE_BIN="$FAKE/target/release/tako"
OTHER_BIN="$FAKE/other/tako"
WIN_MD="$FAKE/docs/src/content/docs/windows-support.md"
AGENT_MD="$FAKE/docs/src/content/docs/agent-support.md"
MARKER="$SANDBOX/used-explicit"
OUT="$SANDBOX/out"
mkdir -p "$FAKE/scripts/lib" "$FAKE/docs/src/content/docs" "$FAKE/target/debug" "$FAKE/target/release" "$FAKE/other" "$FIX"

cp "$ROOT/scripts/gen-windows-support-docs.mjs" "$ROOT/scripts/gen-agent-support-docs.mjs" "$FAKE/scripts/"
cp "$ROOT/scripts/lib/tako-bin.mjs" "$FAKE/scripts/lib/"

# 偽 Cargo.toml。**別の節の version を掴まないこと**も同時に測る（[package] は 0.0.1）
cat > "$FAKE/Cargo.toml" <<'TOML'
[package]
name = "dummy"
version = "0.0.1"

[workspace.package]
version = "9.9.9"
edition = "2021"
TOML

# 本物のバイナリから JSON を採取（読み取りのみ。生成内容を変えないための土台）
"$TAKO" platform --platform windows --json > "$FIX/windows.json" || { echo "platform --json の採取に失敗"; exit 1; }
"$TAKO" agent-support --json > "$FIX/agent.json" || { echo "agent-support --json の採取に失敗"; exit 1; }

# 「古いバイナリ」用に**中身の違う** JSON も作る（counts の葉を 1 つ増やす）。
# これで版の門番を外したときに **md が実際に巻き戻る**のが観測できる
mutate(){ node -e '
const fs = require("node:fs");
const j = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
const bump = (o) => {
  for (const k of Object.keys(o)) {
    if (typeof o[k] === "number") { o[k] += 1; return true; }
    if (o[k] && typeof o[k] === "object" && bump(o[k])) return true;
  }
  return false;
};
if (!bump(j.counts)) { console.error("counts に数値が無い"); process.exit(1); }
fs.writeFileSync(process.argv[2], JSON.stringify(j));
' "$1" "$2"; }
mutate "$FIX/windows.json" "$FIX/windows-old.json" || exit 1
mutate "$FIX/agent.json" "$FIX/agent-old.json" || exit 1
if cmp -s "$FIX/windows.json" "$FIX/windows-old.json"; then
  echo "古い版用の JSON が同じ内容になっている（テストの前提が崩れている）"; exit 1
fi

# tako のスタブ。版と返す JSON は**生成時に埋め込む**（置き場ごとに別の版を持たせるため）
make_stub(){ # $1=置き場 $2=版 $3=印を残すか（mark / plain） $4=fixture の接尾辞（"" / "-old"）
  local mark=""
  if [ "$3" = "mark" ]; then mark="echo used >> \"${MARKER}\";"; fi
  cat > "$1" <<STUB
#!/bin/bash
case "\$1" in
  --version) echo "tako $2" ;;
  platform) ${mark} cat "${FIX}/windows$4.json" ;;
  agent-support) ${mark} cat "${FIX}/agent$4.json" ;;
  *) echo "想定外の呼び出し: \$*" >&2; exit 2 ;;
esac
STUB
  chmod +x "$1"
}
make_stub "$DEBUG_BIN" 9.9.9 plain ""
make_stub "$OTHER_BIN" 9.9.9 mark ""

# gen スクリプトを偽リポジトリで走らせる。TAKO_BIN は既定経路の測定では必ず外す
run_win(){ ( cd "$FAKE" && env -u TAKO_BIN node scripts/gen-windows-support-docs.mjs ${GEN_ARGS:-} ) >"$OUT" 2>&1; echo $?; }
run_agent(){ ( cd "$FAKE" && env -u TAKO_BIN node scripts/gen-agent-support-docs.mjs ${GEN_ARGS:-} ) >"$OUT" 2>&1; echo $?; }
run_with_bin(){ ( cd "$FAKE" && TAKO_BIN="$1" node "scripts/gen-$2-support-docs.mjs" --check ) >"$OUT" 2>&1; echo $?; }
sum(){ shasum "$1" | awk '{print $1}'; }

echo "== A: 既定の探索で版が一致していれば通る =="
GEN_ARGS="" rc="$(run_win)"; check "windows: 生成が通る" "$rc" "0"
if [ -s "$WIN_MD" ]; then ok "windows: md が生成された"; else ng "windows: md が生成されていない"; fi
GEN_ARGS="--check" rc="$(run_win)"; check "windows: --check が緑" "$rc" "0"
contains "windows: 同期している旨が出る" "同期しています" "$OUT"
GEN_ARGS="" rc="$(run_agent)"; check "agent: 生成が通る" "$rc" "0"
GEN_ARGS="--check" rc="$(run_agent)"; check "agent: --check が緑" "$rc" "0"
contains "agent: 同期している旨が出る" "同期しています" "$OUT"

echo "== B: 古いバイナリなら偽の赤ではなく理由つきで落ちる =="
WIN_SUM="$(sum "$WIN_MD")"; AGENT_SUM="$(sum "$AGENT_MD")"
make_stub "$DEBUG_BIN" 0.0.1 plain "-old"
GEN_ARGS="--check" rc="$(run_win)"
check "windows: --check が非 0 で落ちる" "$rc" "1"
head -1 "$OUT" > "$SANDBOX/out1"
contains "windows: 1 行目で古いと分かる" "が古いバイナリです" "$SANDBOX/out1"
contains "windows: 1 行目に両方の版が出る" "（v0.0.1 ≠ Cargo.toml の v9.9.9）" "$SANDBOX/out1"
contains "windows: 1 行目に直し方が出る" "cargo build -p tako-cli" "$SANDBOX/out1"
contains "windows: どのバイナリかが出る" "target/debug/tako" "$SANDBOX/out1"
absent "windows: 偽の赤（同期していません）を出さない" "同期していません" "$OUT"
absent "windows: スタックトレースを出さない" "    at " "$OUT"
contains "windows: 明示指定の口を案内する" "TAKO_BIN" "$OUT"
GEN_ARGS="" rc="$(run_win)"
check "windows: --check 無しでも落ちる" "$rc" "1"
check "windows: md を巻き戻さない（古い版は別内容を返す）" "$(sum "$WIN_MD")" "$WIN_SUM"
GEN_ARGS="--check" rc="$(run_agent)"
check "agent: --check が非 0 で落ちる" "$rc" "1"
contains "agent: 理由が読める" "が古いバイナリです" "$OUT"
absent "agent: 偽の赤（同期していません）を出さない" "同期していません" "$OUT"
GEN_ARGS="" rc="$(run_agent)"
check "agent: --check 無しでも落ちる" "$rc" "1"
check "agent: md を巻き戻さない（古い版は別内容を返す）" "$(sum "$AGENT_MD")" "$AGENT_SUM"

echo "== C: TAKO_BIN での明示指定 =="
rm -f "$MARKER"
rc="$(run_with_bin "$OTHER_BIN" windows)"
check "絶対パスの TAKO_BIN が通る（既定の debug は古いまま）" "$rc" "0"
if [ -f "$MARKER" ]; then ok "指定したバイナリが実際に使われた"; else ng "指定したバイナリが使われていない"; fi
rm -f "$MARKER"
# **リポジトリの外を cwd にして**打つ。cwd 基準で解決していたらここで落ちる
rc="$( ( cd "$SANDBOX" && TAKO_BIN="other/tako" node "$FAKE/scripts/gen-windows-support-docs.mjs" --check ) >"$OUT" 2>&1; echo $? )"
check "相対パスの TAKO_BIN が通る（cwd ではなくリポジトリルート基準）" "$rc" "0"
if [ -f "$MARKER" ]; then ok "相対指定でもそのバイナリが使われた"; else ng "相対指定が効いていない"; fi
rc="$(run_with_bin "$OTHER_BIN" agent)"
check "agent 側でも TAKO_BIN が効く" "$rc" "0"
make_stub "$OTHER_BIN" 0.0.2 mark "-old"
rc="$(run_with_bin "$OTHER_BIN" windows)"
check "明示しても古ければ落ちる" "$rc" "1"
contains "明示だったことが分かる" "TAKO_BIN で明示されたパスです" "$OUT"
make_stub "$OTHER_BIN" 9.9.9 mark ""
rc="$(run_with_bin "$FAKE/nope/tako" windows)"
check "存在しない TAKO_BIN は落ちる" "$rc" "1"
contains "見つからない理由が読める" "TAKO_BIN が指す tako が見つかりません" "$OUT"

echo "== D: release へのフォールバックと、黙って別を選ばないこと =="
rm -f "$DEBUG_BIN"
make_stub "$RELEASE_BIN" 9.9.9 plain ""
GEN_ARGS="--check" rc="$(run_win)"; check "debug が無ければ release を使う" "$rc" "0"
# debug が古い / release が現行 = 共有ツリーで実際に起きていた形。黙って release へ逃げない
make_stub "$DEBUG_BIN" 0.0.1 plain "-old"
GEN_ARGS="--check" rc="$(run_win)"
check "debug が古ければ release があっても落ちる" "$rc" "1"
contains "落ちる理由は debug が古いこと" "target/debug/tako" "$OUT"
rm -f "$RELEASE_BIN"

echo "== E: バイナリがどこにも無いとき =="
rm -f "$DEBUG_BIN"
GEN_ARGS="--check" rc="$(run_win)"; check "見つからなければ落ちる" "$rc" "1"
contains "作り方が 1 行で出る" "cargo build -p tako-cli" "$OUT"
absent "無いのに「同期していません」とは言わない" "同期していません" "$OUT"

echo "== F: 実装が 1 か所であること（番犬） =="
for f in scripts/gen-windows-support-docs.mjs scripts/gen-agent-support-docs.mjs; do
  if grep -q "from './lib/tako-bin.mjs'" "$ROOT/${f}"; then ok "${f} が共有モジュールを使っている"; else ng "${f} が共有モジュールを使っていない"; fi
  if grep -q "function takoBin" "$ROOT/${f}"; then ng "${f} に takoBin() の複製が残っている"; else ok "${f} に takoBin() の複製が無い"; fi
done
DUP="$(grep -rl "target/debug/tako" "$ROOT/scripts" --include='*.mjs' | grep -vc 'lib/tako-bin.mjs')"
check "探索順を直書きする .mjs は共有モジュールだけ" "$DUP" "0"

echo "== G: 本物のツリーでの実測 =="
REAL_VER="$("$TAKO" --version 2>/dev/null | head -1 | awk '{print $2}')"
WANT_VER="$(awk '/^\[workspace.package\]/{f=1;next} /^\[/{f=0} f && /^version *=/{gsub(/[",]/,"");print $3;exit}' "$ROOT/Cargo.toml")"
if [ "$REAL_VER" = "$WANT_VER" ]; then
  rc="$( ( cd "$ROOT" && env -u TAKO_BIN node scripts/gen-windows-support-docs.mjs --check ) >"$OUT" 2>&1; echo $? )"
  check "本物のツリーで windows --check が緑" "$rc" "0"
  rc="$( ( cd "$ROOT" && env -u TAKO_BIN node scripts/gen-agent-support-docs.mjs --check ) >"$OUT" 2>&1; echo $? )"
  check "本物のツリーで agent --check が緑" "$rc" "0"
  if cmp -s "$WIN_MD" "$ROOT/docs/src/content/docs/windows-support.md"; then ok "偽リポジトリの生成物が本物と 1 バイト同じ"; else ng "生成内容が本物と食い違う"; fi
  if cmp -s "$AGENT_MD" "$ROOT/docs/src/content/docs/agent-support.md"; then ok "agent 側も本物と 1 バイト同じ"; else ng "agent の生成内容が本物と食い違う"; fi
else
  skip "本物の tako が v${REAL_VER}（Cargo.toml は v${WANT_VER}）なのでツリー実測は飛ばす"
fi

echo
echo "PASS=${PASS} FAIL=${FAIL} SKIP=${SKIP}"
[ "$FAIL" -eq 0 ]
