#!/bin/bash
# measure-shutdown-hup-2633.sh — #2633 の段 1: 製品の GUI の終了とログインシェルの問い合わせの打ち切りが、
# rc の初期化中の `pyenv rehash` を途中で断ってロックを残すかを、修正前の木のまま実測する
#
# 測る経路（各 RUNS 回。既定 5）:
#   (a) 直接ペイン（器なし = 設定の tmux_persist が off / tmux が無いときの製品のペイン）の rc の初期化中に
#       GUI を終了する。終了は製品の Cmd+Q と同じ口（SIGTERM → cx.quit → on_app_quit。#777）
#   (b) 器（tmux）のペインの rc の初期化中に GUI を終了する（ソケットは明示 = GUI の終了で器を落とさない
#       製品の既定と同じ。使い捨ての器は GUI の pid から作った名前のときだけ落とされる）
#   (c) ログインシェルへの問い合わせ（`probe::group_output_with_timeout`。`zsh -l -c` を自分のグループで起こし、
#       上限（既定 5 秒）で `kill(-pgid, SIGKILL)`）が rc の初期化中に打ち切られる。入口は製品の CLI の
#       `tako check-health`（CLI が自分で診断する経路 = exe::find がログインシェルへ聞く）
#
# 砂場: HOME は `mktemp -d` の中（利用者のホームの外 = isolated-gui.sh はペインの zsh を砂場の ZDOTDIR へ
# 向けず、呼び手の HOME の rc をそのまま読む = 製品のペインと同じ読み方）。本物の ~/.pyenv と利用者の rc は
# 使わない・書かない。偽の pyenv:
#   - 入口 `pyenv`（C。下の SIGWATCH_C）: 子を fork して libexec/pyenv-rehash を exec し（本物の pyenv も
#     bash の入口から rehash を exec する = rehash の bash が 1 本）、子を待ちながら**届いたシグナルを送り主の
#     pid（si_pid）つきで記録する**。(a)(b) はグループに居残って子と同じシグナルを受け（inside）、(c) は
#     自分だけ新しいグループへ出て（outside）グループごとの SIGKILL を受けずに子の死に方を記録する
#   - libexec/pyenv-rehash（bash）: 本物（Homebrew の pyenv 2.x）の取り方をそのまま写す = EXIT の trap を
#     先に置き、noclobber でロック（shims/.pyenv-shim）を作り、取れなければ 0.1 秒ずつ PYENV_REHASH_TIMEOUT
#     秒まで待つ。作業の代わりに FAKE_REHASH_SECS 秒眠る（ロックを持つ窓を広げる注入）。trap の入口と rm の
#     結果に印を書く
#
# 本番の GUI / data dir / tmux には触らない。落とすのは自分で起こした pid と自分のソケットだけ。
# 窓は tako-vd（用意できなければ終了コード 4 = 未実測）。描画は要らない（ファイルとシグナルの記録だけを見る）。
#
# 補足（既定で回す）:
#   a32: (a) の rehash の解釈系だけ macOS 同梱の bash 3.2 に替える
#   pty: 送り主の読み方の対照（forkpty）。macOS はセッションリーダー（制御プロセス）の終了でカーネルが前面の
#        グループへ送る HUP の si_pid を「終了したリーダーの pid」と記録する（mode 1）。PTY の親側を閉じても
#        前面のグループには届かない（リーダーにだけ届く。mode 2）。だから (a) の「from zsh」2 回は
#        「zsh がジョブへ中継した 1 回 + zsh の終了でカーネルが送った 1 回」と読む
# 対照: HOLD_GUI=0（rehash が終わった後に GUI を終了）・HOLD_PROBE=0（rehash が上限より早く終わる）
#
# 使い方: bash scripts/measure-shutdown-hup-2633.sh   （ONLY=a|b|a32|c|pty で 1 つだけ・RUNS=N で回数・KEEP_TMP=1 で一時 dir を残す）
set -uo pipefail

unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_CLI TAKO_MCP_URL CLAUDE_CODE_PLUGIN_DIRS \
  TAKO_ORCHESTRATOR_ROLE TAKO_BACKEND_SOCKET TAKO_OSC_SINK TAKO_TMUX_SOCKET ZDOTDIR \
  TAKO_ISO_USER_RC TAKO_2627_LEGACY TAKO_LOGIN_SHELL_TIMEOUT_SECS PYENV_ROOT PYENV_REHASH_TIMEOUT
