//! scroll_mirror — tmux バックエンドのスクロールバックをローカルへミラーして描画する（#159）
//!
//! バックエンドペインのスクロールバックは tmux サーバー側にあり、外側 alacritty には
//! 積もらない（tmux は差分再描画で流すため。2026-07-13 実測）。従来はホイールごとに
//! tmux copy-mode をサブプロセスで駆動していたが、① 行単位でしか動けない（ピクセル
//! スクロール不可能）② 1 操作 = tmux 往復 数十 ms（慣性スクロールに追従できない）
//! ③ copy-mode 滞在中のキー飲まれ、の 3 制約が原理的に残る。
//!
//! 本モジュールはスクロール開始時に `capture-pane -e` で履歴をチャンク取得し、
//! ANSI を alacritty の一時 Term でパースして [`ScreenLine`] 列としてミラーする。
//! 以降のスクロール描画は完全ローカル（直接ペインと同じサブライン描画経路）になり、
//! copy-mode には一切入らない。
//!
//! 座標系: `position` は「ライブ画面最下部からの遡り行数」（0.0 = 最下部、増えると
//! 過去方向、行小数）。表示合成は UI 層が `lines`（履歴。古い→新しい）とライブ画面を
//! 連結して行う。

use alacritty_terminal::event::VoidListener;
use alacritty_terminal::term::{test::TermSize, Config, Term};
use alacritty_terminal::vte::ansi::{Processor, StdSyncHandler};

use crate::screen::{snapshot, ScreenLine};
use crate::scroll::ScrollTarget;
use crate::theme::Theme;
use crate::tmux::run_tmux;

/// 1 回の capture で取得する履歴行数。トラックパッドの初動を待たせない程度に小さく、
/// 追加取得が頻発しない程度に大きく
pub const MIRROR_CHUNK: usize = 500;

/// tmux 履歴のローカルミラー（1 ペイン分のスクロール表示状態）
#[derive(Debug, Clone, Default)]
pub struct ScrollMirror {
    /// パース済み履歴行（古い→新しい）。tmux 履歴の**末尾** `lines.len()` 行に対応する
    /// （先頭側はまだロードしていない可能性がある）
    pub lines: Vec<ScreenLine>,
    /// tmux 履歴の総行数（取得時点の `#{history_size}`。増分検知の基準）
    pub total_history: usize,
    /// 表示位置（行小数。0.0 = 最下部 = ライブ画面ぴったり、最大 = total_history）
    pub position: f32,
}

impl ScrollMirror {
    /// ロード済み範囲で position をクランプした値（描画・スクロールバーの実効位置）。
    /// 未ロード領域へは追加チャンクのロード完了までスクロールを止める
    pub fn effective_position(&self) -> f32 {
        self.position.clamp(0.0, self.lines.len() as f32)
    }

    /// 相対スクロール（正 = 過去方向）。クランプ後の実効位置を返す
    pub fn scroll_by(&mut self, delta_rows: f32) -> f32 {
        self.position = (self.position + delta_rows).clamp(0.0, self.total_history as f32);
        self.effective_position()
    }

    /// さらに過去へのチャンク（先頭側 prepend）が必要か
    pub fn wants_more_history(&self, rows: usize) -> bool {
        self.lines.len() < self.total_history
            && self.position + rows as f32 > (self.lines.len().saturating_sub(rows)) as f32
    }
}

/// tmux 履歴の末尾から `skip_newest` 行飛ばして `want` 行を ANSI 付きで取得し、
/// パース済み [`ScreenLine`] 列（古い→新しい）と `#{history_size}` を返す。
/// `skip_newest = 0` が最新側チャンク。履歴が足りなければ取れた分だけ返す。
/// tmux 不在・セッション消滅では None
pub fn capture_history(
    target: &ScrollTarget,
    skip_newest: usize,
    want: usize,
    theme: &Theme,
) -> Option<(Vec<ScreenLine>, usize)> {
    let (socket, t) = target.locate();
    let start = format!("-{}", skip_newest + want);
    let end = format!("-{}", skip_newest + 1);
    let output = run_tmux(
        socket,
        &[
            "display-message",
            "-p",
            "-t",
            &t,
            "#{history_size} #{pane_width}",
            ";",
            "capture-pane",
            "-e",
            "-p",
            "-t",
            &t,
            "-S",
            &start,
            "-E",
            &end,
        ],
    )
    .ok()?;
    let mut it = output.lines();
    let meta = it.next()?;
    let mut m = meta.split_whitespace();
    let history: usize = m.next()?.parse().ok()?;
    let cols: usize = m.next()?.parse().ok()?;
    if history == 0 {
        return Some((Vec::new(), 0));
    }
    // capture 範囲が履歴先頭より上へはみ出た場合、tmux は取れる範囲だけ返す
    let raw: Vec<&str> = it.collect();
    let lines = parse_ansi_lines(&raw, cols, theme);
    Some((lines, history))
}

