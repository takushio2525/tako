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

  # tako CLI の PATH 注入（Issue #601）。このファイルが source されるのは最初の
  # プロンプト直前 = ~/.bashrc の後なので、「ユーザーが自分で tako を通しているか」を
  # 正しく判定できる。足すのは PATH の末尾、しかも見つからないときだけ。
  # ~/.bashrc は書き換えないので tako の外の bash は不変。逃げ道は TAKO_NO_PATH_INJECTION=1
  if [[ -z ${TAKO_NO_PATH_INJECTION-} ]]; then
    _tako_root=${BASH_SOURCE[0]%/*}
    _tako_cli_dir=
    # $(<file) は bash では fork しない。tako が起動時に書く（不在・空 = 注入しない）
    [[ -r $_tako_root/cli-dir ]] && _tako_cli_dir=$(<"$_tako_root/cli-dir")
    if [[ -n $_tako_cli_dir && -x $_tako_cli_dir/tako ]] \
      && [[ ":$PATH:" != *":$_tako_cli_dir:"* ]] \
      && ! command -v tako >/dev/null 2>&1; then
      export PATH="$PATH:$_tako_cli_dir"
    fi
    unset _tako_root _tako_cli_dir
  fi
fi
