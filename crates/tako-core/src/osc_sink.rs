//! 器が OSC を素通ししない環境向けの、シェル統合の側路（#766）
//!
//! ## なぜ要るか
//!
//! シェル統合（OSC 7 / 133。FR-2.4.1）は「ペインのシェルが出したバイト列を tako が
//! PTY 読み取りで拾う」形で成立している。macOS の tmux は `allow-passthrough on` +
//! DCS 包み（`ESC P tmux; … ESC \`）でこれを通すので、器があっても届く。
//!
//! **psmux は通さない**。実測（#766 起票時）で素の OSC・DCS（ESC 二重化あり / なし）の
//! 3 形すべてが外へ出ず、同時に流した平文だけが届いた。upstream のソースを見ると理由が
//! はっきりする（2026-08-21 時点の master / v3.3.8）:
//!
//! - `allow-passthrough` は**選択肢として存在するだけ**。`src/server/options.rs` の
//!   get / set と config パースにしか現れず、**値を読んで素通しする側が無い**
//! - `Ptmux`（DCS の tmux 形式）の実装は**リポジトリに 1 箇所も無い**
//! - psmux は「パースして画面モデルへ落とし、クライアントへ描き直す」多重化器で、
//!   OSC 7 / 133 / 633 / 1337 は**自分で消費する**（`#{pane_current_path}` /
//!   `#{pane_current_command}` の材料にしている）。画面モデルに置き場の無いバイト列は
//!   原理的にクライアントへ出ない = **私用 OSC を使う抜け道も無い**
//!
//! つまり器の側で直る話ではない（upstream の新機能が要る）。tako が今できるのは
//! **同じバイト列を別の経路で運ぶ**ことだけ。
//!
//! ## 運ぶのは「解釈済みの状態」ではなく OSC バイト列そのまま
//!
//! ファイルへ書くのは統合スクリプトが出すはずだった **OSC のバイト列そのもの**で、
//! 解釈は PTY 経路と同じ [`crate::osc_tap`] に通す。状態機械が 1 本のままになるので
//! 「macOS では Failed(3) だが Windows では Idle」のような分岐が構造的に起きない。
//!
//! ## 待ち合わせ先は「パス」だけでは足りない（#1199）
//!
//! 器へ渡すペイン固有の env は、**器の中の全シェルへ配られる**。psmux の `-e` は
//! セッション単位ではなく**サーバーのグローバル環境**へ入る（実測: `show-environment -g`
//! に `TAKO_PANE_ID` / `TAKO_OSC_SINK` が並ぶ）。さらに psmux は**プリウォーム済みの
//! シェルの一団**（`__warm__`）を持っており、その環境も同じ表から作られる。
//!
//! つまり「パスだけの待ち合わせ」では、**ペインのシェル以外もそのペインの側路へ書ける**。
//! ウォームプールのシェルは cwd がユーザーのホームなので、tako がペインをリサイズする
//! たび（psmux はプールもクライアントの寸法へ合わせる）にプロンプトを描き直し、
//! `OSC 7 <ホーム>` を書き込む = **ペインの cwd がホームへ巻き戻る**（#1199 の実測。
//! ペインのシェルだけ側路を止めても、リサイズごとにファイルが復活した）。
//!
//! なので待ち合わせ先に**書き手の同一性**を載せる: 統合スクリプトは
//! [`resolve_writer_path`] で `<pane>.osc` を `<pane>@p<自分の pid>.osc` へ解決し、
//! tako は**そのペインのシェルの pid**（器ありは `#{pane_pid}` / 器なしは PTY 直下の子）
//! ぶんだけを読む。解決は**冪等**で、解決後の値を `TAKO_OSC_SINK` へ書き戻すので、
//! ペインの中で起こした子シェル（入れ子の pwsh・ユーザー自身の tmux）は
//! **同じファイルを共有**して従来どおり cwd を更新できる。
//!
//! 素の `<pane>.osc` も読み続ける（[`SinkReader`]）。器を跨いで生き延びたペイン
//! （`survives_app_exit`）のシェルは**更新前のスクリプト**を読み込んだままなので、
//! そこだけは解決前のパスへ書く。次にそのシェルが起き直れば新しい規則へ乗る
//!
//! 器の中の別のシェルのぶんは溜まる（psmux はリサイズのたびにプールを補充するので、
//! 実測でリサイズ 8 回 = 12 本）。書き手が確定したら
//! [`SinkReader::prune_foreign`] がそれ以外を捨てる
//!
//! ## 書き込みと読み取りの取り決め
//!
//! - 書き手はペインの中のシェル 1 個だけ。1 回のプロンプト（= `D` + `A` + cwd の束）を
//!   **まとめて 1 回で上書き**する（`.new` へ書いて rename = 差し替えは原子的）
//! - 読み手は tako の定期更新。**中身が前回と変わっていたら**そのまま
//!   [`crate::osc_tap`] へ通す。追記ではなく上書きなので、読み取りと書き込みが
//!   競合してもバイト列が混ざらない（ファイルは常に完全な 1 束を持つ）
//! - 同じ束が連続したときは 1 回しか通らない（例: `ls` を 2 回）。状態は同じ値へ
//!   遷移するだけなので実害が無く、追記方式の「読んで truncate する隙に書かれた分を
//!   落とす」窓を作らないほうを採った

