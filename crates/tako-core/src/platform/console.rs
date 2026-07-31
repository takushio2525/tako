//! ペインの疑似コンソールの文字コード（抽象境界 B19。#655）
//!
//! ## なぜ要るか
//!
//! tako の描画経路は入口から出口まで **UTF-8 専用**である。
//! ConPTY から届いたバイト列は alacritty の `vte` パーサ → `Term` グリッド →
//! `screen::snapshot` と流れるが、どこにも「UTF-8 以外かもしれない」経路は無い。
//!
//! ところが Windows のコンソールは「**バイト列をどの符号として解釈するか**」を
//! コードページとして持っており、疑似コンソール（ConPTY）は起動時にそれを
//! **システムの OEM コードページ**で初期化する。日本語版 Windows では 932 になる
//! （実測。`tests/encoding_conpty.rs`）。
//!
//! この状態で子プロセスが UTF-8 バイトを `WriteFile` すると、**conhost が
//! CP932 として解釈して**内部バッファへ積む。tako が受け取るのはその「化けた
//! 文字」を UTF-8 で符号化し直したバイト列なので、**tako 側でどう復号しても
//! 元には戻せない**（例: `日本語テスト` → `譌･譛ｬ隱槭ユ繧ｹ繝`）。
//! 直すにはコンソールのコードページ自体を UTF-8 にするしかない。
//!
//! ## なぜ今まで気づかなかったか
//!
//! **PowerShell 7（pwsh）が起動時に自分でコードページを UTF-8 にするため**、
//! pwsh 7 が入っている開発機では偶然すべて正しく描画されていた（実測）。
//! Windows 同梱の PowerShell 5.1 と `cmd.exe` はそれをしないので、
//! **pwsh 7 が入っていない素の Windows** ではペインが CP932 のままになる。
//! 「システムの地域設定で UTF-8 を切った」場合も同じ結果になる。
//!
//! unix 側で `LC_CTYPE=UTF-8` を既定注入しているのと同じ趣旨の措置で、
//! **tako が自分の前提を自分で敷く**（`terminal.rs` の `default_locale_env` 参照）。
//!
//! ## やり方
//!
//! 疑似コンソールのコードページは、そのコンソールに**接続しているプロセス**から
//! しか変えられない。tako 本体は接続していないので、`AttachConsole` で一時的に
//! 相乗りして `SetConsoleCP` / `SetConsoleOutputCP` を呼び、すぐ離れる。
//!
//! ## 限界（正直に書いておく）
//!
//! これは**ペイン起動時の初期値**を敷くだけで、その後シェルのプロファイル等が
//! コードページを変えれば当然そちらが勝つ（unix で `LC_CTYPE` をユーザーの
//! rc ファイルが上書きできるのと同じ）。tako の既定を正しくするのが目的。

/// 起動したペインの疑似コンソールを UTF-8 に固定した結果。
/// **失敗してもペインの起動は続行する**（描画が化けるだけで、動かなくなるよりはよい）
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PinOutcome {
    /// UTF-8 に固定した
    Pinned,
    /// このプラットフォームでは何もしない（unix はロケール env 側で担保）
    NotNeeded,
    /// 固定できなかった（理由つき。診断ログ用で、ペイン内容は含めない）
    Failed(String),
    /// tako 自身がコンソールを持っているので触らなかった。
    /// debug ビルド（コンソールサブシステム）で起きる。release は GUI サブシステムなので通る
    SkippedHasOwnConsole,
}

/// ペインの疑似コンソールを UTF-8 へ固定する（**1 回だけ試す**）。
/// `pid` は ConPTY 直下の子プロセス。
///
/// tako 自身がコンソールを持っている場合は**何もしない**。相乗りするには自分の
/// コンソールを一度手放す必要があり、手放すと標準ハンドルが無効になって
/// dev 実行時のログ出力が壊れるため
pub fn pin_pane_to_utf8(pid: u32) -> PinOutcome {
    imp::pin_pane_to_utf8(pid)
}

/// 子が疑似コンソールへ接続し終えるのを待ってから固定する（**呼び出しは即座に返る**）。
///
/// `CreateProcess` の直後はまだ接続が済んでおらず `AttachConsole` が
/// `ERROR_GEN_FAILURE`（31）で失敗する。接続が成立するまでの実測は
/// **cmd.exe で 43〜61ms、PowerShell 5.1 で 97ms**（`tests/encoding_conpty.rs` の計測）。
///
/// ペインの起動は UI スレッドから走るのでここでは待てない。別スレッドで短間隔に
/// 試し、成立したら固定して終わる。シェルが最初の 1 行を描くより十分早く決着する
/// （PowerShell 5.1 のプロンプトは実測 500ms 超）
pub fn pin_pane_to_utf8_when_ready(pid: u32) {
    imp::pin_pane_to_utf8_when_ready(pid);
}

/// 自分のコンソールの有無を問わず固定を試みる（**テスト・診断用**）。
///
/// 呼び出し側は「自分のコンソールを失ってよい」ことを理解していること
pub fn force_pin_pane_to_utf8(pid: u32) -> PinOutcome {
    imp::force_pin_pane_to_utf8(pid)
}

/// このプラットフォームで固定が要るか（Windows だけ `true`）。
///
/// **固定の呼び出し側が「pid をどう調べるか」を決める前に問う**ためにある。
/// 器（psmux）の中のシェルの pid は器へ問い合わせないと分からず、その問い合わせは
/// サブプロセス起動である。unix では固定自体が [`PinOutcome::NotNeeded`] になるので、
/// これを先に見ておけば**無駄なサブプロセスもスレッドも作らない**（#659）
pub fn pin_needed() -> bool {
    cfg!(windows)
}

