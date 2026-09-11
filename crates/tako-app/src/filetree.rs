//! filetree — 左サイドバーのファイルツリー（FR-3.1 / FR-3.7）
//!
//! 「タブ = ワークスペース」: アクティブタブ内の**全ペインの cwd**（OSC 7 検知）を
//! ワークスペースフォルダとして並べるマルチルートツリー（VSCode の
//! 「フォルダをワークスペースに追加」相当。2026-06-13 にフォーカスペイン連動から変更）。
//! 状態・読み込み・フラット化は GPUI 非依存（描画は main.rs 側）。
//! 内容の更新はポーリング（表示中のみ。notify クレートは必要になったら再判断 =
//! `architecture.md`「コンセプト②の実現」）。

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use tako_core::remote_fs::{RemoteEntry, RemoteFolder, RemoteOrigin, RemoteRef};
use tako_core::sidebar::Truncation;

/// 1 ディレクトリの最大表示エントリ数（巨大ディレクトリの暴走防止）
const MAX_ENTRIES: usize = 500;
/// 展開を辿る最大深さ（暴走防止の最後の砦）。
///
/// #1398 でリンクを辿るようになるまで、この上限に**到達し得る経路は無かった**
/// （リンクを辿らない = 実ディレクトリの深さしか増えない）。循環そのものは
/// `collect_rows` が canonical パスの照合で打ち切るので、ここは
/// 「照合が空振りした場合」に効く二重の歯止め
const MAX_DEPTH: usize = 16;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub path: PathBuf,
    pub name: String,
    pub is_dir: bool,
}

// git ステータスの分類・伝播・無視の判定は tako-core が正本（#1009）。
// ここは表示のために引くだけで、UI 層に判定ロジックを持たない
pub use tako_core::git_tree::{TreeGitMap, TreeGitState, TreeGitStatus};

/// 表示用にフラット化した 1 行。`root` = ワークスペースフォルダの見出し行
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub entry: Entry,
    pub depth: usize,
    pub expanded: bool,
    pub root: bool,
    /// git の状態（#1009。ディレクトリ行・ルート行には配下からの伝播が入る）
    pub git_status: Option<TreeGitStatus>,
    /// `Some` = リモート（SSH 先）の行（#919）。**ローカル FS の操作を一切通さない**
    /// ための目印で、クリック・右クリック・D&D はここを見て分岐する。
    /// `entry.path` は表示と行の同定にだけ使い、ファイルシステムへは渡さない
    pub remote: Option<RemoteRef>,
    /// 読み込み中・失敗の説明行（クリックできない情報行）。
    /// #919 の「静かな失敗禁止」= 失敗を行として必ず見せる
    pub note: Option<RowNote>,
}

/// 情報行の種別（クリック不可の行に何を出すか）
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RowNote {
    /// 読み込み中
    Loading,
    /// 失敗（要約 + 次の一手 + 詳細をそのまま出す）
    Error(String),
    /// 空ディレクトリ
    Empty,
    /// 表示を上限（[`MAX_ENTRIES`]）で切り詰めた（#1402）。
    /// **失敗ではない**ので描画は red ではなく muted（`sidebar::render_note_row`）
    Truncated { shown: usize, total: usize },
}

/// 1 ディレクトリの読み取り結果（#1402）。
///
/// 以前は `Vec<Entry>` だけを返していたので、**上限で切り詰めた**ことと
/// **読めなかった**ことが呼び出し側に届かず、どちらも「空のディレクトリ」と
/// 同じ見え方になっていた（機械可読側の `tree git-status` は `truncated` を
/// 申告するのに画面だけ黙る = #1402 の非対称）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DirListing {
    /// 表示するエントリ（上限で切り詰めたあと）
    pub entries: Vec<Entry>,
    /// 上限で切り詰めた事実。判断は CLI / MCP（`tree git-status` の `truncated`）と
    /// **同じ 1 実装**（`tako_core::sidebar::Truncation`）
    pub truncation: Truncation,
    /// `read_dir` が失敗した理由（`None` = 読めた）。
    /// **空ディレクトリと区別する**ために持つ（権限なしを黙って空にしない）
    pub error: Option<String>,
}

/// 1 リモートディレクトリの読み込み状態
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RemoteDir {
    pub entries: Vec<RemoteEntry>,
    /// 失敗の説明（`RemoteError::report()` をそのまま持つ）
    pub error: Option<String>,
    /// 読み込みを投げて結果を待っている
    pub loading: bool,
}

/// ファイルツリーの状態。`visible` は FR-3.7（折りたたみで純粋なターミナルに戻る）
#[derive(Default)]
pub struct FileTree {
    pub visible: bool,
    roots: Vec<PathBuf>,
    /// 展開中ディレクトリ（ルート自身も含む。絶対パスがキーなのでルート間で共有できる）
    expanded: HashSet<PathBuf>,
    /// ディレクトリごとの読み取り結果（#1402 で `Vec<Entry>` から広げた）
    cache: HashMap<PathBuf, DirListing>,
    /// git status キャッシュ（#1009。絶対パス → 状態。ディレクトリ伝播込み）
    git_cache: TreeGitMap,
    /// rows() の結果キャッシュ（状態変化時に無効化し、render での再構築を回避する）
    rows_cache: Option<Vec<Row>>,
    /// ドット始まりの項目を表示するか（#550。既定 false）。
    /// キャッシュには全項目を保持し、表示時にだけ絞るので切替は即時に効く。
    /// なお VSCode / Zed の既定は `.git` 等の個別除外リストで、ドット全体は隠さない
    /// （実機で確認済み）。tako はホームを開いた初回印象が壊れるのを優先して
    /// ドット全体を既定で隠し、ワンクリックで戻せる形にしている
    show_hidden: bool,

    // --- リモート（SSH 先）のワークスペースフォルダ（#919 / #65） -------------
    //
    // ローカルと**別の器**に持つ: `PathBuf` は OS 依存の区切りを持つので、
    // リモートの POSIX パス（Windows の `/C:/...` を含む）を混ぜると
    // Windows 側で `join` / `parent` が `\\` を作って壊れる。
    // 読み込みはネットワーク I/O なので**展開したときだけ**背景で取る（ポーリングしない）
    /// リモートルート（開いた順・経路つき）。#1041: 経路（明示 open / `ssh` 検知）で
    /// ローカルルートの前後どちらに出るかが決まる（規則は
    /// `tako_core::sidebar::remote_root_order` が正本）
    remote_roots: Vec<RemoteFolder>,
    /// 展開中のリモートディレクトリ（ルート自身も含む）
    remote_expanded: HashSet<RemoteRef>,
    /// リモートディレクトリの読み込み状態
    remote_cache: HashMap<RemoteRef, RemoteDir>,
}

impl FileTree {
    pub fn roots(&self) -> &[PathBuf] {
        &self.roots
    }

    /// ドット始まり項目の表示状態（#550）
    pub fn show_hidden(&self) -> bool {
        self.show_hidden
    }

    /// ドット始まり項目の表示切替（#550）。変化があれば true（再描画判断用）
    pub fn set_show_hidden(&mut self, show: bool) -> bool {
        if self.show_hidden == show {
            return false;
        }
        self.show_hidden = show;
        self.rows_cache = None;
        true
    }

    /// ワークスペースフォルダ列の同期（FR-3.1。呼び出し側がタブ内ペインの cwd を集める）。
    /// 重複は除き、既存ルートの展開状態は維持する。変化があれば true（再描画判断用）。
    ///
    /// #550: **新しく増えたルートは自動展開したうえで先頭へ出す**。ホーム 1 本で始まった
    /// ツリーにペインの `cd` 由来ルートが足されたとき、末尾に積むと 100 行以上下へ埋もれて
    /// 見出しだけが切り替わったように見えるため。既存ルートの並びは維持するので、
    /// 同じ集合で再同期しても順序は暴れない（呼び出し側はポーリングで毎秒叩く）
    pub fn set_roots(&mut self, roots: Vec<PathBuf>) -> bool {
        let mut deduped: Vec<PathBuf> = Vec::new();
        for root in roots {
            if !deduped.contains(&root) {
                deduped.push(root);
            }
        }
        // 新規ルートを先頭、既存ルートは現在の表示順のまま後ろへ
        let mut ordered: Vec<PathBuf> = deduped
            .iter()
            .filter(|r| !self.roots.contains(r))
            .cloned()
            .collect();
        ordered.extend(self.roots.iter().filter(|r| deduped.contains(r)).cloned());
        if self.roots == ordered {
            return false;
        }
        // 消えたルートの状態は畳む（配下の展開・キャッシュは refresh が掃除する）
        for old in &self.roots {
            if !ordered.contains(old) {
                self.expanded.remove(old);
                self.cache.remove(old);
            }
        }
        for root in &ordered {
            if !self.roots.contains(root) {
                self.expanded.insert(root.clone());
                self.cache
                    .entry(root.clone())
                    .or_insert_with(|| read_dir_sorted(root));
            }
        }
        self.roots = ordered;
        self.rows_cache = None;
        true
    }

