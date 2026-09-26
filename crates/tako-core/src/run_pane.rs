//! 実行ペイン（Code Runner #453 / `tako run-interactive` #305 / コマンド提案カード #666）の
//! 状態と、終了コードの受け渡し（#1657）
//!
//! ## 終了コードは画面ではなく側路ファイルで運ぶ
//!
//! #1657 以前は、実行ペインの最後に `__TAKO_EXIT=<code>` の 1 行を**画面へ**出し、
//! 読む側（`tako run --wait` / `tako_run_interactive_status` / カードの実行記録）が
//! 画面からそれを拾っていた。内部の契約がユーザーの目にそのまま見えていたのが #1657 の
//! 症状の 1 つで、狭いペインでは折り返しで割れる（#651）弱さも抱えていた。
//!
//! 画面の外へ運ぶ経路は 3 つ比べた:
//!
//! - **OSC シーケンス**: 器なしの PTY なら `osc_tap` が拾えるが、tmux の中では DCS で
//!   包み直す必要があり、psmux は**素通ししない**（`osc_sink` の doc）。しかも
//!   tmux のパススルーは**そのときクライアントが繋がっている**ときしか届かないので、
//!   GUI を閉じているあいだに終わった実行を取りこぼす
//! - **環境変数でパスだけ教える**: psmux の `-e` はサーバーのグローバル環境へ入る
//!   （#1199 の実測）ので、別のシェルが同じ値を持つ
//! - **起動スクリプトへパスを埋め込んだファイル**（採用）: 直 PTY / tmux / psmux の
//!   どれでも 1 経路で、GUI の接続状態にも依らない
//!
//! 書き手（`platform::shell::run_pane_command` が組むスクリプト）は `<code>\n` を書く。
//! **末尾の改行までそろって初めて読める**取り決めにして、書き込み途中の読み取り
//! （`12` だけ見えて `127` の途中）を構造的に除く。書けなかったときだけ画面へ
//! マーカーを出す（退避路）ので、読む側は「ファイル → 画面のマーカー」の順に見る
//! （`tako_control::dispatch::run_pane_exit_code` の 1 実装）。
//!
//! ## 後片付け
//!
//! ファイルは Enter でペインを閉じたときにスクリプト自身が消す。× で閉じた・
//! 置き換えた（再利用）・auto_close で閉じたときは読む側が消す。
//! それでも残ったもの（プロセスごと落ちた等）は、次に実行ペインを作るときに
//! **生きていないペインのぶんで、十分古いもの**だけを消す（[`prepare`]）。
//! 古さを条件に足すのは、同じデータディレクトリを見る別のホスト
//! （テストの並列実行）が今まさに使っているファイルを消さないため。

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::PaneId;

/// 側路ファイルを置くディレクトリ名（`<data_dir>/<この名前>/`）
const DIR: &str = "run-exit";

/// 生きていないペインの残骸を消してよい古さ（[`prepare`] の掃除）
const STALE_AFTER: Duration = Duration::from_secs(24 * 60 * 60);

/// #1657 の A/B（`TAKO_1657_LEGACY=1`）。**同一バイナリのまま** #1657 前へ戻す:
/// 実行ペインを再利用しない / 終了コードを画面のマーカー行で出す / 案内を出さない
pub fn legacy_1657() -> bool {
    std::env::var_os("TAKO_1657_LEGACY").is_some()
}

/// Code Runner の実行ペインを見分ける鍵（#1657 の再利用条件）。
///
/// **同じファイルの同じプロファイル**なら同じ実行ペインを使い回す。コマンドで
/// 見分けないのは、宣言（`tako:run:`）の引数を直しながら再生を押し直すたびに
/// ペインが積む形に戻ってしまうため（#1657 の症状そのもの）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunKey {
    /// 正規化済みのファイルパス（`platform::path::canonicalize` を通した値）
    pub path: PathBuf,
    /// 解決したプロファイル名（既定は `default`）
    pub profile: String,
}

/// 実行ペインの状態（GUI のバッジ・`tako list` の `run`・`RunInteractiveStatus` の答え）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunStatus {
    /// まだ終了コードが届いていない
    Running,
    /// 終了した（終了コード）
    Exited(i32),
}

