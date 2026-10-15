#!/bin/bash
# tako:run bash scripts/test-nightly-queue-path-2119.sh
#
# 夜間リリースの PATH で古い tako が先に当たっても、重い処理が順番待ちを通る（#2119）ことのモックテスト。
#
# launchd から走る scripts/nightly-release.sh は PATH の先頭に ~/.cargo/bin を置く。そこに順番待ち（#1994）を
# 知らない古い tako が居ると、scripts/lib/queue.sh の run_heavy（build-app.sh のリリースビルドが通る）は
# PATH の先頭の 1 本だけを見て「順番待ちを知らない」と決め、並ばずに走らせていた。
#
# 偽の tako 3 種（古い = queue を知らない / 新しい = 順番待ちを知る .app の同梱 CLI の代役 / 固まる）を置き、
# 夜間と同じ PATH（nightly-release.sh の export 行をそのまま評価し、テストの外の本物の tako が居る置き場だけ抜く）で
# run_heavy を build-app.sh と同じ `set -euo pipefail` の /bin/bash（3.2）から呼ぶ。
# **本物の tako・本番の順番待ちには一切触れない**（.app の置き場は TAKO_QUEUE_FALLBACKS で偽物へ差し替える）。
#
#   ① A/B: 修正前（TAKO_2119_LEGACY=1）は並ばない・修正後は .app の tako で並ぶ（low / 3h も渡る）
#   ② PATH の後ろ（~/.local/bin）に知っている tako が居ればそれで並ぶ（.app は見ない）
#   ③ PATH の先頭が知っている tako なら今までどおり（知らせる行も出さない）
#   ④ 知っている tako が無い → 直に走らせ終了コードを返す（夜間を止めない）
#   ⑤ tako が 1 本も無い（CI）→ 直に走らせ、何も知らせない
#   ⑥ 応答しない tako → 上限の秒数で切って直に走らせる / 後ろの知っている tako で並ぶ
#   ⑦ 入れ子（TAKO_QUEUE_JOB）・TAKO_NO_QUEUE=1 は調べもせずに直に走らせる
#   ⑧ 終了コードの素通し・--priority・1 回だけ調べる（2 回目は調べ直さない）
#   ⑨ 既定の候補に /Applications と ~/Applications の tako.app が入る（調べはしない）
#   ⑩ 呼び手の実物 = scripts/build-app.sh のリリースビルド（cargo は代役・終了コード 86 で止める）が
#      夜間の PATH で順番待ちを通る / 知っている tako が無い・応答しないときは直に走る（macOS のみ）
#
# 使い方: bash scripts/test-nightly-queue-path-2119.sh（/bin/bash でも通す）

set -uo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
QUEUE_SH="$REPO_ROOT/scripts/lib/queue.sh"

# 外側の順番待ちの枠の中で走らせても、このテストの run_heavy が入れ子と見なされないように
unset TAKO_QUEUE_JOB TAKO_NO_QUEUE TAKO_QUEUE_PRIORITY TAKO_QUEUE_MAX_WAIT TAKO_QUEUE_FALLBACKS
unset TAKO_QUEUE_PROBE_TIMEOUT TAKO_2119_LEGACY _TAKO_QUEUE_SUPPORTED _TAKO_QUEUE_BIN

PASS=0
FAIL=0
pass() { PASS=$((PASS + 1)); echo "  [OK] $1"; }
fail() { FAIL=$((FAIL + 1)); echo "  [NG] $1"; }
expect_eq() { if [ "$2" = "$3" ]; then pass "$1"; else fail "$1（期待: $2 / 実際: $3）"; fi; }
expect_has() { case "$3" in *"$2"*) pass "$1" ;; *) fail "$1（「$2」が無い: $3）" ;; esac; }
expect_lacks() { case "$3" in *"$2"*) fail "$1（「$2」が在る: $3）" ;; *) pass "$1" ;; esac; }

T="$(mktemp -d /tmp/tk2119-XXXXXX)"
cleanup() {
  # 固まる tako の代役が残っていれば落とす（自分が起こした pid だけ）
  if [ -f "$T/hang.pids" ]; then
    while read -r p; do kill "$p" 2>/dev/null; done < "$T/hang.pids"
  fi
  rm -rf "$T"
}
trap cleanup EXIT

