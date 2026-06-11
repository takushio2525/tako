//! OSC 7 / 133 のストリームタップ（Layer 3 パッシブ検知の入口。FR-2.4.1）
//!
//! alacritty_terminal のパーサ（vte）は OSC 7（cwd 通知）/ OSC 133（プロンプトマーク）を
//! unhandled として捨てるため、PTY 読み取りバイト列を EventLoop へ渡る前にここで観測する。
//! `TapPty` は `EventedPty` の純粋な委譲ラッパで、`read` が返した範囲だけを
//! `OscScanner` に通す。**バイト列は一切変更しない**（本パースは従来どおり alacritty が行う）。
//!
//! 既知の制約: DCS パススルー（tmux の `ESC P tmux; ESC ESC ] ... ESC \`）内の OSC も
//! 拾う。tmux パススルーは「外側ターミナルへ届けたい」意図のシーケンスなので許容する。

use std::io;
use std::path::PathBuf;
use std::sync::Arc;

use alacritty_terminal::event::{OnResize, WindowSize};
use alacritty_terminal::tty::{ChildEvent, EventedPty, EventedReadWrite};
use polling::{Event as PollingEvent, PollMode, Poller};

/// OSC ペイロードの最大長。超えたシーケンスは丸ごと捨てる（暴走シーケンス対策）
const MAX_OSC_LEN: usize = 2048;

/// タップが検知したイベント。IO スレッドから UI 層へ中継される
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OscEvent {
    /// OSC 7: シェルの cwd 通知（`file://host/path` をデコード済み）。
    /// ホスト名は検証しない（ssh 先からの通知も届く）。ローカルパスとして使う側で
    /// 存在確認すること
    CwdChanged(PathBuf),
    /// OSC 133: プロンプト/コマンド境界マーク（FinalTerm 系シェル統合）
    Mark(PromptMark),
}

/// OSC 133 のマーク種別
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptMark {
    /// `133;A` — プロンプト描画開始
    PromptStart,
    /// `133;B` — プロンプト終端（ユーザー入力受付開始）
    CommandStart,
    /// `133;C` — コマンド実行開始（出力開始）
    CommandExecuted,
    /// `133;D[;exit]` — コマンド終了。exit code は省略されることがある
    CommandFinished(Option<i32>),
}

/// 分割読みに耐える最小 OSC スキャナ。
/// OSC = `ESC ]` payload (`BEL` | `ESC \`)。対象コード（7 / 133）以外は捨てる
#[derive(Debug, Default)]
pub struct OscScanner {
    state: ScanState,
    buf: Vec<u8>,
    /// MAX_OSC_LEN 超過中（終端まで読み捨てる）
    overflow: bool,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum ScanState {
    #[default]
    Ground,
    /// ESC 直後
    Esc,
    /// `ESC ]` の中（payload 蓄積中）
    Osc,
    /// payload 中に ESC が来た（次が `\` なら ST 終端）
    OscEsc,
}

impl OscScanner {
    pub fn new() -> Self {
        Self::default()
    }

    /// バイト列を走査し、完結した対象 OSC をイベントとして返す
    pub fn scan(&mut self, bytes: &[u8]) -> Vec<OscEvent> {
        let mut out = Vec::new();
        for &b in bytes {
            self.step(b, &mut out);
        }
        out
    }

    fn step(&mut self, b: u8, out: &mut Vec<OscEvent>) {
        match self.state {
            ScanState::Ground => {
                if b == 0x1b {
                    self.state = ScanState::Esc;
                }
            }
            ScanState::Esc => match b {
                b']' => {
                    self.buf.clear();
                    self.overflow = false;
                    self.state = ScanState::Osc;
                }
                0x1b => {} // ESC 連打は Esc のまま
                _ => self.state = ScanState::Ground,
            },
            ScanState::Osc => match b {
                0x07 => self.finish(out), // BEL 終端
                0x1b => self.state = ScanState::OscEsc,
                _ => {
                    if self.buf.len() < MAX_OSC_LEN {
                        self.buf.push(b);
                    } else {
                        self.overflow = true;
                    }
                }
            },
            ScanState::OscEsc => {
                if b == b'\\' {
                    self.finish(out); // ST（ESC \）終端
                } else {
                    // 終端以外の ESC はシーケンス中断。この ESC から先頭処理をやり直す
                    self.state = ScanState::Esc;
                    if b != 0x1b {
                        // ESC の次の文字として再解釈（`]` なら新しい OSC が始まる）
                        self.step(b, out);
                    }
                }
            }
        }
    }

