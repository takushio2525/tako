#!/usr/bin/env bash
# bundle-install.sh — .app を「置き場のパスを一度も空けずに」設置する共有ヘルパ（Issue #1042）
#
# 使い方: source "$REPO_ROOT/scripts/lib/bundle-install.sh"
#         install_bundle_in_place <src.app> <dest.app>
#
# 正本は Rust 側の `tako_core::platform::bundle_install`（`replace_bundle_in_place`）で、
# これはその写し。**手順が食い違わないこと**を
# `crates/tako-control/tests/bundle_install_watchdog.rs` が機械検証する。
#
# なぜ `rm -rf` → `cp -R` ではいけないか:
#
#   Dock のピン留めは .app への file URL ブックマーク（com.apple.dock の
#   persistent-apps[].tile-data.book）で持たれ、CNID（inode）を優先して解決する。
#   置き場が一瞬でも空くと、追跡している側は「アプリが消えた／どこかへ移動した」と
#   読んでしまい、ピンが外れる（#1042）。実測でも rm -rf の直後にブックマークの解決が
#   失敗することを確認している。
#
# 正しい形は「隣へステージ → アトミックに入れ替え → 旧版を捨てる」。入れ替えは
# renamex_np(2) の RENAME_SWAP で、macOS には CLI が無いので python3 から呼ぶ。
# python3 が無い環境では旧来の rm -rf → cp -R へ落ちる（警告を出す）。

# 使える python3 を 1 つ選ぶ（見つからなければ空文字）。
# TAKO_BUNDLE_INSTALL_PYTHON は**呼び出しのたびに**見る（モックテストが
# `TAKO_BUNDLE_INSTALL_PYTHON=... install_bundle_in_place ...` の形で差し替えられるように）
_bundle_install_python() {
  if [[ -n "${TAKO_BUNDLE_INSTALL_PYTHON:-}" ]]; then
    printf '%s' "${TAKO_BUNDLE_INSTALL_PYTHON}"
    return 0
  fi
  local candidate
  for candidate in /usr/bin/python3 python3; do
    if command -v "$candidate" >/dev/null 2>&1; then
      printf '%s' "$candidate"
      return 0
    fi
  done
  printf ''
}

# 2 つのパスの中身をアトミックに入れ替える（renamex_np + RENAME_SWAP）
_bundle_swap_paths() {
  local a="$1" b="$2" py
  py="$(_bundle_install_python)"
  [[ -n "$py" ]] || return 1
  "$py" - "$a" "$b" <<'PY'
import ctypes, ctypes.util, os, sys
RENAME_SWAP = 0x2
libc = ctypes.CDLL(ctypes.util.find_library("c"), use_errno=True)
rc = libc.renamex_np(os.fsencode(sys.argv[1]), os.fsencode(sys.argv[2]),
                     ctypes.c_uint(RENAME_SWAP))
if rc != 0:
    err = ctypes.get_errno()
    sys.stderr.write("renamex_np failed: errno=%d (%s)\n" % (err, os.strerror(err)))
    sys.exit(1)
PY
}

# 標準的なバンドルの形（トップレベルが Contents ただ 1 つ）か。
# 余分なトップレベル項目があると Contents だけ替えても残ってしまうので、
# そのときはバンドルごと入れ替える側に倒す
_bundle_only_contents() {
  local entries
  entries="$(ls -A "$1" 2>/dev/null)"
  [[ "$entries" == "Contents" && -d "$1/Contents" ]]
}

# .app を置き場へ設置する。置き場のパスを一度も空けない（#1042）
#
#   install_bundle_in_place <src.app> <dest.app>
#
# 標準出力へ使った手段（contents-swap / swap / fresh / move-aside）を 1 語で出す
install_bundle_in_place() {
  local src="$1" dest="$2"
  local parent staging staged
  parent="$(dirname "$dest")"
  staging="${parent}/.tako-replace-$$"
  staged="${staging}/$(basename "$dest")"

  rm -rf "$staging"
  mkdir -p "$staging" || return 1
  # ditto は署名と拡張属性を保つ（cp -R では保てない）
  if ! ditto "$src" "$staged"; then
    rm -rf "$staging"
    return 1
  fi

  if [[ ! -e "$dest" ]]; then
    # 置き場が空 → mv 1 回でアトミックに置ける（窓ゼロ）
    if mv "$staged" "$dest"; then
      rm -rf "$staging"
      echo "fresh"
      return 0
    fi
    rm -rf "$staging"
    return 1
  fi

  # まず Contents/ だけの入れ替えを試す。成功すれば .app 自体の inode が変わらないので、
  # Dock のブックマークは張り直しすら要らない（実測で isStale が false のまま）
  if _bundle_only_contents "$dest" && _bundle_only_contents "$staged" \
     && _bundle_swap_paths "$dest/Contents" "$staged/Contents" 2>/dev/null; then
    rm -rf "$staging"
    echo "contents-swap"
    return 0
  fi

  if _bundle_swap_paths "$dest" "$staged" 2>/dev/null; then
    # 入れ替え完了。staging 側には旧版が居るので捨てる
    rm -rf "$staging"
    echo "swap"
    return 0
  fi

  # swap が使えない環境。更新できないよりはマシなので旧挙動へ落ちる
  echo "警告: アトミックな入れ替えが使えないため、置き場を一度空けて設置します。" >&2
  echo "      Dock のピン留めが外れることがあります（#1042）。" >&2
  rm -rf "$dest"
  if ! cp -R "$staged" "$dest"; then
    rm -rf "$staging"
    return 1
  fi
  rm -rf "$staging"
  echo "move-aside"
}
