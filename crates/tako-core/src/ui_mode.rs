//! ui_mode — GUI ライク表示モードの状態と、ペインごとの表示決定（Issue #691 / #694）
//!
//! 仕様の正は `.agent/plans/2026-07-gui-mode.md`。ここに置くのは
//! **表示レイヤの判定だけ**で、PTY / tmux バックエンド / persist には一切関わらない
//! （設計原則「表示レイヤのみの切替」。同じペインの別レンダラを選ぶだけ）。
//!
//! 判定を純関数にしてあるのは、誤爆（チャット化・スターター化すべきでないペインを
//! 置き換える）が実害の大きい失敗だから。GUI を起動せずに表の全行を unit test できる。

use crate::terminal::CommandState;
use std::time::Duration;

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
    /// 準備中プレースホルダ（#720）。表示種別がまだ確定していない過渡期に、
    /// 生ターミナル（direnv のロードログ・プロンプト・TUI の起動途中）を見せないための覆い
    Preparing,
}

impl PaneDisplay {
    /// CLI / MCP へ出す安定した識別子（#720。AI が「いま画面に何が出ているか」を知る手段）
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Terminal => "terminal",
            Self::Starter => "starter",
            Self::Chat => "chat",
            Self::Preparing => "preparing",
        }
    }
}

/// 過渡期に何の確定を待っているか（#720）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettleKind {
    /// 素のシェルの起動待ち（新規ペイン）。プロンプトが出た時点でスターターへ抜ける
    Shell,
    /// エージェント TUI の起動待ち（worker spawn / スターターの master・solo）。
    /// claude の起動 + `claude agents --json` への登録まで数秒〜十数秒かかる
    Agent,
}

/// シェルのプロンプト（OSC 133）が出るまで待つ上限。
/// シェル統合が無い環境ではプロンプトが永久に来ないので、ここで諦めてターミナル表示にする
pub const SETTLE_SHELL_LIMIT: Duration = Duration::from_millis(4_000);

/// エージェント TUI がチャットとして確定するまで待つ上限。
///
/// 隔離実測（#720 / セルフテスト 96c、実 claude）: スターター押下からチャット確定まで
/// **13.8 秒**（claude の起動 + `claude agents --json` への登録 + 2 秒 tick の待ち合わせ）。
/// 上限はその約 1.8 倍を取ってある。起動失敗・ログイン要求などで確定しない場合は
/// ここで諦めて素の画面を見せる（**止まっているものは見せる方が正しい**）。
/// プレースホルダには「ターミナルを表示」があるので、待たずに抜けることもできる
pub const SETTLE_AGENT_LIMIT: Duration = Duration::from_secs(25);

/// 過渡期の状態（`elapsed` はペイン生成 / 起動操作からの経過時間）
#[derive(Debug, Clone, Copy)]
pub struct SettleState {
    pub kind: SettleKind,
    pub elapsed: Duration,
}

impl SettleState {
    pub fn limit(self) -> Duration {
        match self.kind {
            SettleKind::Shell => SETTLE_SHELL_LIMIT,
            SettleKind::Agent => SETTLE_AGENT_LIMIT,
        }
    }

    /// まだ過渡期か。**上限を過ぎたら必ず false**（永遠にローディングを出さない）
    pub fn active(self) -> bool {
        self.elapsed < self.limit()
    }
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
    /// 表示種別の確定を待っている過渡期（#720）。`None` = 過渡期の管理外 = 即確定させる。
    /// **チャット / スターターの確定より後に効く**ので、確定できるものは待たせない
    pub settle: Option<SettleState>,
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
            settle: None,
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
/// 不明はターミナル表示にする（誤って隠す方が、ターミナルのままより実害が大きい）。
///
/// ただし**生成直後の過渡期だけは例外**（#720）: 「不明だからターミナル」を素直に
/// 適用すると、シェルの起動〜エージェントの起動完了までの数秒間だけ生ターミナルが
/// 映って消える（direnv のロードログやプロンプトのちらつき）。確定するまでの
/// 上限つきの猶予を `settle` で与え、その間は準備中プレースホルダで覆う
pub fn pane_display(input: PaneDisplayInput) -> PaneDisplay {
    if !input.mode.is_gui() {
        return PaneDisplay::Terminal;
    }
    // 「コマンド入力へ」で明示的にターミナルにしたペインと alt screen TUI は
    // 過渡期より優先して即ターミナル（待たせる理由が無い / 覆っても中身を描けない）
    if input.released || input.alt_screen {
        return PaneDisplay::Terminal;
    }
    if input.claude_chat {
        return PaneDisplay::Chat;
    }
    if input.is_idle_shell() {
        return PaneDisplay::Starter;
    }
    if input.settle.is_some_and(SettleState::active) {
        return PaneDisplay::Preparing;
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

/// スターターのボタン（`.agent/plans/2026-07-gui-mode.md` §2.2）。
/// カード 3 枚 + 下部の控えめなリンク（`Setup`。#720）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StarterAction {
    /// AI チームに任せる = シェルへ `tako master` を書き込む
    Master,
    /// AI と 1 対 1 で話す = シェルへ `tako solo` を書き込む
    Solo,
    /// コマンド入力へ = このペインだけターミナル表示に戻す（何も起動しない）
    UseTerminal,
    /// 初期設定をやり直す = シェルへ `tako setup` を書き込む（#720）
    Setup,
}

impl StarterAction {
    /// シェルへ書き込む tako サブコマンド（`UseTerminal` は何も起動しないので None）。
    /// master / solo は「エージェント CLI の起動そのもの」= CLI_ONLY なので、
    /// dispatch ではなくシェルへのコマンド書き込みが正（副次効果として、
    /// ターミナル表示に切り替えると実行されたコマンドが履歴に見える = 学習経路）。
    /// setup も同じ理由でシェル書き込み（welcome バナー #549 と同一方式）
    pub fn subcommand(self) -> Option<&'static str> {
        match self {
            Self::Master => Some("master"),
            Self::Solo => Some("solo"),
            Self::Setup => Some("setup"),
            Self::UseTerminal => None,
        }
    }

    /// 押下後にチャットビューへ向かうか（#720 の過渡期プレースホルダの種別）。
    ///
    /// setup は **false**: `tako setup` は診断結果と質問をターミナルに出す対話ウィザードで、
    /// 出力そのものがユーザーの読むものだから、覆わずにすぐ見せる方が正しい
    pub fn expects_chat(self) -> bool {
        matches!(self, Self::Master | Self::Solo)
    }

    /// 要素 ID・ログ・テストで使う安定した識別子
    pub fn id(self) -> &'static str {
        match self {
            Self::Master => "master",
            Self::Solo => "solo",
            Self::UseTerminal => "terminal",
            Self::Setup => "setup",
        }
    }

