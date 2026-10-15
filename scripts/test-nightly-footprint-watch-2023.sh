#!/bin/bash
# tako:run bash scripts/test-nightly-footprint-watch-2023.sh
# test-nightly-footprint-watch-2023.sh — footprint の回帰の見張り（#2023）のモックテスト
#
# 本番には一切触らない: HOME・履歴の置き場・ロックは一時ディレクトリ、計測スクリプト・tako・
# cargo・osascript は代役、push 先は一時ディレクトリの bare リポ。GUI も cargo も起こさない。
#
#   A. 判定（scripts/lib/footprint-watch.py）を合成した履歴で: 悪化の検出 / 基準づくり中 /
#      描画なしの夜は描画の値を比べない / 未実測・条件崩れは判定しない / 注入の行は基準に入らない /
#      描画回数の上限 / 続いている悪化は重ねない / 列が増えても履歴が壊れない
#   B. 見張り（scripts/nightly-footprint-watch.sh）を通しで: 起票は 1 件だけ（続く夜は重ねない・
#      新しい項目は同じ 1 件を書き換える）/ 蓋閉じ（終了コード 4）・時間切れ・ビルドの時間切れは
#      未実測で起票しない / 起票できない夜は状態を動かさず次の夜にやり直す / 測り済みは飛ばす /
#      worktree とロックを残さない
#   C. 夜間リリースのフック: 見張りは後始末とロックの解放の後・リリースの後に回り、
#      見張りの結果（失敗・未実測・遅い）で夜間リリースの終了コードが変わらない /
#      dry-run・多重起動・TAKO_NIGHTLY_FOOTPRINT=0 では回らない
#
# ここが落ちるということは、夜の見張りが「鳴らない」「毎晩積む」「リリースを壊す」のどれかへ戻ったという意味。
# launchd と同じ /bin/bash（3.2）で走らせる。
set -uo pipefail

cd "$(dirname "$0")/.."
REPO_ROOT=$PWD
PY="$REPO_ROOT/scripts/lib/footprint-watch.py"
PASS=0
FAIL=0
TMP_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/tako-fpw-test-XXXXXX")"
trap 'rm -rf "$TMP_ROOT"' EXIT

ok() { echo "  PASS: $1"; PASS=$((PASS + 1)); }
ng() { echo "  FAIL: $1"; FAIL=$((FAIL + 1)); }
assert_eq() { if [ "$2" = "$3" ]; then ok "$1"; else ng "$1 (expected=[$2], actual=[$3])"; fi; }
assert_contains() { if printf '%s' "$2" | grep -qF -- "$3"; then ok "$1"; else ng "$1 (not found: '$3')"; fi; }
assert_not_contains() { if printf '%s' "$2" | grep -qF -- "$3"; then ng "$1 (found but should not: '$3')"; else ok "$1"; fi; }

# 計測スクリプトの列は本物から取る（列を足したら自動で追従する）
HEADER="$(sed -n 's/^HEADER="\(.*\)"$/\1/p' "$REPO_ROOT/scripts/measure-footprint-2011.sh")"
[ -n "$HEADER" ] || { echo "measure-footprint-2011.sh の HEADER を読めない"; exit 1; }

# std の 1 行（列=値 の上書きを引数で受ける）。値は S0 の 2 回目に近いもの
fake_row() {
  python3 - "$HEADER" "$@" <<'PY'
import sys
header = sys.argv[1].split("\t")
d = dict(scenario="persist:30std", mode="persist", panes="30", front="4", back_tab="26", bg="0", previews="5",
         startup_ms="900", footprint_mb="345.0", malloc_mb="150.0", gfx_mb="60.0", other_mb="135.0", tmux_mb="106.0",
         threads="48", children="31", cpu_pct="0.85", child_cpu_pct="0.20", energy_mw="10.0", wakeups_s="95.0",
         body_renders_s="0.20", flushes_s="0.10", busy_footprint_mb="350.0", busy_malloc_mb="150.0", busy_gfx_mb="65.0",
         busy_cpu_pct="6.00", busy_child_cpu_pct="0.50", busy_energy_mw="80.0", busy_body_renders_s="29.9",
         busy_flushes_s="100.0", filled="30/30", drawn="あり", lid="開", locked="-", env="- stub-claude")
for kv in sys.argv[2:]:
    k, v = kv.split("=", 1)
    d[k] = v
print("\t".join(header))
print("\t".join(d.get(k, "-") for k in header))
PY
}

# 履歴へ 1 行足す（A 用）: <履歴> <日付> <sha> <status> [列=値 …]
add_row() {
  local h="$1" date="$2" sha="$3" status="$4" row
  shift 4
  row="$TMP_ROOT/row-$RANDOM.tsv"
  if [ "$status" = ok ]; then fake_row "$@" > "$row"; else : > "$row"; fi
  python3 "$PY" record --history "$h" --date "$date" --sha "$sha" --version 0.8.40 --status "$status" \
    --reason "$([ "$status" = ok ] && echo - || echo 'tako-vd を用意できない')" --elapsed 240 --row "$row"
}
judge_kv() { # <履歴> <状態> <出力 dir> [追加の引数] → 出力 dir/result
  local h="$1" st="$2" out="$3"
  shift 3
  rm -rf "$out"
  python3 "$PY" judge --history "$h" --state "$st" --out-dir "$out" "$@"
}
kv_of() { sed -n "s/^$2=//p" "$1/result" | tail -1; }

# ============================================================================
echo "A. 判定（合成した履歴）"
# ============================================================================
A="$TMP_ROOT/a"
mkdir -p "$A"

