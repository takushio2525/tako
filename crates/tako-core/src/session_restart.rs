//! session_restart — エージェントペインを「会話を失わずに」建て直す判断（Issue #1067）
//!
//! # 2 つのモード
//!
//! | モード | 何が変わるか | 何が残るか |
//! |---|---|---|
//! | [`SessionRestartMode::Harness`] | エージェント CLI のプロセス（= 実行中のバイナリ・env） | **会話そのまま**（`--resume`） |
//! | [`SessionRestartMode::Handoff`] | セッション（新しい会話） | 引き継ぎファイル経由の**要約** |
//!
//! ハーネス更新は #498（claude の自動更新後もプロセスが古い版のまま残る）の
//! ワンクリック解決手段で、**会話コンテキストを 1 文字も失わない**のが要点。
//! 引き継ぎ再起動は #749 の自動ハンドオフ（ctx% 高騰）を人の操作で起こせるようにしたもので、
//! **ctx をリセットできる**代わりに引き継ぎファイルに書いた分しか残らない。
//!
//! # ここに置くもの / 置かないもの
//!
//! - 置く: モードの語彙・実行してよいかの判断・後始末の段取り・画面から拾える手がかり（すべて純関数）
//! - 置かない: session_id の解決（`tako-control::agents` / `sessions`）・
//!   プロセスの終了（`tako-control::platform::process`）・
//!   コマンドの送達（`tako-core::shell_send` + `tako-app` の駆動）
//!
//! # 出た項目が断られない（#1006）と「理由が出る」（#1067）を両立させる
//!
//! 判断材料は 2 種類ある:
//!
//! - **構造的**（このペインで原理的に可能か）: セッションの有無・role・agent 系統・
//!   会話の解決可否。GUI のメニュー項目の**出し分け**はこれで決める
//!   （出た項目が断られない = #1006 の原則）
//! - **一時的**（今この瞬間は待つべきか）: 生成中・キュー滞留・入力欄の下書き・
//!   選択肢ダイアログ。これは**実行時に断って理由を返す**（メニューから消すと
//!   ユーザーが機能そのものを見つけられなくなるうえ、右クリックした瞬間の状態で
//!   項目が消えたり出たりする）
//!
//! この 2 段は [`can_restart`]（両方見る = 実行判断）と
//! [`menu_modes`]（構造だけ見る = 出し分け）で表してある。

use crate::agent_support::{self, keys, Agent};

/// 再起動の種別（CLI の possible values / MCP の enum / GUI の分岐の正本）
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionRestartMode {
    /// エージェント CLI のプロセスだけを建て直し、`--resume` で**同じ会話**を続ける
    Harness,
    /// 引き継ぎを書かせてから**新しいセッション**へ交代する（#749 の手動版）
    Handoff,
}

impl SessionRestartMode {
    /// 受け付ける値の一覧
    pub const VALUES: [&'static str; 2] = ["harness", "handoff"];

    /// ワイヤ表記（応答 JSON・CLI の表示）
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Harness => "harness",
            Self::Handoff => "handoff",
        }
    }

    /// 文字列から解釈する（大文字小文字と前後空白は無視。`remote_open` と同じ寛容さ）
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "harness" => Some(Self::Harness),
            "handoff" => Some(Self::Handoff),
            _ => None,
        }
    }

    /// 不正値のエラー文に添える案内
    pub fn values_hint() -> String {
        Self::VALUES.join(" | ")
    }

    /// この agent 系統でそのモードが使えるか（判断は能力マトリクス #982 の 1 マス）
    pub fn capability_key(self) -> &'static str {
        match self {
            Self::Harness => keys::SESSION_RESTART_HARNESS,
            Self::Handoff => keys::SESSION_RESTART_HANDOFF,
        }
    }
}

/// 再起動できない理由。**「できない」を黙って無視しない**ための型（`PaneSshBlock` と同じ思想）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestartBlock {
    /// ターミナルセッションが無い（プレビュー・Web ビュー等）
    NoSession,
    /// AI エージェントのペインではない（role が付いていない）
    NotAgent,
    /// その agent 系統では手段が違う（claude 以外の resume・引き継ぎ）
    AgentUnsupported { agent: Agent },
    /// claude の会話（session_id）を解決できない = resume 先が分からない
    SessionUnresolved,
    /// エージェント CLI のプロセスが見つからない = 終了させる相手が分からない
    AgentProcessNotFound,
    /// 引き継ぎ再起動は master ペインだけ（worker / solo は引き継ぎ機構を持たない）
    HandoffNeedsMaster,
    /// 生成中・コマンド実行中（一時的）
    Busy,
    /// キューに未送信の指示が残っている（一時的。#572）
    QueuedMessages,
    /// 入力欄に人間の下書きがある（一時的）
    UserDraft,
    /// 選択肢ダイアログを表示中（一時的。#748）
    Dialog,
    /// 会話 ID は分かったが、その会話の記録（transcript）が見つからない（#1967）。
    /// 終了させてから resume しても `No conversation found` で落ちて会話も画面も失う
    ConversationMissing,
    /// このペインの建て直しがまだ終わっていない（一時的。#1967）。
    /// 重ねて押すと、起動し直したばかりのエージェントをまた終了させる
    RestartInProgress,
}