unset CLAUDE_CODE_CHILD_SESSION CLAUDE_CODE_SESSION_ID CLAUDECODE CLAUDE_CONFIG_DIR

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
RUNS="${RUNS:-5}"
ONLY="${ONLY:-}"
# (a)(b): rehash がロックを持つ秒（GUI の終了がその間に来るように長め）
HOLD_GUI="${HOLD_GUI:-20}"
# (c): 問い合わせの上限（既定 5 秒）より長く持つ
HOLD_PROBE="${HOLD_PROBE:-10}"
# 製品の .app を Finder から起こしたときの PATH（launchd の既定）。brew の PATH は砂場の .zprofile が足す
BASE_PATH=/usr/bin:/bin:/usr/sbin:/sbin
BREW_PREFIX=/opt/homebrew
TMUX_BIN="$BREW_PREFIX/bin/tmux"
TMUX_SOCK_DIR="${TMUX_TMPDIR:-/private/tmp}/tmux-$(id -u)"

want() { [ -z "$ONLY" ] || [ "$ONLY" = "$1" ]; }
now_ms() { perl -MTime::HiRes=time -e 'printf "%d\n", time * 1000'; }

# shellcheck source=lib/isolated-gui.sh
. "$REPO_ROOT/scripts/lib/isolated-gui.sh"
# ビルドは HOME を差し替える前に（後だとツールチェーンを砂場へ取り直す）
isolated_gui_bins || exit 1
command -v cc >/dev/null || { echo "cc が要る"; exit 1; }
[ -x "$TMUX_BIN" ] || { echo "tmux が要る（${TMUX_BIN}）"; exit 1; }

TMP="$(mktemp -d /tmp/t2633-XXXXXX)"
TMP="$(cd "$TMP" && pwd -P)"
OUT="${OUT:-$TMP/report.tsv}"
APP_PID=""
MY_SOCKETS=""
cleanup() {
  local s
  [ -n "$APP_PID" ] && stop_isolated_gui "$APP_PID"
  # 自分のソケットだけ: サーバーを止め、止めた後に残るソケットのファイルも消す
  for s in $MY_SOCKETS; do
    "$TMUX_BIN" -L "$s" kill-server >/dev/null 2>&1 || true
    [ -S "$TMUX_SOCK_DIR/$s" ] && rm -f "${TMUX_SOCK_DIR:?}/${s:?}"
  done
  [ -n "${KEEP_TMP:-}" ] && { echo "一時 dir を残した: $TMP"; return; }
  rm -rf "${TMP:?}"
}
trap cleanup EXIT

# ---- 観測器（偽の pyenv の入口）----------------------------------------------------------------
SIGWATCH_C='
#include <errno.h>
#include <fcntl.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>

#define MAXEV 64
static volatile sig_atomic_t nev = 0;
static int ev_sig[MAXEV];
static pid_t ev_pid[MAXEV];
static int ev_code[MAXEV];
static long long ev_ms[MAXEV];
static const int WATCH[] = {SIGHUP, SIGINT, SIGQUIT, SIGTERM, SIGCONT, SIGPIPE, SIGTSTP, SIGTTIN, SIGTTOU, SIGUSR1, SIGUSR2, SIGALRM};

static long long now_ms(void) {
  struct timespec ts;
  clock_gettime(CLOCK_REALTIME, &ts);
  return (long long)ts.tv_sec * 1000 + ts.tv_nsec / 1000000;
}

static void on_sig(int sig, siginfo_t *si, void *uc) {
  (void)uc;
  int i = nev;
  if (i < MAXEV) {
    ev_sig[i] = sig;
    ev_pid[i] = si ? si->si_pid : -1;
    ev_code[i] = si ? si->si_code : -1;
    ev_ms[i] = now_ms();
    nev = i + 1;
  }
}

static FILE *logf;
static int flushed = 0;
static void flush_events(void) {
  while (flushed < nev) {
    fprintf(logf, "t=%lld ev=signal sig=%s from=%d code=%d\n", ev_ms[flushed], strsignal(ev_sig[flushed]), (int)ev_pid[flushed], ev_code[flushed]);
    flushed++;
  }
  fflush(logf);
}