echo "A1: 基準 3 回 + 出力中の footprint +30% → 悪化として 1 項目だけ挙がる"
H="$A/h1.tsv"
add_row "$H" "2026-10-10 05:30" aaaa1111 ok
add_row "$H" "2026-10-11 05:30" aaaa2222 ok busy_footprint_mb=352.0
add_row "$H" "2026-10-12 05:30" aaaa3333 ok busy_footprint_mb=348.0
add_row "$H" "2026-10-13 05:30" aaaa4444 ok busy_footprint_mb=455.0
judge_kv "$H" "$A/none" "$A/o1"
assert_eq "判定は regression" "regression" "$(kv_of "$A/o1" verdict)"
assert_eq "起票する" "report" "$(kv_of "$A/o1" action)"
assert_eq "挙がるのは出力中の footprint だけ" "busy_footprint_mb" "$(kv_of "$A/o1" metrics)"
assert_contains "title は見張りの目印で始まる" "$(cat "$A/o1/title.txt")" "[footprint 回帰] 出力中の footprint +30.0%"
assert_contains "本文に基準と上限の表" "$(cat "$A/o1/body.md")" "| 出力中の footprint | 455.0 MB | 350.0 MB | 420.0 MB |"
assert_contains "本文に再計測の最簡コマンド" "$(cat "$A/o1/body.md")" "bash scripts/measure-footprint-2011.sh"
assert_eq "前に測れたコミット" "aaaa3333" "$(kv_of "$A/o1" prev_sha)"

echo "A2: 閾値の内側（+19%）は鳴らない / 閾値の倍率 0.5 にすると鳴る（閾値を下げる注入）"
H="$A/h2.tsv"
add_row "$H" "2026-10-10 05:30" b1 ok
add_row "$H" "2026-10-11 05:30" b2 ok
add_row "$H" "2026-10-12 05:30" b3 ok
add_row "$H" "2026-10-13 05:30" b4 ok busy_footprint_mb=416.0
judge_kv "$H" "$A/none" "$A/o2"
assert_eq "+19% は clean" "clean" "$(kv_of "$A/o2" verdict)"
judge_kv "$H" "$A/none" "$A/o2b" --scale 0.5
assert_eq "倍率 0.5 で regression" "regression" "$(kv_of "$A/o2b" verdict)"

echo "A3: 基準が 2 回しか無い → 基準づくり中（起票しない）"
H="$A/h3.tsv"
add_row "$H" "2026-10-10 05:30" c1 ok
add_row "$H" "2026-10-11 05:30" c2 ok
add_row "$H" "2026-10-12 05:30" c3 ok busy_footprint_mb=900.0
judge_kv "$H" "$A/none" "$A/o3"
assert_eq "baseline" "baseline" "$(kv_of "$A/o3" verdict)"
assert_eq "状態は動かさない" "keep" "$(kv_of "$A/o3" action)"

echo "A4: 描画なしの夜（drawn=なし）は描画の値が桁違いでも比べず、描画なしの夜どうしで残りを比べる"
H="$A/h4.tsv"
add_row "$H" "2026-10-10 05:30" d1 ok
add_row "$H" "2026-10-11 05:30" d2 ok
add_row "$H" "2026-10-12 05:30" d3 ok
add_row "$H" "2026-10-13 05:30" d4 ok drawn=なし locked=ロック busy_footprint_mb=900.0 busy_cpu_pct=30.0 cpu_pct=5.0 busy_body_renders_s=0.0
judge_kv "$H" "$A/none" "$A/o4"
assert_eq "描画ありの基準しか無いので基準づくり中（誤検出しない）" "baseline" "$(kv_of "$A/o4" verdict)"
assert_contains "出力中の footprint は比べないと出る" "$(cat "$A/o4/lines")" "出力中の footprint: 900.0 MB（描画なしの夜なので比べない）"
add_row "$H" "2026-10-14 05:30" d5 ok drawn=なし
add_row "$H" "2026-10-15 05:30" d6 ok drawn=なし
add_row "$H" "2026-10-16 05:30" d7 ok drawn=なし busy_footprint_mb=999.0 busy_cpu_pct=40.0 busy_body_renders_s=0.0
judge_kv "$H" "$A/none" "$A/o4b"
assert_eq "描画なしの基準 3 回が揃えば clean（描画の値は無視）" "clean" "$(kv_of "$A/o4b" verdict)"
add_row "$H" "2026-10-17 05:30" d8 ok drawn=なし footprint_mb=520.0
judge_kv "$H" "$A/none" "$A/o4c"
assert_eq "描画なしの夜でも放置の footprint の悪化は拾う" "footprint_mb" "$(kv_of "$A/o4c" metrics)"

echo "A5: 未実測の行・条件が崩れた行は判定しない（状態も動かさない）"
H="$A/h5.tsv"
add_row "$H" "2026-10-10 05:30" e1 ok
add_row "$H" "2026-10-11 05:30" e2 ok
add_row "$H" "2026-10-12 05:30" e3 ok
add_row "$H" "2026-10-13 05:30" e4 unmeasured
printf 'open_metrics=busy_footprint_mb\ntodo=u-7\nsince=2026-10-01\n' > "$A/st5"
judge_kv "$H" "$A/st5" "$A/o5"
assert_eq "未実測" "unmeasured" "$(kv_of "$A/o5" verdict)"
assert_eq "状態は keep" "keep" "$(kv_of "$A/o5" action)"
assert_eq "続いている悪化はそのまま" "busy_footprint_mb" "$(kv_of "$A/o5" open_metrics)"
add_row "$H" "2026-10-14 05:30" e5 ok filled=27/30 busy_footprint_mb=100.0
judge_kv "$H" "$A/none" "$A/o5b"
assert_eq "中身が入り切らない行は condition" "condition" "$(kv_of "$A/o5b" verdict)"

