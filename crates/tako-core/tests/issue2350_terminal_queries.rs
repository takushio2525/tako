//! 端末の問い合わせへの応答の実 PTY テスト（Issue #2350）
//!
//! ペインの中のプログラムが端末へ尋ねる色（OSC 10 / 11 / 12 / 4）・明暗（`CSI ? 996 n`）・
//! 寸法（CSI 14 t / 18 t）に、直接のペインと器（tmux）越しのペインの両方で答えが返ることを、
//! 実 PTY で確かめる。UI 層の役（`SessionNotice::ThemeQuery` を受けたその場でテーマを添えて
//! 答える・テーマを切り替えたら `notify_color_scheme_change`）はテストが tako-app と同じ形で担う。
//!
//! 読み方（固定の待ちは置かない）: 子の perl が raw にした端末へ「問い合わせ + DA1（CSI c）」を
//! 書き、DA1 の答えが来た時点で読み終える（端末は DA1 に必ず答えるので、答えない問い合わせでも
//! DA1 で締まる）。DA1 より前に届いたバイト列が問い合わせへの答え。結果はファイルへ書かせる。
//!
//! A/B（`TAKO_2350_LEGACY=1`）は同一プロセスの OnceLock なので、このテストではなく
//! 実経路のスクリプト `scripts/test-terminal-queries-2350.sh` が受け持つ。
#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use futures::channel::mpsc::UnboundedReceiver;
use tako_core::terminal_query::ThemeQuery;
use tako_core::theme::{Rgb, Theme};
use tako_core::{SessionEvent, SessionNotice, SpawnCommand, SpawnOptions, TerminalSession};

/// 1 回のプローブ（問い合わせ 12 種 × DA1）の上限。実測は直接のペインで 1 種 5 ms 前後
const PROBE_DEADLINE: Duration = Duration::from_secs(60);

