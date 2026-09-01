//! SSH ペインの接続進行状況（#1010）
//!
//! 「操作したのに何も起きない」時間を作らないための判定。**ここは純粋関数だけ**で、
//! 画面の採取（`TerminalSession::visible_lines`）と時計は呼び出し側（tako-app）が持つ。
//! そのおかげで macOS 上から両プラットフォーム・両言語ぶんを機械検査できる。
//!
//! # なぜ画面 1 本で決めないか
//!
//! 開き方は 3 通りある（#1006）。
//!
//! - `split` / `tab` = 新しいペインで [`crate::remote_fs::ssh_pane_script`] を走らせる。
//!   画面は**まっさら**で、そこに出ている文字は tako が印字したバナーだけ。
//!   なので「tako 以外の行が出た = ssh が何か言った」で判定できる
//! - `pane` = 既存のシェルへ ssh の 1 行を打つ（#640 の送達確認つき経路）。
//!   画面には**それまでの出力とプロンプトが残っている**うえ、打った行は端末幅で
//!   折り返されるので「新しく増えた行」を素直には切り出せない。
//!   そこで ControlMaster のソケット（= 接続が成立して初めて作られる）と
//!   画面が動いたことを併せて見る
//!
//! # 何をもって「もう黙っていない」とするか
//!
//! このインジケータの仕事は**沈黙を覆うこと**なので、パスワードを聞かれた時点でも
//! 役目は終わり（画面に指示が出ている）。逆に ssh 自身が失敗したときは
//! **消さずに理由へ置き換える**（#919 の契約と同じ考え方）

use crate::i18n::Lang;

/// 接続の段階
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectPhase {
    /// まだ何も起きていない = 沈黙。ここだけ「接続中…」を出す
    Connecting,
    /// 繋がった / 相手が何か言った（パスワード等）。次の指示は画面に在るので表示を畳む
    Opened,
    /// ssh 自身が失敗した。**表示は消さず理由に置き換える**
    Failed { reason: Option<String> },
    /// 接続が成立して使えている（#1040 でここから切断を見張る）。表示は畳む
    Connected,
    /// 切れたので自動で繋ぎ直している最中（#1040）。**表示は出し続ける**
    Reconnecting { attempt: u32, waiting_secs: u64 },
    /// 繋ぎ直せずに諦めた（#1040）。理由 + 次の一手を出したまま消さない
    GaveUp,
}

impl ConnectPhase {
    pub fn as_str(&self) -> &'static str {
        match self {
            ConnectPhase::Connecting => "connecting",
            ConnectPhase::Opened => "opened",
            ConnectPhase::Failed { .. } => "failed",
            ConnectPhase::Connected => "connected",
            ConnectPhase::Reconnecting { .. } => "reconnecting",
            ConnectPhase::GaveUp => "gave_up",
        }
    }

    /// まだ画面を覆う必要がある段階か（= 表示を出し続ける）
    pub fn is_visible(&self) -> bool {
        !matches!(self, ConnectPhase::Opened | ConnectPhase::Connected)
    }

    /// このペインを**まだそのホストのターミナルとして数えるか**（#1041）。
    ///
    /// 「フォルダを開いたらターミナルも繋ぐ」が二重にペインを作らないための判定。
    /// `Failed` / `GaveUp` は数えない: 前の試行が死んでいるペインを理由に
    /// 新しい接続を断ると、**ユーザーが開き直しても何も起きない**
    /// （`open` は SFTP で繋がったときにしか来ないので、相手は到達可能）。
    /// 逆に `Connecting` は数える（結果待ちの最中に 2 枚目を作らない）
    pub fn occupies_host(&self) -> bool {
        !matches!(self, ConnectPhase::Failed { .. } | ConnectPhase::GaveUp)
    }
}

/// [`classify`] の材料
#[derive(Debug, Clone)]
pub struct ConnectInputs<'a> {
    /// 走り始めてから画面に増えた行（`fresh_pane` が false のときは
    /// 「打った行が載っている最後の 1 行」も含めて渡してよい）
    pub new_lines: &'a [String],
    /// ControlMaster のソケットが在るか（= このホストへの接続が成立している）
    pub master_socket: bool,
    /// 走り始めたときから画面が動いたか
    pub screen_changed: bool,
    /// tako が印字したものしか載っていないペインか（`split` / `tab` = true）
    pub fresh_pane: bool,
}

/// tako が [`crate::remote_fs::ssh_pane_script`] で印字する行の頭。
/// 日英どちらの文面でも共通なので、これで「tako の行」を言語に依らず見分けられる
pub const TAKO_LINE_PREFIX: &str = "tako: ";

