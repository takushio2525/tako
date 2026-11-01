//! layout.json の書き出しを UI スレッドの外で行う専用の書き手（#2610）。
//!
//! `save_layout`（tako-app）は 2 秒 tick と dispatch ごとに呼ばれ、構成が変わっていれば
//! 書き出す。以前は書き出し（直前の版の読み直し・世代バックアップ・整形・write・rename）を
//! メインスレッドで同期していたので、ディスクが詰まると GUI が丸ごと固まった
//! （本番の perf.log で最大 98,982ms・閾値超えの p50 でも 150〜200ms）。ここでは:
//!
//! - メインスレッドは状態の採取（capture）と前回との比較だけを行い、版を [`LayoutWriter::post`]
//!   で渡して戻る。I/O を待たない
//! - 書き手のスレッドは 1 本だけ。待っている版は常に最新の 1 つで、書き出しの最中に次々と
//!   来た版は最新以外を書かずに捨てる（合流）。1 本が渡された順に書くので、
//!   **古い版が新しい版を上書きしない**
//! - ファイルの置き換えは従来どおり [`crate::layout::save`]（tmp + rename・何かを失う保存の
//!   前の世代バックアップ・空の保存の拒否）。書き出しの途中でプロセスが落ちても
//!   layout.json は前の版か新しい版のどちらか
//! - 消す（[`crate::layout::remove`]）も同じ列に並べる。先に渡した版が、消した後で書き戻さない
//! - 終了と、新しいプロセスへの引き渡し（更新の起動し直し）の前は [`LayoutWriter::flush`] で
//!   書き終わりを待つ（待つ長さは従来の同期の書き出しと同じで、短くはならない）
//!
//! 書けなかった版は「保存済み」に数えない: 呼び手は [`LayoutWriter::latest_failed`] を見て
//! 次の呼び出しで書き直す（従来の「失敗したらキーを進めずに次の tick でやり直す」と同じ）。

use std::io;
use std::panic::AssertUnwindSafe;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use crate::layout::LayoutFile;

/// 書き手へ渡す 1 件
#[derive(Debug)]
pub enum LayoutOp {
    /// この版で layout.json を置き換える
    Write(Box<LayoutFile>),
    /// layout.json を消す（最後のペインの明示クローズ・永続化 OFF）
    Remove,
}

/// 書き出し 1 件の所要がこれを超えたら perf.log へ残す（メインスレッドは待っていないので
/// 「専有」ではない。ディスクの詰まりを後から読めるようにするための記録）
const SLOW_WRITE_LOG_OVER: Duration = Duration::from_millis(1000);

/// 書き手の内訳（`tako persist` の `save_layout` に合流する）
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WriterStats {
    /// 実際にディスクへ書いた回数
    pub written: u64,
    /// 書く前に新しい版が来たので書かずに捨てた回数（合流）
    pub coalesced: u64,
    /// 書けなかった回数（理由は persist.log の「保存失敗」）
    pub failed: u64,
    /// 書き出し 1 件の最長（ms）
    pub max_write_ms: u64,
    /// 待っている版か書き出し中の版があるか
    pub pending: bool,
}

type Sink = Box<dyn FnMut(&LayoutOp) -> io::Result<()> + Send>;
type OnError = Box<dyn Fn(&str) + Send + Sync>;

struct State {
    /// 待っている版（常に最新の 1 つ）と通し番号
    pending: Option<(u64, LayoutOp)>,
    /// 書き手が 1 件を処理している最中か
    busy: bool,
    /// 最後に渡した版の通し番号
    posted: u64,
    /// 最後に書けなかった版の通し番号
    failed_seq: Option<u64>,
    stats: WriterStats,
    /// 持ち主が手放した（スレッドは待っている版を書き終えてから抜ける）
    shutdown: bool,
}

struct Shared {
    state: Mutex<State>,
    /// 書き手を起こす（版が来た）・待ち手を起こす（1 件終わった）の両方に使う
    cv: Condvar,
    sink: Mutex<Sink>,
    on_error: OnError,
}

/// layout.json の専用の書き手。tako-app の GUI が 1 つ持つ
pub struct LayoutWriter {
    shared: Arc<Shared>,
    /// スレッドを起こせなかった（資源不足）。そのときは渡された場で同期で書く = 従来と同じ
    inline: bool,
}

