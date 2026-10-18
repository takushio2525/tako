#!/bin/bash
# test-lsp-watched-files-2034.sh — tako の外で変わったファイルの通知（#2034）の実経路テスト（隔離 GUI・偽サーバ・実 rust-analyzer）
#
# 何を確かめるか（Issue #2034 の受け入れ条件のうち、GUI と CLI を通して言えるもの）:
#   ① 偽サーバ（`--disk-model` = 実サーバと同じく、開いていないファイルは読み込んだ時点の中身を持ち、
#      知らせでだけ読み直す）: 編集モードで開いた main.rs から参照・定義 → 外で lib.rs を整形相当に
#      書き換え → 知らせ（URI・種別）が届き、参照・定義が新しい行 / git checkout 相当（rename・作成・削除）/
#      開いている main.rs を外で書き換えると GUI が読み直して didChange（偽サーバの本文が新しい中身）
#   ② A/B: `TAKO_2034_LEGACY=1` では同じ手順で lib.rs が古い行のまま（tako がディスクから読む抜粋が別の行）
#   ③ 費用: GUI のスレッド数・FD 数を「サーバ 0 本 → 1 本 → 2 本」で新旧比べる・アイドル 30 秒の CPU 時間・
#      登録に当たらない 2000 ファイルを書いたときの CPU 時間
#   ④ 実の rust-analyzer（`TAKO_2034_REAL=1` のときだけ）: 小さな cargo プロジェクトで `cargo fmt` の前後に
#      参照と定義を問い、新腕は整形後の行。旧腕は記録だけ（GUI は実パスで開くので rust-analyzer 自前の監視が
#      効きうる。リンクを通る根での旧腕のずれは `issue2034_lsp_watched_files` の `--ignored` の実サーバの e2e）
#
# 使い方: bash scripts/test-lsp-watched-files-2034.sh
#   偽サーバがビルド済みなら TAKO_2034_SKIP_BUILD=1（heavy.sh でビルドしてから回すとき）。
#   tako / tako-app は isolated_gui_bins が毎回今の木から建てる（最新なら cargo は何もしない = #1643）
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

# 実の rust-analyzer は HOME を差し替える前に控える。rustup の proxy は一時ディレクトリで既定の
# toolchain を引いて見失うので、リポジトリの toolchain の実体を名指す
REAL_RA="$(cd "$REPO_ROOT" && rustup which rust-analyzer 2>/dev/null || true)"
REAL_RUSTUP_HOME="${RUSTUP_HOME:-$(rustup show home 2>/dev/null || true)}"
REAL_CARGO_HOME="${CARGO_HOME:-$HOME/.cargo}"
REAL_CARGO="$(cd "$REPO_ROOT" && rustup which cargo 2>/dev/null || true)"

TMP="$(mktemp -d /tmp/tako-2034-XXXXXX)"
APP_PID=""
TMUX_SOCKET="tako-2034-$$"
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

if [ "${TAKO_2034_SKIP_BUILD:-0}" != "1" ]; then
  echo "偽サーバをビルドします…"
  (cd "$REPO_ROOT" && cargo build -q -p tako-control --bin tako-lsp-fake) || exit 1
fi
isolated_gui_bins || exit 1
FAKE="$(dirname "$TAKO_BIN")/tako-lsp-fake"
[ -x "$FAKE" ] || { echo "偽サーバが無い: $FAKE"; exit 1; }

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
lsp() { "$TAKO_BIN" lsp "$@" --json 2>/dev/null; }
status() { "$TAKO_BIN" lsp status --json 2>/dev/null; }

# 状態で待つ（上限 $2 秒）。$1 = 条件を評価するシェルの式
wait_cond() {
  local cond="$1" limit="${2:-30}" i
  for i in $(seq 1 $((limit * 10))); do
    eval "$cond" && return 0
    sleep 0.1
  done
  return 1
}

