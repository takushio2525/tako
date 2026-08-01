//! ui_mode — GUI ライク表示モードの状態と、ペインごとの表示決定（Issue #691 / #694）
//!
//! 仕様の正は `.agent/plans/2026-07-gui-mode.md`。ここに置くのは
//! **表示レイヤの判定だけ**で、PTY / tmux バックエンド / persist には一切関わらない
//! （設計原則「表示レイヤのみの切替」。同じペインの別レンダラを選ぶだけ）。
//!
//! 判定を純関数にしてあるのは、誤爆（チャット化・スターター化すべきでないペインを
//! 置き換える）が実害の大きい失敗だから。GUI を起動せずに表の全行を unit test できる。

use crate::terminal::CommandState;

/// UI 表示モード（グローバル 1 値。settings.json `ui_mode` に永続化）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum UiMode {
    /// 既定。従来どおり全ペインをターミナルとして描く（既存ユーザーの体験は不変）
    #[default]
    Terminal,
    /// 初心者向け表示。ペイン種別ごとにスターター / チャット / ターミナルを出し分ける
    Gui,
}

impl UiMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Terminal => "terminal",
            Self::Gui => "gui",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "terminal" => Some(Self::Terminal),
            "gui" => Some(Self::Gui),
            _ => None,
        }
    }

    pub fn toggled(self) -> Self {
        match self {
            Self::Terminal => Self::Gui,
            Self::Gui => Self::Terminal,
        }
    }

    pub fn is_gui(self) -> bool {
        matches!(self, Self::Gui)
    }

    /// CLI / MCP / エラーメッセージが共有する選択肢（語彙の正）
    pub const VALUES: [&'static str; 2] = ["terminal", "gui"];
}

/// 1 ペインをどう描くか（`.agent/plans/2026-07-gui-mode.md` §2.1 判定表の結果）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaneDisplay {
    /// 従来のターミナル描画（terminal モード時は必ずこれ）
    Terminal,
    /// スターター（3 ボタン）。アイドルシェルのペインだけ
    Starter,
    /// チャットビュー（claude 対話ペイン）。実装は G2
    Chat,
}

/// 判定の入力。**呼び出し側は毎 render これを組み立てる**ので、
/// 新しいサブプロセス起動を必要とする材料は入れない（既存のキャッシュ済み値だけを使う）
#[derive(Debug, Clone, Copy)]
pub struct PaneDisplayInput {
    /// グローバルの表示モード
    pub mode: UiMode,
    /// スターターの「コマンド入力へ」でこのペインだけ解除されている（揮発フラグ）
    pub released: bool,
    /// alt screen（vim 等の TUI）。構造的に置き換え不可。
    ///
    /// **ペインの中で動いているプログラム**の状態を渡すこと。tmux バックエンド越しの
    /// ペインでは、外側のエミュレータのフラグは tmux クライアント自身の alt screen を
    /// 指していて中身とは無関係（実測で確認済み。#702）
    pub alt_screen: bool,
    /// claude 対話 TUI が稼働していると確定した（G2 で配線。G1 は常に false）
    pub claude_chat: bool,
    /// OSC 133 由来のコマンド実行状態
    pub state: CommandState,
    /// role 付きペイン（master / solo / worker 等）= エージェント用途
    pub has_role: bool,
    /// バックエンドセッションに実行中の子プロセスがある
    /// （sleep_guard の判定結果を流用。2 秒 tick の background 計算のキャッシュ）
    pub busy_children: bool,
}

impl PaneDisplayInput {
    /// terminal モードの既定（判定が必ず Terminal になる入力）
    pub fn terminal_mode() -> Self {
        Self {
            mode: UiMode::Terminal,
            released: false,
            alt_screen: false,
            claude_chat: false,
            state: CommandState::Unknown,
            has_role: false,
            busy_children: false,
        }
    }

    /// 子プロセスの無いアイドルシェルか（スターターを出す条件の本体）。
    /// `Failed` は「直前のコマンドがエラーで終わった」= 画面にエラーが出ているので
    /// 隠さない。`Unknown` はシェル統合の signal that が無く判断不能なので隠さない
    pub fn is_idle_shell(&self) -> bool {
        self.state == CommandState::Idle && !self.has_role && !self.busy_children
    }
}

/// ペイン表示の決定（判定表を上から先勝ちで評価する）。
///
/// **保守的に倒す**のが原則: 置き換えるのは確信がある場合だけで、
/// 不明はターミナル表示にする（誤って隠す方が、ターミナルのままより実害が大きい）
pub fn pane_display(input: PaneDisplayInput) -> PaneDisplay {
    if !input.mode.is_gui() {
        return PaneDisplay::Terminal;
    }
    if input.released || input.alt_screen {
        return PaneDisplay::Terminal;
    }
    if input.claude_chat {
        return PaneDisplay::Chat;
    }
    if input.is_idle_shell() {
        return PaneDisplay::Starter;
    }
    PaneDisplay::Terminal
}