use std::path::{Path, PathBuf};

/// ペインの中のシェルへ側路の書き先を教える環境変数。
///
/// これが設定されているときだけ統合スクリプトは側路へ書く（未設定なら従来どおり
/// コンソールへ OSC を出す）。器へは**ペイン固有の値**として渡す必要がある
/// （[`crate::backend::PANE_SCOPED_ENV`] / [`crate::backend::session_pinned_env`]）
pub const SINK_ENV: &str = "TAKO_OSC_SINK";

/// 側路のファイルを置くディレクトリ名（`<data_dir>/<この名前>/`）
const DIR: &str = "osc";

/// 書き手の pid を載せる区切り（`<pane>@p<pid>.osc`）。
///
/// `@` は Windows / POSIX のどちらでもファイル名に使える。数字だけの接尾辞にすると
/// ペイン ID との区別がつかないので `p` を前置する
const WRITER_MARK: &str = "@p";

/// ペインの側路ファイルのパス（純粋関数）。
///
/// **解決前**の待ち合わせ先で、統合スクリプトへ env で教える値でもある。
/// 実際に読む先は [`writer_sink_path`]（#1199）
pub fn sink_path(data_dir: &Path, pane_id: u64) -> PathBuf {
    data_dir.join(DIR).join(format!("{pane_id}.osc"))
}

/// **書き手を特定した**側路ファイルのパス（純粋関数。#1199）。
///
/// `writer_pid` は「そのペインのシェル」の pid（器ありは器が言う `#{pane_pid}`、
/// 器なしは PTY 直下の子）。器の中の別のシェル（psmux のウォームプール）は
/// 自分の pid でファイル名を作るので、ここへは絶対に当たらない
pub fn writer_sink_path(data_dir: &Path, pane_id: u64, writer_pid: u32) -> PathBuf {
    data_dir
        .join(DIR)
        .join(format!("{pane_id}{WRITER_MARK}{writer_pid}.osc"))
}

/// 統合スクリプトが行う待ち合わせ先の解決（#1199）。
///
/// **スクリプト側（`shell-integration/tako.ps1`）と同じ規則の正本**で、番犬テストが
/// 両方を突き合わせる。`env` に入っている解決前のパスへ自分の pid を載せる。
///
/// **冪等**であることが要件: 解決後の値は `TAKO_OSC_SINK` へ書き戻され、ペインの中で
/// 起こした子シェルがそれを継承する。そこで再解決すると子の pid が載って
/// ファイルが分かれ、tako が読まない場所へ書くことになる
pub fn resolve_writer_path(env_value: &str, pid: u32) -> String {
    if writer_pid_of(env_value).is_some() {
        return env_value.to_string();
    }
    let base = env_value.strip_suffix(".osc").unwrap_or(env_value);
    format!("{base}{WRITER_MARK}{pid}.osc")
}

