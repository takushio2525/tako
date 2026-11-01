//! ペインログのファイル I/O を専用の書き手スレッドで行う（#2618）。
//!
//! [`crate::pane_log::PaneLogManager`] は GUI の 1 つの Mutex 越しに、メインスレッド（直接ペインの
//! 取り込み・ペインを閉じたときのフラッシュ・`tako logs` の状態読み）と背景（器のペインの取り込み・
//! 器を kill する直前のフラッシュ・全体上限の強制）の両方から使われる。以前は追記・ローテーション・
//! 全体上限の削除を **Mutex を握ったまま呼び手のスレッドで** 行っていたので、ディスクが詰まると:
//!
//! - メインスレッドの取り込み・閉じたときのフラッシュがその I/O の長さだけ GUI を止める
//!   （追記 5 秒の注入で `periodic_prep:pane_log が 5002ms`・閉じる IPC が 15,031ms）
//! - 背景が I/O の間 Mutex を握るので、メインスレッドで Mutex を取る口（`tako logs status` 等）が
//!   その長さだけ待つ（同じ注入で `dispatch:Logs が 9939ms`）
//!
//! ここでは:
//!
//! - マネージャーは Mutex の内側で**メモリの操作だけ**を行い、ファイルへの操作を [`WriteOp`] にして
//!   [`PaneLogWriter::submit`] で列へ積む（I/O を待たない）
//! - 書き手のスレッドは 1 本だけで、積まれた順に実行する。積むのは常にマネージャーの Mutex の内側なので
//!   **列の順 = 状態を進めた順**になり、ファイルの中身の並びは同期で書いていたときと同じ
//!   （ローテーションの退避も同じ列に並ぶので、退避の前後の行が入れ替わらない）
//! - 終了の前は [`PaneLogWriter::flush`] で書き終わりを待つ（閉じたときの最後の行とマーカーを残す）
//! - 1 件の中の panic で書き手を止めない（失敗は従来どおり握りつぶす = ログ機能の失敗で本体を巻き込まない）
//!
//! A/B（`TAKO_2618_LEGACY=1`）と単体テストの既定は [`PaneLogWriter::inline`]（積んだ場で同期で実行 =
//! 修正前と同じ）。

use std::collections::VecDeque;
use std::io::Write;
use std::panic::AssertUnwindSafe;
use std::path::PathBuf;
use std::sync::{mpsc, Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// 1 件の所要がこれを超えたら `on_slow` へ 1 行渡す（メインスレッドは待っていないので「専有」ではない。
/// ディスクの詰まりを後から読めるようにするための記録。#2610 の layout.json の書き手と同じ長さ）
const SLOW_OP_LOG_OVER: Duration = Duration::from_millis(1000);

/// 書き手へ渡すファイルへの操作 1 件
#[derive(Debug)]
pub(crate) enum WriteOp {
    /// 現ファイルを `.1` へ退避する（ローテーション）
    Rotate(PathBuf),
    /// 行を追記する。ファイルが無ければヘッダを付けて作る（有無は書く時点で見る = 同期のときと同じ）
    Append { path: PathBuf, body: String },
    /// ログディレクトリ全体の上限を強制し、消したファイル数を返す
    EnforceCap {
        dir: PathBuf,
        max_total_bytes: u64,
        /// 積んだ時点で開いていたファイル（消さない）
        exclude: Vec<PathBuf>,
        reply: mpsc::Sender<usize>,
    },
}

impl WriteOp {
    /// 列に残っている量の目安（追記の本文のバイト数）
    fn backlog_bytes(&self) -> u64 {
        match self {
            WriteOp::Append { body, .. } => body.len() as u64,
            WriteOp::Rotate(_) | WriteOp::EnforceCap { .. } => 0,
        }
    }

    /// perf.log の記録に出す種別（パス・中身は出さない = 診断ログにペインの内容を出さない）
    fn kind(&self) -> &'static str {
        match self {
            WriteOp::Rotate(_) => "ローテーション",
            WriteOp::Append { .. } => "追記",
            WriteOp::EnforceCap { .. } => "全体上限の強制",
        }
    }

    /// 本物の I/O。失敗は握りつぶす（ログ機能の失敗で本体を巻き込まない = 修正前と同じ）
    pub(crate) fn run(&self) {
        match self {
            WriteOp::Rotate(path) => crate::pane_log::rotate(path),
            WriteOp::Append { path, body } => append_file(path, body),
            WriteOp::EnforceCap {
                dir,
                max_total_bytes,
                exclude,
                reply,
            } => {
                let removed = crate::pane_log::enforce_total_cap_at(dir, *max_total_bytes, exclude);
                let _ = reply.send(removed);
            }
        }
    }
}

