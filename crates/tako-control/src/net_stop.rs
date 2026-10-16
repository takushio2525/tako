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
//!
//! 前の 2 つは「会話が先へ進んだ」とはっきり言える画面なので [`NetScreen::Progressed`] で返す
//! （前に送った停止は片付いた = 次に見える停止は、鍵が同じでも新しい停止。#2157）。
//!
//! # ターンの締めの後の通知行（#2157）
//!
//! claude はターンの外の出来事（plugin / hooks の再読み込み等）を `⏺` の項目として会話の末尾へ
//! 足す（`⏺ tako: plugin.json changed — reloaded …`）。実際にエラーの 0.2 秒後に出て、最後の項目が
//! 通知になったペインを取りこぼした。ターンの締めの行（`✻ Cooked for 31m 35s · done 9:20 AM`）より
//! 下が `⏺` の項目だけなら、それはターンの外の通知なので読み飛ばし、締めの行の上の項目を見る。
//! 人の発言（`❯`）・ツールの結果（`⎿`）が混ざれば会話が進んでいるので読み飛ばさない。
//! 新しいターンは必ず締めの行で終わるか、生成中（中断の案内）なので、締めの行の無いまま
//! assistant の発言が残ることはない

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
    /// 会話が先へ進んでいる（最後の項目が人の発言・生成中 / 再試行中）。送らないことは
    /// `Clear` と同じで、加えて「前の停止は片付いた」と言える（#2157）
    Progressed,
    /// 該当しない（読めない・接続系でない停止を含む）
    Clear,
}

/// 会話の項目の頭（claude の発言 = `⏺`（Windows は `●`）・ツールの結果 = `⎿`・人の発言 = `❯`）
const ENTRY_MARKS: [char; 4] = ['⏺', '●', '⎿', '❯'];

/// claude の発言の頭（ターンの外の通知もこの記号で出る）
const SAY_MARKS: [char; 2] = ['⏺', '●'];

/// ターンの締めの行の頭の記号（claude のスピナーの字形。Windows の版は `*` / `·`）
const CLOSING_GLYPHS: [char; 7] = ['✻', '✶', '✳', '✢', '✽', '*', '·'];

/// #2157 の A/B。`TAKO_2157_LEGACY=1` で #2157 **前**の判定へ戻す
/// （ターンの締めの後の通知行を読み飛ばさない・中断の案内を末尾の窓の全体で探す・
/// 会話が進んだと言わない = 前に送った停止と同じ形の次の停止を「同じ停止」とみなす）。
/// 2 秒 tick から引かれるので env はプロセスで 1 回だけ読む
pub fn legacy_2157() -> bool {
    static LEGACY: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *LEGACY.get_or_init(|| std::env::var("TAKO_2157_LEGACY").is_ok_and(|v| v == "1"))
}

/// 画面（上から順の行）を判定する
pub fn detect(lines: &[String]) -> NetScreen {
    detect_with(lines, legacy_2157())
}