echo "A6: 注入の行（env が標準でない）は判定されるが、次の夜の基準には入らない"
H="$A/h6.tsv"
add_row "$H" "2026-10-10 05:30" f1 ok
add_row "$H" "2026-10-11 05:30" f2 ok
add_row "$H" "2026-10-12 05:30" f3 ok
add_row "$H" "2026-10-13 05:30" f4 ok "env=TAKO_2009_LEGACY=1 stub-claude" footprint_mb=700.0 busy_footprint_mb=700.0
judge_kv "$H" "$A/none" "$A/o6"
assert_eq "注入の行は悪化として挙がる" "regression" "$(kv_of "$A/o6" verdict)"
assert_contains "本文に足した env を書く" "$(cat "$A/o6/body.md")" "足した env: \`TAKO_2009_LEGACY=1\`（この行は基準に入らない）"
assert_contains "本文の基準の回数は実際の回数" "$(cat "$A/o6/body.md")" "同じ条件の直近 3 回の中央値"
add_row "$H" "2026-10-14 05:30" f5 ok
judge_kv "$H" "$A/none" "$A/o6b"
assert_eq "注入の後の標準の夜は clean（注入の値が基準に混ざらない）" "clean" "$(kv_of "$A/o6b" verdict)"
assert_eq "基準は標準の 3 回" "3" "$(kv_of "$A/o6b" baseline_n)"

echo "A7: 出力中の描画回数が #1979 の上限（30 + ぶれ 1.5）を超えたら基準が無くても挙がる"
H="$A/h7.tsv"
add_row "$H" "2026-10-10 05:30" g1 ok busy_body_renders_s=58.0
judge_kv "$H" "$A/none" "$A/o7"
assert_eq "上限超えは regression" "busy_body_renders_s" "$(kv_of "$A/o7" metrics)"
H="$A/h7b.tsv"
add_row "$H" "2026-10-10 05:30" g2 ok busy_body_renders_s=30.4
judge_kv "$H" "$A/none" "$A/o7b"
assert_eq "S0 の実測の上端 30.4 は鳴らない" "baseline" "$(kv_of "$A/o7b" verdict)"

echo "A8: 続いている悪化は重ねない / 新しい項目だけが起票を起こす / 回帰なしで解消"
printf 'open_metrics=busy_footprint_mb\ntodo=u-3\nsince=2026-10-13 05:30\n' > "$A/st8"
judge_kv "$A/h1.tsv" "$A/st8" "$A/o8"
assert_eq "同じ項目の悪化は continuing" "continuing" "$(kv_of "$A/o8" action)"
H="$A/h8.tsv"
add_row "$H" "2026-10-10 05:30" h1 ok
add_row "$H" "2026-10-11 05:30" h2 ok
add_row "$H" "2026-10-12 05:30" h3 ok
add_row "$H" "2026-10-13 05:30" h4 ok busy_footprint_mb=455.0 cpu_pct=1.40
judge_kv "$H" "$A/st8" "$A/o8b"
assert_eq "放置の CPU が新しく悪化したら report" "report" "$(kv_of "$A/o8b" action)"
assert_eq "新しい項目は放置の CPU だけ" "cpu_pct" "$(kv_of "$A/o8b" new_metrics)"
assert_eq "続いている項目と合わせて持つ" "busy_footprint_mb,cpu_pct" "$(kv_of "$A/o8b" open_metrics)"
assert_contains "放置の CPU はポイントで言う" "$(cat "$A/o8b/title.txt")" "放置の CPU +0.55 ポイント"
judge_kv "$A/h2.tsv" "$A/st8" "$A/o8c"
assert_eq "回帰なしの夜は clear" "clear" "$(kv_of "$A/o8c" action)"

echo "A9: 計測スクリプトが列を足しても履歴が壊れない（和集合で揃え、古い行は -）"
H="$A/h9.tsv"
add_row "$H" "2026-10-10 05:30" i1 ok
row="$TMP_ROOT/row-extra.tsv"
fake_row | awk -F'\t' 'BEGIN{OFS="\t"} NR==1{print $0, "new_col"; next} {print $0, "42"}' > "$row"
python3 "$PY" record --history "$H" --date "2026-10-11 05:30" --sha i2 --status ok --elapsed 200 --row "$row"
assert_eq "見出しに新しい列" "new_col" "$(head -1 "$H" | tr '\t' '\n' | tail -1)"
assert_eq "古い行は -" "-" "$(sed -n 2p "$H" | awk -F'\t' '{print $NF}')"
assert_eq "新しい行は値" "42" "$(sed -n 3p "$H" | awk -F'\t' '{print $NF}')"

echo "A10: measured は「描画ありで測り済み」だけを済みと読む / find-open は目印の未完了だけ"
H="$A/h10.tsv"
add_row "$H" "2026-10-10 05:30" j1 ok drawn=なし
python3 "$PY" measured --history "$H" --sha j1; assert_eq "描画なしだけなら測り直す" "1" "$?"
add_row "$H" "2026-10-11 05:30" j1 ok
python3 "$PY" measured --history "$H" --sha j1; assert_eq "描画ありがあれば済み" "0" "$?"
list='{"tasks":[{"id":"u-1","title":"[footprint 回帰] 古い","status":"done"},{"id":"u-2","title":"別の件","status":"open"},{"id":"u-5","title":"[footprint 回帰] x","status":"open"},{"id":"u-9","title":"[footprint 回帰] y","status":"open"}]}'
assert_eq "完了・別件を除いた最初の 1 件" "u-5" "$(printf '%s' "$list" | python3 "$PY" find-open)"
assert_eq "状態の id が未完了ならそれ" "u-9" "$(printf '%s' "$list" | python3 "$PY" find-open --prefer u-9)"
assert_eq "状態の id が完了済みなら別の未完了" "u-5" "$(printf '%s' "$list" | python3 "$PY" find-open --prefer u-1)"