/// チャットビューにしてよいかの材料（判定表の「claude 対話 TUI 稼働」行。#702）。
///
/// 材料はどれも既存の仕組みから取れるものだけにしてある:
/// `session_id` / `interactive` は `agents::live_claude_sessions_by_backend`
/// （pid 祖先辿り + sticky #466）、`agent_running` は sleep_guard の子プロセス判定（#372）。
#[derive(Debug, Clone, Copy, Default)]
pub struct ChatEligibility<'a> {
    /// live 解決で得た claude の session_id（transcript の参照キー）
    pub session_id: Option<&'a str>,
    /// `claude agents --json` の `kind == "interactive"` = 対話 TUI
    pub interactive: bool,
    /// ペインに実行中の子プロセスがある = claude のプロセスがまだ生きている
    pub agent_running: bool,
}

/// チャット化する場合に描画対象の session_id を返す（しないなら None）。
///
/// **3 つの証拠が揃ったときだけ**チャットにする。1 つでも欠けたらターミナル表示のまま:
/// - session_id が無い → 描く会話が無い（空のチャットで画面を覆うのが最悪の失敗）
/// - interactive でない → `claude -p` 等の一時セッション。人が読む会話ではない
/// - 子プロセスが無い → claude はもう終了している。sticky（#466）は agents の一時失敗に
///   耐えるため記憶を保持し続けるので、**生存の根拠はプロセス側から取る**
pub fn chat_session(input: ChatEligibility<'_>) -> Option<&str> {
    let session = input.session_id?.trim();
    if session.is_empty() || !input.interactive || !input.agent_running {
        return None;
    }
    Some(session)
}

/// 入力を受け付けない（read-only チャットにする）ペインか（§2.4）。
/// worker への指示は master 経由が原則なので、worker のチャットは読むだけにする
pub fn is_read_only_role(role: &str) -> bool {
    let role = role.trim();
    role.contains("orchestrator-worker") || role.starts_with("worker")
}

/// スターターの 3 ボタン（`.agent/plans/2026-07-gui-mode.md` §2.2）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StarterAction {
    /// AI チームに任せる = シェルへ `tako master` を書き込む
    Master,
    /// AI と 1 対 1 で話す = シェルへ `tako solo` を書き込む
    Solo,
    /// コマンド入力へ = このペインだけターミナル表示に戻す（何も起動しない）
    UseTerminal,
}

