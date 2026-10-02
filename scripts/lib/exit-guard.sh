# exit-guard.sh — set -e と EXIT trap を併用するスクリプトの終了コードの番人（Issue #1864）
#
# macOS 同梱の bash 3.2（/bin/bash も /bin/sh も中身はこれ）は、set -e が効いている
# ときに展開エラー（set -u の未定義変数・${x:?}・不正な置換・readonly への代入）で死ぬと、
# EXIT trap の中の $? が 0 になり、trap が exit しなくてもスクリプトの終了コードが 0 に
# 化ける（trap が無ければ 1。set -e が無ければ 1。bash 5 は 1）。夜間リリース
# （launchd が /bin/bash で起動する）が途中で死んでも「成功」に見えるのが最悪の型。
#
# 死に方は trap の中から見分けられない（$? も $BASH_COMMAND も正常終了と同じ形になる）ので、
# **成功の印を立ててから抜けた 0 だけを本物の 0 とする**（#1861 の release.sh --promote と同じ方式）:
#
#   . "$REPO_ROOT/scripts/lib/exit-guard.sh"
#   tako_exit_trap cleanup [何の処理か]   # `trap cleanup EXIT` の代わり
#   ...
#   tako_exit 0                          # 成功で抜ける所はすべてこれ
#
# - 印の無い 0（素の `exit 0`・末尾への到達・展開エラーでの死）は 1 にして、stderr へ 1 行出す
# - 非 0 の exit はそのまま通す（本物の失敗なので印は要らない）
# - 後始末のコマンドは補正の後に走る。補正後の終了コードは TAKO_EXIT_GUARD_RC、
#   補正したかどうかは TAKO_EXIT_GUARD_TRIPPED（1 = 化けかけた）で読める
# - `trap - EXIT` で外した後は印は要らない（trap が無ければ化けない）
# - INT / TERM などのシグナルの trap はこれまでどおり素の `trap` で張る（ここは EXIT だけ）
# - POSIX sh からも source できる書き方にしてある（scripts/verify-setup-multiagent.sh は #!/bin/sh）
#
# 番犬: crates/tako-control/tests/shell_scripts.rs が、set -e を宣言して素の EXIT trap を張る
# スクリプトを file:line で名指しして落とす。規約は .agent/conventions.md の
# 「シェルスクリプトは macOS 同梱の bash 3.2 で通す」節。

# tako_exit_trap <後始末のコマンド> [何の処理か]
#   EXIT trap を張る。<後始末のコマンド> は trap の文字列と同じく、走るときに評価される。
#   [何の処理か] はエラー文の主語（既定は「<スクリプト名> の処理」）。張り直すと印は倒れる
tako_exit_trap() {
  TAKO_EXIT_GUARD_CMD=${1:-:}
  TAKO_EXIT_GUARD_WHAT=${2:-"${0##*/} の処理"}
  TAKO_EXIT_GUARD_OK=0
  TAKO_EXIT_GUARD_TRIPPED=0
  TAKO_EXIT_GUARD_RC=0
  trap '_tako_exit_guard_on_exit' EXIT
}

# tako_exit [終了コード]
#   成功の印を立てて抜ける（既定は 0）。trap を張った後に 0 で抜ける所はすべてこれを通す
tako_exit() {
  TAKO_EXIT_GUARD_OK=1
  exit "${1:-0}"
}

# EXIT trap の本体。最初の行で $? を受ける（間に何か挟むと $? が上書きされる）
_tako_exit_guard_on_exit() {
  TAKO_EXIT_GUARD_RC=$?
  if [ "$TAKO_EXIT_GUARD_RC" -eq 0 ] && [ "${TAKO_EXIT_GUARD_OK:-0}" != 1 ]; then
    TAKO_EXIT_GUARD_RC=1
    TAKO_EXIT_GUARD_TRIPPED=1
    echo "エラー: ${TAKO_EXIT_GUARD_WHAT:-処理}が途中で止まった（終了コード 0 のまま抜けかけたので 1 にした。上の出力の最後のエラー行を見る）" >&2
  fi
  eval "${TAKO_EXIT_GUARD_CMD:-:}"
  exit "$TAKO_EXIT_GUARD_RC"
}