int main(int argc, char **argv) {
  const char *root = getenv("PYENV_ROOT");
  const char *logp = getenv("FAKE_WATCH_LOG");
  const char *mode = getenv("FAKE_WATCH_MODE");
  const char *linger_s = getenv("FAKE_WATCH_LINGER_MS");
  long linger = linger_s ? atol(linger_s) : 3000;
  if (argc < 2 || !root || !logp) return 2;
  char path[4096];
  snprintf(path, sizeof path, "%s/libexec/pyenv-%s", root, argv[1]);
  logf = fopen(logp, "a");
  if (!logf) return 2;
  int fg = isatty(0) ? (int)tcgetpgrp(0) : -1;
  pid_t parent_pgid = getpgrp();
  pid_t child = fork();
  if (child < 0) return 2;
  if (child == 0) {
    argv[1] = path;
    execv(path, argv + 1);
    _exit(127);
  }
  const char *where = "inside";
  /* グループの頭（zsh -c の最後のコマンドとして exec された等）は新しいグループを作れない = 出られない */
  if (mode && strcmp(mode, "outside") == 0) where = (setpgid(0, 0) == 0 && getpgrp() != parent_pgid) ? "outside" : "outside-failed";
  /* 自分は端末もパイプも持たない（本物の pyenv は rehash の bash 1 本。問い合わせのパイプの読み切りを延ばさない） */
  int null = open("/dev/null", O_RDWR);
  if (null >= 0) { dup2(null, 0); dup2(null, 1); dup2(null, 2); if (null > 2) close(null); }
  struct sigaction sa;
  memset(&sa, 0, sizeof sa);
  sa.sa_sigaction = on_sig;
  sa.sa_flags = SA_SIGINFO;
  sigemptyset(&sa.sa_mask);
  for (size_t i = 0; i < sizeof WATCH / sizeof WATCH[0]; i++) sigaddset(&sa.sa_mask, WATCH[i]);
  for (size_t i = 0; i < sizeof WATCH / sizeof WATCH[0]; i++) sigaction(WATCH[i], &sa, NULL);
  fprintf(logf, "t=%lld ev=start watcher=%d ppid=%d child=%d pgid_before=%d pgid_now=%d sid=%d tty_fg=%d where=%s\n",
          now_ms(), (int)getpid(), (int)getppid(), (int)child, (int)parent_pgid, (int)getpgrp(), (int)getsid(0), fg, where);
  fflush(logf);
  int status = 0;
  for (;;) {
    pid_t r = waitpid(child, &status, 0);
    flush_events();
    if (r == child) break;
    if (r < 0 && errno != EINTR) { fprintf(logf, "t=%lld ev=wait-error errno=%d\n", now_ms(), errno); break; }
  }
  if (WIFSIGNALED(status))
    fprintf(logf, "t=%lld ev=child-signaled child=%d sig=%s\n", now_ms(), (int)child, strsignal(WTERMSIG(status)));
  else
    fprintf(logf, "t=%lld ev=child-exit child=%d code=%d\n", now_ms(), (int)child, WEXITSTATUS(status));
  fflush(logf);
  /* 子が終わった後に届く分（zsh の終了でカーネルが前面のグループへ送る HUP 等）も拾う */
  long long until = now_ms() + linger;
  while (now_ms() < until) { usleep(50000); flush_events(); }
  fprintf(logf, "t=%lld ev=watch-end\n", now_ms());
  fflush(logf);
  return WIFSIGNALED(status) ? 128 + WTERMSIG(status) : WEXITSTATUS(status);
}
'

# 偽の rehash（本物の Homebrew の pyenv 2.x の libexec/pyenv-rehash の取り方を写す。作業の代わりに眠る）
REHASH_SH='#!/usr/bin/env bash
set -e
SHIM_PATH="${PYENV_ROOT}/shims"
PROTOTYPE_SHIM_PATH="${SHIM_PATH}/.pyenv-shim"
LOG="$PYENV_ROOT/rehash.log"
echo "start pid=$$ bash=$BASH_VERSION" >> "$LOG"
mkdir -p "$SHIM_PATH"

acquire_lock() {
  local ret
  set -o noclobber
  echo > "$PROTOTYPE_SHIM_PATH" 2>| /dev/null || ret=1
  set +o noclobber
  [ -z "${ret}" ]
}

trap release_lock EXIT

remove_prototype_shim() {
  rm -f "$PROTOTYPE_SHIM_PATH"
}

release_lock() {
  echo "trap-enter pid=$$" >> "$LOG"
  if remove_prototype_shim; then echo "trap-rm-ok pid=$$" >> "$LOG"; else echo "trap-rm-fail rc=$? pid=$$" >> "$LOG"; fi
}

unset acquired
start=$SECONDS
while (( SECONDS <= start + ${PYENV_REHASH_TIMEOUT:-60} )); do
  if acquire_lock 2>/dev/null; then
    acquired=1
    break
  else
    sleep 0.1 2>/dev/null || sleep 1
  fi