impl RestartBlock {
    /// ワイヤ表記（応答の `reason`）
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NoSession => "no_session",
            Self::NotAgent => "not_agent",
            Self::AgentUnsupported { .. } => "agent_unsupported",
            Self::SessionUnresolved => "session_unresolved",
            Self::AgentProcessNotFound => "agent_process_not_found",
            Self::HandoffNeedsMaster => "handoff_needs_master",
            Self::Busy => "busy",
            Self::QueuedMessages => "queued_messages",
            Self::UserDraft => "user_draft",
            Self::Dialog => "dialog",
            Self::ConversationMissing => "conversation_missing",
            Self::RestartInProgress => "restart_in_progress",
        }
    }

    /// 状態が変われば通るようになる理由か（= メニューからは消さない。モジュールの説明を参照）
    pub fn is_transient(self) -> bool {
        matches!(
            self,
            Self::SessionUnresolved
                | Self::AgentProcessNotFound
                | Self::Busy
                | Self::QueuedMessages
                | Self::UserDraft
                | Self::Dialog
                | Self::ConversationMissing
                | Self::RestartInProgress
        )
    }

    /// 理由 + 次の一手（dispatch / CLI / MCP / GUI のバナーにそのまま出す。規約どおり日本語）
    pub fn message(self, pane: u64, mode: SessionRestartMode) -> String {
        match self {
            Self::NoSession => format!(
                "pane {pane} にはターミナルセッションが無い（プレビュー等）ので再起動できない。\
                 エージェントが動いているペインを指定する"
            ),
            Self::NotAgent => format!(
                "pane {pane} は AI エージェントのペインではない（role が無い）ので\
                 セッション再起動の対象外。`tako list` の role で対象ペインを確認する"
            ),
            Self::AgentUnsupported { agent } => match mode {
                SessionRestartMode::Harness => format!(
                    "pane {pane} の系統（{}）は tako からの resume に未対応なので\
                     ハーネス更新できない（claude のみ）。手動なら codex は `codex resume`、\
                     agy は `agy --conversation` を使う。対応状況は `tako agent-support` を参照",
                    agent.as_str()
                ),
                SessionRestartMode::Handoff => format!(
                    "pane {pane} の系統（{}）は tako からの引き継ぎ再起動に未対応\
                     （claude のみ実測済み）。対応状況は `tako agent-support` を参照",
                    agent.as_str()
                ),
            },
            Self::SessionUnresolved => format!(
                "pane {pane} の claude の会話（session_id）を解決できないので\
                 ハーネス更新できない（resume 先が分からないまま終了させると会話を失う）。\
                 `tako sessions list` で会話を確認し、必要なら `tako sessions resume <id>` を使う"
            ),
            Self::AgentProcessNotFound => match mode {
                SessionRestartMode::Harness => format!(
                    "pane {pane} で動いているエージェント CLI のプロセスが見つからず、\
                     シェルが入力待ちとも確かめられないのでハーネス更新できない\
                     （終了させる相手が分からないまま resume の行を打つと、\
                     動いているエージェントへの指示として入力されてしまう）。\
                     すでに終了しているなら `tako sessions resume <id>` で会話を別ペインへ開く"
                ),
                // #1967: エージェントが居ないペインへ引き継ぎの依頼を積むと、
                // 届け先が無いまま黙って時間切れになる（後任も立たない）
                SessionRestartMode::Handoff => format!(
                    "pane {pane} ではエージェントが動いていないので引き継ぎを書かせられない\
                     （依頼を受け取る相手が居ない）。同じ会話を戻すなら \
                     `tako session-restart --mode harness --pane {pane}` を使う"
                ),
            },
            Self::HandoffNeedsMaster => format!(
                "pane {pane} は master ペインではないので引き継ぎ再起動できない\
                 （引き継ぎファイルの機構は master だけが持つ）。\
                 会話を保ったまま建て直すなら mode=harness を使う"
            ),
            Self::Busy => format!(
                "pane {pane} は生成中 / コマンド実行中なので再起動しない\
                 （途中の作業を失う）。終わってからもう一度実行する"
            ),
            Self::QueuedMessages => format!(
                "pane {pane} には未送信の指示がキューに残っているので再起動しない\
                 （その指示が失われる）。エージェントが処理し終えてからもう一度実行する"
            ),
            Self::UserDraft => format!(
                "pane {pane} の入力欄に下書きが残っているので再起動しない\
                 （打ちかけの指示が失われる）。送信するか消してからもう一度実行する"
            ),
            Self::Dialog => format!(
                "pane {pane} は選択肢ダイアログを表示中なので再起動しない。\
                 `tako orchestrator respond --pane {pane}` で応答してからもう一度実行する"
            ),
            Self::ConversationMissing => format!(
                "pane {pane} の会話の記録（claude の transcript）が見つからないので\
                 ハーネス更新できない（終了させてから resume しても \
                 `No conversation found` で落ち、会話も画面も失う）。\
                 `tako sessions list` で再開できる会話（resumable）を確認する"
            ),
            Self::RestartInProgress => format!(
                "pane {pane} の建て直しがまだ終わっていないので重ねて再起動しない\
                 （起動し直したばかりのエージェントをまた終了させてしまう）。\
                 `tako session-restart --pane {pane}` の last_restart で進み具合を確かめ、\
                 終わってからもう一度実行する"
            ),
        }
    }
}

/// 判断の材料（すべて呼び出し側が観測して詰める。ここでは I/O をしない）
#[derive(Debug, Clone)]
pub struct RestartFacts {
    /// ターミナルセッションを持っている
    pub has_session: bool,
    /// AI エージェントの role が付いている
    pub is_agent: bool,
    /// master role（引き継ぎ機構を持つ）
    pub is_master: bool,
    /// このペインで動いている agent 系統
    pub agent: Agent,
    /// claude の会話（session_id）を解決できた（harness の**実行**に必須。
    /// 解決には I/O が要るので、メニューの出し分け（[`is_eligible`]）では見ない）
    pub session_resolved: bool,
    /// 終了させるエージェント CLI のプロセスが見つかった（harness の**実行**に必須。
    /// `session_resolved` と同じ理由でメニューの出し分けでは見ない）。
    ///
    /// **見つからないときに進めてはいけない**: 相手を終了させないまま resume の行を
    /// 打つと、動いているエージェントへの「指示」として入力される（#694 / #1006 と同じ罠）
    pub agent_process_found: bool,
    /// 会話の記録（transcript）が実在する（harness の**実行**に必須。#1967）。
    ///
    /// `session_resolved` は「どの会話か分かった」までで、生きた claude（`agents`）から
    /// 引いた ID は記録の実在を確かめていない。記録が無いまま終了させると
    /// resume が `No conversation found` で落ち、会話も画面も失う
    pub conversation_found: bool,
    /// エージェントのプロセスは居ないが、シェルが**入力待ち**だと確かめられた（#1967）。
    ///
    /// エージェントがすでに終わっているペイン（再起動の失敗・クラッシュの後）でも、
    /// シェルがプロンプトで待っているなら resume の行を打っても誰の入力欄にも
    /// 流れ込まない。これを認めないと「会話が戻らない」ペインを同じ操作で戻せない
    /// （本番 10/9 は zsh だけが残り、master が手で再開した）。
    /// **生成中かどうかの材料とは別物**（下の `agent_busy` の doc を参照）
    pub shell_idle: bool,
    /// このペインの建て直し（#1967 の `AgentRelaunch`）がまだ終わっていない
    pub restart_in_progress: bool,
    /// エージェントが生成中（画面の中断ヒント由来）。
    ///
    /// **OSC 133 の `Running` は使えない**（#1067 の実機実測）: エージェントは
    /// ペインのシェルが起動した前景コマンドなので、**エージェントが立っている間は
    /// ずっと `Running`** になる。これを busy と読むと、ハーネス更新の対象である
    /// エージェントペインが**常に** `busy` で断られる（実測: アイドルな worker で
    /// `reason=busy`）。「そのペインでコマンドが走っているか」は素のシェルを相手に
    /// する判定（`remote_open::can_ssh_pane`）では正しいが、ここでは意味が逆になる
    pub agent_busy: bool,
    /// キューに未送信の指示がある（#572）
    pub queued_messages: bool,
    /// 入力欄に人間の下書きがある
    pub user_draft: bool,
    /// 選択肢ダイアログを表示中（#748）
    pub dialog: bool,
}

impl Default for RestartFacts {
    fn default() -> Self {
        Self {
            has_session: false,
            is_agent: false,
            is_master: false,
            agent: Agent::Claude,
            session_resolved: false,
            agent_process_found: false,
            conversation_found: false,
            shell_idle: false,
            restart_in_progress: false,
            agent_busy: false,
            queued_messages: false,
            user_draft: false,
            dialog: false,
        }
    }
}

/// このペインでそのモードが**原理的に**可能か（構造的な条件だけを見る）。
///
/// GUI のメニュー項目の出し分けはこちらを使う（#1006 の「出た項目が断られない」は
/// 構造の話で、生成中のような一時的な状態で項目が出たり消えたりするのは別問題）。
///
/// **`session_resolved` はここでは見ない**（[`can_restart`] が見る）。理由は 2 つ:
/// 会話 ID の解決は `claude agents --json` の起動やカタログ読みを伴い**メニューの
/// 描画中に払える処理ではない**（#772 の教訓）うえ、その解決は**間欠的に失敗する**
/// （#1011 が sticky 解決を入れた理由）ので、構造の可否として扱うと項目が出たり
/// 消えたりする
pub fn is_eligible(mode: SessionRestartMode, facts: &RestartFacts) -> Result<(), RestartBlock> {
    if !facts.has_session {
        return Err(RestartBlock::NoSession);
    }
    if !facts.is_agent {
        return Err(RestartBlock::NotAgent);
    }
    if !agent_support::supports(facts.agent, mode.capability_key()) {
        return Err(RestartBlock::AgentUnsupported { agent: facts.agent });
    }
    if mode == SessionRestartMode::Handoff && !facts.is_master {
        return Err(RestartBlock::HandoffNeedsMaster);
    }
    Ok(())
}

