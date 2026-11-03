#!/bin/bash
# test-lsp-loading-limit-1899.sh — LSP の続き 3（#1899）の GUI の実経路テスト（隔離 GUI・偽サーバ・実 rust-analyzer）
#
# 何を確かめるか（Issue #1899 の 2 点目とコメントの GUI の ①②）:
#   ① visual-test 節 completion-loading-limit（偽サーバの読み込みが終わらない・上限は 2 秒へ縮める =
#      負荷は使わない）: 読み込み中に打つと「読み込み中」→ 上限で**黙って消えずに**理由の 1 行
#      （基準画像との差分は矩形の外 0 px）→ 空白で消える → 打ち直すと「読み込み中」へ戻りまた理由の 1 行
#   ② visual-test 節 hover-key-loading（偽サーバの読み込みの終わりは合図のファイル）: 読み込み中に
#      キー（⇧⌘H・Windows は Ctrl+Shift+H）で頼むとマウスと同じく語の真下に「読み込み中」の 1 行・
#      ヘッダは問い合わせ中の印だけ → 済むとカードへ差し替わる / 右クリックの「ホバー情報を表示」でも同じ
#   ③ visual-test 節 hover-footer-1899: 切ったカードの脚注に `tako lsp hover --pane N --line L --column C --full`
#   ④ A/B: `TAKO_1899_LEGACY=1` で ①②③ が名指しで落ちる（画像は before/ へ分ける）
#   ⑤ 回帰: #1869 completion-loading・#1893 hover-loading / hover-1893・#1682 completion が緑のまま
#   ⑥ 実の rust-analyzer: #1682 の completion-real（候補が出て Enter で入る）が緑のまま
#      （整形の実サーバは scripts/test-lsp-format-line-diff-1899.sh の ⑤）
#   ⑦ 右クリックメニューの割り込みを数える仕組み（`vt1899_menu`）を広げた節（lsp-context-menu /
#      xref-1685 / rename-1686 / hover-key-loading）: 割り込みを注入（`TAKO_VISUAL_1899_INTERFERE=1` =
#      待ちの後にメニューを閉じる）すると `interfered=` に数えて緑、上限を超える注入（=9）は
#      割り込みとして名指しの FAILED（実装の不具合の文と違う）
#
# 使い方: bash scripts/test-lsp-loading-limit-1899.sh
#   画像を残すときは TAKO_1899_DUMP_DIR=<dir>（既定は一時ディレクトリ = 終わると消える）。両テーマを落とす
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

# 実の rust-analyzer は HOME を差し替える前に控える（rustup の proxy なので、HOME を変えると
# toolchain を見失う = #1682 / #1893 のスクリプトと同じ扱い）
REAL_RA="$(command -v rust-analyzer 2>/dev/null || true)"
REAL_RUSTUP_HOME="${RUSTUP_HOME:-$(rustup show home 2>/dev/null || true)}"
REAL_CARGO_HOME="${CARGO_HOME:-$HOME/.cargo}"

TMP="$(mktemp -d /tmp/tako-1899-XXXXXX)"
DUMP="${TAKO_1899_DUMP_DIR:-$TMP/dump}"
TMUX_SOCKET="tako-1899-$$"
cleanup() {
  stop_isolated_gui "${ISOLATED_GUI_PID:-}"
  tmux -L "$TMUX_SOCKET" kill-server >/dev/null 2>&1 || true
  rm -rf "${TMP:?}"
}
trap cleanup EXIT

. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
# 隔離 GUI は言語サーバを自動では起こさない（#2048）。この検証は起こす前提なので明示する
export TAKO_LSP_AUTOSTART=1

echo "visual-test 版の tako-app と偽サーバをビルドします…"
(cd "$REPO_ROOT" && run_heavy cargo build -q -p tako-control --bin tako-lsp-fake) || exit 1
isolated_gui_bins visual-test || exit 1