done

if [ -z "${acquired}" ]; then
  echo "pyenv: cannot rehash: $PROTOTYPE_SHIM_PATH exists"
  echo "gave-up pid=$$" >> "$LOG"
  exit 1
fi
echo "locked pid=$$" >> "$LOG"
sleep "${FAKE_REHASH_SECS:-0}"
echo "done pid=$$" >> "$LOG"
'

# 砂場の HOME を 1 つ作る: make_home <dir> <rehash を置く rc（.zshrc | .zprofile）>
make_home() {
  local h="$1" rcfile="$2" pr="$1/.pyenv"
  mkdir -p "$pr/bin" "$pr/libexec" || return 1
  printf '%s' "$SIGWATCH_C" > "$TMP/sigwatch.c"
  cc -O1 -o "$pr/bin/pyenv" "$TMP/sigwatch.c" || return 1
  # 解釈系: 既定は本物と同じ `/usr/bin/env bash`（PATH 次第。brew の bash が先なら 5.x）。REHASH_SHEBANG で差し替える
  printf '%s' "$REHASH_SH" | sed "1s|.*|${REHASH_SHEBANG:-#!/usr/bin/env bash}|" > "$pr/libexec/pyenv-rehash"
  chmod +x "$pr/libexec/pyenv-rehash"
  # brew の PATH（Homebrew の利用者の .zprofile の brew shellenv の代役）
  printf '%s\n' "export PATH=\"$BREW_PREFIX/bin:$BREW_PREFIX/sbin:\$PATH\"" > "$h/.zprofile"
  # pyenv init の rehash の形（`pyenv init -` が出す `command pyenv rehash 2>/dev/null`）
  cat >> "$h/$rcfile" <<'EOF'
print -r -- "rc-read file=RCFILE pid=$$ ppid=$PPID argv0=$0 opts=$- zdotdir=${ZDOTDIR-unset} home=$HOME pane=${TAKO_PANE_ID-}" >> "$HOME/rc-read.log"
export PYENV_ROOT="$HOME/.pyenv"
export PATH="$PYENV_ROOT/bin:$PATH"
command pyenv rehash 2>/dev/null
EOF
  sed -i '' "s/RCFILE/$rcfile/" "$h/$rcfile"
  [ "$rcfile" = .zshrc ] && printf '%s\n' "PROMPT='sandbox %% '" "RPROMPT=''" >> "$h/.zshrc"
  return 0
}

# 届いたシグナルの送り主を名前へ: who <pid>（GUI / ペインの zsh / その他）。zsh の終了でカーネルが送る HUP も
# macOS は送り主を終了した zsh の pid と記録する（pty の対照）ので「zsh」に入る
who() {
  case "$1" in
    0) echo kernel ;;
    "$CUR_GUI") echo gui ;;
    "$CUR_ZSH") echo zsh ;;
    "$CUR_WATCHER") echo self ;;
    *) echo "pid$1" ;;
  esac
}
# 観測器の記録を 1 行へ: summarize <watch.log> <基準の ms> → "signals=[…] 子(rehash) は …"
summarize() {
  local log="$1" t0="$2" line sigs="" child=""
  while read -r line; do
    case "$line" in
      *ev=signal*)
        local t sig from
        t=$(sed -n 's/^t=\([0-9]*\).*/\1/p' <<<"$line")
        sig=$(sed -n 's/.* sig=\(.*\) from=.*/\1/p' <<<"$line")
        from=$(sed -n 's/.* from=\([-0-9]*\).*/\1/p' <<<"$line")
        sigs="${sigs:+$sigs → }${sig}(from $(who "$from"), +$((t - t0))ms)"
        ;;
      *ev=child-signaled*)
        local t
        t=$(sed -n 's/^t=\([0-9]*\).*/\1/p' <<<"$line")
        child="子(rehash) は ${line##*sig=} で死亡 +$((t - t0))ms"
        ;;
      *ev=child-exit*)
        local t
        t=$(sed -n 's/^t=\([0-9]*\).*/\1/p' <<<"$line")
        child="子(rehash) は正常終了 code=${line##*code=} +$((t - t0))ms"
        ;;
    esac
  done < "$log"
  echo "signals=[${sigs:-なし}] ${child:-子の結末の記録なし}"
}

wait_for() { # wait_for <秒> <条件の関数>
  local i
  for i in $(seq 1 $(($1 * 10))); do
    "$2" && return 0
    sleep 0.1
  done
  return 1
}

