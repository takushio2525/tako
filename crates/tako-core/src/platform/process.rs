//! 子プロセス起動のプラットフォーム差（抽象境界）
//!
//! GUI プロセス（tako-app）から**コンソール系の子プロセスを起動するときの
//! ウィンドウ抑止**を閉じ込める。プロセスの停止（terminate）は制御側の
//! `tako-control::platform::process` が扱う（こちらは起動時の作法のみ）。
//!
//! ## なぜ要るか（#586）
//!
//! tako-app は release で GUI サブシステムとしてリンクされる。GUI サブシステムの
//! プロセスは自前のコンソールを持たないため、そこから **console サブシステムの
//! 子**（git / claude / tako CLI 等）を起動すると Windows が**子のために
//! コンソールウィンドウを新規作成する**。`Stdio::piped()` にしても防げない
//! （実測: 子の `GetConsoleWindow()` が非 NULL・`IsWindowVisible` = 1）。
//!
//! git タブは 2 秒ポーリングで git を叩くため、対策しないとウィンドウが
//! 明滅し続ける。`CREATE_NO_WINDOW` を付けると子はコンソールを持たなくなる
//! （実測: 子の `GetConsoleWindow()` が NULL）。
//!
//! ## 使いどころ
//!
//! **出力をパイプ / 破棄して受け取るだけの子**に付ける。
//! ペインの PTY 起動（ConPTY）は疑似コンソールへ接続するため対象外
//! （そちらは alacritty_terminal 側が面倒を見る）。

use std::process::Command;

/// 子プロセスにコンソールウィンドウを作らせない。
///
/// Windows 以外では何もしない（呼び出し側に `cfg` を書かせないための境界）。
/// 新しく外部コマンドを叩くコードを足すときは、GUI プロセス（tako-app）から
/// 到達しうるなら必ずこれを通すこと
pub fn no_console_window(cmd: &mut Command) -> &mut Command {
    imp::no_console_window(cmd)
}

/// その pid のプロセスが生きているか（**残骸の掃除の判断に使う**。#916）。
///
/// unix は `kill(pid, 0)`（権限が無くて EPERM でも「居る」= true）。
/// Windows は先にプロセスオブジェクトの終了コード（[`super::procinfo::exit_code`]）を問い、
/// 終了済みなら「居ない」。言えない回は [`super::procinfo::snapshot_checked`] の在籍で見る
/// （あちらは Windows 実装が正）。読み方は [`alive_unless_exited`] の 1 実装が持つ（#2275）。
///
/// **材料が無い回は「居る」側へ倒す**（#1597）。この答えは「消す / 撃つ」の手前に
/// 置かれるので、Windows の在籍の列挙（Toolhelp）が失敗した回に false を返すと
/// **全 pid が残骸に見える**。判定は [`alive_in_snapshot`] の 1 実装が持つ。
///
/// ゾンビ（終了済みで親が未刈り取り）は unix では true になる。掃除の判断としては
/// それで正しい（親がまだ居る = そのプロセス列は現役の可能性がある）。
/// Windows の「終了済みだがハンドルが残るプロセスオブジェクト」は false になる。
/// ハンドルは親に限らず誰でも持てて、漏れれば消えないため（#2275）。
/// 「終わったか」を待ち合わせる用途には使わないこと（`remote.rs` の `has_terminated` が
/// ゾンビ判定込みでそれを担う）
pub fn pid_alive(pid: u32) -> bool {
    imp::pid_alive(pid) || injected_linger(pid) || super::procinfo::injected_reuse(pid)
}

/// #2275 の注入: **テストプロセスでだけ**、居なくなった pid を最初に見てから n ミリ秒は
/// 「居る」と答える（`TAKO_2275_INJECT_ALIVE_MS=<n>`）。
///
/// 「終わったプロセスを生存確認がしばらく居ると読む」形（Windows CI の
/// `queue::tests::居なくなったwrapperの枠は返る` の間欠）を macOS で再現する。
/// 負荷は生成しない。製品のバイナリでは [`crate::paths::is_test_process`] で無視する。
///
/// 窓は**プロセスごと**に数える。env を継承した子のテストプロセスは、死んで久しい pid でも
/// 自分が最初に見た時点から n ms は「居る」と読む（`test_data_residue` の掃除役の子など）。
/// 実物（終わった直後の観測の遅れ）より広く効くので、その形の検査はこの注入では測れない
pub const ENV_INJECT_2275: &str = "TAKO_2275_INJECT_ALIVE_MS";