/// 実行してよいか（構造 + 会話の解決 + 一時的な状態）。dispatch はこちらを通す
pub fn can_restart(mode: SessionRestartMode, facts: &RestartFacts) -> Result<(), RestartBlock> {
    is_eligible(mode, facts)?;
    // #1967: 建て直しの最中に重ねない（最初に見る。起動し直したばかりのエージェントを
    // また終了させると、会話を開いたまま宙ぶらりんの状態を 2 つ作る）
    if facts.restart_in_progress {
        return Err(RestartBlock::RestartInProgress);
    }
    // **画面の状態を先に見る**（順序は「失うものが大きい順」: 生成中 > キュー > 下書き）。
    // 「いま手を出すな」は待てば解ける第一の安全規則で、しかもここで断るときは
    // 会話 ID を引く必要すら無い。逆順にすると、生成中のペインに対して
    // 「`tako sessions list` で会話を確認せよ」という**その瞬間には無意味な**案内が出る
    if facts.agent_busy {
        return Err(RestartBlock::Busy);
    }
    if facts.queued_messages {
        return Err(RestartBlock::QueuedMessages);
    }
    if facts.user_draft {
        return Err(RestartBlock::UserDraft);
    }
    if facts.dialog {
        return Err(RestartBlock::Dialog);
    }
    match mode {
        // 会話 ID が引けないまま / 記録が無いまま / 相手が分からないまま終了させると会話を失う
        SessionRestartMode::Harness => {
            if !facts.session_resolved {
                return Err(RestartBlock::SessionUnresolved);
            }
            if !facts.conversation_found {
                return Err(RestartBlock::ConversationMissing);
            }
            // エージェントが居なくても、シェルが入力待ちなら打ってよい（#1967）
            if !facts.agent_process_found && !facts.shell_idle {
                return Err(RestartBlock::AgentProcessNotFound);
            }
        }
        // 引き継ぎは新しい会話を立てるので会話 ID は要らないが、**依頼を受け取って
        // 引き継ぎを書くエージェント**が居なければ何も起きない（#1967）。断るのは
        // 「エージェントが見つからず、シェルが入力待ち」と**確かめられたとき**だけ
        // （プロセスの特定は実行ファイルのパス頼みなので、見つからないだけで断ると
        // 名前の違うエージェント CLI を相手にした依頼まで止めてしまう）
        SessionRestartMode::Handoff => {
            if facts.shell_idle {
                return Err(RestartBlock::AgentProcessNotFound);
            }
        }
    }
    Ok(())
}

/// このペインで**メニューに出す**モード（構造的に可能なものだけ）。
///
/// GUI と `menu` 相当の応答が同じ関数を通るので、「画面に出ている項目」と
/// 「AI が引ける選択肢」がずれない
pub fn menu_modes(facts: &RestartFacts) -> Vec<SessionRestartMode> {
    [SessionRestartMode::Harness, SessionRestartMode::Handoff]
        .into_iter()
        .filter(|m| is_eligible(*m, facts).is_ok())
        .collect()
}

// ─────────────── 旧プロセスの終了と建て直しの段取り ───────────────
//
// ハーネス更新は「エージェントを終わらせる → 素のシェルへ resume の行を打つ」の 2 段。
// 実測（claude 2.1.258 / tmux 3.6 / #1067）:
//
// - **SIGTERM で 1 秒以内に落ち、代替画面も戻る**（`#{alternate_on}` が 1 → 0）。
//   シェルのプロンプトが画面に戻るので、#640 の送達フロー（画面のエコーを見る）が
//   そのまま噛み合う
// - 終了時に claude 自身が `Resume this session with:` と
//   `claude --resume <session-id>` を**画面へ印字する**。これは終了したまさにその
//   プロセスの会話なので、カタログ由来の推定より強い手がかりになる
// - `--resume` で会話が続くことと、**session_id が resume をまたいで変わらない**ことも実測
//
// プロセスが落ちきる前に打つと、resume の行が**動いている claude の入力欄へ**
// 流れ込む（#694 / #1006 で踏んだ「代替画面のまま書く」と同じ事故）。
// そのため「落ちたことを確かめてから打つ」を型にしてある。

/// SIGTERM から SIGKILL へ上げるまでの猶予
pub const TERMINATE_GRACE_SECS: u64 = 5;

/// 建て直しを諦めるまでの上限（相手が落ちない・落ちても打てない状況で永久に待たない）
pub const RELAUNCH_TIMEOUT_SECS: u64 = 30;

/// 旧プロセスが落ちた後、シェルのプロンプトが戻るのを待つ上限（#1967）。
///
/// シェル統合の印（OSC 133;A）が来ない環境でも永久に待たないための上限で、
/// 過ぎたら従来どおり画面の据え置きで判断する送達フロー（#640 / #1940）へ渡す
pub const SHELL_BACK_GRACE_SECS: u64 = 10;

/// 起動コマンドを送った後、エージェントが**すぐ終わらないか**を見張る時間（#1967）。
///
/// 会話が見つからない claude は起動から 1〜2 秒で `No conversation found` を出して
/// 終わる。見張らないと、建て直しは「送った」で成功扱いになり、ペインには
/// シェルだけが残って誰も気づかない（本番 10/9 の症状そのもの）
pub const LAUNCH_WATCH_SECS: u64 = 10;

/// 建て直しの試行回数の上限（最初の 1 回 + 打ち直し 1 回。#1967）
pub const MAX_RELAUNCH_ATTEMPTS: u32 = 2;

/// A/B 用の逃げ道（`TAKO_1967_LEGACY=1`）。#1967 の修正を切って旧挙動を再現する:
/// 再開コマンドはカタログのメタだけで組み（model / effort / system prompt が落ちる）、
/// 終了の案内は 1 行だけから拾い（折り返した ID を採る）、落ちたら即打ち（プロンプトの
/// 戻りを待たない）、起動後は見張らない。実測と番犬の検出力の確認に使う
pub fn legacy_1967() -> bool {
    static LEGACY: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *LEGACY.get_or_init(|| {
        matches!(
            std::env::var("TAKO_1967_LEGACY").ok().as_deref(),
            Some("1" | "true" | "on")
        )
    })
}

/// 次にやること（1 tick ぶん）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelaunchStep {
    /// まだ落ちていない / 落ちたがシェルがまだプロンプトへ戻っていない。待つ
    Wait,
    /// 猶予を過ぎたので強制終了する（SIGKILL）
    Force,
    /// 落ちてシェルが戻った。resume の行を送達フローへ積む
    Launch,
    /// 上限に達した。理由を残して諦める
    GiveUp,
}