impl LayoutWriter {
    /// 本物の書き出し（`layout::save` / `layout::remove`）で書き手を起こす。
    /// 書けなかった理由は `on_error` へ 1 行で渡す（GUI は persist.log へ残す）
    pub fn spawn(on_error: impl Fn(&str) + Send + Sync + 'static) -> Self {
        Self::spawn_with(
            |op: &LayoutOp| match op {
                LayoutOp::Write(layout) => crate::layout::save(layout).map(|_| ()),
                LayoutOp::Remove => {
                    crate::layout::remove();
                    Ok(())
                }
            },
            on_error,
        )
    }

    /// 書き出しの中身を差し替えて起こす（テストが遅れ・失敗を注入する口）
    pub fn spawn_with(
        sink: impl FnMut(&LayoutOp) -> io::Result<()> + Send + 'static,
        on_error: impl Fn(&str) + Send + Sync + 'static,
    ) -> Self {
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                pending: None,
                busy: false,
                posted: 0,
                failed_seq: None,
                stats: WriterStats::default(),
                shutdown: false,
            }),
            cv: Condvar::new(),
            sink: Mutex::new(Box::new(sink)),
            on_error: Box::new(on_error),
        });
        let worker = Arc::clone(&shared);
        let inline = std::thread::Builder::new()
            .name("tako-layout-writer".into())
            .spawn(move || run(&worker))
            .map_err(|e| {
                (shared.on_error)(&format!(
                    "layout の書き手のスレッドを起こせない（以後はメインスレッドで書く）: {e}"
                ))
            })
            .is_err();
        Self { shared, inline }
    }

    /// 版を渡す（I/O を待たずに戻る）。まだ書いていない前の版があれば、それは書かずに捨てる
    pub fn post(&self, layout: LayoutFile) -> u64 {
        self.enqueue(LayoutOp::Write(Box::new(layout)))
    }

    /// layout.json を消す。前に渡した版はこの後で書き戻さない
    pub fn post_remove(&self) -> u64 {
        self.enqueue(LayoutOp::Remove)
    }

    fn enqueue(&self, op: LayoutOp) -> u64 {
        let mut st = lock(&self.shared.state);
        st.posted += 1;
        let seq = st.posted;
        if self.inline {
            drop(st);
            execute(&self.shared, seq, op);
            return seq;
        }
        if st.pending.replace((seq, op)).is_some() {
            st.stats.coalesced += 1;
        }
        drop(st);
        self.shared.cv.notify_all();
        seq
    }

    /// 渡した版をすべて書き終わるまで待つ（終了・新しいプロセスへの引き渡しの前）。待った長さを返す
    pub fn flush(&self) -> Duration {
        let t0 = Instant::now();
        let mut st = lock(&self.shared.state);
        while st.pending.is_some() || st.busy {
            st = self
                .shared
                .cv
                .wait(st)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
        }
        t0.elapsed()
    }

    /// 最後に渡した版が書けずに終わったか。真なら呼び手はその版を「保存済み」から外し、
    /// 次の呼び出しで書き直す（新しい版をまだ処理中なら偽 = その結果を待つ）
    pub fn latest_failed(&self) -> bool {
        let st = lock(&self.shared.state);
        st.failed_seq == Some(st.posted) && st.pending.is_none() && !st.busy
    }

    /// 内訳（`tako persist`）
    pub fn stats(&self) -> WriterStats {
        let st = lock(&self.shared.state);
        WriterStats {
            pending: st.pending.is_some() || st.busy,
            ..st.stats
        }
    }
}

impl Drop for LayoutWriter {
    fn drop(&mut self) {
        // 待っている版は書き終えてから抜ける（GUI はプロセスと同じ寿命なので、ここへ来るのはテスト）
        lock(&self.shared.state).shutdown = true;
        self.shared.cv.notify_all();
    }
}