const PROBE: &str = r#"
use strict;
use warnings;
use Time::HiRes qw(time);
use IO::Select;
my $out = shift @ARGV or die "usage: probe.pl <out>";
my @all = (
    [ 'osc11',    "\e]11;?\a" ],
    [ 'osc10',    "\e]10;?\a" ],
    [ 'csi14t',   "\e[14t" ],
    [ 'csi18t',   "\e[18t" ],
    [ 'osc12',    "\e]12;?\a" ],
    [ 'osc4_1',   "\e]4;1;?\a" ],
    [ 'osc4_200', "\e]4;200;?\a" ],
    [ 'osc11st',  "\e]11;?\e\\" ],
    [ 'dsr996',   "\e[?996n" ],
    [ 'osc52',    "\e]52;c;?\a" ],
    [ 'osc4_bad', "\e]4;999;?\a" ],
    [ 'osc11_bad', "\e]11;nope\a" ],
);
open( my $tty, '+<', '/dev/tty' ) or die "no tty: $!";
binmode $tty;
my $ws = "\0" x 8;
my $req = $^O eq 'darwin' ? 0x40087468 : 0x5413;
ioctl( $tty, $req, $ws ) or die "TIOCGWINSZ: $!";
my ( $rows, $cols, $xpix, $ypix ) = unpack( 'S4', $ws );
chomp( my $saved = `stty -g < /dev/tty` );
system('stty raw -echo < /dev/tty');
my $sel = IO::Select->new($tty);
my @lines = ("W:$rows $cols $xpix $ypix");
for my $q (@all) {
    my ( $name, $seq ) = @$q;
    syswrite( $tty, $seq . "\e[c" );
    my ( $buf, $da1, $start ) = ( '', 0, time );
    while ( time - $start < 3 ) {
        last unless $sel->can_read( 3 - ( time - $start ) );
        my $n = sysread( $tty, my $chunk, 4096 );
        last unless $n;
        $buf .= $chunk;
        if ( $buf =~ s/\e\[\?[0-9;]*c// ) { $da1 = 1; last; }
    }
    my $ms = int( ( time - $start ) * 1000 );
    ( my $vis = $buf ) =~ s/\e/^[/g;
    $vis =~ s/\a/^G/g;
    $vis =~ s/([\x00-\x1f])/sprintf('^%c', ord($1) + 64)/ge;
    push @lines, "R:$name=[$vis] ${ms}ms da1=$da1";
}
system("stty $saved < /dev/tty");
open( my $fh, '>', "$out.part" ) or die;
print $fh "$_\n" for @lines, 'R:done';
close $fh;
rename "$out.part", $out;
"#;

/// テストの一時置き場（終わったら消す）
struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("tako-2350-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("一時ディレクトリを作れる");
        std::fs::write(dir.join("probe.pl"), PROBE).expect("プローブを書ける");
        Self(dir)
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// 隔離ソケットの tmux サーバー（終わったら kill-server する）
struct TmuxServer(String);

impl Drop for TmuxServer {
    fn drop(&mut self) {
        let _ = std::process::Command::new(tako_core::tmux::tmux_bin())
            .args(["-L", &self.0, "kill-server"])
            .output();
    }
}

/// 子のスクリプト: `start` を待ってプローブ 1 回目 → `go` を待って 2 回目 → `go3` を待って 3 回目
fn child_script(dir: &TempDir) -> String {
    let p = |n: &str| dir.path(n).display().to_string();
    format!(
        "while [ ! -f '{start}' ]; do sleep 0.05; done; perl '{probe}' '{out1}'; \
         while [ ! -f '{go}' ]; do sleep 0.05; done; perl '{probe}' '{out2}'; \
         while [ ! -f '{go3}' ]; do sleep 0.05; done; perl '{probe}' '{out3}'; sleep 60",
        start = p("start"),
        probe = p("probe.pl"),
        out1 = p("out1"),
        go = p("go"),
        out2 = p("out2"),
        go3 = p("go3"),
        out3 = p("out3"),
    )
}

/// ペインの寸法を変える（境界: 変わった直後の問い合わせ）。80 列 × 20 行・セル 10 × 20 px
const RESIZED: (usize, usize, u16, u16) = (80, 20, 10, 20);

/// UI 層の役をするペイン（tako-app の `on_term_event` と同じ手順でイベントを処理する）
struct Pane {
    session: TerminalSession,
    rx: UnboundedReceiver<SessionEvent>,
    theme: Theme,
    /// 背景色の問い合わせ（OSC 11）へ答えた回数。器（tmux）の問い合わせ直しを数える
    background_answers: usize,
}

impl Pane {
    fn spawn(options: SpawnOptions) -> Self {
        let (mut session, rx) = TerminalSession::spawn(100, 30, options).expect("spawn できる");
        // セルの寸法を決めておく（CSI 14 t の答え = 行数 × 18・列数 × 9）
        session.resize(100, 30, 9, 18);
        Self {
            session,
            rx,
            theme: Theme::default_dark(),
            background_answers: 0,
        }
    }

    /// 届いているイベントを全部処理する。テーマが要る問い合わせには**その場で**答える
    fn pump(&mut self) {
        while let Ok(event) = self.rx.try_recv() {
            match self.session.process_event(event) {
                Some(SessionNotice::ThemeQuery(query)) => {
                    if matches!(&query, ThemeQuery::Color(c) if c.index() == 257) {
                        self.background_answers += 1;
                    }
                    self.session.answer_theme_query(query, &self.theme);
                }
                Some(SessionNotice::Exited) => {
                    panic!("子が終わった: {}", self.session.visible_lines().join("\n"))
                }
                _ => {}
            }
        }
    }

    /// tako-app の `apply_theme` と同じ手順でテーマを差し替える
    fn set_theme(&mut self, theme: Theme) {
        let changed = tako_core::terminal_query::answers_differ(&self.theme, &theme);
        self.theme = theme;
        if changed {
            self.session.notify_color_scheme_change(&self.theme);
        }
    }

    /// 条件が立つまでイベントを流しながら待つ（上限つき）
    fn wait_until(&mut self, what: &str, mut done: impl FnMut(&mut Self) -> bool) {
        let start = Instant::now();
        while start.elapsed() < PROBE_DEADLINE {
            self.pump();
            if done(self) {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!(
            "{what} が {PROBE_DEADLINE:?} 以内に立たない。画面:\n{}",
            self.session.visible_lines().join("\n")
        );
    }

    /// プローブの結果を待って読む
    fn probe_result(&mut self, out: &Path) -> Probe {
        self.wait_until("プローブの結果", |_| {
            std::fs::read_to_string(out).is_ok_and(|s| s.contains("R:done"))
        });
        Probe(std::fs::read_to_string(out).expect("結果を読める"))
    }
}

/// プローブの結果（`R:<名前>=[<答え>] <ms>ms da1=<0|1>` の行と `W:rows cols xpix ypix`）
struct Probe(String);

impl Probe {
    fn reply(&self, name: &str) -> String {
        let head = format!("R:{name}=[");
        let line = self
            .0
            .lines()
            .find(|l| l.starts_with(&head))
            .unwrap_or_else(|| panic!("{name} の行が無い: {}", self.0));
        assert!(line.ends_with("da1=1"), "DA1 で締まっていない: {line}");
        let body = &line[head.len()..];
        body[..body.rfind("] ").expect("答えの終わり")].to_string()
    }

    fn winsize(&self) -> (u32, u32, u32, u32) {
        let w = self
            .0
            .lines()
            .find_map(|l| l.strip_prefix("W:"))
            .expect("W の行");
        let v: Vec<u32> = w.split(' ').map(|n| n.parse().expect("数")).collect();
        (v[0], v[1], v[2], v[3])
    }
}

/// OSC の答えの形（`^[` / `^G` で見える化した形）
fn osc(code: &str, c: Rgb, st: bool) -> String {
    format!(
        "^[]{code};rgb:{0:02x}{0:02x}/{1:02x}{1:02x}/{2:02x}{2:02x}{3}",
        c.r,
        c.g,
        c.b,
        if st { "^[\\" } else { "^G" }
    )
}

fn sh(script: String, cwd: &Path) -> SpawnOptions {
    SpawnOptions {
        command: Some(SpawnCommand {
            program: "/bin/sh".into(),
            args: vec!["-c".into(), script],
        }),
        cwd: Some(cwd.to_path_buf()),
        env: vec![],
        scrollback_lines: None,
    }
}

fn touch(path: PathBuf) {
    std::fs::write(path, b"").expect("合図を書ける");
}

/// 寸法の答えが子へ見せている TIOCGWINSZ と一致する（`want` = 行・列・幅 px・高さ px の前提）
fn assert_sizes(r: &Probe, what: &str, want: (u32, u32, u32, u32)) {
    let (rows, cols, xpix, ypix) = r.winsize();
    assert_eq!((rows, cols, xpix, ypix), want, "{what}: 前提の寸法");
    assert_eq!(r.reply("csi14t"), format!("^[[4;{ypix};{xpix}t"), "{what}");
    assert_eq!(r.reply("csi18t"), format!("^[[8;{rows};{cols}t"), "{what}");
}

/// 最初の寸法（[`Pane::spawn`] の 100 列 × 30 行・セル 9 × 18 px）
const FIRST: (u32, u32, u32, u32) = (30, 100, 900, 540);

/// [`RESIZED`] の寸法（行・列・幅 px・高さ px）
fn resized() -> (u32, u32, u32, u32) {
    let (cols, rows, cw, ch) = RESIZED;
    (
        rows as u32,
        cols as u32,
        cols as u32 * u32::from(cw),
        rows as u32 * u32::from(ch),
    )
}

#[test]
fn 直接のペインは色と明暗と寸法に答えosc52には答えない() {
    let dir = TempDir::new("direct");
    let mut pane = Pane::spawn(sh(child_script(&dir), &dir.0));
    let dark = Theme::default_dark();
    touch(dir.path("start"));
    let r = pane.probe_result(&dir.path("out1"));

    assert_eq!(r.reply("osc11"), osc("11", dark.background, false));
    assert_eq!(
        r.reply("osc11st"),
        osc("11", dark.background, true),
        "ST には ST で"
    );
    assert_eq!(r.reply("osc10"), osc("10", dark.foreground, false));
    assert_eq!(r.reply("osc12"), osc("12", dark.cursor, false));
    assert_eq!(r.reply("osc4_1"), osc("4;1", dark.ansi[1], false));
    assert_eq!(
        r.reply("osc4_200"),
        osc("4;200", Rgb::new(255, 0, 215), false)
    );
    assert_eq!(r.reply("dsr996"), "^[[?997;1n", "ダークのテーマ");
    assert_sizes(&r, "直接", FIRST);
    assert_eq!(r.reply("osc52"), "", "OSC 52 の読み出しには答えない");
    assert_eq!(r.reply("osc4_bad"), "", "範囲外の添字には答えない");
    assert_eq!(r.reply("osc11_bad"), "", "不正な問い合わせには答えない");

    // テーマを切り替えた後の問い合わせには切り替えた後の色が返る
    let light = Theme::default_light();
    pane.set_theme(light.clone());
    touch(dir.path("go"));
    let r = pane.probe_result(&dir.path("out2"));
    assert_eq!(r.reply("osc11"), osc("11", light.background, false));
    assert_eq!(r.reply("osc10"), osc("10", light.foreground, false));
    assert_eq!(r.reply("dsr996"), "^[[?997;2n", "ライトのテーマ");
    assert!(
        !pane.session.color_scheme_updates(),
        "購読していない子（perl）へは知らせない"
    );

    // 境界: ペインの寸法を変えた直後の問い合わせには変えた後の寸法で答える
    let (cols, rows, cw, ch) = RESIZED;
    pane.session.resize(cols, rows, cw, ch);
    touch(dir.path("go3"));
    let r = pane.probe_result(&dir.path("out3"));
    assert_sizes(&r, "直接（寸法を変えた直後）", resized());
}

/// tmux がライト / ダークの通知（mode 2031）を扱う版か（3.6 以上。tako が確かめたのは 3.6b の
/// `tty_start_tty` の購読と `server_client_report_theme` の問い合わせ直し）。読めなければ偽
fn tmux_handles_theme_reports() -> bool {
    let Ok(out) = std::process::Command::new(tako_core::tmux::tmux_bin())
        .arg("-V")
        .output()
    else {
        return false;
    };
    // `tmux 3.6b` / `tmux next-3.7` の数字の部分（major.minor）
    let text = String::from_utf8_lossy(&out.stdout);
    let digits: String = text
        .chars()
        .skip_while(|c| !c.is_ascii_digit())
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    let mut parts = digits.split('.').map(|p| p.parse::<u32>().unwrap_or(0));
    let (major, minor) = (parts.next().unwrap_or(0), parts.next().unwrap_or(0));
    (major, minor) >= (3, 6)
}

#[test]
fn 器越しのペインはtmuxが学んだ色で答えテーマの切り替えに追従する() {
    if !tako_core::tmux_test_gate::container() {
        return;
    }
    if !tmux_handles_theme_reports() {
        // CI（TAKO_TEST_REQUIRE_TMUX）では skip が panic になる = 古い tmux を黙って通さない
        tako_core::tmux_test_gate::skip(
            "tmux 3.6 より前はライト / ダークの通知（mode 2031）を購読しない",
        );
        return;
    }
    let dir = TempDir::new("tmux");
    let server = TmuxServer(format!("tako2350-{}", std::process::id()));
    let options =
        tako_core::tmux_backend::wrap_options(sh(child_script(&dir), &dir.0), &server.0, "q2350");
    let mut pane = Pane::spawn(options);
    // tmux は attach 時に外側の端末（このペイン）へライト / ダークの通知を購読する
    pane.wait_until("tmux の mode 2031 の購読", |p| {
        p.session.color_scheme_updates()
    });
    let dark = Theme::default_dark();
    touch(dir.path("start"));
    let r = pane.probe_result(&dir.path("out1"));
    // 色は tmux が答える（tako が tmux の問い合わせに答えた値。OSC 4 は tmux が tako へ転送する）
    assert_eq!(r.reply("osc11"), osc("11", dark.background, false));
    assert_eq!(r.reply("osc11st"), osc("11", dark.background, true));
    assert_eq!(r.reply("osc10"), osc("10", dark.foreground, false));
    assert_eq!(r.reply("osc4_1"), osc("4;1", dark.ansi[1], false));
    assert_eq!(r.reply("dsr996"), "^[[?997;1n");
    assert_sizes(&r, "器越し", FIRST);

    // テーマを切り替える → 購読している tmux へ知らせる → tmux が背景色を問い合わせ直す
    let light = Theme::default_light();
    let before = pane.background_answers;
    pane.set_theme(light.clone());
    pane.wait_until("tmux の背景色の問い合わせ直し", |p| {
        p.background_answers > before
    });
    touch(dir.path("go"));
    let r = pane.probe_result(&dir.path("out2"));
    assert_eq!(
        r.reply("osc11"),
        osc("11", light.background, false),
        "器越しの答えが新しいテーマへ追従する"
    );
    assert_eq!(r.reply("osc10"), osc("10", light.foreground, false));
    assert_eq!(r.reply("dsr996"), "^[[?997;2n");

    // 境界: ペインの寸法を変えた直後。tmux が中のペインを変えたことを tmux 自身に尋ねて待つ
    let (cols, rows, cw, ch) = RESIZED;
    pane.session.resize(cols, rows, cw, ch);
    let want = format!("{cols}x{rows}");
    pane.wait_until("tmux の中のペインの寸法の追従", |_| {
        std::process::Command::new(tako_core::tmux::tmux_bin())
            .args([
                "-L",
                &server.0,
                "display",
                "-p",
                "-t",
                "q2350",
                "#{pane_width}x#{pane_height}",
            ])
            .output()
            .is_ok_and(|o| String::from_utf8_lossy(&o.stdout).trim() == want)
    });
    touch(dir.path("go3"));
    let r = pane.probe_result(&dir.path("out3"));
    assert_sizes(&r, "器越し（寸法を変えた直後）", resized());
}
