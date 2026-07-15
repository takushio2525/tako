//! 表示中プレビューファイルのイベント駆動監視（Issue #233）。
//!
//! ファイル自身ではなく親ディレクトリを非再帰で監視する。エディタの原子的保存
//! （一時ファイルを rename で置換）後も監視が失われず、削除後の再作成も検知できる。

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use futures::channel::mpsc::{unbounded, UnboundedReceiver};
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};

pub const RELOAD_DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(300);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreviewWatchSignal {
    Paths(Vec<PathBuf>),
    Rescan,
}

pub struct PreviewFileWatcher {
    watcher: RecommendedWatcher,
    watched_dirs: HashSet<PathBuf>,
    targets: Arc<RwLock<HashSet<PathBuf>>>,
}

impl PreviewFileWatcher {
    pub fn new() -> notify::Result<(Self, UnboundedReceiver<PreviewWatchSignal>)> {
        let (tx, rx) = unbounded();
        let targets = Arc::new(RwLock::new(HashSet::new()));
        let callback_targets = Arc::clone(&targets);
        let watcher = notify::recommended_watcher(move |result: notify::Result<Event>| {
            let signal = match result {
                Ok(event) => signal_for_event(&event, &read_targets(&callback_targets)),
                // 監視バックエンドがイベント欠落を報告した場合と同様に、表示中だけを
                // 1 回再確認する。エラー内容やファイルパスは診断ログへ出さない。
                Err(_) => Some(PreviewWatchSignal::Rescan),
            };
            if let Some(signal) = signal {
                let _ = tx.unbounded_send(signal);
            }
        })?;
        Ok((
            Self {
                watcher,
                watched_dirs: HashSet::new(),
                targets,
            },
            rx,
        ))
    }

    /// 表示中かつ対応形式のファイルだけへ監視対象を同期する。
    pub fn sync_paths<I>(&mut self, paths: I) -> notify::Result<()>
    where
        I: IntoIterator<Item = PathBuf>,
    {
        let desired_targets: HashSet<PathBuf> = paths.into_iter().collect();
        let desired_dirs: HashSet<PathBuf> = desired_targets
            .iter()
            .filter_map(|path| path.parent().map(Path::to_path_buf))
            .collect();

        let mut added: Vec<PathBuf> = Vec::new();
        for dir in desired_dirs.difference(&self.watched_dirs) {
            if let Err(error) = self.watcher.watch(dir, RecursiveMode::NonRecursive) {
                for added_dir in added {
                    let _ = self.watcher.unwatch(&added_dir);
                }
                return Err(error);
            }
            added.push(dir.to_path_buf());
        }
        for dir in self.watched_dirs.difference(&desired_dirs) {
            // OS 側ですでに消えたディレクトリ等は、登録集合から外すことを優先する。
            let _ = self.watcher.unwatch(dir);
        }
        self.watched_dirs = desired_dirs;
        *write_targets(&self.targets) = desired_targets;
        Ok(())
    }
}

fn read_targets(
    targets: &Arc<RwLock<HashSet<PathBuf>>>,
) -> std::sync::RwLockReadGuard<'_, HashSet<PathBuf>> {
    targets.read().unwrap_or_else(|error| error.into_inner())
}

fn write_targets(
    targets: &Arc<RwLock<HashSet<PathBuf>>>,
) -> std::sync::RwLockWriteGuard<'_, HashSet<PathBuf>> {
    targets.write().unwrap_or_else(|error| error.into_inner())
}

fn signal_for_event(event: &Event, targets: &HashSet<PathBuf>) -> Option<PreviewWatchSignal> {
    if event.need_rescan() {
        return Some(PreviewWatchSignal::Rescan);
    }
    // 内容変更を伴わないイベントでは再ロードしない。
    // Access（読み取り / open / close）と Modify(Metadata(_))（権限・xattr・mtime
    // のみの変更。macOS FSEvents は Spotlight インデックス更新でこれを送る）を除外する。
    match event.kind {
        EventKind::Access(_) => return None,
        EventKind::Modify(notify::event::ModifyKind::Metadata(_)) => return None,
        _ => {}
    }
    let mut paths: Vec<PathBuf> = event
        .paths
        .iter()
        .filter(|path| targets.contains(*path))
        .cloned()
        .collect();
    paths.sort();
    paths.dedup();
    (!paths.is_empty()).then_some(PreviewWatchSignal::Paths(paths))
}

#[cfg(test)]
mod tests {
    use super::*;
    use notify::event::{AccessKind, ModifyKind};

    #[test]
    fn 対象ファイルの変更だけを通知する() {
        let target = PathBuf::from("/tmp/preview.md");
        let targets = HashSet::from([target.clone()]);
        let changed = Event::new(EventKind::Modify(ModifyKind::Data(
            notify::event::DataChange::Any,
        )))
        .add_path(target.clone())
        .add_path(PathBuf::from("/tmp/sibling.md"));
        assert_eq!(
            signal_for_event(&changed, &targets),
            Some(PreviewWatchSignal::Paths(vec![target]))
        );

        let access = Event::new(EventKind::Access(AccessKind::Read))
            .add_path(PathBuf::from("/tmp/preview.md"));
        assert_eq!(signal_for_event(&access, &targets), None);

        let metadata = Event::new(EventKind::Modify(ModifyKind::Metadata(
            notify::event::MetadataKind::Any,
        )))
        .add_path(PathBuf::from("/tmp/preview.md"));
        assert_eq!(signal_for_event(&metadata, &targets), None);
    }

    #[test]
    fn os監視で実ファイル変更を受信する() {
        let dir = std::env::temp_dir().join(format!(
            "tako-preview-watch-test-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("preview.md");
        std::fs::write(&path, "# before\n").unwrap();
        let path = path.canonicalize().unwrap();

        let (mut watcher, mut rx) = PreviewFileWatcher::new().unwrap();
        watcher.sync_paths([path.clone()]).unwrap();
        // FSEvents ストリームの起動は watch 登録と非同期。登録直後のテスト書き込みが
        // ストリーム開始より先行しないよう、この OS 結合テストだけ少し待つ。
        std::thread::sleep(std::time::Duration::from_millis(200));
        std::fs::write(&path, "# after\n").unwrap();

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        let mut received = false;
        while std::time::Instant::now() < deadline {
            if let Ok(signal) = rx.try_recv() {
                if matches!(signal, PreviewWatchSignal::Paths(paths) if paths.contains(&path)) {
                    received = true;
                    break;
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        assert!(received, "OS ネイティブ監視が 3 秒以内に変更を通知する");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