fn injected_linger(pid: u32) -> bool {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    use std::time::{Duration, Instant};

    static WINDOW: OnceLock<Option<Duration>> = OnceLock::new();
    let window = *WINDOW.get_or_init(|| {
        std::env::var(ENV_INJECT_2275)
            .ok()?
            .trim()
            .parse::<u64>()
            .ok()
            .filter(|ms| *ms > 0)
            .map(Duration::from_millis)
    });
    let Some(window) = window else {
        return false;
    };
    if !crate::paths::is_test_process() {
        return false;
    }
    static FIRST_GONE: OnceLock<Mutex<HashMap<u32, Instant>>> = OnceLock::new();
    let mut seen = FIRST_GONE
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    seen.entry(pid).or_insert_with(Instant::now).elapsed() < window
}

/// pid が居なくなるまで待つ（上限つき）。居なくなったら `true`、上限まで居たら `false`。
///
/// **終わらせた直後の 1 回の [`pid_alive`] で生死を決めない**ための待ち合わせ（#2275）。
/// 生存確認の材料が終わりを映すまでに遅れうる経路（在籍の列挙など）があるので、
/// 「居なくなった」を前提にする検査は状態が変わるまで待ってから進む。
///
/// 判定は [`pid_alive`] そのものなので、unix のゾンビ（親が刈り取っていない子）は
/// 上限まで居るのまま。**自分の子は `wait()` で看取ってから使う**
pub fn wait_gone(pid: u32, limit: std::time::Duration) -> bool {
    let deadline = std::time::Instant::now() + limit;
    loop {
        if !pid_alive(pid) {
            // #2284 の注入の台帳へ（看取りを確かめた pid）。注入が無ければ何もしない
            super::procinfo::note_reaped_gone(pid);
            return true;
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

/// 看取った子（終わって `wait()` 済み）を、**その pid を再利用させないまま**持つ（#2284）。
///
/// Windows の pid は、プロセスオブジェクトへのハンドルがすべて閉じた時点で解放され、
/// 直後に起こされた別のプロセスへ**再利用されうる**。`Child` を drop すると親のハンドルが
/// 閉じるので、「看取った pid = 確実に死んでいる pid」として生死の判定を確かめるテストが
/// drop した後に読むと、pid を取った別プロセスの生存を読む（#2284 の 10/24 の再発）。
/// これを持つ間は `Child`（= ハンドル）を持ち続けるので、オブジェクトは「終了済みだが残る」
/// 状態に留まり pid は再利用されない（[`alive_unless_exited`] の Microsoft の仕様の節）。
/// その間の [`pid_alive`] は終了コードで「居ない」と答える（#2275）。
///
/// unix は `wait()` で刈り取った時点で pid が解放されるので押さえにならないが、
/// 割り当てが順送りなのですぐ再利用されることは実質ない。
/// **起動の作法（[`no_console_window`] など）は呼び手が `cmd` に付ける**
pub struct ReapedChild {
    child: std::process::Child,
}

impl ReapedChild {
    /// `cmd` を起こし、終わるまで待って看取る
    pub fn run(cmd: &mut Command) -> std::io::Result<Self> {
        let child = cmd.spawn()?;
        super::procinfo::pin_against_reuse(child.id(), true);
        // 先に包む: wait が失敗しても Drop で押さえを外す
        let mut reaped = Self { child };
        reaped.child.wait()?;
        Ok(reaped)
    }

    /// 看取った子の pid（これを持つ間は再利用されない）
    pub fn pid(&self) -> u32 {
        self.child.id()
    }
}

impl Drop for ReapedChild {
    fn drop(&mut self) {
        super::procinfo::pin_against_reuse(self.child.id(), false);
    }
}

/// 在籍の列挙 1 枚から 1 件の生死を読む（**Windows の腕の純粋部分**。#1597）。
///
/// `procs` は [`super::procinfo::snapshot_checked`] の結果:
///
/// - `Some(procs)`（1 件以上）= 列挙できた。載っていない pid は**居ない**
/// - `None` / `Some(&[])` = 列挙できなかった回。**「居る」側へ倒す**
///
/// 空の在籍表を「誰も居ない」と読まないのが要点。呼び出したプロセス自身が必ず
/// 載るので 0 件は失敗の別の顔でしかなく、そこを「全員不在」と読むと
/// 残骸の掃除が**生きている別プロセスの置き場まで消しに行く**（#1597 / #625）。
///
/// cfg で割れていないので **macOS からも Windows の腕を単体で検証できる**
pub fn alive_in_snapshot(procs: Option<&[super::procinfo::ProcEntry]>, pid: u32) -> bool {
    match procs {
        Some(procs) if !procs.is_empty() => procs.iter().any(|p| p.pid == pid),
        _ => true,
    }
}

/// `GetExitCodeProcess` がまだ終わっていないプロセスに返す値
/// （`STILL_ACTIVE` = `STATUS_PENDING`。minwinbase.h）
pub const STILL_ACTIVE: u32 = 259;

/// 終了コードで「終わった」と言えれば居ない。言えなければ `roster`（在籍表の読み）に聞く
/// （**Windows の腕の純粋部分**。#2275）。
///
/// `exit_code` は [`super::procinfo::exit_code`] の結果:
///
/// - `Some(code)` で `code != STILL_ACTIVE` = **終了済み = 居ない**。Microsoft の仕様では
///   終了時に「終了状態が `STILL_ACTIVE` から終了コードへ変わる → プロセスオブジェクトが
///   signaled になる」の順で、オブジェクトは**最後のハンドルが閉じるまで消えない**
///   （`ExitProcess` / Terminating a Process）。その間も pid は在籍の列挙に載りうる
///   （載るか・いつ消えるかは仕様に無い）。`Child::wait()` の直後はまさにこの状態で、
///   在籍表だけで読むと終わったプロセスを「居る」と答えた（#2275）
/// - `Some(STILL_ACTIVE)`（走っている。259 で終わったものと区別できない）/
///   `None`（開けない = 居ない・権限が無い）は在籍表で読む（#1597 の規則のまま）
///
/// 終了済みを「居ない」と言い切ってよいのは、オブジェクトが残る間は pid が再利用されず、
/// 同じ pid の別プロセスを死んだと読むことが無いから。
/// cfg で割れていないので **macOS からも Windows の腕を単体で検証できる**
pub fn alive_unless_exited(exit_code: Option<u32>, roster: impl FnOnce() -> bool) -> bool {
    if exited(exit_code) {
        return false;
    }
    roster()
}

/// 終了コードで「終わった」と言えるか（#2275 / #2284 の境界の 1 実装）。
///
/// `Some(code)` で `code != STILL_ACTIVE` のときだけ真。`None`（開けない・unix）と
/// `Some(STILL_ACTIVE)`（走っている。259 で終わったものと区別できない）は言えない。
/// 根拠は [`alive_unless_exited`] の Microsoft の仕様の節
pub fn exited(exit_code: Option<u32>) -> bool {
    exit_code.is_some_and(|code| code != STILL_ACTIVE)
}

/// **走っている**プロセスの起動時刻（UNIX 秒。#2284）。終了済みなら `None`。
///
/// [`super::procinfo::start_time_unix`] は Windows では終了済みでもプロセスオブジェクトが
/// 残る間（誰かがハンドルを持つ間）は読める。`GetProcessTimes` は終了済みのプロセスにも
/// 答える API で、`lpExitTime` は「プロセスの終了時刻を受け取る。終わっていなければ未定義」
/// とされている。「起動時刻が取れた = 生きている」と読む呼び手（蓋閉じ継続の記録の
/// 所有者の確認 = `tako-control::platform::lid`）はこちらを通す。読み方は
/// [`started_unless_exited`] の 1 実装。
///
/// unix は終了コードを持たない（`exit_code` が `None`）ので
/// [`super::procinfo::start_time_unix`] と同じ値を返す（振る舞いを変えない）
pub fn live_start_time_unix(pid: u32) -> Option<u64> {
    started_unless_exited(super::procinfo::start_time_unix(pid), || {
        super::procinfo::exit_code(pid)
    })
}

/// 読めた起動時刻を、終了コードで「終わった」と言えれば捨てる（**Windows の腕の純粋部分**。#2284）。
///
/// 起動時刻を先に読み、終了コードは後に問う（起動時刻が取れたときだけ）。読んだ直後に
/// 終わったものも捨てられる順で、終了コードの読み方は [`exited`] の 1 実装（#2275 と同じ境界）。
/// cfg で割れていないので **macOS からも Windows の腕を単体で検証できる**
pub fn started_unless_exited(
    started: Option<u64>,
    exit_code: impl FnOnce() -> Option<u32>,
) -> Option<u64> {
    let started = started?;
    (!exited(exit_code())).then_some(started)
}

#[cfg(not(windows))]
mod imp {
    use std::process::Command;

    pub fn no_console_window(cmd: &mut Command) -> &mut Command {
        cmd
    }

    pub fn pid_alive(pid: u32) -> bool {
        // `kill` の第 1 引数は 0 で「自分のプロセスグループ」、負で「プロセスグループ /
        // 全プロセス」を指す**特別値**。`as` で潰すと u32::MAX が -1 になり
        // 「全プロセスへ送る」= 常に成功してしまう（テストで実際に踏んだ）。
        // pid_t の正の範囲に収まらない値は「居ない」と答える
        let Ok(pid) = libc::pid_t::try_from(pid) else {
            return false;
        };
        if pid <= 0 {
            return false;
        }
        // SAFETY: signal 0 は送らずに存在と権限だけを確かめる呼び出し。
        // 引数は値渡しでポインタを触らない
        let rc = unsafe { libc::kill(pid, 0) };
        if rc == 0 {
            return true;
        }
        // 権限が無くて拒否された = 相手は居る
        std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }
}

#[cfg(windows)]
mod imp {
    use std::os::windows::process::CommandExt;
    use std::process::Command;

    /// `CREATE_NO_WINDOW`（winbase.h）。コンソールを持たない親から起動された
    /// コンソールアプリに、コンソールウィンドウを与えない
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    pub fn no_console_window(cmd: &mut Command) -> &mut Command {
        cmd.creation_flags(CREATE_NO_WINDOW)
    }

    pub fn pid_alive(pid: u32) -> bool {
        if pid == 0 {
            return false;
        }
        // 先にプロセスオブジェクトへ終了コードを問う（#2275）。終わった直後でも誰かが
        // ハンドルを持つ間はオブジェクトが残り、在籍の列挙に載りうるため。
        // 言えない回は在籍の列挙（Toolhelp）で見る。どちらも procinfo 側にあるのでそれを使う
        // （OpenProcess を新たに宣言せずに済み、実装は 1 か所に留まる）。
        // **失敗と不在を混ぜない**ため `snapshot_checked` を通し、読み方は
        // 共有の純粋関数へ渡す（列挙できなかった回は「居る」= #1597）
        super::alive_unless_exited(super::super::procinfo::exit_code(pid), || {
            super::alive_in_snapshot(super::super::procinfo::snapshot_checked().as_deref(), pid)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 自分のpidは生きている() {
        assert!(pid_alive(std::process::id()));
    }

    /// `kill` の特別値（0 / 負）へ落ちないこと。ここを `as` で潰すと
    /// u32::MAX が -1 = 「全プロセス」になり、生きていない pid が生きて見える
    #[test]
    fn 特別値になる範囲外のpidは生きていない() {
        assert!(!pid_alive(u32::MAX), "pid_t の範囲外");
        assert!(!pid_alive(0), "0 はプロセスグループの指定");
        assert!(
            !pid_alive(i32::MAX as u32),
            "pid_t の上限は実在しない（macOS / Linux の pid 上限より大きい）"
        );
    }

    fn entry(pid: u32) -> super::super::procinfo::ProcEntry {
        super::super::procinfo::ProcEntry {
            pid,
            ppid: 1,
            name: format!("p{pid}.exe"),
        }
    }

    /// **Windows の腕の規則**（macOS からも回る）。列挙できた回だけが
    /// 「居ない」と言える。#1597 の事故はここを逆に倒したことで起きる
    #[test]
    fn 在籍を列挙できなかった回は居る側へ倒す() {
        let procs = [entry(7), entry(9)];
        assert!(alive_in_snapshot(Some(&procs), 7), "載っている pid は居る");
        assert!(
            !alive_in_snapshot(Some(&procs), 8),
            "列挙できた回は載っていない pid を「居ない」と言える"
        );
        assert!(
            alive_in_snapshot(None, 8),
            "列挙に失敗した回に「居ない」と答えると全 pid が残骸に見える（#1597）"
        );
        assert!(
            alive_in_snapshot(Some(&[]), 8),
            "空の在籍表は列挙の失敗（自分自身が必ず載るので 0 件は在り得ない）"
        );
    }

    /// **#2275 の名指し**（macOS からも回る）。終了コードが取れた終了済みのプロセスは、
    /// 在籍表に載っていても居ない。修正前の Windows の腕は在籍表だけで読み、`Child::wait()` の
    /// 直後（ハンドルが残る = オブジェクトが残る）に終わった wrapper を「居る」と答えた
    #[test]
    fn issue2275_終了コードが取れた終了済みは在籍表に載っていても居ない() {
        let procs = [entry(7)];
        let listed = || alive_in_snapshot(Some(&procs), 7);
        assert!(
            !alive_unless_exited(Some(1), listed),
            "終了コード 1（TerminateProcess）で終わったのに在籍表を信じた（#2275）"
        );
        assert!(
            !alive_unless_exited(Some(0), listed),
            "0 で終わったものも終了済み"
        );
        assert!(
            !alive_unless_exited(Some(1), || unreachable!(
                "終了済みと言えたら在籍表は採らない"
            )),
            "終了済みなら列挙を待たずに答える"
        );
        // 言えない回は在籍表の規則（#1597）のまま
        assert!(
            alive_unless_exited(Some(STILL_ACTIVE), listed),
            "走っている"
        );
        assert!(
            alive_unless_exited(None, listed),
            "開けない回は在籍表で読む"
        );
        assert!(
            !alive_unless_exited(None, || alive_in_snapshot(Some(&procs), 8)),
            "開けず載っていない pid は居ない"
        );
        assert!(
            alive_unless_exited(None, || alive_in_snapshot(None, 8)),
            "開けず列挙もできない回は居る側へ倒す（#1597）"
        );
    }

    /// **#2284 の名指し**（macOS からも回る）。終了コードが取れた終了済みのプロセスの起動時刻は、
    /// 読めても「走っている」材料にしない。修正前の `lid::probe_owner` は起動時刻が取れた時点で
    /// `Alive` と答え、終わったのにオブジェクトが残る pid（Windows）を生きていると読んだ
    #[test]
    fn issue2284_終了済みのプロセスオブジェクトの起動時刻は走っている材料にしない() {
        const STARTED: u64 = 1_700_000_000;
        assert_eq!(
            started_unless_exited(Some(STARTED), || Some(0)),
            None,
            "0 で終わったのに起動時刻を「走っている」と読んだ（#2284）"
        );
        assert_eq!(
            started_unless_exited(Some(STARTED), || Some(1)),
            None,
            "TerminateProcess（1）で終わったものも終了済み"
        );
        // 言えない回は読めた起動時刻をそのまま返す（unix はいつもここ = 振る舞いを変えない）
        assert_eq!(
            started_unless_exited(Some(STARTED), || Some(STILL_ACTIVE)),
            Some(STARTED),
            "走っている"
        );
        assert_eq!(
            started_unless_exited(Some(STARTED), || None),
            Some(STARTED),
            "終了コードを開けない回（unix は常に）は起動時刻のまま"
        );
        assert_eq!(
            started_unless_exited(None, || unreachable!(
                "起動時刻が無ければ終了コードは問わない"
            )),
            None
        );
        // 境界は #2275 と同じ 1 実装
        assert!(exited(Some(0)) && exited(Some(1)));
        assert!(!exited(Some(STILL_ACTIVE)) && !exited(None));
    }

    /// 自分は走っているので、起動時刻は境界の値そのもの（Windows では終了コードが STILL_ACTIVE）
    #[test]
    fn 走っている自分の起動時刻はstart_time_unixと同じ() {
        let me = std::process::id();
        assert_eq!(
            live_start_time_unix(me),
            super::super::procinfo::start_time_unix(me)
        );
    }

    /// **#2275 の実プロセスでの回帰**。看取った（`wait()` が返った）子の pid は、`Child` が
    /// ハンドルを持ったまま = Windows では終了済みのプロセスオブジェクトが残った状態でも、
    /// その場の 1 回で居ないと読める（終了状態は signaled より先に決まる = 仕様で待ちが要らない）。
    /// CI の Windows で回るのが実機の証拠になる
    #[test]
    fn issue2275_看取った直後の子はハンドルが残っていても居ない() {
        // 眠るだけの子（Windows は ping で眠る）。起動は 1 か所で抑止を通す（#628 の番犬の数え方）
        let (program, args): (&str, &[&str]) = if cfg!(windows) {
            ("ping", &["-n", "30", "127.0.0.1"])
        } else {
            ("sleep", &["30"])
        };
        for round in 0..20 {
            let mut cmd = Command::new(program);
            cmd.args(args).stdout(std::process::Stdio::null());
            let mut child = no_console_window(&mut cmd)
                .spawn()
                .expect("眠るだけの子を起こせる");
            let pid = child.id();
            assert!(pid_alive(pid), "{round} 回目: 起こした子 {pid} が居ない");
            child.kill().expect("止められる");
            child.wait().expect("看取れる");
            // `child` はまだ drop しない = ハンドルが残る（Windows は終了済みのオブジェクトが残る）
            if cfg!(windows) {
                let code = super::super::procinfo::exit_code(pid);
                assert!(
                    code.is_some_and(|c| c != STILL_ACTIVE),
                    "{round} 回目: 終了済みのオブジェクトを開いて終了コードを読めない（{code:?}）"
                );
            }
            assert!(
                !pid_alive(pid),
                "{round} 回目: 看取った pid {pid} を居ると読んだ（#2275）"
            );
            drop(child);
        }
    }

    /// [`ReapedChild`] の両端（#2284）: 起こせないコマンドは `Err`。起こせた子は終わるまで
    /// 待って看取り、押さえを持つ間のその pid は居ない（Windows は終了コード = #2275）
    #[test]
    fn reaped_childは起こせないコマンドをerrで返し看取った子のpidを持つ() {
        let mut missing = Command::new("tako-no-such-program-2284");
        assert!(
            ReapedChild::run(no_console_window(&mut missing)).is_err(),
            "起こせないコマンドを看取れたことにした"
        );
        // すぐ終わる子。起動は 1 か所で抑止を通す（#628 の番犬の数え方）
        let (program, args): (&str, &[&str]) = if cfg!(windows) {
            ("cmd", &["/C", "exit 0"])
        } else {
            ("true", &[])
        };
        let mut cmd = Command::new(program);
        cmd.args(args).stdout(std::process::Stdio::null());
        let reaped = ReapedChild::run(no_console_window(&mut cmd)).expect("起こして看取れる");
        let pid = reaped.pid();
        assert!(
            !pid_alive(pid),
            "看取った子 {pid} を押さえの間に居ると読んだ"
        );
        drop(reaped);
    }

    /// `wait_gone` の両端: 居続ける pid は上限で諦めて `false`、居ない pid は待たずに `true`
    #[test]
    fn wait_goneは居続けるpidを上限で諦め居ないpidは待たない() {
        use std::time::Duration;
        assert!(
            !wait_gone(std::process::id(), Duration::from_millis(30)),
            "自分は居なくならない"
        );
        assert!(
            wait_gone(u32::MAX, Duration::ZERO),
            "居ない pid は上限 0 でも 1 回目で true"
        );
    }

    #[test]
    fn no_console_windowを通しても子プロセスの結果は変わらない() {
        // 境界が「ウィンドウを出さない」以外の副作用を持たないことの回帰テスト。
        // 実行するのは各プラットフォームで確実に存在するコマンドに限る
        let mut cmd = if cfg!(windows) {
            let mut c = Command::new("cmd");
            c.args(["/C", "echo tako"]);
            c
        } else {
            let mut c = Command::new("echo");
            c.arg("tako");
            c
        };
        let out = no_console_window(&mut cmd)
            .output()
            .expect("子プロセスを起動できない");
        assert!(out.status.success());
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "tako");
    }
}
