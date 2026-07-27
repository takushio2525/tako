# tako シェル統合（zsh）— OSC 7（cwd）/ OSC 133（プロンプトマーク）発行。FR-2.4.1
#
# tako は ZDOTDIR をこのディレクトリに向けてシェルを起動する。この .zshenv は
# 1) ZDOTDIR を元に戻す（以降の .zprofile / .zshrc はユーザーのものが読まれる）
# 2) ユーザーの .zshenv を読み込む
# 3) インタラクティブシェルならフックを登録する
# パスの percent エンコードは行わない（% を含むパスのみ誤検知しうる。実用上稀）

# 統合ディレクトリ（このファイルが置かれている `<data_dir>/shell-integration/zsh`）は
# 直後に ZDOTDIR を戻すと辿れなくなるので、先に控える。
# 新しい環境変数を増やさない（export すると子プロセスへ漏れる）ためにシェル変数で持つ
typeset -g _tako_zdotdir="${ZDOTDIR-}"

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

  # 入力予測（zsh-autosuggestions 同梱。Issue #600）
  #
  # なぜ .zshenv で source せず precmd まで遅らせるか:
  #   1) ユーザーが自分で zsh-autosuggestions を導入しているかは .zshrc を読み終える
  #      まで分からない。二重注入ガードは「最初のプロンプト」でしか正しく判定できない
  #   2) このプラグインは既存 widget を包む。zsh-syntax-highlighting 等より**後**に
  #      読み込むのが上流の推奨で、それは .zshrc の後ということ
  # ON/OFF は `<data_dir>/shell-integration/autosuggest`（tako が書く）を毎プロンプト
  # 見て決める。ファイル不在は ON（既定 ON）。$(<file) は zsh では fork しない
  if [[ -z ${TAKO_NO_AUTOSUGGESTIONS-} && -n $_tako_zdotdir ]]; then
    typeset -g _tako_as_plugin="${_tako_zdotdir:h}/zsh-autosuggestions/zsh-autosuggestions.zsh"
    typeset -g _tako_as_state="${_tako_zdotdir:h}/autosuggest"
    typeset -g _tako_as_owner=
    # 後続の precmd フックが $? を見ることがあるので、必ず 0 で返す
    # （tako のフックはこの時点で既に $? を潰しているため、挙動は現状どおり）
    _tako_autosuggest_sync() {
      # ユーザー自身が導入済みなら以後いっさい触らない（二重注入ガード）
      if [[ -z $_tako_as_owner ]] && (( ${+functions[_zsh_autosuggest_start]} )); then
        _tako_as_owner=user
      fi
      [[ $_tako_as_owner == user ]] && return 0

      local want=on
      [[ -r $_tako_as_state ]] && want="$(<$_tako_as_state)"

      if [[ $want == off ]]; then
        [[ $_tako_as_owner == tako ]] && _zsh_autosuggest_disable
        return 0
      fi
      if [[ $_tako_as_owner == tako ]]; then
        (( ${+_ZSH_AUTOSUGGEST_DISABLED} )) && _zsh_autosuggest_enable
        return 0
      fi
      [[ -r $_tako_as_plugin ]] || return 0
      builtin source "$_tako_as_plugin" || return 0
      _tako_as_owner=tako
      # プラグインが自分で登録する precmd フックは「次の」プロンプトからしか効かない
      # （zsh は precmd_functions を複製してから回すため）。最初のプロンプトから
      # 予測を出すためにここで直接呼ぶ
      _zsh_autosuggest_start
      return 0
    }
    precmd_functions+=(_tako_autosuggest_sync)
  fi
fi