# 本番側の控え（測定の前後で比べる）
real_lock="$(iso_real_home)/.pyenv/shims/.pyenv-shim"
prod_state() {
  local lock gui tmuxs
  lock=$([ -e "$real_lock" ] && echo present || echo absent)
  gui=$(ps -A -o pid=,args= | awk '$2 == "/Applications/tako.app/Contents/MacOS/tako-app" { print $1 }' | tr '\n' ' ')
  tmuxs=$("$TMUX_BIN" -L tako list-sessions -F '#{session_name}' 2>/dev/null | wc -l | tr -d ' ')
  echo "利用者の ~/.pyenv のロック=${lock} 本番 GUI pid=${gui:-なし} 本番 tmux のセッション数=${tmuxs}"
}
PROD_BEFORE=$(prod_state)
echo "測定前の本番: $PROD_BEFORE"
printf 'case\trun\tlock_after\ttrap\tsignals\tchild\textra\n' > "$OUT"

# ---- pty: 送り主の読み方の対照 ------------------------------------------------------------------------
# forkpty でセッションリーダー L（制御端末つき）と、L が前面にしたグループ G（HUP と CONT を送り主つきで記録）を作る。
#   mode 1: L は誰にもシグナルを送らずに終了する → G が受けた HUP の送り主の記録を見る
#   mode 2: L は HUP を無視して居残り、親が PTY の親側を閉じる → G に届くかを見る
PTYEXP_C='
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <util.h>
#include <sys/wait.h>
static volatile sig_atomic_t n = 0;
static pid_t from[16];
static int sigs[16];
static void h(int s, siginfo_t *si, void *u) { (void)u; if (n < 16) { sigs[n] = s; from[n] = si->si_pid; n++; } }
int main(int argc, char **argv) {
  int mode = argc > 1 ? atoi(argv[1]) : 1;
  int fds[2];
  if (pipe(fds) != 0) return 2;
  int master;
  pid_t L = forkpty(&master, NULL, NULL, NULL);
  if (L == 0) {
    pid_t G = fork();
    if (G == 0) {
      setpgid(0, 0);
      struct sigaction sa;
      memset(&sa, 0, sizeof sa);
      sa.sa_sigaction = h;
      sa.sa_flags = SA_SIGINFO;
      sigaction(SIGHUP, &sa, NULL);
      sigaction(SIGCONT, &sa, NULL);
      for (int i = 0; i < 40; i++) usleep(100000);
      char buf[512];
      int len = snprintf(buf, sizeof buf, "sid=%d got=%d", (int)getsid(0), (int)n);
      for (int i = 0; i < n; i++) len += snprintf(buf + len, sizeof buf - len, " %s:%d", sigs[i] == SIGHUP ? "HUP" : "CONT", (int)from[i]);
      buf[len++] = 10;
      write(fds[1], buf, len);
      _exit(0);
    }
    setpgid(G, G);
    tcsetpgrp(0, G);
    if (mode == 2) { signal(SIGHUP, SIG_IGN); sleep(6); _exit(0); }
    usleep(500000);
    _exit(0);
  }
  close(fds[1]);
  if (mode == 2) { usleep(500000); close(master); }
  char out[512];
  ssize_t k = read(fds[0], out, sizeof out - 1);
  if (k > 0) { out[k] = 0; printf("mode=%d L=%d %s", mode, (int)L, out); }
  int st;
  waitpid(L, &st, 0);
  return 0;
}
'
if want pty; then
  echo "=== pty: 送り主の読み方の対照（forkpty）==="
  printf '%s' "$PTYEXP_C" > "$TMP/ptyexp.c"
  cc -O1 -o "$TMP/ptyexp" "$TMP/ptyexp.c" 2>/dev/null || { echo "ptyexp をビルドできない"; exit 1; }
  m1=$("$TMP/ptyexp" 1)
  m2=$("$TMP/ptyexp" 2)
  echo "  mode 1（L はシグナルを送らずに終了）: $m1"
  echo "  mode 2（L は HUP を無視・親が親側を閉じる）: $m2"
  l1=$(sed -n 's/.* L=\([0-9]*\) .*/\1/p' <<<"$m1")
  case "$m1" in *" got=1 HUP:$l1"*) echo "  → カーネルが前面のグループへ送った HUP 1 回の送り主は「終了したリーダー」と記録される" ;; *) echo "  → 想定外（mode 1 の記録が HUP 1 回・送り主 L ではない）" ;; esac
  case "$m2" in *" got=0"*) echo "  → 親側を閉じても前面のグループには届かない" ;; *) echo "  → 想定外（mode 2 で前面のグループに届いた）" ;; esac