echo "A11: ブランチの行（ref が origin/main 以外）は main の行と比べられるが、main の基準には入らない"
H="$A/h11.tsv"
add_row "$H" "2026-10-10 05:30" k1 ok
add_row "$H" "2026-10-11 05:30" k2 ok
add_row "$H" "2026-10-12 05:30" k3 ok
row="$TMP_ROOT/row-branch.tsv"
fake_row busy_footprint_mb=700.0 > "$row"
python3 "$PY" record --history "$H" --date "2026-10-12 12:00" --sha kb --ref test/branch --status ok --elapsed 240 --row "$row"
judge_kv "$H" "$A/none" "$A/o11"
assert_eq "ブランチの悪化は main の基準と比べて挙がる" "busy_footprint_mb" "$(kv_of "$A/o11" metrics)"
add_row "$H" "2026-10-13 05:30" k4 ok
judge_kv "$H" "$A/none" "$A/o11b"
assert_eq "次の main の夜の基準は main の 3 回だけ" "3" "$(kv_of "$A/o11b" baseline_n)"
assert_eq "前に測れたコミットにブランチを数えない" "k3" "$(kv_of "$A/o11b" prev_sha)"

# ============================================================================
echo
echo "B. 見張りの通し（計測・tako・cargo は代役）"
# ============================================================================
B="$TMP_ROOT/b"
mkdir -p "$B/bin" "$B/home"
for s in osascript; do printf '#!/bin/sh\nexit 0\n' > "$B/bin/$s"; chmod +x "$B/bin/$s"; done
# cargo の代役: 呼ばれた引数を残す。FAKE_CARGO_SLEEP 秒だけ眠る（ビルドの時間切れの注入）
cat > "$B/bin/cargo" <<'SH'
#!/bin/sh
echo "cargo $*" >> "$FAKE_LOG"
[ -n "${FAKE_CARGO_SLEEP:-}" ] && sleep "$FAKE_CARGO_SLEEP"
exit 0
SH
chmod +x "$B/bin/cargo"
# 計測スクリプトの代役: FAKE_MEASURE（ok / nodisplay / hang / fail / norow）・FAKE_VALUES（列=値 …）
cat > "$B/measure.sh" <<'SH'
#!/bin/bash
echo "$$" > "$FAKE_DIR/measure.pid"
{ echo "args=$*"; echo "no_queue=${TAKO_NO_QUEUE:-}"; echo "stub=${TAKO_FOOTPRINT_STUB_CLAUDE:-}"
  echo "tako_bin=${TAKO_BIN:-}"; echo "socket=${TAKO_SOCKET:-}"; } > "$FAKE_DIR/measure.env"
case "${FAKE_MEASURE:-ok}" in
  nodisplay) echo "未実測: tako-vd を用意できないので検証用 GUI を開かない（テストの注入）" >&2; exit 4 ;;
  hang) trap 'echo term > "$FAKE_DIR/measure.term"; exit 130' TERM; while :; do sleep 0.2; done ;;
  fail) echo "ERROR: テストの失敗" >&2; exit 1 ;;
  norow) exit 0 ;;
esac
label="${TAKO_FOOTPRINT_ENV:--}"
[ "${TAKO_FOOTPRINT_STUB_CLAUDE:-0}" = 1 ] && label="$label stub-claude"
# shellcheck disable=SC2086
bash "$FAKE_DIR/fake_row.sh" "env=$label" ${FAKE_VALUES:-} > "$TAKO_FOOTPRINT_OUT"
echo "  窓: ディスプレイ指定 tako-vd: name で解決 → tako-vd（テスト）"
echo "  persist:30std  放置 345.0 MB（テスト）"
SH
chmod +x "$B/measure.sh"
# fake_row を代役の計測から呼べる形で置く（HEADER は本物から）
{ echo '#!/bin/bash'; declare -f fake_row; printf 'HEADER=%q\n' "$HEADER"; echo 'fake_row "$@"'; } > "$B/fake_row.sh"
# tako の代役: todo list / add / update を JSON のファイルで持つ。FAKE_TODO_DOWN=1 で「tako の外」
cat > "$B/tako" <<'PY'
#!/usr/bin/env python3
import json, os, sys
store = os.path.join(os.environ["FAKE_DIR"], "todo.json")
args = sys.argv[1:]
with open(os.path.join(os.environ["FAKE_DIR"], "tako.log"), "a") as f:
    f.write(" ".join(args if args[:1] == ["queue"] else args[:3]) + "\n")
if os.environ.get("FAKE_TODO_DOWN") == "1":
    print("tako の外で実行されている", file=sys.stderr)
    sys.exit(1)
if args[:1] == ["queue"]:
    # 順番待ちを知っている tako のふり（`queue --help` に --max-wait・`queue <コマンド>` は記録して走らせる）
    if args[1:] == ["--help"]:
        print("Usage: tako queue [--max-wait <時間>] <コマンド>")
        sys.exit(0)
    os.execvp(args[1], args[1:])
data = json.load(open(store)) if os.path.exists(store) else {"tasks": []}
def opt(name):
    return args[args.index(name) + 1] if name in args else None
if args[:2] == ["todo", "list"]:
    print(json.dumps({"tasks": [t for t in data["tasks"] if t["status"] == "open"]}, ensure_ascii=False))
elif args[:2] == ["todo", "add"]:
    t = {"id": "u-%d" % (len(data["tasks"]) + 1), "title": args[2], "body": opt("--body"), "kind": opt("--kind"),
         "project": opt("--project"), "attach": opt("--attach"), "status": "open"}
    data["tasks"].append(t)
    print(json.dumps(t, ensure_ascii=False))
elif args[:2] == ["todo", "update"]:
    t = next(t for t in data["tasks"] if t["id"] == args[2])
    t["title"], t["body"] = opt("--title"), opt("--body")
    print(json.dumps(t, ensure_ascii=False))
elif args[:2] == ["todo", "done"]:
    next(t for t in data["tasks"] if t["id"] == args[2])["status"] = "done"
json.dump(data, open(store, "w"), ensure_ascii=False)
PY
chmod +x "$B/tako"