# 偽サーバが受けた知らせ（uri type の行）
changes() {
  python3 - "$1" <<'PY' 2>/dev/null
import json, sys
for line in open(sys.argv[1]):
    try:
        m = json.loads(line)
    except Exception:
        continue
    if m.get("method") == "workspace/didChangeWatchedFiles":
        for c in m["params"]["changes"]:
            print(c["uri"], c["type"])
PY
}

# 知らせの一覧にその行があるか（$1 = ログ・$2 = パターン）。受け取ってから探す（pipefail の下の
# `| grep -q` は左辺が書き終わる前に抜けると偽になる。#2272）
changes_has() {
  local c
  c="$(changes "$1")" && grep -q -- "$2" <<<"$c"
}

# 偽サーバの本文の模型（--doc-log）に、その中身の文書が載ったか（$1 = ログ・$2 = 中身のファイル）
doc_has() {
  python3 - "$1" "$2" <<'PY' 2>/dev/null
import json, sys
want = open(sys.argv[2]).read()
for line in open(sys.argv[1]):
    try:
        doc = json.loads(line).get("fake_doc")
    except Exception:
        continue
    if doc and doc.get("text") == want:
        sys.exit(0)
sys.exit(1)
PY
}

server_state() { status | json 'd["servers"][0]["state"]'; }
running_servers() { status | json 'len([s for s in d["servers"] if s["state"] == "running"])'; }

# GUI のスレッド数・FD 数・CPU 時間（秒）
threads_of() { ps -M -p "$1" 2>/dev/null | tail -n +2 | wc -l | tr -d ' '; }
fds_of() { lsof -p "$1" 2>/dev/null | tail -n +2 | wc -l | tr -d ' '; }
cpu_of() {
  ps -o time= -p "$1" 2>/dev/null | python3 -c '
import sys
t = sys.stdin.read().strip()
parts = t.split(":")
secs = 0.0
for p in parts:
    secs = secs * 60 + float(p)
print(f"{secs:.2f}")'
}

# プロジェクトを作る: $1 = 置き場
make_project() {
  local p="$1"
  mkdir -p "$p/src"
  printf '[package]\nname = "p"\nversion = "0.1.0"\nedition = "2021"\n' > "$p/Cargo.toml"
  printf 'mod util;\nfn main() {\n    util::target();\n}\n' > "$p/src/main.rs"
  printf 'pub fn target() {}\n' > "$p/src/util.rs"
  printf 'fn uses() {\n    crate::util::target();\n}\n' > "$p/src/gone.rs"
}

# 偽サーバの規則（参照 = 模型でその語が現れる所すべて・定義 = 最初の `fn target`）
printf '[{"globPattern": {"baseUri": "{root}", "pattern": "**/*.rs"}}]' > "$TMP/watchers.json"
printf '[{"method": "textDocument/references", "find": "target"}]' > "$TMP/xref.json"
printf '[{"method": "textDocument/definition", "find": "fn target"}]' > "$TMP/goto.json"

# 1 回ぶんの起動（$1 = 腕の名前、残りは GUI へ渡す env）。起動したら pid を APP_PID へ
launch_arm() {
  local arm="$1"
  shift
  : > "$TMP/$arm-fake.jsonl"
  : > "$TMP/$arm-doc.jsonl"
  launch_isolated_gui "$TMP/$arm-app.log" \
    TAKO_LSP_BIN_RUST_ANALYZER="$FAKE" \
    TAKO_LSP_BIN_PYRIGHT="$FAKE" \
    TAKO_LSP_FAKE_DISK_MODEL=1 \
    TAKO_LSP_FAKE_WATCHERS="$TMP/watchers.json" \
    TAKO_LSP_FAKE_XREF="$TMP/xref.json" \
    TAKO_LSP_FAKE_GOTO="$TMP/goto.json" \
    TAKO_LSP_FAKE_LOG="$TMP/$arm-fake.jsonl" \
    TAKO_LSP_FAKE_DOC_LOG="$TMP/$arm-doc.jsonl" \
    ${1+"$@"} || exit $?
  APP_PID="$ISOLATED_GUI_PID"
  wait_isolated_gui "$TMP/$arm-app.log" || exit 1
}

