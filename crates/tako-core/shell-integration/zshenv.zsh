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

# tako CLI の PATH 注入（Issue #601）
#
# zip 配布では CLI が tako.app の中（Contents/MacOS/tako）にしか無く、外からは見えない。
# tako が開いたシェルの中だけで PATH に足して「tako の中では常に `tako` が打てる」ようにする。
# ~/.zshrc は書き換えないので tako の外の zsh は不変（この .zshenv は ZDOTDIR を直後に
# 戻すので、ペイン内で起動する孫 zsh も読み込まない = ここが走るのは各ペインの先頭シェルだけ）。
#
# 足すのは **PATH の末尾**、しかも `tako` が他に見つからないときだけ。ユーザーが自分で
# 通した実体（Homebrew の /opt/homebrew/bin/tako 等）の解決順は変えない。
# 逃げ道は TAKO_NO_PATH_INJECTION=1
_tako_add_cli_path() {
  [[ -n ${TAKO_NO_PATH_INJECTION-} || -z $_tako_zdotdir ]] && return 0
  local dir=
  # $(<file) は zsh では fork しない。tako が起動時に書く（不在・空 = 注入しない）
  [[ -r ${_tako_zdotdir:h}/cli-dir ]] && dir="$(<${_tako_zdotdir:h}/cli-dir)"
  [[ -n $dir && -x $dir/tako ]] || return 0
  # 既に入っている（親シェルからの継承を含む）なら二重に足さない。(Ie) は完全一致検索
  (( ${path[(Ie)$dir]} )) && return 0
  # ユーザーが自分で PATH を通しているなら手を出さない（解決順を変えない）
  (( ${+commands[tako]} )) && return 0
  # zsh の `path` は PATH と連動した配列。末尾へ足す
  path+=("$dir")
  return 0
}

# コマンドを与えられたシェル（`$SHELL -l -i -c <コマンド>` = コマンドペイン・
# エージェントペイン）はプロンプトが出ない = precmd フックが一度も回らないので、
# ここで足すしかない。判定に `-o interactive` だけを使わないのは #1031 で
# コマンドペインのラッパーが `-i` 付き（= 対話）になったため —— zsh は `-c` のとき
# だけ ZSH_EXECUTION_STRING を設定するので、それを「プロンプトが出ないシェル」の印にする。
# 素の対話シェルは ~/.zshrc を読み終えた後（precmd）に回す —— .zshrc や .zprofile で
# PATH を組み立てるユーザーは多く、この時点の PATH で「tako が既にあるか」を
# 判定すると必ず誤るため
if [[ -n ${TAKO_PANE_ID-} && ( ! -o interactive || -n ${ZSH_EXECUTION_STRING-} ) ]]; then
  _tako_add_cli_path
fi