/// `path` へ `body` を追記する（ファイルが無ければヘッダを付けて作る）
fn append_file(path: &std::path::Path, body: &str) {
    let Some(dir) = path.parent() else {
        return;
    };
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    let existed = path.is_file();
    let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    else {
        return;
    };
    let mut buf = String::with_capacity(body.len() + 64);
    if !existed {
        buf.push_str(&crate::pane_log::file_header());
    }
    buf.push_str(body);
    let _ = f.write_all(buf.as_bytes());
}

/// 書き手の内訳
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WriterStats {
    /// 実行した操作の数
    pub done: u64,
    /// 操作 1 件の最長（ms）
    pub max_op_ms: u64,
    /// 列に残っている追記の本文のバイト数
    pub backlog_bytes: u64,
    /// 列に残っている操作か実行中の操作があるか
    pub pending: bool,
}

type Sink = Box<dyn FnMut(&WriteOp) + Send>;
type OnSlow = Box<dyn Fn(&str) + Send + Sync>;

struct State {
    queue: VecDeque<WriteOp>,
    /// 書き手が 1 件を実行している最中か
    busy: bool,
    stats: WriterStats,
    /// 持ち主がすべて手放した（スレッドは列を書き終えてから抜ける）
    shutdown: bool,
}

struct Shared {
    state: Mutex<State>,
    /// 書き手を起こす（積まれた）・待ち手を起こす（1 件終わった）の両方に使う
    cv: Condvar,
    sink: Mutex<Sink>,
    on_slow: OnSlow,
}

/// 持ち主の印。最後の [`PaneLogWriter`] が落ちたら書き手のスレッドへ終わりを知らせる
struct Owner {
    shared: Arc<Shared>,
}

impl Drop for Owner {
    fn drop(&mut self) {
        lock(&self.shared.state).shutdown = true;
        self.shared.cv.notify_all();
    }
}

/// ペインログの専用の書き手（の持ち手）。複製は同じ列を指す
#[derive(Clone)]
pub struct PaneLogWriter {
    owner: Arc<Owner>,
    /// 積んだ場で同期で実行する（A/B・単体テストの既定・スレッドを起こせなかったとき）
    inline: bool,
}

impl std::fmt::Debug for PaneLogWriter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PaneLogWriter")
            .field("inline", &self.inline)
            .field("stats", &self.stats())
            .finish()
    }
}

impl PaneLogWriter {
    /// 積んだ場で同期で実行する書き手（修正前と同じ = Mutex の内側で I/O する）
    pub fn inline() -> Self {
        Self::build(
            Box::new(|op: &WriteOp| op.run()),
            Box::new(|_: &str| {}),
            true,
        )
    }

    /// 本物の I/O で書き手のスレッドを起こす。1 件が遅かったときの 1 行を `on_slow` へ渡す
    /// （GUI は perf.log へ残す）
    pub fn spawn(on_slow: impl Fn(&str) + Send + Sync + 'static) -> Self {
        Self::spawn_with(|op: &WriteOp| op.run(), on_slow)
    }

    /// 実行の中身を差し替えて起こす（テストが遅れ・失敗を注入する口）
    pub(crate) fn spawn_with(
        sink: impl FnMut(&WriteOp) + Send + 'static,
        on_slow: impl Fn(&str) + Send + Sync + 'static,
    ) -> Self {
        Self::build(Box::new(sink), Box::new(on_slow), false)
    }