/// 旧プロセスの終了を待つ 1 tick ぶんの判断（純関数）。
///
/// `shell_back` = 落ちた後にシェルがプロンプトへ戻ったか（[`shell_back`] で決める）。
/// 戻る前に打つと、direnv 等の起動フックの最中の先行入力になり、画面の変化を
/// 実行と取り違える（#1940 と同じ型）。`forced` = すでに SIGKILL を送ったか（二重送出を避ける）
pub fn relaunch_step(
    agent_alive: bool,
    shell_back: bool,
    elapsed_secs: u64,
    forced: bool,
) -> RelaunchStep {
    if !agent_alive {
        // 落ちていれば上限を過ぎていても打つ（会話を宙ぶらりんにしない）
        if shell_back || elapsed_secs >= RELAUNCH_TIMEOUT_SECS {
            return RelaunchStep::Launch;
        }
        return RelaunchStep::Wait;
    }
    if elapsed_secs >= RELAUNCH_TIMEOUT_SECS {
        return RelaunchStep::GiveUp;
    }
    if elapsed_secs >= TERMINATE_GRACE_SECS && !forced {
        return RelaunchStep::Force;
    }
    RelaunchStep::Wait
}

/// 落ちた後にシェルがプロンプトへ戻ったか（純関数。#1967）。
///
/// `prompts_before` = 終了要求を出す**前**に控えたプロンプトの印（OSC 133;A）の回数。
/// `None` は待つ基準が無い（このペインで印を観測していない = シェル統合が無い、
/// または最初からシェルが入力待ちでエージェントが居ない）ので、すぐ戻ったとみなす
pub fn shell_back(prompts_before: Option<u64>, prompts_now: u64, dead_for_secs: u64) -> bool {
    match prompts_before {
        None => true,
        Some(before) => prompts_now > before || dead_for_secs >= SHELL_BACK_GRACE_SECS,
    }
}

/// dispatch が host へ渡す建て直しの依頼（#1967）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelaunchRequest {
    /// 終了させたエージェントのプロセス（None = もとから居ない = シェルが入力待ち）
    pub pid: Option<u32>,
    /// 起動コマンド（`resume_launch` が組んだ。**診断ログへは出さない**）
    pub command: String,
    /// 終了要求の**前**に控えたプロンプトの印の回数（[`shell_back`] の基準）
    pub prompts_before: Option<u64>,
    /// どの組み立てを通したか（`resume_launch::ResumeRecipe` の綴り。診断ログ用）
    pub recipe: String,
}

/// 起動後の見張りの判定（#1967）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchVerdict {
    /// まだ見張っている
    Pending,
    /// 見張りの間に終わらなかった = 立ち上がった
    Started,
    /// すぐ終わってシェルへ戻った
    Exited {
        /// claude が「会話が見つからない」と言って終わった
        conversation_missing: bool,
        /// シェルが報告した終了コード（OSC 133;D。無ければ None）
        exit_code: Option<i32>,
    },
}

/// 会話が無い claude が出す文言（実測 2.1.294: `No conversation found with session ID: <id>`）
const NO_CONVERSATION_TEXT: &str = "No conversation found";

/// 画面に「会話が見つからない」が何回出ているか（折り返しに依らない数え方）。
/// 送った時点の値を控えて、増えたかで判断する（過去の失敗の行が画面に残るため）
pub fn conversation_missing_count(screen: &[String]) -> usize {
    let needle = squash(NO_CONVERSATION_TEXT);
    squash_lines(screen).matches(needle.as_str()).count()
}

/// 起動後の見張り 1 tick ぶんの判断（純関数）。
///
/// - `submit` / `now` = 起動コマンドの Enter を送った時点 / 今のシェル統合の印
/// - `missing_before` = 送った時点の [`conversation_missing_count`]
///
/// 終了の印（133;D）が増えた = エージェントが終わってシェルへ戻った。
/// 印を出さないシェルでも「会話が見つからない」の文言が増えたら終わったとみなす
pub fn launch_verdict(
    submit: crate::shell_send::ShellMarks,
    now: crate::shell_send::ShellMarks,
    screen: &[String],
    missing_before: usize,
    watched_secs: u64,
) -> LaunchVerdict {
    let missing = conversation_missing_count(screen) > missing_before;
    if now.finished > submit.finished || missing {
        return LaunchVerdict::Exited {
            conversation_missing: missing,
            exit_code: (now.finished > submit.finished)
                .then_some(now.last_exit)
                .flatten(),
        };
    }
    if watched_secs >= LAUNCH_WATCH_SECS {
        LaunchVerdict::Started
    } else {
        LaunchVerdict::Pending
    }
}

/// 建て直しが失敗した理由（#1967。綴りは `last_restart.reason` と persist.log にそのまま出る）
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RelaunchFailure {
    /// 旧プロセスが SIGKILL 後も終わらなかった
    ProcessStuck,
    /// 起動コマンドを打ち切りまでに送り届けられなかった
    CommandFlowTimeout,
    /// 起動した claude が「会話が見つからない」で終わった
    ConversationNotFound,
    /// 起動したエージェントがすぐ終わった（不正な引数・未認証等）
    AgentExited,
    /// 建て直しの途中でペインが閉じられた
    PaneClosed,
}

impl RelaunchFailure {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ProcessStuck => "process_stuck",
            Self::CommandFlowTimeout => "command_flow_timeout",
            Self::ConversationNotFound => "conversation_not_found",
            Self::AgentExited => "agent_exited",
            Self::PaneClosed => "pane_closed",
        }
    }

    /// 理由 + 次の一手（dispatch の応答・CLI / MCP。規約どおり日本語）
    pub fn message(self, pane: u64, exit_code: Option<i32>) -> String {
        let code = exit_code.map_or_else(String::new, |c| format!("（終了コード {c}）"));
        match self {
            Self::ProcessStuck => format!(
                "pane {pane} のエージェントが SIGKILL 後も終わらないので建て直しを中止した。\
                 ペインで直接終了させてから `tako session-restart --mode harness --pane {pane}` をやり直す"
            ),
            Self::CommandFlowTimeout => format!(
                "pane {pane} のシェルへ再開コマンドを送り届けられなかった（シェルが入力を受け付けない）。\
                 ペインの状態を `tako read --pane {pane}` で確かめ、\
                 `tako session-restart --mode harness --pane {pane}` をやり直す"
            ),
            Self::ConversationNotFound => format!(
                "pane {pane} で再開した claude が「会話が見つからない」で終わった{code}。\
                 `tako sessions list` で会話の記録（resumable）を確かめ、\
                 `tako sessions resume <id>` で開き直す"
            ),
            Self::AgentExited => format!(
                "pane {pane} で再開したエージェントがすぐ終わった{code}。\
                 `tako read --pane {pane}` で理由を確かめ、\
                 `tako session-restart --mode harness --pane {pane}` をやり直す"
            ),
            Self::PaneClosed => format!(
                "pane {pane} が建て直しの途中で閉じられた。\
                 会話は `tako sessions resume <id>` で別ペインへ開ける"
            ),
        }
    }
}

/// 建て直しの進み具合（#1967。`last_restart.phase`）
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RestartPhase {
    /// 旧プロセスの終了とシェルの戻りを待っている
    WaitingExit,
    /// 起動コマンドを送達フローで送っている
    Launching,
    /// 送った後、すぐ終わらないかを見張っている
    Watching,
    /// 立ち上がった（見張りの間に終わらなかった）
    Started,
    /// 失敗した（`reason` に理由）
    Failed,
}