if [[ -o interactive && -n ${TAKO_PANE_ID-} ]]; then
  # tako の tmux バックエンド（Phase 5.5 / FR-5）配下なら:
  # 1) OSC をパススルー（DCS tmux; … ST。allow-passthrough）で包み、外の tako へ届かせる
  # 2) TMUX / TMUX_PANE を unset し、ユーザー自身の tmux 利用（ネスト）を素通しにする
  #    （バックエンドは見えない裏方。素の `tmux` が今まで通り既定サーバーに繋がる）
  # 器かどうかは **tako が明示したソケット名**（TAKO_BACKEND_SOCKET）で判定する（#1105）。
  # 名前の接頭辞 `tako*` は、この env を渡さない古い tako が立てたセッション用の
  # フォールバック。接頭辞だけに頼っていたので、TAKO_TMUX_SOCKET に `tako` で
  # 始まらない名前を与えると統合が黙って無効化されていた（cwd 追従とコマンド状態が死ぬ）
  _tako_tmux=
  if [[ -n ${TMUX-} ]]; then
    _tako_sock=${${TMUX%%,*}:t}
    if [[ -n ${TAKO_BACKEND_SOCKET-} ]]; then
      # 右辺はクォートする（zsh の `==` は右辺をパターンとして扱う）
      [[ $_tako_sock == "${TAKO_BACKEND_SOCKET}" ]] && _tako_tmux=1
    elif [[ $_tako_sock == tako* ]]; then
      _tako_tmux=1
    fi
    unset _tako_sock
    [[ -n $_tako_tmux ]] && unset TMUX TMUX_PANE
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

  # tako CLI の PATH 注入（Issue #601）。最初のプロンプト直前 = ユーザーの .zshrc の
  # 後に一度だけ実行する。一度足せば PATH はこのシェルに残るのでフックから自分を外す
  # （zsh は precmd_functions を複製してから回すので、実行中の書き換えは安全）
  _tako_path_sync() {
    _tako_add_cli_path
    precmd_functions=(${precmd_functions:#_tako_path_sync})
    return 0
  }
  precmd_functions+=(_tako_path_sync)

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

    # 確定キーのヒント + Tab 確定（Issue #614）
    #
    # 予測（ゴースト）が出ていても確定キーが右矢印だと気づけない、という実機報告への対処。
    # 1) ゴーストの直後へ `[→ か Tab で確定]` を薄く出す（チュートリアル。既定 10 回で消える）
    # 2) **ゴーストが出ているときだけ** Tab を確定にし、それ以外は従来の補完へ委譲する
    #
    # ヒントは POSTDISPLAY（ゴースト本体と同じ場所）へ足す。これはプラグインが
    # 「確定するテキスト」として読む変数でもあるので、**プラグインが POSTDISPLAY に
    # 触る前後で必ず外す / 付け直す**必要がある。その唯一の関門が
    # `_zsh_autosuggest_highlight_reset`（全 widget の入口）と
    # `_zsh_autosuggest_highlight_apply`（全 widget の出口・`zle -R` の直前）なので、
    # この 2 つだけを包む。個々の accept 系 widget を包むより漏れが無く、
    # 非同期で予測が届く経路（`zle -F` → `autosuggest-suggest`）も同じ関門を通る
    typeset -g _tako_as_hint_state="${_tako_zdotdir:h}/autosuggest-hint"
    typeset -g _tako_as_hint_text_state="${_tako_zdotdir:h}/autosuggest-hint-text"
    typeset -g _tako_as_tab_state="${_tako_zdotdir:h}/autosuggest-tab"
    typeset -gi _tako_as_hint_left=0
    typeset -g _tako_as_hint_body=      # 表示する文言（tako が言語別に書く）
    typeset -g _tako_as_hint_suffix=    # 実際に付け足した文字列（除去に使う）
    typeset -g _tako_as_hint_hl=        # region_highlight へ足したエントリ
    typeset -g _tako_as_hint_line=      # この行で出すか（空 = 未決定 / show / hide）
    typeset -g _tako_as_tab_on=
    typeset -g _tako_as_tab_orig=       # 元々 ^I に割り当てられていた widget

    # 付け足したヒントを取り除く。**プラグインが POSTDISPLAY を読む前に必ず通る**
    _tako_as_hint_strip() {
      emulate -L zsh
      if [[ -n $_tako_as_hint_hl ]]; then
        region_highlight=("${(@)region_highlight:#$_tako_as_hint_hl}")
        _tako_as_hint_hl=
      fi
      if [[ -n $_tako_as_hint_suffix ]]; then
        [[ $POSTDISPLAY == *"$_tako_as_hint_suffix" ]] &&
          POSTDISPLAY=${POSTDISPLAY%"$_tako_as_hint_suffix"}
        _tako_as_hint_suffix=
      fi
      return 0
    }

    # 予測が確定した直後（描画の直前）にヒントを付け足す
    _tako_as_hint_apply() {
      emulate -L zsh
      (( $#POSTDISPLAY )) || { _tako_as_hint_strip; return 0 }
      [[ -n $_tako_as_hint_suffix ]] && return 0
      # 出すかどうかは**その行の最初の 1 回で決めて、行のあいだ変えない**。
      # 残り回数をここで直接見ると、消費した瞬間（残り 0）に同じ行の再描画から
      # 案内が消えてしまう。消費もこの 1 回だけ（1 打鍵ごとではない）
      if [[ -z $_tako_as_hint_line ]]; then
        if (( _tako_as_hint_left > 0 )) && [[ -n $_tako_as_hint_body ]]; then
          _tako_as_hint_line=show
          local -i n=$(( _tako_as_hint_left - 1 ))
          (( n < 0 )) && n=0
          _tako_as_hint_left=$n
          print -r -- $n >| $_tako_as_hint_state 2>/dev/null
        else
          _tako_as_hint_line=hide
        fi
      fi
      [[ $_tako_as_hint_line == show ]] || return 0
      local suffix="  $_tako_as_hint_body"
      # 行が溢れるくらいなら出さない（折り返してまで見せるものではない）
      (( $#BUFFER + $#POSTDISPLAY + $#suffix + 8 <= COLUMNS )) || return 0
      local -i start=$(( $#BUFFER + $#POSTDISPLAY ))
      POSTDISPLAY+="$suffix"
      _tako_as_hint_suffix="$suffix"
      # プラグインのハイライトは付け足す前の範囲で計算済みなので、ヒントぶんは自分で塗る
      _tako_as_hint_hl="$start $(( start + $#suffix )) $ZSH_AUTOSUGGEST_HIGHLIGHT_STYLE"
      region_highlight+=("$_tako_as_hint_hl")
      return 0
    }

    # ^I（Tab）: ゴーストがあれば確定、無ければ**元のバインドへ委譲**する。
    # 補完メニュー内の Tab は menuselect キーマップなのでここを通らない = 影響しない
    _tako_autosuggest_tab() {
      _tako_as_hint_strip
      if [[ -n $_tako_as_tab_on && -z ${TAKO_NO_AUTOSUGGEST_TAB-} ]] &&
         (( $#POSTDISPLAY )) && (( CURSOR == $#BUFFER )) && [[ $KEYMAP != vicmd ]]; then
        zle autosuggest-accept
        return 0
      fi
      if [[ -n $_tako_as_tab_orig ]] && (( ${+widgets[$_tako_as_tab_orig]} )); then
        zle "$_tako_as_tab_orig" -- "$@"
      else
        zle expand-or-complete -- "$@"
      fi
    }

    # プラグインを読み込んだ直後に一度だけ: 関門 2 つを包み、^I を差し替える
    _tako_autosuggest_hint_install() {
      emulate -L zsh
      if [[ $functions[_zsh_autosuggest_highlight_reset] != *_tako_as_hint_strip* ]]; then
        eval "_tako_as_orig_highlight_reset() { $functions[_zsh_autosuggest_highlight_reset] }"
        _zsh_autosuggest_highlight_reset() {
          _tako_as_hint_strip
          _tako_as_orig_highlight_reset "$@"
        }
      fi
      if [[ $functions[_zsh_autosuggest_highlight_apply] != *_tako_as_hint_apply* ]]; then
        eval "_tako_as_orig_highlight_apply() { $functions[_zsh_autosuggest_highlight_apply] }"
        _zsh_autosuggest_highlight_apply() {
          _tako_as_orig_highlight_apply "$@"
          _tako_as_hint_apply
        }
      fi
      if (( ! ${+widgets[tako-autosuggest-tab]} )); then
        # .zshrc の後なので、ユーザーが ^I を張り替えていればその widget が取れる
        _tako_as_tab_orig="${${(z)$(builtin bindkey '^I')}[2]}"
        [[ -z $_tako_as_tab_orig ||
           $_tako_as_tab_orig == (undefined-key|tako-autosuggest-tab) ]] &&
          _tako_as_tab_orig=expand-or-complete
        zle -N tako-autosuggest-tab _tako_autosuggest_tab
        builtin bindkey '^I' tako-autosuggest-tab
      fi
      return 0
    }

    # 毎プロンプト: 残り回数・文言・Tab 確定の可否を読み直す（既存ペインにも効かせる）
    _tako_autosuggest_hint_sync() {
      emulate -L zsh
      _tako_as_hint_line=
      local raw=
      [[ -r $_tako_as_tab_state ]] && raw="$(<$_tako_as_tab_state)"
      if [[ ${raw//[[:space:]]/} == off ]]; then
        _tako_as_tab_on=
      else
        _tako_as_tab_on=1
      fi

      raw=
      [[ -r $_tako_as_hint_state ]] && raw="$(<$_tako_as_hint_state)"
      raw=${raw//[[:space:]]/}
      if [[ -n ${TAKO_NO_AUTOSUGGEST_HINT-} || $raw == off ]]; then
        _tako_as_hint_left=0
        return 0
      elif [[ $raw == <-> ]]; then
        _tako_as_hint_left=$raw
      else
        # 不在・壊れた値は既定回数（tako 側の TAKO_AUTOSUGGEST_HINT_DEFAULT と揃える）
        _tako_as_hint_left=10
      fi

      # 文言は tako が言語別に書く。1 行目 = Tab 確定あり、2 行目 = Tab 確定なし
      local -a texts=()
      [[ -r $_tako_as_hint_text_state ]] && texts=("${(@f)$(<$_tako_as_hint_text_state)}")
      if [[ -n $_tako_as_tab_on ]]; then
        _tako_as_hint_body=${texts[1]-}
      else
        _tako_as_hint_body=${texts[2]-}
      fi
      return 0
    }

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
        _tako_autosuggest_hint_sync
        return 0
      fi
      [[ -r $_tako_as_plugin ]] || return 0
      builtin source "$_tako_as_plugin" || return 0
      # 自前の Tab widget はプラグインに包ませない（包まれると POSTDISPLAY が
      # 空にされた状態で呼ばれ、ゴーストの有無を判定できなくなる）
      ZSH_AUTOSUGGEST_IGNORE_WIDGETS+=(tako-autosuggest-tab)
      _tako_as_owner=tako
      # プラグインが自分で登録する precmd フックは「次の」プロンプトからしか効かない
      # （zsh は precmd_functions を複製してから回すため）。最初のプロンプトから
      # 予測を出すためにここで直接呼ぶ
      _zsh_autosuggest_start
      # 確定キーのヒント + Tab 確定（#614）。プラグインを読んだ後にしか仕掛けられない
      _tako_autosuggest_hint_install
      _tako_autosuggest_hint_sync
      return 0
    }
    precmd_functions+=(_tako_autosuggest_sync)
  fi
fi
