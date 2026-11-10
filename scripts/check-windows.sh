#!/usr/bin/env bash
# macOS から Windows ターゲットのコンパイル検査と clippy を行う（Issue #467 / P0・#2850）。
#
# 目的: mac で先行開発している間に Windows ビルドを腐らせないこと。
# 「一括反映」の差分が純粋な機能実装だけになり、ビルド修復作業が混ざらないようにする。
#
# 設計の正: .agent/plans/2026-07-windows-port-architecture.md
# 実機での build / 実行は別途必要（本スクリプトはリンクを行わない型検査と clippy まで）。
#
# 使い方:
#   scripts/check-windows.sh            # CI の Windows ジョブと同じ 3 つの feature 宇宙を clippy で検査
#   scripts/check-windows.sh -p tako-core   # cargo の追加引数はそのまま渡る（1 本だけ回す）
#
# 終了コード: 0 = エラーも警告も clippy の指摘も無い / 3 = 警告か clippy の指摘がある
#             （エラーは無い。#1537 / #2850）/ 2 = 前提ツールが足りない / それ以外 = cargo のエラー
set -euo pipefail

TARGET="x86_64-pc-windows-msvc"
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

missing=()

installed="$(rustup target list --installed 2>/dev/null)" || installed=""
if ! grep -qx "$TARGET" <<<"$installed"; then
  missing+=("rustup target add $TARGET")
fi

if ! command -v cargo-xwin >/dev/null 2>&1; then
  # MSVC の CRT と Windows SDK を取得して cc / rustc に渡す。
  # ring などの C 依存クレートはこれが無いとビルドスクリプトで失敗する
  missing+=("cargo install cargo-xwin")
fi

LLVM_BIN=""
if command -v brew >/dev/null 2>&1; then
  LLVM_BIN="$(brew --prefix llvm 2>/dev/null)/bin"
fi
if [ ! -x "${LLVM_BIN}/clang-cl" ]; then
  # clang-cl / lld-link / llvm-lib / llvm-rc が要る（Apple 標準の clang には lld が無い）
  missing+=("brew install llvm")
fi

if [ ${#missing[@]} -gt 0 ]; then
  echo "Windows クロスチェックの前提ツールが足りません。次を実行してください:" >&2
  for m in ${missing[@]+"${missing[@]}"}; do echo "  $m" >&2; done
  echo >&2
  echo "（CI は使わない方針のため、この検査は各自の macOS 上で行います）" >&2
  exit 2
fi

# PWA を rust_embed で埋め込むため、dist が無いと tako-control がコンパイルできない。
# 手順の正本は scripts/build-pwa.sh（#1309。crates/tako-control/build.rs も同じことをする）
scripts/build-pwa.sh --if-missing

# gpui の build script は Windows マニフェストを llvm-rc で埋め込む。
# .rc 内の相対パスが OUT_DIR 基準で解決されるため、そのままでは manifest を見つけられない。
# INCLUDE に gpui のクレートディレクトリを渡すと解決する（2026-07-25 実測）
GPUI_DIR="$(cargo metadata --format-version 1 2>/dev/null | python3 -c '
import json, os, sys
try:
    pkgs = json.load(sys.stdin)["packages"]
except Exception:
    sys.exit(0)
hit = [os.path.dirname(p["manifest_path"]) for p in pkgs if p["name"] == "gpui"]
print(hit[0] if hit else "")
')"

if [ -z "$GPUI_DIR" ]; then
  echo "gpui のクレートディレクトリを解決できませんでした" >&2
  exit 1
fi

echo "target: $TARGET"
echo "gpui:   $GPUI_DIR"
echo

# **clippy で見る**（#2850）。clippy はコンパイラのエラーと警告も含めて見る（check の上位）ので、
# 1 回で #1537 の警告と clippy の指摘の両方を拾う。Windows の cfg の内側は macOS の clippy から
# 見えず、Windows の clippy はそれまで誰も回していなかった（2026-11-10 の着手時 13 件）。引数なしでは
# AGENTS.md の lint 行・CI の Windows ジョブと同じ 3 つの feature 宇宙を回す（visual-test の宇宙に
# しか出ない指摘が着手時にあった）。clippy の `-- -D warnings` は付けない（ゲートが数える。理由は
# scripts/warning-gate.sh）
#
# **警告も 0 件に保つ**（#1537 / #1827）。以前はエラーだけを見ていて、Windows の cfg の内側
# だけで出る警告（unix 向けの補助が Windows で未使用 など）が 9 月に 31 件・11 月に 36 件まで
# 積み上がり、新しい警告が埋もれていた。数え方は CI の Windows ジョブと同じ 1 実装
# （scripts/warning-gate.sh）を通す。JSON などで出力を取りたいときは `--message-format` を
# 渡せば、ゲートを通さずに cargo の出力をそのまま返す
#
# **`--all-targets` を既定にする**（#1441）。既定（lib / bin のみ）だと `tests/` が
# 検査対象に入らず、統合テストの `std::os::unix` を `cfg(unix)` で囲い忘れても
# ここは error 0 のまま緑になる。CI の Windows ジョブは
# 「テストのコンパイル検査（#1264）」= `cargo test --workspace --no-run` を回すので、
# **手元が緑なのに CI だけ赤**になる（2026-09-13 に PR #1444 で実測: E0433
# `cannot find unix in os` が 2 件。旧レシピは error 0 を返していた）。
# 呼び出し側が明示したときは重ねない
ALL_TARGETS=(--all-targets)
for arg in "$@"; do
  if [ "$arg" = "--all-targets" ]; then
    ALL_TARGETS=()
    break
  fi
done
GATE=(bash "${REPO_ROOT}/scripts/warning-gate.sh")
for arg in "$@"; do
  case "$arg" in
    --message-format*) GATE=() ;;
  esac
done
xwin_clippy() {
  PATH="${LLVM_BIN}:$PATH" INCLUDE="$GPUI_DIR" \
    ${GATE[@]+"${GATE[@]}"} cargo xwin clippy --target "$TARGET" "$@"
}

if [ "$#" -eq 0 ]; then
  # 3 つの宇宙を全部回してから返す。エラー（3 以外）を警告（3）より優先して返す
  rc=0
  for universe in workspace visual-test no-gpui; do
    r=0
    case "$universe" in
      workspace) xwin_clippy --workspace --all-targets || r=$? ;;
      visual-test) xwin_clippy -p tako-app --features visual-test --all-targets || r=$? ;;
      no-gpui) xwin_clippy -p tako-core -p tako-control -p tako-cli --all-targets || r=$? ;;
    esac
    if [ "$r" -ne 0 ] && { [ "$rc" -eq 0 ] || [ "$rc" -eq 3 ]; }; then
      rc="$r"
    fi
  done
  exit "$rc"
fi
xwin_clippy --workspace ${ALL_TARGETS[@]+"${ALL_TARGETS[@]}"} "$@"
