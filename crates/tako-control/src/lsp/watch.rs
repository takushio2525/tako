//! tako の外で変わったファイルの監視（`workspace/didChangeWatchedFiles`。#2034）
//!
//! 言語サーバが `client/registerCapability` で登録したグロブ（読み取りと照合は
//! `tako_core::lsp::watch`）の**根だけ**を OS のファイル監視で見て、当たった変化をまとめて
//! サーバへ知らせる。これが無いと、`cargo fmt` / `git checkout` / 別のエディタで書き換えた
//! **開いていないファイル**をサーバは古い中身のまま持ち続け、参照・定義・診断の位置がずれる。
//!
//! ## 費用（#2011 の軽量化と衝突させない）
//!
//! - **走査しない**。監視は OS のイベント（macOS = FSEvents・Windows = ReadDirectoryChangesW。
//!   `notify` の `RecommendedWatcher` = 表示中プレビューの監視 #233 と同じ依存）だけで、
//!   ディレクトリを辿らない。変化の種別を決めるのに、当たったパス 1 つにつき stat 1 回
//! - 監視は **manager 全体で 1 つ**（サーバの本数に比例しない）。動いているあいだのスレッドは
//!   OS の監視 1 本 + まとめ送り 1 本の **2 本**で、どのサーバも登録していなければ 0 本（監視を落とす）
//! - まとめ送りは [`DEBOUNCE`] 静かになるまで待ち、続いても [`MAX_DELAY`] ごとに 1 回送る
//!   （`git checkout` の何百ファイルを 1 通にする。送信キューの上限 256 通を溢れさせない）
//! - アイドルでは何も起きない（OS が知らせてこない限りスレッドは待っているだけ）
//!
//! ## 開いている文書
//!
//! 開いている文書の中身はクライアントのもの（LSP の約束）で、サーバは監視の知らせでは読み直さない。
//! エディタの文書が外で変わったら、エディタの読み直し（#1659。未編集なら追従・編集中なら競合）が
//! 本文を差し替え、その本文を `didChange` で送る（`LspManager::sync` = 打鍵と同じ 1 本）。
//! 知らせは VS Code / Zed と同じく開いている文書にも送る（サーバが捨てる）。
//!
//! `TAKO_2034_LEGACY=1` で旧挙動（申告しない・登録を受けても見ない）へ戻す（A/B の入口）。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use serde_json::{json, Value};
use tako_core::lsp::watch::{plan_roots, WatchRoot};
use tako_core::platform::path::{canonicalize_or_self, relative_under};

/// 最後の変化からこれだけ静かになったら送る
pub const DEBOUNCE: Duration = Duration::from_millis(100);
/// 変化が続いても、最初の変化からこれだけ経ったら送る
pub const MAX_DELAY: Duration = Duration::from_secs(1);

/// `TAKO_2034_LEGACY=1` で監視しない（A/B の入口）
pub fn legacy() -> bool {
    matches!(
        std::env::var("TAKO_2034_LEGACY").ok().as_deref(),
        Some("1" | "true" | "on")
    )
}

/// まとめた変化（パス → 窓の中で作成を見たか）
pub type Batch = BTreeMap<PathBuf, bool>;

/// まとめた変化を受け取る関数（manager がサーバへ振り分ける）
pub type BatchHandler = Arc<dyn Fn(Batch) + Send + Sync>;

/// 監視の実体（manager に 1 つ）
pub struct WatchHub {
    on_batch: BatchHandler,
    /// 動いている監視（`None` = 登録が無いので落としている）
    watcher: Option<RecommendedWatcher>,
    /// 見ている根（与えた形 → 再帰か）
    watched: BTreeMap<PathBuf, bool>,
    /// OS が知らせてくる実パス → 与えた形（macOS の `/var` → `/private/var` 等。#970 の比較キー）
    aliases: Arc<Mutex<Vec<(PathBuf, PathBuf)>>>,
    /// 見なかった根と理由（広すぎる・無い・OS が断った）
    skipped: Vec<(PathBuf, String)>,
    counters: Arc<Counters>,
}

#[derive(Default)]
struct Counters {
    /// OS から受けたパスの数（種別で捨てたものは数えない）
    events: AtomicU64,
    /// まとめて振り分けた回数
    batches: AtomicU64,
    /// OS がイベントを落とした（溢れた）と知らせてきた回数。走査はしないので、その間の変化は届かない
    overflows: AtomicU64,
    /// 監視を起こした回数（登録が無くなると落とし、また登録されたら起こす）
    starts: AtomicU64,
}

impl WatchHub {
    pub fn new(on_batch: BatchHandler) -> Self {
        Self {
            on_batch,
            watcher: None,
            watched: BTreeMap::new(),
            aliases: Arc::new(Mutex::new(Vec::new())),
            skipped: Vec::new(),
            counters: Arc::new(Counters::default()),
        }
    }

