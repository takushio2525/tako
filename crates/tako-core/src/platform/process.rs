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
/// Windows は [`super::procinfo::snapshot_checked`] の在籍で見る（あちらは Windows 実装が正）。
///
/// **材料が無い回は「居る」側へ倒す**（#1597）。この答えは「消す / 撃つ」の手前に
/// 置かれるので、Windows の在籍の列挙（Toolhelp）が失敗した回に false を返すと
/// **全 pid が残骸に見える**。判定は [`alive_in_snapshot`] の 1 実装が持つ。
///
/// ゾンビ（終了済みで親が未刈り取り）は unix では true になる。掃除の判断としては
/// それで正しい（親がまだ居る = そのプロセス列は現役の可能性がある）。
/// 「終わったか」を待ち合わせる用途には使わないこと（`remote.rs` の `has_terminated` が
/// ゾンビ判定込みでそれを担う）
pub fn pid_alive(pid: u32) -> bool {
    imp::pid_alive(pid)
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
        // Windows は在籍の列挙（Toolhelp）が procinfo 側にあるのでそれを使う。
        // OpenProcess を新たに宣言せずに済み、実装は 1 か所に留まる。
        // **失敗と不在を混ぜない**ため `snapshot_checked` を通し、読み方は
        // 共有の純粋関数へ渡す（列挙できなかった回は「居る」= #1597）
        super::alive_in_snapshot(super::super::procinfo::snapshot_checked().as_deref(), pid)
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