/// ペインの履歴・マウス要求状態（`history_state` の結果）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HistoryState {
    /// `#{history_size}`（ミラー更新の増分検知）
    pub history: usize,
    /// `#{mouse_any_flag}`（内側アプリがマウスレポートを要求しているか）
    pub mouse: bool,
    /// `#{mouse_sgr_flag}`（SGR 形式か。false なら X10 レガシー形式で送る）
    pub sgr: bool,
}

/// 現在の履歴行数とマウス要求状態を取得する（ミラー更新の増分検知 +
/// マウス要求アプリの判定 + レポート形式の判定）。セッション消滅では None
pub fn history_state(target: &ScrollTarget) -> Option<HistoryState> {
    let (socket, t) = target.locate();
    let output = run_tmux(
        socket,
        &[
            "display-message",
            "-p",
            "-t",
            &t,
            "#{history_size} #{mouse_any_flag} #{mouse_sgr_flag}",
        ],
    )
    .ok()?;
    let line = output.lines().next()?;
    let mut m = line.split_whitespace();
    let history: usize = m.next()?.parse().ok()?;
    let mouse = m.next()? == "1";
    let sgr = m.next() == Some("1");
    Some(HistoryState {
        history,
        mouse,
        sgr,
    })
}

/// マウス要求アプリへのホイールレポートを tmux サーバーへ**直接注入**する（#167）。
/// 外側クライアント PTY への書き込みは、書き込み停滞（UI / イベントループのストール、
/// PTY バッファ詰まり）でシーケンスが途中分割され escape-time（10ms）を跨ぐと、
/// tmux が ESC を単独キー確定して残骸（`4;45;18M` 等）が平文として内側 TUI の
/// 入力欄へ入る（実 claude で再現済み）。`send-keys -H` はソケット越しの
/// 構造化データのため、この経路の断片化が構造的に起きない。
/// `delta_lines` 正 = 上（過去）方向。呼び出し側でレート制限
/// （`TerminalSession::take_wheel_budget`）を通してから使うこと
pub fn send_wheel(target: &ScrollTarget, delta_lines: i32, col: usize, row: usize, sgr: bool) {
    if delta_lines == 0 {
        return;
    }
    let (socket, t) = target.locate();
    let event = crate::terminal::wheel_report_bytes(sgr, delta_lines > 0, col, row);
    let mut args: Vec<String> = vec!["send-keys".into(), "-t".into(), t, "-H".into()];
    for _ in 0..delta_lines.unsigned_abs() {
        for byte in &event {
            args.push(format!("{byte:02x}"));
        }
    }
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let _ = run_tmux(socket, &arg_refs);
}

/// ANSI（SGR 主体）付きの capture-pane 出力行を色解決済み [`ScreenLine`] へパースする。
/// 自前 SGR パーサではなく alacritty の一時 Term に流し込む: SGR 全種・全角スペーサー・
/// 行跨ぎの属性継続を実端末と同一の解釈で処理し、`screen::snapshot` と同じ行合成を通す
pub fn parse_ansi_lines(raw: &[&str], cols: usize, theme: &Theme) -> Vec<ScreenLine> {
    if raw.is_empty() {
        return Vec::new();
    }
    let cols = cols.max(2);
    let rows = raw.len();
    let mut term = Term::new(Config::default(), &TermSize::new(cols, rows), VoidListener);
    let mut parser: Processor<StdSyncHandler> = Processor::new();
    for (i, line) in raw.iter().enumerate() {
        parser.advance(&mut term, line.as_bytes());
        if i + 1 < rows {
            parser.advance(&mut term, b"\r\n");
        }
    }
    snapshot(&term, theme).lines
}

#[cfg(test)]
mod tests {
    use super::*;