fi

# ---- (a)(b) GUI の終了 ------------------------------------------------------------------------------
# (a)(b) は rehash を bash 5.x（brew）で、補足の a32 は macOS 同梱の bash 3.2 で走らせる
mkdir -p "$TMP/home-gui" "$TMP/home-gui32"
make_home "$TMP/home-gui" .zshrc || { echo "砂場を作れない"; exit 1; }
REHASH_SHEBANG="#!/bin/bash" make_home "$TMP/home-gui32" .zshrc || { echo "砂場を作れない"; exit 1; }
use_home() { H_GUI="$1"; PR_GUI="$H_GUI/.pyenv"; LOCK_GUI="$PR_GUI/shims/.pyenv-shim"; }
use_home "$TMP/home-gui"

locked_now() { grep -q '^locked' "$PR_GUI/rehash.log" 2>/dev/null; }
child_done() { grep -q 'ev=watch-end' "$PR_GUI/watch.log" 2>/dev/null; }
gui_gone() { ! kill -0 "$CUR_GUI" 2>/dev/null; }

# 1 回分: gui_case <a|b|a32> <n>
gui_case() {
  local kind="$1" n="$2" run sock t_quitt_gone state trapm sum extra="" pane_line="" survivors
  run="$TMP/$kind-$n"
  mkdir -p "$run/data"
  rm -f "${LOCK_GUI:?}" "${PR_GUI:?}/rehash.log" "${PR_GUI:?}/watch.log" "${H_GUI:?}/rc-read.log"
  sock="t2633$kind$n-$$"
  MY_SOCKETS="$MY_SOCKETS $sock"
  local -a persist=()
  [ "$kind" = b ] && persist=(TAKO_PERSIST=1)
  launch_isolated_gui "$run/app.log" HOME="$H_GUI" SHELL=/bin/zsh PATH="$BASE_PATH" \
    TAKO_DATA_DIR="$run/data" TAKO_TMUX_SOCKET="$sock" ${persist[@]+"${persist[@]}"} \
    FAKE_REHASH_SECS="$HOLD_GUI" FAKE_WATCH_LOG="$PR_GUI/watch.log" FAKE_WATCH_MODE=inside || exit $?
  APP_PID=$ISOLATED_GUI_PID
  CUR_GUI=$APP_PID
  [ -n "$ISOLATED_GUI_ZDOTDIR" ] && extra="砂場ZDOTDIRを向けた(想定外) "
  if ! wait_for 90 locked_now; then
    echo "  $kind $n: 90 秒たっても rehash がロックを取らない（ペインの zsh が rc を読んでいない）"
    tail -5 "$run/app.log"
    stop_isolated_gui "$APP_PID"; APP_PID=""
    printf '%s\t%s\t-\t-\t-\t-\t無効(ロック無し)\n' "$kind" "$n" >> "$OUT"
    return
  fi
  CUR_ZSH=$(sed -n 's/.* pid=\([0-9]*\) ppid=.*/\1/p' "$H_GUI/rc-read.log" | tail -1)
  CUR_WATCHER=$(sed -n 's/.*ev=start watcher=\([0-9]*\).*/\1/p' "$PR_GUI/watch.log" | tail -1)
  pane_line=$(ps -o pid=,ppid=,pgid=,tty=,args= -p "$CUR_ZSH" 2>/dev/null | sed 's/  */ /g')
  local zsh_parent
  zsh_parent=$(ps -o ppid= -p "$CUR_ZSH" 2>/dev/null | tr -d ' ')
  pane_line="$pane_line / 親: $(ps -o args= -p "$zsh_parent" 2>/dev/null | cut -c1-80)"
  # 控え: GUI の子孫のペインのプロセス（終了後に居残りを数える。撃つのは最後の片付けだけ）
  local panes
  panes=$(iso_pane_processes "$APP_PID")
  sleep 1
  locked_now && [ -e "$LOCK_GUI" ] || extra="${extra}終了前にロックが無い(想定外) "
  t_quit=$(now_ms)
  kill -TERM "$APP_PID" 2>/dev/null
  if wait_for 15 gui_gone; then t_gone=$(now_ms); else t_gone=""; extra="${extra}GUIが15秒で終わらない "; kill -KILL "$APP_PID" 2>/dev/null; fi
  wait "$APP_PID" 2>/dev/null
  APP_PID=""
  wait_for $((HOLD_GUI + 15)) child_done || extra="${extra}観測器が終わらない "
  state=$([ -e "$LOCK_GUI" ] && echo 残る || echo 無い)
  trapm=$(grep -o '^trap-[a-z-]*\( rc=[0-9]*\)\{0,1\}' "$PR_GUI/rehash.log" | tr '\n' ',' | sed 's/,$//')
  sum=$(summarize "$PR_GUI/watch.log" "$t_quit")
  # GUI の終了が rehash のロックを持つ間だった証拠: locked → quit → 子の結末 の順
  local quit_log
  quit_log=$(grep -c 'on_app_quit: 終了処理開始' "$run/data/persist.log" 2>/dev/null | tr -d ' ')
  echo "  $kind $n: ロック=${state}・trap=[${trapm:-走らず}]・${sum}・GUI 終了 +$((${t_gone:-$t_quit} - t_quit))ms・on_app_quit の行=${quit_log:-0}"
  echo "         ペインの zsh: ${pane_line}"
  echo "         rc の印: $(tail -1 "$H_GUI/rc-read.log" 2>/dev/null)"
  if [ "$kind" = b ]; then
    local alive
    alive=$("$TMUX_BIN" -L "$sock" list-panes -a -F '#{pane_pid}' 2>/dev/null | tr '\n' ' ')
    echo "         器のペインの pid（GUI 終了の後）: ${alive:-なし}（zsh=${CUR_ZSH}）"
    [ -n "$alive" ] && extra="${extra}器のシェル生存 "
  fi
  survivors=$(iso_pane_alive "$panes" | tr '\n' ' ')
  # 控えた pid だけを撃つ（複数の pid を語に割る）
  # shellcheck disable=SC2086
  [ -n "$survivors" ] && { extra="${extra}居残り:${survivors}"; kill -TERM $survivors 2>/dev/null; }
  # 残ったときの害: 後から開くログイン zsh が待つ秒（PYENV_REHASH_TIMEOUT を 3 秒へ縮めて測る。待った rehash は
  # 本物と同じく諦めた trap でロックを消す）
  if [ "$state" = 残る ]; then
    local t0 waited
    t0=$(date +%s)
    env -i HOME="$H_GUI" PATH="$BASE_PATH" TERM=dumb PYENV_REHASH_TIMEOUT=3 FAKE_REHASH_SECS=0 FAKE_WATCH_LINGER_MS=0 \
      FAKE_WATCH_LOG="$run/next-watch.log" /bin/zsh -l -i -c exit >/dev/null 2>&1
    waited=$(($(date +%s) - t0))
    extra="${extra}次のログインzshの待ち=${waited}s "
  fi
  [ "$kind" = b ] && "$TMUX_BIN" -L "$sock" kill-server >/dev/null 2>&1
  printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\n' "$kind" "$n" "$state" "${trapm:-走らず}" "${sum%% 子*}" "子${sum#* 子}" "$extra" >> "$OUT"
  command cp -f "$PR_GUI/watch.log" "$run/watch.log" 2>/dev/null
  command cp -f "$PR_GUI/rehash.log" "$run/rehash.log" 2>/dev/null
  rm -f "${LOCK_GUI:?}"
}

