#!/bin/bash
# test-lsp-target-dir-2035.sh — rust-analyzer の初回の check と利用者の cargo build が target のロックを
# 取り合わないこと（#2035）の実経路テスト（隔離 GUI・実の rust-analyzer）
#
# 何を確かめるか:
#   ① 新腕（既定 = initializationOptions の cargo.targetDir = true）: 編集モードで .rs を開くと rust-analyzer が
#      起き、初回の build script を走らせる。その最中（注入: build.rs が hold-secs の秒数だけ眠る = 負荷を
#      かけずにロックを長く握らせる）に利用者が同じプロジェクトで `cargo build` → stderr に
#      `Blocking waiting for file lock` が出ず、待たずに終わる。rust-analyzer の build script は
#      target/rust-analyzer の下で走った（build.rs が OUT_DIR を記録する）
#   ② 旧腕（TAKO_2035_LEGACY=1 = 初期化オプションを送らない）: 同じ手順で `Blocking waiting for file lock`
#      が出て、rust-analyzer の build script が終わるまで待たされる（検出力の実証）
#   ③ 上書き: `tako lsp options --name rust-analyzer --set cargo.targetDir=false` → 動いているサーバは
#      current=false → `tako lsp restart` で status の init_options が false・current=true → null で既定へ戻る
#   ④ 境界: `--set` に --name が無い・キーの形が崩れている・知らないサーバ名は断る（終了コード非 0）
#   ⑤ tako 自身のワークスペース（`TAKO_2035_MEASURE_TAKO=1` のときだけ。重い = heavy.sh の枠で回す）:
#      新腕で rust-analyzer の初回の check の所要と target/rust-analyzer の大きさ（build script・proc-macro の
#      重複ぶんの内訳つき）を測り、その最中に `cargo build -p tako-core` を撃ってロック待ちが出ないこと
#
# 使い方: bash scripts/test-lsp-target-dir-2035.sh
#   ⑤ は ~/dev/.tako-tools/heavy.sh 経由で TAKO_2035_MEASURE_TAKO=1
#   （tako / tako-app は isolated_gui_bins が毎回今の木から建てる。最新なら cargo は何もしない = #1643）
#
# 窓は仮想ディスプレイ tako-vd へ出す（`scripts/lib/isolated-gui.sh` の 1 実装。#1141 / #1490）。
# 落とすのは自分で起こした pid だけ。
set -uo pipefail

unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_ORCHESTRATOR_ROLE

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PASS=0
FAIL=0
pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
fail() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }
check_eq() {
  if [ "$2" = "$3" ]; then pass "$1"; else fail "$1（期待 '${2}' / 実際 '${3}'）"; fi
}

# 実の rust-analyzer / cargo は HOME を差し替える前に控える。rustup の proxy は一時ディレクトリで既定の
# toolchain を引いて見失うので、リポジトリの toolchain の実体を名指す（#2034 と同じ）
REAL_RA="$(cd "$REPO_ROOT" && rustup which rust-analyzer 2>/dev/null || true)"
REAL_RUSTUP_HOME="${RUSTUP_HOME:-$(rustup show home 2>/dev/null || true)}"
REAL_CARGO_HOME="${CARGO_HOME:-$HOME/.cargo}"
REAL_CARGO="$(cd "$REPO_ROOT" && rustup which cargo 2>/dev/null || true)"
if [ -z "$REAL_RA" ] || [ -z "$REAL_CARGO" ]; then
  echo "未実測: rust-analyzer / cargo の実体が見つからない（rustup component add rust-analyzer）"
  exit 4
fi

TMP="$(mktemp -d /tmp/tako-2035-XXXXXX)"
APP_PID=""
TMUX_SOCKET="tako-2035-$$"
cleanup() {
  stop_isolated_gui "$APP_PID"
  stop_isolated_gui "${ISOLATED_GUI_PID:-}"
  tmux -L "$TMUX_SOCKET" kill-server >/dev/null 2>&1 || true
  rm -rf "$TMP"
}
trap cleanup EXIT

. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
# 隔離 GUI は言語サーバを自動では起こさない（#2048）。この検証は起こす前提なので明示する
export TAKO_LSP_AUTOSTART=1

isolated_gui_bins || exit 1

