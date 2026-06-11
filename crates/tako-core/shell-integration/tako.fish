# tako シェル統合（fish）— OSC 7（cwd）/ OSC 133（プロンプトマーク）発行。FR-2.4.1
#
# tako は XDG_DATA_DIRS にこのファイルの vendor_conf.d を前置して起動する。
# fish が自動で source する（ユーザー設定の変更は不要）

status is-interactive; or exit
set -q TAKO_PANE_ID; or exit

function _tako_report_cwd --on-variable PWD
    printf '\e]7;file://%s%s\a' (hostname) "$PWD"
end
function _tako_preexec --on-event fish_preexec
    set -g _tako_ran_command 1
    printf '\e]133;C\a'
end
function _tako_postexec --on-event fish_postexec
    printf '\e]133;D;%d\a' $status
end
function _tako_prompt --on-event fish_prompt
    printf '\e]133;A\a'
end
_tako_report_cwd
