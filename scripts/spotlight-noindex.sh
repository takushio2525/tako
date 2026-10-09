#!/bin/bash
# spotlight-noindex.sh — cargo の target/ を Spotlight の索引から外す（macOS。#1968）
#
# 何のためか:
#   cargo の target/ はビルドのたびに数千ファイル（.rlib / .d / .o）が書き換わる。
#   Spotlight はこれを毎回索引し直し、mds_stores + mds が CPU を食い続ける
#   （#1968 の実測: mds_stores 48% + mds 37%・worktree 14 本の target が索引対象・
#   1 本の target だけで .rlib が 530 件）。PC 全体が重くなり、tako も遅れる。
#
# どう外すか（実測で比べた結果）:
#   - フォルダ名の末尾が `.noindex` = 索引されない（効く）
#   - `.noindex` の実体へのシンボリックリンク経由で書いても索引されない（効く）
#   - フォルダの中に `.metadata_never_index` を置く = **索引される（効かない）**
#   なので target/ の実体を target.noindex/ へ移し、`target` は同じ場所を指すリンクにする。
#   `target/debug/tako` のようなパス（scripts / CI / docs に 50 行以上ある）はそのまま通り、
#   移した直後の cargo は作り直さない（`Fresh`）。
#
#   **`cargo clean` はリンクだけを消して実体を残す**（実測）。その後のビルドは素の target/ を
#   作り直して索引対象へ戻るので、このスクリプトを `--apply` で通し直す
#   （取り残しの target.noindex/ = clean 前の古いビルド物は消してから移す）。
#   `scripts/clean-target.sh` はこの形を知っていて、clean の後にリンクを張り直す。
#
# 使い方:
#   scripts/spotlight-noindex.sh                 # 状態だけ（このリポジトリと全 worktree。何も変えない）
#   scripts/spotlight-noindex.sh --apply         # 上の全部を索引の外へ移す
#   scripts/spotlight-noindex.sh [--apply] <作業ツリー>...   # 指定したものだけ
#
#   作ったばかりの worktree（target/ がまだ無い）へ `--apply` すると、空の target.noindex/ と
#   リンクを先に用意するので、最初のビルドから索引されない。
#
# **ビルド中の作業ツリーは動かさない**: cargo はビルド中 `target/<profile>/.cargo-lock` を
#   開いたまま握る。どれかを開いているプロセスが居れば、その作業ツリーは飛ばして理由を出す
#   （移す瞬間に target が一瞬無くなるので、走っているビルドを壊しうる）。
#
# 利用者の設定で外す方法（リポジトリの外も含めて一度で済む）は `.agent/commands.md` の同じ行。

set -uo pipefail

APPLY=0
DIRS=()
for arg in "$@"; do
    case "$arg" in
        --apply) APPLY=1 ;;
        -h | --help)
            sed -n '2,32p' "$0" | sed 's/^# \{0,1\}//'
            exit 0
            ;;
        -*)
            echo "不明な引数: ${arg}（--apply のみ）" >&2
            exit 2
            ;;
        *) DIRS+=("$arg") ;;
    esac
done

if [ "$(uname)" != "Darwin" ]; then
    echo "Spotlight は macOS だけ（何もしない）"
    exit 0
fi

# 対象を決める: 指定が無ければ、このリポジトリの全作業ツリー（共有ツリー + worktree）
if [ "${#DIRS[@]}" -eq 0 ]; then
    repo="$(cd "$(dirname "$0")/.." && pwd)"
    while IFS= read -r line; do
        case "$line" in
            "worktree "*) DIRS+=("${line#worktree }") ;;
        esac
    done < <(git -C "$repo" worktree list --porcelain 2>/dev/null)
    if [ "${#DIRS[@]}" -eq 0 ]; then
        DIRS=("$repo")
    fi
fi

# 索引に載っているファイルの数（実体のパスで数える。リンクの先は mdfind が辿らない）
indexed_count() {
    local real="$1"
    [ -d "$real" ] || {
        echo 0
        return
    }
    # `kMDItemFSName == "*"` は 0 件を返す（実測）。全種別の根の型で数える
    mdfind -count -onlyin "$real" 'kMDItemContentTypeTree = "public.item"' 2>/dev/null || echo "?"
}

# cargo のロックを開いているプロセスが居るか（= ビルド中）
building() {
    local target="$1" locks=() f
    for f in "$target"/*/.cargo-lock "$target"/*/*/.cargo-lock; do
        [ -e "$f" ] && locks+=("$f")
    done
    [ "${#locks[@]}" -eq 0 ] && return 1
    [ -n "$(lsof -t ${locks[@]+"${locks[@]}"} 2>/dev/null)" ]
}

moved=0
skipped=0
for dir in ${DIRS[@]+"${DIRS[@]}"}; do
    dir="${dir%/}"
    if [ ! -d "$dir" ]; then
        echo "- ${dir}: 見つからない（飛ばす）"
        skipped=$((skipped + 1))
        continue
    fi
    if ! git -C "$dir" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
        echo "- ${dir}: git の作業ツリーではない（飛ばす）"
        skipped=$((skipped + 1))
        continue
    fi
    t="$dir/target"
    n="$dir/target.noindex"

    if [ -L "$t" ]; then
        dest="$(readlink "$t")"
        if [ "$dest" = "target.noindex" ]; then
            echo "- ${dir}: 済み（target → target.noindex。索引 $(indexed_count "$n") 件）"
        else
            echo "- ${dir}: target が別の場所へのリンク（${dest}）なので触らない"
            skipped=$((skipped + 1))
        fi
        continue
    fi

    if [ -e "$t" ] && [ ! -d "$t" ]; then
        echo "- ${dir}: target がディレクトリでもリンクでもない（飛ばす）"
        skipped=$((skipped + 1))
        continue
    fi

    if [ ! -e "$t" ]; then
        if [ "$APPLY" -eq 1 ]; then
            mkdir -p "$n" && ln -s target.noindex "$t" && {
                echo "- ${dir}: 未ビルド → 空の target.noindex とリンクを用意した"
                moved=$((moved + 1))
            }
        else
            echo "- ${dir}: 未ビルド（--apply で最初のビルドから索引の外へ）"
        fi
        continue
    fi

    # ここから: target が素のディレクトリ = 索引対象
    count="$(indexed_count "$t")"
    if [ "$APPLY" -ne 1 ]; then
        if [ -d "$n" ]; then
            echo "- ${dir}: 索引対象（${count} 件）。cargo clean の取り残しの target.noindex あり"
        else
            echo "- ${dir}: 索引対象（${count} 件）"
        fi
        continue
    fi
    if building "$t"; then
        echo "- ${dir}: ビルド中なので飛ばす（後で通し直す）"
        skipped=$((skipped + 1))
        continue
    fi
    if [ -d "$n" ]; then
        # cargo clean の後に作り直された target/ がある = target.noindex は clean 前の古いビルド物
        rm -rf "$n"
    fi
    if mv "$t" "$n" && ln -s target.noindex "$t"; then
        echo "- ${dir}: 索引の外へ移した（それまで ${count} 件）"
        moved=$((moved + 1))
    else
        echo "- ${dir}: 移せなかった" >&2
        skipped=$((skipped + 1))
    fi
done

if [ "$APPLY" -eq 1 ]; then
    echo "移した: ${moved} / 飛ばした: ${skipped}（索引からの削除は Spotlight が数分で反映する）"
else
    echo "状態だけ表示した。索引の外へ移すには: scripts/spotlight-noindex.sh --apply"
fi
[ "$skipped" -eq 0 ]
