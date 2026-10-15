#!/bin/bash
# tako:run bash -c '. scripts/lib/queue.sh && run_heavy echo ok'
#
# scripts/lib/queue.sh — 重い処理（cargo build / test / clippy・build-app・Playwright）を
# tako の順番待ち（`tako queue <コマンド>`。#1994 / エピック #1992 の S2）へ通す入口。
#
# 使い方:
#   . "$REPO_ROOT/scripts/lib/queue.sh"
#   run_heavy cargo build -p tako-cli
#   run_heavy --priority low cargo build --release    # 人や worker の後ろへ並ぶ
#   （夜間リリースは env の TAKO_QUEUE_PRIORITY=low / TAKO_QUEUE_MAX_WAIT=3h で中のビルドを丸ごと low にする）
#
# - 順番待ちを知っている tako が見つかれば `tako queue <コマンド>` で並ぶ。
#   マシン全体の同時数・空きメモリ・メモリ圧で割り当てられ、取り分（CARGO_BUILD_JOBS /
#   RUST_TEST_THREADS）が未設定なら渡される。終了コードはコマンドのものがそのまま返る
# - 探す順（#2119）: ① PATH の tako を上から全部（先頭の 1 本だけで決めない）→
#   ② .app の同梱 CLI（TAKO_QUEUE_FALLBACKS。: 区切り。既定は /Applications と ~/Applications の tako.app）。
#   launchd の夜間リリースは PATH の先頭が ~/.cargo/bin で、そこに順番待ちを知らない古い tako が
#   居ると、後ろに知っている tako が居ても並ばずに走っていた。古い tako を飛ばしたときは 1 行知らせる
# - 順番待ちを知っているかは `tako queue --help` に --max-wait が載るかで見る。応答しない tako で
#   止まらないよう TAKO_QUEUE_PROBE_TIMEOUT 秒（既定 10）で切る（perl の alarm。perl が無ければ切らない）
# - tako が無い（CI）・順番待ちを知る tako が見つからない・入れ子（TAKO_QUEUE_JOB がある = 既に枠の中）・
#   TAKO_NO_QUEUE=1 のときは、そのまま走らせる
# - tako が起動していなければ `tako queue` 自身が待たずに走らせる（fail-open）
# - テストは TAKO_QUEUE_FALLBACKS を空（= PATH だけ）か偽の置き場へ差し替える。
#   A/B は TAKO_2119_LEGACY=1（PATH の先頭の tako だけを見る = 修正前）
#
# bash 3.2（macOS 同梱）でも通す（空配列の展開を使わない。#1499 / #1518）。

# 順番待ちを知っている tako の置き場と、知っているか（1 回だけ調べて覚える。読み込み直しても調べ直さない）
_TAKO_QUEUE_SUPPORTED="${_TAKO_QUEUE_SUPPORTED:-}"
_TAKO_QUEUE_BIN="${_TAKO_QUEUE_BIN:-}"

# その tako が順番待ちを知っているか（`queue --help` に --max-wait が載るか）。
# 0 = 知っている / 1 = 知らない（古い・壊れている）/ 2 = 上限の秒数のあいだ応答しない
_tako_queue_probe() {
  local out="" rc=0
  # `| grep -q` は pipefail の下で書き手の SIGPIPE を拾って取りこぼす（#1933）ので case で見る
  if command -v perl >/dev/null 2>&1; then
    out="$(perl -e 'alarm shift; exec @ARGV or exit 127' "${TAKO_QUEUE_PROBE_TIMEOUT:-10}" "$1" queue --help 2>/dev/null)" || rc=$?
  else
    out="$("$1" queue --help 2>/dev/null)" || rc=$?
  fi
  case "$out" in
    *--max-wait*) return 0 ;;
  esac
  # SIGALRM（14）で切られた = 128 + 14
  [ "$rc" -eq 142 ] && return 2
  return 1
}

# 知らせる行のパス（ホームは ~ にする）
_tako_queue_show() {
  if [ -n "${HOME:-}" ]; then
    case "$1" in
      "$HOME"/*) printf '~%s' "${1#"$HOME"}"; return 0 ;;
    esac
  fi
  printf '%s' "$1"
}

# 探す候補（: 区切り）= PATH の tako を上から全部 + .app の同梱 CLI（TAKO_QUEUE_FALLBACKS。空 = PATH だけ）
_tako_queue_candidates() {
  local path_takos
  path_takos="$(type -a -P tako 2>/dev/null | tr '\n' ':')" || true
  printf '%s%s' "$path_takos" "${TAKO_QUEUE_FALLBACKS-/Applications/tako.app/Contents/MacOS/tako:${HOME:-}/Applications/tako.app/Contents/MacOS/tako}"
}

_tako_queue_supported() {
  if [ -z "$_TAKO_QUEUE_SUPPORTED" ]; then
    _TAKO_QUEUE_SUPPORTED=no
    _TAKO_QUEUE_BIN=""
    if [ "${TAKO_2119_LEGACY:-0}" = 1 ]; then
      # 修正前: PATH の先頭の tako だけを見る
      if command -v tako >/dev/null 2>&1; then
        case "$(tako queue --help 2>/dev/null)" in
          *--max-wait*) _TAKO_QUEUE_SUPPORTED=yes; _TAKO_QUEUE_BIN=tako ;;
        esac
      fi
    else
      local cand rest r why skipped="" seen="|"
      rest="$(_tako_queue_candidates)"
      while [ -n "$rest" ]; do
        cand="${rest%%:*}"
        case "$rest" in
          *:*) rest="${rest#*:}" ;;
          *) rest="" ;;
        esac
        [ -n "$cand" ] && [ -x "$cand" ] && [ ! -d "$cand" ] || continue
        case "$seen" in
          *"|$cand|"*) continue ;;
        esac
        seen="$seen$cand|"
        r=0
        _tako_queue_probe "$cand" || r=$?
        if [ "$r" -eq 0 ]; then
          _TAKO_QUEUE_SUPPORTED=yes
          _TAKO_QUEUE_BIN="$cand"
          break
        fi
        if [ "$r" -eq 2 ]; then
          why="${TAKO_QUEUE_PROBE_TIMEOUT:-10} 秒応答しない"
        else
          why="順番待ち（#1994）を知らない古い版"
        fi
        skipped="${skipped:+${skipped}、}$(_tako_queue_show "$cand")（${why}）"
      done
      if [ -n "$skipped" ]; then
        if [ -n "$_TAKO_QUEUE_BIN" ]; then
          echo "run_heavy: ${skipped}を飛ばして $(_tako_queue_show "$_TAKO_QUEUE_BIN") で並ぶ" >&2
        else
          echo "run_heavy: ${skipped}を飛ばした。順番待ちを知る tako が見つからないので並ばずに走らせる" >&2
        fi
      fi
    fi
  fi
  [ "$_TAKO_QUEUE_SUPPORTED" = yes ]
}

run_heavy() {
  local priority=""
  if [ "${1:-}" = "--priority" ]; then
    priority="${2:-}"
    shift 2
  fi
  if [ $# -eq 0 ]; then
    echo "run_heavy: 走らせるコマンドが無い" >&2
    return 2
  fi
  if [ -n "${TAKO_QUEUE_JOB:-}" ] || [ -n "${TAKO_NO_QUEUE:-}" ] || ! _tako_queue_supported; then
    "$@"
    return $?
  fi
  if [ -n "$priority" ]; then
    "${_TAKO_QUEUE_BIN:-tako}" queue --priority "$priority" "$@"
  else
    "${_TAKO_QUEUE_BIN:-tako}" queue "$@"
  fi
}
