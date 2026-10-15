//! 接続エラーで止まったエージェントの画面の切り出しと、回線の到達確認（FR-2.27.17 / Issue #2112）。
//!
//! 判断（送るか・いつ確かめるか）は `tako_core::net_resume` の純関数が持ち、ここは
//! **画面の文字列から「接続エラーでターンを終えて入力待ち」を見分ける**ことと、
//! **到達確認の I/O**（名前解決 + TCP 接続）だけを持つ。駆動は tako-app の 2 秒 tick。
//!
//! # 窓を固定しない（#1847 の 1 つ目の症状の型を踏まない）
//!
//! `detect_worker_error` の `api_error` は「空行を除いた末尾 15 行」に `API Error` があるかで見る。
//! 狭いペインではエラーの行も claude のフッター（区切り線・入力欄・モード表示・帯）も
//! 折り返して行数が増えるので、エラーの先頭行が 15 行の外へ押し出されうる。
//! ここは**入力欄を錨にして上へ辿る**: 入力欄の上の区切り線から上へ見て、最初に当たる
//! 会話の行（`⏺` / `●` / `⎿` / `❯`）が**最後の会話の項目**で、それが接続系の
//! `API Error` なら止まっている。折り返しは項目の続きの行を結合して 1 論理行に戻す。
//!
//! # 止まっていると言わないもの
//!
//! - 生成中・再試行中（中断の案内 `esc to interrupt` が見えている）
//! - 最後の項目が人の発言（`❯ 続き`）= もう誰かが送った
//! - 最後のターンにログイン失効（#757）・使用制限（#813）・利用阻害（#1106）の行がある
//! - 入力欄が見えない（選択ダイアログ・メニュー。[`NetScreen::Dialog`] で返す）

use std::time::{Duration, Instant};

use tako_core::net_resume::{connection_error_kind, ConnErrorKind, ProbeTarget};

/// 画面から見た接続エラーの停止
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetStop {
    pub kind: ConnErrorKind,
    /// 停止の根拠になった 1 論理行（状態照会の応答にだけ載せる。診断ログには出さない）
    pub message: String,
    /// この停止を他の停止と見分ける鍵（同じ停止に 2 回送らないための正本）。
    /// 直前の人の発言・エラーの 1 論理行・ターンの締めの行（`✻ Worked for 0s · done 6:55 PM`）
    /// から作る。帯・ヒント・ステータス行は入れない（時間で変わるので、同じ停止が別物に見える）。
    /// 「続き」が届いてまた止まれば、直前の人の発言と締めの行が変わるので別の鍵になる
    pub key: u64,
}

/// 画面の判定結果
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NetScreen {
    /// 接続系のエラーでターンを終え、入力欄が空いて止まっている
    Stopped(NetStop),
    /// 接続系のエラーが見えるが、入力欄の代わりに選択ダイアログ・メニューが出ている（送らない）
    Dialog(NetStop),
    /// 該当しない
    Clear,
}

/// 会話の項目の頭（claude の発言 = `⏺`（Windows は `●`）・ツールの結果 = `⎿`・人の発言 = `❯`）
const ENTRY_MARKS: [char; 4] = ['⏺', '●', '⎿', '❯'];

