# tako シェル統合（fish）— OSC 7（cwd）/ OSC 133（プロンプトマーク）発行。FR-2.4.1
#
# tako は XDG_DATA_DIRS にこのファイルの vendor_conf.d を前置して起動する。
# fish が自動で source する（ユーザー設定の変更は不要）

status is-interactive; or exit
set -q TAKO_PANE_ID; or exit

# tako の tmux バックエンド（Phase 5.5 / FR-5）配下なら OSC をパススルーで包み、
# TMUX を unset してユーザー自身の tmux 利用（ネスト）を素通しにする（zsh 版と同じ）
set -g _tako_tmux ''
if set -q TMUX
    set -l sock (string split ',' -- $TMUX)[1]
    if string match -qr '/tako[^/]*$' -- $sock
        set -g _tako_tmux 1
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
function _tako_prompt --on-event fish_prompt
    _tako_emit (printf '\e]133;A\a')
end
_tako_report_cwd