# 見張りが動く一時リポ（origin つき）。見張りと補助と queue.sh を置き、origin/main に積む
git init --quiet --bare "$B/origin.git"
git init --quiet "$B/repo"
git -C "$B/repo" symbolic-ref HEAD refs/heads/main
git -C "$B/repo" config user.name "tako fpw test"
git -C "$B/repo" config user.email "fpw@example.invalid"
git -C "$B/repo" config commit.gpgsign false
git -C "$B/repo" remote add origin "$B/origin.git"
mkdir -p "$B/repo/scripts/lib" "$B/repo/target/release"
cp "$REPO_ROOT/scripts/nightly-footprint-watch.sh" "$B/repo/scripts/"
cp "$REPO_ROOT/scripts/lib/footprint-watch.py" "$REPO_ROOT/scripts/lib/queue.sh" "$B/repo/scripts/lib/"
# build-pwa.sh の代役（呼ばれた引数を残す。cargo より先に呼ばれることを B1 で見る）
printf '#!/bin/sh\necho "build-pwa $*" >> "$FAKE_LOG"\n' > "$B/repo/scripts/build-pwa.sh"
printf '/target\n' > "$B/repo/.gitignore"
printf '[workspace.package]\nversion = "0.8.40"\n' > "$B/repo/Cargo.toml"
git -C "$B/repo" add -A
git -C "$B/repo" commit --quiet -m "init"
git -C "$B/repo" push --quiet origin HEAD:main
new_commit() {
  echo "$1" >> "$B/repo/NOTES.md"
  git -C "$B/repo" add -A
  git -C "$B/repo" commit --quiet -m "[改善] $1"
  git -C "$B/repo" push --quiet origin HEAD:main
}

# 1 晩ぶん走らせる（env は launchd に寄せて HOME と PATH だけ + 代役の指定）
WATCH_RC=0
night() {
  WATCH_RC=0
  env -i HOME="$B/home" PATH="$B/bin:/usr/bin:/bin:/usr/sbin:/sbin:/opt/homebrew/bin" \
    FAKE_DIR="$B" FAKE_LOG="$B/calls.log" TAKO_NO_QUEUE=1 \
    TAKO_FOOTPRINT_WATCH_MEASURE="$B/measure.sh" TAKO_FOOTPRINT_WATCH_TAKO="$B/tako" \
    "$@" /bin/bash "$B/repo/scripts/nightly-footprint-watch.sh" > "$B/out.txt" 2>&1 || WATCH_RC=$?
}
HIST="$B/home/.claude-orchestrator/state/tako-footprint/history.tsv"
STATEF="$B/home/.claude-orchestrator/state/tako-footprint/state"
todo_count() { python3 -c 'import json,sys; print(len(json.load(open(sys.argv[1]))["tasks"]))' "$B/todo.json" 2>/dev/null || echo 0; }
rows_of() { if [ -f "$HIST" ]; then tail -n +2 "$HIST" | grep -c . || true; else echo 0; fi; }
wt_count() { git -C "$B/repo" worktree list | grep -c .; }

echo "B1: 1 晩目 → 測って基準づくり中・計測へは隔離の印だけを渡す・worktree とロックを残さない"
new_commit n1
night
assert_eq "終了コード 0" "0" "$WATCH_RC"
assert_contains "基準づくり中" "$(cat "$B/out.txt")" "基準づくり中"
assert_contains "窓の行（tako-vd の上）を流す" "$(cat "$B/out.txt")" "窓: ディスプレイ指定 tako-vd"
assert_contains "ビルドを release で通す" "$(cat "$B/calls.log")" "cargo build --release -q -p tako-cli -p tako-app"
assert_eq "cargo より先に PWA の dist を作る（2 回目のビルドを空振りにする）" "build-pwa --if-missing|cargo build --release -q -p tako-cli -p tako-app" "$(head -2 "$B/calls.log" | tr '\n' '|' | sed 's/|$//')"
assert_eq "計測は std" "args=std" "$(sed -n 1p "$B/measure.env")"
assert_eq "計測の中では並び直さない" "no_queue=1" "$(sed -n 2p "$B/measure.env")"
assert_eq "偽の claude で回す（子の CPU を claude CLI の版で揺らさない）" "stub=1" "$(sed -n 3p "$B/measure.env")"
assert_eq "呼び出し元の TAKO_BIN を渡さない" "tako_bin=" "$(sed -n 4p "$B/measure.env")"
assert_eq "履歴に 1 行" "1" "$(rows_of)"
assert_eq "worktree を残さない" "1" "$(wt_count)"
assert_eq "ロックを残さない" "no" "$([ -d "$B/home/.claude-orchestrator/locks/tako-footprint-watch.lock" ] && echo yes || echo no)"
assert_not_contains "ログにホームの実パスを出さない" "$(cat "$B/out.txt")" "$B/home"

echo "B2: 同じコミットの夜 → 測り済みで飛ばす（計測を起こさない）"
rm -f "$B/measure.pid"
night
assert_eq "終了コード 0" "0" "$WATCH_RC"
assert_contains "測り済み" "$(cat "$B/out.txt")" "測り済み"
assert_eq "計測を起こさない" "no" "$([ -f "$B/measure.pid" ] && echo yes || echo no)"

echo "B3: 基準 3 回の後に出力中の footprint +30% → tako todo に 1 件"
new_commit n2; night
new_commit n3; night
new_commit n4; night FAKE_VALUES="busy_footprint_mb=455.0"
assert_eq "終了コード 0" "0" "$WATCH_RC"
assert_contains "載せた" "$(cat "$B/out.txt")" "tako todo へ載せた（u-1）"
assert_eq "todo は 1 件" "1" "$(todo_count)"
title="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["tasks"][0]["title"])' "$B/todo.json")"
assert_contains "title" "$title" "[footprint 回帰] 出力中の footprint +30.0%"
body="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["tasks"][0]["body"])' "$B/todo.json")"
assert_contains "本文に前のコミットからの変更件数" "$body" "からの変更: 1 件"
assert_eq "種類は review" "review" "$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["tasks"][0]["kind"])' "$B/todo.json")"
assert_contains "状態に todo の id" "$(cat "$STATEF")" "todo=u-1"