impl StarterAction {
    /// シェルへ書き込む tako サブコマンド（`UseTerminal` は何も起動しないので None）。
    /// master / solo は「エージェント CLI の起動そのもの」= CLI_ONLY なので、
    /// dispatch ではなくシェルへのコマンド書き込みが正（副次効果として、
    /// ターミナル表示に切り替えると実行されたコマンドが履歴に見える = 学習経路）
    pub fn subcommand(self) -> Option<&'static str> {
        match self {
            Self::Master => Some("master"),
            Self::Solo => Some("solo"),
            Self::UseTerminal => None,
        }
    }

    /// 要素 ID・ログ・テストで使う安定した識別子
    pub fn id(self) -> &'static str {
        match self {
            Self::Master => "master",
            Self::Solo => "solo",
            Self::UseTerminal => "terminal",
        }
    }

    pub const ALL: [Self; 3] = [Self::Master, Self::Solo, Self::UseTerminal];
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn モードの語彙は往復する() {
        for s in UiMode::VALUES {
            let mode = UiMode::parse(s).expect("既知の値");
            assert_eq!(mode.as_str(), s);
        }
        assert_eq!(UiMode::parse(" GUI "), Some(UiMode::Gui));
        assert_eq!(UiMode::parse("simple"), None);
        assert_eq!(UiMode::default(), UiMode::Terminal);
        assert_eq!(UiMode::Terminal.toggled(), UiMode::Gui);
        assert_eq!(UiMode::Gui.toggled(), UiMode::Terminal);
    }

    /// GUI モードの入力（アイドルシェル）を起点に、1 項目だけ変えて表を検査する
    fn gui_idle() -> PaneDisplayInput {
        PaneDisplayInput {
            mode: UiMode::Gui,
            state: CommandState::Idle,
            ..PaneDisplayInput::terminal_mode()
        }
    }

    #[test]
    fn terminalモードでは常にターミナル表示() {
        // 既存ユーザーへの影響ゼロ: どんな材料でも判定が動かない
        for state in [
            CommandState::Idle,
            CommandState::Running,
            CommandState::Unknown,
            CommandState::Failed(1),
        ] {
            let input = PaneDisplayInput {
                state,
                claude_chat: true,
                ..PaneDisplayInput::terminal_mode()
            };
            assert_eq!(pane_display(input), PaneDisplay::Terminal, "{state:?}");
        }
    }

    #[test]
    fn guiモードのアイドルシェルはスターター() {
        assert_eq!(pane_display(gui_idle()), PaneDisplay::Starter);
    }

    #[test]
    fn 揮発解除フラグは最優先でターミナル表示() {
        let input = PaneDisplayInput {
            released: true,
            claude_chat: true,
            ..gui_idle()
        };
        assert_eq!(pane_display(input), PaneDisplay::Terminal);
    }

    #[test]
    fn alt_screenはチャットより優先してターミナル表示() {
        let input = PaneDisplayInput {
            alt_screen: true,
            claude_chat: true,
            ..gui_idle()
        };
        assert_eq!(pane_display(input), PaneDisplay::Terminal);
    }

    #[test]
    fn claude確定ペインはチャット() {
        let input = PaneDisplayInput {
            claude_chat: true,
            // claude 稼働中は子プロセスがあり Idle でもないのが普通。
            // それでもチャット判定が勝つ（表の順序）
            state: CommandState::Running,
            busy_children: true,
            has_role: true,
            ..gui_idle()
        };
        assert_eq!(pane_display(input), PaneDisplay::Chat);
    }

    #[test]
    fn 判断できないペインはターミナル表示のまま() {
        // 実行中 / エラー保持 / シェル統合なし / role 付き / 子プロセスあり
        let cases = [
            PaneDisplayInput {
                state: CommandState::Running,
                ..gui_idle()
            },
            PaneDisplayInput {
                state: CommandState::Failed(2),
                ..gui_idle()
            },
            PaneDisplayInput {
                state: CommandState::Unknown,
                ..gui_idle()
            },
            PaneDisplayInput {
                has_role: true,
                ..gui_idle()
            },
            PaneDisplayInput {
                busy_children: true,
                ..gui_idle()
            },
        ];
        for input in cases {
            assert_eq!(
                pane_display(input),
                PaneDisplay::Terminal,
                "保守的判定が崩れている: {input:?}"
            );
            assert!(!input.is_idle_shell());
        }
    }

    /// チャット判定の起点（3 つの証拠が揃った状態）
    fn chat_ok<'a>(session: &'a str) -> ChatEligibility<'a> {
        ChatEligibility {
            session_id: Some(session),
            interactive: true,
            agent_running: true,
        }
    }

    #[test]
    fn 証拠が揃ったときだけチャット化する() {
        assert_eq!(chat_session(chat_ok("abc-123")), Some("abc-123"));
        // 前後の空白は落として返す（transcript の参照キーになる）
        assert_eq!(chat_session(chat_ok("  abc-123 ")), Some("abc-123"));

        let missing = [
            // session_id が無い / 空 = 描く会話が無い
            ChatEligibility {
                session_id: None,
                ..chat_ok("x")
            },
            ChatEligibility {
                session_id: Some("   "),
                ..chat_ok("x")
            },
            // `claude -p` 等の非対話セッション
            ChatEligibility {
                interactive: false,
                ..chat_ok("abc-123")
            },
            // claude が終了済み（sticky の記憶だけが残っている状態）
            ChatEligibility {
                agent_running: false,
                ..chat_ok("abc-123")
            },
        ];
        for input in missing {
            assert_eq!(
                chat_session(input),
                None,
                "証拠が欠けたらチャット化しない: {input:?}"
            );
        }
    }

    #[test]
    fn チャット判定はpane_displayの表に載る() {
        // 判定の結果を claude_chat に入れれば表が Chat を返す（配線の契約）
        let session = chat_session(chat_ok("abc-123"));
        let input = PaneDisplayInput {
            claude_chat: session.is_some(),
            state: CommandState::Running,
            busy_children: true,
            has_role: true,
            ..gui_idle()
        };
        assert_eq!(pane_display(input), PaneDisplay::Chat);
    }

    #[test]
    fn workerロールだけ読み取り専用() {
        for role in [
            "orchestrator-worker",
            "orchestrator-worker:3",
            "worker",
            "worker-2",
        ] {
            assert!(is_read_only_role(role), "worker は read-only: {role}");
        }
        for role in ["orchestrator-master", "master", "solo", "master:sol", ""] {
            assert!(!is_read_only_role(role), "master / solo は入力可: {role}");
        }
    }

    #[test]
    fn スターターの起動コマンドは既存cliそのもの() {
        assert_eq!(StarterAction::Master.subcommand(), Some("master"));
        assert_eq!(StarterAction::Solo.subcommand(), Some("solo"));
        assert_eq!(StarterAction::UseTerminal.subcommand(), None);
        let ids: Vec<&str> = StarterAction::ALL.iter().map(|a| a.id()).collect();
        assert_eq!(ids, ["master", "solo", "terminal"]);
    }
}
