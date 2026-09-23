//! PTY へ渡す argv の回帰テスト（#884）。**実際に子プロセスを起こして測る**。
//!
//! Windows には argv という概念が無く、`CreateProcessW` へ渡すのは 1 本の
//! コマンドライン文字列なので、`alacritty_terminal` が `program` と `args` を
//! 空白で連結する。既定（`escape_args = false`）は**各語を素のまま**つなぐため、
//! 空白を含む語が子側の CRT パーサで複数語へ割れていた。
//! `platform::shell::apply_arg_escaping` がそれを閉じているのを、ここで実測で固定する。
//!
//! Windows 以外ではスキップする（unix は `execvp` へ argv がそのまま渡る）。
//! `escape_args` は `#[cfg(target_os = "windows")]` でフィールドごと消えるため、
//! **macOS のユニットテストからは分岐の中身を踏めない**。この網が唯一の実挙動の担保。

#![cfg(windows)]

use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

use tako_core::terminal::{
    SessionEvent, SessionNotice, SpawnCommand, SpawnOptions, TerminalSession,
};

/// 子が**起動する**のを待つ上限の基準値。「これくらいで起動するはず」という見積もりではなく、
/// 無限に待たないための天井なので厚く取る。
///
/// Windows CI の ConPTY + PowerShell（`-NoLogo -NoProfile -ExecutionPolicy Bypass -File`）の
/// 起動は負荷で 30 秒を超える（#1628: 画面には期待どおり `ARGC=1` が出ているのに、
/// 30 秒の上限を使い切って落ちた = 壊れていたのは待ちのほう）。
/// **待ちは観測できた事象で段を切り、天井は待ちの根拠にしない**
const CHILD_START_BASE: Duration = Duration::from_secs(90);

/// 起点（子が動き出した印）が見えてから完走するまでの上限の基準値。
/// ここから先はプローブが `Write-Output` を数行出すだけなので、起動ぶんの猶予は要らない
const AFTER_ANCHOR_BASE: Duration = Duration::from_secs(30);

/// 子の終了を観測したあと、最後の出力が画面へ乗るのを見る猶予の基準値。
///
/// 画面（`visible_lines`）は IO スレッドが書く `Term` を直接読むので通常は取りこぼさないが、
/// Windows の `ChildExit` は**PTY の読み手とは別の見張り**から来るため最後の読みを追い越せる。
/// **これも固定待ちではなく状態待ち**（出た時点で返る）で、出なければここで諦める
const EXIT_SETTLE_BASE: Duration = Duration::from_secs(2);

/// この機の混み具合。**プロセスで 1 回だけ読む**
/// （Windows の読み手は約 120ms ブロックするので毎周期は呼ばない）
fn busy() -> Option<f64> {
    static BUSY: std::sync::OnceLock<Option<f64>> = std::sync::OnceLock::new();
    *BUSY.get_or_init(tako_core::wait_budget::machine_busy)
}

/// 状態待ちの上限（混み具合で**伸ばすだけ**・4 倍で打ち切り）。
/// 政策は `tako_core::wait_budget` の 1 実装を通す（2 か所に書かない）
fn budget_for(base: Duration) -> Duration {
    tako_core::wait_budget::state_wait_budget(base, busy())
}

/// 秒を診断行の書式で
fn secs(d: Duration) -> String {
    format!("{:.1} 秒", d.as_secs_f64())
}

/// 上限を「基準値 → 実際に使った値」で見せる（混み具合で伸びたかが読める）
fn cap_note(base: Duration) -> String {
    format!("{}（基準 {}）", secs(budget_for(base)), secs(base))
}

/// 実行環境（混み具合）。失敗ログに残す（`.agent/conventions.md`）
fn busy_note() -> String {
    busy().map_or_else(|| "読めない".to_string(), |b| format!("{b:.2}"))
}

/// 子の終了を観測したかどうかの表示（診断行用）
fn exit_note(child_exited: bool) -> &'static str {
    if child_exited {
        "終了を観測した"
    } else {
        "終了を観測していない"
    }
}

