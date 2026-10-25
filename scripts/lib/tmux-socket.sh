# tmux-socket.sh — 検証スクリプトが起こした隔離 tmux の器を、ソケットファイルまで片付ける 1 実装（Issue #2399）
#
# tmux（3.6b で実測）は `kill-server` で落としても、最後のセッションが閉じて自分で抜けても
# `$TMUX_TMPDIR/tmux-<uid>/<名前>` のソケットファイルを残す。`kill-server` だけの後片付けは
# 走るたびに 1 個ずつためる（棚卸しの数は .agent/conventions.md の同じ節）。
# Rust のテストの側の 1 実装は `tako_core::tmux_backend::kill_server`（#2393）。
#
#   . "$REPO_ROOT/scripts/lib/tmux-socket.sh"
#   cleanup() { …; tmux_socket_kill "$SOCKET"; }   # `tmux -L "$SOCKET" kill-server` の代わり
#   trap cleanup EXIT                              # 途中で落ちた経路でも片付く
#
# - 名前はいくつでも渡せる。**空の名前は何もしない**（器を起こす前に後片付けが走った = 正常）
# - `tako`（本番の器）・`default`（tmux の既定）・`/` を含む名前は触らずに stderr へ 1 行出す
# - ソケットファイルを消すのは「落とせた（終了コード 0）」か「もう居ない（no server running）」のときだけ。
#   それ以外（Permission denied・版の食い違い）は器が生きているかもしれないので**ファイルを残す**
#   （消すと名前で辿れない器が残り、`tako tmux cleanup --servers` からも見えなくなる）
# - tmux は `REAL_TMUX`（PATH を偽の tmux へ差し替えたスクリプトが本物を握る変数）→ `tmux` の順で引く
# - 常に 0 を返し、呼び手の `set -e` の下でも途中で抜けない（後片付けの途中で止めない）
#
# 番犬: crates/tako-control/tests/shell_scripts.rs の `器を起こすスクリプトはソケットファイルまで消す` が、
# 器を起こす（`TAKO_PERSIST=1` で GUI を起こす / `tmux -L … new-session`）スクリプトの
# `tmux -L … kill-server` を file:line で名指しする。規約は .agent/conventions.md の
# 「検証スクリプトの器はソケットファイルまで消す（Issue #2399）」節。

# tmux_socket_kill <名前>...
tmux_socket_kill() {
  local name out rc dir
  dir="${TMUX_TMPDIR:-/tmp}/tmux-$(id -u)"
  for name in "$@"; do
    case "$name" in
      '') continue ;;
      tako | default | */* | . | ..)
        echo "tmux_socket_kill: '${name}' は検証用の器の名前ではないので触らない" >&2
        continue
        ;;
    esac
    # `|| rc=$?` で受ける（素の代入は呼び手の `set -e` の下で失敗がスクリプトごと抜ける =
    # EXIT の trap なら後ろの後片付けが飛ぶ）
    rc=0
    out="$("${REAL_TMUX:-tmux}" -L "$name" kill-server 2>&1)" || rc=$?
    case "$rc:$out" in
      0:* | *"no server running"*) rm -f "${dir:?}/${name:?}" ;;
      # ファイルが無い / tmux が無い = 器も無いので黙って抜ける
      *"No such file or directory"* | 127:*) : ;;
      *) echo "tmux_socket_kill: 器 ${name} を落とせないのでソケットファイルを残す（${out}）" >&2 ;;
    esac
  done
  return 0
}
