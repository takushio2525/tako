#!/bin/sh
# gh-stub.sh — 入れ方・使い方動画の収録で `gh` の名前に置く台。GitHub へは一切出ない。
#
# 何のためか:
#   対話のセットアップの「設定共有」（resources/setup/system-prompt.md の Step 3.5）は、gh がログイン済みなら
#   同意を得て `gh repo create <名前> --private` → `tako config init --remote <URL>` を代行する。
#   収録の砂場から本物の GitHub へリポジトリを作りに行かないよう、この台が 3 つにだけ答える:
#     - `gh --version`            版を名乗る（tako が gh の有無を見る）
#     - `gh auth status`          ログイン済みと答える（終了コード 0。tako は終了コードだけを見る）
#     - `gh repo create <名前> --private`
#                                 砂場の中に空のリポジトリ（DEMO_REMOTES/<名前>）を作り、本物と同じ形の URL を 1 行出す
#   tako の側（`tako config init --remote` の git init → 書き出し → コミット → push）は本物の経路のまま動き、
#   push は砂場の中のリポジトリで受ける（record.sh が git の insteadOf で https://github.com/demo/ を砂場へ向ける）。
#   それ以外の操作と、private でないリポジトリの作成は断る。
#
# 使い方（record.sh が砂場の bin/gh から呼ぶ）:
#   DEMO_REMOTES  リポジトリを作る砂場の置き場（必須）
#   DEMO_CALLS    あれば、呼ばれ方を 1 行ずつ残す

[ -n "${DEMO_CALLS:-}" ] && echo "gh $*" >> "$DEMO_CALLS"

case "${1:-}" in
    --version | version)
        echo "gh version 2.83.0 (demo stand-in)"
        exit 0
        ;;
    auth)
        [ "${2:-}" = status ] && exit 0
        ;;
    repo)
        if [ "${2:-}" = create ]; then
            name=""
            private=""
            shift 2
            for arg in "$@"; do
                case "$arg" in
                    --private) private=1 ;;
                    -*) ;;
                    *) [ -n "$name" ] || name="$arg" ;;
                esac
            done
            [ -n "$name" ] && [ -n "$private" ] && [ -n "${DEMO_REMOTES:-}" ] || {
                echo "デモ用の gh: リポジトリは名前つきの --private だけ作れます" >&2
                exit 1
            }
            case "$name" in */* | .* | *[!A-Za-z0-9._-]*)
                echo "デモ用の gh: 使えない名前です: $name" >&2
                exit 1
                ;;
            esac
            [ -e "$DEMO_REMOTES/$name" ] && {
                echo "デモ用の gh: 同じ名前のリポジトリが既に在ります: $name" >&2
                exit 1
            }
            git init -q --bare "$DEMO_REMOTES/$name" || exit 1
            echo "https://github.com/demo/$name"
            exit 0
        fi
        ;;
esac
echo "デモ用の gh: この操作は実行しません（gh $*）" >&2
exit 1
