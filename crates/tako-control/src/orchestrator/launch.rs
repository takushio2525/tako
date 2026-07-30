//! spawn の起動保証（Issue #665）。
//!
//! spawn は「ペインを割って起動コマンドを流し、プロンプトを積む」までしかせず、
//! それが**実際に届いたか**を誰も確認していなかった（#640 の 6 時間空回りの温床）。
//! ここでは起動を段階に分け、各段階を画面から検証し、届いていなければ再送する
//! ための語彙と純粋関数を定義する。
//!
//! 設計方針:
//! - 画面分類は **GUI 非依存の純粋関数**にする。tako-app の状態機械はこれを呼ぶだけで、
//!   判定そのものは `cargo test` で検証できる（#515 と同じ方針）
//! - 「エージェントが起動している」判定を最優先する。起動済みのペインへ起動コマンドを
//!   再送すると、エージェントの入力欄へゴミを打ち込むことになるため、
//!   再送は**シェルプロンプトが最終行に見えている**ときだけに限る
//! - 段階はレジストリ（workers.yaml）へ書き、プロセス外（CLI / MCP）から読めるようにする

use serde::{Deserialize, Serialize};

/// 起動保証の段階。数値が大きいほど先へ進んでいる（`rank`）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LaunchPhase {
    /// 発射指示を受け取った（ペインはまだ起動していないかもしれない）
    Queued,
    /// ペインのシェルが動き出した（画面に出力が出た）
    ShellReady,
    /// 起動コマンドを書き込んだ
    LaunchSent,
    /// エージェント CLI の起動を画面で確認した
    AgentStarted,
    /// プロンプトの送達フローを開始した
    PromptSent,
    /// プロンプトが入力欄から消えた = 送達を確認した
    PromptDelivered,
    /// 保証に失敗した（再送を使い切った / 起動コマンドが存在しない等）
    Failed,
}

impl LaunchPhase {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::ShellReady => "shell_ready",
            Self::LaunchSent => "launch_sent",
            Self::AgentStarted => "agent_started",
            Self::PromptSent => "prompt_sent",
            Self::PromptDelivered => "prompt_delivered",
            Self::Failed => "failed",
        }
    }

    pub fn from_slug(s: &str) -> Option<Self> {
        match s {
            "queued" => Some(Self::Queued),
            "shell_ready" => Some(Self::ShellReady),
            "launch_sent" => Some(Self::LaunchSent),
            "agent_started" => Some(Self::AgentStarted),
            "prompt_sent" => Some(Self::PromptSent),
            "prompt_delivered" => Some(Self::PromptDelivered),
            "failed" => Some(Self::Failed),
            _ => None,
        }
    }

    /// 進捗の順序。`Failed` は 0（どの段階よりも「進んでいない」扱い）
    pub fn rank(self) -> u8 {
        match self {
            Self::Failed => 0,
            Self::Queued => 1,
            Self::ShellReady => 2,
            Self::LaunchSent => 3,
            Self::AgentStarted => 4,
            Self::PromptSent => 5,
            Self::PromptDelivered => 6,
        }
    }

    /// これ以上遷移しない段階か（呼び出し側のポーリング終了条件）
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::PromptDelivered | Self::Failed)
    }

    /// 「worker は実際に動き出した」と言えるか。
    /// `PromptSent` 止まりでも起動自体は確認できているため true にする
    pub fn agent_running(self) -> bool {
        self.rank() >= Self::AgentStarted.rank()
    }

    /// master 向けの日本語説明（CLI / MCP のエラー文面に載せる）
    pub fn describe(self) -> &'static str {
        match self {
            Self::Queued => "ペインの起動待ち（シェルがまだ動き出していない）",
            Self::ShellReady => "シェルは動いたが起動コマンドをまだ送っていない",
            Self::LaunchSent => "起動コマンドは送ったがエージェント CLI の起動を確認できていない",
            Self::AgentStarted => "エージェント CLI は起動したがプロンプトをまだ送っていない",
            Self::PromptSent => "プロンプトを送ったが入力欄からの消失を確認できていない",
            Self::PromptDelivered => "エージェント CLI の起動とプロンプトの送達を確認した",
            Self::Failed => "起動に失敗した",
        }
    }
}