# 編集モードで開く（$1 = ファイル）→ ペイン id を出す
open_editing() {
  local root pane
  root="$("$TAKO_BIN" list 2>/dev/null | json 'd["tabs"][0]["panes"][0]["id"]')"
  pane="$("$TAKO_BIN" open "$1" --pane "$root" --right 2>/dev/null | json 'd["pane"]')"
  "$TAKO_BIN" edit start --pane "$pane" >/dev/null 2>&1
  printf '%s' "$pane"
}

# 参照の要約（ファイル名:行:抜粋 を並べた 1 行）
refs_of() {
  lsp references --pane "$1" --line 3 --column 10 --limit 0 \
    | json '" | ".join(sorted(i["path"].rsplit("/",1)[1]+":"+str(i["line"])+":"+i["text"] for i in d["items"]))'
}
def_of() {
  lsp definition --pane "$1" --line 3 --column 10 --open none \
    | json 'd["locations"][0]["path"].rsplit("/",1)[1]+":"+str(d["locations"][0]["line"])'
}

# 腕 1 本: 新腕なら知らせを待ってから、旧腕なら同じだけ待ってから問う。
# 結果は ARM_REFS / ARM_DEF / ARM_T0 / ARM_T1 / ARM_T2 / ARM_FD0 / ARM_FD1 / ARM_IDLE_CPU / ARM_BURST_CPU
run_arm() {
  local arm="$1" legacy="$2" p
  p="$TMP/$arm-proj"
  make_project "$p"
  if [ "$legacy" = 1 ]; then
    launch_arm "$arm" TAKO_2034_LEGACY=1
  else
    launch_arm "$arm"
  fi
  local log="$TMP/$arm-fake.jsonl"
  sleep 2
  ARM_T0="$(threads_of "$APP_PID")"
  ARM_FD0="$(fds_of "$APP_PID")"
  local src
  src="$(open_editing "$p/src/main.rs")"
  wait_cond '[ "$(server_state)" = running ]' 30 \
    || { fail "$arm: 言語サーバが起きない"; return; }
  # 登録の応答（tako → 偽サーバ）が届くまで待つ = 新腕は監視が動き出している
  wait_cond 'grep -q "\"id\":\"watch-register\"" "$log"' 30 || fail "$arm: 登録の応答が届かない"
  local real_p
  real_p="$(status | json 'd["servers"][0]["root"]')"
  if [ "$legacy" != 1 ]; then
    # 温め: 監視が流れ出すまで書き続ける（実時間で待たない）
    local n=0
    while ! changes_has "$log" "warm.rs "; do
      printf '// warm %s\n' "$n" > "$p/src/warm.rs"
      n=$((n + 1))
      [ "$n" -lt 100 ] || { fail "$arm: 温めの知らせが届かない"; break; }
      sleep 0.2
    done
  fi
  sleep 1.5
  ARM_T1="$(threads_of "$APP_PID")"
  ARM_FD1="$(fds_of "$APP_PID")"
  echo "  [$arm] 根: $real_p"
  echo "  [$arm] 書き換える前: $(refs_of "$src")"

  # cargo fmt 相当: util.rs の宣言の前に 3 行
  printf '//! 整形で増えた行\n\n#[inline]\npub fn target() {}\n' > "$p/src/util.rs"
  if [ "$legacy" != 1 ]; then
    wait_cond 'changes_has "$log" "util.rs "' 10 || fail "$arm: util.rs の知らせが届かない"
  else
    sleep 2
  fi
  ARM_REFS="$(refs_of "$src")"
  ARM_DEF="$(def_of "$src")"
  echo "  [$arm] 整形の後の参照: $ARM_REFS"
  echo "  [$arm] 整形の後の定義: $ARM_DEF"

  # git checkout 相当: rename で置き換え・作成・削除
  printf '\n\npub fn target() {}\n' > "$p/src/.util.rs.tmp"
  mv "$p/src/.util.rs.tmp" "$p/src/util.rs"
  printf 'fn more() {\n    crate::util::target();\n}\n' > "$p/src/added.rs"
  rm -f "${p:?}/src/gone.rs"
  if [ "$legacy" != 1 ]; then
    wait_cond 'changes_has "$log" "gone.rs 3" && changes_has "$log" "added.rs 1"' 10 \
      || fail "$arm: 差し替えの知らせが揃わない（$(changes "$log" | tr '\n' ' ')）"
  else
    sleep 2
  fi
  ARM_CHECKOUT="$(refs_of "$src")"
  echo "  [$arm] 差し替えの後の参照: $ARM_CHECKOUT"

  # 開いている main.rs を外で書き換える → GUI が読み直して didChange（偽サーバの本文の模型が新しい中身）
  printf 'mod util;\n\nfn main() {\n    util::target();\n}\n' > "$TMP/$arm-main.rs"
  cp "$TMP/$arm-main.rs" "$p/src/main.rs"
  if wait_cond 'doc_has "$TMP/$arm-doc.jsonl" "$TMP/$arm-main.rs"' 10; then
    ARM_OPEN_DOC=synced
  else
    ARM_OPEN_DOC=stale
  fi
  echo "  [$arm] 開いている文書を外で書き換えた後の本文の同期: $ARM_OPEN_DOC"

  # 2 本目のサーバ（.py を偽サーバで）: 1 本あたりのスレッドの増分
  local q="$TMP/$arm-py"
  mkdir -p "$q"
  printf 'x = 1\n' > "$q/a.py"
  open_editing "$q/a.py" >/dev/null
  wait_cond '[ "$(running_servers)" = 2 ]' 30 \
    || fail "$arm: 2 本目のサーバが起きない"
  sleep 1.5
  ARM_T2="$(threads_of "$APP_PID")"
  ARM_FD2="$(fds_of "$APP_PID")"

  # アイドル 30 秒の CPU 時間
  local c0 c1
  c0="$(cpu_of "$APP_PID")"
  sleep 30
  c1="$(cpu_of "$APP_PID")"
  ARM_IDLE_CPU="$(python3 -c "print(f'{$c1 - $c0:.2f}')")"
  # 登録に当たらないファイルを 2000 個（ビルドの成果物を模す）書いたときの CPU 時間
  mkdir -p "$p/target/debug"
  c0="$(cpu_of "$APP_PID")"
  python3 - "$p/target/debug" <<'PY'
import os, sys
d = sys.argv[1]
for n in range(2000):
    with open(os.path.join(d, f"obj{n}.o"), "w") as f:
        f.write("x")
PY
  sleep 3
  c1="$(cpu_of "$APP_PID")"
  ARM_BURST_CPU="$(python3 -c "print(f'{$c1 - $c0:.2f}')")"
  ARM_STATUS="$(status)"
  echo "  [$arm] スレッド: サーバ 0 本 $ARM_T0 → 1 本 $ARM_T1 → 2 本 $ARM_T2 / FD: $ARM_FD0 → $ARM_FD1 → $ARM_FD2"
  echo "  [$arm] CPU: アイドル 30 秒 ${ARM_IDLE_CPU} 秒 / 当たらない 2000 ファイル ${ARM_BURST_CPU} 秒"
  echo "  [$arm] file_watch: $(printf '%s' "$ARM_STATUS" | json 'json.dumps({k: d["file_watch"][k] for k in ("active","events","batches","overflows","starts")})')"
  "$TAKO_BIN" lsp stop >/dev/null 2>&1
  stop_isolated_gui "$APP_PID"; APP_PID=""
}