#[cfg(not(windows))]
mod imp {
    use super::PinOutcome;

    pub fn pin_pane_to_utf8(_pid: u32) -> PinOutcome {
        PinOutcome::NotNeeded
    }

    pub fn force_pin_pane_to_utf8(_pid: u32) -> PinOutcome {
        PinOutcome::NotNeeded
    }

    pub fn pin_pane_to_utf8_when_ready(_pid: u32) {}
}

#[cfg(windows)]
mod imp {
    use super::PinOutcome;
    use std::sync::Mutex;

    /// UTF-8 のコードページ番号（`winnls.h` の `CP_UTF8`）
    const CP_UTF8: u32 = 65001;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn AttachConsole(dwProcessId: u32) -> i32;
        fn FreeConsole() -> i32;
        fn GetConsoleCP() -> u32;
        fn SetConsoleCP(wCodePageID: u32) -> i32;
        fn SetConsoleOutputCP(wCodePageID: u32) -> i32;
        fn GetLastError() -> u32;
    }

    /// コンソールへの接続は**プロセス単位**の状態なので、同時に 2 本走らせない。
    /// （ペインを一度に複数開く経路がある。復元 = `layout.json` の一括 spawn）
    ///
    /// 「自分がコンソールを持っているか」の判定も必ずこのロックの内側で行う。
    /// 外に出すと、別ペインの固定が相乗りしている一瞬を「tako 自身がコンソールを
    /// 持っている」と誤読して、固定をまるごと諦めてしまう
    static ATTACH_LOCK: Mutex<()> = Mutex::new(());

    /// 接続が成立するまで諦めない上限。実測は 100ms 以下なので十分な余裕がある
    const READY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);
    /// 再試行の間隔。1 回の `AttachConsole` は失敗しても軽い
    const RETRY_INTERVAL: std::time::Duration = std::time::Duration::from_millis(5);

    /// 自分がコンソールに接続しているか。GUI サブシステム（release）を
    /// ショートカットから起動した場合は接続していない。
    ///
    /// **`GetConsoleWindow()` では判定できない**（実測）。疑似コンソールに
    /// 繋がっているプロセスにはコンソール「ウィンドウ」が無いので、
    /// 端末から起動された場合でも NULL が返り、接続を見落とす。
    /// `GetConsoleCP()` は未接続のときだけ 0 を返すので、こちらで判定する
    /// （接続中: 932 / `FreeConsole` 後: 0 を実測）
    fn has_own_console() -> bool {
        (unsafe { GetConsoleCP() }) != 0
    }

    pub fn pin_pane_to_utf8(pid: u32) -> PinOutcome {
        let _guard = ATTACH_LOCK.lock();
        if has_own_console() {
            return PinOutcome::SkippedHasOwnConsole;
        }
        attach_and_pin(pid)
    }

    pub fn force_pin_pane_to_utf8(pid: u32) -> PinOutcome {
        let _guard = ATTACH_LOCK.lock();
        // 自分のコンソールを持っているなら手放してから相乗りする
        if has_own_console() {
            unsafe { FreeConsole() };
        }
        attach_and_pin(pid)
    }

    pub fn pin_pane_to_utf8_when_ready(pid: u32) {
        // UI スレッドを止めないために別スレッドへ逃がす。ロックは 1 回の試行ごとに
        // 取り直すので、複数ペインを同時に開いても互いを待たせ続けない
        std::thread::Builder::new()
            .name("tako-pin-console-cp".into())
            .spawn(move || {
                let deadline = std::time::Instant::now() + READY_TIMEOUT;
                loop {
                    match pin_pane_to_utf8(pid) {
                        // 成功、または「このプロセスでは触らない」と決まった場合は終わり
                        PinOutcome::Pinned => return,
                        PinOutcome::NotNeeded | PinOutcome::SkippedHasOwnConsole => return,
                        // 子がまだ疑似コンソールへ接続していないだけのことが多い
                        PinOutcome::Failed(reason) => {
                            if std::time::Instant::now() >= deadline {
                                tracing::debug!(
                                    "ペインのコードページを UTF-8 に固定できず: {reason}"
                                );
                                return;
                            }
                        }
                    }
                    std::thread::sleep(RETRY_INTERVAL);
                }
            })
            .ok();
    }

    /// `AttachConsole` している時間は**最短**にする。接続中は自分の標準出力が
    /// ペインへ向くため、この間に何かを書くとユーザーの画面へ漏れる
    fn attach_and_pin(pid: u32) -> PinOutcome {
        unsafe {
            if AttachConsole(pid) == 0 {
                return PinOutcome::Failed(format!("AttachConsole err={}", GetLastError()));
            }
            let out = SetConsoleOutputCP(CP_UTF8);
            let input = SetConsoleCP(CP_UTF8);
            let err = GetLastError();
            FreeConsole();
            if out == 0 || input == 0 {
                return PinOutcome::Failed(format!("SetConsoleCP out={out} in={input} err={err}"));
            }
        }
        PinOutcome::Pinned
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 存在しないプロセスへの固定は失敗を返すだけで落ちない() {
        // 割り当てられることのない pid。Windows では失敗、unix では NotNeeded
        let got = pin_pane_to_utf8(u32::MAX);
        assert!(
            matches!(
                got,
                PinOutcome::Failed(_) | PinOutcome::NotNeeded | PinOutcome::SkippedHasOwnConsole
            ),
            "想定外の結果: {got:?}"
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn unixでは何もしない() {
        assert_eq!(pin_pane_to_utf8(1), PinOutcome::NotNeeded);
    }
}