/// 画面から読み取った起動状況（`classify_launch_screen` の返り値）
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LaunchScreen {
    /// エージェント CLI が起動している（入力欄 / 信頼ダイアログ / 生成中表示）
    AgentReady,
    /// 起動コマンドが失敗した（コマンドが見つからない等）。再送しても直らない
    LaunchError { detail: String },
    /// シェルのプロンプトが最終行に見えている = 起動コマンドが実行されていない、
    /// または実行されたが即終了した。**再送してよい唯一の状態**
    ShellPrompt,
    /// まだ判断できない（出力なし / 起動処理の途中）
    Unknown,
}

/// 起動コマンドが「そもそも実行できなかった」ことを示す行のパターン。
/// 日本語 Windows の PowerShell はメッセージ全体が日本語になるため、
/// 英語版と日本語版の両方を拾う（実機の文面より）
const LAUNCH_ERROR_PATTERNS: &[&str] = &[
    // 英語シェル共通。文言はバージョンで揺れるので語尾まで固定しない:
    //   PowerShell 5.1: 「is not recognized as **the** name of a cmdlet, ...」
    //   PowerShell 7.6: 「is not recognized as **a** name of a cmdlet, ...」（実測）
    //   cmd.exe:        「is not recognized as an internal or external command」
    // 「the name」で固定していたため PowerShell 7 の実機で検出できず、
    // 無駄な再送を 3 回してから失敗していた（#665 の E2E で発覚）
    "is not recognized as",
    "commandnotfoundexception",
    // POSIX シェル
    "command not found",
    "no such file or directory",
    // PowerShell（日本語）: 「用語 '...' は、コマンドレット、関数、... として認識されません。」
    "として認識されません",
    "認識されませんでした",
];

/// 画面（可視行）からエージェント CLI の起動状況を判定する。
///
/// 判定順序が重要:
/// 1. **エージェントの起動を最優先**で判定する（起動済みへの再送は入力欄を汚す）
/// 2. 次に起動エラー（再送しても直らないので即失敗にする）
/// 3. 最後にシェルプロンプト（= 再送してよい状態）
pub fn classify_launch_screen(lines: &[String]) -> LaunchScreen {
    if agent_markers_present(lines) {
        return LaunchScreen::AgentReady;
    }
    if let Some(detail) = launch_error_line(lines) {
        return LaunchScreen::LaunchError { detail };
    }
    match last_non_empty(lines) {
        Some(line) if looks_like_shell_prompt(line) => LaunchScreen::ShellPrompt,
        _ => LaunchScreen::Unknown,
    }
}

/// エージェント CLI が画面に出ているか（入力欄 / 信頼ダイアログ / 生成中表示）
fn agent_markers_present(lines: &[String]) -> bool {
    crate::claude_tui::is_trust_dialog(lines)
        || crate::claude_tui::input_line(lines).is_some()
        || crate::claude_tui::is_busy(lines)
}

/// 末尾側から起動コマンドの失敗を探す。見つかった箇所を detail として返す。
///
/// **行単位の照合だけでは足りない**: シェルのエラーメッセージはペイン幅で折り返され、
/// 「…として認識されません。」のようなパターンが行境界をまたぐ（実測: 60 桁の
/// ペインで日本語 PowerShell のメッセージが 2 行に割れ、検出できなかった）。
/// 連結してからも照合する
fn launch_error_line(lines: &[String]) -> Option<String> {
    // **空行を捨ててから 12 行取る**。順序を逆にすると、画面下部が空行だらけの
    // ペイン（起動直後はこれが普通）で窓が空行だけになり、上部に出ている
    // エラーメッセージを見逃す（実測: PowerShell のエラーを検出できず 3 回再送した）
    let tail: Vec<&str> = lines
        .iter()
        .rev()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .take(12)
        .collect();

    // ① 行単位（detail に該当行をそのまま載せられる）
    if let Some(line) = tail.iter().find(|l| contains_launch_error(l)) {
        return Some(line.to_string());
    }

    // ② 折り返し対策: 末尾を（古い順に戻して）連結してから照合する。
    //    ハードラップは単語の途中で割れるので区切りを入れずに繋ぐ
    let joined: String = tail.iter().rev().copied().collect::<Vec<_>>().concat();
    if contains_launch_error(&joined) {
        // 折り返しで割れているぶん、どの行かは特定できない。末尾側をまとめて返す
        return Some(truncate_chars(&joined, 200));
    }
    None
}