    /// ディレクトリを展開する（既に展開中なら何もしない）
    pub fn expand_dir(&mut self, path: &Path) {
        if !self.expanded.contains(path) {
            self.expanded.insert(path.to_path_buf());
            self.cache
                .entry(path.to_path_buf())
                .or_insert_with(|| read_dir_sorted(path));
            self.rows_cache = None;
        }
    }

    /// ディレクトリ行（ルート見出し行を含む）のクリック: 展開 ⇄ 折りたたみ
    pub fn toggle_dir(&mut self, path: &Path) {
        if self.expanded.contains(path) {
            self.expanded.remove(path);
        } else {
            self.expanded.insert(path.to_path_buf());
            self.cache
                .entry(path.to_path_buf())
                .or_insert_with(|| read_dir_sorted(path));
        }
        self.rows_cache = None;
    }

    /// git status キャッシュを更新する。変化があれば true
    pub fn apply_git_status(&mut self, status: TreeGitMap) -> bool {
        if self.git_cache == status {
            return false;
        }
        self.git_cache = status;
        self.rows_cache = None;
        true
    }

    /// 表示中のツリーが持つ git 状態（テストから行の状態を直接引くため）
    #[cfg(test)]
    pub fn git_status_of(&self, path: &Path) -> Option<TreeGitStatus> {
        self.git_cache.get(path)
    }

    /// 表示行: ルート見出し行 + 展開状態に従った深さ優先の中身。
    /// キャッシュがあればクローンを返し、なければ構築してキャッシュする
    pub fn rows(&mut self) -> Vec<Row> {
        if let Some(cached) = &self.rows_cache {
            return cached.clone();
        }
        let rows = self.build_rows();
        self.rows_cache = Some(rows.clone());
        rows
    }