echo
echo "== ① 新腕（偽サーバ・--disk-model） =="
run_arm new 0
NEW_REFS="$ARM_REFS"; NEW_DEF="$ARM_DEF"; NEW_CHECKOUT="$ARM_CHECKOUT"; NEW_OPEN_DOC="$ARM_OPEN_DOC"
NEW_T0="$ARM_T0"; NEW_T1="$ARM_T1"; NEW_T2="$ARM_T2"; NEW_IDLE="$ARM_IDLE_CPU"; NEW_BURST="$ARM_BURST_CPU"; NEW_STATUS="$ARM_STATUS"
check_eq "整形の後の参照は新しい行" "gone.rs:2:crate::util::target(); | main.rs:3:util::target(); | util.rs:4:pub fn target() {}" "$NEW_REFS"
check_eq "整形の後の定義は新しい行" "util.rs:4" "$NEW_DEF"
check_eq "差し替えの後の参照（gone.rs が消え added.rs が増え util.rs は 3 行目）" \
  "added.rs:2:crate::util::target(); | main.rs:3:util::target(); | util.rs:3:pub fn target() {}" "$NEW_CHECKOUT"
check_eq "開いている文書を外で書き換えると didChange で同期" "synced" "$NEW_OPEN_DOC"
check_eq "知らせを送れなかった変化は 0" "0" "$(printf '%s' "$NEW_STATUS" | json 'sum(s["watched_files"]["dropped"] for s in d["servers"])')"

