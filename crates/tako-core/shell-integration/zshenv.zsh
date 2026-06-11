# tako シェル統合（zsh）— OSC 7（cwd）/ OSC 133（プロンプトマーク）発行。FR-2.4.1
#
# tako は ZDOTDIR をこのディレクトリに向けてシェルを起動する。この .zshenv は
# 1) ZDOTDIR を元に戻す（以降の .zprofile / .zshrc はユーザーのものが読まれる）
# 2) ユーザーの .zshenv を読み込む
# 3) インタラクティブシェルならフックを登録する
# パスの percent エンコードは行わない（% を含むパスのみ誤検知しうる。実用上稀）

if [[ -n ${TAKO_ORIG_ZDOTDIR-} ]]; then
  export ZDOTDIR="$TAKO_ORIG_ZDOTDIR"
  unset TAKO_ORIG_ZDOTDIR
else
  unset ZDOTDIR
fi
if [[ -f "${ZDOTDIR:-$HOME}/.zshenv" ]]; then
  builtin source "${ZDOTDIR:-$HOME}/.zshenv"
fi

if [[ -o interactive && -n ${TAKO_PANE_ID-} ]]; then
  _tako_report_cwd() {
    builtin printf '\e]7;file://%s%s\a' "${HOST-}" "$PWD"
  }
  _tako_preexec() {
    builtin printf '\e]133;C\a'
  }
  _tako_precmd() {
    local ret=$?
    if [[ -n ${_tako_ran_command-} ]]; then
      builtin printf '\e]133;D;%d\a' "$ret"
    fi
    _tako_ran_command=
    builtin printf '\e]133;A\a'
    _tako_report_cwd
  }
  _tako_mark_exec() {
    _tako_ran_command=1
    _tako_preexec "$@"
  }
  typeset -ag precmd_functions preexec_functions chpwd_functions
  precmd_functions+=(_tako_precmd)
  preexec_functions+=(_tako_mark_exec)
  chpwd_functions+=(_tako_report_cwd)
fi