    /// 見る根を `roots`（全サーバの登録から）へ揃える。空なら監視を落とす（スレッドも終わる）
    pub fn apply(&mut self, roots: Vec<WatchRoot>) {
        let home = tako_core::paths::home_dir();
        let (wanted, broad) = plan_roots(roots, home.as_deref());
        let mut skipped: Vec<(PathBuf, String)> = broad
            .into_iter()
            .map(|(path, reason)| (path, reason.to_string()))
            .collect();
        if wanted.is_empty() {
            self.stop();
            self.skipped = skipped;
            return;
        }
        if self.watcher.is_none() {
            match self.start() {
                Ok(watcher) => self.watcher = Some(watcher),
                Err(error) => {
                    // 監視を起こせない（OS の上限等）: 知らせないだけで、LSP の他の機能は止めない
                    crate::diag::persist_log(&format!(
                        "LSP ファイル監視: 起こせない（{}）",
                        error_kind(&error)
                    ));
                    skipped.extend(wanted.into_iter().map(|r| (r.path, "start".to_string())));
                    self.skipped = skipped;
                    return;
                }
            }
        }
        let Some(watcher) = self.watcher.as_mut() else {
            return;
        };
        let wanted: BTreeMap<PathBuf, bool> =
            wanted.into_iter().map(|r| (r.path, r.recursive)).collect();
        // 外す: もう要らない根・再帰かどうかが変わった根
        for (path, recursive) in &self.watched {
            if wanted.get(path) != Some(recursive) {
                let _ = watcher.unwatch(path);
            }
        }
        let mut watched = BTreeMap::new();
        for (path, recursive) in wanted {
            if self.watched.get(&path) == Some(&recursive) {
                watched.insert(path, recursive);
                continue;
            }
            let mode = if recursive {
                RecursiveMode::Recursive
            } else {
                RecursiveMode::NonRecursive
            };
            match watcher.watch(&path, mode) {
                Ok(()) => {
                    watched.insert(path, recursive);
                }
                Err(error) => skipped.push((path, error_kind(&error).to_string())),
            }
        }
        let aliases: Vec<(PathBuf, PathBuf)> = watched
            .keys()
            .filter_map(|given| {
                let real = canonicalize_or_self(given);
                (real != *given).then(|| (real, given.clone()))
            })
            .collect();
        *self.aliases.lock().unwrap_or_else(|e| e.into_inner()) = aliases;
        self.watched = watched;
        self.skipped = skipped;
        if self.watched.is_empty() {
            self.stop();
        }
    }

    /// 監視を落とす（OS の監視のスレッドが終わり、送り手が消えてまとめ送りのスレッドも終わる）
    pub fn stop(&mut self) {
        self.watcher = None;
        self.watched.clear();
        self.aliases
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
    }

    pub fn is_active(&self) -> bool {
        self.watcher.is_some()
    }

    /// `tako lsp status` の `file_watch`
    pub fn status(&self) -> Value {
        json!({
            "enabled": !legacy(),
            "active": self.is_active(),
            "roots": self.watched.iter().map(|(path, recursive)| json!({
                "path": path.display().to_string(),
                "recursive": recursive,
            })).collect::<Vec<_>>(),
            "skipped": self.skipped.iter().map(|(path, reason)| json!({
                "path": path.display().to_string(),
                "reason": reason,
            })).collect::<Vec<_>>(),
            "events": self.counters.events.load(Ordering::Relaxed),
            "batches": self.counters.batches.load(Ordering::Relaxed),
            "overflows": self.counters.overflows.load(Ordering::Relaxed),
            "starts": self.counters.starts.load(Ordering::Relaxed),
        })
    }

    /// OS の監視とまとめ送りのスレッドを起こす
    fn start(&mut self) -> notify::Result<RecommendedWatcher> {
        let (tx, rx) = mpsc::channel::<(PathBuf, bool)>();
        let counters = Arc::clone(&self.counters);
        let watcher = notify::recommended_watcher(move |result: notify::Result<Event>| {
            let event = match result {
                Ok(event) => event,
                Err(_) => {
                    counters.overflows.fetch_add(1, Ordering::Relaxed);
                    return;
                }
            };
            if event.need_rescan() {
                counters.overflows.fetch_add(1, Ordering::Relaxed);
                return;
            }
            // 中身の変わらないイベント（読んだ・権限 / mtime だけ）は捨てる（#233 と同じ判定）
            match event.kind {
                EventKind::Access(_)
                | EventKind::Modify(notify::event::ModifyKind::Metadata(_)) => return,
                _ => {}
            }
            let created = matches!(event.kind, EventKind::Create(_));
            for path in event.paths {
                counters.events.fetch_add(1, Ordering::Relaxed);
                let _ = tx.send((path, created));
            }
        })?;
        let aliases = Arc::clone(&self.aliases);
        let counters = Arc::clone(&self.counters);
        let on_batch = Arc::clone(&self.on_batch);
        std::thread::Builder::new()
            .name("lsp-watch".into())
            .spawn(move || collect_loop(&rx, &aliases, &counters, &*on_batch))
            .map_err(notify::Error::io)?;
        self.counters.starts.fetch_add(1, Ordering::Relaxed);
        Ok(watcher)
    }
}