echo
echo "== ② 旧腕（TAKO_2034_LEGACY=1）: 同じ手順で位置がずれる =="
run_arm legacy 1
OLD_T0="$ARM_T0"; OLD_T1="$ARM_T1"; OLD_T2="$ARM_T2"; OLD_IDLE="$ARM_IDLE_CPU"; OLD_BURST="$ARM_BURST_CPU"
check_eq "旧腕: 整形の後も古い行（抜粋は別の行の中身）" "gone.rs:2:crate::util::target(); | main.rs:3:util::target(); | util.rs:1://! 整形で増えた行" "$ARM_REFS"
check_eq "旧腕: 差し替えの後も消えた gone.rs を返し added.rs を返さない" "gone.rs:2: | main.rs:3:util::target(); | util.rs:1:" "$ARM_CHECKOUT"
check_eq "旧腕: 定義も古い行" "util.rs:1" "$ARM_DEF"
check_eq "旧腕も開いている文書は didChange で同期（#1659 の経路は前から在る）" "synced" "$ARM_OPEN_DOC"

echo
echo "== ③ 費用（新腕 − 旧腕） =="
echo "  スレッド（サーバ 0 → 1 → 2 本）: 新 ${NEW_T0} ${NEW_T1} ${NEW_T2} / 旧 ${OLD_T0} ${OLD_T1} ${OLD_T2}"
D1=$(( (NEW_T1 - NEW_T0) - (OLD_T1 - OLD_T0) ))
D2=$(( (NEW_T2 - NEW_T1) - (OLD_T2 - OLD_T1) ))
echo "  監視が足したスレッド: 1 本目のサーバで ${D1} / 2 本目で ${D2}"
check_eq "2 本目のサーバで監視のスレッドは増えない" "0" "$D2"
if [ "$D1" -le 2 ]; then pass "1 本目で監視が足すスレッドは 2 本以下（${D1}）"; else fail "1 本目で監視が足すスレッドが多い（${D1}）"; fi
echo "  CPU（アイドル 30 秒）: 新 ${NEW_IDLE} 秒 / 旧 ${OLD_IDLE} 秒"
echo "  CPU（当たらない 2000 ファイル）: 新 ${NEW_BURST} 秒 / 旧 ${OLD_BURST} 秒"

# --- ④ 実の rust-analyzer -------------------------------------------------------------
echo
if [ "${TAKO_2034_REAL:-0}" != "1" ]; then
  echo "  [--] 未実測: 実の rust-analyzer は TAKO_2034_REAL=1 のときだけ"
elif [ -z "$REAL_RA" ] || [ -z "$REAL_CARGO" ]; then
  echo "  [--] 未実測: rust-analyzer / cargo の実体が見つからない"