    fn theme() -> Theme {
        Theme::default_dark()
    }

    #[test]
    fn sgr付き行をパースして色が解決される() {
        let raw = vec!["\x1b[31mRED\x1b[0m plain", "\x1b[1;4mbold-under\x1b[0m"];
        let lines = parse_ansi_lines(&raw, 20, &theme());
        assert_eq!(lines.len(), 2);
        assert!(lines[0].text.starts_with("RED plain"));
        let red = lines[0]
            .runs
            .iter()
            .find(|r| r.range.start == 0)
            .expect("先頭ラン");
        assert_eq!(red.fg, theme().ansi[1]);
        let bold = &lines[1].runs[0];
        assert!(bold.bold && bold.underline);
    }

    #[test]
    fn 属性が行を跨いで継続する() {
        // capture-pane -e は行末で属性を閉じないことがある（次行へ SGR 継続）
        let raw = vec!["\x1b[32mgreen continues", "still green\x1b[0m done"];
        let lines = parse_ansi_lines(&raw, 30, &theme());
        let g0 = lines[0].runs.iter().find(|r| r.range.start == 0).unwrap();
        let g1 = lines[1].runs.iter().find(|r| r.range.start == 0).unwrap();
        assert_eq!(g0.fg, theme().ansi[2]);
        assert_eq!(g1.fg, theme().ansi[2], "SGR が行を跨いで継続する");
    }

    #[test]
    fn 全角文字のセル列が正しい() {
        let raw = vec!["あiう"];
        let lines = parse_ansi_lines(&raw, 10, &theme());
        assert!(lines[0].text.starts_with("あiう"));
        assert!(lines[0].has_wide);
        // あ=col0(幅2), i=col2, う=col3(幅2)
        assert_eq!(&lines[0].cell_cols[..3], &[0, 2, 3]);
    }

    #[test]
    fn 空入力は空を返す() {
        assert!(parse_ansi_lines(&[], 80, &theme()).is_empty());
    }

    #[test]
    fn ミラーのスクロールとクランプ() {
        let mut m = ScrollMirror {
            lines: vec![ScreenLine {
                text: String::new(),
                runs: Vec::new(),
                cell_cols: Vec::new(),
                has_wide: false,
            }],
            total_history: 100,
            position: 0.0,
        };
        assert_eq!(m.scroll_by(2.5), 1.0, "ロード済み 1 行までにクランプ");
        assert_eq!(m.position, 2.5, "論理位置は total_history までを保持");
        assert_eq!(m.scroll_by(200.0), 1.0);
        assert_eq!(m.position, 100.0, "total_history でクランプ");
        assert_eq!(m.scroll_by(-150.0), 0.0);
        assert_eq!(m.position, 0.0);
    }