export HOME="$TMP/home"
export TAKO_ISOLATED=1
export TAKO_DATA_DIR="$TMP/data"
export TAKO_DISCOVERY_DIR="$TMP/disc"
export TAKO_ORCHESTRATOR_DIR="$TMP/orch"
export TAKO_SESSIONS_FILE="$TMP/sessions.yaml"
export TAKO_PANE_LOG_DIR="$TMP/panelogs"
export TAKO_WORKERS_FILE="$TMP/workers.yaml"
export TAKO_TMUX_SOCKET="$TMUX_SOCKET"
export TAKO_AUTO_RENAME=0
mkdir -p "$HOME" "$TAKO_DATA_DIR" "$TAKO_DISCOVERY_DIR" "$TAKO_ORCHESTRATOR_DIR"
for d in "$HOME" "$TAKO_DATA_DIR" "$TAKO_ORCHESTRATOR_DIR"; do
  case "$d" in
    "$TMP"/*) : ;;
    *) echo "隔離されていないディレクトリ: $d"; exit 1 ;;
  esac
done

json() { python3 -c "import json,sys; d=json.load(sys.stdin); print(eval(sys.argv[1]))" "$1" 2>/dev/null; }
status() { "$TAKO_BIN" lsp status --json 2>/dev/null; }
user_cargo() { RUSTUP_HOME="$REAL_RUSTUP_HOME" CARGO_HOME="$REAL_CARGO_HOME" "$REAL_CARGO" "$@"; }

# 状態で待つ（上限 $2 秒）。$1 = 条件を評価するシェルの式
wait_cond() {
  local cond="$1" limit="${2:-30}" i
  for i in $(seq 1 $((limit * 10))); do
    eval "$cond" && return 0
    sleep 0.1
  done
  return 1
}

ra_state() { status | json '[s["state"] for s in d["servers"] if s["id"] == "rust-analyzer"][0]'; }

# 編集モードで開く（$1 = ファイル）→ ペイン id を出す
open_editing() {
  local root pane
  root="$("$TAKO_BIN" list 2>/dev/null | json 'd["tabs"][0]["panes"][0]["id"]')"
  pane="$("$TAKO_BIN" open "$1" --pane "$root" --right 2>/dev/null | json 'd["pane"]')"
  "$TAKO_BIN" edit start --pane "$pane" >/dev/null 2>&1
  printf '%s' "$pane"
}

# GUI の子孫の pid（$1 = 根の pid）
descendants() {
  ps -A -o pid=,ppid= | python3 -c '
import sys
root = int(sys.argv[1])
kids = {}
for line in sys.stdin:
    p = line.split()
    if len(p) == 2:
        kids.setdefault(int(p[1]), []).append(int(p[0]))
out, todo = [], [root]
while todo:
    for k in kids.get(todo.pop(), []):
        out.append(k)
        todo.append(k)
print(" ".join(map(str, out)))' "$1"
}

# GUI の子孫で走っている cargo / rustc / build script の数（$1 = GUI の pid）
cargo_children() {
  local pids
  pids="$(descendants "$1")"
  [ -n "$pids" ] || { echo 0; return; }
  # shellcheck disable=SC2086
  ps -o comm= -p $(echo $pids | tr ' ' ',') 2>/dev/null \
    | grep -cE '(^|/)(cargo|rustc|build-script-build)$' || true
}

# 1 回ぶんの起動（$1 = 腕の名前、残りは GUI へ渡す env）。起動したら pid を APP_PID へ
launch_arm() {
  local arm="$1"
  shift
  launch_isolated_gui "$TMP/$arm-app.log" \
    TAKO_LSP_BIN_RUST_ANALYZER="$REAL_RA" \
    RUSTUP_HOME="$REAL_RUSTUP_HOME" CARGO_HOME="$REAL_CARGO_HOME" \
    CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-2}" \
    ${1+"$@"} || exit $?
  APP_PID="$ISOLATED_GUI_PID"
  wait_isolated_gui "$TMP/$arm-app.log" || exit 1
}

# 注入のプロジェクト（$1 = 置き場）: 依存なし（package cache を取り合わない）・build.rs が hold-secs の
# 秒数だけ眠る（rust-analyzer の初回の build script が build directory のロックを握る時間を作る）。
# build.rs は起動のたびに marks/<pid> へ OUT_DIR を書く = どの target で走ったかが分かる
make_fixture() {
  local p="$1"
  mkdir -p "$p/src" "$p/marks"
  cp "$REPO_ROOT/rust-toolchain.toml" "$p/" 2>/dev/null || true
  printf '[package]\nname = "lockprobe"\nversion = "0.1.0"\nedition = "2021"\nbuild = "build.rs"\n[workspace]\n' > "$p/Cargo.toml"
  printf 'fn main() {\n    println!("lockprobe");\n}\n' > "$p/src/main.rs"
  cat > "$p/build.rs" <<'RS'
use std::{env, fs, path::Path, thread, time::Duration};

fn main() {
    let dir = env::var("CARGO_MANIFEST_DIR").unwrap();
    let out = env::var("OUT_DIR").unwrap();
    let secs: u64 = fs::read_to_string(Path::new(&dir).join("hold-secs"))
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0);
    fs::write(Path::new(&dir).join("marks").join(std::process::id().to_string()), &out).unwrap();
    thread::sleep(Duration::from_secs(secs));
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=hold-secs");
}
RS
  echo "${TAKO_2035_HOLD_SECS:-20}" > "$p/hold-secs"
}

# 腕 1 本（①②）: 開く → rust-analyzer の build script が走り出す → 利用者の cargo build。
# 結果は ARM_RA_OUT（rust-analyzer の build script の OUT_DIR）/ ARM_BUILD_RC / ARM_BUILD_SECS / ARM_BLOCKING
lock_arm() {
  local arm="$1" p="$TMP/$1/proj"
  shift
  make_fixture "$p"
  launch_arm "$arm" ${1+"$@"}
  local t_open
  t_open="$(date +%s)"
  open_editing "$p/src/main.rs" >/dev/null
  # rust-analyzer の build script が走り出す（marks に 1 つ目が出る）まで。cargo metadata と build.rs の
  # コンパイルを含む
  if ! wait_cond '[ -n "$(ls "$p/marks" 2>/dev/null)" ]' 180; then
    fail "$arm: rust-analyzer の build script が 180 秒以内に走らない（$(ra_state)）"
    ARM_RA_OUT=""; ARM_BUILD_RC=""; ARM_BUILD_SECS=""; ARM_BLOCKING=""
    stop_isolated_gui "$APP_PID"; APP_PID=""
    return
  fi
  ARM_RA_OUT="$(cat "$p/marks/"* | head -1)"
  echo "  [$arm] rust-analyzer の build script が走り出した（開いてから $(($(date +%s) - t_open)) 秒）: OUT_DIR=${ARM_RA_OUT#"$p"/}"
  # 利用者のビルドでは眠らない（眠るのは rust-analyzer が先に走らせた 1 回だけ）
  echo 0 > "$p/hold-secs"
  local t0
  t0="$(date +%s)"
  (cd "$p" && user_cargo build) > "$TMP/$arm-build.out" 2> "$TMP/$arm-build.err" &
  local build_pid=$!
  # 上限 180 秒（注入の 20 秒 + 余裕）。超えたら自分で起こした cargo だけを落とす
  local i
  for i in $(seq 1 1800); do
    kill -0 "$build_pid" 2>/dev/null || break
    sleep 0.1
  done
  if kill -0 "$build_pid" 2>/dev/null; then
    kill "$build_pid" 2>/dev/null
    fail "$arm: 利用者の cargo build が 180 秒で終わらない"
  fi
  wait "$build_pid"
  ARM_BUILD_RC=$?
  ARM_BUILD_SECS=$(($(date +%s) - t0))
  ARM_BLOCKING="$(grep -c 'Blocking waiting for file lock' "$TMP/$arm-build.err" || true)"
  echo "  [$arm] 利用者の cargo build: 終了 ${ARM_BUILD_RC}・${ARM_BUILD_SECS} 秒・Blocking の行 ${ARM_BLOCKING}"
  grep 'Blocking' "$TMP/$arm-build.err" | sed "s|^|  [$arm]   stderr: |"
  "$TAKO_BIN" lsp stop >/dev/null 2>&1
  stop_isolated_gui "$APP_PID"; APP_PID=""
}

echo "== ① 新腕（既定 = cargo.targetDir = true） =="
lock_arm new
case "$ARM_RA_OUT" in
  */target/rust-analyzer/*) pass "新腕: rust-analyzer の build script は target/rust-analyzer の下で走った" ;;
  *) fail "新腕: rust-analyzer の build script の OUT_DIR が target/rust-analyzer の下でない（${ARM_RA_OUT}）" ;;
esac
check_eq "新腕: 利用者の cargo build は成功" "0" "$ARM_BUILD_RC"
check_eq "新腕: Blocking waiting for file lock が出ない" "0" "$ARM_BLOCKING"
NEW_SECS="$ARM_BUILD_SECS"

echo
echo "== ② 旧腕（TAKO_2035_LEGACY=1 = 初期化オプションを送らない） =="
lock_arm legacy TAKO_2035_LEGACY=1
case "$ARM_RA_OUT" in
  */target/debug/*) pass "旧腕: rust-analyzer の build script は利用者と同じ target/debug で走った" ;;
  *) fail "旧腕: rust-analyzer の build script の OUT_DIR が target/debug の下でない（${ARM_RA_OUT}）" ;;
esac
check_eq "旧腕: 利用者の cargo build は（待たされたうえで）成功" "0" "$ARM_BUILD_RC"
if [ "${ARM_BLOCKING:-0}" -ge 1 ]; then
  pass "旧腕: Blocking waiting for file lock が出る（検出力）"
else
  fail "旧腕: Blocking waiting for file lock が出ない（注入が効いていない）"
fi
echo "  利用者の cargo build の所要: 新 ${NEW_SECS} 秒 / 旧 ${ARM_BUILD_SECS} 秒（注入の眠り ${TAKO_2035_HOLD_SECS:-20} 秒）"

echo
echo "== ③ 上書き（tako lsp options）と ④ 境界 =="
p="$TMP/opt/proj"
make_fixture "$p"
echo 0 > "$p/hold-secs"
launch_arm opt
open_editing "$p/src/main.rs" >/dev/null
wait_cond '[ "$(ra_state)" = running ]' 120 || fail "③: rust-analyzer が稼働しない（$(ra_state)）"
sent() { status | json 'json.dumps([s["init_options"] for s in d["servers"] if s["id"] == "rust-analyzer"][0], sort_keys=True)'; }
check_eq "③ 既定で送った値が status の init_options で読める" '{"cargo": {"targetDir": true}}' "$(sent)"
out="$("$TAKO_BIN" lsp options --name rust-analyzer --set cargo.targetDir=false --json 2>&1)"
check_eq "③ set の応答: 実効値が false" '{"cargo": {"targetDir": false}}' \
  "$(printf '%s' "$out" | json 'json.dumps(d["servers"][0]["effective"], sort_keys=True)')"
check_eq "③ set の直後は動いているサーバが古い値（current=false）" "False" \
  "$(printf '%s' "$out" | json 'd["servers"][0]["running"][0]["current"]')"
check_eq "③ settings.json に書いた" "False" \
  "$(json 'd["lsp_init_options"]["rust-analyzer"]["cargo.targetDir"]' < "$TAKO_DATA_DIR/settings.json")"
"$TAKO_BIN" lsp restart --name rust-analyzer >/dev/null 2>&1
wait_cond '[ "$(sent)" = "{\"cargo\": {\"targetDir\": false}}" ] && [ "$(ra_state)" = running ]' 120 \
  || true
check_eq "③ restart の後は新しい値を送った" '{"cargo": {"targetDir": false}}' "$(sent)"
check_eq "③ restart の後は current=true" "True" \
  "$("$TAKO_BIN" lsp options --json 2>/dev/null | json 'd["servers"][0]["running"][0]["current"]')"
"$TAKO_BIN" lsp options 2>/dev/null | sed 's/^/  | /'
"$TAKO_BIN" lsp options --name rust-analyzer --set cargo.targetDir=null >/dev/null 2>&1
check_eq "③ null で既定へ戻した（settings.json のキーが消える）" "None" \
  "$(json 'd.get("lsp_init_options")' < "$TAKO_DATA_DIR/settings.json")"
check_eq "③ 戻した実効値" '{"cargo": {"targetDir": true}}' \
  "$("$TAKO_BIN" lsp options --json 2>/dev/null | json 'json.dumps(d["servers"][0]["effective"], sort_keys=True)')"

reject() {
  local label="$1"
  shift
  local msg rc
  msg="$("$TAKO_BIN" lsp options "$@" 2>&1)"
  rc=$?
  if [ "$rc" -ne 0 ]; then pass "④ $label を断る（終了 $rc: $(printf '%s' "$msg" | head -1)）"; else fail "④ $label を通した: $msg"; fi
}
reject "--name の無い --set" --set cargo.targetDir=false
reject "キーの形が崩れた --set" --name rust-analyzer --set "bad key=1"
reject "知らないサーバ名" --name no-such-server
reject "= の無い --set" --name rust-analyzer --set cargo.targetDir
check_eq "④ 断った後も settings.json は変わらない" "None" \
  "$(json 'd.get("lsp_init_options")' < "$TAKO_DATA_DIR/settings.json")"
"$TAKO_BIN" lsp stop >/dev/null 2>&1
stop_isolated_gui "$APP_PID"; APP_PID=""

# GUI が居なくても同じ実装で読み書きできる（#1944 の install と同じ CLI のプロセスの経路）
out="$("$TAKO_BIN" lsp options --name rust-analyzer --set cargo.targetDir=false --json 2>&1)"
check_eq "③ GUI が居なくても options を書ける（local）" "True" "$(printf '%s' "$out" | json 'd["local"]')"
check_eq "③ GUI が居なくても settings.json へ書く" "False" \
  "$(json 'd["lsp_init_options"]["rust-analyzer"]["cargo.targetDir"]' < "$TAKO_DATA_DIR/settings.json")"
"$TAKO_BIN" lsp options --name rust-analyzer --set cargo.targetDir=null >/dev/null 2>&1

# --- ⑤ tako 自身のワークスペース ------------------------------------------------------
echo
if [ "${TAKO_2035_MEASURE_TAKO:-0}" != "1" ]; then
  echo "  [--] 未実測: tako 自身のワークスペースの初回の check は TAKO_2035_MEASURE_TAKO=1 のときだけ（重い）"
else
  echo "== ⑤ tako 自身のワークスペース（新腕） =="
  RA_DIR="$REPO_ROOT/target/rust-analyzer"
  if [ -e "$RA_DIR" ]; then
    echo "  [--] 未実測: $RA_DIR が既にある（初回にならない。消してから回す）"
  else
    launch_arm tako CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-4}"
    t0="$(date +%s)"
    open_editing "$REPO_ROOT/crates/tako-core/src/lib.rs" >/dev/null
    # rust-analyzer の cargo が走り出すまで
    wait_cond '[ "$(cargo_children "$APP_PID")" -ge 1 ]' 300 || fail "⑤ rust-analyzer の cargo が走り出さない"
    echo "  rust-analyzer の cargo が走り出した（開いてから $(($(date +%s) - t0)) 秒）"
    # 走っている最中に利用者のビルド（tako-core は既にビルド済み = ロックさえ取れればすぐ終わる）
    sleep 20
    tb="$(date +%s)"
    (cd "$REPO_ROOT" && user_cargo build -p tako-core) > "$TMP/tako-build.out" 2> "$TMP/tako-build.err"
    rc=$?
    blocking="$(grep -c 'Blocking waiting for file lock' "$TMP/tako-build.err" || true)"
    echo "  rust-analyzer の check の最中の cargo build -p tako-core: 終了 ${rc}・$(($(date +%s) - tb)) 秒・Blocking の行 ${blocking}・その時点の rust-analyzer の cargo/rustc $(cargo_children "$APP_PID") 本"
    check_eq "⑤ tako 自身でも check の最中の cargo build にロック待ちが出ない" "0" "$blocking"
    # 初回の check が終わる = rust-analyzer の子孫に cargo / rustc が 30 秒続けて居ない
    quiet=0
    while [ "$quiet" -lt 30 ]; do
      if [ "$(cargo_children "$APP_PID")" -eq 0 ]; then quiet=$((quiet + 1)); else quiet=0; fi
      sleep 1
      if [ $(($(date +%s) - t0)) -gt 3600 ]; then fail "⑤ 初回の check が 1 時間で終わらない"; break; fi
    done
    secs=$(($(date +%s) - t0 - 30))
    echo "  初回の check の所要（開いてから cargo / rustc が居なくなるまで）: ${secs} 秒（CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS:-4}）"
    kb() { du -sk "$1" 2>/dev/null | cut -f1; }
    total="$(kb "$RA_DIR")"
    build="$(kb "$RA_DIR/debug/build")"
    # check では通常の依存は .rmeta だけになる。.rlib / .dylib はホスト向けに本当にコンパイルしたもの
    # （build script と proc-macro とその依存）= 利用者の target/debug と 2 重になるのはここと debug/build
    sum_kb() { find "$RA_DIR/debug/deps" \( "$@" \) -print0 2>/dev/null | xargs -0 du -ck 2>/dev/null | tail -1 | cut -f1; }
    procs="$(sum_kb -name '*.dylib' -o -name '*.so' -o -name '*.dll')"
    rlibs="$(sum_kb -name '*.rlib')"
    rmetas="$(sum_kb -name '*.rmeta')"
    echo "  target/rust-analyzer: ${total} KB（うち build script ${build} KB・proc-macro ${procs:-0} KB・ホスト向けの rlib ${rlibs:-0} KB = 利用者の target/debug と 2 重になるぶん / check の rmeta ${rmetas:-0} KB = 分けなくても増えるぶん）"
    echo "  参考: 利用者の target/debug: $(kb "$REPO_ROOT/target/debug") KB"
    "$TAKO_BIN" lsp stop >/dev/null 2>&1
    stop_isolated_gui "$APP_PID"; APP_PID=""
  fi
fi

echo
echo "結果: PASS=$PASS FAIL=$FAIL"
[ "$FAIL" -eq 0 ]