/// スクリプトが出す失敗行に必ず入る文字列（日英どちらの文面にも入っている）
pub const SCRIPT_FAILURE_MARK: &str = "ssh exit 255";

/// ssh 自身が出す失敗行の見分け。**OpenSSH が印字する英語のまま**並べる
/// （ssh の出力はロケールに依らない）。ここに無い理由でも、スクリプト経路なら
/// [`SCRIPT_FAILURE_MARK`] で拾えるので取りこぼしは「表示が畳まれる」だけで済む
const SSH_ERROR_PATTERNS: &[&str] = &[
    "ssh: connect to host",
    "ssh: Could not resolve hostname",
    "Permission denied",
    "Host key verification failed",
    "kex_exchange_identification",
    "Connection closed by",
    "Connection refused",
    "Connection timed out",
    "Operation timed out",
    "No route to host",
    "Network is unreachable",
    "Too many authentication failures",
    "REMOTE HOST IDENTIFICATION HAS CHANGED",
    "Bad configuration option",
    "Bad owner or permissions",
];

/// 相手が入力を待っている行の見分け（小文字化して比べる）。
/// ここまで来たら**沈黙は破れている**ので表示を畳む
const PROMPT_PATTERNS: &[&str] = &[
    "password:",
    "passphrase for key",
    "verification code:",
    "(yes/no",
    "two-factor",
    "one-time password",
];

/// ssh 自身が出す失敗行か（表は [`SSH_ERROR_PATTERNS`] の 1 本だけ）。
/// #1040 の再接続判定が同じ表を引くために公開している
pub fn is_ssh_error_line(line: &str) -> bool {
    SSH_ERROR_PATTERNS.iter().any(|p| line.contains(p))
}

fn is_tako_line(line: &str) -> bool {
    line.trim_start().starts_with(TAKO_LINE_PREFIX)
}

/// 判定（詳細はモジュール doc）
pub fn classify(inputs: &ConnectInputs) -> ConnectPhase {
    // 「tako 以外が書いた中身のある行」だけを見る
    let mut interesting: Vec<&str> = Vec::new();
    for line in inputs.new_lines {
        let trimmed = line.trim_end();
        if trimmed.trim().is_empty() {
            continue;
        }
        // ① tako のスクリプトが出した失敗行 = ssh が exit 255 で落ちた。
        //    理由は**その直前の行**（スクリプトの文面がそう言っている）
        if trimmed.contains(SCRIPT_FAILURE_MARK) {
            let reason = interesting
                .iter()
                .rev()
                .find(|l| !l.trim().is_empty())
                .map(|l| l.trim().to_string());
            return ConnectPhase::Failed { reason };
        }
        if is_tako_line(trimmed) {
            continue;
        }
        interesting.push(trimmed);
    }

    // ② ssh 自身の失敗行（`pane` 経路にはスクリプトが無いのでこちらで拾う）
    for line in &interesting {
        if is_ssh_error_line(line) {
            return ConnectPhase::Failed {
                reason: Some(line.trim().to_string()),
            };
        }
    }

    // ③ 入力待ち = もう黙っていない
    for line in &interesting {
        let lower = line.to_lowercase();
        if PROMPT_PATTERNS.iter().any(|p| lower.contains(p)) {
            return ConnectPhase::Opened;
        }
    }

    // ④ まっさらなペインなら「tako 以外の行が出た」だけで十分
    if inputs.fresh_pane && !interesting.is_empty() {
        return ConnectPhase::Opened;
    }

    // ⑤ 既存シェルの経路は、接続が成立（ソケットが在る）していて画面も動いたら畳む。
    //    ソケットだけで畳まないのは、**ツリーが先に繋いでいると開始前から在る**ため
    //    （その場合でも「打った行が出る」まではまだ沈黙している）
    if inputs.master_socket && inputs.screen_changed {
        return ConnectPhase::Opened;
    }

    ConnectPhase::Connecting
}

/// 覚え始めた時点の「新しく出た行」の起点（`pane` 経路用）。
///
/// 画面は**端末の行数ぶん常に返ってくる**（空行込みなので行数は変わらない）ので、
/// 行数の差では切り出せない。打った行はプロンプト（= 最後の非空行）の**続き**に
/// 載るので、そこを起点にすると「打った行 + そのあとに出たもの」だけを見られる。
///
/// 全部空なら 0（= 画面全体を見る）
pub fn baseline_index(lines: &[String]) -> usize {
    lines
        .iter()
        .rposition(|l| !l.trim().is_empty())
        .unwrap_or(0)
}

/// これ以上待っても意味が無い秒数。ここを越えたら**失敗を騙らず**表示を畳む
/// （ssh が黙ったまま生き続ける形はあり得るので、居座るチップを作らない）
pub const SILENT_CAP_SECS: u64 = 120;