    /// tmux 実サーバーでの capture e2e（tmux が無い環境はスキップ）
    #[test]
    #[cfg(unix)]
    fn tmux履歴をミラーとして取得できる() {
        use crate::terminal::{SpawnCommand, SpawnOptions};
        use crate::tmux_backend::{available, kill_server, wrap_options};
        if !available() {
            eprintln!("skip: tmux が無い環境");
            return;
        }
        let socket = format!("tako-coretest-mir-{}", std::process::id());
        struct Cleanup(String);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                kill_server(&self.0);
            }
        }
        let _cleanup = Cleanup(socket.clone());
        let options = SpawnOptions {
            command: Some(SpawnCommand {
                program: "/bin/sh".into(),
                args: vec![
                    "-c".into(),
                    "i=0; while [ $i -lt 100 ]; do printf '\\033[31mL-%d\\033[0m ok\\n' $i; i=$((i+1)); done; exec sleep 60"
                        .into(),
                ],
            }),
            cwd: Some(std::env::temp_dir()),
            env: vec![],
        };
        let (session, _rx) =
            crate::TerminalSession::spawn(80, 24, wrap_options(options, &socket, "tako-e2e-mir"))
                .expect("spawn できる");
        // 出力が揃うまで待つ
        let ready = (0..100).any(|_| {
            std::thread::sleep(std::time::Duration::from_millis(100));
            session
                .visible_lines()
                .iter()
                .any(|l| l.trim_end().ends_with("L-99 ok"))
        });
        assert!(ready, "出力が揃わない");
        let target = ScrollTarget::Backend {
            socket: socket.clone(),
            session: "tako-e2e-mir".into(),
        };
        let state = history_state(&target).expect("履歴状態が取れる");
        assert!(state.history > 0, "履歴が積もっている");
        assert!(!state.mouse, "シェル画面はマウス要求なし");
        // 最新側 10 行を取得: 履歴末尾は L-99 の 1 行上まで（L-99 はライブ画面内）
        let (lines, history) = capture_history(&target, 0, 10, &theme()).expect("capture できる");
        assert_eq!(history, state.history);
        assert_eq!(lines.len(), 10);
        // 色が解決されている（赤ラン）
        let has_red = lines
            .iter()
            .any(|l| l.runs.iter().any(|r| r.fg == theme().ansi[1]));
        assert!(has_red, "SGR 赤が解決されている");
        // さらに古いチャンクと連続している（末尾行番号 = 最新側チャンク先頭の 1 つ前）
        let (older, _) = capture_history(&target, 10, 10, &theme()).expect("capture できる");
        assert_eq!(older.len(), 10);
        let num = |l: &ScreenLine| -> Option<i32> {
            l.text
                .trim_start_matches("L-")
                .split_whitespace()
                .next()?
                .parse()
                .ok()
        };
        let newest_first = num(&lines[0]);
        let older_last = num(&older[9]);
        if let (Some(a), Some(b)) = (newest_first, older_last) {
            assert_eq!(b + 1, a, "チャンクが連続している（{b} の次が {a}）");
        } else {
            panic!(
                "行番号が読めない: newest_first={:?} older_last={:?}",
                lines[0].text, older[9].text
            );
        }
    }

    /// `send_wheel`（tmux 直接注入。#167）でホイールレポートが内側アプリへ
    /// **生のまま**届く e2e。外側クライアント PTY を経由しない（= tty_keys パース・
    /// 部分 write・escape-time と無縁）ことがこの経路の存在意義
    #[test]
    #[cfg(unix)]
    fn ホイールレポートのtmux直接注入が内側に生で届く() {
        use crate::terminal::{SpawnCommand, SpawnOptions};
        use crate::tmux_backend::{available, kill_server, wrap_options};
        if !available() {
            eprintln!("skip: tmux が無い環境");
            return;
        }
        let socket = format!("tako-coretest-sw-{}", std::process::id());
        struct Cleanup(String);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                kill_server(&self.0);
            }
        }
        let _cleanup = Cleanup(socket.clone());
        // 内側アプリ: raw mode + 受信バイトの ESC を「^[」に可視化して即時 echo
        let inner = r#"stty raw -echo; printf '\033[?1000h\033[?1006h'; exec perl -e '$|=1; while (sysread(STDIN,$b,4096)) { $b =~ s/\x1b/^[/g; syswrite(STDOUT,$b) }'"#;
        let options = SpawnOptions {
            command: Some(SpawnCommand {
                program: "/bin/sh".into(),
                args: vec!["-c".into(), inner.into()],
            }),
            cwd: Some(std::env::temp_dir()),
            env: vec![],
        };
        let (session, _rx) =
            crate::TerminalSession::spawn(80, 24, wrap_options(options, &socket, "tako-e2e-sw"))
                .expect("spawn できる");
        let mut mouse_on = false;
        for _ in 0..100 {
            if session.mouse_reporting() {
                mouse_on = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        assert!(mouse_on, "内側のマウス要求が伝わる");
        let target = ScrollTarget::Backend {
            socket: socket.clone(),
            session: "tako-e2e-sw".into(),
        };
        // SGR 上方向 3 イベント + 下方向 1 イベント + X10 上方向 1 イベント
        send_wheel(&target, 3, 5, 5, true);
        send_wheel(&target, -1, 5, 5, true);
        send_wheel(&target, 1, 5, 5, false);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let expect_sgr_up = "^[[<64;6;6M^[[<64;6;6M^[[<64;6;6M";
        let expect_sgr_down = "^[[<65;6;6M";
        let expect_x10 = "^[[M`&&"; // 32+64=0x60(`)、32+6=0x26(&)
        loop {
            let screen = session.visible_lines().join("\n");
            if screen.contains(expect_sgr_up)
                && screen.contains(expect_sgr_down)
                && screen.contains(expect_x10)
            {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "直接注入のレポートが内側へ届かない。画面: {screen:?}"
            );
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    }
}