echo "B4: 次の夜も同じ悪化 → 重ねて積まない（add も update も呼ばない）"
: > "$B/tako.log"
new_commit n5; night FAKE_VALUES="busy_footprint_mb=455.0"
assert_eq "終了コード 0" "0" "$WATCH_RC"
assert_contains "重ねない" "$(cat "$B/out.txt")" "重ねて載せない"
assert_eq "todo は 1 件のまま" "1" "$(todo_count)"
assert_not_contains "add を呼ばない" "$(cat "$B/tako.log")" "todo add"

echo "B5: 新しい項目（放置の CPU）も悪化 → 同じ 1 件を書き換える"
new_commit n6; night FAKE_VALUES="busy_footprint_mb=455.0 cpu_pct=1.60"
assert_contains "書き換えた" "$(cat "$B/out.txt")" "未完了の u-1 を書き換えた"
assert_eq "todo は 1 件のまま" "1" "$(todo_count)"
title="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["tasks"][0]["title"])' "$B/todo.json")"
assert_contains "title に放置の CPU" "$title" "放置の CPU"

echo "B6: 蓋閉じ（計測が終了コード 4）→ 未実測で抜ける・起票しない・状態は動かさない"
cp "$STATEF" "$B/state.before"
: > "$B/tako.log"
new_commit n7; night FAKE_MEASURE=nodisplay
assert_eq "終了コード 3（未実測）" "3" "$WATCH_RC"
assert_contains "未実測と言う" "$(cat "$B/out.txt")" "未実測: tako-vd を用意できない"
assert_eq "tako を呼ばない" "" "$(cat "$B/tako.log")"
assert_eq "状態はそのまま" "$(cat "$B/state.before")" "$(cat "$STATEF")"
assert_contains "履歴に未実測の行" "$(tail -1 "$HIST")" "unmeasured"

echo "B7: 計測が固まる → 上限で止めて未実測（計測の後片付けへ TERM が届く）"
new_commit n8; night FAKE_MEASURE=hang TAKO_FOOTPRINT_WATCH_TIMEOUT=2
assert_eq "終了コード 3" "3" "$WATCH_RC"
assert_contains "時間切れ" "$(cat "$B/out.txt")" "計測が上限 2 秒に届いた"
assert_eq "計測へ TERM が届いた（後片付けが走る）" "term" "$(cat "$B/measure.term" 2>/dev/null)"
pid="$(cat "$B/measure.pid")"
assert_eq "計測のプロセスは残らない" "gone" "$(kill -0 "$pid" 2>/dev/null && echo alive || echo gone)"
assert_eq "worktree を残さない" "1" "$(wt_count)"

echo "B8: ビルドが上限に届く → 未実測（計測を起こさない）"
rm -f "$B/measure.pid"
new_commit n9; night FAKE_CARGO_SLEEP=5 TAKO_FOOTPRINT_WATCH_BUILD_TIMEOUT=1
assert_eq "終了コード 3" "3" "$WATCH_RC"
assert_contains "ビルドの時間切れ" "$(cat "$B/out.txt")" "ビルドが上限に届いた"
assert_eq "計測を起こさない" "no" "$([ -f "$B/measure.pid" ] && echo yes || echo no)"

echo "B9: 描画なしの夜（drawn=なし）→ 描画の値が桁違いでも起票しない"
: > "$B/tako.log"
new_commit n10; night FAKE_VALUES="drawn=なし locked=ロック busy_footprint_mb=999.0 busy_cpu_pct=30.0 busy_body_renders_s=0.0"
assert_eq "終了コード 0" "0" "$WATCH_RC"
assert_eq "tako を呼ばない" "" "$(cat "$B/tako.log")"

echo "B10: 回帰なしの夜 → 見張りの状態を消す（todo は残す = 人が片付ける）"
new_commit n11; night
assert_contains "解消" "$(cat "$B/out.txt")" "基準の範囲へ戻った"
assert_eq "状態ファイルが消えた" "no" "$([ -f "$STATEF" ] && echo yes || echo no)"
assert_eq "todo は残る" "1" "$(todo_count)"

echo "B11: tako todo へ届かない夜 → 終了コード 1・状態を動かさない → 届く夜にやり直して載せる"
FAKE_DIR="$B" python3 "$B/tako" todo done u-1 >/dev/null
new_commit n12; night FAKE_VALUES="threads=60" FAKE_TODO_DOWN=1
assert_eq "終了コード 1" "1" "$WATCH_RC"
assert_contains "載せられない" "$(cat "$B/out.txt")" "tako todo へ載せられない"
assert_eq "状態は作らない" "no" "$([ -f "$STATEF" ] && echo yes || echo no)"
new_commit n13; night FAKE_VALUES="threads=60"
assert_contains "次の夜に載せた（前の 1 件は完了済みなので新しく）" "$(cat "$B/out.txt")" "tako todo へ載せた（u-2）"

echo "B12: 計測が結果の行を出さずに 0 で終わる → 失敗（起票しない）"
new_commit n14; night FAKE_MEASURE=norow
assert_eq "終了コード 1" "1" "$WATCH_RC"
assert_contains "結果の行なし" "$(cat "$B/out.txt")" "結果の行なし"

echo "B13: --status は読むだけ（履歴・判定・載せた悪化）"
before="$(cksum < "$HIST")"
env -i HOME="$B/home" PATH="/usr/bin:/bin:/opt/homebrew/bin" /bin/bash "$B/repo/scripts/nightly-footprint-watch.sh" --status > "$B/status.txt" 2>&1
assert_contains "直近の行" "$(cat "$B/status.txt")" "直近の 10 回"
assert_contains "載せた悪化" "$(cat "$B/status.txt")" "載せた悪化: threads（u-2"
assert_eq "履歴を書き換えない" "$before" "$(cksum < "$HIST")"
assert_not_contains "ホームの実パスを出さない" "$(cat "$B/status.txt")" "$B/home"