/// まとめ送りのスレッド: 最初の変化を待ち、静かになるか上限が来たら 1 回渡す。
/// OS の監視が落ちる（送り手が消える）と抜ける
fn collect_loop(
    rx: &mpsc::Receiver<(PathBuf, bool)>,
    aliases: &Mutex<Vec<(PathBuf, PathBuf)>>,
    counters: &Counters,
    on_batch: &(dyn Fn(Batch) + Send + Sync),
) {
    while let Ok(first) = rx.recv() {
        let mut batch = Batch::new();
        let mut add = |(path, created): (PathBuf, bool)| {
            *batch.entry(path).or_insert(false) |= created;
        };
        add(first);
        let started = Instant::now();
        let mut closed = false;
        loop {
            let left = MAX_DELAY.saturating_sub(started.elapsed());
            if left.is_zero() {
                break;
            }
            match rx.recv_timeout(DEBOUNCE.min(left)) {
                Ok(item) => add(item),
                Err(mpsc::RecvTimeoutError::Timeout) => break,
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    closed = true;
                    break;
                }
            }
        }
        let batch = to_given(batch, &aliases.lock().unwrap_or_else(|e| e.into_inner()));
        counters.batches.fetch_add(1, Ordering::Relaxed);
        on_batch(batch);
        if closed {
            return;
        }
    }
}

/// OS が知らせた実パスを、サーバへ渡した形（登録の base・根）へ戻す。
/// macOS の FSEvents は `/var/…` を `/private/var/…` で知らせる = そのままでは base の配下と判定できず、
/// サーバの知っている URI とも綴りが食い違う
fn to_given(batch: Batch, aliases: &[(PathBuf, PathBuf)]) -> Batch {
    if aliases.is_empty() {
        return batch;
    }
    batch
        .into_iter()
        .map(|(path, created)| {
            let given = aliases.iter().find_map(|(real, given)| {
                relative_under(real, &path).map(|rel| {
                    if rel.is_empty() {
                        given.clone()
                    } else {
                        given.join(tako_core::file_uri::native_separators(&rel).as_ref())
                    }
                })
            });
            (given.unwrap_or(path), created)
        })
        .fold(Batch::new(), |mut out, (path, created)| {
            *out.entry(path).or_insert(false) |= created;
            out
        })
}

/// 失敗の種類（診断ログ用。パスは書かない）
fn error_kind(error: &notify::Error) -> &'static str {
    match &error.kind {
        notify::ErrorKind::PathNotFound => "missing",
        notify::ErrorKind::MaxFilesWatch => "limit",
        notify::ErrorKind::Io(_) => "io",
        notify::ErrorKind::WatchNotFound => "not_watched",
        notify::ErrorKind::InvalidConfig(_) => "config",
        notify::ErrorKind::Generic(_) => "generic",
    }
}

/// 根の変化を受けたら、そのパスがまだ在るかで種別を決めるための stat（まとめ 1 回に 1 パス 1 回）
pub fn exists(path: &Path) -> bool {
    path.symlink_metadata().is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 実パスは渡した形へ戻す() {
        let aliases = vec![(PathBuf::from("/private/var/x"), PathBuf::from("/var/x"))];
        let batch = Batch::from([
            (PathBuf::from("/private/var/x/src/a.rs"), true),
            (PathBuf::from("/var/x/src/a.rs"), false),
            (PathBuf::from("/elsewhere/b.rs"), false),
        ]);
        let out = to_given(batch, &aliases);
        assert_eq!(
            out,
            Batch::from([
                (PathBuf::from("/elsewhere/b.rs"), false),
                (PathBuf::from("/var/x/src/a.rs"), true),
            ]),
            "同じファイルの 2 つの綴りは 1 つへ畳み、作成を見たかは足し合わせる"
        );
    }

    #[test]
    fn まとめ送りは続けて届いた分を_1_回で渡し_送り手が消えたら抜ける() {
        let (tx, rx) = mpsc::channel();
        // 先に全部積んでから受け手を起こす（送り手が途中で止まって窓が切れる、を実時間に任せない）
        for n in 0..300 {
            tx.send((PathBuf::from(format!("/w/f{}.rs", n % 150)), n == 0))
                .unwrap();
        }
        drop(tx);
        let got: Arc<Mutex<Vec<Batch>>> = Arc::default();
        let sink = Arc::clone(&got);
        let handle = std::thread::spawn(move || {
            collect_loop(
                &rx,
                &Mutex::new(Vec::new()),
                &Counters::default(),
                &move |batch| sink.lock().unwrap().push(batch),
            )
        });
        handle.join().unwrap();
        let got = got.lock().unwrap();
        assert_eq!(got.len(), 1, "続けて届いた 300 件は 1 回にまとまる");
        assert_eq!(got[0].len(), 150, "同じパスは 1 つへ畳む");
        assert_eq!(got[0].get(Path::new("/w/f0.rs")), Some(&true));
    }
}