else
  real_arm() {
    local arm="$1" p="$TMP/real-$1"
    shift
    mkdir -p "$p/src"
    cp "$REPO_ROOT/rust-toolchain.toml" "$p/" 2>/dev/null || true
    printf '[package]\nname = "p"\nversion = "0.1.0"\nedition = "2021"\n[workspace]\n' > "$p/Cargo.toml"
    printf 'mod util;\nfn main() {\n    let _ = util::helper();\n    util::a();\n}\n' > "$p/src/main.rs"
    # 整形で helper が 2 行目 → 7 行目へ動く
    printf 'pub fn a(){let x=1;let y=2;let z=x+y;println!("{}",z);}\npub fn helper()->u32{1}\n' > "$p/src/util.rs"
    launch_isolated_gui "$TMP/real-$arm.log" \
      TAKO_LSP_BIN_RUST_ANALYZER="$REAL_RA" \
      RUSTUP_HOME="$REAL_RUSTUP_HOME" CARGO_HOME="$REAL_CARGO_HOME" CARGO_BUILD_JOBS=2 \
      TAKO_LSP_XREF_TIMEOUT_SECS=300 TAKO_LSP_GOTO_TIMEOUT_SECS=300 ${1+"$@"} || exit $?
    APP_PID="$ISOLATED_GUI_PID"
    wait_isolated_gui "$TMP/real-$arm.log" || exit 1
    local src
    src="$(open_editing "$p/src/main.rs")"
    real_refs() {
      lsp references --pane "$src" --line 3 --column 18 --limit 0 \
        | json '" | ".join(sorted(i["path"].rsplit("/",1)[1]+":"+str(i["line"])+":"+i["text"] for i in d["items"]))'
    }
    echo "  [$arm] 整形の前: $(real_refs)"
    (cd "$p" && RUSTUP_HOME="$REAL_RUSTUP_HOME" CARGO_HOME="$REAL_CARGO_HOME" "$REAL_CARGO" fmt) \
      || fail "$arm: cargo fmt が失敗"
    local want="util.rs:$(grep -n '^pub fn helper' "$p/src/util.rs" | cut -d: -f1):pub fn helper() -> u32 {"
    echo "  [$arm] 整形の後の util.rs の helper: ${want}（grep -n）"
    local t0 got="" i
    t0="$(date +%s)"
    for i in $(seq 1 20); do
      got="$(real_refs)"
      case "$got" in *"$want"*) break ;; esac
      sleep 0.5
    done
    REAL_GOT="$got"
    REAL_DEF="$(lsp definition --pane "$src" --line 3 --column 18 --open none \
      | json 'd["locations"][0]["path"].rsplit("/",1)[1]+":"+str(d["locations"][0]["line"])+":"+d["locations"][0]["text"]')"
    REAL_WANT="$want"
    echo "  [$arm] 整形の後の参照（$(($(date +%s) - t0)) 秒以内に問い直し）: $REAL_GOT"
    echo "  [$arm] 整形の後の定義: $REAL_DEF"
    "$TAKO_BIN" lsp status --json | json '"  ['"$arm"'] watched_files="+json.dumps(d["servers"][0]["watched_files"]["patterns"])'
    "$TAKO_BIN" lsp stop >/dev/null 2>&1
    stop_isolated_gui "$APP_PID"; APP_PID=""
  }
  echo "== ④ 実の rust-analyzer（cargo fmt の前後） =="
  real_arm new
  case "$REAL_GOT" in *"$REAL_WANT"*) pass "新腕: 参照が整形後の行（${REAL_WANT}）" ;; *) fail "新腕: 参照が整形後の行でない（${REAL_GOT}）" ;; esac
  check_eq "新腕: 定義が整形後の行" "$REAL_WANT" "$REAL_DEF"
  # 旧腕は合否にしない: GUI は文書を実パスで開く（OpenFile の canonicalize）ので、rust-analyzer 自前の監視が
  # 効いて整形後の行を返しうる。ずれるのは根がシンボリックリンクを通るとき = tako の manager を通す実サーバの
  # e2e（`TAKO_2034_REAL_RA=… cargo test -p tako-control --test issue2034_lsp_watched_files -- --ignored`）が測る
  real_arm legacy TAKO_2034_LEGACY=1
  echo "  [--] 記録: 旧腕（申告なし・実パスの根）の整形後の参照 = ${REAL_GOT}（rust-analyzer 自前の監視が拾えば整形後の行）"
fi

echo
echo "結果: PASS=$PASS FAIL=$FAIL"
[ "$FAIL" -eq 0 ]