echo "B15: ビルドは起票に使う tako の順番待ちを通す（PATH の先頭の古い tako = 順番待ちを知らない、に負けない）"
printf '#!/bin/sh\necho "old tako $*" >> "$FAKE_DIR/old-tako.log"\nexit 0\n' > "$B/bin/tako"
chmod +x "$B/bin/tako"
: > "$B/tako.log"
new_commit n16
env -i HOME="$B/home" PATH="$B/bin:/usr/bin:/bin:/usr/sbin:/sbin:/opt/homebrew/bin" \
  FAKE_DIR="$B" FAKE_LOG="$B/calls.log" \
  TAKO_FOOTPRINT_WATCH_MEASURE="$B/measure.sh" TAKO_FOOTPRINT_WATCH_TAKO="$B/tako" \
  /bin/bash "$B/repo/scripts/nightly-footprint-watch.sh" > "$B/out.txt" 2>&1
assert_contains "PWA と cargo build を tako queue 経由で走らせた" "$(cat "$B/tako.log")" "cargo build --release -q -p tako-cli -p tako-app"
assert_contains "並んだのは queue" "$(cat "$B/tako.log")" "queue /bin/bash -c"
assert_eq "古い tako には並ばせない" "" "$(cat "$B/old-tako.log" 2>/dev/null)"
rm -f "$B/bin/tako"

echo "B14: 別の見張りが走っている間は飛ばす（ロック）"
mkdir -p "$B/home/.claude-orchestrator/locks/tako-footprint-watch.lock"
sleep 30 &
holder=$!
echo "$holder" > "$B/home/.claude-orchestrator/locks/tako-footprint-watch.lock/pid"
new_commit n15; night
assert_contains "飛ばす" "$(cat "$B/out.txt")" "別の実行"
kill "$holder" 2>/dev/null; wait "$holder" 2>/dev/null
rm -rf "$B/home/.claude-orchestrator/locks/tako-footprint-watch.lock"