    fn build_rows(&self) -> Vec<Row> {
        let mut rows = Vec::new();
        // #1041: リモートルートをローカルの前後どちらへ出すかは
        // `tako_core::sidebar::remote_root_order`（並び規則の正本）が決める。
        // ここは分けてもらった 2 本を前後に並べるだけ = CLI / MCP の
        // `remote-folder list` と必ず同じ並びになる
        let order = tako_core::sidebar::remote_root_order(
            &self.remote_roots,
            crate::ssh_folders::remote_root_placement(),
        );
        for remote in &order.leading {
            self.collect_remote_rows(remote, 0, true, &mut rows);
        }
        for root in &self.roots {
            let expanded = self.expanded.contains(root);
            rows.push(Row {
                entry: Entry {
                    path: root.clone(),
                    name: root
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| root.display().to_string()),
                    is_dir: true,
                },
                depth: 0,
                expanded,
                root: true,
                // #1009: ワークスペースフォルダにも配下の変更が伝播する
                // （折りたたんでいても「このプロジェクトに変更がある」が分かる）
                git_status: self.git_cache.get(root),
                remote: None,
                note: None,
            });
            if expanded {
                // #1398: 祖先の実体（canonical パス）を積みながら降りる
                let mut chain = vec![tako_core::platform::path::canonicalize_or_self(root)];
                self.collect_rows(root, 1, &mut chain, &mut rows);
            }
        }
        // #976: 自動検知で増えたリモートルートは**ローカルの後ろに普通に並ぶ**。
        // #919 は「開いた直後に見えないと分からない」として全部先頭へ hoist して
        // いたが、ssh の自動検知（#976）で日常的に増えるものになったので、
        // 特別扱いをやめて「ワークスペースフォルダが 1 つ増えた」と同じ見え方へ寄せた。
        // #1041 で**明示的に開いた分だけ**先頭（上の `leading`）へ戻している
        for remote in &order.trailing {
            self.collect_remote_rows(remote, 0, true, &mut rows);
        }
        rows
    }

    /// 展開中ディレクトリの中身を深さ優先で積む。
    ///
    /// `chain` は**ここまでに入ったディレクトリの実体**（canonical パス。起点は
    /// ルート）。#1398 でリンクを辿るようになったので、`a/link -> a` や
    /// `da/to_b -> ../db` + `db/to_a -> ../da` のように**同じ実体へ戻る展開**が
    /// 起こり得る。戻ってきたら打ち切り行（[`loop_cut_row`]）で止める
    fn collect_rows(
        &self,
        dir: &Path,
        depth: usize,
        chain: &mut Vec<PathBuf>,
        rows: &mut Vec<Row>,
    ) {
        if depth >= MAX_DEPTH {
            return;
        }
        let Some(listing) = self.cache.get(dir) else {
            return;
        };
        for entry in &listing.entries {
            // #550: ドット始まりは既定で隠す（ルート見出しは対象外 = ユーザーが
            // 明示的に開いた `~/.claude` 等は隠さない）
            if !self.show_hidden && is_hidden_name(&entry.name) {
                continue;
            }
            let expanded = entry.is_dir && self.expanded.contains(&entry.path);
            // #1009: ディレクトリにも配下からの伝播が返る（`TreeGitMap::get` が面倒を見る）
            let git_status = self.git_cache.get(&entry.path);
            rows.push(Row {
                entry: entry.clone(),
                depth,
                expanded,
                root: false,
                git_status,
                remote: None,
                note: None,
            });
            if !expanded {
                continue;
            }
            // canonicalize は**展開中のディレクトリの数だけ**（= 数十）なので、
            // read_dir と同じ費用の桁に収まる。解決できないときは入力をそのまま
            // 返る（`canonicalize_or_self`）ので、偽の循環判定にはならない
            // （そのぶんは MAX_DEPTH が受け止める）
            let real = tako_core::platform::path::canonicalize_or_self(&entry.path);
            if chain.contains(&real) {
                rows.push(loop_cut_row(&entry.path, depth + 1, &real));
                continue;
            }
            chain.push(real);
            self.collect_rows(&entry.path, depth + 1, chain, rows);
            chain.pop();
        }
        // #1402: 「読めなかった」「上限で切り詰めた」を**このディレクトリの最後の行**
        // として見せる（黙って捨てると「ファイルが存在しない」ように見える）。
        // リモート行（#919）・リンクの打ち切り（#1398）と同じ器・同じ描画経路
        if let Some(note) = local_note_of(listing) {
            rows.push(local_note_row(dir, depth, note));
        }
    }

    /// 1 ディレクトリだけ即座に読み直す（#559。新規作成・リネームの直後に呼び、
    /// 2 秒ポーリングを待たずに結果を見せる）。ディレクトリ 1 個の read_dir なので
    /// UI スレッドで同期に呼んでよい（`expand_dir` と同じ扱い）
    pub fn refresh_dir(&mut self, dir: &Path) {
        if !self.cache.contains_key(dir) && !self.roots.iter().any(|r| r == dir) {
            return;
        }
        let fresh = read_dir_sorted(dir);
        if self.cache.get(dir) != Some(&fresh) {
            self.cache.insert(dir.to_path_buf(), fresh);
            self.rows_cache = None;
        }
    }

    /// 同期 refresh（テスト用。本番は refresh_targets → scan_dirs → apply_refresh）
    #[cfg(test)]
    pub fn refresh(&mut self) -> bool {
        let results = scan_dirs(&self.refresh_targets());
        self.apply_refresh(results)
    }

    /// background executor 向け: スキャン対象のディレクトリ一覧を返す
    pub fn refresh_targets(&self) -> Vec<PathBuf> {
        let mut targets: Vec<PathBuf> = self.roots.clone();
        targets.extend(self.expanded.iter().cloned());
        targets.dedup();
        targets
    }

    // --- リモート（SSH 先）のワークスペースフォルダ（#919 / #65） -------------

    /// 開いているリモートルート（経路つき・開いた順）
    pub fn remote_roots(&self) -> &[RemoteFolder] {
        &self.remote_roots
    }

    /// リモートルートを追加する（既にあれば何もしない）。追加したら true。
    /// 追加時は展開して読み込み待ちにする = 開いた直後から中身を取りに行く
    pub fn add_remote_root(&mut self, folder: RemoteFolder) -> bool {
        if let Some(existing) = self
            .remote_roots
            .iter_mut()
            .find(|f| f.remote == folder.remote)
        {
            // #1041: 開き直しで経路が明示へ上がることがある（自動検知で載っていた
            // home をユーザーが選び直した = 主作業対象へ格上げ）
            if folder.is_explicit() {
                existing.origin = RemoteOrigin::Explicit;
            }
            // 既に開いているものを開き直したら、内容だけ取り直す
            self.remote_cache.remove(&folder.remote);
            self.remote_expanded.insert(folder.remote);
            self.rows_cache = None;
            return false;
        }
        self.remote_expanded.insert(folder.remote.clone());
        // #976: 末尾へ積む（開いた順に並ぶ）。ローカルルートと同じ「並んだ順」の
        // 規則にすると、リモートだけ最新が飛び込んでくる特別扱いが消える。
        // #1041 の前後の振り分けは描画時（`build_rows`）に規則へ問う
        self.remote_roots.push(folder);
        self.rows_cache = None;
        true
    }

    /// リモートルートの経路を差し替える（#1041。格上げ・格下げの反映）。変わったら true
    pub fn set_remote_root_origin(&mut self, remote: &RemoteRef, origin: RemoteOrigin) -> bool {
        let Some(existing) = self.remote_roots.iter_mut().find(|f| &f.remote == remote) else {
            return false;
        };
        if existing.origin == origin {
            return false;
        }
        existing.origin = origin;
        self.rows_cache = None;
        true
    }

    /// リモートルートを閉じる。配下の展開状態・キャッシュも落とす。閉じたら true
    pub fn remove_remote_root(&mut self, remote: &RemoteRef) -> bool {
        let before = self.remote_roots.len();
        self.remote_roots.retain(|f| &f.remote != remote);
        if self.remote_roots.len() == before {
            return false;
        }
        let prefix = format!("{}/", remote.path.trim_end_matches('/'));
        let under = |r: &RemoteRef| {
            r.host == remote.host && (r.path == remote.path || r.path.starts_with(&prefix))
        };
        self.remote_expanded.retain(|r| !under(r));
        self.remote_cache.retain(|r, _| !under(r));
        self.rows_cache = None;
        true
    }

    /// リモートディレクトリの展開 ⇄ 折りたたみ。展開したら読み込み待ちにする
    pub fn toggle_remote_dir(&mut self, remote: &RemoteRef) {
        if self.remote_expanded.contains(remote) {
            self.remote_expanded.remove(remote);
        } else {
            self.remote_expanded.insert(remote.clone());
        }
        self.rows_cache = None;
    }

    /// 内容を取り直す（更新ボタン・開き直し）。配下のキャッシュも落とす
    pub fn invalidate_remote(&mut self, remote: &RemoteRef) {
        let prefix = format!("{}/", remote.path.trim_end_matches('/'));
        self.remote_cache.retain(|r, _| {
            !(r.host == remote.host && (r.path == remote.path || r.path.starts_with(&prefix)))
        });
        self.rows_cache = None;
    }

    /// これから読む必要があるリモートディレクトリ（展開済みでキャッシュが無いもの）。
    /// 呼び出し側が background へ投げ、結果を [`Self::apply_remote_dir`] へ返す。
    /// **ポーリングしない**（ネットワーク I/O を毎秒叩かない）ので、
    /// 一度読んだものは `invalidate_remote` されるまで再取得しない
    pub fn remote_pending(&mut self) -> Vec<RemoteRef> {
        let targets: Vec<RemoteRef> = self
            .remote_expanded
            .iter()
            .filter(|r| !self.remote_cache.contains_key(r))
            .cloned()
            .collect();
        // 二重投げを防ぐため、返した時点で loading を立てる
        for t in &targets {
            self.remote_cache.insert(
                t.clone(),
                RemoteDir {
                    loading: true,
                    ..Default::default()
                },
            );
        }
        if !targets.is_empty() {
            self.rows_cache = None;
        }
        targets
    }

    /// 読み込み結果を反映する（`Err` は失敗の説明を行として見せる）
    pub fn apply_remote_dir(
        &mut self,
        remote: RemoteRef,
        result: Result<Vec<RemoteEntry>, String>,
    ) {
        let state = match result {
            Ok(entries) => RemoteDir {
                entries,
                error: None,
                loading: false,
            },
            Err(report) => RemoteDir {
                entries: Vec::new(),
                error: Some(report),
                loading: false,
            },
        };
        self.remote_cache.insert(remote, state);
        self.rows_cache = None;
    }

    /// その行の読み込み状態（ヘッダの表示・再試行の判断に使う）
    pub fn remote_dir(&self, remote: &RemoteRef) -> Option<&RemoteDir> {
        self.remote_cache.get(remote)
    }

    /// リモートルート（またはその配下）の行を積む
    fn collect_remote_rows(
        &self,
        remote: &RemoteRef,
        depth: usize,
        root: bool,
        rows: &mut Vec<Row>,
    ) {
        if depth >= MAX_DEPTH {
            return;
        }
        let expanded = self.remote_expanded.contains(remote);
        // #976: ルートの名前も**ローカルと同じ「フォルダ名」だけ**にする。
        // どのホストかは行の SSH バッジ（`render_remote_row`）が示すので、
        // 名前に `host: ` を混ぜると同じ深さのローカルルートと形が揃わない
        let name = match root && crate::ssh_folders::legacy_mode() {
            // A/B（`TAKO_976_LEGACY=1`）: #919 の `host: 末尾要素` へ戻す
            true => format!("{}: {}", remote.host, remote.base_name()),
            false => remote.base_name(),
        };
        rows.push(Row {
            entry: Entry {
                // 表示と行の同定にだけ使う（FS へは渡さない）
                path: PathBuf::from(&remote.path),
                name,
                is_dir: true,
            },
            depth,
            expanded,
            root,
            git_status: None,
            remote: Some(remote.clone()),
            note: None,
        });
        if !expanded {
            return;
        }
        let child_depth = depth + 1;
        let Some(state) = self.remote_cache.get(remote) else {
            // 展開直後（読み込みを投げる前）も「待っている」と分かるようにする
            rows.push(self.remote_note_row(remote, child_depth, RowNote::Loading));
            return;
        };
        if state.loading {
            rows.push(self.remote_note_row(remote, child_depth, RowNote::Loading));
            return;
        }
        if let Some(err) = &state.error {
            rows.push(self.remote_note_row(remote, child_depth, RowNote::Error(err.clone())));
            return;
        }
        if state.entries.is_empty() {
            rows.push(self.remote_note_row(remote, child_depth, RowNote::Empty));
            return;
        }
        for entry in &state.entries {
            // #550 と同じ規則: ドット始まりは既定で隠す（ルート見出しは対象外）
            if !self.show_hidden && is_hidden_name(&entry.name) {
                continue;
            }
            let child = RemoteRef::new(remote.host.clone(), entry.path.clone());
            if entry.is_dir() {
                self.collect_remote_rows(&child, child_depth, false, rows);
            } else {
                rows.push(Row {
                    entry: Entry {
                        path: PathBuf::from(&entry.path),
                        name: entry.name.clone(),
                        is_dir: false,
                    },
                    depth: child_depth,
                    expanded: false,
                    root: false,
                    git_status: None,
                    remote: Some(child),
                    note: None,
                });
            }
        }
    }

    fn remote_note_row(&self, remote: &RemoteRef, depth: usize, note: RowNote) -> Row {
        Row {
            entry: Entry {
                path: PathBuf::from(&remote.path),
                name: String::new(),
                is_dir: false,
            },
            depth,
            expanded: false,
            root: false,
            git_status: None,
            remote: Some(remote.clone()),
            note: Some(note),
        }
    }

    /// background executor の結果を適用する。変化があれば true
    pub fn apply_refresh(&mut self, results: Vec<(PathBuf, Option<DirListing>)>) -> bool {
        let mut changed = false;
        for (dir, listing) in results {
            if let Some(fresh) = listing {
                if self.cache.get(&dir) != Some(&fresh) {
                    self.cache.insert(dir, fresh);
                    changed = true;
                }
            } else {
                changed |= self.cache.remove(&dir).is_some();
                changed |= self.expanded.remove(&dir);
            }
        }
        if changed {
            self.rows_cache = None;
        }
        changed
    }
}