/// 解決済みパスに載っている書き手の pid（載っていなければ `None`。純粋関数）。
///
/// 差し替え途中の `.new`（統合スクリプトは `.new` へ書いて rename する）も同じ
/// 書き手のものとして拾う。書き手が死んで取り残された `.new` が掃除から漏れないため
pub fn writer_pid_of(path: &str) -> Option<u32> {
    let path = path.strip_suffix(".new").unwrap_or(path);
    path.strip_suffix(".osc")?
        .rsplit_once(WRITER_MARK)?
        .1
        .parse()
        .ok()
}

/// 側路を使う準備をして書き先を返す。作れなければ `None`（統合が従来どおり
/// コンソールへ出すだけになる = 器の中では効かないが、壊れはしない）。
///
/// 前のペインの残骸は消す。ペイン ID は再起動をまたいで再利用されるので（#210）、
/// 残骸を残すと**前回の最後の状態**を今回の起動直後に食わせてしまう
pub fn prepare(data_dir: &Path, pane_id: u64) -> Option<PathBuf> {
    let path = sink_path(data_dir, pane_id);
    std::fs::create_dir_all(path.parent()?).ok()?;
    discard_all(data_dir, pane_id);
    Some(path)
}

/// そのペインの側路ファイルを**解決済みぶんまで**消す（#1199）。
///
/// 器のウォームプールのシェルも `<pane>@p<自分の pid>.osc` を作るので、
/// ペイン ID 単位で掃除しないと `<data_dir>/osc/` に残骸が積む
pub fn discard_all(data_dir: &Path, pane_id: u64) {
    let dir = data_dir.join(DIR);
    let _ = std::fs::remove_file(dir.join(format!("{pane_id}.osc")));
    let _ = std::fs::remove_file(dir.join(format!("{pane_id}.osc.new")));
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return;
    };
    let prefix = format!("{pane_id}{WRITER_MARK}");
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if name.starts_with(&prefix) && writer_pid_of(name).is_some() {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// 側路 1 本の読み取り状態。前回通したバイト列を覚えておき、変化したぶんだけ通す
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SinkCursor {
    last: Vec<u8>,
}

impl SinkCursor {
    /// ファイルを読み、前回と違えばそのバイト列を返す（同じ / 読めない場合は `None`）。
    ///
    /// 1 束は 100 バイト程度なので、ペインごと 1 tick に 1 回読んでも実質ゼロコスト。
    /// 上限を設けているのは、想定外に育ったファイル（tako が居ない間に書かれ続けた等）で
    /// メモリと解析時間を食わないため
    pub fn take_new(&mut self, path: &Path) -> Option<Vec<u8>> {
        const MAX: u64 = 64 * 1024;
        let len = std::fs::metadata(path).ok()?.len();
        if len == 0 || len > MAX {
            return None;
        }
        let bytes = std::fs::read(path).ok()?;
        if bytes == self.last {
            return None;
        }
        self.last = bytes.clone();
        Some(bytes)
    }
}

/// ペイン 1 つぶんの側路の読み口（#1199）。
///
/// 待ち合わせ先が 2 つある:
///
/// - `<pane>@p<pid>.osc` — **そのペインのシェル**（と、その中で起こした子シェル）。
///   `pid` が分かるまでは読めない（器へ `#{pane_pid}` を聞いてから張る）
/// - `<pane>.osc` — 解決前のパスへ書く書き手。更新前のスクリプトを読み込んだままの
///   シェル（器を跨いで生き延びたペイン）だけがここへ来る
///
/// 両方に新しい束が来ていたら**解決済みを後に**通す（そのペインのシェルの言うことを
/// 最後に反映する）
#[derive(Debug)]
pub struct SinkReader {
    dir: PathBuf,
    pane_id: u64,
    legacy: SinkCursor,
    writer: Option<(u32, SinkCursor)>,
    /// 器の中の別のシェルが作った待ち合わせ先を掃除した最後の時刻（#1199）
    last_prune: Option<std::time::Instant>,
}

impl SinkReader {
    pub fn new(data_dir: &Path, pane_id: u64) -> Self {
        Self {
            dir: data_dir.to_path_buf(),
            pane_id,
            legacy: SinkCursor::default(),
            writer: None,
            last_prune: None,
        }
    }

    /// そのペインのシェルの pid が分かったら張る（同じ pid の再指定は無視 =
    /// 読み取り位置を巻き戻さない）
    pub fn set_writer_pid(&mut self, pid: u32) {
        if self.writer.as_ref().map(|(p, _)| *p) == Some(pid) {
            return;
        }
        self.writer = Some((pid, SinkCursor::default()));
    }

    /// 張ってある書き手の pid
    pub fn writer_pid(&self) -> Option<u32> {
        self.writer.as_ref().map(|(p, _)| *p)
    }

    /// 解決前の待ち合わせ先（統合スクリプトへ env で教える値）
    pub fn legacy_path(&self) -> PathBuf {
        sink_path(&self.dir, self.pane_id)
    }

    /// 新しく来た束を古い順に返す（無ければ空）
    pub fn take_new(&mut self) -> Vec<Vec<u8>> {
        let mut out = Vec::new();
        let legacy = sink_path(&self.dir, self.pane_id);
        if let Some(bytes) = self.legacy.take_new(&legacy) {
            out.push(bytes);
        }
        if let Some((pid, cursor)) = self.writer.as_mut() {
            let path = writer_sink_path(&self.dir, self.pane_id, *pid);
            if let Some(bytes) = cursor.take_new(&path) {
                out.push(bytes);
            }
        }
        self.prune_foreign_on_schedule();
        out
    }

    /// 器の中の別のシェルが作った待ち合わせ先を捨てる（#1199）。
    ///
    /// psmux はプリウォーム済みのシェルを**リサイズのたびに補充する**ので、
    /// 補充されたシェルの数だけ `<pane>@p<そのシェルの pid>.osc` が増える
    /// （実測: リサイズ 8 回で 12 本）。放っておくと `<data_dir>/osc/` が育ち続けるので、
    /// 書き手が確定したあとは**その 1 本以外を捨てる**。
    /// 捨てた先へ相手が書き直しても、また捨てるだけで実害が無い
    /// （読むのは書き手の 1 本だけなので、消し損ねても誤反映は起きない）
    pub fn prune_foreign(&self) {
        let Some(keep) = self.writer_pid() else {
            return; // 書き手が確定するまでは消さない（ペイン自身のぶんを消しうる）
        };
        let Ok(entries) = std::fs::read_dir(self.dir.join(DIR)) else {
            return;
        };
        let prefix = format!("{}{WRITER_MARK}", self.pane_id);
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            if !name.starts_with(&prefix) {
                continue;
            }
            if writer_pid_of(name).is_some_and(|pid| pid != keep) {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }

    /// 掃除の間隔（ペインごと）。リサイズ 1 回で 1〜2 本増える程度なので、
    /// 定期更新（2 秒 tick）のたびに `read_dir` する必要は無い
    const PRUNE_INTERVAL: std::time::Duration = std::time::Duration::from_secs(30);

    fn prune_foreign_on_schedule(&mut self) {
        let now = std::time::Instant::now();
        if self
            .last_prune
            .is_some_and(|at| now.duration_since(at) < Self::PRUNE_INTERVAL)
        {
            return;
        }
        self.last_prune = Some(now);
        self.prune_foreign();
    }

    /// ペインを閉じたときの後始末
    pub fn discard(&self) {
        discard_all(&self.dir, self.pane_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("tako-osc-sink-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn 側路のパスはペインごとに分かれる() {
        let base = Path::new("/data");
        assert_eq!(
            sink_path(base, 3),
            Path::new("/data").join("osc").join("3.osc")
        );
        assert_ne!(sink_path(base, 3), sink_path(base, 4));
    }

    #[test]
    fn prepareはディレクトリを作り前のペインの残骸を消す() {
        let dir = temp_dir("prepare");
        let path = sink_path(&dir, 7);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"\x1b]133;D;9\x07").unwrap();

        let got = prepare(&dir, 7).expect("準備できる");
        assert_eq!(got, path);
        assert!(
            !path.exists(),
            "残骸が消えていない（前回の最後の状態を食わせてしまう）"
        );
    }

    #[test]
    fn 変化したときだけ通す() {
        let dir = temp_dir("cursor");
        let path = prepare(&dir, 1).unwrap();
        let mut cursor = SinkCursor::default();

        assert_eq!(
            cursor.take_new(&path),
            None,
            "ファイルが無ければ何も通さない"
        );

        std::fs::write(&path, b"\x1b]133;A\x07").unwrap();
        assert_eq!(
            cursor.take_new(&path).as_deref(),
            Some(&b"\x1b]133;A\x07"[..])
        );
        assert_eq!(cursor.take_new(&path), None, "同じ内容は 2 回通さない");

        std::fs::write(&path, b"\x1b]133;D;3\x07").unwrap();
        assert_eq!(
            cursor.take_new(&path).as_deref(),
            Some(&b"\x1b]133;D;3\x07"[..])
        );
    }

    #[test]
    fn 空と大きすぎるファイルは通さない() {
        let dir = temp_dir("guard");
        let path = prepare(&dir, 2).unwrap();
        let mut cursor = SinkCursor::default();

        std::fs::write(&path, b"").unwrap();
        assert_eq!(cursor.take_new(&path), None);

        std::fs::write(&path, vec![b'x'; 64 * 1024 + 1]).unwrap();
        assert_eq!(cursor.take_new(&path), None);
    }

    #[test]
    fn 解決は書き手のpidを載せる() {
        assert_eq!(
            resolve_writer_path("/data/osc/7.osc", 4242),
            "/data/osc/7@p4242.osc"
        );
        assert_eq!(writer_pid_of("/data/osc/7@p4242.osc"), Some(4242));
        assert_eq!(writer_pid_of("/data/osc/7.osc"), None);
        // ペイン ID との区別（`7.osc` の `7` を pid と読まない）
        assert_eq!(writer_pid_of("7.osc"), None);
    }

    #[test]
    fn 解決は冪等なので子シェルが継承した値を再解決しない() {
        let once = resolve_writer_path("/data/osc/7.osc", 100);
        // ペインの中で起こした子シェル（pid 200）が env を継承して読み直す
        let twice = resolve_writer_path(&once, 200);
        assert_eq!(
            twice, once,
            "再解決すると子シェルの pid でファイルが分かれ、tako が読まない場所へ書く"
        );
    }

    #[test]
    fn writer_sink_pathは解決規則と同じ形を作る() {
        let dir = Path::new("/data");
        let path = writer_sink_path(dir, 7, 4242);
        let env_value = sink_path(dir, 7).display().to_string();
        assert_eq!(
            path.display().to_string(),
            resolve_writer_path(&env_value, 4242),
            "tako が読む先とスクリプトが書く先がずれている"
        );
    }

    #[test]
    fn prepareは解決済みの残骸も消す() {
        let dir = temp_dir("prepare-writer");
        let stale_writer = writer_sink_path(&dir, 7, 999);
        std::fs::create_dir_all(stale_writer.parent().unwrap()).unwrap();
        std::fs::write(&stale_writer, b"\x1b]7;file:///stale\x07").unwrap();
        // 別ペインのぶんは消さない
        let other = writer_sink_path(&dir, 8, 999);
        std::fs::write(&other, b"\x1b]7;file:///other\x07").unwrap();

        prepare(&dir, 7).expect("準備できる");
        assert!(
            !stale_writer.exists(),
            "前回のペインの残骸を今回の起動直後に食わせてしまう"
        );
        assert!(other.exists(), "別ペインの側路を消してはいけない");
    }

    /// #1199 の核心: 器の中の**別のシェル**（psmux のウォームプール）が書いたぶんを
    /// そのペインの状態へ反映してはいけない
    #[test]
    fn 側路は書き手のpidが一致するぶんだけ通す() {
        let dir = temp_dir("reader");
        prepare(&dir, 1).expect("準備できる");
        let mut reader = SinkReader::new(&dir, 1);

        // 器の中の別のシェル（ホームに居る）が書いた束
        let foreign = writer_sink_path(&dir, 1, 777);
        std::fs::write(&foreign, b"\x1b]7;file:///C:/Users/winuser\x07").unwrap();
        assert!(
            reader.take_new().is_empty(),
            "pid を張る前に解決済みを読むと、器の中の別のシェルのぶんを食う"
        );

        // そのペインのシェルの pid が分かった
        reader.set_writer_pid(4242);
        assert!(
            reader.take_new().is_empty(),
            "張った pid 以外のファイルは読まない"
        );

        let mine = writer_sink_path(&dir, 1, 4242);
        std::fs::write(&mine, b"\x1b]7;file:///C:/dev/project\x07").unwrap();
        assert_eq!(
            reader.take_new(),
            vec![b"\x1b]7;file:///C:/dev/project\x07".to_vec()]
        );

        // 別のシェルが上書きし直しても通らない（#1199 の巻き戻り）
        std::fs::write(&foreign, b"\x1b]7;file:///C:/Users/testuser\x07").unwrap();
        assert!(reader.take_new().is_empty());
    }

    #[test]
    fn 更新前のスクリプトを読んだシェルの解決前のパスも読む() {
        let dir = temp_dir("reader-legacy");
        let legacy = prepare(&dir, 2).expect("準備できる");
        let mut reader = SinkReader::new(&dir, 2);
        reader.set_writer_pid(4242);

        std::fs::write(&legacy, b"\x1b]133;A\x07").unwrap();
        assert_eq!(reader.take_new(), vec![b"\x1b]133;A\x07".to_vec()]);
    }

    #[test]
    fn 同じpidの再指定は読み取り位置を巻き戻さない() {
        let dir = temp_dir("reader-repin");
        prepare(&dir, 3).expect("準備できる");
        let mut reader = SinkReader::new(&dir, 3);
        reader.set_writer_pid(4242);
        let mine = writer_sink_path(&dir, 3, 4242);
        std::fs::write(&mine, b"\x1b]133;A\x07").unwrap();
        assert_eq!(reader.take_new().len(), 1);
        reader.set_writer_pid(4242);
        assert!(
            reader.take_new().is_empty(),
            "同じ束を毎 tick 通すと状態機械が同じ遷移を繰り返す"
        );
    }

    #[test]
    fn 差し替え途中のnewも同じ書き手のものとして拾う() {
        assert_eq!(writer_pid_of("/d/osc/1@p42.osc.new"), Some(42));
        assert_eq!(writer_pid_of("/d/osc/1.osc.new"), None);
    }

    #[test]
    fn 器の中の別のシェルが作った待ち合わせ先は掃除される() {
        let dir = temp_dir("prune");
        prepare(&dir, 1).expect("準備できる");
        let mine = writer_sink_path(&dir, 1, 4242);
        let foreign_a = writer_sink_path(&dir, 1, 777);
        let foreign_b = writer_sink_path(&dir, 1, 778);
        let other_pane = writer_sink_path(&dir, 2, 779);
        let legacy = sink_path(&dir, 1);
        // 書き手が死んで取り残された差し替え途中のファイル
        let stray_new = dir.join("osc").join("1@p780.osc.new");
        for p in [
            &mine,
            &foreign_a,
            &foreign_b,
            &other_pane,
            &legacy,
            &stray_new,
        ] {
            std::fs::write(p, b"\x1b]133;A\x07").unwrap();
        }

        let mut reader = SinkReader::new(&dir, 1);
        reader.prune_foreign();
        assert!(
            foreign_a.exists(),
            "書き手が確定する前に消すと、ペイン自身のぶんを落としうる"
        );

        reader.set_writer_pid(4242);
        reader.prune_foreign();
        assert!(mine.exists(), "書き手のぶんを消してはいけない");
        assert!(
            !foreign_a.exists(),
            "別のシェルのぶんが残っている（#1199 で溜まる）"
        );
        assert!(!foreign_b.exists());
        assert!(
            !stray_new.exists(),
            "取り残された .new が掃除から漏れている"
        );
        assert!(other_pane.exists(), "別ペインのぶんを消してはいけない");
        assert!(legacy.exists(), "解決前のパスは互換のために残す");
    }

    #[test]
    fn readerのdiscardは解決済みも消す() {
        let dir = temp_dir("reader-discard");
        let legacy = prepare(&dir, 9).expect("準備できる");
        std::fs::write(&legacy, b"\x1b]133;A\x07").unwrap();
        let mine = writer_sink_path(&dir, 9, 4242);
        std::fs::write(&mine, b"\x1b]133;A\x07").unwrap();

        let mut reader = SinkReader::new(&dir, 9);
        reader.set_writer_pid(4242);
        reader.discard();
        assert!(!legacy.exists());
        assert!(!mine.exists());
    }
}
