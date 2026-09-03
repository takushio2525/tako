# tako シェル統合（fish）— OSC 7（cwd）/ OSC 133（プロンプトマーク）発行。FR-2.4.1
#
# tako は XDG_DATA_DIRS にこのファイルの vendor_conf.d を前置して起動する。
# fish が自動で source する（ユーザー設定の変更は不要）

status is-interactive; or exit
set -q TAKO_PANE_ID; or exit

# tako の tmux バックエンド（Phase 5.5 / FR-5）配下なら OSC をパススルーで包み、
# TMUX を unset してユーザー自身の tmux 利用（ネスト）を素通しにする（zsh 版と同じ）
# 器の判定は tako が明示したソケット名（TAKO_BACKEND_SOCKET）を優先。
# 接頭辞 `tako*` は古い tako 用のフォールバック（#1105。zsh 版と同じ）
set -g _tako_tmux ''
if set -q TMUX
    set -l sock (string split ',' -- $TMUX)[1]
    set -l name (string replace -r '^.*/' '' -- $sock)
    if set -q TAKO_BACKEND_SOCKET
        if test "$name" = "$TAKO_BACKEND_SOCKET"
            set -g _tako_tmux 1
        end
    else if string match -q 'tako*' -- $name
        set -g _tako_tmux 1
    end
    if test -n "$_tako_tmux"
        set -e TMUX
        set -e TMUX_PANE
    end
end

function _tako_emit
    if test -n "$_tako_tmux"
        # パススルー内の ESC は二重化する（tmux の仕様）
        set -l body (string replace -a \e \e\e -- $argv[1])
        printf '\ePtmux;%s\e\\' $body
    else
        printf '%s' $argv[1]
    end
end

function _tako_report_cwd --on-variable PWD
    _tako_emit (printf '\e]7;file://%s%s\a' (hostname) "$PWD")
end
function _tako_preexec --on-event fish_preexec
    set -g _tako_ran_command 1
    _tako_emit (printf '\e]133;C\a')
end
function _tako_postexec --on-event fish_postexec
    set -l ret $status
    _tako_emit (printf '\e]133;D;%d\a' $ret)
end
# tako CLI の PATH 注入（Issue #601）
#
# このファイルは vendor_conf.d 経由で config.fish より**前**に読まれるので、
# 「ユーザーが自分で tako を通しているか」はここでは判定できない。判定と注入は最初の
# プロンプト（= config.fish の後）まで遅らせる。足すのは PATH の末尾、しかも `tako` が
# 見つからないときだけ。設定ファイルは書き換えず universal 変数も使わないので、
# tako の外の fish は不変。逃げ道は TAKO_NO_PATH_INJECTION=1
#
# 自分の置き場所 `<統合ルート>/fish-data/fish/vendor_conf.d/tako.fish` から統合ルートを
# 逆算する（外部コマンドを起こさない。構成が変われば一致せず、単に何もしない）
set -g _tako_root (string replace -r '/fish-data/fish/vendor_conf\.d/[^/]*$' '' -- (status filename))

function _tako_add_cli_path
    set -q TAKO_NO_PATH_INJECTION; and return
    test -r $_tako_root/cli-dir; or return
    read -l dir <$_tako_root/cli-dir
    test -n "$dir" -a -x "$dir/tako"; or return
    contains -- $dir $PATH; and return
    type -q tako; and return
    set -gx PATH $PATH $dir
end

function _tako_prompt --on-event fish_prompt
    _tako_emit (printf '\e]133;A\a')
    if not set -q _tako_path_done
        set -g _tako_path_done 1
        _tako_add_cli_path
    end
end
_tako_report_cwd