# ============================================================================
echo
echo "C. 夜間リリースのフック（release.sh・見張りは代役）"
# ============================================================================
make_nightly_env() { # <dir> <変更を積むか 1/0> <タグを打つか 1/0>
  local dir="$1" change="$2" tag="$3" stub
  mkdir -p "$dir/home/.local/bin" "$dir/repo/scripts/lib"
  for stub in cargo osascript gh; do printf '#!/bin/sh\nexit 0\n' > "$dir/home/.local/bin/$stub"; chmod +x "$dir/home/.local/bin/$stub"; done
  git init --quiet --bare "$dir/origin.git"
  git init --quiet "$dir/repo"
  git -C "$dir/repo" symbolic-ref HEAD refs/heads/main
  git -C "$dir/repo" config user.name "tako nightly test"
  git -C "$dir/repo" config user.email "nightly@example.invalid"
  git -C "$dir/repo" config commit.gpgsign false
  git -C "$dir/repo" config tag.gpgsign false
  git -C "$dir/repo" remote add origin "$dir/origin.git"
  cp "$REPO_ROOT/scripts/nightly-release.sh" "$dir/repo/scripts/"
  cp "$REPO_ROOT/scripts/lib/nightly-reserve.sh" "$REPO_ROOT/scripts/lib/exit-guard.sh" "$dir/repo/scripts/lib/"
  printf '#!/bin/sh\necho "STUB release.sh $*"\nexit 0\n' > "$dir/repo/scripts/release.sh"
  chmod +x "$dir/repo/scripts/release.sh"
  printf '/target\n/dist\n/scripts/nightly-footprint-watch.sh\n' > "$dir/repo/.gitignore"
  printf '[workspace.package]\nversion = "0.8.40"\n' > "$dir/repo/Cargo.toml"
  printf '# Changelog\n\n## [0.8.40] - 2026-01-01\n\nInitial\n' > "$dir/repo/CHANGELOG.md"
  printf '# dummy lock\n' > "$dir/repo/Cargo.lock"
  git -C "$dir/repo" add -A
  git -C "$dir/repo" commit --quiet -m "init"
  [ "$tag" = 1 ] && git -C "$dir/repo" tag -a v0.8.40 -m v0.8.40
  git -C "$dir/repo" push --quiet origin HEAD:main
  [ "$tag" = 1 ] && git -C "$dir/repo" push --quiet origin v0.8.40
  if [ "$change" = 1 ]; then
    echo change > "$dir/repo/NOTES.md"
    git -C "$dir/repo" add -A
    git -C "$dir/repo" commit --quiet -m "[改善] 変更"
    git -C "$dir/repo" push --quiet origin HEAD:main
  fi
  # 見張りの代役（共有ツリーに置く = 夜間リリースは install_root の見張りを呼ぶ）:
  # 呼ばれた時点でロックが残っているか・origin のタグを記録し、FAKE_WATCH_RC で終わる
  cat > "$dir/repo/scripts/nightly-footprint-watch.sh" <<'SH'
#!/bin/bash
here="$(cd "$(dirname "$0")/.." && pwd)"
{
  echo "called"
  [ -d "$HOME/.claude-orchestrator/locks/tako-nightly-release.lock" ] && echo "home_lock=held" || echo "home_lock=free"
  [ -d "$here/.git/tako-nightly-release.lock" ] && echo "repo_lock=held" || echo "repo_lock=free"
  echo "tags=$(git -C "$here" ls-remote --tags origin 2>/dev/null | grep -c 'refs/tags/v0.8.41$' || true)"
} > "$here/../watch.marker"
echo "代役の見張り（rc=${FAKE_WATCH_RC:-0}）"
[ -n "${FAKE_WATCH_SLEEP:-}" ] && sleep "$FAKE_WATCH_SLEEP"
exit "${FAKE_WATCH_RC:-0}"
SH
  chmod +x "$dir/repo/scripts/nightly-footprint-watch.sh"
}
NIGHT_RC=0
run_nightly_c() { # <dir> [env 名=値 …] [-- 夜間の引数]
  local dir="$1"
  shift
  local -a envs=()
  while [ $# -gt 0 ] && [ "$1" != "--" ]; do envs+=("$1"); shift; done
  [ "${1:-}" = "--" ] && shift
  NIGHT_RC=0
  env -i HOME="$dir/home" ${envs[@]+"${envs[@]}"} /bin/bash "$dir/repo/scripts/nightly-release.sh" "$@" > "$dir/out.txt" 2>&1 || NIGHT_RC=$?
}
marker() { cat "$1/watch.marker" 2>/dev/null || echo "not-called"; }

echo "C1: 変更なしの夜 → 夜間は 0 で抜け、見張りはロックの解放の後に回る"
D="$TMP_ROOT/c1"; make_nightly_env "$D" 0 1
run_nightly_c "$D"
assert_eq "夜間の終了コード 0" "0" "$NIGHT_RC"
assert_contains "変更なし" "$(cat "$D/out.txt")" "SKIP: 変更なし"
assert_contains "見張りが回った" "$(marker "$D")" "called"
assert_contains "HOME 単位のロックは解放済み" "$(marker "$D")" "home_lock=free"
assert_contains "リポジトリ単位のロックも解放済み" "$(marker "$D")" "repo_lock=free"
assert_contains "見張りの出力を時刻つきでログへ" "$(cat "$D/home/.claude-orchestrator/logs/tako-nightly-release.log")" "footprint: 代役の見張り（rc=0）"

echo "C2: 見張りが失敗（1）・未実測（3）・遅い → 夜間の終了コードは 0 のまま"
for rc in 1 3; do
  D="$TMP_ROOT/c2-$rc"; make_nightly_env "$D" 0 1
  run_nightly_c "$D" FAKE_WATCH_RC=$rc
  assert_eq "見張り rc=${rc} でも夜間は 0" "0" "$NIGHT_RC"
  assert_contains "見張りの終了コードを記録" "$(cat "$D/out.txt")" "見張りの終了コード ${rc}"
done
D="$TMP_ROOT/c2-slow"; make_nightly_env "$D" 0 1
run_nightly_c "$D" FAKE_WATCH_SLEEP=2 FAKE_WATCH_RC=1
assert_eq "遅い見張りでも夜間は 0" "0" "$NIGHT_RC"

echo "C3: リリースした夜 → 見張りはタグの push・Release の後に回り、夜間は 0"
D="$TMP_ROOT/c3"; make_nightly_env "$D" 1 1
run_nightly_c "$D" FAKE_WATCH_RC=1
assert_eq "夜間の終了コード 0" "0" "$NIGHT_RC"
assert_contains "リリースした" "$(cat "$D/out.txt")" "完了: v0.8.41"
assert_contains "見張りの時点で v0.8.41 は origin にある" "$(marker "$D")" "tags=1"
log_c3="$(cat "$D/home/.claude-orchestrator/logs/tako-nightly-release.log")"
done_line="$(printf '%s\n' "$log_c3" | grep -n '完了: v0.8.41' | cut -d: -f1)"
watch_line="$(printf '%s\n' "$log_c3" | grep -n 'footprint: 回帰の見張りを始める' | cut -d: -f1)"
assert_eq "見張りはリリースの完了より後" "yes" "$([ -n "$done_line" ] && [ -n "$watch_line" ] && [ "$watch_line" -gt "$done_line" ] && echo yes || echo no)"

echo "C4: 夜間が失敗で抜ける夜（タグが無い = exit 1）→ 見張りは回るが終了コード 1 は保たれる"
D="$TMP_ROOT/c4"; make_nightly_env "$D" 0 0
run_nightly_c "$D" FAKE_WATCH_RC=0
assert_eq "夜間の終了コード 1 のまま" "1" "$NIGHT_RC"
assert_contains "見張りは回る" "$(marker "$D")" "called"

echo "C5: dry-run・TAKO_NIGHTLY_FOOTPRINT=0・多重起動では回らない"
D="$TMP_ROOT/c5a"; make_nightly_env "$D" 1 1
run_nightly_c "$D" -- --dry-run
assert_eq "dry-run は 0" "0" "$NIGHT_RC"
assert_eq "dry-run では回らない" "not-called" "$(marker "$D")"
D="$TMP_ROOT/c5b"; make_nightly_env "$D" 0 1
run_nightly_c "$D" TAKO_NIGHTLY_FOOTPRINT=0
assert_eq "止めた夜は回らない" "not-called" "$(marker "$D")"
assert_contains "止めてあると言う" "$(cat "$D/out.txt")" "見張りは止めてある"
D="$TMP_ROOT/c5c"; make_nightly_env "$D" 0 1
mkdir -p "$D/home/.claude-orchestrator/locks/tako-nightly-release.lock"
sleep 30 &
holder=$!
echo "$holder" > "$D/home/.claude-orchestrator/locks/tako-nightly-release.lock/pid"
run_nightly_c "$D"
kill "$holder" 2>/dev/null; wait "$holder" 2>/dev/null
assert_contains "多重起動で抜けた" "$(cat "$D/out.txt")" "SKIP: 多重起動"
assert_eq "多重起動では回らない" "not-called" "$(marker "$D")"

echo "C6: 見張りのスクリプトが無い install_root（古い main）→ 飛ばして 0"
D="$TMP_ROOT/c6"; make_nightly_env "$D" 0 1
rm -f "$D/repo/scripts/nightly-footprint-watch.sh"
run_nightly_c "$D"
assert_eq "夜間の終了コード 0" "0" "$NIGHT_RC"
assert_contains "飛ばすと言う" "$(cat "$D/out.txt")" "見張りを飛ばす"

echo
echo "結果: PASS ${PASS} / FAIL ${FAIL}"
[ "$FAIL" -eq 0 ]