    fn finish(&mut self, out: &mut Vec<OscEvent>) {
        if !self.overflow {
            if let Some(event) = parse_osc(&self.buf) {
                out.push(event);
            }
        }
        self.buf.clear();
        self.overflow = false;
        self.state = ScanState::Ground;
    }
}

/// OSC payload（`ESC ]` と終端の間）から対象イベントを取り出す
fn parse_osc(payload: &[u8]) -> Option<OscEvent> {
    if let Some(uri) = payload.strip_prefix(b"7;") {
        parse_cwd(uri)
    } else if let Some(mark) = payload.strip_prefix(b"133;") {
        parse_mark(mark)
    } else {
        None
    }
}

/// `file://host/path`（パスは percent エンコード）を PathBuf へ
fn parse_cwd(uri: &[u8]) -> Option<OscEvent> {
    let uri = std::str::from_utf8(uri).ok()?;
    let rest = uri.strip_prefix("file://")?;
    // ホスト部（空 / localhost / ホスト名）を最初の '/' まで読み飛ばす
    let path_start = rest.find('/')?;
    let path = percent_decode(&rest[path_start..])?;
    if path.is_empty() {
        return None;
    }
    Some(OscEvent::CwdChanged(PathBuf::from(path)))
}

/// `A` / `B` / `C` / `D[;exit[;...]]`。各マークの後ろに `;key=value` が続く方言も先頭だけ見る
fn parse_mark(mark: &[u8]) -> Option<OscEvent> {
    let (head, rest) = match mark.iter().position(|&b| b == b';') {
        Some(i) => (&mark[..i], &mark[i + 1..]),
        None => (mark, &[][..]),
    };
    let mark = match head {
        b"A" => PromptMark::PromptStart,
        b"B" => PromptMark::CommandStart,
        b"C" => PromptMark::CommandExecuted,
        b"D" => {
            // exit code の後ろにさらにパラメータが続く方言（`D;0;aid=...`）に備え先頭区切りまで
            let code = rest.split(|&b| b == b';').next().unwrap_or(&[]);
            let exit = std::str::from_utf8(code).ok().and_then(|s| s.parse().ok());
            PromptMark::CommandFinished(exit)
        }
        _ => return None,
    };
    Some(OscEvent::Mark(mark))
}

/// percent デコード（`%41` → `A`）。不正な % 列は None
fn percent_decode(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = bytes.get(i + 1..i + 3)?;
            let hex = std::str::from_utf8(hex).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// 検知イベントの送り先（IO スレッドから呼ばれる）
pub type OscSink = Box<dyn Fn(OscEvent) + Send>;

/// `EventedPty` の委譲ラッパ。読み取りバイト列を `OscScanner` で観測する
pub struct TapPty<P: EventedPty> {
    // writer()/register() と reader() の可変借用を両立させるため inner は Reader 側が持つ
    reader: TapReader<P>,
}

/// `io::Read` 実装。読んだ範囲をスキャナへ通してから返す
pub struct TapReader<P: EventedPty> {
    inner: P,
    scanner: OscScanner,
    sink: OscSink,
}

impl<P: EventedPty> TapPty<P> {
    pub fn new(inner: P, sink: OscSink) -> Self {
        Self {
            reader: TapReader {
                inner,
                scanner: OscScanner::new(),
                sink,
            },
        }
    }
}

impl<P: EventedPty> io::Read for TapReader<P> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.reader().read(buf)?;
        for event in self.scanner.scan(&buf[..n]) {
            (self.sink)(event);
        }
        Ok(n)
    }
}

impl<P: EventedPty> EventedReadWrite for TapPty<P> {
    type Reader = TapReader<P>;
    type Writer = P::Writer;

    unsafe fn register(
        &mut self,
        poller: &Arc<Poller>,
        event: PollingEvent,
        mode: PollMode,
    ) -> io::Result<()> {
        // Safety: 登録対象は inner の fd であり、生存期間の要求は inner にそのまま委譲される
        unsafe { self.reader.inner.register(poller, event, mode) }
    }

    fn reregister(
        &mut self,
        poller: &Arc<Poller>,
        event: PollingEvent,
        mode: PollMode,
    ) -> io::Result<()> {
        self.reader.inner.reregister(poller, event, mode)
    }

    fn deregister(&mut self, poller: &Arc<Poller>) -> io::Result<()> {
        self.reader.inner.deregister(poller)
    }

    fn reader(&mut self) -> &mut Self::Reader {
        &mut self.reader
    }

