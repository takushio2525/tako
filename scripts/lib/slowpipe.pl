#!/usr/bin/perl
# slowpipe.pl — コマンドの stdout / stderr を「いつも満杯」の FIFO につなぎ、write を毎回カーネルの中で
# 待たせる検査用の道具（#2690 / #2736。負荷は生成しない）。
#
# CI の macOS の /bin/bash 3.2 は SIGCHLD を SA_RESTART なしで受けるので、待たずに走らせた子
# （プロセス置換・背景の &）が終わった瞬間にシェルが write の中で待っていると echo が EINTR で落ち、
# set -e でそこから先が走らない。CI では読み手と同じパイプを取り合った一瞬だけの間欠なので、ここで毎回にする。
#
# 読み手は 1 ms ごとに 32 バイトだけ空け、腕が書いたら・512 バイト空けても書かなければ詰め物の NUL で
# 満杯へ戻す（ただ遅く読むだけだと、腕が書かない間に空きが溜まって、続けて書く行の write が待たない）。
# 詰め物は**別に開いた**非塞の書き口から書く: O_NONBLOCK は開いた口ごとの属性で dup した口は共有する
# ので、腕と同じ口で切り替えると、その間の腕の write が EAGAIN で落ちる（#2690 の差し戻し。
# 名前の無いパイプは開き直せないので FIFO）。
#
# 使い方: perl scripts/lib/slowpipe.pl <FIFO を置く dir> <コマンド...>
#   詰め物を除いた出力をそのまま出し、コマンドの終了コードで終わる。FIFO は終わるときに消す。
#   TAKO_2690_SLOWPIPE_WIDE_US=<マイクロ秒> = 詰め物を書く前の待ち（窓を広げても腕が落ちないことを見る）
use strict;
use warnings;
use Fcntl qw(F_GETFL F_SETFL O_NONBLOCK O_RDONLY O_WRONLY);
use POSIX qw(:sys_wait_h mkfifo EAGAIN);
use Time::HiRes qw(usleep);
my $wide = $ENV{TAKO_2690_SLOWPIPE_WIDE_US} || 0;
my $dir = shift @ARGV;
my $FIONREAD = $^O eq 'darwin' ? 0x4004667f : 0x541B;
my $fifo = "$dir/slowpipe.$$.fifo";
mkfifo($fifo, 0600) or die "mkfifo: $!";
# 読み手・詰め物の書き口・腕の書き口は、それぞれ別に開く（詰め物の口だけがずっと非塞）
sysopen(my $r, $fifo, O_RDONLY | O_NONBLOCK) or die "open r: $!";
sysopen(my $fill_w, $fifo, O_WRONLY | O_NONBLOCK) or die "open fill: $!";
# 詰め物の NUL で満杯にする（PIPE_BUF 以下の write は丸ごとか 0 なので、小さい塊へ落としていく）
sub refill {
  usleep($wide) if $wide;
  for my $chunk (512, 64, 8, 1) {
    while (1) {
      my $n = syswrite($fill_w, "\0" x $chunk);
      last unless defined $n && $n > 0;
    }
  }
}
sub avail {
  my $b = pack('L', 0);
  ioctl($r, $FIONREAD, $b) or die "FIONREAD: $!";
  return unpack('L', $b);
}
refill();
my $pid = fork();
die "fork: $!" unless defined $pid;
if ($pid == 0) {
  close $r;
  close $fill_w;
  sysopen(my $w, $fifo, O_WRONLY) or die "open w: $!";
  open(STDOUT, '>&', $w) or die;
  open(STDERR, '>&', $w) or die;
  close $w;
  exec @ARGV or die "exec: $!";
}
binmode STDOUT;
$| = 1;
my $status = 0;
my $expect = avail();    # 前の回に読んだ後に残っているはずの量（増えていれば腕が書いた）
my $freed = 0;           # 満杯へ戻してから空けた量
while (1) {
  if (waitpid($pid, WNOHANG) == $pid) { $status = $?; last; }
  my $now = avail();
  if ($now > $expect || $freed >= 512) {
    refill();
    $freed = 0;
    $now = avail();
  }
  my $n = sysread($r, my $buf, 32);
  if (!defined $n) {
    die "read: $!" unless $! == EAGAIN;
    ($n, $buf) = (0, '');
  }
  $buf =~ tr/\0//d;
  print $buf;
  $freed += $n;
  $expect = $now - $n;
  usleep(1000);
}
close $fill_w;
fcntl($r, F_SETFL, fcntl($r, F_GETFL, 0) & ~O_NONBLOCK) or die "fcntl: $!";
while (1) {
  my $n = sysread($r, my $buf, 65536);
  last if !defined $n || $n == 0;
  $buf =~ tr/\0//d;
  print $buf;
}
unlink $fifo;
exit($status & 127 ? 128 + ($status & 127) : $status >> 8);