/// 画面（上から順の行）を判定する
pub fn detect(lines: &[String]) -> NetScreen {
    let Some(input) = input_box(lines) else {
        // 入力欄が無い。選択ダイアログの裏に接続エラーが見えていれば「ダイアログ中」と返す
        // （見送りの理由を状態に出すため）。送らないことは Clear と同じ
        if crate::claude_tui::is_choice_dialog(lines) {
            if let Some(stop) = lines
                .iter()
                .rev()
                .find_map(|l| connection_error_kind(l).map(|k| (k, l)))
                .map(|(kind, l)| {
                    let message = strip_mark(l.trim()).to_string();
                    NetStop {
                        kind,
                        key: stop_key(None, &message, None),
                        message,
                    }
                })
            {
                return NetScreen::Dialog(stop);
            }
        }
        return NetScreen::Clear;
    };
    // 生成中・再試行中（`Retrying in 3s · attempt 3/4` の行には中断の案内が付く）
    if crate::claude_tui::interrupt_hint_visible(lines) {
        return NetScreen::Clear;
    }
    // 入力欄の上の区切り線（input - 1）より上で、最後の会話の項目を探す
    let top = input - 1;
    let Some(entry) = (0..top).rev().find(|&i| is_entry(&lines[i])) else {
        return NetScreen::Clear;
    };
    if lines[entry].trim_start().starts_with('❯') {
        return NetScreen::Clear; // 人の発言が最後 = もう送られている
    }
    let message = join_entry(&lines[entry..top]);
    let Some(kind) = connection_error_kind(&message) else {
        return NetScreen::Clear;
    };
    // 最後のターン（直前の人の発言から入力欄まで）に、回線では直らない停止の行が無いか
    let echo = (0..entry).rev().find(|&i| {
        let t = lines[i].trim_start();
        t.starts_with('❯') && !t.trim_start_matches('❯').trim().is_empty()
    });
    if blocked_by_other_stop(&lines[echo.unwrap_or(0)..top]) {
        return NetScreen::Clear;
    }
    let key = stop_key(
        echo.map(|i| lines[i].trim()),
        &message,
        closing_line(&lines[entry..top]),
    );
    NetScreen::Stopped(NetStop { kind, message, key })
}

/// ターンの締めの行（`✻ Worked for 0s · done 6:55 PM`）。項目の後の最初の空でない行が
/// 記号で始まり経過時間（` for 0s`）を持つときだけ採る（帯・ヒントは採らない）
fn closing_line(after_entry: &[String]) -> Option<&str> {
    let mut rest = after_entry.iter().skip(1).map(|l| l.trim());
    // 項目の続きの行（折り返し）を読み飛ばす
    let line = rest
        .by_ref()
        .skip_while(|t| !t.is_empty())
        .find(|t| !t.is_empty())?;
    let glyph = line.chars().next().is_some_and(|c| !c.is_alphanumeric());
    (glyph && line.contains(" for ") && line.chars().any(|c| c.is_ascii_digit())).then_some(line)
}

fn stop_key(echo: Option<&str>, message: &str, closing: Option<&str>) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    echo.hash(&mut h);
    message.hash(&mut h);
    closing.hash(&mut h);
    h.finish()
}

/// 画面末尾だけ（`AGENT_TUI_TAIL_LINES`）では足りず、画面全体を読み直すべきか。
///
/// 入力欄は見えているのに、その上に会話の項目が 1 つも無いときだけ true
/// （始めたばかりの会話を背の高いペインで見ると、会話は画面の上端・入力欄は下端で、
/// あいだが空行で埋まる。末尾の窓では最後の項目に届かない）。
/// 2 秒 tick は普段は末尾だけを読み、全体はこの場合に限る
pub fn needs_full_screen(tail: &[String]) -> bool {
    // 生成中なら全体を読んでも止まっていないと言うだけ（読む前に抜ける）
    !crate::claude_tui::interrupt_hint_visible(tail)
        && input_box(tail).is_some_and(|input| !(0..input - 1).any(|i| is_entry(&tail[i])))
}

/// 入力欄（`❯` で始まり、すぐ上が区切り線の行）の位置。**最も下にあるもの**を採る
/// （会話の中の人の発言も `❯` で始まるが、その上は区切り線ではない）
fn input_box(lines: &[String]) -> Option<usize> {
    (1..lines.len())
        .rev()
        .find(|&i| lines[i].trim_start().starts_with('❯') && is_rule(&lines[i - 1]))
}

/// 区切り線（`────`）。ラベルが混ざる版もあるので 8 割が罫線なら区切りとみなす
fn is_rule(line: &str) -> bool {
    let t = line.trim();
    let n = t.chars().count();
    n >= 8 && t.chars().filter(|c| matches!(c, '─' | '━')).count() * 10 >= n * 8
}

fn is_entry(line: &str) -> bool {
    line.trim_start()
        .chars()
        .next()
        .is_some_and(|c| ENTRY_MARKS.contains(&c))
}

/// 会話の項目の頭の記号を落とす
fn strip_mark(text: &str) -> &str {
    text.trim_start_matches(|c: char| ENTRY_MARKS.contains(&c) || c.is_whitespace())
}

