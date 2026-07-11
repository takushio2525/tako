#!/usr/bin/env bash
set -euo pipefail

# scripts/clean-target.sh — target/ の掃除（ディスク圧迫対策）
#
# 使い方:
#   scripts/clean-target.sh           # dry-run（削減見積もり表示のみ）
#   scripts/clean-target.sh --run     # 実行（cargo clean + 古い worktree 削除）
#
# 何をするか:
#   1. target/ を cargo clean で全削除（dev / release 両方）
#   2. git worktree list で prune 済み worktree のゴミを掃除
#   3. 削減サイズを報告

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$REPO_ROOT"

DRY_RUN=true
if [[ "${1:-}" == "--run" ]]; then
    DRY_RUN=false
fi

size_of() {
    du -sh "$1" 2>/dev/null | cut -f1 || echo "0B"
}

echo "=== tako target 掃除 ==="
echo ""

# target/ のサイズ
if [[ -d target ]]; then
    TARGET_SIZE=$(size_of target)
    echo "target/: ${TARGET_SIZE}"
else
    TARGET_SIZE="0B"
    echo "target/: なし"
fi

# worktree の状態
PRUNABLE=$(git worktree list --porcelain 2>/dev/null | grep -c "^worktree.*prunable" || echo 0)
echo "prunable worktree: ${PRUNABLE} 件"
echo ""

if $DRY_RUN; then
    echo "[dry-run] 実行するには: scripts/clean-target.sh --run"
    echo ""
    echo "実行時の動作:"
    echo "  1. cargo clean（target/ 全削除。削減: ${TARGET_SIZE}）"
    echo "  2. git worktree prune（到達不能な worktree メタデータを除去）"
    exit 0
fi

echo "実行中..."
echo ""

# 1. cargo clean
echo "$ cargo clean"
cargo clean
echo "  → target/ 削除完了（${TARGET_SIZE} 解放）"
echo ""

# 2. git worktree prune
echo "$ git worktree prune"
git worktree prune
echo "  → worktree メタデータ掃除完了"
echo ""

echo "完了。次回のビルドはフルコンパイルになります。"