impl RunStatus {
    /// wire の表現（`running` / `exited`）
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Exited(_) => "exited",
        }
    }

    /// 終了コード（実行中は `None`）
    pub fn exit_code(self) -> Option<i32> {
        match self {
            Self::Running => None,
            Self::Exited(code) => Some(code),
        }
    }

    /// 終了の結果（実行中は `None`）。**0 だけが成功**
    pub fn outcome(self) -> Option<RunOutcome> {
        self.exit_code().map(|code| {
            if code == 0 {
                RunOutcome::Succeeded
            } else {
                RunOutcome::Failed
            }
        })
    }
}

/// 終了の結果（バッジの色と文言の出し分け）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunOutcome {
    Succeeded,
    Failed,
}

impl RunOutcome {
    /// wire の表現（`success` / `failure`）
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Succeeded => "success",
            Self::Failed => "failure",
        }
    }
}

/// 実行ペインのメタデータ。セッション内で使い捨て（layout.json には保存しない）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InteractiveMeta {
    /// 完了後の自動 close 方針（`success` / `always` / `never`）
    auto_close: String,
    /// ユーザーのコマンド（包む前）
    command: String,
    /// 終了コードの側路ファイル（`None` = 用意できなかった = 画面のマーカーだけが頼り）
    exit_file: Option<PathBuf>,
    /// Code Runner の再利用の鍵（`tako run-interactive` / カードは `None` = 再利用しない）
    run_key: Option<RunKey>,
    status: RunStatus,
}

impl InteractiveMeta {
    pub fn new(auto_close: String, command: String, exit_file: Option<PathBuf>) -> Self {
        Self {
            auto_close,
            command,
            exit_file,
            run_key: None,
            status: RunStatus::Running,
        }
    }

    pub fn auto_close(&self) -> &str {
        &self.auto_close
    }

    pub fn command(&self) -> &str {
        &self.command
    }

    pub fn exit_file(&self) -> Option<&Path> {
        self.exit_file.as_deref()
    }

    pub fn run_key(&self) -> Option<&RunKey> {
        self.run_key.as_ref()
    }

    pub fn set_run_key(&mut self, key: RunKey) {
        self.run_key = Some(key);
    }

    pub fn status(&self) -> RunStatus {
        self.status
    }

    /// 終了コードを確定する。**一度確定したら動かさない**（後から画面に出た別の
    /// マーカーらしき行で上書きしない）。戻り値は状態が変わったか
    pub fn settle(&mut self, code: i32) -> bool {
        if self.status != RunStatus::Running {
            return false;
        }
        self.status = RunStatus::Exited(code);
        true
    }

    /// auto_close の方針どおりなら閉じるべきか（#1662。**判定はこの 1 本**）。
    ///
    /// 終わっていない（`Running`）ものは閉じない。`success` は 0 のときだけ、
    /// `always` は終了コードによらず、`never` と不明な綴りは閉じない
    pub fn wants_close(&self) -> bool {
        let Some(code) = self.status.exit_code() else {
            return false;
        };
        match self.auto_close.as_str() {
            "always" => true,
            "success" => code == 0,
            _ => false,
        }
    }
}

/// auto_close で閉じた実行ペインの結末（#1662）。
///
/// GUI が終わりを検知して閉じると、あとから `tako run --wait` /
/// `tako_run_interactive_status` が聞きに来たときには**ペインがもう無い**。
/// ペインと一緒に結末まで消すと「成功したので閉じた」が「そんなペインは無い」に
/// 化けるので、閉じる側がここへ控えを残す
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClosedRun {
    pub pane: PaneId,
    pub exit_code: i32,
    /// ユーザーのコマンド（包む前）
    pub command: String,
}

/// 閉じた実行ペインの控えを残す上限（古いものから捨てる）。
///
/// 控えを読むのは「閉じた直後に結末を聞きに来る」呼び手だけなので、
/// 同時に `--wait` で待たれる実行ペインの数を十分に上回れば足りる
pub const CLOSED_RUNS_CAP: usize = 64;

