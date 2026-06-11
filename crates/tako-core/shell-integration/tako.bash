# tako シェル統合（bash）— OSC 7（cwd）/ OSC 133（プロンプトマーク）発行。FR-2.4.1
#
# tako は PROMPT_COMMAND="source <このファイル>" を注入して起動する。最初のプロンプト
# 直前に source され、自分を正規のフック（PROMPT_COMMAND + DEBUG trap）へ置き換える。
# ユーザーの .bashrc が PROMPT_COMMAND を上書き代入した場合は統合されない（無害に劣化）

if [[ -n ${TAKO_PANE_ID-} && $- == *i* && -z ${_TAKO_BASH_DONE-} ]]; then
  _TAKO_BASH_DONE=1

  _tako_report_cwd() {
    printf '\e]7;file://%s%s\a' "${HOSTNAME-}" "$PWD"
  }
  _tako_precmd() {
    local ret=$?
    if [[ -n ${_tako_ran_command-} ]]; then
      printf '\e]133;D;%d\a' "$ret"
    fi
    _tako_ran_command=
    _tako_at_prompt=1
    printf '\e]133;A\a'
    _tako_report_cwd
  }
  # プロンプト後の最初のコマンドで C を打つ（bash-preexec 相当の最小実装）
  _tako_debug() {
    if [[ -n ${_tako_at_prompt-} && $BASH_COMMAND != _tako_precmd ]]; then
      _tako_at_prompt=
      _tako_ran_command=1
      printf '\e]133;C\a'
    fi
  }

  PROMPT_COMMAND=_tako_precmd
  trap _tako_debug DEBUG
fi