H="$T/home"
APP_TAKO="$T/Applications/tako.app/Contents/MacOS/tako"
mkdir -p "$H/.cargo/bin" "$H/.local/bin" "$(dirname "$APP_TAKO")" "$T/hang" "$T/head"

# 古い tako（v0.2.7 相当）: queue を知らない
cat > "$H/.cargo/bin/tako" <<'SH'
#!/bin/sh
echo "old $*" >> "$TK_LOG"
if [ "${1:-}" = queue ]; then
  echo "error: unrecognized subcommand 'queue'" >&2
  exit 2
fi
exit 0
SH
# 順番待ちを知る tako（.app の同梱 CLI の代役）: 呼ばれ方と env を記録して、枠の中としてコマンドを走らせる
cat > "$T/new-tako" <<'SH'
#!/bin/sh
me="${TK_NAME:-new}"
if [ "${1:-}" = queue ] && [ "${2:-}" = --help ]; then
  echo "probe $me" >> "$TK_LOG"
  echo "Usage: tako queue [OPTIONS] [COMMAND]..."
  echo "      --max-wait <MAX_WAIT>  待ちの上限"
  exit 0
fi
if [ "${1:-}" = queue ]; then
  shift
  echo "queue $me prio=${TAKO_QUEUE_PRIORITY:-} max_wait=${TAKO_QUEUE_MAX_WAIT:-} args=$*" >> "$TK_LOG"
  [ "${1:-}" = --priority ] && shift 2
  TAKO_QUEUE_JOB=42 exec "$@"
fi
exit 0
SH
# 固まる tako: 自分の pid を残して眠り続ける（perl の alarm で切られる）
cat > "$T/hang/tako" <<'SH'
#!/bin/sh
echo "hang $*" >> "$TK_LOG"
echo $$ >> "$TK_HANG_PIDS"
exec sleep 30
SH
chmod +x "$H/.cargo/bin/tako" "$T/new-tako" "$T/hang/tako"
cp "$T/new-tako" "$APP_TAKO"

# 夜間と同じ PATH: nightly-release.sh の export 行を HOME=$H で評価する。
# テストの外の置き場に本物の tako が居れば（この機の /opt/homebrew/bin 等）、そこだけ抜く
PATH_LINE="$(grep -m1 '^export PATH="\$HOME/.cargo/bin' "$REPO_ROOT/scripts/nightly-release.sh")"
if [ -z "$PATH_LINE" ]; then
  echo "nightly-release.sh の PATH の export 行が見つからない" >&2
  exit 1
