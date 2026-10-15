#!/usr/bin/env bash
# test-runner-runtime-1735.sh — Code Runner の実行環境の表の Node / Rust の行の実経路テスト（#1735 / FR-3.18.3）
#
# 隔離した data / tmux / HOME で**実 tako-app** を立て、
#
#   ① `.nvmrc`（`20`）のあるプロジェクトで `tako run a.js` が nvm の v20 の node で走る
#      （一時 HOME の偽の `~/.nvm/versions/node/v<版>/bin/node` = 版を名乗るだけのスクリプト。
#      `v18` / `v20.9.0` / `v20.11.1` を並べ、前方一致の一番新しい v20.11.1 が選ばれる）。
#      `node_modules/.bin` が PATH の先頭、その次が版の bin
#   ② `package.json` のあるプロジェクト（`node index.js` = 名前で呼ぶ）でも PATH の前置で同じ版が走る
#   ③ `tako run a.js --list` と MCP `tako_run_resolve` の `runtimes` に node の候補が出て、字面まで一致する
#   ④ Rust: `rust-toolchain.toml` のあるプロジェクトで `cargo run`（プロジェクト種別 cargo）と
#      単体の `rustc`（拡張子既定）の実行時に `RUSTUP_TOOLCHAIN` が届く（偽の cargo / rustc が env を書き出す。
#      toolchain の実体は無くてよい = 本物の rustup に導入させない）
#   ⑤ 境界: `.nvmrc` が別名（`lts/*`）/ 指す版が入っていない（`16`）→ 候補に出さず理由を出す /
#      `.nvmrc` と `.node-version` が両方 → `.nvmrc` / `.ts` も同じ版（`npx tsx` の PATH）
#   ⑥ 本物の fnm（この PC にあれば。**読み取りだけ**）: `.node-version` の版を fnm の置き場から選び、
#      その node で走る
#   ⑦ A/B: 修正前のバイナリ（既定は `/Applications/tako.app` = main から入れたもの。`OLD_APP_BIN` /
#      `OLD_TAKO_BIN` で差し替え）では ① の候補に node が出ず、① の判定が FAILED・④ の env が届かない
#
# を実測する。
#
# **本番の tako / 設定・本物の nvm / fnm / rustup には一切書き込まない**（data / HOME は mktemp 配下、
# tmux は専用ソケット、落とすのは自分で起こした pid だけ。node の版は入れない）。
# 窓は常設の仮想ディスプレイ（tako-vd）へ出る（#1141）。
#
# 使い方: bash scripts/test-runner-runtime-1735.sh
#
# **CI には載せない**（実 GUI + 仮想ディスプレイが要る）。手元で走らせる実経路テスト。
set -uo pipefail

# **本番 GUI を指す env を最初に落とす**（#1449 / #1450 で本番にペインが漏れた実例が 2 件）
unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_ORCHESTRATOR_ROLE

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PASS=0
FAIL=0

pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
fail() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }
check_eq() {
  if [ "$2" = "$3" ]; then pass "$1"; else fail "$1（期待 '${2}' / 実際 '${3}'）"; fi
}

command -v python3 >/dev/null 2>&1 || { echo "python3 が無いので走らせられない（未実測）"; exit 1; }

TMP="$(mktemp -d /tmp/tako-1735-XXXXXX)"
# 比較は実体パスで行う（macOS の /tmp は /private/tmp へのリンク。dispatch は canonicalize する）
TMP="$(cd "$TMP" && pwd -P)"
APP_PID=""
TMUX_SOCKET="tako-1735-$$"
cleanup() {
  stop_isolated_gui "$APP_PID"
  tmux -L "$TMUX_SOCKET" kill-server >/dev/null 2>&1 || true
  rm -rf "$TMP"
}
trap cleanup EXIT

# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
isolated_gui_bins || exit 1
NEW_APP_BIN="$APP_BIN"
NEW_TAKO_BIN="$TAKO_BIN"
OLD_APP_BIN="${OLD_APP_BIN:-/Applications/tako.app/Contents/MacOS/tako-app}"
OLD_TAKO_BIN="${OLD_TAKO_BIN:-/Applications/tako.app/Contents/MacOS/tako}"

# 本物の fnm の置き場（HOME を差し替える前に見る。読み取りだけ）
REAL_FNM_DIR="${FNM_DIR:-$HOME/.local/share/fnm}"
REAL_FNM_VER=""
if [ -d "$REAL_FNM_DIR/node-versions" ]; then
  REAL_FNM_VER="$(ls "$REAL_FNM_DIR/node-versions" 2>/dev/null | grep '^v[0-9]' | tail -1)"
fi
[ -n "$REAL_FNM_VER" ] && [ -x "$REAL_FNM_DIR/node-versions/$REAL_FNM_VER/installation/bin/node" ] \
  || REAL_FNM_VER=""

# --- 隔離した環境 -------------------------------------------------------------
export HOME="$TMP/home"
mkdir -p "$HOME"
export TAKO_ISOLATED=1
export TAKO_DATA_DIR="$TMP/data"
export TAKO_DISCOVERY_DIR="$TMP/disc"
export TAKO_ORCHESTRATOR_DIR="$TMP/orch"
export TAKO_SESSIONS_FILE="$TMP/sessions.yaml"
export TAKO_PANE_LOG_DIR="$TMP/panelogs"
export TAKO_WORKERS_FILE="$TMP/workers.yaml"
export TAKO_TMUX_SOCKET="$TMUX_SOCKET"
# 実機の道具の置き場を指す env は GUI へ渡さない（隔離 HOME の偽物だけを見せる）
unset NVM_DIR NVM_HOME FNM_DIR XDG_CONFIG_HOME XDG_DATA_HOME RUSTUP_TOOLCHAIN
# 万一 rustup が呼ばれても toolchain を落としてこない
export RUSTUP_AUTO_INSTALL=0
mkdir -p "$TAKO_DATA_DIR" "$TAKO_DISCOVERY_DIR" "$TAKO_ORCHESTRATOR_DIR"
for d in "$HOME" "$TAKO_DATA_DIR" "$TAKO_ORCHESTRATOR_DIR"; do
  case "$d" in
    "$TMP"/*) : ;;
    *) echo "隔離されていないディレクトリ: $d"; exit 1 ;;
  esac
done

# 偽の nvm: 版を名乗り、自分のパスと PATH を書き出すだけの node
mk_fake_node() { # mk_fake_node <版>
  local dir="$HOME/.nvm/versions/node/$1/bin"
  mkdir -p "$dir"
  cat > "$dir/node" <<SH
#!/bin/sh
if [ "\${1-}" = "--version" ]; then echo "$1"; exit 0; fi
out="\${1:-noarg}.node-out"
{ echo "version=$1"; echo "self=\$0"; echo "path=\$PATH"; } > "\$out"
echo "FAKE_NODE=$1"
SH
  chmod +x "$dir/node"
}
mk_fake_node v18.20.4
mk_fake_node v20.9.0
mk_fake_node v20.11.1

# 偽の cargo / rustc: 実行時の RUSTUP_TOOLCHAIN を書き出す（本物の rustup を通さない）
STUBS="$TMP/stubs"
mkdir -p "$STUBS"
cat > "$STUBS/cargo" <<'SH'
#!/bin/sh
{ echo "args=$*"; echo "toolchain=${RUSTUP_TOOLCHAIN-<unset>}"; } > "$PWD/cargo.out"
echo "FAKE_CARGO toolchain=${RUSTUP_TOOLCHAIN-<unset>}"
SH
cat > "$STUBS/rustc" <<'SH'
#!/bin/sh
o=""; prev=""
for a in "$@"; do [ "$prev" = "-o" ] && o="$a"; prev="$a"; done
{ echo "args=$*"; echo "toolchain=${RUSTUP_TOOLCHAIN-<unset>}"; } > "$PWD/rustc.out"
if [ -n "$o" ]; then printf '#!/bin/sh\necho built-ran\n' > "$o"; chmod +x "$o"; fi
SH
chmod +x "$STUBS/cargo" "$STUBS/rustc"
export PATH="$STUBS:$PATH"

# --- 材料 ---------------------------------------------------------------------
# 探索は `.git` の段で止まるので、材料の根に置いて /tmp の置き忘れを拾わないようにする
FIX="$TMP/fixtures"
mkdir -p "$FIX/.git"
# 本物の node でも偽の node でも同じ形（`<ファイル>.node-out`）を書き出す
JS='const fs = require("fs");
fs.writeFileSync(__filename + ".node-out",
  "version=" + process.version + "\nself=" + process.execPath + "\npath=" + process.env.PATH + "\n");
console.log("REAL_NODE=" + process.version);
'
mk_js() { mkdir -p "$(dirname "$1")"; printf '%s' "$JS" > "$1"; }

# ① .nvmrc（20）+ node_modules/.bin
mk_js "$FIX/nvmproj/src/a.js"
printf '20\n' > "$FIX/nvmproj/.nvmrc"
mkdir -p "$FIX/nvmproj/node_modules/.bin"
cp "$FIX/nvmproj/src/a.js" "$FIX/nvmproj/src/a.ts"
# ② package.json（プロジェクト種別 npm = `node index.js` を名前で呼ぶ）
mk_js "$FIX/npmproj/index.js"
printf '{ "name": "npmproj1735", "version": "0.0.0" }\n' > "$FIX/npmproj/package.json"
printf 'v20\n' > "$FIX/npmproj/.nvmrc"
mkdir -p "$FIX/npmproj/node_modules/.bin"
# ⑤ 境界
mk_js "$FIX/alias/a.js"
printf 'lts/*\n' > "$FIX/alias/.nvmrc"
mk_js "$FIX/missing/a.js"
printf '16\n' > "$FIX/missing/.nvmrc"
mk_js "$FIX/both/a.js"
printf '18\n' > "$FIX/both/.nvmrc"
printf '20\n' > "$FIX/both/.node-version"
# ④ Rust
mkdir -p "$FIX/cargoproj/src"
printf '[package]\nname = "cargoproj1735"\nversion = "0.1.0"\nedition = "2021"\n' > "$FIX/cargoproj/Cargo.toml"
printf 'fn main() { println!("hi"); }\n' > "$FIX/cargoproj/src/main.rs"
printf '[toolchain]\nchannel = "1.79.0-fake1735"\n' > "$FIX/cargoproj/rust-toolchain.toml"
mkdir -p "$FIX/loosers"
printf 'fn main() { println!("hi"); }\n' > "$FIX/loosers/a.rs"
printf 'nightly-fake1735\n' > "$FIX/loosers/rust-toolchain"
# ⑥ 本物の fnm
if [ -n "$REAL_FNM_VER" ]; then
  mk_js "$FIX/fnmproj/a.js"
  printf '%s\n' "${REAL_FNM_VER#v}" > "$FIX/fnmproj/.node-version"
fi

PY_LIST='import json,sys
try:
    d = json.load(sys.stdin)
except Exception as e:
    print("__NOT_JSON__=%s" % e); sys.exit(0)
rt = d.get("runtime") or {}
print("manager=%s" % rt.get("manager"))
print("key=%s" % rt.get("key"))
print("program=%s" % " ".join(rt.get("program") or []))
print("path_prepend=%s" % ":".join(rt.get("path_prepend") or []))
print("env=%s" % json.dumps(rt.get("env") or {}, sort_keys=True))
print("command=%s" % (d.get("profiles") or [{}])[0].get("command"))
print("warnings=%s" % " | ".join(d.get("warnings") or []))
rts = d.get("runtimes") or []
print("ids=%s" % " ".join(r.get("id", "") for r in rts))
print("auto_ids=%s" % " ".join(r.get("id", "") for r in rts if r.get("auto")))
print("kinds=%s" % " ".join(sorted(set(r.get("kind", "") for r in rts))))'
field() { printf '%s\n' "$2" | sed -n "s/^${1}=//p" | head -1; }
oneline() { printf '%s' "$1" | tr '\n' ' '; }

start_gui() { # start_gui <ログ> [VAR=VAL…]
  local log="$1"; shift
  launch_isolated_gui "$log" ${1+"$@"} || exit $?
  APP_PID="$ISOLATED_GUI_PID"
  wait_isolated_gui "$log" || return 1
  ROOT="$("$TAKO_BIN" list 2>/dev/null | python3 -c 'import json,sys
d = json.load(sys.stdin)
print(d["tabs"][0]["panes"][0]["id"])')"
  [ -n "$ROOT" ] || { echo "ルートペインを採れない"; return 1; }
  echo "  pid=$APP_PID root pane=$ROOT"
}

list() { # list <file> → 一覧の要約（1 行 1 項目）
  "$TAKO_BIN" run "$@" --list --pane "$ROOT" 2>&1 | python3 -c "$PY_LIST"
}

# run_file <file> <待つ印のファイル> → 実行ペインの終了状態（`exited <コード>`）。印のファイルが
# 書かれるか終わるまで待つ
run_file() {
  local out pane st status
  rm -f "$2"
  out="$("$TAKO_BIN" run "$1" --pane "$ROOT" --auto-close success 2>&1)"
  pane="$(printf '%s' "$out" | python3 -c 'import json,sys
try:
    print(json.load(sys.stdin)["pane"])
except Exception:
    pass' 2>/dev/null)"
  [ -n "$pane" ] || { echo "__NO_PANE__ $(oneline "$out")"; return; }
  status=""
  for _ in $(seq 1 60); do
    st="$("$TAKO_BIN" run-interactive-status "$pane" 2>&1)"
    status="$(printf '%s' "$st" | python3 -c 'import json,sys
try:
    d = json.load(sys.stdin); print("%s %s" % (d.get("status"), d.get("exit_code")))
except Exception:
    pass' 2>/dev/null)"
    case "$status" in exited*) break ;; esac
    sleep 1
  done
  echo "$status"
}
kv() { sed -n "s/^${1}=//p" "$2" 2>/dev/null | head -1; }

# ① の判定（A/B で同じ関数を使う）: nvm の v20.11.1 で走ったか
judge_nvm() { # judge_nvm <書き出し> → OK / FAILED
  if [ "$(kv version "$1")" = "v20.11.1" ] \
    && [ "$(kv self "$1")" = "$HOME/.nvm/versions/node/v20.11.1/bin/node" ]; then
    echo OK
  else
    echo FAILED
  fi
}

echo "== 隔離 GUI を起こす（この変更のバイナリ） =="
start_gui "$TMP/app.log" ${REAL_FNM_VER:+"FNM_DIR=$REAL_FNM_DIR"} || exit 1

echo
echo "== ① .nvmrc（20）のあるプロジェクトで tako run a.js =="
L1="$(list "$FIX/nvmproj/src/a.js")"
echo "  list: manager=$(field manager "$L1") key=$(field key "$L1") command=$(field command "$L1")"
echo "  候補: $(field ids "$L1")"
check_eq "一覧の runtime は nvm の v20.11.1（前方一致の一番新しい版）" "nvm v20.11.1" \
  "$(field manager "$L1") $(field key "$L1")"
# 本物の fnm（v20 を持たない）が同じ .nvmrc を読んでも「fnm 20 が入っていない」と騒がない
check_eq "① nvm が版を見つけたので他の道具の不足は知らせない" "" "$(field warnings "$L1")"
OUT1="$FIX/nvmproj/src/a.js.node-out"
S1="$(run_file "$FIX/nvmproj/src/a.js" "$OUT1")"
echo "  実行: $S1 / $(kv version "$OUT1") / self=$(kv self "$OUT1")"
check_eq "① nvm の v20.11.1 の node で走る（偽の node が名乗る版）" "OK" "$(judge_nvm "$OUT1")"
check_eq "① 終了コード 0" "exited 0" "$S1"
P1="$(kv path "$OUT1")"
check_eq "① PATH の先頭は node_modules/.bin" "$FIX/nvmproj/node_modules/.bin" "${P1%%:*}"
P1_REST="${P1#*:}"
check_eq "① その次は版の bin" "$HOME/.nvm/versions/node/v20.11.1/bin" "${P1_REST%%:*}"

echo
echo "== ② package.json のあるプロジェクト（node を名前で呼ぶ）も PATH の前置で同じ版 =="
L2="$(list "$FIX/npmproj/index.js")"
echo "  list: manager=$(field manager "$L2") key=$(field key "$L2") command=$(field command "$L2")"
OUT2="$FIX/npmproj/index.js.node-out"
S2="$(run_file "$FIX/npmproj/index.js" "$OUT2")"
echo "  実行: $S2 / $(kv version "$OUT2") / self=$(kv self "$OUT2")"
check_eq "② v20 → nvm の v20.11.1" "v20.11.1" "$(kv version "$OUT2")"
P2="$(kv path "$OUT2")"
check_eq "② PATH の先頭は node_modules/.bin" "$FIX/npmproj/node_modules/.bin" "${P2%%:*}"

echo
echo "== ③ CLI --list と MCP tako_run_resolve の runtimes に node の候補が出て字面まで一致する =="
check_eq "③ 候補の kind は node" "node" "$(field kinds "$L1")"
case " $(field ids "$L1") " in
  *" nvm:v20.11.1 "*" system:node "*) pass "③ nvm の版と PATH の node が並ぶ" ;;
  *) fail "③ 候補が足りない: $(field ids "$L1")" ;;
esac
check_eq "③ 自動で選ぶのは 1 つ" "nvm:v20.11.1" "$(field auto_ids "$L1")"
cat > "$TMP/mcp-in.jsonl" <<JSONL
{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test-1735","version":"0"}}}
{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"tako_run_resolve","arguments":{"path":"$FIX/nvmproj/src/a.js","pane":$ROOT}}}
JSONL
MCP_SOCKET="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["socket"])' "$TAKO_DISCOVERY_DIR/control.json")"
MCP_TOKEN="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["token"])' "$TAKO_DISCOVERY_DIR/control.json")"
env TAKO_SOCKET="$MCP_SOCKET" TAKO_TOKEN="$MCP_TOKEN" \
  "$TAKO_BIN" mcp serve < "$TMP/mcp-in.jsonl" > "$TMP/mcp-out.jsonl" 2> "$TMP/mcp-err.log"
PY_MCP='import json,sys
for line in open(sys.argv[1]):
    line = line.strip()
    if not line:
        continue
    try:
        msg = json.loads(line)
    except Exception:
        continue
    if msg.get("id") != 2:
        continue
    content = (msg.get("result") or {}).get("content") or []
    print(content[0].get("text") if content else "__NO_CONTENT__")
    sys.exit(0)
print("__NO_RESPONSE__")'
MCP_TEXT="$(python3 -c "$PY_MCP" "$TMP/mcp-out.jsonl")"
CLI_TEXT="$("$TAKO_BIN" run "$FIX/nvmproj/src/a.js" --list --pane "$ROOT" 2>&1)"
NORM_RT='import json,sys; print(json.dumps(json.loads(sys.stdin.read())["runtimes"], sort_keys=True, ensure_ascii=False))'
NORM_ALL='import json,sys; d = json.loads(sys.stdin.read()); d.pop("probe", None); print(json.dumps(d, sort_keys=True, ensure_ascii=False))'
CLI_RT="$(printf '%s' "$CLI_TEXT" | python3 -c "$NORM_RT" 2>/dev/null)"
MCP_RT="$(printf '%s' "$MCP_TEXT" | python3 -c "$NORM_RT" 2>/dev/null)"
echo "  CLI runtimes: $CLI_RT"
[ -n "$MCP_RT" ] || echo "  (MCP の生応答) $(oneline "$MCP_TEXT" | head -c 300)"
[ -n "$CLI_RT" ] && [ "$CLI_RT" != "[]" ] || fail "CLI の runtimes が空か JSON でない: $(oneline "$CLI_TEXT" | head -c 300)"
check_eq "③ runtimes が字面まで一致する" "$CLI_RT" "$MCP_RT"
check_eq "③ probe 以外の応答全体も一致する" \
  "$(printf '%s' "$CLI_TEXT" | python3 -c "$NORM_ALL" 2>/dev/null)" \
  "$(printf '%s' "$MCP_TEXT" | python3 -c "$NORM_ALL" 2>/dev/null)"

echo
echo "== ④ Rust: rust-toolchain の値が実行時の RUSTUP_TOOLCHAIN へ届く =="
L4="$(list "$FIX/cargoproj/src/main.rs")"
echo "  list: manager=$(field manager "$L4") key=$(field key "$L4") env=$(field env "$L4") command=$(field command "$L4")"
check_eq "④ 一覧の runtime は rustup の 1.79.0-fake1735" "rustup 1.79.0-fake1735" \
  "$(field manager "$L4") $(field key "$L4")"
S4="$(run_file "$FIX/cargoproj/src/main.rs" "$FIX/cargoproj/cargo.out")"
echo "  cargo の実行: $S4 / $(kv args "$FIX/cargoproj/cargo.out") / toolchain=$(kv toolchain "$FIX/cargoproj/cargo.out")"
check_eq "④ cargo run に RUSTUP_TOOLCHAIN が届く" "1.79.0-fake1735" "$(kv toolchain "$FIX/cargoproj/cargo.out")"
S4b="$(run_file "$FIX/loosers/a.rs" "$FIX/loosers/rustc.out")"
echo "  rustc の実行: $S4b / $(kv args "$FIX/loosers/rustc.out") / toolchain=$(kv toolchain "$FIX/loosers/rustc.out")"
check_eq "④ 単体の rustc（rust-toolchain の素の 1 行）にも届く" "nightly-fake1735" \
  "$(kv toolchain "$FIX/loosers/rustc.out")"
check_eq "④ 単体の rustc の後の実行も走る" "exited 0" "$S4b"

echo
echo "== ⑤ 境界 =="
LA="$(list "$FIX/alias/a.js")"
echo "  lts/*: manager=$(field manager "$LA") ids=$(field ids "$LA") warnings=$(field warnings "$LA")"
check_eq "⑤ 別名は自動で選ばずシステムへ" "system" "$(field manager "$LA")"
case "$(field warnings "$LA")" in
  *"lts/*"*) pass "⑤ 別名を解かない理由が warnings に載る" ;;
  *) fail "⑤ warnings に別名の理由が無い" ;;
esac
LM="$(list "$FIX/missing/a.js")"
echo "  16: manager=$(field manager "$LM") ids=$(field ids "$LM") warnings=$(field warnings "$LM")"
check_eq "⑤ 入っていない版は自動で選ばずシステムへ" "system" "$(field manager "$LM")"
case " $(field ids "$LM") " in
  *"v16"*) fail "⑤ 入っていない版が候補に出た" ;;
  *) pass "⑤ 入っていない版は候補に出ない" ;;
esac
case "$(field warnings "$LM")" in
  *16*) pass "⑤ 入っていない理由が warnings に載る" ;;
  *) fail "⑤ warnings に入っていない理由が無い" ;;
esac
LB="$(list "$FIX/both/a.js")"
check_eq "⑤ .nvmrc（18）と .node-version（20）が両方 → .nvmrc" "nvm v18.20.4" \
  "$(field manager "$LB") $(field key "$LB")"
LT="$(list "$FIX/nvmproj/src/a.ts")"
echo "  .ts: manager=$(field manager "$LT") command=$(field command "$LT") path_prepend=$(field path_prepend "$LT")"
check_eq "⑤ .ts も同じ版（npx tsx は PATH で効く）" "nvm v20.11.1" "$(field manager "$LT") $(field key "$LT")"
check_eq "⑤ .ts の PATH の先頭も node_modules/.bin" "$FIX/nvmproj/node_modules/.bin" \
  "$(field path_prepend "$LT" | cut -d: -f1)"

echo
echo "== ⑥ 本物の fnm（読み取りだけ） =="
if [ -n "$REAL_FNM_VER" ]; then
  LF="$(list "$FIX/fnmproj/a.js")"
  echo "  list: manager=$(field manager "$LF") key=$(field key "$LF")"
  check_eq "⑥ fnm の ${REAL_FNM_VER} を選ぶ" "fnm ${REAL_FNM_VER}" "$(field manager "$LF") $(field key "$LF")"
  OUTF="$FIX/fnmproj/a.js.node-out"
  SF="$(run_file "$FIX/fnmproj/a.js" "$OUTF")"
  echo "  実行: $SF / $(kv version "$OUTF")"
  check_eq "⑥ 本物の node（fnm の版）で走る" "$REAL_FNM_VER" "$(kv version "$OUTF")"
else
  echo "  (この PC に fnm の版が無いので未実測)"
fi

echo
echo "== ⑦ A/B: 修正前のバイナリで立て直す =="
stop_isolated_gui "$APP_PID"
APP_PID=""
if [ -x "$OLD_APP_BIN" ] && [ -x "$OLD_TAKO_BIN" ]; then
  APP_BIN="$OLD_APP_BIN"
  TAKO_BIN="$OLD_TAKO_BIN"
  echo "  修正前: $("$TAKO_BIN" --version 2>&1 | head -1)"
  start_gui "$TMP/app-old.log" ${REAL_FNM_VER:+"FNM_DIR=$REAL_FNM_DIR"} || exit 1
  LO="$(list "$FIX/nvmproj/src/a.js")"
  echo "  修正前の list: manager=$(field manager "$LO") ids='$(field ids "$LO")'"
  check_eq "⑦ 修正前は候補に node が出ない" "" "$(field ids "$LO")"
  SO="$(run_file "$FIX/nvmproj/src/a.js" "$OUT1")"
  echo "  修正前の実行: $SO / $(kv version "$OUT1") / self=$(kv self "$OUT1")"
  check_eq "⑦ 修正前は ① の判定が FAILED（PATH の node へ戻る）" "FAILED" "$(judge_nvm "$OUT1")"
  SOR="$(run_file "$FIX/cargoproj/src/main.rs" "$FIX/cargoproj/cargo.out")"
  check_eq "⑦ 修正前は cargo run に RUSTUP_TOOLCHAIN が届かない" "<unset>" \
    "$(kv toolchain "$FIX/cargoproj/cargo.out")"
  APP_BIN="$NEW_APP_BIN"
  TAKO_BIN="$NEW_TAKO_BIN"
else
  echo "  (修正前のバイナリが無いので A/B は未実測: $OLD_APP_BIN)"
fi

echo
echo "== 結果: ${PASS} PASS / ${FAIL} FAIL =="
[ "$FAIL" -eq 0 ]
