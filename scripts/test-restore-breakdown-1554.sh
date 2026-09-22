#!/usr/bin/env bash
# test-restore-breakdown-1554.sh — 復元の内訳が全ペインを説明することの実経路テスト（#1554）
#
# 隔離した data / tmux で**実 tako-app** を立て、
#   ① 内訳の 7 カテゴリ（tmux 再 attach / Claude resume / 新規シェル / プレビュー /
#      Web ビュー / たまり場・退避 / 復元失敗）の**合計が復元ペイン数と一致**する
#   ② 復元できなかったペインは `復元失敗（ペイン N）: <分類>: <エラー>` として
#      persist.log に 1 行ずつ残る（#1554 前は `eprintln!` 止まり = GUI の stderr は
#      どこにも出ないので痕跡ゼロ）
#   ③ たまり場ペイン（FR-2.15.5）と退避タブ配下のペイン（#1487）は
#      **失敗ではなく `たまり場・退避`** として数えられる（実測でこれが差 1〜2 の正体）
#   ④ 失敗 0 件の復元では従来の 4 カテゴリに加えて新カテゴリが 0 で並び、
#      「内訳が全ペインを説明できていない」行は出ない
#   ⑤ A/B `TAKO_1554_LEGACY=1` で修正前の症状（4 カテゴリのみ・合計が足りない・
#      個別の失敗行なし）が同一バイナリで再現する
#   ⑥ 端末ペインを 1 つも起こせない layout では既存の「1 つも起動できない」致命終了が
#      働き、内訳の 1 行目と**二重に報告しない**（個別の失敗行だけは残る）
# を実測する。
#
# 復元の失敗は**起動するプログラムを存在しないパスにする**ことで作る
# （`SHELL=/nonexistent/...` + `TAKO_TMUX_BIN=/nonexistent/...`）。注入用の env を
# 本番コードへ足さずに本物の `SessionError::Pty` を通せる唯一の道で、
# **ペインの cwd を壊す手では作れない**（実測: alacritty_terminal 0.26 の
# `tty::unix::new` は `pre_exec` の中で `libc::chdir` の戻り値を捨てる =
# "Set working directory, ignoring invalid paths."）。
# プレビュー / Web ビューのペインが残るので「1 つも起動できない」の致命終了には
# ならず、内訳の 1 行目まで到達する。致命終了の側（⑥）は端末ペインだけの
# layout で別に見る。
#
# **本番の tako / 設定には一切触らない**（data / HOME は mktemp 配下、tmux は専用ソケット、
# 落とすのは自分で起こした pid だけ）。窓は常設の仮想ディスプレイ（tako-vd）へ出る（#1141）。
#
# **CI には登録しない**（実 GUI を立てるので表示のある実機でだけ回る = 他の
# `isolated-gui.sh` 系スクリプトと同じ扱い）。CI 側の担保は単体テスト
# （`tako_control::restore_report`）と番犬
# （`crates/tako-control/tests/issue1554_restore_breakdown_watchdog.rs`）。
#
# 使い方: bash scripts/test-restore-breakdown-1554.sh
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
check_has() {
  case "$3" in
    *"$2"*) pass "$1" ;;
    *) fail "$1（'${2}' が無い / 実際: ${3}）" ;;
  esac
}
check_not() {
  case "$3" in
    *"$2"*) fail "$1（'${2}' が出ている / 実際: ${3}）" ;;
    *) pass "$1" ;;
  esac
}

TMP="$(mktemp -d /tmp/tako-1554-XXXXXX)"
APP_PID=""
TMUX_SOCKET="tako-1554-$$"
cleanup() {
  stop_isolated_gui "$APP_PID"
  tmux -L "$TMUX_SOCKET" kill-server >/dev/null 2>&1 || true
  rm -rf "$TMP"
}
trap cleanup EXIT

# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
isolated_gui_bins || exit 1

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
# 復元経路そのものを見るので永続は ON（TAKO_ISOLATED は既定で OFF にする）
export TAKO_PERSIST=1
mkdir -p "$TAKO_DATA_DIR" "$TAKO_DISCOVERY_DIR" "$TAKO_ORCHESTRATOR_DIR"
for d in "$HOME" "$TAKO_DATA_DIR" "$TAKO_ORCHESTRATOR_DIR"; do
  case "$d" in
    "$TMP"/*) : ;;
    *) echo "隔離されていないディレクトリ: $d"; exit 1 ;;
  esac
done

LAYOUT="$TAKO_DATA_DIR/layout.json"
PLOG="$TAKO_DATA_DIR/persist.log"

#   start_app <ログの名前> [VAR=VAL …]
start_app() {
  local name="$1"
  shift
  : > "$PLOG"
  # 面を起こすのは launch_isolated_gui の中（#1490）
  launch_isolated_gui "$TMP/app-${name}.log" ${@+"$@"}
  APP_PID="$ISOLATED_GUI_PID"
  wait_isolated_gui "$TMP/app-${name}.log" || exit 1
}
stop_app() {
  stop_isolated_gui "$APP_PID"
  APP_PID=""
  sleep 1
}
# 内訳の 1 行目（4 カテゴリ時代から共通の `tmux 再 attach` を目印にする。
# layout 読み込みの「復元成功: … （tmux あり: …）」とは別行）
summary_line() {
  grep '復元成功' "$PLOG" 2>/dev/null | grep 'tmux 再 attach' | tail -1
}
detail_line() { grep '復元の内訳:' "$PLOG" 2>/dev/null | tail -1; }

echo "== 隔離 GUI を起こして検証用の構成を組む =="
start_app 1
TABS="$("$TAKO_BIN" list | python3 -c 'import json,sys; print(len(json.load(sys.stdin)["tabs"]))')"
if [ "$TABS" != "1" ]; then
  echo "繋がった先が隔離インスタンスではない（タブ ${TABS} 枚）。中止する。"; exit 1
fi
echo "  pid=$APP_PID data=$TAKO_DATA_DIR"

TAB1="$("$TAKO_BIN" list | python3 -c 'import json,sys; print(json.load(sys.stdin)["tabs"][0]["id"])')"
P1="$("$TAKO_BIN" list | python3 -c 'import json,sys; print(json.load(sys.stdin)["tabs"][0]["panes"][0]["id"])')"
# 素のシェル 2 本（この 2 本が ①②③ で起動に失敗する側）
P2="$("$TAKO_BIN" split --pane "$P1" --right | tr -dc '0-9')"
# プレビューペイン（PTY を起こさない側）
echo '# 1554' > "$TMP/note.md"
"$TAKO_BIN" open "$TMP/note.md" --pane "$P1" --right >/dev/null 2>&1
# Web ビューペイン（PTY を起こさない側。#1554 以前はどのカテゴリにも入らなかった）
"$TAKO_BIN" web open "https://example.com" --pane "$P2" --down >/dev/null 2>&1
# たまり場ペイン（FR-2.15.5）
P3="$("$TAKO_BIN" split --pane "$P1" --down | tr -dc '0-9')"
"$TAKO_BIN" background --pane "$P3" >/dev/null 2>&1
# 退避タブ（#1487。最後の 1 枚にならないよう新しいタブを作ってから退避する）
TAB2="$("$TAKO_BIN" tab new | python3 -c 'import json,sys; print(json.load(sys.stdin)["tab"])')"
"$TAKO_BIN" tab select "$TAB1" >/dev/null 2>&1
"$TAKO_BIN" background --tab "$TAB2" >/dev/null 2>&1
sleep 3   # layout.json の保存（2 秒ポーリング）を待つ
stop_app

echo "== layout.json の構成を確かめる =="
# 解析・細工・1 行目の読み取りは 1 本のツールへ寄せる（3 か所へ写すと形式の理解が
# ずれる。**実測の保存形は内部タグ** `{"type":"pane"}` / `{"type":"split"}`）
TOOL="$TMP/layout_tool.py"
cat > "$TOOL" <<'PYEOF'
import json, re, sys

def panes(node, acc):
    kind = node.get("type")
    if kind == "pane" or ("id" in node and "first" not in node):
        acc.append(node)
    elif kind == "split" or "first" in node:
        panes(node["first"], acc)
        panes(node["second"], acc)
    else:
        raise SystemExit(f"未知のツリー節: {sorted(node.keys())}")
    return acc

def tab_panes(d):
    out = []
    for t in d["tabs"]:
        panes(t["tree"], out)
    return out

def shelved_panes(d):
    out = []
    for e in d.get("shelved_tabs", []):
        panes(e["tab"]["tree"], out)
    return out

def plain(ps):
    return [p for p in ps if not p.get("preview") and not p.get("webview")]

cmd, path = sys.argv[1], sys.argv[2]
if cmd == "shape":
    d = json.load(open(path))
    tab, shelved, bg = tab_panes(d), shelved_panes(d), d.get("backgrounded", [])
    print("parsed 1")
    print("tab_panes", len(tab))
    print("shelved_tab_panes", len(shelved))
    print("backgrounded", len(bg))
    print("all_panes", len(tab) + len(shelved) + len(bg))
    print("previews", sum(1 for p in tab if p.get("preview")))
    print("webviews", sum(1 for p in tab if p.get("webview")))
    print("hidden", len(shelved) + len(bg))
    print("plain_count", len(plain(tab)))
    print("plain_ids_list", " ".join(str(p["id"]) for p in plain(tab)))
elif cmd == "only_plain":
    # 端末ペイン 1 本だけの layout を書き出す（プレビュー / Web ビュー / たまり場・
    # 退避を外すと「1 つも起動できない」の致命終了が必ず起きる = ⑥ の条件）
    dst = sys.argv[3]
    d = json.load(open(path))
    keep = plain(tab_panes(d))[0]
    tab = dict(d["tabs"][0])
    tab["tree"] = keep
    tab["focused"] = keep["id"]
    d["tabs"] = [tab]
    d["active_tab"] = tab["id"]
    d["shelved_tabs"] = []
    d["backgrounded"] = []
    json.dump(d, open(dst, "w"), ensure_ascii=False)
    print(keep["id"])
elif cmd == "summary":
    # path は 1 行目そのもの（ファイルではない）
    m = re.search(r"復元成功: (\d+) タブ / (\d+) ペイン（(.+)）", path)
    if not m:
        raise SystemExit(f"1 行目を解析できない: {path!r}")
    seg = {}
    for part in m.group(3).split(" / "):
        label, _, n = part.rpartition(" ")
        seg[label] = int(n)
    print("parsed 1")
    print("tabs", int(m.group(1)))
    print("panes", int(m.group(2)))
    print("total", sum(seg.values()))
    print("categories", len(seg))
    for k, v in seg.items():
        print("seg:" + k.replace(" ", "_"), v)
else:
    raise SystemExit(f"未知のコマンド: {cmd}")
PYEOF

python3 "$TOOL" shape "$LAYOUT" > "$TMP/shape.txt" || { echo "layout.json を解析できない"; exit 1; }
get() { awk -v k="$1" '$1==k {print $2}' "$TMP/shape.txt"; }
# 値が複数語の行（`plain_ids_list 1 2`）は $2 だけでは 1 つ目しか採れない
get_list() { awk -v k="$1" '$1==k {$1=""; sub(/^ /, ""); print}' "$TMP/shape.txt"; }
check_eq "layout.json を解析できた" "1" "$(get parsed)"
check_eq "たまり場ペインが 1 件" "1" "$(get backgrounded)"
check_eq "退避タブ配下のペインが 1 件" "1" "$(get shelved_tab_panes)"
check_eq "プレビューペインが 1 件" "1" "$(get previews)"
check_eq "Web ビューペインが 1 件" "1" "$(get webviews)"

echo "== ①②③ 端末ペインの起動を失敗させた復元 =="
# 失敗の作り方: 起動するプログラム自体を存在しないパスにする。tmux を引けなくすると
# `spawn_session` は器を被せず `$SHELL -l` を直に起こすので、`SHELL` の差し替えが効く
SABOTAGE_ENV_1="SHELL=$TMP/nonexistent-shell"
SABOTAGE_ENV_2="TAKO_TMUX_BIN=$TMP/nonexistent-tmux"
cp "$LAYOUT" "$TMP/layout-good.json"
start_app 2 "$SABOTAGE_ENV_1" "$SABOTAGE_ENV_2"
sleep 1
SUM_LINE="$(summary_line)"
DET_LINE="$(detail_line)"
echo "  1 行目: $SUM_LINE"
echo "  2 行目: $DET_LINE"
python3 "$TOOL" summary "$SUM_LINE" > "$TMP/verdict.txt" || { echo "1 行目を解析できない"; exit 1; }
v() { awk -v k="$1" '$1==k {print $2}' "$TMP/verdict.txt"; }
check_eq "1 行目を解析できた" "1" "$(v parsed)"
check_eq "① 内訳の合計 == 復元ペイン数" "$(v panes)" "$(v total)"
check_eq "① 1 行目のペイン数が layout と一致" "$(get all_panes)" "$(v panes)"
check_eq "① カテゴリが 7 つ並ぶ" "7" "$(v categories)"
check_eq "① プレビューの件数が layout と一致" "$(get previews)" "$(v seg:プレビュー)"
check_eq "① Web ビューの件数が layout と一致" "$(get webviews)" "$(v seg:Web_ビュー)"
check_eq "③ たまり場・退避の件数が layout と一致" "$(get hidden)" "$(v seg:たまり場・退避)"
check_eq "② 復元失敗が端末ペインの数と一致" "$(get plain_count)" "$(v seg:復元失敗)"
check_eq "① 内訳が全ペインを説明できていない行は出ない" "0" \
  "$(grep -c '説明できていない' "$PLOG" | tr -d ' ')"
echo "  失敗行:"; grep '復元失敗（ペイン' "$PLOG" | sed 's/^/    /'
check_eq "② 失敗行が端末ペインの数だけ出る" "$(get plain_count)" \
  "$(grep -c '復元失敗（ペイン' "$PLOG" | tr -d ' ')"
for pane in $(get_list plain_ids_list); do
  check_eq "② 失敗行がペイン ${pane} を名指す" "1" \
    "$(grep -c "復元失敗（ペイン ${pane}）" "$PLOG" | tr -d ' ')"
done
check_has "② 失敗行が理由の分類を載せる" "起動できない" "$(grep '復元失敗（ペイン' "$PLOG" | tail -1)"
check_has "③ 2 行目にたまり場・退避の内訳が出る" "たまり場・退避 2（たまり場 1 / 退避タブ 1）" "$DET_LINE"
check_has "② 2 行目に失敗の理由が出る" "復元失敗 $(get plain_count)（起動できない" "$DET_LINE"
check_not "② 診断にペインの中身が漏れていない" "example.com" "$(grep '復元' "$PLOG" | tr '\n' ' ')"
# 同じ内訳が CLI / MCP（dispatch）からも読める = persist の last_restore（FR-5.7）
LAST_RESTORE="$("$TAKO_BIN" persist 2>/dev/null | python3 -c 'import json,sys; print(json.load(sys.stdin).get("last_restore") or "")')"
check_has "② tako persist の last_restore に同じ内訳が載る" \
  "たまり場・退避 2 / 復元失敗 $(get plain_count)" "$LAST_RESTORE"
stop_app

echo "== ⑤ A/B: TAKO_1554_LEGACY=1 で修正前の症状が再現する =="
cp "$TMP/layout-good.json" "$LAYOUT"
start_app legacy "$SABOTAGE_ENV_1" "$SABOTAGE_ENV_2" TAKO_1554_LEGACY=1
sleep 1
LEG_LINE="$(summary_line)"
echo "  1 行目: $LEG_LINE"
python3 "$TOOL" summary "$LEG_LINE" > "$TMP/legacy.txt" || { echo "旧挙動の 1 行目を解析できない"; exit 1; }
l() { awk -v k="$1" '$1==k {print $2}' "$TMP/legacy.txt"; }
check_eq "⑤ 旧挙動の 1 行目を解析できた" "1" "$(l parsed)"
check_eq "⑤ 旧挙動は 4 カテゴリ" "4" "$(l categories)"
if [ -n "$(l total)" ] && [ "$(l total)" -lt "$(l panes)" ]; then
  pass "⑤ 旧挙動は内訳の合計がペイン数より小さい（合計 $(l total) / $(l panes) ペイン = Issue の症状）"
else
  fail "⑤ 旧挙動で症状が再現しない（合計 '$(l total)' / '$(l panes)' ペイン）"
fi
check_eq "⑤ 旧挙動は個別の失敗行を残さない" "0" "$(grep -c '復元失敗（ペイン' "$PLOG" | tr -d ' ')"
check_eq "⑤ 旧挙動は食い違いも名指さない" "0" "$(grep -c '説明できていない' "$PLOG" | tr -d ' ')"
stop_app

echo "== ④ エッジ: 失敗 0 件の復元（細工なし） =="
cp "$TMP/layout-good.json" "$LAYOUT"
start_app 4
sleep 1
OK_LINE="$(summary_line)"
echo "  1 行目: $OK_LINE"
python3 "$TOOL" summary "$OK_LINE" > "$TMP/ok.txt" || { echo "1 行目を解析できない"; exit 1; }
o() { awk -v k="$1" '$1==k {print $2}' "$TMP/ok.txt"; }
check_eq "④ 1 行目を解析できた" "1" "$(o parsed)"
check_has "④ 従来の 4 カテゴリはそのまま並ぶ" "tmux 再 attach" "$OK_LINE"
check_eq "④ 復元失敗が 0 で並ぶ" "0" "$(o seg:復元失敗)"
check_eq "④ Web ビューが 1 で並ぶ" "1" "$(o seg:Web_ビュー)"
check_eq "④ たまり場・退避の件数が layout と一致" "$(get hidden)" "$(o seg:たまり場・退避)"
check_eq "④ 失敗行は出ない" "0" "$(grep -c '復元失敗（ペイン' "$PLOG" | tr -d ' ')"
check_eq "④ 食い違いの名指しも出ない" "0" "$(grep -c '説明できていない' "$PLOG" | tr -d ' ')"
check_eq "④ 合計 == ペイン数（失敗 0 件でも成り立つ）" "$(o panes)" "$(o total)"
stop_app

echo "== ⑥ エッジ: 端末ペインだけの layout で全滅（「1 つも起動できない」と二重に出ない） =="
python3 "$TOOL" only_plain "$TMP/layout-good.json" "$LAYOUT" >/dev/null || { echo "端末ペインだけの layout を作れない"; exit 1; }
: > "$PLOG"
launch_isolated_gui "$TMP/app-fatal.log" "$SABOTAGE_ENV_1" "$SABOTAGE_ENV_2"
FATAL_PID="$ISOLATED_GUI_PID"
APP_PID="$FATAL_PID"
# 致命終了を待つ（上限つき。生き残ったら検査を落として自分で片付ける）
FATAL_ALIVE=1
for i in $(seq 1 100); do
  if ! kill -0 "$FATAL_PID" 2>/dev/null; then FATAL_ALIVE=0; break; fi
  sleep 0.2
done
if [ "$FATAL_ALIVE" -eq 0 ]; then
  wait "$FATAL_PID" 2>/dev/null
  RC=$?
  APP_PID=""
  pass "⑥ 端末ペインを 1 つも起こせないと終了する（終了コード ${RC}）"
else
  fail "⑥ 端末ペインを 1 つも起こせないのに終了しない"
  stop_app
fi
check_eq "⑥ 「1 つも起動できない」が persist.log に残る" "1" \
  "$(grep -c 'fatal: 復元したペインを 1 つも起動できない' "$PLOG" | tr -d ' ')"
check_eq "⑥ 個別の失敗行も残る（どのペインが起きなかったか）" "1" \
  "$(grep -c '復元失敗（ペイン' "$PLOG" | tr -d ' ')"
check_eq "⑥ 内訳の 1 行目は出ない（致命終了と二重に報告しない）" "0" \
  "$(grep 'tmux 再 attach' "$PLOG" | grep -c '復元成功' | tr -d ' ')"

echo
echo "===================="
echo "PASS: $PASS / FAIL: $FAIL"
echo "===================="
[ "$FAIL" -eq 0 ]