    fn build(sink: Sink, on_slow: OnSlow, inline: bool) -> Self {
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                queue: VecDeque::new(),
                busy: false,
                stats: WriterStats::default(),
                shutdown: false,
            }),
            cv: Condvar::new(),
            sink: Mutex::new(sink),
            on_slow,
        });
        let mut inline = inline;
        if !inline {
            let worker = Arc::clone(&shared);
            // スレッドを起こせない（資源不足）ときは積んだ場で書く = 修正前と同じ
            inline = std::thread::Builder::new()
                .name("tako-pane-log-writer".into())
                .spawn(move || run(&worker))
                .is_err();
        }
        Self {
            owner: Arc::new(Owner { shared }),
            inline,
        }
    }

    fn shared(&self) -> &Shared {
        &self.owner.shared
    }

    /// 背景のスレッドで書くか（偽なら積んだ場で同期で書く）
    pub fn is_background(&self) -> bool {
        !self.inline
    }

    /// 操作を積む（背景なら I/O を待たずに戻る）。呼び手はマネージャーの Mutex の内側で積む =
    /// 列の順が状態を進めた順になる
    pub(crate) fn submit(&self, op: WriteOp) {
        if self.inline {
            execute(self.shared(), op);
            return;
        }
        let mut st = lock(&self.shared().state);
        st.stats.backlog_bytes += op.backlog_bytes();
        st.queue.push_back(op);
        drop(st);
        self.shared().cv.notify_all();
    }

    /// 積んだ操作をすべて実行し終わるまで待つ（終了の前）。待った長さを返す。
    /// **マネージャーの Mutex を握ったまま呼ばない**（書き手はその Mutex を取らないので詰まりはしないが、
    /// 待つ間ほかの呼び手を止める）
    pub fn flush(&self) -> Duration {
        let t0 = Instant::now();
        let mut st = lock(&self.shared().state);
        while !st.queue.is_empty() || st.busy {
            st = self
                .shared()
                .cv
                .wait(st)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
        }
        t0.elapsed()
    }

    /// 列に残っている追記の本文のバイト数（ディスクが詰まった間に溜まった量）
    pub fn backlog_bytes(&self) -> u64 {
        lock(&self.shared().state).stats.backlog_bytes
    }

    /// 内訳
    pub fn stats(&self) -> WriterStats {
        let st = lock(&self.shared().state);
        WriterStats {
            pending: !st.queue.is_empty() || st.busy,
            ..st.stats
        }
    }
}

fn lock(state: &Mutex<State>) -> MutexGuard<'_, State> {
    state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// 1 件を実行する（中の panic で呼び手・書き手を止めない）。所要が長ければ `on_slow` へ 1 行
fn execute(shared: &Shared, op: WriteOp) {
    let t0 = Instant::now();
    let kind = op.kind();
    {
        let mut sink = shared
            .sink
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _ = std::panic::catch_unwind(AssertUnwindSafe(|| (sink)(&op)));
    }
    let took = t0.elapsed();
    if took >= SLOW_OP_LOG_OVER {
        (shared.on_slow)(&format!(
            "ペインログの{kind}（背景）が {}ms（メインスレッドは待っていない。#2618）",
            took.as_millis()
        ));
    }
    let mut st = lock(&shared.state);
    st.stats.done += 1;
    st.stats.max_op_ms = st.stats.max_op_ms.max(took.as_millis() as u64);
}