    pub const ALL: [Self; 4] = [Self::Master, Self::Solo, Self::UseTerminal, Self::Setup];
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
        assert_eq!(StarterAction::Setup.subcommand(), Some("setup"));
        assert_eq!(StarterAction::UseTerminal.subcommand(), None);
        let ids: Vec<&str> = StarterAction::ALL.iter().map(|a| a.id()).collect();
        assert_eq!(ids, ["master", "solo", "terminal", "setup"]);
        // 過渡期の種別: エージェントを待つのは master / solo だけ（#720）。
        // setup はターミナルの対話ウィザードなので覆わない
        assert!(StarterAction::Master.expects_chat());
        assert!(StarterAction::Solo.expects_chat());
        assert!(!StarterAction::Setup.expects_chat());
        assert!(!StarterAction::UseTerminal.expects_chat());
    }

    /// #720: 生成直後の過渡期は生ターミナルではなく準備中プレースホルダで覆う
    #[test]
    fn 過渡期は準備中プレースホルダになる() {
        let settling = |kind: SettleKind, elapsed: Duration| PaneDisplayInput {
            // 起動途中のシェル = プロンプト未達（Unknown）+ コマンド実行中の両方を見る
            state: CommandState::Unknown,
            settle: Some(SettleState { kind, elapsed }),
            ..gui_idle()
        };
        for kind in [SettleKind::Shell, SettleKind::Agent] {
            assert_eq!(
                pane_display(settling(kind, Duration::from_millis(0))),
                PaneDisplay::Preparing,
                "{kind:?}: 生成直後は覆う"
            );
            let limit = SettleState {
                kind,
                elapsed: Duration::ZERO,
            }
            .limit();
            assert_eq!(
                pane_display(settling(kind, limit - Duration::from_millis(1))),
                PaneDisplay::Preparing,
                "{kind:?}: 上限直前はまだ覆う"
            );
            // **上限を過ぎたら必ず抜ける**（永遠にローディングを出さない）
            assert_eq!(
                pane_display(settling(kind, limit)),
                PaneDisplay::Terminal,
                "{kind:?}: 上限で通常判定へ落ちる"
            );
            assert_eq!(
                pane_display(settling(kind, limit + Duration::from_secs(600))),
                PaneDisplay::Terminal,
                "{kind:?}: 上限超過も同じ"
            );
        }
        // エージェントの猶予はシェルより長い（claude の起動 + agents 登録を待つため）
        assert!(SETTLE_AGENT_LIMIT > SETTLE_SHELL_LIMIT);
    }

    /// #720: 過渡期は**確定できるものを待たせない**（表の最後に置く意味）
    #[test]
    fn 過渡期より確定した表示が優先する() {
        let fresh = SettleState {
            kind: SettleKind::Agent,
            elapsed: Duration::from_millis(0),
        };
        // チャット確定 → 即チャット（プレースホルダを挟まない）
        assert_eq!(
            pane_display(PaneDisplayInput {
                claude_chat: true,
                state: CommandState::Running,
                busy_children: true,
                settle: Some(fresh),
                ..gui_idle()
            }),
            PaneDisplay::Chat
        );
        // アイドルシェル確定 → 即スターター
        assert_eq!(
            pane_display(PaneDisplayInput {
                settle: Some(fresh),
                ..gui_idle()
            }),
            PaneDisplay::Starter
        );
        // 「コマンド入力へ」の明示ターミナルと alt screen TUI は過渡期でも即ターミナル
        for input in [
            PaneDisplayInput {
                released: true,
                state: CommandState::Unknown,
                settle: Some(fresh),
                ..gui_idle()
            },
            PaneDisplayInput {
                alt_screen: true,
                state: CommandState::Unknown,
                settle: Some(fresh),
                ..gui_idle()
            },
        ] {
            assert_eq!(pane_display(input), PaneDisplay::Terminal, "{input:?}");
        }
        // terminal モードは過渡期を持ち込んでも不変
        assert_eq!(
            pane_display(PaneDisplayInput {
                settle: Some(fresh),
                state: CommandState::Unknown,
                ..PaneDisplayInput::terminal_mode()
            }),
            PaneDisplay::Terminal
        );
    }
}