/// ディレクトリ列をスキャンする（background executor で呼べる純粋 I/O）。
/// 存在しないディレクトリは None を返す（**読めないだけ**のディレクトリは
/// `Some` で理由つき = #1402。消滅とは扱いが違う）
pub fn scan_dirs(targets: &[PathBuf]) -> Vec<(PathBuf, Option<DirListing>)> {
    targets
        .iter()
        .map(|dir| {
            if dir.is_dir() {
                (dir.clone(), Some(read_dir_sorted(dir)))
            } else {
                (dir.clone(), None)
            }
        })
        .collect()
}

/// 隠し項目の判定（#550）。Unix 慣習のドット始まりのみ（Windows の隠し属性は
/// 境界 B1 側の関心事なのでここでは扱わない）
pub fn is_hidden_name(name: &str) -> bool {
    name.starts_with('.')
}

/// このディレクトリについて出す情報行（#1402）。`None` = 出すものが無い。
///
/// 読めなかったことを先に出す（切り詰めは読めたときだけ起こるので同時には立たない）。
/// **空ディレクトリには何も出さない**: ローカル行に `RowNote::Empty` を足すと
/// 空フォルダ全部の見え方が変わる = この Issue の範囲を超える（リモート行が
/// `Empty` を出すのは、読み込みが非同期で「待っている / 空だった」の区別が
/// 必要なため）。区別すべきは「読めなかった」と「空」で、それは Error 行で足りる
fn local_note_of(listing: &DirListing) -> Option<RowNote> {
    if legacy_1402() {
        return None;
    }
    if let Some(err) = &listing.error {
        return Some(RowNote::Error(crate::ui_text::sidebar::note_read_failed(
            err,
        )));
    }
    if listing.truncation.truncated() {
        return Some(RowNote::Truncated {
            shown: listing.truncation.shown,
            total: listing.truncation.total,
        });
    }
    None
}

/// ローカル行の情報行（押せない行）。描画はリモート行・リンクの打ち切りと
/// 同じ 1 実装（`sidebar::render_note_row`）を通る
fn local_note_row(path: &Path, depth: usize, note: RowNote) -> Row {
    Row {
        entry: Entry {
            // 行の同定にだけ使う（押せない行なので FS へは渡らない）
            path: path.to_path_buf(),
            // 表示は note が持つ（名前を入れると実在の行のように見える）
            name: String::new(),
            is_dir: false,
        },
        depth,
        expanded: false,
        root: false,
        git_status: None,
        remote: None,
        note: Some(note),
    }
}

/// #1402 の A/B。`TAKO_1402_LEGACY=1` で**同一バイナリのまま**旧挙動へ戻す
/// （切り詰めたぶんと読めなかった事実を行に出さない = 「黙って捨てる」の再現）
fn legacy_1402() -> bool {
    static LEGACY: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *LEGACY.get_or_init(|| std::env::var("TAKO_1402_LEGACY").map(|v| v == "1") == Ok(true))
}

/// 展開の打ち切りを**行として見せる**情報行（#1398。行の形は
/// [`local_note_row`] と同じで、こちらは理由の組み立てを持つ）。
///
/// リンクを辿るようになった結果、同じ実体へ戻る展開が起こり得る。黙って空にすると
/// 「押しても何も出ない」= #1398 で直した症状そのものに戻るので、理由と戻り先の実体を
/// 行に出す（`.agent/conventions.md`「弾いたら黙って捨てない」・#919 の静かな失敗禁止）。
/// 描画はリモート行の状態行と同じ 1 実装（`sidebar::render_note_row`）を通る
fn loop_cut_row(path: &Path, depth: usize, real: &Path) -> Row {
    Row {
        entry: Entry {
            // 行の同定にだけ使う（押せない行なので FS へは渡らない）
            path: path.to_path_buf(),
            // 表示は note が持つ。名前を入れると同じ名前の行が 2 つ出て
            // 「2 周目が出ている」ように見えてしまう
            name: String::new(),
            is_dir: false,
        },
        depth,
        expanded: false,
        root: false,
        git_status: None,
        remote: None,
        note: Some(RowNote::Error(crate::ui_text::sidebar::note_symlink_loop(
            &real.display().to_string(),
        ))),
    }
}

/// エントリがディレクトリかを「**リンクを辿った先**」で決める（#1398）。
///
/// `DirEntry::file_type()` はリンクを辿らないので、ディレクトリへのシンボリック
/// リンクは `is_dir = false`（= ファイル行）になっていた。一方で**開く側**は辿る
/// （`dispatch::OpenFile` の `Path::is_file()` / `tako file open-in-tako` /
/// ⌘+クリックの `open_plan::route`）ので、ツリーだけが逆の判断をしていた =
/// 「フォルダなのに chevron が出ず、押すと『ファイルではない』で弾かれる」。
///
/// 追加の `stat` はリンクのエントリの数だけ（通常のファイル / ディレクトリは
/// `file_type()` で決まる）なので、固定費は増えない。
///
/// **辿れないリンク（切れたリンク・`ELOOP`）はファイル行のまま**にする: 中身を
/// 出せないものをディレクトリとして見せると「展開しても空」= 理由の出ない静かな
/// 失敗（#919 / #1399）に戻る。ファイル行なら押したときに `OpenFile` が理由を返し、
/// #1399 の通知欄に出る。
///
/// Windows のジャンクション / シンボリックリンクも `std::fs::metadata` が辿るので
/// cfg の分岐は要らない（実機での確認は別途 = #467）
fn entry_is_dir(path: &Path, file_type: &std::fs::FileType) -> bool {
    if file_type.is_symlink() {
        return std::fs::metadata(path).is_ok_and(|m| m.is_dir());
    }
    file_type.is_dir()
}