fn detect_with(lines: &[String], legacy: bool) -> NetScreen {
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
    if legacy && crate::claude_tui::interrupt_hint_visible(lines) {
        return NetScreen::Clear;
    }
    // 入力欄の上の区切り線（input - 1）より上で、最後の会話の項目を探す
    let top = input - 1;
    let Some(last) = (0..top).rev().find(|&i| is_entry(&lines[i])) else {
        return NetScreen::Clear;
    };
    // 中断の案内は最後の項目から下（スピナーの行・フッター）でだけ探す。会話の本文
    // （ツールの結果に写った `esc to interrupt` 等）を理由に止まった画面を見落とさない（#2157）
    if !legacy && crate::claude_tui::interrupt_hint_visible(&lines[last..]) {
        return NetScreen::Progressed;
    }
    if lines[last].trim_start().starts_with('❯') {
        // 人の発言が最後 = もう送られている
        return if legacy {
            NetScreen::Clear
        } else {
            NetScreen::Progressed
        };
    }
    let mut entry = last;
    let mut message = join_entry(&lines[last..top]);
    let mut kind = connection_error_kind(&message);
    // 最後の項目がターンの外の通知なら、締めの行の上の項目を見る（#2157）
    if kind.is_none() && !legacy {
        if let Some(e) = entry_before_notices(lines, top) {
            let m = join_entry(&lines[e..top]);
            if let Some(k) = connection_error_kind(&m) {
                (entry, message, kind) = (e, m, Some(k));
            }
        }
    }
    let Some(kind) = kind else {
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

/// ターンの締めの行（`✻ Cooked for 31m 35s · done 9:20 AM` / `✻ Worked for 0s · done 6:55 PM`）か。
/// スピナーの記号 1 字・動詞 1 語・`for`・数字で始まる経過時間、の並びだけを採る
/// （`⏺ Waited for 5s` のような発言・箇条書きの本文を締めと取り違えない）
fn is_turn_closing(t: &str) -> bool {
    let mut w = t.split_whitespace();
    let (Some(glyph), Some(verb), Some(word), Some(took)) =
        (w.next(), w.next(), w.next(), w.next())
    else {
        return false;
    };
    let mut g = glyph.chars();
    g.next().is_some_and(|c| CLOSING_GLYPHS.contains(&c))
        && g.next().is_none()
        && verb.chars().all(char::is_alphabetic)
        && word == "for"
        && took.starts_with(|c: char| c.is_ascii_digit())
}

/// 入力欄の上（`top` より上）が「…項目 / ターンの締めの行 / claude の記号の項目（通知）だけ」の
/// 形なら、締めの行の上の最後の項目の位置を返す（#2157）。締めの行より下に人の発言・
/// ツールの結果がある・締めの行が無い・締めの行の下に何も無いなら None
fn entry_before_notices(lines: &[String], top: usize) -> Option<usize> {
    let mut notices = 0usize;
    for i in (0..top).rev() {
        let t = lines[i].trim();
        if t.is_empty() {
            continue;
        }
        if is_turn_closing(t) {
            if notices == 0 {
                return None;
            }
            let entry = (0..i).rev().find(|&j| is_entry(&lines[j]))?;
            return (!lines[entry].trim_start().starts_with('❯')).then_some(entry);
        }
        if is_entry(&lines[i]) {
            if !t.starts_with(SAY_MARKS) {
                return None;
            }
            notices += 1;
        }
        // それ以外は項目の続きの行（折り返し）
    }
    None
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
        // #2157 から「止まっていない」のうち会話が進んだとはっきり言える画面は Progressed
        // （送らないことは Clear と同じ。旧判定では Clear）
        assert_eq!(
            detect_with(&lines(RETRYING_100), false),
            NetScreen::Progressed
        );
        assert_eq!(detect_with(&lines(RETRYING_100), true), NetScreen::Clear);
        // 「続き」を送った直後（人の発言が最後の項目）
        let sent = ENOTFOUND_100.replace(
            "✻ Worked for 0s · done 6:55 PM",
            "✻ Worked for 0s · done 6:55 PM\n\n❯ つづき",
        );
        assert_eq!(detect_with(&lines(&sent), false), NetScreen::Progressed);
        assert_eq!(detect_with(&lines(&sent), true), NetScreen::Clear);
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

    /// 60 桁の区切り線と入力欄・フッター（#2157 の実画面の幅）
    fn footer60() -> String {
        let rule = "─".repeat(60);
        format!("{rule}\n❯ \n{rule}\n  ⏵⏵ auto mode on (shift+tab to cycle)")
    }

    /// pane 2670 で取りこぼした実画面（2026-10-16 09:20 JST。60 桁・claude 2.1.294）。
    /// 会話記録では API エラーの 0.2 秒後に `system informational`（level=notice）の
    /// `tako: plugin.json changed — reloaded` が足され、最初の 2 秒 tick の時点で最後の項目は
    /// もう通知だった。直前の人の発言は架空の文へ置き換えた
    fn screen_2670(notices: &str) -> String {
        format!(
            "❯ 実装を続けて\n\n\
             ⏺ API Error: Can't reach the API server — check your\n  internet or DNS (ENOTFOUND)\n\n\
             ✻ Cooked for 31m 35s · done 9:20 AM\n\n\
             {notices}{}",
            footer60()
        )
    }

    const NOTICE_PLUGIN: &str = "⏺ tako: plugin.json changed — reloaded (modules:\n  hooks/register.ts unchanged; 14 hooks: …)\n";
    const NOTICE_HOOKS: &str = "⏺ tako: hooks.json changed — reloaded (modules:\n  hooks/register.ts unchanged; 14 hooks: …)\n";

    #[test]
    fn issue2157_締めの後の通知行を読み飛ばして接続エラーの停止と見分ける() {
        let screen = screen_2670(&format!("{NOTICE_PLUGIN}\n"));
        let NetScreen::Stopped(s) = detect_with(&lines(&screen), false) else {
            panic!("通知行が挟まった実画面を止まっていると読めない");
        };
        assert_eq!(s.kind, ConnErrorKind::Dns);
        assert_eq!(
            s.message,
            "API Error: Can't reach the API server — check your internet or DNS (ENOTFOUND)"
        );
        // 旧判定（#2157 前）はこの画面を読めなかった = 取りこぼしの原因
        assert_eq!(detect_with(&lines(&screen), true), NetScreen::Clear);
        // 通知が出る前と後で同じ停止（同じ鍵）。出る前に見ていても 2 回目の停止と取り違えない
        let before = screen_2670("");
        let NetScreen::Stopped(b) = detect_with(&lines(&before), false) else {
            panic!("通知が出る前の画面");
        };
        assert_eq!(b.key, s.key);
        // 通知が 2 つ（plugin.json と hooks.json）・空行なしで続いても同じ
        let two = screen_2670(&format!("{NOTICE_PLUGIN}\n{NOTICE_HOOKS}"));
        assert!(matches!(
            detect_with(&lines(&two), false),
            NetScreen::Stopped(t) if t.key == s.key
        ));
    }

    /// 本当に会話が先へ進んだ画面・締めの行で区切れない画面は、通知行の読み飛ばしの対象にしない
    #[test]
    fn issue2157_会話が進んだ画面は通知と取り違えない() {
        let reply = "❯ つづき\n\n⏺ 続けます。\n\n✻ Worked for 12s · done 9:41 AM\n\n";
        let cases: [(&str, String, NetScreen); 6] = [
            // エラーの後に人の発言と assistant の発言（新しいターンの締めの後に通知）
            (
                "人の発言と返答の後の通知",
                screen_2670(&format!("{reply}{NOTICE_PLUGIN}")),
                NetScreen::Clear,
            ),
            // 締めの後にツールの呼び出しと結果（新しいターンの途中・締めがまだ無い）
            (
                "締めの後のツールの結果",
                screen_2670("⏺ Bash(ls)\n  ⎿  a.txt\n"),
                NetScreen::Clear,
            ),
            // 通知の後で生成中（中断の案内）
            (
                "通知の後で生成中",
                screen_2670(&format!(
                    "{NOTICE_PLUGIN}\n✻ Thinking… (esc to interrupt)\n\n"
                )),
                NetScreen::Progressed,
            ),
            // 通知の後に人の発言（送信済み）
            (
                "通知の後に人の発言",
                screen_2670(&format!("{NOTICE_PLUGIN}\n❯ つづき\n\n")),
                NetScreen::Progressed,
            ),
            // 締めの後の項目が回線では直らない停止（ログイン失効）
            (
                "締めの後のログイン失効",
                screen_2670("⏺ Login expired · Please run /login\n"),
                NetScreen::Clear,
            ),
            // 発言の本文が「記号 動詞 for 数字」の形（締めの行と取り違えない）
            (
                "締めに似た発言",
                "❯ 実装を続けて\n\n⏺ API Error: Can't reach the API server — check your internet or DNS (ENOTFOUND)\n\n⏺ Waited for 5s before retrying\n\n⏺ 待ちます\n"
                    .to_string()
                    + &footer60(),
                NetScreen::Clear,
            ),
        ];
        for (name, screen, want) in cases {
            assert_eq!(detect_with(&lines(&screen), false), want, "{name}");
        }
        // 締めの行が無いまま通知（エラーとの間に締めが無い）は読み飛ばさない（安全側）
        let no_closing = format!(
            "❯ 実装を続けて\n\n⏺ API Error: Can't reach the API server — check your internet or DNS (ENOTFOUND)\n\n{NOTICE_PLUGIN}{}",
            footer60()
        );
        assert_eq!(detect_with(&lines(&no_closing), false), NetScreen::Clear);
    }

    /// 中断の案内は最後の項目から下でだけ探す。会話の本文（ツールの結果に写った文言）で
    /// 止まった画面を見落とさない
    #[test]
    fn issue2157_本文に写った中断の案内では止まった画面を見落とさない() {
        let screen = format!(
            "❯ 判定を読んで\n\n⏺ Read(claude_tui.rs)\n  ⎿  l.contains(\"esc to interrupt\") || l.contains(\"Generating\")\n\n\
             ⏺ API Error: Can't reach the API server — check your internet or DNS (ENOTFOUND)\n\n\
             ✻ Worked for 3m 2s · done 9:41 AM\n\n{}",
            footer60()
        );
        assert!(matches!(
            detect_with(&lines(&screen), false),
            NetScreen::Stopped(_)
        ));
        assert_eq!(detect_with(&lines(&screen), true), NetScreen::Clear);
        // スピナーの行・フッターにあれば従来どおり生成中
        let generating = screen.replace(
            "  ⏵⏵ auto mode on (shift+tab to cycle)",
            "  ⏵⏵ auto mode on (shift+tab to cycle) · esc to interrupt",
        );
        assert_eq!(
            detect_with(&lines(&generating), false),
            NetScreen::Progressed
        );
    }

    #[test]
    fn issue2157_締めの行の形() {
        for t in [
            "✻ Cooked for 31m 35s · done 9:20 AM",
            "✻ Worked for 0s · done 6:55 PM",
            "✻ Crunched for 0s · done 7:09 PM",
            "✻ Sautéed for 2m 1s",
            "* Worked for 4s",
        ] {
            assert!(is_turn_closing(t), "{t}");
        }
        for t in [
            "⏺ Waited for 5s",
            "✻ Thinking… (esc to interrupt)",
            "- Worked for 4s",
            "✻ Worked for long",
            "✻✻ Worked for 4s",
            "✻ Worked",
            "",
        ] {
            assert!(!is_turn_closing(t), "{t}");
        }
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