if want a; then
  echo "=== (a) 直接ペイン × ${RUNS}: rc の初期化中（rehash がロックを持つ間）に GUI を終了（SIGTERM = Cmd+Q の口）==="
  for n in $(seq 1 "$RUNS"); do gui_case a "$n"; done
fi
if want b; then
  echo "=== (b) 器のペイン × ${RUNS}: 同じ手順（TAKO_PERSIST=1・明示ソケット）==="
  for n in $(seq 1 "$RUNS"); do gui_case b "$n"; done
fi
if want a32; then
  use_home "$TMP/home-gui32"
  echo "=== （補足）a32 直接ペイン × ${RUNS}: rehash の解釈系だけ macOS 同梱の bash 3.2（/bin/bash）に替えて (a) と同じ手順 ==="
  for n in $(seq 1 "$RUNS"); do gui_case a32 "$n"; done
  use_home "$TMP/home-gui"
fi

# ---- (c) ログインシェルの問い合わせの打ち切り -------------------------------------------------------
if want c; then
  H_C="$TMP/home-probe"
  mkdir -p "$H_C"
  make_home "$H_C" .zprofile || { echo "砂場を作れない"; exit 1; }
  PR_C="$H_C/.pyenv"
  LOCK_C="$PR_C/shims/.pyenv-shim"
  echo "=== (c) ログインシェルの問い合わせ × ${RUNS}: .zprofile の rehash がロックを持つ間に上限（既定 5 秒）で打ち切られる ==="
  for n in $(seq 1 "$RUNS"); do
    run="$TMP/c-$n"
    mkdir -p "$run/data" "$run/disc"
    rm -f "${LOCK_C:?}" "${PR_C:?}/rehash.log" "${PR_C:?}/watch.log" "${H_C:?}/rc-read.log"
    t0=$(now_ms)
    env -i HOME="$H_C" PATH="$BASE_PATH" SHELL=/bin/zsh TERM=dumb USER="$(id -un)" LOGNAME="$(id -un)" TAKO_LANG=ja \
      TAKO_ISOLATED=1 TAKO_DATA_DIR="$run/data" TAKO_DISCOVERY_DIR="$run/disc" \
      FAKE_REHASH_SECS="$HOLD_PROBE" FAKE_WATCH_LOG="$PR_C/watch.log" FAKE_WATCH_MODE=outside \
      "$TAKO_BIN" check-health --json > "$run/health.json" 2> "$run/health.err"
    rc=$?
    t1=$(now_ms)
    # 観測器はグループの外に居るので、子の結末を書いて抜けるまで待つ（最後の起動 + 余韻 3 秒）
    sleep 4
    state=$([ -e "$LOCK_C" ] && echo 残る || echo 無い)
    holder=$(sed -n 's/^locked pid=\([0-9]*\)/\1/p' "$PR_C/rehash.log" | head -1)
    probes=$(grep -c 'ev=start' "$PR_C/watch.log" 2>/dev/null | tr -d ' ')
    trapm=$(grep "pid=$holder\$" "$PR_C/rehash.log" | grep -o '^trap-[a-z-]*' | tr '\n' ',' | sed 's/,$//')
    holder_end=$(grep "child=$holder " "$PR_C/watch.log" | grep -o 'ev=child-[a-z]*.*' | head -1)
    holder_t=$(grep "child=$holder " "$PR_C/watch.log" | grep 'ev=child-' | sed -n 's/^t=\([0-9]*\).*/\1/p' | head -1)
    holder_s=$(grep "ev=start .*child=$holder " "$PR_C/watch.log" | sed -n 's/^t=\([0-9]*\).*/\1/p' | head -1)
    outside=$(grep -c "where=outside$" "$PR_C/watch.log" 2>/dev/null | tr -d " ")
    timeouts=$(grep -o "秒で応答なし" "$run/health.json" 2>/dev/null | wc -l | tr -d " ")
    sigs=$(grep -c 'ev=signal' "$PR_C/watch.log" 2>/dev/null | tr -d ' ')
    extra="問い合わせ=${probes}回（観測器がグループ外=${outside}）・ロックを持った rehash の起動から打ち切りまで=$(( ${holder_t:-0} - ${holder_s:-0} ))ms・check-health rc=${rc} $((t1 - t0))ms・打ち切りの語=${timeouts}"
    if [ "$state" = 残る ]; then
      t2=$(date +%s)
      env -i HOME="$H_C" PATH="$BASE_PATH" TERM=dumb PYENV_REHASH_TIMEOUT=3 FAKE_REHASH_SECS=0 FAKE_WATCH_LINGER_MS=0 \
        FAKE_WATCH_LOG="$run/next-watch.log" /bin/zsh -l -c exit >/dev/null 2>&1
      extra="${extra}・次のログインzshの待ち=$(($(date +%s) - t2))s"
    fi
    echo "  c $n: ロック=${state}・ロックを持った rehash(pid=${holder:-?}) の trap=[${trapm:-走らず}]・結末=${holder_end:-?} +$(( ${holder_t:-$t0} - t0 ))ms・観測器が受けたシグナル=${sigs}・${extra}"
    printf 'c\t%s\t%s\t%s\t%s\t%s\t%s\n' "$n" "$state" "${trapm:-走らず}" "観測器(グループ外)の受信=${sigs}" "${holder_end:-?} +$(( ${holder_t:-$t0} - t0 ))ms" "$extra" >> "$OUT"
    command cp -f "$PR_C/watch.log" "$run/watch.log" 2>/dev/null
    command cp -f "$PR_C/rehash.log" "$run/rehash.log" 2>/dev/null
    rm -f "${LOCK_C:?}"
  done
fi

echo
echo "測定後の本番: $(prod_state)"
echo "測定前の本番: $PROD_BEFORE"
echo "表: $OUT"
cat "$OUT"