fn contains_launch_error(text: &str) -> bool {
    let lower = text.to_lowercase();
    LAUNCH_ERROR_PATTERNS.iter().any(|p| lower.contains(p))
}

/// 文字境界を壊さずに先頭 n 文字へ切り詰める
fn truncate_chars(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

fn last_non_empty(lines: &[String]) -> Option<&str> {
    lines
        .iter()
        .rev()
        .map(|l| l.trim_end())
        .find(|l| !l.trim().is_empty())
}

/// 1 行がシェルのプロンプトに見えるか。
///
/// **偽陽性は再送 = 入力欄汚染につながる**ため保守的に判定する。
/// 偽陰性（見逃し）は再送タイムアウトで救済されるので許容できる
fn looks_like_shell_prompt(line: &str) -> bool {
    let t = line.trim();
    if t.is_empty() {
        return false;
    }
    // PowerShell: 「PS C:\Users\x\dev\tako>」（末尾にコマンドが続く場合は入力途中なので対象外）
    if t.starts_with("PS ") && t.ends_with('>') {
        return true;
    }
    // cmd.exe: 「C:\Users\x>」
    if t.ends_with('>') && t.contains(":\\") {
        return true;
    }
    // POSIX シェル: 「user@host:~/dev$」「%」「#」
    matches!(t.chars().last(), Some('$') | Some('%') | Some('#'))
}

/// レジストリ（workers.yaml）に残す起動保証の記録
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct LaunchRecord {
    /// `LaunchPhase` の slug
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub phase: String,
    /// 起動コマンドの送信回数（初回 + 再送）
    #[serde(default)]
    pub attempts: u32,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub updated_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

impl LaunchRecord {
    pub fn phase(&self) -> Option<LaunchPhase> {
        LaunchPhase::from_slug(&self.phase)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn phase_rank_is_monotonic() {
        let order = [
            LaunchPhase::Queued,
            LaunchPhase::ShellReady,
            LaunchPhase::LaunchSent,
            LaunchPhase::AgentStarted,
            LaunchPhase::PromptSent,
            LaunchPhase::PromptDelivered,
        ];
        for w in order.windows(2) {
            assert!(
                w[0].rank() < w[1].rank(),
                "{:?} は {:?} より前のはず",
                w[0],
                w[1]
            );
        }
        // Failed はどの段階よりも「進んでいない」
        assert_eq!(LaunchPhase::Failed.rank(), 0);
        assert!(LaunchPhase::Failed.is_terminal());
        assert!(LaunchPhase::PromptDelivered.is_terminal());
        assert!(!LaunchPhase::LaunchSent.is_terminal());
    }

    #[test]
    fn phase_slug_roundtrip() {
        for p in [
            LaunchPhase::Queued,
            LaunchPhase::ShellReady,
            LaunchPhase::LaunchSent,
            LaunchPhase::AgentStarted,
            LaunchPhase::PromptSent,
            LaunchPhase::PromptDelivered,
            LaunchPhase::Failed,
        ] {
            assert_eq!(LaunchPhase::from_slug(p.as_str()), Some(p));
        }
        assert_eq!(LaunchPhase::from_slug("nope"), None);
    }

    #[test]
    fn agent_running_starts_at_agent_started() {
        assert!(!LaunchPhase::LaunchSent.agent_running());
        assert!(LaunchPhase::AgentStarted.agent_running());
        assert!(LaunchPhase::PromptDelivered.agent_running());
        assert!(!LaunchPhase::Failed.agent_running());
    }

    #[test]
    fn powershell_prompt_is_shell_prompt() {
        // #640 の症状そのもの: 起動コマンドが届かず素の PowerShell のまま
        let screen = lines(&["Windows PowerShell", "", "PS C:\\Users\\x\\dev\\tako>"]);
        assert_eq!(classify_launch_screen(&screen), LaunchScreen::ShellPrompt);
    }

    #[test]
    fn cmd_and_posix_prompts_are_shell_prompts() {
        assert_eq!(
            classify_launch_screen(&lines(&["C:\\Users\\x>"])),
            LaunchScreen::ShellPrompt
        );
        assert_eq!(
            classify_launch_screen(&lines(&["user@host:~/dev/tako$"])),
            LaunchScreen::ShellPrompt
        );
        assert_eq!(
            classify_launch_screen(&lines(&["host%"])),
            LaunchScreen::ShellPrompt
        );
    }

    #[test]
    fn agent_input_line_beats_shell_prompt() {
        // 起動済みペインを ShellPrompt と誤判定すると再送で入力欄を汚す。
        // シェルプロンプトの残骸が上に残っていてもエージェント優先で判定する
        let screen = lines(&["PS C:\\Users\\x\\dev\\tako> claude --model opus", "", "❯ "]);
        assert_eq!(classify_launch_screen(&screen), LaunchScreen::AgentReady);
    }

    #[test]
    fn trust_dialog_counts_as_agent_ready() {
        let screen = lines(&[
            "Do you trust the files in this folder?",
            "  1. Yes, proceed",
            "  2. No, exit",
        ]);
        assert_eq!(classify_launch_screen(&screen), LaunchScreen::AgentReady);
    }

    #[test]
    fn busy_screen_counts_as_agent_ready() {
        // 生成中は入力欄が出ないが、エージェントは確実に動いている
        let screen = lines(&["* Thinking… (3s · esc to interrupt)"]);
        assert_eq!(classify_launch_screen(&screen), LaunchScreen::AgentReady);
    }

    #[test]
    fn powershell_command_not_found_is_launch_error() {
        let screen = lines(&[
            "PS C:\\Users\\x> nonexistent-agent",
            "nonexistent-agent : The term 'nonexistent-agent' is not recognized as the name of a cmdlet, function,",
            "script file, or operable program.",
            "PS C:\\Users\\x>",
        ]);
        match classify_launch_screen(&screen) {
            LaunchScreen::LaunchError { detail } => {
                assert!(detail.contains("not recognized"), "detail={detail}");
            }
            other => panic!("LaunchError のはず: {other:?}"),
        }
    }

    #[test]
    fn japanese_powershell_command_not_found_is_launch_error() {
        // 日本語 Windows のメッセージ（#604 で表示言語まわりを直したので実機で出る）
        let screen = lines(&[
            "nonexistent : 用語 'nonexistent' は、コマンドレット、関数、スクリプト ファイル、",
            "または操作可能なプログラムの名前として認識されません。",
            "PS C:\\Users\\x>",
        ]);
        assert!(matches!(
            classify_launch_screen(&screen),
            LaunchScreen::LaunchError { .. }
        ));
    }

    #[test]
    fn 画面下部が空行でもエラーを見逃さない() {
        // 起動直後のペインは内容が上、下は空行だらけ。末尾 N 行を素直に切ると
        // 窓が空行だけになりエラーを見逃す（実測: 3 回も再送してしまった）
        let mut screen = lines(&[
            "PS C:\\Users\\x\\dev\\proj> & claude",
            "&: The term 'claude' is not recognized as a name of a cmdlet.",
            "PS C:\\Users\\x\\dev\\proj>",
        ]);
        screen.extend(std::iter::repeat_n(String::new(), 25));
        assert!(
            matches!(
                classify_launch_screen(&screen),
                LaunchScreen::LaunchError { .. }
            ),
            "空行を挟んでもエラーを検出すること"
        );
    }

    #[test]
    fn powershell7の実機メッセージを検出する() {
        // PowerShell 7.6 の実測（#665 E2E で採取）。5.1 の「the name」と違って
        // 「a name」なので、語尾まで固定したパターンでは素通りしていた
        let screen = lines(&[
            "PS C:\\Users\\x\\dev\\proj> $env:TAKO_ORCHESTRATOR_ROLE = 'worker'; & claude --effort max",
            "&: The term 'claude' is not recognized as a name of a cmdlet, function, script file, or executable program.",
            "Check the spelling of the name, or if a path was included, verify that the path is correct and try again.",
            "PS C:\\Users\\x\\dev\\proj>",
        ]);
        assert!(
            matches!(
                classify_launch_screen(&screen),
                LaunchScreen::LaunchError { .. }
            ),
            "PowerShell 7 の文言でも即失敗にすること（再送しても直らない）"
        );
    }

    #[test]
    fn 折り返しで割れたエラーメッセージも検出する() {
        // 実測（60 桁ペイン）: 日本語 PowerShell の「…として認識されません。」が
        // 行境界をまたいで割れ、行単位の照合では素通りしていた
        let screen = lines(&[
            "PS C:\\Users\\x\\dev> $env:TAKO_ORCHESTRATOR_ROLE = 'worker'; & clau",
            "de --effort max",
            "& : 用語 'claude' は、コマンドレット、関数、スクリプト ファイル、また",
            "は操作可能なプログラムの名前として認識されません。名前が正しく記述され",
            "ていることを確認し、パスが含まれている場合はそのパスが正しいことを確認",
            "してから、再試行してください。",
            "PS C:\\Users\\x\\dev>",
        ]);
        assert!(
            matches!(
                classify_launch_screen(&screen),
                LaunchScreen::LaunchError { .. }
            ),
            "折り返しても LaunchError と判定すること（さもないと無駄な再送を 3 回する）"
        );
    }

    #[test]
    fn 折り返し連結でも無関係な画面を誤検出しない() {
        // 連結照合は偽陽性が怖い。ふつうのシェル画面が LaunchError にならないこと
        let screen = lines(&[
            "PS C:\\Users\\x\\dev\\tako> git status",
            "On branch main",
            "nothing to commit, working tree clean",
            "PS C:\\Users\\x\\dev\\tako>",
        ]);
        assert_eq!(classify_launch_screen(&screen), LaunchScreen::ShellPrompt);
    }

    #[test]
    fn posix_command_not_found_is_launch_error() {
        let screen = lines(&["bash: nonexistent-agent: command not found", "user@host:~$"]);
        assert!(matches!(
            classify_launch_screen(&screen),
            LaunchScreen::LaunchError { .. }
        ));
    }

    #[test]
    fn launch_error_does_not_win_over_running_agent() {
        // 過去の失敗行がスクロールバックに残っていても、いま動いていれば AgentReady
        let screen = lines(&[
            "bash: claude: command not found",
            "user@host:~$ /usr/local/bin/claude",
            "❯ ",
        ]);
        assert_eq!(classify_launch_screen(&screen), LaunchScreen::AgentReady);
    }

    #[test]
    fn empty_or_unknown_screen_is_unknown() {
        assert_eq!(classify_launch_screen(&[]), LaunchScreen::Unknown);
        assert_eq!(
            classify_launch_screen(&lines(&["", "   ", ""])),
            LaunchScreen::Unknown
        );
        // 起動処理の途中（プロンプトもエージェントも見えない）
        assert_eq!(
            classify_launch_screen(&lines(&["Loading configuration..."])),
            LaunchScreen::Unknown
        );
    }

    #[test]
    fn command_still_being_typed_is_not_a_shell_prompt() {
        // プロンプト行にコマンドが続いている = 実行済み。再送してはいけない
        let screen = lines(&["PS C:\\Users\\x\\dev\\tako> claude --model opus"]);
        assert_eq!(classify_launch_screen(&screen), LaunchScreen::Unknown);
    }

    #[test]
    fn launch_record_phase_roundtrip() {
        let rec = LaunchRecord {
            phase: LaunchPhase::AgentStarted.as_str().to_string(),
            attempts: 2,
            updated_at: "2026-07-30T00:00:00Z".into(),
            detail: None,
        };
        assert_eq!(rec.phase(), Some(LaunchPhase::AgentStarted));
        let bad = LaunchRecord::default();
        assert_eq!(bad.phase(), None);
    }
}