/// ディレクトリを読んで「ディレクトリ先・名前（大文字小文字無視）順」に並べる。
///
/// #1402: 結果は `Vec<Entry>` ではなく [`DirListing`] を返す。**切り詰めた**ことと
/// **読めなかった**ことは呼び出し側（描画）が知らなければ行に出せず、どちらも
/// 「空のディレクトリ」と同じ見え方になる（ユーザーには「ファイルが存在しない」に
/// 見える = #1402 の実害。`conventions.md`「弾いたら黙って捨てない」）。
///
/// 切り詰めは `sort_by` の**あと**なので、サブディレクトリが上限ぶんあるディレクトリでは
/// ファイルが 1 つも残らない（= 総数を添えて申告する意味がここにある）。
fn read_dir_sorted(path: &Path) -> DirListing {
    let reader = match std::fs::read_dir(path) {
        Ok(reader) => reader,
        Err(err) => {
            // 権限なし・消滅を**空と区別して**返す（行として見せるのは呼び出し側）
            return DirListing {
                entries: Vec::new(),
                truncation: Truncation::default(),
                error: Some(err.to_string()),
            };
        }
    };
    let mut entries: Vec<Entry> = reader
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let path = e.path();
            let is_dir = entry_is_dir(&path, &e.file_type().ok()?);
            Some(Entry { path, name, is_dir })
        })
        .collect();
    entries.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    let truncation = Truncation::new(entries.len(), MAX_ENTRIES);
    entries.truncate(truncation.shown);
    DirListing {
        entries,
        truncation,
        error: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// テスト用一時ディレクトリの後始末。**一時ディレクトリ配下であることを検証してから**
    /// 消す（変数名の取り違えで実ディレクトリを消す事故を構造的に防ぐ）
    fn remove_temp_dir(dir: &Path) {
        assert!(
            dir.starts_with(std::env::temp_dir()),
            "一時ディレクトリ以外を削除しようとしている: {}",
            dir.display()
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    fn fixture(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("tako-filetree-test-{}-{name}", std::process::id()));
        remove_temp_dir(&dir);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::create_dir_all(dir.join("docs")).unwrap();
        std::fs::write(dir.join("README.md"), "x").unwrap();
        std::fs::write(dir.join("src/main.rs"), "x").unwrap();
        dir
    }

    /// 中身を自分で作る空の使い捨てディレクトリ（件数を数える検査用。#1402）。
    ///
    /// 上の `fixture` は #1312 より前の書き方（作って最後に自分で消す = **panic で
    /// 落ちた回は残る**）。新しいテストは #1312 の器を使う: スコープを抜けた時点で
    /// 消え、`Drop` は巻き戻しでも走るので落ちた回も残骸を出さない
    fn empty_scratch(tag: &str) -> tako_core::test_residue::ScratchDir {
        tako_core::test_residue::ScratchDir::new(&format!("filetree-{tag}"))
    }

    /// (name, depth, root) のタプル列に写す（検証用）
    fn names(tree: &mut FileTree) -> Vec<(String, usize, bool)> {
        tree.rows()
            .iter()
            .map(|r| (r.entry.name.clone(), r.depth, r.root))
            .collect()
    }

    #[test]
    fn ルート見出しの下にディレクトリ先の名前順で並ぶ() {
        let dir = fixture("t1");
        let mut tree = FileTree::default();
        assert!(tree.set_roots(vec![dir.clone()]));
        // 同じルート列の再設定は変化なし
        assert!(!tree.set_roots(vec![dir.clone()]));
        let root_name = dir.file_name().unwrap().to_string_lossy().into_owned();
        assert_eq!(
            names(&mut tree),
            vec![
                (root_name, 0, true),
                ("docs".to_string(), 1, false),
                ("src".to_string(), 1, false),
                ("README.md".to_string(), 1, false),
            ]
        );
        remove_temp_dir(&dir);
    }

    #[test]
    fn 複数ルートが順序と重複除去つきで並ぶ() {
        let dir = fixture("t2");
        let mut tree = FileTree::default();
        assert!(tree.set_roots(vec![
            dir.join("src"),
            dir.join("docs"),
            dir.join("src"), // 重複は除かれる
        ]));
        assert_eq!(tree.roots(), &[dir.join("src"), dir.join("docs")]);
        let rows = names(&mut tree);
        let roots: Vec<_> = rows.iter().filter(|(_, _, root)| *root).collect();
        assert_eq!(roots.len(), 2);
        assert!(rows.contains(&("main.rs".to_string(), 1, false)));
        // ルート見出しの折りたたみで中身が消える
        tree.toggle_dir(&dir.join("src"));
        assert!(!names(&mut tree).contains(&("main.rs".to_string(), 1, false)));
        // ルートが減っても残りの展開状態は維持される
        assert!(tree.set_roots(vec![dir.join("docs")]));
        assert_eq!(names(&mut tree).len(), 1, "docs は空ディレクトリ");
        remove_temp_dir(&dir);
    }

    #[test]
    fn 展開で子が出て折りたたみで消える() {
        let dir = fixture("t3");
        let mut tree = FileTree::default();
        tree.set_roots(vec![dir.clone()]);
        tree.toggle_dir(&dir.join("src"));
        let rows = tree.rows();
        let main_rs = rows.iter().find(|r| r.entry.name == "main.rs").unwrap();
        assert_eq!(main_rs.depth, 2);
        assert!(rows.iter().any(|r| r.entry.name == "src" && r.expanded));
        tree.toggle_dir(&dir.join("src"));
        assert!(!tree.rows().iter().any(|r| r.entry.name == "main.rs"));
        remove_temp_dir(&dir);
    }

    #[test]
    fn refreshは追加と消滅を拾う() {
        let dir = fixture("t4");
        let mut tree = FileTree::default();
        tree.set_roots(vec![dir.clone()]);
        assert!(!tree.refresh(), "変化が無ければ false");
        std::fs::write(dir.join("new.txt"), "x").unwrap();
        assert!(tree.refresh());
        assert!(tree.rows().iter().any(|r| r.entry.name == "new.txt"));
        // 展開中ディレクトリの消滅 → 展開状態ごと畳まれる
        tree.toggle_dir(&dir.join("docs"));
        std::fs::remove_dir_all(dir.join("docs")).unwrap();
        assert!(tree.refresh());
        assert!(!tree
            .rows()
            .iter()
            .any(|r| r.entry.name == "docs" && r.expanded));
        remove_temp_dir(&dir);
    }

    /// 性能計測（通常テストでは走らせない）: `cargo test -p tako-app --release -- --ignored --nocapture perf_`
    #[test]
    #[ignore]
    fn perf_ツリー計測() {
        use std::time::Instant;
        // 合成: 5000 ファイルの大ディレクトリ（node_modules / target 相当）
        let big = std::env::temp_dir().join(format!("tako-filetree-perf-{}", std::process::id()));
        remove_temp_dir(&big);
        std::fs::create_dir_all(&big).unwrap();
        for i in 0..5000 {
            std::fs::write(big.join(format!("file-{i:05}.txt")), "x").unwrap();
        }

        let t0 = Instant::now();
        let listing = read_dir_sorted(&big);
        eprintln!(
            "[perf] read_dir_sorted 5000 エントリ: {:?}（{} 行に切り詰め / 全 {} 件）",
            t0.elapsed(),
            listing.entries.len(),
            listing.truncation.total
        );

        // 実リポジトリ相当: tako リポジトリルートを root に、複数ディレクトリ展開
        let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .unwrap();
        let mut tree = FileTree::default();
        tree.set_roots(vec![repo.clone(), big.clone()]);
        for sub in ["crates", ".agent", "scripts", "poc"] {
            tree.toggle_dir(&repo.join(sub));
        }
        let t1 = Instant::now();
        let changed = tree.refresh();
        eprintln!(
            "[perf] refresh（root 2 + 展開 4）: {:?} changed={}",
            t1.elapsed(),
            changed
        );
        let t2 = Instant::now();
        let rows = tree.rows();
        eprintln!("[perf] rows(): {:?}（{} 行）", t2.elapsed(), rows.len());
        remove_temp_dir(&big);
    }

    /// #550: ドット始まりは既定で隠れ、トグルで出る。ルート見出し自身は隠さない
    #[test]
    fn ドット始まりは既定で隠れトグルで出る() {
        let dir = fixture("t5");
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        std::fs::write(dir.join(".env"), "x").unwrap();
        let mut tree = FileTree::default();
        tree.set_roots(vec![dir.clone()]);
        assert!(!tree.show_hidden(), "既定は非表示");
        let visible: Vec<String> = tree.rows().iter().map(|r| r.entry.name.clone()).collect();
        assert!(!visible.iter().any(|n| n == ".git"));
        assert!(!visible.iter().any(|n| n == ".env"));
        assert!(visible.iter().any(|n| n == "README.md"));

        assert!(tree.set_show_hidden(true));
        assert!(!tree.set_show_hidden(true), "同値なら変化なし");
        let visible: Vec<String> = tree.rows().iter().map(|r| r.entry.name.clone()).collect();
        assert!(visible.iter().any(|n| n == ".git"));
        assert!(visible.iter().any(|n| n == ".env"));

        // ルート見出しがドットディレクトリでも隠さない（明示的に開いたフォルダ）
        tree.set_show_hidden(false);
        tree.set_roots(vec![dir.join(".git")]);
        let rows = tree.rows();
        assert_eq!(rows.len(), 1);
        assert!(rows[0].root && rows[0].entry.name == ".git");
        remove_temp_dir(&dir);
    }

    /// #550: 増えたルートは自動展開のうえ先頭へ。同じ集合の再同期で順序は暴れない
    #[test]
    fn 新しいルートは先頭に出て順序は安定する() {
        let dir = fixture("t6");
        let home = dir.join("docs");
        let project = dir.join("src");
        let mut tree = FileTree::default();
        tree.set_roots(vec![home.clone()]);
        assert_eq!(tree.roots(), std::slice::from_ref(&home));

        // ペインが cd して project が増える → 先頭へ
        assert!(tree.set_roots(vec![home.clone(), project.clone()]));
        assert_eq!(tree.roots(), &[project.clone(), home.clone()]);
        // 自動展開されているので中身が見える
        assert!(tree.rows().iter().any(|r| r.entry.name == "main.rs"));
        // 同じ集合をポーリングで再同期しても並びは変わらない
        assert!(!tree.set_roots(vec![home.clone(), project.clone()]));
        assert_eq!(tree.roots(), &[project, home]);
        remove_temp_dir(&dir);
    }

    /// #559: 新規作成の直後に該当ディレクトリだけ読み直せる
    #[test]
    fn refresh_dirは対象ディレクトリだけ読み直す() {
        let dir = fixture("t7");
        let mut tree = FileTree::default();
        tree.set_roots(vec![dir.clone()]);
        std::fs::write(dir.join("created.txt"), "x").unwrap();
        assert!(!tree.rows().iter().any(|r| r.entry.name == "created.txt"));
        tree.refresh_dir(&dir);
        assert!(tree.rows().iter().any(|r| r.entry.name == "created.txt"));
        // 未知のディレクトリは無視（キャッシュを作らない）
        tree.refresh_dir(Path::new("/no/such/dir"));
        remove_temp_dir(&dir);
    }

    /// #1402 で期待値を更新: 以前は「見出しだけ残り中身は空」= 読めなかったことが
    /// **どこにも出ない**（空フォルダと同じ見え方）を固定していた。読めない理由を
    /// 行として出す（`conventions.md`「弾いたら黙って捨てない」）
    #[test]
    fn 読めないルートは見出しの下に理由の行が出る() {
        let mut tree = FileTree::default();
        tree.set_roots(vec![PathBuf::from("/no/such/dir")]);
        let rows = tree.rows();
        assert_eq!(rows.len(), 2, "理由の行が出ていない（#1402 の症状）");
        assert!(rows[0].root);
        let note = &rows[1];
        assert_eq!(note.depth, 1, "ルートの中身と同じ深さに出る");
        assert!(
            note.entry.name.is_empty(),
            "押せる行のように見えてはいけない"
        );
        assert!(!note.entry.is_dir && !note.root && note.remote.is_none());
        match &note.note {
            // 理由（OS のメッセージ）をそのまま載せる = 空と区別が付く
            Some(RowNote::Error(report)) => assert!(!report.is_empty(), "理由が空"),
            other => panic!("読めない理由が Error 行として出ていない: {other:?}"),
        }
        // **消滅した**パスは次の refresh で対象外として畳まれる（従来どおり）。
        // 読めるのに読めない状態が**続く**ケース（権限なし）の持続は
        // `読めないディレクトリは空と区別して行に出る` が見ている
        tree.refresh();
        assert_eq!(
            tree.rows().len(),
            1,
            "消滅したルートは畳まれる（従来の挙動）"
        );
    }

    /// #1402 (a): 上限を超えたディレクトリは「何件まで表示 / 全何件」を行で申告する。
    ///
    /// Issue の実測（560 件 → 行 501 / note 0）を回帰として固定する。
    /// 切り詰めたぶんは行として存在しないので、**申告が無いと
    /// 「そのファイルが存在しない」と読める**のがこのバグの実害
    #[test]
    fn 上限を超えたディレクトリは切り詰めを行で申告する() {
        let total = MAX_ENTRIES + 60;
        let scratch = empty_scratch("trunc-over");
        let dir = scratch.path();
        for i in 0..total {
            std::fs::write(dir.join(format!("f-{i:05}.txt")), "x").unwrap();
        }
        let mut tree = FileTree::default();
        tree.set_roots(vec![dir.to_path_buf()]);
        let rows = tree.rows();
        assert_eq!(
            rows.len(),
            1 + MAX_ENTRIES + 1,
            "ルート見出し + 上限ぶん + 申告の 1 行にならない（#1402）"
        );
        let note = rows.last().expect("行がある");
        assert_eq!(
            note.note,
            Some(RowNote::Truncated {
                shown: MAX_ENTRIES,
                total
            }),
            "切り詰めの申告が最後の行に無い"
        );
        assert_eq!(note.depth, 1, "中身と同じ深さに出る");
        assert!(note.entry.name.is_empty() && !note.entry.is_dir);
        // 501 件目以降は行として存在しない（= だから申告が要る）
        let dropped = format!("f-{MAX_ENTRIES:05}.txt");
        assert!(
            !rows.iter().any(|r| r.entry.name == dropped),
            "{dropped} が行として出ている（前提が崩れている）"
        );
        // 画面に出る文言（`render_note_row` が引くのと同じ関数）に総数が載る。
        // 数字は日英どちらの文言にも入るので言語を固定せずに見られる
        let text = crate::ui_text::sidebar::note_truncated(MAX_ENTRIES, total);
        assert!(
            text.contains(&total.to_string()) && text.contains(&MAX_ENTRIES.to_string()),
            "件数が文言に出ていない: {text}"
        );
    }

    /// #1402 (c) + 受け入れ条件 5: 申告の有無は上限ちょうどで切り替わり、
    /// **CLI / MCP（`tree git-status` の `truncated`）と同じ判断**から出る。
    ///
    /// 画面だけが黙る / 画面だけが申告する、のどちらへも倒れないことを
    /// 境界の 3 点（上限 -1 / ちょうど / +1）で固定する
    #[test]
    fn 切り詰めの申告は上限ちょうどでは出ない() {
        for count in [MAX_ENTRIES - 1, MAX_ENTRIES, MAX_ENTRIES + 1] {
            let scratch = empty_scratch(&format!("trunc-{count}"));
            let dir = scratch.path();
            for i in 0..count {
                std::fs::write(dir.join(format!("f-{i:05}.txt")), "x").unwrap();
            }
            let mut tree = FileTree::default();
            tree.set_roots(vec![dir.to_path_buf()]);
            let rows = tree.rows();
            let notes: Vec<&Row> = rows.iter().filter(|r| r.note.is_some()).collect();
            // 機械可読側と同じ 1 実装が出す答え
            let expected = Truncation::new(count, MAX_ENTRIES).truncated();
            assert_eq!(
                !notes.is_empty(),
                expected,
                "{count} 件のとき画面の申告と `Truncation::truncated()` が食い違う"
            );
            assert_eq!(
                rows.len(),
                1 + count.min(MAX_ENTRIES) + usize::from(expected),
                "{count} 件のときの行数"
            );
        }
    }

    /// #1402: サブディレクトリが上限ぶんあるディレクトリでは**ファイルが 1 行も出ない**
    /// （並びがディレクトリ先なので切り詰めがファイルを丸ごと食う）。
    /// この形が申告なしで起きるのが Issue の壊れるシナリオそのもの
    #[test]
    fn ディレクトリが上限ぶんあるとファイルは出ないが申告は出る() {
        let scratch = empty_scratch("trunc-dirs");
        let dir = scratch.path();
        for i in 0..MAX_ENTRIES {
            std::fs::create_dir(dir.join(format!("d-{i:05}"))).unwrap();
        }
        std::fs::write(dir.join("only-file.txt"), "x").unwrap();
        let mut tree = FileTree::default();
        tree.set_roots(vec![dir.to_path_buf()]);
        let rows = tree.rows();
        assert!(
            !rows.iter().any(|r| r.entry.name == "only-file.txt"),
            "前提（ディレクトリ先の並びで切り詰める）が変わっている"
        );
        assert_eq!(
            rows.last().and_then(|r| r.note.clone()),
            Some(RowNote::Truncated {
                shown: MAX_ENTRIES,
                total: MAX_ENTRIES + 1
            }),
            "ファイルが消えたのに申告が無い（#1402 の壊れるシナリオ）"
        );
    }

    /// #1402 (b): 読めないディレクトリ（権限なし）が**空ディレクトリと区別**して見える。
    ///
    /// 空は行なし・読めないは理由の行。`chmod` は unix 限定なので cfg で囲む
    /// （消滅したパスでの経路は `読めないルートは見出しの下に理由の行が出る` が
    /// 両 OS で見ている）
    #[cfg(unix)]
    #[test]
    fn 読めないディレクトリは空と区別して行に出る() {
        use std::os::unix::fs::PermissionsExt;

        /// 0o000 にしたディレクトリを**必ず**読める形へ戻す器（`Drop` は panic の
        /// 巻き戻しでも走る）。戻さないと使い捨ての親ごと消せない
        /// （`remove_dir_all` が中を読めずに失敗して残骸になる）
        struct Unlocked(PathBuf);
        impl Drop for Unlocked {
            fn drop(&mut self) {
                let _ = std::fs::set_permissions(&self.0, std::fs::Permissions::from_mode(0o755));
            }
        }

        let scratch = empty_scratch("unreadable");
        let dir = scratch.path();
        let empty = dir.join("empty");
        let locked = dir.join("locked");
        std::fs::create_dir(&empty).unwrap();
        std::fs::create_dir(&locked).unwrap();
        std::fs::write(locked.join("hidden-by-permission.txt"), "x").unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
        let _unlock = Unlocked(locked.clone());

        let mut tree = FileTree::default();
        tree.set_roots(vec![dir.to_path_buf()]);
        tree.expand_dir(&empty);
        tree.expand_dir(&locked);
        let rows = tree.rows();
        let note_of = |parent: &Path| -> Option<RowNote> {
            rows.iter()
                .find(|r| r.note.is_some() && r.entry.path == parent)
                .and_then(|r| r.note.clone())
        };
        // 空ディレクトリには何も足さない（見え方を変えない）
        assert_eq!(note_of(&empty), None, "空ディレクトリに行が増えている");
        match note_of(&locked) {
            Some(RowNote::Error(report)) => assert!(
                !report.is_empty(),
                "読めない理由が空（空ディレクトリと区別が付かない）"
            ),
            other => panic!("読めないディレクトリが Error 行にならない: {other:?}"),
        }
        // 中身は出ていない（出せないから理由を出している）
        assert!(!rows
            .iter()
            .any(|r| r.entry.name == "hidden-by-permission.txt"));
    }

    /// #1009: git の状態は**ファイル行だけでなくディレクトリ行とルート見出し行**にも載る。
    /// 折りたたんだままのディレクトリにも件数が出る（中身を読まずに色を付けられる）
    fn git_map(root: &Path, entries: &[(&str, char, char)]) -> TreeGitMap {
        let mut map = TreeGitMap::default();
        map.merge_repo(
            root,
            &tako_core::git::GitStatus {
                branch: "main".into(),
                upstream: String::new(),
                entries: entries
                    .iter()
                    .map(|(path, index, worktree)| tako_core::git::GitStatusEntry {
                        path: (*path).to_string(),
                        index: *index,
                        worktree: *worktree,
                    })
                    .collect(),
            },
        );
        map
    }

    #[test]
    fn git状態はファイルとディレクトリとルート行に載る() {
        let dir = fixture("t8");
        let mut tree = FileTree::default();
        tree.set_roots(vec![dir.clone()]);
        assert!(tree.apply_git_status(git_map(
            &dir,
            &[("README.md", '.', 'M'), ("src/main.rs", '?', '?')]
        )));
        // 同じ内容を入れ直しても「変化なし」= 無駄な再描画を起こさない
        assert!(!tree.apply_git_status(git_map(
            &dir,
            &[("README.md", '.', 'M'), ("src/main.rs", '?', '?')]
        )));

        let rows = tree.rows();
        let badge = |name: &str| {
            rows.iter()
                .find(|r| r.entry.name == name)
                .and_then(|r| r.git_status)
                .map(|s| s.badge())
        };
        assert_eq!(badge("README.md").as_deref(), Some("M"));
        // 折りたたまれた src にも配下 1 件が出る
        assert_eq!(badge("src").as_deref(), Some("1"));
        // ルート見出し行は配下 2 件
        let root_row = rows.iter().find(|r| r.root).unwrap();
        assert_eq!(root_row.git_status.map(|s| s.badge()).as_deref(), Some("2"));
        // 変更が無いファイルには何も付かない
        assert!(rows
            .iter()
            .find(|r| r.entry.name == "docs")
            .unwrap()
            .git_status
            .is_none());
        assert_eq!(
            tree.git_status_of(&dir.join("README.md")).map(|s| s.state),
            Some(TreeGitState::Modified)
        );
        remove_temp_dir(&dir);
    }

    #[test]
    fn git管理外のツリーには何も付かない() {
        let dir = fixture("t9");
        let mut tree = FileTree::default();
        tree.set_roots(vec![dir.clone()]);
        // scan は git 管理外のルートを黙って飛ばす = 空の表になる
        assert!(tako_core::git_tree::scan(std::slice::from_ref(&dir)).is_empty());
        assert!(tree.rows().iter().all(|r| r.git_status.is_none()));
        remove_temp_dir(&dir);
    }

    // --- シンボリックリンク（#1398） ---------------------------------------
    //
    // unix 限定。Windows のジャンクション / シンボリックリンクは**作成に権限が要る**
    // ので実機での確認が別途必要（判定側の `entry_is_dir` は cfg で分岐しない =
    // `std::fs::metadata` が両 OS で reparse point を辿る）

    /// リンクを 1 本張る。張れない環境では**飛ばさずに落とす**
    /// （静かに skip すると穴が残る）
    #[cfg(unix)]
    fn symlink(target: &Path, link: &Path) {
        std::os::unix::fs::symlink(target, link).unwrap_or_else(|e| {
            panic!(
                "シンボリックリンクを作れない {} -> {}: {e}",
                link.display(),
                target.display()
            )
        });
    }

    /// 名前で 1 行引く
    #[cfg(unix)]
    fn row_of(tree: &mut FileTree, name: &str) -> Option<Row> {
        tree.rows().iter().find(|r| r.entry.name == name).cloned()
    }

    /// (name, depth, is_dir, note の有無) に写す（打ち切りの検査用）
    #[cfg(unix)]
    fn shape(tree: &mut FileTree) -> Vec<(String, usize, bool, bool)> {
        tree.rows()
            .iter()
            .map(|r| {
                (
                    r.entry.name.clone(),
                    r.depth,
                    r.entry.is_dir,
                    r.note.is_some(),
                )
            })
            .collect()
    }

    /// #1398 の本体: ディレクトリへのリンクは**展開できるディレクトリ行**になる
    #[cfg(unix)]
    #[test]
    fn ディレクトリへのシンボリックリンクは展開できる() {
        let dir = fixture("sym-dir");
        std::fs::create_dir_all(dir.join("real")).unwrap();
        std::fs::write(dir.join("real/inner.txt"), "x").unwrap();
        symlink(&dir.join("real"), &dir.join("link"));

        let mut tree = FileTree::default();
        tree.set_roots(vec![dir.clone()]);
        let link = row_of(&mut tree, "link").expect("link 行がある");
        assert!(
            link.entry.is_dir,
            "ディレクトリへのシンボリックリンクがファイル扱いになっている（#1398）: {:?}",
            shape(&mut tree)
        );

        let before = tree.rows().len();
        tree.toggle_dir(&dir.join("link"));
        let after = tree.rows();
        assert!(
            after.len() > before,
            "link を展開しても行が増えない（before={before} after={}）: {:?}",
            after.len(),
            shape(&mut tree)
        );
        assert!(
            after
                .iter()
                .any(|r| r.entry.name == "inner.txt" && r.depth == 2),
            "リンク先の中身が出ていない: {:?}",
            shape(&mut tree)
        );
        remove_temp_dir(&dir);
    }

    /// ファイルへのリンクは**ファイル行のまま**（`OpenFile` で開ける側）
    #[cfg(unix)]
    #[test]
    fn ファイルへのシンボリックリンクはファイル行のまま() {
        let dir = fixture("sym-file");
        symlink(&dir.join("README.md"), &dir.join("alias.md"));

        let mut tree = FileTree::default();
        tree.set_roots(vec![dir.clone()]);
        let row = row_of(&mut tree, "alias.md").expect("alias.md 行がある");
        assert!(!row.entry.is_dir, "ファイルへのリンクがディレクトリ扱い");
        // 開く側（dispatch::OpenFile の `Path::is_file()`）と判断が一致している
        assert!(dir.join("alias.md").is_file());
        remove_temp_dir(&dir);
    }

    /// 切れたリンクはファイル行として残す（中身を出せないものをディレクトリに
    /// 見せると「展開しても空」= 理由の出ない静かな失敗になる）
    #[cfg(unix)]
    #[test]
    fn 切れたシンボリックリンクはファイル行として残る() {
        let dir = fixture("sym-broken");
        symlink(&dir.join("no-such-target"), &dir.join("broken"));

        let mut tree = FileTree::default();
        tree.set_roots(vec![dir.clone()]);
        let row = row_of(&mut tree, "broken").expect("broken 行がある");
        assert!(!row.entry.is_dir, "辿れないリンクがディレクトリ扱い");
        remove_temp_dir(&dir);
    }

    /// 相対パスのリンクも辿る（`link -> real` を相対で張る形）
    #[cfg(unix)]
    #[test]
    fn 相対パスのシンボリックリンクも辿る() {
        let dir = fixture("sym-rel");
        std::fs::create_dir_all(dir.join("real")).unwrap();
        std::fs::write(dir.join("real/inner.txt"), "x").unwrap();
        symlink(Path::new("real"), &dir.join("rel"));

        let mut tree = FileTree::default();
        tree.set_roots(vec![dir.clone()]);
        assert!(
            row_of(&mut tree, "rel").expect("rel 行がある").entry.is_dir,
            "相対パスのリンクを辿れていない: {:?}",
            shape(&mut tree)
        );
        remove_temp_dir(&dir);
    }

    /// 相互に指し合うリンク（`a -> b` / `b -> a`）は辿れない（ELOOP）ので
    /// ファイル行になり、展開もされない
    #[cfg(unix)]
    #[test]
    fn 相互に指し合うシンボリックリンクは展開されない() {
        let dir = fixture("sym-eloop");
        symlink(&dir.join("b"), &dir.join("a"));
        symlink(&dir.join("a"), &dir.join("b"));

        let mut tree = FileTree::default();
        tree.set_roots(vec![dir.clone()]);
        for name in ["a", "b"] {
            assert!(
                !row_of(&mut tree, name).expect("行がある").entry.is_dir,
                "循環リンク {name} がディレクトリ扱い（辿れないのに展開できる形）"
            );
        }
        // 展開を指示しても増えない（有限時間で返る）
        tree.toggle_dir(&dir.join("a"));
        tree.toggle_dir(&dir.join("b"));
        assert!(
            tree.rows().iter().all(|r| r.depth <= 1),
            "循環リンクの配下が出ている: {:?}",
            shape(&mut tree)
        );
        remove_temp_dir(&dir);
    }

    /// 自分の祖先を指すリンク（`real/up -> ..`）は、展開しても**打ち切り行**で止まる
    #[cfg(unix)]
    #[test]
    fn 祖先を指すシンボリックリンクは打ち切り行で止まる() {
        let dir = fixture("sym-ancestor");
        std::fs::create_dir_all(dir.join("real")).unwrap();
        symlink(Path::new(".."), &dir.join("real/up"));

        let mut tree = FileTree::default();
        tree.set_roots(vec![dir.clone()]);
        tree.expand_dir(&dir.join("real"));
        let up = row_of(&mut tree, "up").expect("up 行がある");
        assert!(up.entry.is_dir, "`..` へのリンクがファイル扱い");

        tree.expand_dir(&dir.join("real/up"));
        let rows = tree.rows();
        let dump: Vec<(String, usize, bool, bool)> = rows
            .iter()
            .map(|r| {
                (
                    r.entry.name.clone(),
                    r.depth,
                    r.entry.is_dir,
                    r.note.is_some(),
                )
            })
            .collect();
        let note = rows
            .iter()
            .find(|r| matches!(r.note, Some(RowNote::Error(_))))
            .unwrap_or_else(|| panic!("循環の打ち切りが行として出ていない: {dump:?}"));
        assert_eq!(note.depth, 3, "打ち切り行の深さ");
        // 打ち切ったので祖先の中身（README.md 等）が 2 周目で出てこない
        let readme_depths: Vec<usize> = rows
            .iter()
            .filter(|r| r.entry.name == "README.md")
            .map(|r| r.depth)
            .collect();
        assert_eq!(readme_depths, vec![1], "祖先の中身が 2 周目に出ている");
        remove_temp_dir(&dir);
    }

    /// 2 つの実ディレクトリを相互に指すリンク（`da/to_b -> ../db` /
    /// `db/to_a -> ../da`）で、往復の 2 周目が打ち切られる
    #[cfg(unix)]
    #[test]
    fn 実ディレクトリ間を往復するリンクは打ち切られる() {
        let dir = fixture("sym-mutual");
        std::fs::create_dir_all(dir.join("da")).unwrap();
        std::fs::create_dir_all(dir.join("db")).unwrap();
        symlink(Path::new("../db"), &dir.join("da/to_b"));
        symlink(Path::new("../da"), &dir.join("db/to_a"));

        let mut tree = FileTree::default();
        tree.set_roots(vec![dir.clone()]);
        tree.expand_dir(&dir.join("da"));
        tree.expand_dir(&dir.join("da/to_b"));
        tree.expand_dir(&dir.join("da/to_b/to_a"));
        let rows = tree.rows();
        assert!(
            rows.iter()
                .any(|r| matches!(r.note, Some(RowNote::Error(_)))),
            "往復の 2 周目が打ち切られていない"
        );
        // da は 1 回だけ（2 周目は打ち切り行に置き換わる）
        assert_eq!(
            rows.iter().filter(|r| r.entry.name == "to_a").count(),
            1,
            "to_a が 2 回以上出ている: {:?}",
            rows.iter()
                .map(|r| (r.entry.name.clone(), r.depth))
                .collect::<Vec<_>>()
        );
        remove_temp_dir(&dir);
    }

    /// git のしるし（#1009）はリンク行で壊れない。
    ///
    /// **既知の限界**: リンク経由のパス（`link/inner.txt`）にはしるしが付かない。
    /// `git status` は実体側のパス（`real/inner.txt`）で報告するので表に無く、
    /// git 自身の見え方（`git status` の出力）と揃っている。実体側の行・
    /// ディレクトリ伝播・ルート行は従来どおり
    #[cfg(unix)]
    #[test]
    fn git状態はリンク行で壊れない() {
        let dir = fixture("sym-git");
        std::fs::create_dir_all(dir.join("real")).unwrap();
        std::fs::write(dir.join("real/inner.txt"), "x").unwrap();
        symlink(&dir.join("real"), &dir.join("link"));

        let mut tree = FileTree::default();
        tree.set_roots(vec![dir.clone()]);
        assert!(tree.apply_git_status(git_map(&dir, &[("real/inner.txt", '.', 'M')])));
        tree.expand_dir(&dir.join("real"));
        tree.expand_dir(&dir.join("link"));

        let rows = tree.rows();
        let badge = |path: PathBuf| {
            rows.iter()
                .find(|r| r.entry.path == path)
                .unwrap_or_else(|| panic!("{} の行が無い", path.display()))
                .git_status
                .map(|s| s.badge())
        };
        // 実体側は従来どおり（ファイル = M / ディレクトリ = 配下 1 件）
        assert_eq!(badge(dir.join("real/inner.txt")).as_deref(), Some("M"));
        assert_eq!(badge(dir.join("real")).as_deref(), Some("1"));
        assert_eq!(badge(dir.clone()).as_deref(), Some("1"));
        // リンク経由の行は付かない（git が報告しないパスなので伝播もしない）
        assert_eq!(badge(dir.join("link")), None);
        assert_eq!(badge(dir.join("link/inner.txt")), None);
        remove_temp_dir(&dir);
    }

    /// 段 3 との整合: ディレクトリへのリンクは**開く経路へ行かない**。
    ///
    /// サイドバーのクリック分岐は `is_dir` を見て toggle / open を分けるので、
    /// `is_dir = true` になった行は `OpenFile`（`Path::is_file()` で弾かれる側）へ
    /// 到達しない。⌘+クリック / `tako file open-in-tako` が引く振り分け表
    /// （`open_plan::route`）も同じ答えを返す = GUI / CLI / MCP で意味が揃う
    #[cfg(unix)]
    #[test]
    fn ディレクトリへのリンクは開く経路へ行かない() {
        use tako_core::open_plan::{route, OpenRoute, PreviewRoute};

        let dir = fixture("sym-route");
        std::fs::create_dir_all(dir.join("real")).unwrap();
        symlink(&dir.join("real"), &dir.join("link"));
        symlink(&dir.join("README.md"), &dir.join("alias.md"));

        let entries = read_dir_sorted(&dir).entries;
        let pick = |name: &str| {
            entries
                .iter()
                .find(|e| e.name == name)
                .unwrap_or_else(|| panic!("{name} が無い"))
        };
        let link = pick("link");
        assert!(link.is_dir);
        assert_eq!(
            route(&link.path, link.is_dir),
            OpenRoute::Terminal,
            "ディレクトリへのリンクがプレビュー（= OpenFile）側へ振られている"
        );
        // ファイルへのリンクは従来どおりプレビューへ（`OpenFile` が開ける）
        let alias = pick("alias.md");
        assert!(!alias.is_dir);
        assert_eq!(
            route(&alias.path, alias.is_dir),
            OpenRoute::Preview(PreviewRoute::Markdown)
        );
        assert!(alias.path.is_file(), "開く側の `is_file()` と判断が一致");
        remove_temp_dir(&dir);
    }

    /// 500 件超（`MAX_ENTRIES`）にリンクが混ざっても切り詰めの挙動は変わらない
    /// （切り詰めそのものは #1402 の範囲なので触らない）
    #[cfg(unix)]
    #[test]
    fn 大量のリンクが混ざっても切り詰めは変わらない() {
        let dir = fixture("sym-many");
        std::fs::create_dir_all(dir.join("real")).unwrap();
        for i in 0..600 {
            symlink(&dir.join("real"), &dir.join(format!("l{i:04}")));
        }
        let listing = read_dir_sorted(&dir);
        assert_eq!(
            listing.entries.len(),
            MAX_ENTRIES,
            "切り詰めの上限が変わっている"
        );
        assert!(
            listing.entries.iter().all(|e| e.is_dir),
            "ディレクトリ先の並びにファイル行が混ざっている"
        );
        // #1402: 切り詰めたことは黙らない（リンクが混ざっても総数は実数のまま）
        let on_disk = std::fs::read_dir(&dir).unwrap().count();
        assert!(listing.truncation.truncated());
        assert_eq!(listing.truncation.total, on_disk, "総数が実数と一致しない");
        remove_temp_dir(&dir);
    }
}