/// 閉じた実行ペインの控え（[`crate::Workspace`] が持つ。layout.json には保存しない）
#[derive(Debug, Default)]
pub struct ClosedRuns {
    entries: std::collections::VecDeque<ClosedRun>,
}

impl ClosedRuns {
    /// 控えを残す。同じペインの古い控えは置き換え、上限を超えたら古いものから捨てる
    pub fn record(&mut self, run: ClosedRun) {
        self.entries.retain(|r| r.pane != run.pane);
        self.entries.push_back(run);
        while self.entries.len() > CLOSED_RUNS_CAP {
            self.entries.pop_front();
        }
    }

    /// そのペインの控え（無ければ `None`）
    pub fn get(&self, pane: PaneId) -> Option<&ClosedRun> {
        self.entries.iter().find(|r| r.pane == pane)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// ペインの側路ファイルのパス（純粋関数）
pub fn exit_path(data_dir: &Path, pane_id: u64) -> PathBuf {
    data_dir.join(DIR).join(format!("{pane_id}.code"))
}

/// 側路ファイルを用意して書き先を返す。作れなければ `None`（スクリプトは画面の
/// マーカーへ落ちる = 見た目は #1657 前に戻るが、`--wait` は止まらない）。
///
/// 同じペイン ID の前回のぶんは必ず消す。ペイン ID は再起動をまたいで再利用される
/// ので（#210）、残すと**前回の終了コード**を今回の実行直後に読んでしまう。
/// ついでに「`alive` が偽 = もう居ないペイン」で [`STALE_AFTER`] より古いものを消す
pub fn prepare(data_dir: &Path, pane_id: u64, alive: &dyn Fn(u64) -> bool) -> Option<PathBuf> {
    let path = exit_path(data_dir, pane_id);
    let dir = path.parent()?;
    std::fs::create_dir_all(dir).ok()?;
    discard(&path);
    prune(dir, alive, STALE_AFTER);
    Some(path)
}

/// 生きていないペインの残骸のうち、`older_than` より古いものを消す
fn prune(dir: &Path, alive: &dyn Fn(u64) -> bool, older_than: Duration) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(id) = name
            .to_str()
            .and_then(|n| n.strip_suffix(".code"))
            .and_then(|n| n.parse::<u64>().ok())
        else {
            continue;
        };
        if alive(id) {
            continue;
        }
        let old = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age >= older_than);
        if old {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// 側路ファイルの中身から終了コードを読む（純粋関数）。
///
/// 読めるのは `<code>\n`（Windows の `Set-Content` が書く `<code>\r\n` も可）だけ。
/// **末尾の改行が無いものは書き込み途中**とみなして読まない
pub fn parse(content: &str) -> Option<i32> {
    let body = content.strip_suffix('\n')?;
    let body = body.strip_suffix('\r').unwrap_or(body);
    body.trim().parse().ok()
}

/// 側路ファイルから終了コードを読む（無い・読めない・書き込み途中は `None`）
pub fn read(path: &Path) -> Option<i32> {
    std::fs::read_to_string(path)
        .ok()
        .as_deref()
        .and_then(parse)
}

/// 側路ファイルを消す（無ければ何もしない）
pub fn discard(path: &Path) {
    let _ = std::fs::remove_file(path);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "tako-1657-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn 末尾の改行までそろった中身だけを読む() {
        assert_eq!(parse("0\n"), Some(0));
        assert_eq!(parse("127\n"), Some(127));
        // Windows の Set-Content は CRLF を書く
        assert_eq!(parse("3\r\n"), Some(3));
        // PowerShell のネイティブ exe は負値を返しうる（#875）
        assert_eq!(parse("-1\r\n"), Some(-1));
        // 書き込み途中（改行がまだ無い）は読まない
        assert_eq!(parse("12"), None);
        assert_eq!(parse("3\r"), None);
        assert_eq!(parse(""), None);
        assert_eq!(parse("\n"), None);
        assert_eq!(parse("x\n"), None);
    }

    #[test]
    fn prepareは同じペインの前回ぶんを消す() {
        let dir = temp_dir("prepare");
        let stale = exit_path(&dir, 7);
        std::fs::create_dir_all(stale.parent().unwrap()).unwrap();
        std::fs::write(&stale, "0\n").unwrap();
        let got = prepare(&dir, 7, &|_| true).expect("用意できる");
        assert_eq!(got, stale);
        assert!(!got.exists(), "前回の終了コードが残っている");
        assert_eq!(read(&got), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 掃除は居ないペインの古いものだけを消す() {
        let dir = temp_dir("prune").join(DIR);
        std::fs::create_dir_all(&dir).unwrap();
        for id in [1u64, 2, 3] {
            std::fs::write(dir.join(format!("{id}.code")), "0\n").unwrap();
        }
        std::fs::write(dir.join("memo.txt"), "x").unwrap();
        // 1 = 生きている / 2・3 = 居ない。古さの閾値 0 なら居ないぶんは全部古い
        prune(&dir, &|id| id == 1, Duration::ZERO);
        assert!(
            dir.join("1.code").exists(),
            "生きているペインのぶんを消した"
        );
        assert!(!dir.join("2.code").exists());
        assert!(!dir.join("3.code").exists());
        assert!(dir.join("memo.txt").exists(), "関係ないファイルを消した");
        // 十分新しいものは居なくても残す（並走する別ホストのぶんを消さない）
        std::fs::write(dir.join("4.code"), "0\n").unwrap();
        prune(&dir, &|_| false, STALE_AFTER);
        assert!(dir.join("4.code").exists());
        let _ = std::fs::remove_dir_all(dir.parent().unwrap());
    }

    #[test]
    fn 確定した終了コードは動かさない() {
        let mut meta = InteractiveMeta::new("never".into(), "x".into(), None);
        assert_eq!(meta.status(), RunStatus::Running);
        assert!(meta.settle(1));
        assert!(!meta.settle(0), "確定済みを上書きした");
        assert_eq!(meta.status(), RunStatus::Exited(1));
        assert_eq!(meta.status().outcome(), Some(RunOutcome::Failed));
        assert_eq!(RunStatus::Exited(0).outcome(), Some(RunOutcome::Succeeded));
        assert_eq!(RunStatus::Running.outcome(), None);
    }

    #[test]
    fn auto_closeの判定は方針と終了コードで決まる() {
        let decide = |policy: &str, code: Option<i32>| {
            let mut meta = InteractiveMeta::new(policy.into(), "x".into(), None);
            if let Some(code) = code {
                meta.settle(code);
            }
            meta.wants_close()
        };
        // 終わっていないものはどの方針でも閉じない
        for policy in ["success", "always", "never"] {
            assert!(!decide(policy, None), "{policy}: 実行中に閉じた");
        }
        assert!(decide("success", Some(0)));
        assert!(!decide("success", Some(1)), "失敗を success で閉じた");
        assert!(!decide("success", Some(-1)));
        assert!(decide("always", Some(0)));
        assert!(decide("always", Some(3)));
        assert!(!decide("never", Some(0)));
        assert!(!decide("謎の綴り", Some(0)), "不明な方針で閉じた");
    }

    #[test]
    fn 閉じた実行の控えは上限つきで同じペインは置き換える() {
        let mut runs = ClosedRuns::default();
        let run = |id: u64, code: i32| ClosedRun {
            pane: PaneId::from_raw(id),
            exit_code: code,
            command: format!("cmd-{id}"),
        };
        runs.record(run(1, 0));
        runs.record(run(1, 2));
        assert_eq!(runs.len(), 1, "同じペインの控えが 2 件になった");
        assert_eq!(runs.get(PaneId::from_raw(1)).map(|r| r.exit_code), Some(2));
        for id in 2..(CLOSED_RUNS_CAP as u64 + 10) {
            runs.record(run(id, 0));
        }
        assert_eq!(runs.len(), CLOSED_RUNS_CAP);
        assert!(
            runs.get(PaneId::from_raw(1)).is_none(),
            "古いものから捨てていない"
        );
        let newest = CLOSED_RUNS_CAP as u64 + 9;
        assert_eq!(
            runs.get(PaneId::from_raw(newest))
                .map(|r| r.command.as_str()),
            Some(format!("cmd-{newest}").as_str())
        );
    }
}
