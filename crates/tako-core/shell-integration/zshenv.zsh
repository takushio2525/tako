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
  # tako の tmux バックエンド（Phase 5.5 / FR-5。ソケット名 tako*）配下なら:
  # 1) OSC をパススルー（DCS tmux; … ST。allow-passthrough）で包み、外の tako へ届かせる
  # 2) TMUX / TMUX_PANE を unset し、ユーザー自身の tmux 利用（ネスト）を素通しにする
  #    （バックエンドは見えない裏方。素の `tmux` が今まで通り既定サーバーに繋がる）
  _tako_tmux=
  if [[ -n ${TMUX-} && ${${TMUX%%,*}:t} == tako* ]]; then
    _tako_tmux=1
    unset TMUX TMUX_PANE
  fi
  _tako_emit() {
    if [[ -n $_tako_tmux ]]; then
      # パススルー内の ESC は二重化する（tmux の仕様）。
      # 置換は変数経由（"${...//$'\e'/...}" の置換側 $'…' はリテラル扱いされる）
      local esc=$'\e'
      builtin printf '\ePtmux;%s\e\\' "${1//$esc/$esc$esc}"
    else
      builtin printf '%s' "$1"
    fi
  }
  _tako_report_cwd() {
    _tako_emit $'\e]7;file://'"${HOST-}${PWD}"$'\a'
  }
  _tako_preexec() {
    _tako_emit $'\e]133;C\a'
  }
  _tako_precmd() {
    local ret=$?
    if [[ -n ${_tako_ran_command-} ]]; then
      _tako_emit $'\e]133;D;'"$ret"$'\a'
    fi
    _tako_ran_command=
    _tako_emit $'\e]133;A\a'
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