/// 項目の行と、その続き（空行・次の項目・区切り線の手前まで）を 1 論理行へ結合する
/// （claude は本文を自分で折り返し、続きを字下げする。狭いペインでは 4 行に割れる）
fn join_entry(lines: &[String]) -> String {
    let mut out = strip_mark(lines[0].trim()).to_string();
    for l in &lines[1..] {
        let t = l.trim();
        if t.is_empty() || is_entry(l) || is_rule(l) {
            break;
        }
        out.push(' ');
        out.push_str(t);
    }
    out
}

/// 回線では直らない停止（ログイン失効・使用制限・利用阻害）の行があるか。
/// 折り返しで割れた見出しも読む（#1123 と同じ結合を通す）
fn blocked_by_other_stop(turn: &[String]) -> bool {
    let joined = turn.join("\n");
    let logical = tako_core::limit_resume::unwrapped_tail_of(&joined, turn.len());
    turn.iter()
        .map(String::as_str)
        .chain(logical.iter().map(String::as_str))
        .any(|l| {
            crate::orchestrator::agent_cli::login_expired_line(l)
                || tako_core::limit_resume::is_limit_exhausted_line(l)
                || tako_core::limit_resume::entitlement_block_line(l)
        })
}

/// 到達確認: 名前解決して TCP で接続できるかだけを見る（データは 1 バイトも送らない）。
/// 成功ならかかった時間、失敗なら理由（OS のエラー文。宛先以外の個人の情報は含まない）。
///
/// **UI スレッドから呼ばないこと**（名前解決は時間切れを持たない OS の呼び出しで、
/// 回線の状態によっては数秒塞がる）
pub fn probe(target: &ProbeTarget, timeout: Duration) -> Result<Duration, String> {
    use std::net::{TcpStream, ToSocketAddrs};
    let start = Instant::now();
    let addrs: Vec<std::net::SocketAddr> = (target.host.as_str(), target.port)
        .to_socket_addrs()
        .map_err(|e| format!("dns: {}", short(&e.to_string())))?
        .take(4)
        .collect();
    if addrs.is_empty() {
        return Err("dns: no address".into());
    }
    let mut last = String::new();
    for addr in addrs {
        match TcpStream::connect_timeout(&addr, timeout) {
            Ok(stream) => {
                let _ = stream.shutdown(std::net::Shutdown::Both);
                return Ok(start.elapsed());
            }
            Err(e) => last = format!("connect: {}", short(&e.to_string())),
        }
    }
    Err(last)
}