/// 表示を諦める（畳む）か
pub fn give_up(elapsed_secs: u64) -> bool {
    elapsed_secs >= SILENT_CAP_SECS
}

/// 経過の見せ方（`3s` / `1m20s`）。**秒が動くので「生きている」ことが伝わる**
pub fn elapsed_label(secs: u64) -> String {
    if secs < 60 {
        format!("{secs}s")
    } else {
        format!("{}m{:02}s", secs / 60, secs % 60)
    }
}

/// 失敗の理由が読めなかったときに添える一言（言語つき）。
/// 理由そのものはペインの画面に出ているので、そこを見るよう促す
pub fn reason_fallback(lang: Lang) -> &'static str {
    match lang {
        Lang::Ja => "理由はペインの表示を確認してください",
        Lang::En => "See the pane for the reason",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    fn inputs<'a>(new_lines: &'a [String], fresh: bool) -> ConnectInputs<'a> {
        ConnectInputs {
            new_lines,
            master_socket: false,
            screen_changed: false,
            fresh_pane: fresh,
        }
    }

    #[test]
    fn バナーだけの間は接続中のまま() {
        let l = lines(&["tako: win へ接続しています…（中止は Ctrl+C）", "", "   "]);
        assert_eq!(classify(&inputs(&l, true)), ConnectPhase::Connecting);
    }

    #[test]
    fn 英語のバナーでも接続中のまま() {
        let l = lines(&["tako: connecting to win… (Ctrl+C to cancel)"]);
        assert_eq!(classify(&inputs(&l, true)), ConnectPhase::Connecting);
    }

    #[test]
    fn まっさらなペインはtako以外の行が出たら畳む() {
        let l = lines(&[
            "tako: win へ接続しています…（中止は Ctrl+C）",
            "Last login: Thu Aug 28 09:20:11 2026 from 10.x.x.x",
        ]);
        assert_eq!(classify(&inputs(&l, true)), ConnectPhase::Opened);
    }

    #[test]
    fn スクリプトの失敗行は直前の行を理由にする() {
        // #919 の文面が「理由は上の行です」と言っているのと同じ切り出し
        let l = lines(&[
            "tako: win へ接続しています…（中止は Ctrl+C）",
            "ssh: connect to host win port 22: Operation timed out",
            "tako: win への接続に失敗しました（ssh exit 255）。理由は上の行です",
            "tako: ネットワーク（VPN / Tailscale）・相手の電源・~/.ssh/config を確認してください",
        ]);
        assert_eq!(
            classify(&inputs(&l, true)),
            ConnectPhase::Failed {
                reason: Some("ssh: connect to host win port 22: Operation timed out".into())
            }
        );
    }

    #[test]
    fn 英語の失敗行でも同じ切り出しになる() {
        let l = lines(&[
            "tako: connecting to win… (Ctrl+C to cancel)",
            "ssh: Could not resolve hostname win: nodename nor servname provided",
            "tako: could not connect to win (ssh exit 255). The reason is printed above",
        ]);
        match classify(&inputs(&l, true)) {
            ConnectPhase::Failed { reason } => {
                assert!(reason.unwrap().contains("Could not resolve hostname"))
            }
            other => panic!("失敗として読めていない: {other:?}"),
        }
    }

    #[test]
    fn 既存シェル経路はssh自身の失敗行で理由が出る() {
        // `pane` 経路にはスクリプトが無い（= マーカーも出ない）
        let l = lines(&[
            "user@mac ~ % ssh -o ControlPath=... win",
            "ssh: connect to host win port 22: Connection refused",
            "user@mac ~ %",
        ]);
        assert_eq!(
            classify(&inputs(&l, false)),
            ConnectPhase::Failed {
                reason: Some("ssh: connect to host win port 22: Connection refused".into())
            }
        );
    }

    #[test]
    fn 既存シェル経路はプロンプトが残っていても接続中のまま() {
        // 打った行そのもの（折り返しても）を「繋がった」と読まない
        let l = lines(&[
            "user@mac ~ % ssh -o ControlPath=\"/Users/testuser/Library/Application Su",
            "pport/tako/ssh/win-0123456789abcdef\" -o ControlMaster=auto win",
        ]);
        assert_eq!(classify(&inputs(&l, false)), ConnectPhase::Connecting);
    }

    #[test]
    fn 既存シェル経路はソケットと画面の変化が揃って初めて畳む() {
        let l = lines(&["user@mac ~ % ssh win"]);
        let mut i = inputs(&l, false);
        i.master_socket = true;
        assert_eq!(classify(&i), ConnectPhase::Connecting, "画面が動いていない");
        i.screen_changed = true;
        assert_eq!(classify(&i), ConnectPhase::Opened);
    }

    #[test]
    fn パスワードを聞かれたら沈黙は破れている() {
        let l = lines(&["testuser@win's password:"]);
        assert_eq!(classify(&inputs(&l, false)), ConnectPhase::Opened);
    }

    #[test]
    fn 鍵の確認プロンプトも沈黙ではない() {
        let l = lines(&[
            "The authenticity of host 'win (10.x.x.x)' can't be established.",
            "Are you sure you want to continue connecting (yes/no/[fingerprint])?",
        ]);
        assert_eq!(classify(&inputs(&l, false)), ConnectPhase::Opened);
    }

    #[test]
    fn 認証失敗は理由として読める() {
        let l = lines(&["testuser@win: Permission denied (publickey)."]);
        match classify(&inputs(&l, false)) {
            ConnectPhase::Failed { reason } => {
                assert!(reason.unwrap().contains("Permission denied"))
            }
            other => panic!("失敗として読めていない: {other:?}"),
        }
    }

    #[test]
    fn 理由が無い失敗もマーカーだけで失敗になる() {
        let l = lines(&["tako: win への接続に失敗しました（ssh exit 255）。理由は上の行です"]);
        assert_eq!(
            classify(&inputs(&l, true)),
            ConnectPhase::Failed { reason: None }
        );
    }

    #[test]
    fn 起点はプロンプト行になる() {
        // 画面は端末の行数ぶん返る（後ろは空行）。行数では切り出せないので
        // 「最後の非空行」を起点にする
        let l = lines(&["$ ls", "a.txt  b.txt", "$", "", "", ""]);
        assert_eq!(baseline_index(&l), 2);
        // 打った行が載るのはその行なので、そこから見れば自分の行も新しい行も入る
        assert_eq!(
            &l[baseline_index(&l)..],
            &["$".to_string(), String::new(), String::new(), String::new()]
        );
    }

    #[test]
    fn まっさらな画面の起点は先頭() {
        let l = lines(&["", "", ""]);
        assert_eq!(baseline_index(&l), 0);
    }

    #[test]
    fn 経過の見せ方() {
        assert_eq!(elapsed_label(0), "0s");
        assert_eq!(elapsed_label(59), "59s");
        assert_eq!(elapsed_label(60), "1m00s");
        assert_eq!(elapsed_label(3671), "61m11s");
    }

    #[test]
    fn 諦める境目() {
        assert!(!give_up(SILENT_CAP_SECS - 1));
        assert!(give_up(SILENT_CAP_SECS));
    }

    #[test]
    fn 表示を続ける段階は接続中と失敗だけ() {
        assert!(ConnectPhase::Connecting.is_visible());
        assert!(ConnectPhase::Failed { reason: None }.is_visible());
        assert!(!ConnectPhase::Opened.is_visible());
        // #1040: 使えている間は畳み、繋ぎ直している間と諦めたあとは出し続ける
        assert!(!ConnectPhase::Connected.is_visible());
        assert!(ConnectPhase::Reconnecting {
            attempt: 1,
            waiting_secs: 2
        }
        .is_visible());
        assert!(ConnectPhase::GaveUp.is_visible());
    }

    #[test]
    fn 段階の名前は重複しない() {
        // `tako list` / `read` の `phase` はこの文字列がそのまま出るので、
        // 増やしたときに被っていないことを機械で確かめる（AI が読む値）
        let all = [
            ConnectPhase::Connecting.as_str(),
            ConnectPhase::Opened.as_str(),
            ConnectPhase::Failed { reason: None }.as_str(),
            ConnectPhase::Connected.as_str(),
            ConnectPhase::Reconnecting {
                attempt: 1,
                waiting_secs: 0,
            }
            .as_str(),
            ConnectPhase::GaveUp.as_str(),
        ];
        let mut sorted = all.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), all.len(), "phase の名前が被っている: {all:?}");
    }
}

#[cfg(test)]
mod occupies_host_tests {
    use super::*;

    /// #1041: 生きている / 結果待ちのペインは数え、死んだものは数えない
    #[test]
    fn 死んだ接続はホストを占有しない() {
        for live in [
            ConnectPhase::Connecting,
            ConnectPhase::Opened,
            ConnectPhase::Connected,
            ConnectPhase::Reconnecting {
                attempt: 1,
                waiting_secs: 2,
            },
        ] {
            assert!(live.occupies_host(), "{}", live.as_str());
        }
        for dead in [ConnectPhase::Failed { reason: None }, ConnectPhase::GaveUp] {
            assert!(!dead.occupies_host(), "{}", dead.as_str());
        }
    }
}