    fn writer(&mut self) -> &mut Self::Writer {
        self.reader.inner.writer()
    }
}

impl<P: EventedPty + OnResize> OnResize for TapPty<P> {
    fn on_resize(&mut self, window_size: WindowSize) {
        self.reader.inner.on_resize(window_size);
    }
}

impl<P: EventedPty> EventedPty for TapPty<P> {
    fn next_child_event(&mut self) -> Option<ChildEvent> {
        self.reader.inner.next_child_event()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scan_all(scanner: &mut OscScanner, chunks: &[&[u8]]) -> Vec<OscEvent> {
        chunks
            .iter()
            .flat_map(|chunk| scanner.scan(chunk))
            .collect()
    }

    #[test]
    fn osc7のcwd通知をbel終端で拾える() {
        let mut s = OscScanner::new();
        let events = s.scan(b"\x1b]7;file://mac.local/Users/foo\x07");
        assert_eq!(
            events,
            vec![OscEvent::CwdChanged(PathBuf::from("/Users/foo"))]
        );
    }

    #[test]
    fn osc7のst終端とpercentデコード() {
        let mut s = OscScanner::new();
        let events = s.scan(b"\x1b]7;file:///Users/foo/%E4%BD%9C%E6%A5%AD\x1b\\");
        assert_eq!(
            events,
            vec![OscEvent::CwdChanged(PathBuf::from("/Users/foo/作業"))]
        );
    }

    #[test]
    fn osc133の各マークとexitコード() {
        let mut s = OscScanner::new();
        let events = s.scan(b"\x1b]133;A\x07\x1b]133;B\x07\x1b]133;C\x07\x1b]133;D;1\x07");
        assert_eq!(
            events,
            vec![
                OscEvent::Mark(PromptMark::PromptStart),
                OscEvent::Mark(PromptMark::CommandStart),
                OscEvent::Mark(PromptMark::CommandExecuted),
                OscEvent::Mark(PromptMark::CommandFinished(Some(1))),
            ]
        );
    }

    #[test]
    fn exitコード省略と追加パラメータ方言() {
        let mut s = OscScanner::new();
        let events = s.scan(b"\x1b]133;D\x07\x1b]133;D;0;aid=3\x07\x1b]133;A;cl=m\x07");
        assert_eq!(
            events,
            vec![
                OscEvent::Mark(PromptMark::CommandFinished(None)),
                OscEvent::Mark(PromptMark::CommandFinished(Some(0))),
                OscEvent::Mark(PromptMark::PromptStart),
            ]
        );
    }

    #[test]
    fn 分割読みでもシーケンスを跨いで拾える() {
        let mut s = OscScanner::new();
        let events = scan_all(
            &mut s,
            &[
                b"ls\r\n\x1b]13",
                b"3;C",
                b"\x07echo\x1b]7;file:",
                b"///tmp\x07",
            ],
        );
        assert_eq!(
            events,
            vec![
                OscEvent::Mark(PromptMark::CommandExecuted),
                OscEvent::CwdChanged(PathBuf::from("/tmp")),
            ]
        );
    }

    #[test]
    fn 対象外のoscと通常出力は無視する() {
        let mut s = OscScanner::new();
        let events = s.scan(b"\x1b]0;title\x07plain text\x1b[31mred\x1b[0m\x1b]52;c;Zm9v\x07");
        assert!(events.is_empty());
    }

    #[test]
    fn 上限超過のoscは捨てて次は拾える() {
        let mut s = OscScanner::new();
        let mut long = b"\x1b]133;".to_vec();
        long.extend(std::iter::repeat_n(b'x', MAX_OSC_LEN + 10));
        long.extend(b"\x07");
        long.extend(b"\x1b]133;A\x07");
        let events = s.scan(&long);
        assert_eq!(events, vec![OscEvent::Mark(PromptMark::PromptStart)]);
    }

    #[test]
    fn 中断されたoscの直後のシーケンスを取りこぼさない() {
        // payload 中の ESC の次が ST でない → 中断し、その ESC からやり直す
        let mut s = OscScanner::new();
        let events = s.scan(b"\x1b]133;A\x1b]7;file:///tmp\x07");
        assert_eq!(events, vec![OscEvent::CwdChanged(PathBuf::from("/tmp"))]);
    }

    #[test]
    fn 不正なpercent列とホスト無しuriはnone() {
        assert_eq!(parse_cwd(b"file:///a%zz"), None);
        assert_eq!(parse_cwd(b"file://nohost-nopath"), None);
        assert_eq!(parse_cwd(b"http://example.com/"), None);
    }
}