/// ペインごとの最後の建て直しの記録（#1967。`tako session-restart` の下見に `last_restart` で出る）
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct RestartRecord {
    pub mode: SessionRestartMode,
    pub phase: RestartPhase,
    /// 失敗の理由（`phase=failed` のときだけ）
    pub reason: Option<RelaunchFailure>,
    /// すぐ終わったときの終了コード
    pub exit_code: Option<i32>,
    /// 何回送ったか（打ち直しを含む）
    pub attempts: u32,
    /// どの組み立てで起動コマンドを作ったか
    pub recipe: String,
    pub started_at: String,
    pub finished_at: Option<String>,
}

impl RestartRecord {
    /// まだ終わっていない（重ねて再起動させない = [`RestartBlock::RestartInProgress`]）
    pub fn is_active(&self) -> bool {
        matches!(
            self.phase,
            RestartPhase::WaitingExit | RestartPhase::Launching | RestartPhase::Watching
        )
    }
}

/// claude が終了時に印字する案内の見出し（実測 2.1.258）
const RESUME_HINT_HEADING: &str = "Resume this session with:";

/// 終了した claude が画面へ残した `claude --resume <session-id>` を拾う（純関数）。
///
/// カタログ / `claude agents --json` 由来の session_id は**世代がずれることがある**
/// （同一ペイン番号に複数世代が堆積する = #466 の実測）。終了直後の画面に出る案内は
/// 「いま終わったプロセスの会話」なので、食い違ったらこちらを採る。
///
/// 画面には**過去の終了ぶんも残っている**ので、必ず最後の 1 件を返す。
/// 見出しの直後だけを見る（本文中の `claude --resume ...` を拾わないため）。
///
/// # 折り返しに依らずに読む（#1967）
///
/// 細いペインでは案内が**折り返して**画面の行に分かれる（35 桁なら見出しが 1 行・
/// `claude --resume cb6d0127-b67d-44ac-b` までが 1 行）。旧実装は 1 行の中だけを読み、
/// 形の検査も「8 文字以上」しか見ていなかったので、**途中までの ID を権威として
/// 正しい ID と差し替えていた**（本番 10/9: `…-b897-065b9` で途切れた ID の resume が
/// 実行され、claude は会話が見つからずに終わった。zsh の履歴に実行の記録が残っていた）。
///
/// ここでは空白を落として行を連結し（折り返しは文字を足さず改行位置を変えるだけ）、
/// 見出しの後の `--resume` に続く **UUID の全長**だけを採る。claude の会話 ID は UUID
/// （8-4-4-4-12 の 16 進）なので、途中で切れたものは形の検査で必ず落ちる
pub fn parse_resume_hint(screen: &[String]) -> Option<String> {
    let joined = squash_lines(screen);
    let heading = squash(RESUME_HINT_HEADING);
    let mut found: Option<String> = None;
    let mut rest = joined.as_str();
    while let Some(pos) = rest.find(heading.as_str()) {
        let after = &rest[pos + heading.len()..];
        // 見出しの直後（次の見出しより前・近傍）に `--resume` があるときだけ読む
        let window_end = after
            .find(heading.as_str())
            .unwrap_or(after.len())
            .min(RESUME_HINT_WINDOW);
        let window = &after[..floor_char_boundary(after, window_end)];
        if let Some((_, tail)) = window.split_once("--resume") {
            if let Some(id) = leading_uuid(tail) {
                found = Some(id);
            }
        }
        rest = after;
    }
    found
}

/// 見出しから `--resume` を探す範囲（空白を落とした文字数）。
/// `claude --resume` の前に env の前置きやフルパスが付いても届く長さを取る
const RESUME_HINT_WINDOW: usize = 200;

/// `s` の先頭から UUID（8-4-4-4-12 の 16 進）を 1 つ読む
fn leading_uuid(s: &str) -> Option<String> {
    let cand: String = s.chars().take(36).collect();
    is_uuid(&cand).then_some(cand)
}

/// claude の会話 ID の形（UUID）か
fn is_uuid(s: &str) -> bool {
    s.len() == 36
        && s.bytes().enumerate().all(|(i, b)| match i {
            8 | 13 | 18 | 23 => b == b'-',
            _ => b.is_ascii_hexdigit(),
        })
}

