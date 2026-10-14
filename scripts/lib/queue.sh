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
# - 順番待ちを知っている tako が PATH に居れば `tako queue <コマンド>` で並ぶ。
#   マシン全体の同時数・空きメモリ・メモリ圧で割り当てられ、取り分（CARGO_BUILD_JOBS /
#   RUST_TEST_THREADS）が未設定なら渡される。終了コードはコマンドのものがそのまま返る
# - tako が無い（CI）・順番待ちを知らない古い tako・入れ子（TAKO_QUEUE_JOB がある = 既に枠の中）・
#   TAKO_NO_QUEUE=1 のときは、そのまま走らせる
# - tako が起動していなければ `tako queue` 自身が待たずに走らせる（fail-open）
#
# bash 3.2（macOS 同梱）でも通す（空配列の展開を使わない。#1499 / #1518）。

# 順番待ちを知っている tako か（1 回だけ調べて覚える。読み込み直しても調べ直さない）
_TAKO_QUEUE_SUPPORTED="${_TAKO_QUEUE_SUPPORTED:-}"
_tako_queue_supported() {
  if [ -z "$_TAKO_QUEUE_SUPPORTED" ]; then
    _TAKO_QUEUE_SUPPORTED=no
    # `| grep -q` は pipefail の下で書き手の SIGPIPE を拾って取りこぼす（#1933）ので case で見る
    if command -v tako >/dev/null 2>&1; then
      case "$(tako queue --help 2>/dev/null)" in
        *--max-wait*) _TAKO_QUEUE_SUPPORTED=yes ;;
      esac
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
    tako queue --priority "$priority" "$@"
  else
    tako queue "$@"
  fi
}