/// 段に切って待った結果（#1628）。
/// **失敗メッセージへ「経過 / 待っていた文字列 / どの段で止まったか」を載せる**ための材料
struct StagedWait {
    reached: bool,
    /// 起点が見えるまでの経過（見えなければ None）
    anchor_at: Option<Duration>,
    /// 起点が見えてから待った時間（起点が見えなければ None）
    after_anchor: Option<Duration>,
    /// 待ち全体の経過
    elapsed: Duration,
    child_exited: bool,
    anchor: String,
    needle: String,
}

impl StagedWait {
    /// 「遅いのか出ないのか」が読める診断行。
    /// 起点で止まったのか、起点の先で止まったのかを名指しする
    fn diagnosis(&self) -> String {
        let stage = match self.anchor_at {
            None => format!(
                "起点が出ない（\"{}\" が {}待っても出ない / 上限 {}）",
                self.anchor,
                secs(self.elapsed),
                cap_note(CHILD_START_BASE),
            ),
            Some(at) => format!(
                "起点は出たが完走しない（起点 \"{}\" は {}で出た / そこから {}待った / 上限 {}）",
                self.anchor,
                secs(at),
                secs(self.after_anchor.unwrap_or_default()),
                cap_note(AFTER_ANCHOR_BASE),
            ),
        };
        format!(
            "待っていた文字列: \"{}\"\n止まった段: {}\n経過: {}\n子プロセス: {}\n混み具合: {}",
            self.needle,
            stage,
            secs(self.elapsed),
            exit_note(self.child_exited),
            busy_note(),
        )
    }
}

/// 1 ペイン分の PTY。画面に出た文字列で判定する
struct Pane {
    session: TerminalSession,
    rx: futures::channel::mpsc::UnboundedReceiver<SessionEvent>,
    /// 子の終了（`SessionNotice::Exited`）を一度でも観測したか。
    /// 立ったあとに新しい出力は来ないので、上限を待たずに切り上げる根拠になる
    child_exited: bool,
}

impl Pane {
    fn spawn(options: SpawnOptions) -> Self {
        let (session, rx) = TerminalSession::spawn(140, 40, options).expect("PTY を起動できること");
        Self {
            session,
            rx,
            child_exited: false,
        }
    }

    /// 溜まったイベントを取り込む。子の終了を観測したら覚える
    fn drain(&mut self) {
        while let Ok(ev) = self.rx.try_recv() {
            if matches!(self.session.process_event(ev), Some(SessionNotice::Exited)) {
                self.child_exited = true;
            }
        }
    }