fn short(s: &str) -> String {
    s.chars().take(120).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(screen: &str) -> Vec<String> {
        screen.lines().map(str::to_string).collect()
    }

    fn stopped(screen: &str) -> Option<NetStop> {
        match detect(&lines(screen)) {
            NetScreen::Stopped(s) => Some(s),
            _ => None,
        }
    }

    /// claude 2.1.294 を到達できない API 先（`.invalid`）へ向けて採った実画面（100 桁）
    const ENOTFOUND_100: &str = " ▐▛███▛█   Claude Code v2.1.294
▝▜██████▀  Opus 5.5 · API Usage Billing
 ▝▝   ▝▝   /…/cap/work


❯ hello

⏺ API Error: Can't reach the API server — check your internet or DNS (ENOTFOUND)

✻ Worked for 0s · done 6:55 PM



  tmux detected · scroll with PgUp/PgDn · or add 'set -g mouse on' to ~/.tmux.conf for wheel scro…
────────────────────────────────────────────────────────────────────────────────────────────────────
❯
────────────────────────────────────────────────────────────────────────────────────────────────────
  ⏵⏵ auto mode on (shift+tab to cycle) · ← for agents";

    /// 同じ画面を 32 桁へ縮めたもの（エラーが 4 行に折り返る）
    const ENOTFOUND_32: &str = "           Claude Code v2.1.294
 ▐▛███▛█   Opus 5.5
▝▜██████▀  API Usage Billing
 ▝▝   ▝▝   /…/cap/work


❯ hello

⏺ API Error: Can't reach
  the API server —
  check your internet or
  DNS (ENOTFOUND)

✻ Worked for 0s · done 6:55 PM





────────────────────────────────
❯
────────────────────────────────
  ⏵⏵ auto mode on (shift+tab";

    /// 再試行中（同じ claude を `CLAUDE_CODE_MAX_RETRIES=4` で採った実画面）
    const RETRYING_100: &str = "❯ hello

✻ Can't reach the API server — check your internet or DNS (ENOTFOU… · Retrying in 3s · attempt 3/4
                                                                                ◐ medium · /effort
────────────────────────────────────────────────────────────────────────────────────────────────────
❯
────────────────────────────────────────────────────────────────────────────────────────────────────
  ⏵⏵ auto mode on (shift+tab to cycle) · esc to interrupt · ← for agents";

    #[test]
    fn issue2112_実画面の接続エラーの停止を見分ける() {
        let s = stopped(ENOTFOUND_100).expect("100 桁");
        assert_eq!(s.kind, ConnErrorKind::Dns);
        assert_eq!(
            s.message,
            "API Error: Can't reach the API server — check your internet or DNS (ENOTFOUND)"
        );
        // 折り返しを結合して同じ 1 論理行に戻る
        let s = stopped(ENOTFOUND_32).expect("32 桁");
        assert_eq!(
            s.message,
            "API Error: Can't reach the API server — check your internet or DNS (ENOTFOUND)"
        );
    }

    /// #1847 の 1 つ目の型: 狭いペインでフッターが長い（帯・ステータスライン）と、
    /// 末尾 15 行の窓ではエラーの先頭行が押し出される。入力欄を錨にすれば届く
    #[test]
    fn issue2112_フッターが長い狭いペインでも見分ける() {
        // #1847 の 1 つ目の症状の確かめ: 実画面（100 桁 / 32 桁）は既存の判定でも api_error
        for screen in [ENOTFOUND_100, ENOTFOUND_32] {
            assert_eq!(
                crate::orchestrator::wait::detect_worker_error(screen).map(|(k, _)| k),
                Some(crate::orchestrator::wait::WorkerErrorKind::ApiError)
            );
        }
        let footer = (0..14)
            .map(|i| format!("  status line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let screen = format!("{ENOTFOUND_32}\n{footer}");
        assert!(stopped(&screen).is_some());
        // 対比: 既存の末尾 15 行の判定ではこの画面を api_error と読めない
        assert_eq!(
            crate::orchestrator::wait::detect_worker_error(&screen),
            None,
            "既存の窓の判定が変わったなら #1847 の記述を見直す"
        );
    }

    #[test]
    fn issue2112_再試行中_人の発言の後_下書きの画面は止まっていない() {
        assert_eq!(detect(&lines(RETRYING_100)), NetScreen::Clear);
        // 「続き」を送った直後（人の発言が最後の項目）
        let sent = ENOTFOUND_100.replace(
            "✻ Worked for 0s · done 6:55 PM",
            "✻ Worked for 0s · done 6:55 PM\n\n❯ つづき",
        );
        assert_eq!(detect(&lines(&sent)), NetScreen::Clear);
        // 入力欄に人の文字がある画面も「止まっている」とは言う（見送りは駆動側の
        // 下書き判定 = 文字の dim 属性で決める。文字列だけではプレースホルダと区別できない）
        let draft = ENOTFOUND_100.replace("❯ \n", "❯ つづき\n");
        assert!(stopped(&draft).is_some());
    }

    /// 同じ停止は帯・ヒントが変わっても同じ鍵、「続き」の後にまた止まれば別の鍵
    #[test]
    fn issue2112_停止の鍵は同じ停止で変わらず次の停止で変わる() {
        let first = stopped(ENOTFOUND_100).unwrap().key;
        let band = ENOTFOUND_100.replace(
            "  tmux detected · scroll with PgUp/PgDn · or add 'set -g mouse on' to ~/.tmux.conf for wheel scro…",
            "tako | tako: worker | タブ 2 [ compact ]",
        );
        assert_eq!(
            stopped(&band).unwrap().key,
            first,
            "帯・ヒントで鍵が変わった"
        );
        // 32 桁へ縮めても同じ停止
        assert_eq!(
            stopped(ENOTFOUND_32).unwrap().key,
            first,
            "折り返しで鍵が変わった"
        );
        // 「続き」の後にまた止まった実画面（claude 2.1.294）
        let second = "❯ hello

⏺ API Error: Can't reach the API server — check your internet or DNS (ENOTFOUND)

✻ Worked for 0s · done 6:55 PM

❯ つづき

⏺ API Error: Can't reach the API server — check your internet or DNS (ENOTFOUND)

✻ Crunched for 0s · done 7:09 PM

────────────────────────────────────────────────────────────────────────────────────────────────────
❯ 
────────────────────────────────────────────────────────────────────────────────────────────────────
  ⏵⏵ auto mode on (shift+tab to cycle) · ← for agents";
        assert_ne!(stopped(second).unwrap().key, first);
    }

    #[test]
    fn issue2112_旧い版のツール結果の形も受ける() {
        let screen = "⏺ 実装を続けます

  ⎿  API Error: Unable to connect to API (ENOTFOUND api.anthropic.com)

──────────────────────────────
❯
──────────────────────────────";
        assert_eq!(stopped(screen).map(|s| s.kind), Some(ConnErrorKind::Dns));
    }

    /// 回線では直らない停止・接続系でない停止には反応しない
    #[test]
    fn issue2112_使用制限_ログイン失効_入力待ち_サーバー側は止まっていない() {
        let wrap = |body: &str| {
            format!(
                "❯ 続けて\n\n{body}\n\n──────────────────────────────\n❯ \n──────────────────────────────\n  ⏵⏵ auto mode on"
            )
        };
        let cases = [
            // 使用制限（#813 の担当）
            "⏺ You've hit your session limit · resets 3am (Asia/Tokyo)\n  /upgrade to increase your usage limit.",
            // ログイン失効（#757）。接続エラーの後に失効の行が続く実観測の形
            "⏺ API Error: Unable to connect to API (ENOTFOUND)\n  ⎿  OAuth refresh token is no longer valid; run /login to re-authenticate",
            // 失効の行が接続エラーより上（同じターン）にある
            "  ⎿  Login expired · Please run /login\n\n⏺ API Error: Connection refused — a firewall or proxy may be blocking it (ECONNREFUSED)",
            // 人の入力待ち（質問して止まっている）
            "⏺ どちらの方針で進めますか？ A か B を選んでください。",
            // サーバー側の失敗
            "⏺ API Error: 529 Overloaded. This is a server-side issue, usually temporary — try again in a moment.",
        ];
        for body in cases {
            assert_eq!(detect(&lines(&wrap(body))), NetScreen::Clear, "{body}");
        }
    }

    #[test]
    fn issue2112_選択ダイアログ中はダイアログとして返す() {
        let screen =
            "⏺ API Error: Can't reach the API server — check your internet or DNS (ENOTFOUND)

 Select model
 ❯ 1. Default (recommended)
   2. Opus
   3. Sonnet

 Enter to confirm · Esc to exit";
        assert!(matches!(detect(&lines(screen)), NetScreen::Dialog(_)));
    }

    /// 境界: 画面が空・入力欄だけ・項目が 1 つも無い
    #[test]
    fn issue2112_空の画面と項目の無い画面() {
        assert_eq!(detect(&[]), NetScreen::Clear);
        let only_input = "──────────\n❯ \n──────────";
        assert_eq!(detect(&lines(only_input)), NetScreen::Clear);
        // 末尾の窓に項目が無ければ全体を読み直す。項目があれば窓で足りる・入力欄が無ければ読まない
        assert!(needs_full_screen(&lines(only_input)));
        assert!(!needs_full_screen(&lines(ENOTFOUND_100)));
        assert!(!needs_full_screen(&lines(
            "⏺ 作業中\n\n✻ Thinking… (esc to interrupt)"
        )));
    }

    #[test]
    fn issue2112_到達確認は閉じたポートで失敗し_開いたポートで通る() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let target = ProbeTarget {
            host: "127.0.0.1".into(),
            port,
            source: tako_core::net_resume::ProbeSource::Override,
        };
        assert!(probe(&target, Duration::from_secs(2)).is_ok());
        drop(listener);
        assert!(probe(&target, Duration::from_secs(2)).is_err());
        // 名前解決できない名前（.invalid は解決されないことが保証された TLD）
        let bad = ProbeTarget {
            host: "tako-net-2112.invalid".into(),
            port: 443,
            source: tako_core::net_resume::ProbeSource::Override,
        };
        assert!(probe(&bad, Duration::from_secs(2)).is_err());
    }
}
