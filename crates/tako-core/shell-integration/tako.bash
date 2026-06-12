# tako シェル統合（bash）— OSC 7（cwd）/ OSC 133（プロンプトマーク）発行。FR-2.4.1
#
# tako は PROMPT_COMMAND="source <このファイル>" を注入して起動する。最初のプロンプト
# 直前に source され、自分を正規のフック（PROMPT_COMMAND + DEBUG trap）へ置き換える。
# ユーザーの .bashrc が PROMPT_COMMAND を上書き代入した場合は統合されない（無害に劣化）

if [[ -n ${TAKO_PANE_ID-} && $- == *i* && -z ${_TAKO_BASH_DONE-} ]]; then
  _TAKO_BASH_DONE=1

  # tako の tmux バックエンド（Phase 5.5 / FR-5）配下なら OSC をパススルーで包み、
  # TMUX を unset してユーザー自身の tmux 利用（ネスト）を素通しにする（zsh 版と同じ）
  _tako_tmux=
  if [[ -n ${TMUX-} ]]; then
    _tako_sock=${TMUX%%,*}
    if [[ ${_tako_sock##*/} == tako* ]]; then
      _tako_tmux=1
      unset TMUX TMUX_PANE
    fi
    unset _tako_sock
  fi
  _tako_emit() {
    if [[ -n $_tako_tmux ]]; then
      # パススルー内の ESC は二重化する（tmux の仕様）。
      # 置換は変数経由（"${...//$'\e'/...}" の置換側 $'…' はリテラル扱いされる）
      local esc=$'\e'
      printf '\ePtmux;%s\e\\' "${1//$esc/$esc$esc}"
    else
      printf '%s' "$1"
    fi
  }
  _tako_report_cwd() {
    _tako_emit $'\e]7;file://'"${HOSTNAME-}${PWD}"$'\a'
  }
  _tako_precmd() {
    local ret=$?
    if [[ -n ${_tako_ran_command-} ]]; then
      _tako_emit $'\e]133;D;'"$ret"$'\a'
    fi
    _tako_ran_command=
    _tako_at_prompt=1
    _tako_emit $'\e]133;A\a'
    _tako_report_cwd
  }
  # プロンプト後の最初のコマンドで C を打つ（bash-preexec 相当の最小実装）
  _tako_debug() {
    if [[ -n ${_tako_at_prompt-} && $BASH_COMMAND != _tako_precmd ]]; then
      _tako_at_prompt=
      _tako_ran_command=1
      _tako_emit $'\e]133;C\a'
    fi
  }

  PROMPT_COMMAND=_tako_precmd
  trap _tako_debug DEBUG
fi