/// poisoned でも書き手を止めない（保存が止まる方が「消えた」に直結する）
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// 書き手のスレッドの本体。待っている版を 1 つずつ取り出して書く
fn run(shared: &Shared) {
    loop {
        let (seq, op) = {
            let mut st = lock(&shared.state);
            loop {
                if let Some(job) = st.pending.take() {
                    st.busy = true;
                    break job;
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
        execute(shared, seq, op);
    }
}

/// 1 件を書き、結果を記録して待ち手を起こす。状態のロックは I/O の間は握らない
fn execute(shared: &Shared, seq: u64, op: LayoutOp) {
    let t0 = Instant::now();
    // 書き出しの中の panic で書き手ごと止めない（flush が永久に待つ形を作らない）
    let result = std::panic::catch_unwind(AssertUnwindSafe(|| (lock(&shared.sink))(&op)))
        .unwrap_or_else(|_| Err(io::Error::other("書き出しの途中で panic した")));
    let took = t0.elapsed();
    if took >= SLOW_WRITE_LOG_OVER {
        crate::diag::perf_log(&format!(
            "layout.json の書き出し（背景）が {}ms（#2610。メインスレッドは待っていない）",
            took.as_millis()
        ));
    }
    // 理由は「終わった」を知らせる前に残す（終了の flush が返った後で書きかけにならない）
    if let Err(e) = &result {
        (shared.on_error)(&format!("保存失敗: {e}"));
    }
    let mut st = lock(&shared.state);
    st.busy = false;
    st.stats.max_write_ms = st.stats.max_write_ms.max(took.as_millis() as u64);
    if result.is_ok() {
        if matches!(op, LayoutOp::Write(_)) {
            st.stats.written += 1;
        }
        if st.failed_seq.is_some_and(|f| f < seq) {
            st.failed_seq = None;
        }
    } else {
        st.stats.failed += 1;
        st.failed_seq = Some(seq);
    }
    drop(st);
    shared.cv.notify_all();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    /// 版の見分けは `active_tab` に載せた通し番号で行う（中身は書き手にとって不透明）
    fn version(n: u64) -> LayoutFile {
        LayoutFile {
            version: crate::layout::LAYOUT_VERSION,
            active_tab: n,
            tabs: Vec::new(),
            window: None,
            backgrounded: Vec::new(),
            collapsed: Vec::new(),
            webview_dock: Vec::new(),
            windows: Vec::new(),
            shelved_tabs: Vec::new(),
        }
    }

    /// 書いた順の記録（`Some(n)` = 版 n を書いた・`None` = 消した）
    type Log = Arc<Mutex<Vec<Option<u64>>>>;

    fn record(log: &Log, op: &LayoutOp) {
        lock(log).push(match op {
            LayoutOp::Write(l) => Some(l.active_tab),
            LayoutOp::Remove => None,
        });
    }

    /// 1 件目の書き出しを `gate` が開くまで止める書き手（「書き出しの最中に次の版が来る」を作る）。
    /// 止まったことは `entered` で知らせる。待ちの上限は試験が固まらないための保険で、所要は比べない
    fn gated_writer(log: &Log) -> (LayoutWriter, mpsc::Sender<()>, mpsc::Receiver<()>) {
        let (gate_tx, gate_rx) = mpsc::channel::<()>();
        let (entered_tx, entered_rx) = mpsc::channel::<()>();
        let log = Arc::clone(log);
        let mut first = true;
        let writer = LayoutWriter::spawn_with(
            move |op| {
                if std::mem::take(&mut first) {
                    let _ = entered_tx.send(());
                    let _ = gate_rx.recv_timeout(Duration::from_secs(30));
                }
                record(&log, op);
                Ok(())
            },
            |_| {},
        );
        (writer, gate_tx, entered_rx)
    }

    /// #2610: `post` は書き出しを待たずに戻る（書き出しが止まっていても呼び手は先へ進める）
    #[test]
    fn issue2610_postは書き出しを待たずに戻る() {
        let log: Log = Arc::default();
        let (writer, gate, entered) = gated_writer(&log);
        writer.post(version(1));
        entered
            .recv_timeout(Duration::from_secs(30))
            .expect("書き手が 1 件目を取り出していない");
        // 書き手は gate で止まっている。ここへ戻って来られた = post は I/O を待っていない
        writer.post(version(2));
        assert!(
            lock(&log).is_empty(),
            "書き出しが止まっている間に何かが書かれた（post が同期で書いている）"
        );
        assert!(writer.stats().pending, "書き出し中なのに pending が偽");
        gate.send(()).unwrap();
        writer.flush();
        assert_eq!(*lock(&log), vec![Some(1), Some(2)]);
        assert!(!writer.stats().pending);
    }

    /// #2610: 書き出しの最中に来た版は最新だけを書く（合流）。古い版が新しい版の後に書かれない
    #[test]
    fn issue2610_書き出し中に来た版は最新だけを順に書く() {
        let log: Log = Arc::default();
        let (writer, gate, entered) = gated_writer(&log);
        writer.post(version(1));
        entered.recv_timeout(Duration::from_secs(30)).unwrap();
        for n in 2..=5 {
            writer.post(version(n));
        }
        gate.send(()).unwrap();
        writer.flush();
        assert_eq!(
            *lock(&log),
            vec![Some(1), Some(5)],
            "書き出し中に来た 2〜4 は最新の 5 に合流して捨てる"
        );
        let stats = writer.stats();
        assert_eq!((stats.written, stats.coalesced, stats.failed), (2, 3, 0));
    }

    /// #2610: 消す操作は同じ列に並ぶ。先に渡した版が消した後で書き戻さない
    #[test]
    fn issue2610_消した後で前の版を書き戻さない() {
        let log: Log = Arc::default();
        let (writer, gate, entered) = gated_writer(&log);
        writer.post(version(1));
        entered.recv_timeout(Duration::from_secs(30)).unwrap();
        writer.post(version(2));
        writer.post_remove();
        gate.send(()).unwrap();
        writer.flush();
        assert_eq!(*lock(&log), vec![Some(1), None]);
        assert_eq!(lock(&log).last(), Some(&None), "最後は消した状態");
    }

    /// #2610: 最後の版が書けなければ `latest_failed` が真（呼び手は次の呼び出しで書き直す）。
    /// 理由は従来と同じ「保存失敗: …」の 1 行で渡る。新しい版が書ければ偽へ戻る
    #[test]
    fn issue2610_書けなかった版は保存済みに数えず理由を残す() {
        let errors: Arc<Mutex<Vec<String>>> = Arc::default();
        let errs = Arc::clone(&errors);
        let writer = LayoutWriter::spawn_with(
            |op| match op {
                LayoutOp::Write(l) if l.active_tab == 1 => {
                    Err(io::Error::new(io::ErrorKind::PermissionDenied, "書けない"))
                }
                _ => Ok(()),
            },
            move |msg| lock(&errs).push(msg.to_string()),
        );
        writer.post(version(1));
        writer.flush();
        assert!(writer.latest_failed(), "書けなかったのに保存済み扱い");
        assert_eq!(*lock(&errors), vec!["保存失敗: 書けない".to_string()]);
        writer.post(version(2));
        writer.flush();
        assert!(!writer.latest_failed(), "次の版は書けたのに失敗のまま");
        let stats = writer.stats();
        assert_eq!((stats.written, stats.failed), (1, 1));
    }

    /// #2610: 書き出しの中の panic で書き手が止まらない（flush が永久に待つ形を作らない）
    #[test]
    fn issue2610_書き出しのpanicで書き手が止まらない() {
        let log: Log = Arc::default();
        let sink_log = Arc::clone(&log);
        let writer = LayoutWriter::spawn_with(
            move |op| {
                if matches!(op, LayoutOp::Write(l) if l.active_tab == 1) {
                    panic!("注入した panic");
                }
                record(&sink_log, op);
                Ok(())
            },
            |_| {},
        );
        writer.post(version(1));
        writer.flush();
        assert!(writer.latest_failed());
        writer.post(version(2));
        writer.flush();
        assert_eq!(*lock(&log), vec![Some(2)]);
    }

    /// #2610: 本物のファイル（tmp + rename の `layout::write_file`）でも、最後に渡した版が残る。
    /// 書き出しの途中のファイルは tmp にしか無いので、layout.json は常に読める完全な版
    #[test]
    fn issue2610_本物のファイルへ最後の版が残り途中の版は読めない形で残らない() {
        let dir = tako_core::test_residue::ScratchDir::new("layout-writer-2610");
        let path = dir.join("layout.json");
        let target = path.clone();
        let seen: Arc<Mutex<Vec<u64>>> = Arc::default();
        let seen_in = Arc::clone(&seen);
        let writer = LayoutWriter::spawn_with(
            move |op| {
                // 書く直前の layout.json は常に読める完全な版（前の版か、まだ無いか）
                if let Ok(prev) = crate::layout::load_file(&target) {
                    lock(&seen_in).push(prev.active_tab);
                }
                match op {
                    LayoutOp::Write(l) => crate::layout::write_file(&target, l),
                    LayoutOp::Remove => std::fs::remove_file(&target),
                }
            },
            |_| {},
        );
        for n in 1..=50 {
            writer.post(version(n));
        }
        writer.flush();
        let last = crate::layout::load_file(&path).expect("layout.json が読めない");
        assert_eq!(last.active_tab, 50, "最後に渡した版が残っていない");
        let seen = lock(&seen).clone();
        assert!(
            seen.windows(2).all(|w| w[0] < w[1]),
            "書いた版の並びが戻っている（古い版が新しい版を上書きした）: {seen:?}"
        );
    }
}