export HOME="$TMP/home"
export TAKO_ISOLATED=1
export TAKO_DATA_DIR="$TMP/data"
export TAKO_DISCOVERY_DIR="$TMP/disc"
export TAKO_ORCHESTRATOR_DIR="$TMP/orch"
export TAKO_SESSIONS_FILE="$TMP/sessions.yaml"
export TAKO_PANE_LOG_DIR="$TMP/panelogs"
export TAKO_WORKERS_FILE="$TMP/workers.yaml"
export TAKO_TMUX_SOCKET="$TMUX_SOCKET"
mkdir -p "$HOME" "$TAKO_DATA_DIR" "$TAKO_DISCOVERY_DIR" "$TAKO_ORCHESTRATOR_DIR" "$DUMP/after" "$DUMP/before"
for d in "$HOME" "$TAKO_DATA_DIR" "$TAKO_ORCHESTRATOR_DIR"; do
  case "$d" in
    "$TMP"/*) : ;;
    *) echo "隔離されていないディレクトリ: $d"; exit 1 ;;
  esac
done

# 節を 1 回走らせて、終わるまで**状態で**待つ（プロセスが消えるまで。上限は 300 秒）。
# 画像は既定で after/ へ（旧挙動の腕は呼び出し側が before/ を渡す = 同じ名前で上書きしない）
run_section() {
  local log="$1" section="$2"
  shift 2
  launch_isolated_gui "$log" TAKO_VISUAL_TEST=1 TAKO_VISUAL_ONLY="$section" \
    TAKO_VISUAL_DUMP_DIR="$DUMP/after" ${1+"$@"} || exit $?
  local pid="$ISOLATED_GUI_PID" i
  for i in $(seq 1 3000); do
    kill -0 "$pid" 2>/dev/null || break
    sleep 0.1
  done
  stop_isolated_gui "$pid"
  ISOLATED_GUI_PID=""
  return 0
}

# 節が緑か（名前・ログ）
section_green() {
  local name="$1" log="$2"
  grep "TAKO_VISUAL_PIXEL: $name" "$log" | sed 's/^/    /'
  if grep -q "TAKO_VISUAL_TEST_OK" "$log"; then
    pass "$name の全相が緑（TAKO_VISUAL_TEST_OK）"
  else
    fail "$name が緑にならない"
    grep -E "FAILED|panicked|ERROR" "$log" | tail -5 | sed 's/^/    /'
  fi
}

# 旧挙動の腕が期待どおりの相で名指しで落ちたか
section_named() {
  local name="$1" log="$2" want="$3"
  grep "TAKO_VISUAL_PIXEL: $name" "$log" | sed 's/^/    旧腕: /'
  if grep -q "TAKO_APP_SELF_TEST_FAILED: visual-test $want" "$log"; then
    pass "$name は TAKO_1899_LEGACY=1 で名指しで落ちる: $(grep -o 'TAKO_APP_SELF_TEST_FAILED: .*' "$log" | head -1 | cut -c1-160)"
  else
    fail "$name が TAKO_1899_LEGACY=1 で期待の相で落ちない（検出力が無い）: $(grep -o 'TAKO_APP_SELF_TEST_FAILED: .*' "$log" | head -1)"
  fi
}

echo
echo "== ① visual-test 節 completion-loading-limit（補完の読み込みが上限に当たる） =="
run_section "$TMP/limit.log" completion-loading-limit
section_green completion-loading-limit "$TMP/limit.log"

echo
echo "== ② visual-test 節 hover-key-loading（キーで頼んだホバーの読み込み中を語の真下に） =="
run_section "$TMP/key-loading.log" hover-key-loading
section_green hover-key-loading "$TMP/key-loading.log"

echo
echo "== ③ visual-test 節 hover-footer-1899（切ったカードの脚注に全文の取り方） =="
run_section "$TMP/footer.log" hover-footer-1899
section_green hover-footer-1899 "$TMP/footer.log"

echo
echo "== ④ A/B: 旧挙動（TAKO_1899_LEGACY=1）で同じ節が名指しで落ちる =="
run_section "$TMP/limit-legacy.log" completion-loading-limit TAKO_1899_LEGACY=1 TAKO_VISUAL_DUMP_DIR="$DUMP/before"
section_named completion-loading-limit "$TMP/limit-legacy.log" \
  "completion-loading-limit ②: 上限に当たっても黙って消えず、理由の 1 行が出る"
run_section "$TMP/key-loading-legacy.log" hover-key-loading TAKO_1899_LEGACY=1 TAKO_VISUAL_DUMP_DIR="$DUMP/before"
section_named hover-key-loading "$TMP/key-loading-legacy.log" \
  "hover-key-loading ①: 読み込み中にキーで頼むと、マウスと同じく語の真下に"
run_section "$TMP/footer-legacy.log" hover-footer-1899 TAKO_1899_LEGACY=1 TAKO_VISUAL_DUMP_DIR="$DUMP/before"
section_named hover-footer-1899 "$TMP/footer-legacy.log" \
  "hover-footer-1899: 切ったカードの脚注に人向けの全文の取り方が出る"

echo
echo "== ⑤ 回帰: #1869 completion-loading・#1893 hover-loading / hover-1893・#1682 completion =="
for section in completion-loading hover-loading hover-1893 completion; do
  run_section "$TMP/reg-$section.log" "$section"
  section_green "$section" "$TMP/reg-$section.log"
done

echo
echo "== ⑥ 実の rust-analyzer（visual-test 節 completion-real） =="
if [ -n "$REAL_RA" ] && [ -n "$REAL_RUSTUP_HOME" ]; then
  run_section "$TMP/real.log" completion-real \
    TAKO_LSP_BIN_RUST_ANALYZER="$REAL_RA" RUSTUP_HOME="$REAL_RUSTUP_HOME" CARGO_HOME="$REAL_CARGO_HOME"
  section_green completion-real "$TMP/real.log"
else
  echo "  [--] 未実測: rust-analyzer が PATH に無い"
fi
echo
echo "== ⑦ 右クリックメニューの割り込み（注入）を interfered に数え、FAILED と区別する =="
for section in lsp-context-menu xref-1685 rename-1686 hover-key-loading; do
  run_section "$TMP/inj1-$section.log" "$section" TAKO_VISUAL_1899_INTERFERE=1 TAKO_VISUAL_DUMP_DIR="$DUMP/inject"
  counted="$(grep -c 'TAKO_VISUAL_PIXEL: vt1899-menu interfered=closed' "$TMP/inj1-$section.log")"
  grep -E "TAKO_VISUAL_PIXEL: $section .*interfered=" "$TMP/inj1-$section.log" | sed 's/^/    /'
  if grep -q "TAKO_VISUAL_TEST_OK" "$TMP/inj1-$section.log" && [ "$counted" -ge 1 ]; then
    pass "$section: 注入 1 回ずつの割り込みを interfered に数えて緑（数えた割り込み $counted 回）"
  else
    fail "$section: 注入した割り込みで緑にならない（数えた割り込み $counted 回）"
    grep -E "FAILED|panicked" "$TMP/inj1-$section.log" | tail -3 | sed 's/^/    /'
  fi
  run_section "$TMP/inj9-$section.log" "$section" TAKO_VISUAL_1899_INTERFERE=9 TAKO_VISUAL_DUMP_DIR="$DUMP/inject"
  if grep -q "TAKO_APP_SELF_TEST_FAILED: visual-test $section.*右クリックメニューへの割り込みが 3 回を超えて続いた" "$TMP/inj9-$section.log"; then
    pass "$section: 上限を超える割り込みは割り込みとして名指しの FAILED: $(grep -o 'TAKO_APP_SELF_TEST_FAILED: .*' "$TMP/inj9-$section.log" | head -1 | cut -c1-140)"
  else
    fail "$section: 上限を超える割り込みが割り込みとして名指しされない: $(grep -o 'TAKO_APP_SELF_TEST_FAILED: .*' "$TMP/inj9-$section.log" | head -1)"
  fi
done

ls "$DUMP"/after/*1899*.png "$DUMP"/after/completion-loading-limit*.png "$DUMP"/after/hover-key-loading*.png 2>/dev/null \
  | sort -u | sed 's/^/    画像（after）: /'

echo
echo "結果: PASS=$PASS FAIL=$FAIL"
[ "$FAIL" -eq 0 ]