fi
RAW_PATH="$(HOME="$H" /bin/bash -c "$PATH_LINE"' && printf %s "$PATH"')"
NIGHTLY_PATH=""
DROPPED=""
rest="$RAW_PATH:"
while [ -n "$rest" ]; do
  d="${rest%%:*}"
  rest="${rest#*:}"
  [ -n "$d" ] || continue
  case "$d" in
    "$T"/*) ;;
    *) if [ -e "$d/tako" ]; then DROPPED="$DROPPED $d"; continue; fi ;;
  esac
  NIGHTLY_PATH="${NIGHTLY_PATH:+$NIGHTLY_PATH:}$d"
done

LOG="$T/calls.log"
ERR="$T/err.txt"
# 夜間と同じ条件（HOME・PATH・low・3h・set -euo pipefail の /bin/bash）で run_heavy を 1 回呼ぶ。
# 引数: <PATH> <run_heavy の引数…>。env の追加は呼び手が `env X=Y` で前に付ける
heavy() {
  local p="$1"
  shift
  : > "$LOG"
  env HOME="$H" PATH="$p" TK_LOG="$LOG" TK_HANG_PIDS="$T/hang.pids" \
    TAKO_QUEUE_PRIORITY=low TAKO_QUEUE_MAX_WAIT=3h \
    /bin/bash -euo pipefail -c '. "$1"; shift; run_heavy "$@"' _ "$QUEUE_SH" "$@" 2> "$ERR"
}
JOB='echo "job=${TAKO_QUEUE_JOB:-none}"'

echo "== 前提: 夜間の PATH =="
expect_has "先頭は ~/.cargo/bin（古い tako の置き場）" "$H/.cargo/bin:" "$NIGHTLY_PATH:"
first_tako="$(PATH="$NIGHTLY_PATH" type -P tako)"
expect_eq "PATH で最初に当たる tako は古い tako" "$H/.cargo/bin/tako" "$first_tako"
[ -n "$DROPPED" ] && echo "  （テストの外の本物の tako が居る置き場を抜いた:$(printf '%s' "$DROPPED" | sed "s#${HOME:-/nonexistent}#~#g")）"

echo "== ① A/B: 古い tako が先の夜間の PATH =="
export TAKO_QUEUE_FALLBACKS="$APP_TAKO"
out="$(TAKO_2119_LEGACY=1 heavy "$NIGHTLY_PATH" sh -c "$JOB")"
expect_eq "修正前（TAKO_2119_LEGACY=1）は順番待ちを通らない" "job=none" "$out"
expect_lacks "修正前は .app の tako に並ばない" "queue new" "$(cat "$LOG")"
out="$(heavy "$NIGHTLY_PATH" sh -c "$JOB")"
expect_eq "修正後は順番待ちを通る" "job=42" "$out"
expect_has "修正後は .app の tako で並ぶ（low / 3h も渡る）" "queue new prio=low max_wait=3h args=sh -c" "$(cat "$LOG")"
expect_lacks "古い tako には並ばせない（調べるだけ）" "old queue sh" "$(cat "$LOG")"
expect_has "古い tako を飛ばしたことを 1 行知らせる" "run_heavy: ~/.cargo/bin/tako（順番待ち（#1994）を知らない古い版）を飛ばして $APP_TAKO で並ぶ" "$(cat "$ERR")"

echo "== ② PATH の後ろの ~/.local/bin に知っている tako が居る =="
cp "$T/new-tako" "$H/.local/bin/tako"
out="$(TK_NAME=local heavy "$NIGHTLY_PATH" sh -c "$JOB")"
expect_eq "順番待ちを通る" "job=42" "$out"
expect_has "PATH の順で ~/.local/bin の tako で並ぶ" "queue local" "$(cat "$LOG")"
expect_has "知らせる行は ~/.local/bin/tako を名指す" "を飛ばして ~/.local/bin/tako で並ぶ" "$(cat "$ERR")"
rm -f "$H/.local/bin/tako"

echo "== ③ PATH の先頭が知っている tako（今までどおり）=="
cp "$T/new-tako" "$T/head/tako"
out="$(TK_NAME=head heavy "$T/head:$NIGHTLY_PATH" sh -c "$JOB")"
expect_eq "順番待ちを通る" "job=42" "$out"
expect_eq "先頭の tako だけを調べて並ぶ（古い tako も .app も見ない）" "probe head
queue head prio=low max_wait=3h args=sh -c ${JOB}" "$(cat "$LOG")"
expect_eq "知らせる行を出さない" "" "$(cat "$ERR")"

echo "== ④ 知っている tako が無い（古い tako だけ）=="
rc=0
TAKO_QUEUE_FALLBACKS="$T/nonexistent/tako" heavy "$NIGHTLY_PATH" sh -c 'exit 6' || rc=$?
expect_eq "直に走らせて終了コードを返す" "6" "$rc"
expect_has "並ばずに走らせることを知らせる" "順番待ちを知る tako が見つからないので並ばずに走らせる" "$(cat "$ERR")"
out="$(TAKO_QUEUE_FALLBACKS="" heavy "$NIGHTLY_PATH" sh -c "$JOB")"
expect_eq "TAKO_QUEUE_FALLBACKS が空 = PATH だけを見る" "job=none" "$out"

echo "== ⑤ tako が 1 本も無い（CI）=="
rc=0
TAKO_QUEUE_FALLBACKS="" heavy "/usr/bin:/bin" sh -c 'exit 7' || rc=$?
expect_eq "直に走らせて終了コードを返す" "7" "$rc"
expect_eq "何も知らせない" "" "$(cat "$ERR")"

echo "== ⑥ 応答しない tako =="
start=$(date +%s)
out="$(TAKO_QUEUE_PROBE_TIMEOUT=1 TAKO_QUEUE_FALLBACKS="$T/hang/tako" heavy "$NIGHTLY_PATH" sh -c "$JOB")"
elapsed=$(( $(date +%s) - start ))
expect_eq "固まる .app の tako → 直に走らせる" "job=none" "$out"
if [ "$elapsed" -le 5 ]; then pass "上限（1 秒）で切って先へ進む（${elapsed} 秒）"; else fail "上限で切れていない（${elapsed} 秒）"; fi
expect_has "応答しないことを知らせる（古い tako と並べて名指す）" "~/.cargo/bin/tako（順番待ち（#1994）を知らない古い版）、$T/hang/tako（1 秒応答しない）を飛ばした" "$(cat "$ERR")"
out="$(TAKO_QUEUE_PROBE_TIMEOUT=1 TAKO_QUEUE_FALLBACKS="$APP_TAKO" heavy "$T/hang:/usr/bin:/bin" sh -c "$JOB")"
expect_eq "PATH の先頭が固まる → 後ろの .app の tako で並ぶ" "job=42" "$out"
expect_has "固まった tako を飛ばしたと知らせる" "$T/hang/tako（1 秒応答しない）を飛ばして $APP_TAKO で並ぶ" "$(cat "$ERR")"
sleep 0.2
alive=""
if [ -f "$T/hang.pids" ]; then
  while read -r p; do kill -0 "$p" 2>/dev/null && alive="$alive $p"; done < "$T/hang.pids"
fi
expect_eq "固まる tako を残さない（alarm で終わっている）" "" "$alive"

echo "== ⑦ 入れ子・TAKO_NO_QUEUE =="
out="$(TAKO_QUEUE_JOB=77 heavy "$NIGHTLY_PATH" sh -c "$JOB")"
expect_eq "入れ子は直に走らせる" "job=77" "$out"
expect_eq "入れ子は tako を調べもしない" "" "$(cat "$LOG")"
out="$(TAKO_NO_QUEUE=1 heavy "$NIGHTLY_PATH" sh -c "$JOB")"
expect_eq "TAKO_NO_QUEUE=1 は直に走らせる" "job=none" "$out"
expect_eq "TAKO_NO_QUEUE=1 は tako を調べもしない" "" "$(cat "$LOG")"

echo "== ⑧ 終了コード・--priority・1 回だけ調べる =="
rc=0
heavy "$NIGHTLY_PATH" sh -c 'exit 9' || rc=$?
expect_eq "並んだコマンドの終了コードを返す" "9" "$rc"
heavy "$NIGHTLY_PATH" --priority high sh -c 'exit 0' >/dev/null
expect_has "--priority を tako queue へ渡す" "args=--priority high sh -c" "$(cat "$LOG")"
: > "$LOG"
out="$(env HOME="$H" PATH="$NIGHTLY_PATH" TK_LOG="$LOG" TAKO_QUEUE_FALLBACKS="$APP_TAKO" \
  /bin/bash -euo pipefail -c '. "$1"; run_heavy true; . "$1"; run_heavy true; echo done' _ "$QUEUE_SH" 2>/dev/null)"
expect_eq "2 回とも走る" "done" "$out"
expect_eq "古い tako を調べるのは 1 回だけ（読み込み直しても調べ直さない）" "1" "$(grep -c '^old queue --help' "$LOG")"
expect_eq "知っている tako を調べるのも 1 回だけ" "1" "$(grep -c '^probe new' "$LOG")"

echo "== ⑨ 既定の候補 =="
cands="$(env -u TAKO_QUEUE_FALLBACKS HOME="$H" PATH="/usr/bin:/bin" /bin/bash -c '. "$1"; _tako_queue_candidates' _ "$QUEUE_SH")"
expect_eq "PATH に tako が無ければ .app の 2 か所だけ" "/Applications/tako.app/Contents/MacOS/tako:$H/Applications/tako.app/Contents/MacOS/tako" "$cands"
cands="$(env HOME="$H" PATH="$NIGHTLY_PATH" TAKO_QUEUE_FALLBACKS="$APP_TAKO" /bin/bash -c '. "$1"; _tako_queue_candidates' _ "$QUEUE_SH")"
expect_eq "PATH の tako（上から）の後ろに .app" "$H/.cargo/bin/tako:$APP_TAKO" "$cands"

echo "== ⑩ 呼び手の実物: build-app.sh のリリースビルド =="
if [ "$(uname)" != Darwin ]; then
  echo "  （macOS 専用の build-app.sh なので飛ばす）"
else
  R="$T/repo"
  mkdir -p "$R/scripts"
  cp "$REPO_ROOT/scripts/build-app.sh" "$R/scripts/"
  cp -R "$REPO_ROOT/scripts/lib" "$R/scripts/"
  printf '#!/bin/sh\necho "build-pwa" >> "$TK_LOG"\n' > "$R/scripts/build-pwa.sh"
  printf '[workspace.package]\nversion = "0.0.0"\n' > "$R/Cargo.toml"
  # cargo は夜間と同じ ~/.cargo/bin に置く代役。枠の中か（TAKO_QUEUE_JOB）を残して 86 で止める
  printf '#!/bin/sh\necho "cargo job=${TAKO_QUEUE_JOB:-none} $*" >> "$TK_LOG"\nexit 86\n' > "$H/.cargo/bin/cargo"
  chmod +x "$R/scripts/build-pwa.sh" "$H/.cargo/bin/cargo"
  build_app() {
    : > "$LOG"
    env HOME="$H" PATH="$NIGHTLY_PATH" TK_LOG="$LOG" TK_HANG_PIDS="$T/hang.pids" \
      TAKO_QUEUE_PRIORITY=low TAKO_QUEUE_MAX_WAIT=3h \
      /bin/bash "$R/scripts/build-app.sh" > "$T/build-app.out" 2>&1
  }
  rc=0
  TAKO_2119_LEGACY=1 TAKO_QUEUE_FALLBACKS="$APP_TAKO" build_app || rc=$?
  expect_eq "修正前: cargo の終了コード（86）で止まる" "86" "$rc"
  expect_has "修正前: リリースビルドは順番待ちを通らない" "cargo job=none build --release" "$(cat "$LOG")"
  rc=0
  TAKO_QUEUE_FALLBACKS="$APP_TAKO" build_app || rc=$?
  expect_eq "修正後: 並んだ cargo の終了コード（86）がそのまま返る" "86" "$rc"
  expect_has "修正後: リリースビルドは順番待ちの枠の中で走る" "cargo job=42 build --release" "$(cat "$LOG")"
  expect_has "修正後: .app の tako へ low / 3h で並ぶ" "queue new prio=low max_wait=3h args=cargo build --release" "$(cat "$LOG")"
  expect_has "修正後: 古い tako を飛ばしたことがビルドのログに残る" "~/.cargo/bin/tako（順番待ち（#1994）を知らない古い版）を飛ばして" "$(cat "$T/build-app.out")"
  rc=0
  TAKO_QUEUE_FALLBACKS="" build_app || rc=$?
  expect_eq "知っている tako が無い: 直に走って cargo まで進む（86）" "86" "$rc"
  expect_has "知っている tako が無い: 枠の外で走る" "cargo job=none build --release" "$(cat "$LOG")"
  rc=0
  TAKO_QUEUE_PROBE_TIMEOUT=1 TAKO_QUEUE_FALLBACKS="$T/hang/tako" build_app || rc=$?
  expect_eq "応答しない tako: 直に走って cargo まで進む（86）" "86" "$rc"
  expect_has "応答しない tako: 枠の外で走る" "cargo job=none build --release" "$(cat "$LOG")"
  rm -f "$H/.cargo/bin/cargo"
fi

echo
echo "結果: OK ${PASS} / NG ${FAIL}"
[ "$FAIL" -eq 0 ]