    /// `needle` を含む画面になるまで待つ。返すのは到達したかと、そこまでの経過。
    ///
    /// **終わり方は 3 つで、うち 2 つは観測できた事象**: 文字列が出た / 子が終了して
    /// [`EXIT_SETTLE_BASE`] のあいだに出てこない / 上限 `cap`（安全弁）。
    /// 2 つ目があるので、#884 が再発した壊れた側は上限を待たずに落ちる
    fn wait_for(&mut self, needle: &str, cap: Duration) -> (bool, Duration) {
        let settle = budget_for(EXIT_SETTLE_BASE);
        let start = Instant::now();
        let mut exited_at: Option<Instant> = None;
        loop {
            self.drain();
            if self.screen_text().contains(needle) {
                return (true, start.elapsed());
            }
            if self.child_exited {
                let since = exited_at.get_or_insert_with(Instant::now);
                if since.elapsed() >= settle {
                    return (false, start.elapsed());
                }
            }
            if start.elapsed() >= cap {
                return (false, start.elapsed());
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// `anchor`（子が動き出した印）が出たことを**起点**に `needle` を待つ（#1628）。
    ///
    /// 上限をテスト開始からの固定秒 1 本に置くと、起動が負荷で伸びた日に
    /// **検証したい性質は画面に出ているのに**上限を使い切って落ちる。段を分けると
    /// 起点までの遅さが起点の先の予算を食わないうえ、どちらで詰まったかが失敗時に読める。
    ///
    /// 規約は `.agent/conventions.md`「セルフテストの待ち条件の書き方」。
    /// **固定秒の [`Pane::wait_for`] 1 本で完走を判定する形へ戻さないこと**
    fn wait_staged(&mut self, anchor: &str, needle: &str) -> StagedWait {
        let start = Instant::now();
        let (saw_anchor, anchor_at) = self.wait_for(anchor, budget_for(CHILD_START_BASE));
        let (reached, after_anchor) = if saw_anchor {
            let (reached, after) = self.wait_for(needle, budget_for(AFTER_ANCHOR_BASE));
            (reached, Some(after))
        } else {
            // 起点すら出ていないので先の段は待たない（上限を 2 回ぶん使わない）
            (self.screen_text().contains(needle), None)
        };
        StagedWait {
            reached,
            anchor_at: saw_anchor.then_some(anchor_at),
            after_anchor,
            elapsed: start.elapsed(),
            child_exited: self.child_exited,
            anchor: anchor.to_string(),
            needle: needle.to_string(),
        }
    }

    /// 画面イベントを `dur` のあいだ取り込むだけ（判定はしない）。
    /// PTY が死ぬと以降は無音になるので、器へ問い合わせる合間に回す
    fn pump(&mut self, dur: Duration) {
        let deadline = Instant::now() + dur;
        while Instant::now() < deadline {
            self.drain();
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn screen_text(&self) -> String {
        self.session.visible_lines().join("\n")
    }
}

/// 受け取った argv をそのまま報告する PowerShell スクリプトを、
/// **名前に空白を含むディレクトリ**へ置く（`-File <パス>` 側も同時に試すため）
fn write_argv_probe(tag: &str) -> (PathBuf, PathBuf) {
    let dir = std::env::temp_dir().join(format!("tako-884 probe {}-{tag}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("空白入りディレクトリを作れること");
    let script = dir.join("argv probe.ps1");
    std::fs::write(
        &script,
        "Write-Output (\"ARGC=\" + $args.Count)\r\n\
         for ($i = 0; $i -lt $args.Count; $i++) { Write-Output (\"ARG$i=[\" + $args[$i] + \"]\") }\r\n\
         Write-Output \"PROBE-DONE\"\r\n",
    )
    .expect("プローブを書けること");
    (dir, script)
}

/// 空白を含む引数が **1 語のまま**子へ届くこと（#884 の核心）。
///
/// 修正前はコマンドラインが素の空白連結になるため、
/// `-File <空白入りパス>` と `a b c` の両方が割れて PowerShell が起動に失敗する
/// （`ARGC=1` にならない）
#[test]
fn 空白を含む引数が1語のまま子へ届く() {
    let (dir, script) = write_argv_probe("argc");
    let mut pane = Pane::spawn(SpawnOptions {
        command: Some(SpawnCommand {
            program: "powershell.exe".into(),
            args: vec![
                "-NoLogo".into(),
                "-NoProfile".into(),
                "-ExecutionPolicy".into(),
                "Bypass".into(),
                "-File".into(),
                script.display().to_string(),
                "a b c".into(),
            ],
        }),
        cwd: Some(std::env::temp_dir()),
        env: Vec::new(),
        scrollback_lines: None,
    });
    // 待ちは「テスト開始から 30 秒」ではなく、プローブが最初の 1 行を出したこと
    // （= ConPTY 上で PowerShell が動き出した印）を起点に段で見る（#1628）
    let wait = pane.wait_staged("ARGC=", "PROBE-DONE");
    let screen = pane.screen_text();
    // 子が握っているとディレクトリを消せないので、先に PTY を落とす
    drop(pane);
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        wait.reached,
        "プローブが完走しない\n{}\n画面:\n{screen}",
        wait.diagnosis(),
    );
    assert!(
        screen.contains("ARGC=1"),
        "空白入りの引数が割れている（ARGC=1 にならない）:\n{screen}"
    );
    assert!(
        screen.contains("ARG0=[a b c]"),
        "引数の中身が変わっている:\n{screen}"
    );
}

/// テスト用の psmux（無ければスキップ）。`tmux_backend::wrap_options` が使う
/// `tmux_bin()` と同じ解決を通したいので、ここでも `tmux -V` で確かめる
fn container_bin() -> Option<&'static str> {
    let bin = tako_core::tmux::tmux_bin();
    Command::new(bin)
        .arg("-V")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|_| bin)
}

fn container(bin: &str, socket: &str) -> Command {
    let mut cmd = Command::new(bin);
    cmd.args(["-L", socket]);
    cmd
}

/// 器あり（persist ON）でも、空白を含む cwd のペインが生き残ること（#884 の症状そのもの）。
///
/// 修正前は `-c C:\...\dir with space` が `-c C:\...\dir` + `with` + `space` へ割れ、
/// 余った語が `new-session` の **shell-command** として実行されるため
/// `with: The term 'with' is not recognized` でペインが即死していた
/// （tako の器設定は `remain-on-exit` が off なので画面には何も出ない）
#[test]
fn 器ありでも空白入りcwdのペインが生き残る() {
    let Some(bin) = container_bin() else {
        eprintln!("skip: 器（tmux / psmux）が無い");
        return;
    };
    let space_dir = std::env::temp_dir().join(format!("tako-884 cwd {}", std::process::id()));
    std::fs::create_dir_all(&space_dir).expect("空白入り cwd を作れること");
    let socket = format!("tako-884test-{}", std::process::id());
    let session = format!("tako-884-{}", std::process::id());

    let wrapped = tako_core::tmux_backend::wrap_options(
        SpawnOptions {
            command: None,
            cwd: Some(space_dir.clone()),
            env: vec![("TAKO_PANE_ID".into(), "1".into())],
            scrollback_lines: None,
        },
        &socket,
        &session,
    );
    let mut pane = Pane::spawn(wrapped);

    // 器が「そのセッションのペインがこの cwd で生きている」と答えるまで待ち、
    // **そのあと生き続ける**ことまで見る。
    //
    // 最初に見えた 1 回で合格にしてはいけない（この網を作る過程で実測）:
    // `-c` が割れて存在しないディレクトリになると psmux は**クライアントの cwd**へ
    // 落ちるが、`TerminalSession::spawn` は `working_directory`（`CreateProcessW` の
    // `lpCurrentDirectory`）にも同じ cwd を渡していて、そちらは引用の影響を受けない。
    // 結果、壊れていても +600ms までは「正しい cwd のペインが居る」ように見え、
    // 余った語が shell-command として失敗した +1200ms 頃に消える
    let want = space_dir.display().to_string();
    let alive_now = |seen: &mut String| {
        let Ok(out) = container(bin, &socket)
            .args([
                "list-panes",
                "-a",
                "-F",
                "#{session_name} #{pane_dead} #{pane_current_path}",
            ])
            .output()
        else {
            return false;
        };
        *seen = String::from_utf8_lossy(&out.stdout).to_string();
        // cwd に空白が入るので、書式は「セッション名 / 生死 / 残り全部が cwd」で切る
        seen.lines().any(|line| {
            let mut it = line.trim_end().splitn(3, ' ');
            matches!(
                (it.next(), it.next(), it.next()),
                (Some(name), Some("0"), Some(path)) if name == session && path == want
            )
        })
    };

    // 現れるのを待つ上限は 1 本目と同じ [`CHILD_START_BASE`] 由来（同じ ConPTY の起動を
    // 待っているので、ここだけ固定 30 秒に据え置く理由が無い。#1628）。
    // 子が終了したらそれ以上器の答えは変わらないので、上限を待たずに切り上げる
    let mut seen = String::new();
    let appear_cap = budget_for(CHILD_START_BASE);
    let appear_start = Instant::now();
    let mut appeared = false;
    while appear_start.elapsed() < appear_cap {
        pane.pump(Duration::from_millis(300));
        if alive_now(&mut seen) {
            appeared = true;
            break;
        }
        if pane.child_exited {
            break;
        }
    }
    let appeared_at = appear_start.elapsed();

    // 生き残り確認。壊れているときは 1 秒強で消えるので、その 4 倍を見張る。
    // **ここの固定時間は意図的**: 「消えないこと」という否定の検査なので、
    // 先に現れたこと（`appeared`）をアンカーに置いたうえで一定時間見張る
    // （`.agent/conventions.md`「否定検査には必ずアンカーを置く」）
    let mut alive = appeared;
    let mut died_at = None;
    if appeared {
        let watch_start = Instant::now();
        while watch_start.elapsed() < Duration::from_secs(4) {
            pane.pump(Duration::from_millis(400));
            if !alive_now(&mut seen) {
                alive = false;
                died_at = Some(watch_start.elapsed());
                break;
            }
        }
    }

    let screen = pane.screen_text();
    let child_exited = pane.child_exited;
    drop(pane);
    // 後始末。**`-L` を落とすと全ソケットのサーバーが死ぬ**（psmux 実測）
    let _ = container(bin, &socket).arg("kill-server").output();
    let _ = std::fs::remove_dir_all(&space_dir);

    assert!(
        appeared,
        "空白入り cwd のペインが器の中に現れない\n\
         待っていた文字列: \"{want}\"（器の list-panes の行の cwd 欄）\n\
         経過: {}（上限 {}）\n子プロセス: {}\n混み具合: {}\n\
         器の応答: {seen}\n画面:\n{screen}",
        secs(appeared_at),
        cap_note(CHILD_START_BASE),
        exit_note(child_exited),
        busy_note(),
    );
    assert!(
        alive,
        "空白入り cwd のペインが現れたあと消えた（#884 の症状そのもの）\n\
         待っていた文字列: \"{want}\"（器の list-panes の行の cwd 欄）\n\
         経過: 現れるまで {} / そこから {}で消えた\n子プロセス: {}\n混み具合: {}\n\
         器の応答: {seen}\n画面:\n{screen}",
        secs(appeared_at),
        secs(died_at.unwrap_or_default()),
        exit_note(child_exited),
        busy_note(),
    );
}

/// `-e KEY=<空白入りの値>` も 1 語のまま器へ届くこと（#884 の影響範囲）。
///
/// [`tako_core::tmux_backend::wrap_options`] が `-e` へ載せるのは現状
/// `TAKO_PANE_ID` / `TAKO_TAB_ID` の**数値だけ**なので製品経路からはまだ踏めないが、
/// 割れ方は `-c <cwd>` と同一（同じ argv がそのままコマンドラインへ連結される）。
/// **`-e` に空白入りの値を載せる最初の機能が入った瞬間に黙って壊れる**形なので、
/// ここで境界の側から固定しておく。
///
/// 値は器の環境として読み出して突き合わせる（`show-environment`）。
/// 修正前は `-e TAKO_PANE_ID=1 2` が割れ、余った `2` が **shell-command** として
/// 実行されるためセッションごと落ちる
#[test]
fn 空白を含むenvの値も1語のまま器へ届く() {
    let Some(bin) = container_bin() else {
        eprintln!("skip: 器（tmux / psmux）が無い");
        return;
    };
    let socket = format!("tako-884env-{}", std::process::id());
    let session = format!("tako-884e-{}", std::process::id());
    // 値そのものに空白を入れる（`-e` の右辺が割れるかを見る）
    let want = "1 2";
    let wrapped = tako_core::tmux_backend::wrap_options(
        SpawnOptions {
            command: None,
            cwd: Some(std::env::temp_dir()),
            env: vec![("TAKO_PANE_ID".into(), want.into())],
            scrollback_lines: None,
        },
        &socket,
        &session,
    );
    let mut pane = Pane::spawn(wrapped);

    let read_env = |seen: &mut String| {
        let Ok(out) = container(bin, &socket)
            .args(["show-environment", "-t", &session])
            .output()
        else {
            return false;
        };
        *seen = String::from_utf8_lossy(&out.stdout).to_string();
        seen.lines()
            .any(|l| l.trim_end() == format!("TAKO_PANE_ID={want}"))
    };

    // 上限は 1 本目と同じ [`CHILD_START_BASE`] 由来、切り上げは子の終了で（#1628）
    let mut seen = String::new();
    let cap = budget_for(CHILD_START_BASE);
    let start = Instant::now();
    let mut ok = false;
    while start.elapsed() < cap {
        pane.pump(Duration::from_millis(300));
        if read_env(&mut seen) {
            ok = true;
            break;
        }
        if pane.child_exited {
            break;
        }
    }
    let elapsed = start.elapsed();
    let screen = pane.screen_text();
    let child_exited = pane.child_exited;
    drop(pane);
    let _ = container(bin, &socket).arg("kill-server").output();
    assert!(
        ok,
        "空白入りの env 値が器へ 1 語で届いていない\n\
         待っていた文字列: \"TAKO_PANE_ID={want}\"（器の show-environment の行）\n\
         経過: {}（上限 {}）\n子プロセス: {}\n混み具合: {}\n\
         器の応答: {seen}\n画面:\n{screen}",
        secs(elapsed),
        cap_note(CHILD_START_BASE),
        exit_note(child_exited),
        busy_note(),
    );
}