/// `idx` 以下で最も近い文字境界（`str::floor_char_boundary` の安定版の代わり）
fn floor_char_boundary(s: &str, idx: usize) -> usize {
    let mut i = idx.min(s.len());
    while !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// 空白を落とした文字列（折り返しに依らない照合用。`shell_send` と同じ考え方）
fn squash(s: &str) -> String {
    s.chars().filter(|c| !c.is_whitespace()).collect()
}

/// 画面の行を空白を落として連結する
fn squash_lines(screen: &[String]) -> String {
    screen
        .iter()
        .flat_map(|l| l.chars())
        .filter(|c| !c.is_whitespace())
        .collect()
}

/// 旧実装（`TAKO_1967_LEGACY=1` の A/B 用）。**1 行の中だけ**を読み、形の検査は
/// 「8 文字以上」だけ = 折り返した途中までの ID を採る
pub fn parse_resume_hint_legacy(screen: &[String]) -> Option<String> {
    let mut found: Option<String> = None;
    for (i, line) in screen.iter().enumerate() {
        if !line.contains(RESUME_HINT_HEADING) {
            continue;
        }
        // 見出しと同じ行に続けて書かれている場合も拾う
        let same_line = line.split_once(RESUME_HINT_HEADING).map(|(_, rest)| rest);
        let candidates = [same_line.unwrap_or(""), screen.get(i + 1).map_or("", |s| s)];
        for cand in candidates {
            if let Some(id) = extract_resume_id(cand) {
                found = Some(id);
            }
        }
    }
    found
}

/// `claude --resume <id>` の `<id>` を取り出す（前後に余計な語があっても拾う）
fn extract_resume_id(line: &str) -> Option<String> {
    let rest = line.split_once("--resume")?.1;
    let token = rest.split_whitespace().next()?;
    is_session_id_shape(token).then(|| token.to_string())
}

/// claude の session_id の形（英数とハイフンのみ・十分な長さ）。
/// **`tako-control::transcript::is_valid_session_id` と同じ寛容さ**にそろえてある
/// （こちらはパス操作をしないので、ここでは長さだけを足して誤検出を防ぐ）
fn is_session_id_shape(s: &str) -> bool {
    s.len() >= 8
        && s.len() <= 128
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// 組み上がった resume コマンドから会話 ID を取り出す（差し替え検査の基準にする）。
///
/// **コマンドの形は `sessions::resume_command` が正**なので、こちらは
/// `--resume <id>` の 1 か所だけを読む
pub fn resume_id_of(command: &str) -> Option<String> {
    extract_resume_id(command)
}

/// 拾った手がかりで resume コマンドの session_id を差し替える。
///
/// 差し替えたら `true`。**コマンドの形は組み立て側（`resume_launch`）が正**なので、
/// ここでは id の文字列だけを置き換える（モデル・effort・env 前置きはそのまま残る）。
/// 呼び出し側は、差し替える前に**その ID の会話の記録が実在するか**を確かめること
/// （#1967: 画面から拾った ID は形が正しくても別の世代・別の設定 dir のものでありうる）
pub fn apply_resume_hint(command: &str, old_id: &str, hint: &str) -> (String, bool) {
    if hint == old_id || !command.contains(old_id) {
        return (command.to_string(), false);
    }
    (command.replace(old_id, hint), true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agent_pane() -> RestartFacts {
        RestartFacts {
            has_session: true,
            is_agent: true,
            is_master: false,
            agent: Agent::Claude,
            session_resolved: true,
            agent_process_found: true,
            conversation_found: true,
            ..RestartFacts::default()
        }
    }

    #[test]
    fn 語彙は往復する() {
        for v in SessionRestartMode::VALUES {
            let parsed = SessionRestartMode::parse(v).expect("VALUES は必ず解釈できる");
            assert_eq!(parsed.as_str(), v);
        }
        // 大文字・前後空白も受ける（CLI の手打ち）
        assert_eq!(
            SessionRestartMode::parse(" Harness "),
            Some(SessionRestartMode::Harness)
        );
        assert_eq!(SessionRestartMode::parse("resume"), None);
        assert_eq!(SessionRestartMode::values_hint(), "harness | handoff");
    }

    #[test]
    fn ワイヤ表記は語彙と一致する() {
        let json = serde_json::to_string(&SessionRestartMode::Handoff).unwrap();
        assert_eq!(json, "\"handoff\"");
        let parsed: SessionRestartMode = serde_json::from_str("\"harness\"").unwrap();
        assert_eq!(parsed, SessionRestartMode::Harness);
    }

    #[test]
    fn claudeのエージェントペインはハーネス更新できる() {
        assert_eq!(
            can_restart(SessionRestartMode::Harness, &agent_pane()),
            Ok(())
        );
        // 生成中の判断材料は画面の中断ヒントだけ（下の番犬が OSC 133 の復活を止める）
        let generating = RestartFacts {
            agent_busy: true,
            ..agent_pane()
        };
        assert_eq!(
            can_restart(SessionRestartMode::Harness, &generating),
            Err(RestartBlock::Busy)
        );
    }

    #[test]
    fn 構造的に無理なペインは理由つきで断る() {
        let preview = RestartFacts {
            has_session: false,
            ..agent_pane()
        };
        assert_eq!(
            can_restart(SessionRestartMode::Harness, &preview),
            Err(RestartBlock::NoSession)
        );
        let plain = RestartFacts {
            is_agent: false,
            ..agent_pane()
        };
        assert_eq!(
            can_restart(SessionRestartMode::Harness, &plain),
            Err(RestartBlock::NotAgent)
        );
        let unresolved = RestartFacts {
            session_resolved: false,
            ..agent_pane()
        };
        assert_eq!(
            can_restart(SessionRestartMode::Harness, &unresolved),
            Err(RestartBlock::SessionUnresolved)
        );
        // 会話の解決は**メニューの出し分けには使わない**（I/O が要る + 間欠的に失敗する）
        assert_eq!(
            is_eligible(SessionRestartMode::Harness, &unresolved),
            Ok(())
        );
    }

    /// claude 以外の系統は**マトリクス（#982）が Pending なら**断る。
    /// この判定を `if agent == claude` で散らさないのが #982 の規約
    #[test]
    fn 対応していない系統はマトリクス由来で断る() {
        for agent in [Agent::Codex, Agent::Agy, Agent::Local] {
            let facts = RestartFacts {
                agent,
                ..agent_pane()
            };
            assert_eq!(
                can_restart(SessionRestartMode::Harness, &facts),
                Err(RestartBlock::AgentUnsupported { agent }),
                "{} はまだ resume を配線していない",
                agent.as_str()
            );
        }
    }

    #[test]
    fn 引き継ぎ再起動はmasterだけ() {
        let worker = agent_pane();
        assert_eq!(
            can_restart(SessionRestartMode::Handoff, &worker),
            Err(RestartBlock::HandoffNeedsMaster)
        );
        let master = RestartFacts {
            is_master: true,
            ..agent_pane()
        };
        assert_eq!(can_restart(SessionRestartMode::Handoff, &master), Ok(()));
        // 引き継ぎは会話の解決を要らない（新しい会話を立てるので resume 先が無くてよい）
        let master_no_session = RestartFacts {
            is_master: true,
            session_resolved: false,
            ..agent_pane()
        };
        assert_eq!(
            can_restart(SessionRestartMode::Handoff, &master_no_session),
            Ok(())
        );
    }

    #[test]
    fn 一時的な状態は実行時に断るがメニューからは消えない() {
        let cases = [
            (
                RestartFacts {
                    agent_busy: true,
                    ..agent_pane()
                },
                RestartBlock::Busy,
            ),
            (
                RestartFacts {
                    queued_messages: true,
                    ..agent_pane()
                },
                RestartBlock::QueuedMessages,
            ),
            (
                RestartFacts {
                    user_draft: true,
                    ..agent_pane()
                },
                RestartBlock::UserDraft,
            ),
            (
                RestartFacts {
                    dialog: true,
                    ..agent_pane()
                },
                RestartBlock::Dialog,
            ),
        ];
        for (facts, want) in cases {
            assert_eq!(
                can_restart(SessionRestartMode::Harness, &facts),
                Err(want),
                "実行時には断る: {want:?}"
            );
            assert!(want.is_transient(), "{want:?} は一時的な理由");
            assert_eq!(
                is_eligible(SessionRestartMode::Harness, &facts),
                Ok(()),
                "メニューからは消さない（機能を見つけられなくなる）: {want:?}"
            );
            assert_eq!(
                menu_modes(&facts),
                vec![SessionRestartMode::Harness],
                "一時的な理由でメニューの項目数が揺れない: {want:?}"
            );
        }
    }

    /// 関門の順序は固定する（#1067）。生成中のペインに「会話を確認せよ」を出さない
    /// **番犬**（#1067 の実機実測）: エージェントは「ペインのシェルが起動した前景
    /// コマンド」なので、立っている間ペインの OSC 133 は**ずっと `Running`**。
    /// これを busy の材料に戻すと、ハーネス更新の対象であるエージェントペインが
    /// **常に** `busy` で断られる（実測: アイドルな worker で `reason=busy`）。
    /// 判断材料を増やすときはこの理由を読んでから増やすこと
    #[test]
    fn osc133のコマンド状態を判断材料に戻していない() {
        let src = include_str!("session_restart.rs");
        // テストモジュールより前（= 実装本体）だけを見る
        let body = src.split("#[cfg(test)]").next().unwrap_or(src);
        assert!(
            !body.contains("CommandState"),
            "OSC 133 のコマンド状態を判断材料へ戻してはいけない（理由はこのテストの doc）"
        );
    }

    #[test]
    fn 画面の状態を会話の解決より先に見る() {
        let busy_unresolved = RestartFacts {
            agent_busy: true,
            session_resolved: false,
            agent_process_found: false,
            ..agent_pane()
        };
        assert_eq!(
            can_restart(SessionRestartMode::Harness, &busy_unresolved),
            Err(RestartBlock::Busy),
            "待てば解ける理由を先に返す"
        );
        // 画面が静かになれば会話の解決の話になる
        let idle_unresolved = RestartFacts {
            agent_busy: false,
            ..busy_unresolved
        };
        assert_eq!(
            can_restart(SessionRestartMode::Harness, &idle_unresolved),
            Err(RestartBlock::SessionUnresolved)
        );
    }

    #[test]
    fn メニューは構造的に可能なものだけ並べる() {
        // 素のシェル: 何も出ない
        assert!(menu_modes(&RestartFacts {
            has_session: true,
            ..RestartFacts::default()
        })
        .is_empty());
        // worker: ハーネス更新だけ
        assert_eq!(menu_modes(&agent_pane()), vec![SessionRestartMode::Harness]);
        // master: 両方
        let master = RestartFacts {
            is_master: true,
            ..agent_pane()
        };
        assert_eq!(
            menu_modes(&master),
            vec![SessionRestartMode::Harness, SessionRestartMode::Handoff]
        );
        // 会話が解決できない master でもメニューは変わらない（実行時に理由が出る）
        let master_unresolved = RestartFacts {
            session_resolved: false,
            ..master
        };
        assert_eq!(
            menu_modes(&master_unresolved),
            vec![SessionRestartMode::Harness, SessionRestartMode::Handoff]
        );
    }

    #[test]
    fn 断る理由には次の一手が入る() {
        let blocks = [
            RestartBlock::NoSession,
            RestartBlock::NotAgent,
            RestartBlock::AgentUnsupported {
                agent: Agent::Codex,
            },
            RestartBlock::SessionUnresolved,
            RestartBlock::AgentProcessNotFound,
            RestartBlock::HandoffNeedsMaster,
            RestartBlock::Busy,
            RestartBlock::QueuedMessages,
            RestartBlock::UserDraft,
            RestartBlock::Dialog,
            RestartBlock::ConversationMissing,
            RestartBlock::RestartInProgress,
        ];
        for block in blocks {
            for mode in [SessionRestartMode::Harness, SessionRestartMode::Handoff] {
                let msg = block.message(7, mode);
                assert!(msg.contains("pane 7"), "対象ペインを名指しする: {msg}");
                // 「どうすれば通るか」を必ず添える（コマンドか、待つ / 別モードの案内）
                let has_next_step = msg.contains("tako ")
                    || msg.contains("もう一度")
                    || msg.contains("mode=harness")
                    || msg.contains("指定する");
                assert!(has_next_step, "次の一手を添える: {msg}");
            }
            assert!(!block.as_str().is_empty());
        }
    }

    #[test]
    fn 落ちるまで待ち猶予を過ぎたら強制終了する() {
        assert_eq!(relaunch_step(true, false, 0, false), RelaunchStep::Wait);
        assert_eq!(
            relaunch_step(true, false, TERMINATE_GRACE_SECS - 1, false),
            RelaunchStep::Wait
        );
        assert_eq!(
            relaunch_step(true, false, TERMINATE_GRACE_SECS, false),
            RelaunchStep::Force
        );
        // 二重に SIGKILL は撃たない
        assert_eq!(
            relaunch_step(true, false, TERMINATE_GRACE_SECS, true),
            RelaunchStep::Wait
        );
        assert_eq!(
            relaunch_step(true, false, RELAUNCH_TIMEOUT_SECS, true),
            RelaunchStep::GiveUp
        );
        // 落ちていれば上限を過ぎていても打つ（会話を宙ぶらりんにしない）
        assert_eq!(
            relaunch_step(false, false, RELAUNCH_TIMEOUT_SECS + 10, true),
            RelaunchStep::Launch
        );
        assert_eq!(relaunch_step(false, true, 0, false), RelaunchStep::Launch);
    }

    /// #1967: 落ちてもシェルがプロンプトへ戻るまでは打たない（direnv 等の起動フックの
    /// 最中に書くと先行入力になり、画面の変化を実行と取り違える = #1940 と同じ型）
    #[test]
    fn issue1967_落ちてもシェルが戻るまでは打たない() {
        assert_eq!(relaunch_step(false, false, 1, false), RelaunchStep::Wait);
        assert_eq!(relaunch_step(false, true, 1, false), RelaunchStep::Launch);
        // 戻りの判定: 印が増えた / 基準が無い / 猶予を過ぎた
        assert!(!shell_back(Some(3), 3, 0));
        assert!(shell_back(Some(3), 4, 0));
        assert!(
            shell_back(None, 0, 0),
            "基準が無い = 印を観測していないペイン"
        );
        assert!(shell_back(Some(3), 3, SHELL_BACK_GRACE_SECS));
        assert!(!shell_back(Some(3), 3, SHELL_BACK_GRACE_SECS - 1));
    }

    /// 実測の画面（claude 2.1.258 が SIGTERM で終了した直後。#1067）。
    /// **過去の終了ぶんも残っている**ので最後の 1 件を採る
    #[test]
    fn 終了時の案内から会話idを拾う() {
        let screen: Vec<String> = [
            "direnv: unloading",
            "[testuser@host:/tmp]$ claude",
            "",
            "Resume this session with:",
            "claude --resume 0e7ec5d5-80e8-4070-9968-99b111391068",
            "[testuser@host:/tmp]$ claude --resume 0e7ec5d5-80e8-4070-9968-99b111391068",
            "",
            "Resume this session with:",
            "claude --resume 11112222-3333-4444-5555-666677778888",
            "[testuser@host:/tmp]$",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        assert_eq!(
            parse_resume_hint(&screen).as_deref(),
            Some("11112222-3333-4444-5555-666677778888"),
            "最後に印字された案内を採る"
        );
    }

    #[test]
    fn 案内が無い画面からは拾わない() {
        let screen: Vec<String> = ["$ ls", "Cargo.toml  src", "$ "]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(parse_resume_hint(&screen), None);
        // 会話の本文に出てくる `--resume` は見出しが無いので拾わない
        let chat: Vec<String> = ["⏺ claude --resume abcdefgh を実行してください"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(parse_resume_hint(&chat), None);
        // 見出しはあるが id の形でないもの
        let broken: Vec<String> = ["Resume this session with:", "claude --resume <id>"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(parse_resume_hint(&broken), None);
    }

    #[test]
    fn resumeコマンドから会話idを読む() {
        assert_eq!(
            resume_id_of("claude --model opus --resume abcd-1234 ").as_deref(),
            Some("abcd-1234")
        );
        // 会話 ID を持たないコマンドからは読まない
        assert_eq!(resume_id_of("claude --model opus"), None);
        assert_eq!(resume_id_of(""), None);
    }

    #[test]
    fn 手がかりで会話idを差し替える() {
        let cmd = "unset CLAUDE_CONFIG_DIR; TAKO_ORCHESTRATOR_ROLE=worker:tako \
                   claude --model opus --effort high --resume old-1234-id";
        let (out, changed) = apply_resume_hint(cmd, "old-1234-id", "new-5678-id");
        assert!(changed);
        assert!(out.contains("--resume new-5678-id"), "{out}");
        // モデル・effort・env 前置きは残る（コマンドの形は組み立て側が正）
        assert!(out.contains("--model opus --effort high"), "{out}");
        assert!(out.starts_with("unset CLAUDE_CONFIG_DIR;"), "{out}");
        // 同じ id なら触らない
        let (same, changed) = apply_resume_hint(cmd, "old-1234-id", "old-1234-id");
        assert!(!changed);
        assert_eq!(same, cmd);
        // 元 id がコマンドに無ければ触らない（取り違えて別の語を壊さない）
        let (kept, changed) = apply_resume_hint(cmd, "absent-id", "new-5678-id");
        assert!(!changed);
        assert_eq!(kept, cmd);
    }

    /// #1967 の本番の画面（35 桁）を写したもの。案内の ID が**折り返して** 2 行に分かれる
    fn wrapped_hint_screen(cols: usize) -> Vec<String> {
        let id = "cb6d0127-b67d-44ac-b897-065b9f0678f0";
        let mut lines: Vec<String> = vec!["[testuser@host:minige]$".into(), String::new()];
        for text in [
            "Resume this session with:",
            &format!("claude --resume {id}"),
        ] {
            let chars: Vec<char> = text.chars().collect();
            for chunk in chars.chunks(cols) {
                lines.push(chunk.iter().collect());
            }
        }
        lines.push("[testuser@host:minige]$".into());
        lines
    }

    /// #1967 の真因（②）: 折り返した案内の 1 行目だけを読むと、途中までの ID を
    /// 「権威ある手がかり」として正しい ID と差し替えていた
    #[test]
    fn issue1967_折り返した案内から途中までのidを拾わない() {
        let full = "cb6d0127-b67d-44ac-b897-065b9f0678f0";
        for cols in [16, 20, 35, 45, 60, 80, 200] {
            let screen = wrapped_hint_screen(cols);
            assert_eq!(
                parse_resume_hint(&screen).as_deref(),
                Some(full),
                "{cols} 桁でも全長の ID を読む: {screen:?}"
            );
        }
        // 旧実装は 45 桁で途中までの ID（本番の zsh 履歴に残っていた `…065b9`）を採る
        assert_eq!(
            parse_resume_hint_legacy(&wrapped_hint_screen(45)).as_deref(),
            Some("cb6d0127-b67d-44ac-b897-065b9"),
            "対照: 旧実装が本番の途切れた ID を作る"
        );
        // 途中で画面が切れて ID の後半が無い = 形の検査で落ちる（推測で補わない）
        let mut cut = wrapped_hint_screen(35);
        cut.truncate(cut.len() - 2);
        assert_eq!(parse_resume_hint(&cut), None, "{cut:?}");
    }

    #[test]
    fn issue1967_会話idはuuidの形だけを採る() {
        assert!(is_uuid("0e7ec5d5-80e8-4070-9968-99b111391068"));
        assert!(
            !is_uuid("0e7ec5d5-80e8-4070-9968-99b11139106"),
            "1 文字足りない"
        );
        assert!(!is_uuid("0e7ec5d5_80e8-4070-9968-99b111391068"));
        assert!(!is_uuid("zzzzzzzz-80e8-4070-9968-99b111391068"));
        // 見出しの近くに無い `--resume` は読まない（会話の本文の引用を拾わない）
        let far: Vec<String> = vec![
            "Resume this session with:".into(),
            "x".repeat(RESUME_HINT_WINDOW + 10),
            "claude --resume 0e7ec5d5-80e8-4070-9968-99b111391068".into(),
        ];
        assert_eq!(parse_resume_hint(&far), None);
    }

    #[test]
    fn issue1967_起動後の見張り() {
        use crate::shell_send::ShellMarks;
        let submit = ShellMarks {
            prompts: 5,
            executed: 5,
            finished: 4,
            last_exit: Some(0),
        };
        let quiet: Vec<String> = vec!["claude の画面".into()];
        assert_eq!(
            launch_verdict(submit, submit, &quiet, 0, 1),
            LaunchVerdict::Pending
        );
        assert_eq!(
            launch_verdict(submit, submit, &quiet, 0, LAUNCH_WATCH_SECS),
            LaunchVerdict::Started
        );
        // 終了の印が増えた = すぐ終わった（終了コードつき）
        let exited = ShellMarks {
            finished: 5,
            last_exit: Some(1),
            ..submit
        };
        assert_eq!(
            launch_verdict(submit, exited, &quiet, 0, 2),
            LaunchVerdict::Exited {
                conversation_missing: false,
                exit_code: Some(1)
            }
        );
        // 会話が無い文言（折り返していても数える）。印の無いシェルでも決着する
        let missing: Vec<String> = vec![
            "No conversation f".into(),
            "ound with session ID: cb6d0127".into(),
        ];
        assert_eq!(conversation_missing_count(&missing), 1);
        assert_eq!(
            launch_verdict(submit, submit, &missing, 0, 2),
            LaunchVerdict::Exited {
                conversation_missing: true,
                exit_code: None
            }
        );
        // 送った時点で既に画面に残っていた失敗の行は数えない
        assert_eq!(
            launch_verdict(submit, submit, &missing, 1, 2),
            LaunchVerdict::Pending
        );
    }

    #[test]
    fn issue1967_記録の無い会話と重ねた再起動は断る() {
        let missing = RestartFacts {
            conversation_found: false,
            ..agent_pane()
        };
        assert_eq!(
            can_restart(SessionRestartMode::Harness, &missing),
            Err(RestartBlock::ConversationMissing)
        );
        let running = RestartFacts {
            restart_in_progress: true,
            is_master: true,
            ..agent_pane()
        };
        for mode in [SessionRestartMode::Harness, SessionRestartMode::Handoff] {
            assert_eq!(
                can_restart(mode, &running),
                Err(RestartBlock::RestartInProgress)
            );
        }
        // どちらもメニューからは消さない（一時的）
        assert_eq!(menu_modes(&missing), vec![SessionRestartMode::Harness]);
    }

    /// #1967: エージェントが既に終わってシェルだけが残ったペイン（本番 10/9 の終状態）は、
    /// シェルが入力待ちなら**同じ操作で**会話を戻せる。引き継ぎは依頼を受け取る相手が要る
    #[test]
    fn issue1967_シェルだけが残ったペインはハーネス更新で戻せる() {
        let shell_only = RestartFacts {
            is_master: true,
            agent_process_found: false,
            shell_idle: true,
            ..agent_pane()
        };
        assert_eq!(
            can_restart(SessionRestartMode::Harness, &shell_only),
            Ok(())
        );
        assert_eq!(
            can_restart(SessionRestartMode::Handoff, &shell_only),
            Err(RestartBlock::AgentProcessNotFound)
        );
        let msg = RestartBlock::AgentProcessNotFound.message(9, SessionRestartMode::Handoff);
        assert!(msg.contains("--mode harness"), "次の一手は harness: {msg}");
        // 入力待ちとも確かめられないなら従来どおり断る（打った行が誰かの入力欄へ入る）
        let unknown = RestartFacts {
            shell_idle: false,
            ..shell_only
        };
        assert_eq!(
            can_restart(SessionRestartMode::Harness, &unknown),
            Err(RestartBlock::AgentProcessNotFound)
        );
        // 引き継ぎは「シェルが入力待ち」と確かめられたときだけ断る（何かが前景で動いて
        // いるなら依頼を積む = 名前の違うエージェント CLI でも止めない。従来どおり）
        assert_eq!(can_restart(SessionRestartMode::Handoff, &unknown), Ok(()));
    }

    #[test]
    fn issue1967_失敗の理由には次の一手が入る() {
        for f in [
            RelaunchFailure::ProcessStuck,
            RelaunchFailure::CommandFlowTimeout,
            RelaunchFailure::ConversationNotFound,
            RelaunchFailure::AgentExited,
            RelaunchFailure::PaneClosed,
        ] {
            let msg = f.message(4, Some(1));
            assert!(msg.contains("pane 4"), "{msg}");
            assert!(msg.contains("tako "), "次の一手: {msg}");
            assert_eq!(
                serde_json::to_value(f).unwrap(),
                serde_json::json!(f.as_str()),
                "ワイヤ表記は as_str と一致"
            );
        }
        let record = RestartRecord {
            mode: SessionRestartMode::Harness,
            phase: RestartPhase::Watching,
            reason: None,
            exit_code: None,
            attempts: 1,
            recipe: "master_profile".into(),
            started_at: "t".into(),
            finished_at: None,
        };
        assert!(record.is_active());
        assert!(!RestartRecord {
            phase: RestartPhase::Failed,
            ..record.clone()
        }
        .is_active());
    }
}
