#!/bin/bash
# tako:run bash -c '. scripts/lib/fake-exec.sh && echo "空打ちの引数: $FAKE_EXEC_WARM_ARG"'
#
# scripts/lib/fake-exec.sh — シェルのテストが作る偽の実行ファイル（スタブ）を、作った時点で 1 回
# 空打ちして、macOS の初回 exec の検査をテストの上限の外で済ませる（#2222）。
# Rust のテストの 1 実装 `tako_core::fake_exec` と同じ規則（空打ちの引数・A/B・注入の env が共通）。
#
# 使い方:
#   . "$REPO_ROOT/scripts/lib/fake-exec.sh"
#   cat > "$BIN/claude" <<'SH'            # 1 行目は shebang
#   #!/bin/sh
#   ...
#   SH
#   fake_exec_warm "$BIN/claude" "$BIN/brew"   # 2 行目に空打ちの行を差し込み、実行権を付けて 1 回空打ちする
#   cp "$BIN/claude" "$DEST/claude" && fake_exec_warm "$DEST/claude"   # コピーも新しいファイル = もう 1 回
#
# - macOS は新しいファイル（新しい inode）の初回の exec に検査を挟み、その検査はシステム全体で
#   1 本ずつ進む（単独 0.13〜0.34 秒・6 本同時なら最後は 1.7 秒・他のビルドと重なると 6 秒）。
#   コピーも検査を受け、同じパスへの上書きは受けない。作りたてのスタブを短い上限で初めて叩くと、
#   固まらせていない段まで打ち切られて偽の赤になる（#1933 / #2180 / #1838）
# - 空打ちの行（`[ "${1:-}" = tako-fake-exec-warm ] && exit 0`）が既にあれば差し込まない
#   （コピーへ掛け直しても 2 行にならない = 冪等）
# - 空打ちは上限 120 秒（perl の alarm。perl が無ければ切らない）。終わらなければ stderr へ 1 行残して進む
#   （判定はテストの側の上限と状態で見る）
# - A/B: TAKO_2222_LEGACY=1 は空打ちしない（修正前の形）。
#   注入: TAKO_2222_INJECT_FIRST_EXEC_MS=<ms> で各スタブの初回の exec だけを遅らせる（印は `<スタブ>.first-exec`）
#
# bash 3.2（macOS 同梱）でも通す。`| grep -q` は pipefail の下で取りこぼす（#1933）ので case で見る。

FAKE_EXEC_WARM_ARG=tako-fake-exec-warm

# スタブの 2 行目に空打ちの行（と注入の行）を差し込む（既にあれば何もしない）
_fake_exec_insert() {
  local f="$1" first second ms tmp
  if [ ! -f "$f" ]; then
    echo "fake_exec_warm: スタブが無い: $f" >&2
    return 1
  fi
  first="$(head -n 1 "$f")"
  case "$first" in
    '#!'*) ;;
    *) echo "fake_exec_warm: 1 行目が shebang でない: $f" >&2; return 1 ;;
  esac
  second="$(sed -n 2,3p "$f")"
  case "$second" in
    *"$FAKE_EXEC_WARM_ARG"*) return 0 ;;
  esac
  tmp="$f.fake-exec.$$"
  {
    printf '%s\n' "$first"
    ms="${TAKO_2222_INJECT_FIRST_EXEC_MS:-}"
    case "$ms" in
      '' | *[!0-9]*) ;;
      *) printf 'if [ ! -e "$0.first-exec" ]; then : > "$0.first-exec"; /bin/sleep %d.%03d; fi\n' \
           "$((ms / 1000))" "$((ms % 1000))" ;;
    esac
    printf '[ "${1:-}" = %s ] && exit 0\n' "$FAKE_EXEC_WARM_ARG"
    tail -n +2 "$f"
  } > "$tmp" || return 1
  command mv -f "$tmp" "$f"
}

# fake_exec_warm <スタブ>...: 空打ちの行を差し込み、実行権を付けて 1 回空打ちする
fake_exec_warm() {
  local f rc
  for f in "$@"; do
    _fake_exec_insert "$f" || return 1
    chmod +x "$f" || return 1
    if [ "${TAKO_2222_LEGACY:-}" = 1 ]; then
      continue
    fi
    rc=0
    if command -v perl >/dev/null 2>&1; then
      perl -e 'alarm shift; exec @ARGV or exit 127' 120 "$f" "$FAKE_EXEC_WARM_ARG" > /dev/null 2>&1 || rc=$?
    else
      "$f" "$FAKE_EXEC_WARM_ARG" > /dev/null 2>&1 || rc=$?
    fi
    if [ "$rc" -ne 0 ]; then
      echo "fake_exec_warm（#2222）: 空打ちが exit ${rc} で終わった（142 = 120 秒で終わらない）: $f" >&2
    fi
  done
  return 0
}