/// 書き手のスレッドの本体。列を先頭から 1 件ずつ実行する
fn run(shared: &Shared) {
    loop {
        let op = {
            let mut st = lock(&shared.state);
            loop {
                if let Some(op) = st.queue.pop_front() {
                    st.busy = true;
                    break op;
                }
                if st.shutdown {
                    return;
                }
                st = shared
                    .cv
                    .wait(st)
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
            }
        };
        let size = op.backlog_bytes();
        execute(shared, op);
        {
            let mut st = lock(&shared.state);
            st.busy = false;
            st.stats.backlog_bytes = st.stats.backlog_bytes.saturating_sub(size);
        }
        shared.cv.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pane_log::{
        ChunkKind, CloseOrigin, PaneLogConfig, PaneLogManager, PaneLogMeta, PaneObservation,
        BACKLOG_DEFER_OVER,
    };
    use crate::test_residue::ScratchDir;

    /// 1 件ごとに合図を待ってから本物の I/O を行う書き手（合図の送り手を落とすと以後は素通り）。
    /// 実時間の遅れではなく合図で「書き手が詰まっている」状態を作る
    fn gated() -> (PaneLogWriter, mpsc::Sender<()>) {
        let (go, rx) = mpsc::channel::<()>();
        let writer = PaneLogWriter::spawn_with(
            move |op: &WriteOp| {
                let _ = rx.recv();
                op.run();
            },
            |_: &str| {},
        );
        (writer, go)
    }

    fn meta(tab: u64, label: &str) -> PaneLogMeta {
        PaneLogMeta {
            tab,
            label: Some(label.into()),
        }
    }

    fn counted(history: usize, lines: &[&str], delta: usize) -> PaneObservation {
        PaneObservation {
            history,
            history_limit: 10_000,
            bytes: 0,
            alt_screen: false,
            chunk: ChunkKind::Counted {
                lines: lines.iter().map(|s| s.to_string()).collect(),
                delta,
            },
        }
    }

    /// 時刻（`YYYY-MM-DDTHH:MM:SSZ`）を伏せる（同期と背景で書いた中身を比べるため）
    fn mask_ts(s: &str) -> String {
        let b = s.as_bytes();
        let is_ts = |w: &[u8]| {
            w.len() == 20
                && w.iter().enumerate().all(|(i, c)| match i {
                    4 | 7 => *c == b'-',
                    10 => *c == b'T',
                    13 | 16 => *c == b':',
                    19 => *c == b'Z',
                    _ => c.is_ascii_digit(),
                })
        };
        let mut out = String::new();
        let mut i = 0;
        while i < b.len() {
            if i + 20 <= b.len() && is_ts(&b[i..i + 20]) {
                out.push_str("<ts>");
                i += 20;
            } else {
                let ch = s[i..].chars().next().expect("文字境界");
                out.push(ch);
                i += ch.len_utf8();
            }
        }
        out
    }

    /// dir の中身（ファイル名の時刻を伏せた名前 → 時刻を伏せた中身）を名前順に並べる
    fn contents(dir: &std::path::Path) -> Vec<(String, String)> {
        let mut out: Vec<(String, String)> = std::fs::read_dir(dir)
            .map(|rd| {
                rd.filter_map(Result::ok)
                    .map(|e| {
                        let name = e.file_name().to_string_lossy().to_string();
                        let name = name
                            .split_once('_')
                            .map_or(name.clone(), |(_, r)| r.to_string());
                        let body = std::fs::read_to_string(e.path()).unwrap_or_default();
                        (name, mask_ts(&body))
                    })
                    .collect()
            })
            .unwrap_or_default();
        out.sort();
        out
    }

    /// 同期と背景の両方へ同じ観測を流す（TUI の印・省略・ローテーション・閉じるを含む）
    fn feed(mgr: &mut PaneLogManager) {
        let m = meta(3, "sh");
        for i in 0..40usize {
            let line = format!("row-{i:03} 0123456789");
            mgr.apply(7, &m, counted(i + 1, &[&line], 1));
        }
        // 省略の印（増分 50 のうち 2 行だけ読めた）
        mgr.apply(7, &m, counted(91, &["tail-a", "tail-b"], 50));
        // TUI 区間の印
        let mut tui = counted(91, &[], 0);
        tui.alt_screen = true;
        tui.chunk = ChunkKind::None;
        mgr.apply(7, &m, tui);
        mgr.apply(7, &m, counted(92, &["after-tui"], 1));
        mgr.flush_close(
            7,
            &m,
            &["screen-1".into(), "screen-2".into(), "".into()],
            CloseOrigin::Cli,
            None,
        );
        // 別のペイン（並びが混ざらない）
        mgr.apply(8, &meta(3, "other"), counted(2, &["p8-a", "p8-b"], 2));
    }

    #[test]
    fn 背景の書き手でもファイルの中身と順序は同期で書いたときと同じ() {
        let config = PaneLogConfig {
            // ローテーションを何度も起こす
            max_bytes_per_pane: 200,
            ..Default::default()
        };
        let sync_dir = ScratchDir::new("pane-log-writer-sync");
        let mut sync = PaneLogManager::new(sync_dir.to_path_buf(), config);
        feed(&mut sync);

        let bg_dir = ScratchDir::new("pane-log-writer-bg");
        let (writer, go) = gated();
        let mut bg = PaneLogManager::new(bg_dir.to_path_buf(), config).with_writer(writer.clone());
        feed(&mut bg);
        // 書き手は合図待ちで 1 件も書いていない = 上の apply / flush_close は I/O を待たずに戻った
        assert!(writer.stats().pending, "書き手に積まれたまま");
        assert_eq!(writer.stats().done, 0, "合図の前は 1 件も書かない");
        drop(go);
        writer.flush();
        assert!(!writer.stats().pending);
        let want = contents(&sync_dir);
        assert!(
            want.len() >= 3,
            "ローテーションの .1 と 2 ペインぶんがある: {want:?}"
        );
        assert_eq!(
            contents(&bg_dir),
            want,
            "背景で書いた中身・並びが同期と一致する"
        );
    }

    #[test]
    fn 背景の書き手は書き手が詰まっていてもapplyとflush_closeを待たせない() {
        let dir = ScratchDir::new("pane-log-writer-noblock");
        let (writer, go) = gated();
        let mut mgr = PaneLogManager::new(dir.to_path_buf(), PaneLogConfig::default())
            .with_writer(writer.clone());
        let m = meta(1, "sh");
        // 合図を 1 つも送らない（= 書き手は 1 件目で止まっている）まま、取り込み・閉じるを繰り返す。
        // 修正前の形（Mutex の内側で I/O）ならここで止まって戻らない
        for i in 0..20usize {
            mgr.apply(1, &m, counted(i + 1, &["x"], 1));
        }
        mgr.flush_close(1, &m, &["last-visible".into()], CloseOrigin::Cli, None);
        let written = std::fs::read_dir(dir.path())
            .map(|rd| rd.count())
            .unwrap_or(0);
        assert_eq!(
            written, 0,
            "まだ 1 件も書いていない（I/O は書き手の中だけ）"
        );
        assert!(writer.backlog_bytes() > 0, "列に溜まっている");
        drop(go);
        let _ = writer.flush();
        let files = crate::pane_log::list_files(dir.path());
        assert_eq!(files.len(), 1);
        let body = std::fs::read_to_string(&files[0].path).unwrap();
        let lines: Vec<&str> = body.lines().collect();
        assert!(lines[0].starts_with("# tako pane log"), "{body}");
        assert_eq!(lines.iter().filter(|l| **l == "x").count(), 20, "{body}");
        assert_eq!(lines[lines.len() - 2], "last-visible", "{body}");
        assert!(
            crate::pane_log::close_marker_reason(lines[lines.len() - 1]).is_some(),
            "最後の行はクローズマーカー: {body}"
        );
        assert_eq!(writer.backlog_bytes(), 0);
    }

    #[test]
    fn flushは積んだ操作をすべて書き終えてから戻る() {
        let dir = ScratchDir::new("pane-log-writer-flush");
        let (writer, go) = gated();
        let mut mgr = PaneLogManager::new(dir.to_path_buf(), PaneLogConfig::default())
            .with_writer(writer.clone());
        let m = meta(1, "sh");
        for i in 0..10usize {
            mgr.apply(1, &m, counted(i + 1, &["y"], 1));
        }
        mgr.flush_close(1, &m, &[], CloseOrigin::AppQuit, None);
        // 合図は別のスレッドから 1 つずつ送る（flush は全部を書き終えるまで戻らない）
        let feeder = std::thread::spawn(move || {
            for _ in 0..64 {
                if go.send(()).is_err() {
                    break;
                }
                std::thread::yield_now();
            }
        });
        writer.flush();
        let stats = writer.stats();
        assert!(!stats.pending);
        assert_eq!(stats.done, 11, "追記 10 + マーカー 1 を書き終えている");
        let files = crate::pane_log::list_files(dir.path());
        let body = std::fs::read_to_string(&files[0].path).unwrap();
        assert!(
            body.lines()
                .last()
                .and_then(crate::pane_log::close_marker_reason)
                == Some("close:app-quit"),
            "{body}"
        );
        feeder.join().unwrap();
    }

    #[test]
    fn 列が溜まり過ぎている間は取り込みを見送り基準を進めない() {
        let dir = ScratchDir::new("pane-log-writer-defer");
        let (writer, go) = gated();
        let mut mgr = PaneLogManager::new(dir.to_path_buf(), PaneLogConfig::default())
            .with_writer(writer.clone());
        let m = meta(1, "sh");
        let huge = "z".repeat(BACKLOG_DEFER_OVER as usize + 1);
        mgr.apply(1, &m, counted(1, &[&huge], 1));
        assert_eq!(mgr.scan_baseline(1), Some((1, 0)));
        // 列が上限を超えた: 次の取り込みは見送られ、基準は 1 のまま（次の機会に読み直す）
        mgr.apply(1, &m, counted(5, &["a", "b", "c", "d"], 4));
        assert_eq!(mgr.deferred(), 1);
        assert_eq!(
            mgr.scan_baseline(1),
            Some((1, 0)),
            "見送った回は基準を進めない"
        );
        drop(go);
        writer.flush();
        mgr.apply(1, &m, counted(5, &["a", "b", "c", "d"], 4));
        assert_eq!(
            mgr.scan_baseline(1),
            Some((5, 0)),
            "列が空けば読み直して進む"
        );

        // 閉じたときのフラッシュは列が溜まっていても見送らない（最後の行とマーカーを残す）
        let (writer2, go2) = gated();
        let dir2 = ScratchDir::new("pane-log-writer-defer-close");
        let mut mgr2 = PaneLogManager::new(dir2.to_path_buf(), PaneLogConfig::default())
            .with_writer(writer2.clone());
        mgr2.apply(1, &m, counted(1, &[&huge], 1));
        mgr2.apply(1, &m, counted(3, &["a", "b"], 2));
        assert_eq!(mgr2.deferred(), 1, "取り込みは見送る");
        mgr2.flush_close(1, &m, &["end".into()], CloseOrigin::Cli, None);
        drop(go2);
        writer2.flush();
        // 大きな 1 行でペインあたりの上限を超えたので、閉じる行は `.1` へ退避した後の新しいファイルへ
        // （同期で書いたときと同じ）
        let current = crate::pane_log::list_files(dir2.path())
            .into_iter()
            .find(|f| f.path.extension().is_some_and(|e| e == "log"))
            .expect("現行のファイル");
        let body = std::fs::read_to_string(&current.path).unwrap();
        let lines: Vec<&str> = body.lines().collect();
        assert_eq!(lines.len(), 3, "ヘッダ → 画面 → マーカー: {body}");
        assert_eq!(lines[1], "end", "画面の行が残る");
        assert_eq!(
            crate::pane_log::close_marker_reason(lines[2]),
            Some("close:dispatch(cli)"),
            "マーカーが残る"
        );
    }

    #[test]
    fn 書き手の中のpanicで後続の操作を止めない() {
        let dir = ScratchDir::new("pane-log-writer-panic");
        let mut first = true;
        let writer = PaneLogWriter::spawn_with(
            move |op: &WriteOp| {
                if std::mem::take(&mut first) {
                    panic!("注入: 1 件目で panic");
                }
                op.run();
            },
            |_: &str| {},
        );
        let mut mgr = PaneLogManager::new(dir.to_path_buf(), PaneLogConfig::default())
            .with_writer(writer.clone());
        let m = meta(1, "sh");
        mgr.apply(1, &m, counted(1, &["lost-by-panic"], 1));
        mgr.apply(1, &m, counted(2, &["kept"], 1));
        writer.flush();
        let files = crate::pane_log::list_files(dir.path());
        let body = std::fs::read_to_string(&files[0].path).unwrap();
        assert!(body.contains("kept"), "{body}");
        assert_eq!(writer.stats().done, 2);
    }

    #[test]
    fn 全体上限の強制は先に積んだ追記を書き終えてから消し受け口へ件数を返す() {
        let dir = ScratchDir::new("pane-log-writer-cap");
        let config = PaneLogConfig {
            // 開いていないファイルはすべて消える上限
            max_total_bytes: 1,
            ..Default::default()
        };
        let (writer, go) = gated();
        let mut mgr = PaneLogManager::new(dir.to_path_buf(), config).with_writer(writer.clone());
        mgr.apply(1, &meta(1, "closed"), counted(1, &["c-1"], 1));
        mgr.apply(2, &meta(1, "open"), counted(1, &["o-1"], 1));
        // ペイン 1 を閉じる（最後の行とマーカーはまだ列の中）→ 上限の強制を積む
        mgr.flush_close(
            1,
            &meta(1, "closed"),
            &["c-last".into()],
            CloseOrigin::Cli,
            None,
        );
        let rx = mgr.queue_total_cap();
        // 受け口はマネージャーを借りずに待てる（呼び手は Mutex を放してから待つ）
        drop(go);
        let removed = rx.recv().expect("件数が届く");
        writer.flush();
        assert_eq!(removed, 1, "閉じたペインのファイルだけ消える");
        let files = crate::pane_log::list_files(dir.path());
        assert_eq!(
            files.len(),
            1,
            "閉じたペインのファイルを最後の追記で作り直さない: {files:?}"
        );
        assert_eq!(files[0].pane, Some(2), "開いているペインのファイルは残す");
    }

    #[test]
    fn 同期の書き手は積んだ場で書く() {
        let dir = ScratchDir::new("pane-log-writer-inline");
        let mut mgr = PaneLogManager::new(dir.to_path_buf(), PaneLogConfig::default());
        assert!(!mgr.writer().is_background());
        mgr.apply(1, &meta(1, "sh"), counted(1, &["now"], 1));
        let files = crate::pane_log::list_files(dir.path());
        assert_eq!(files.len(), 1, "flush を待たずにもう書いてある");
        assert_eq!(mgr.queue_total_cap().try_recv(), Ok(0));
    }
}
